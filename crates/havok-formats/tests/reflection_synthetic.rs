//! Hand-made class tables: the signature recipe (its expected values worked out from the byte strings below, with an
//! independent CRC-32), the layout rules at two platforms, and the refusals of a table that cannot be laid out. No game
//! data.
#![allow(
    clippy::unwrap_used,
    reason = "a fixture that does not build is the test's failure"
)]
use havok_formats::packfile::Rules;
use havok_formats::packfile::reflection::{
    ALIGN_16, Class, ClassTable, Enum, EnumItem, Member, MemberType, SERIALIZE_IGNORED,
};

fn member(name: &str, kind: MemberType) -> Member {
    Member {
        name: name.into(),
        class: None,
        enumeration: None,
        kind,
        subtype: MemberType(0),
        array_size: 0,
        flags: 0,
        offset: None,
    }
}

fn class(name: &str, parent: Option<&str>, members: Vec<Member>) -> Class {
    Class {
        name: name.into(),
        parent: parent.map(Into::into),
        size: None,
        interfaces: 0,
        enums: Vec::new(),
        members,
        flags: 0,
        version: 0,
        signature: None,
    }
}

/// `A`: one `real x`. `B : A` implements one interface, declares the enum `E { a = 1, b = 2 }` and has an enum member
/// `e` (stored in a `uint8`), a serialize-ignored pointer to a struct `p`, and a struct member `s` of class `A`.
fn table() -> ClassTable {
    let e = Enum {
        name: "E".into(),
        items: vec![
            EnumItem {
                name: "a".into(),
                value: 1,
            },
            EnumItem {
                name: "b".into(),
                value: 2,
            },
        ],
        flags: 0,
    };
    let mut enum_member = member("e", MemberType::ENUM);
    enum_member.subtype = MemberType::UINT8;
    enum_member.enumeration = Some(e.clone());
    let mut pointer = member("p", MemberType::POINTER);
    pointer.subtype = MemberType::STRUCT;
    pointer.class = Some("A".into());
    pointer.flags = SERIALIZE_IGNORED;
    let mut inner = member("s", MemberType::STRUCT);
    inner.class = Some("A".into());
    let mut b = class("B", Some("A"), vec![enum_member, pointer, inner]);
    b.interfaces = 1;
    b.enums = vec![e];
    ClassTable {
        classes: vec![class("A", None, vec![member("x", MemberType::REAL)]), b],
    }
}

#[test]
fn a_signature_is_the_crc_of_the_description() {
    let t = table();
    // CRC-32 of: i32 0, i32 0, "x", u16 11, u16 0, u16 0, u16 0, i32 1 (all little endian)
    assert_eq!(t.signature("A").unwrap(), 0x68DD_A909);
    // the description of B (interface count, its enum, the member records with the enum and struct class written in
    // front, the ignored pointer as the zero type with its old type as subtype and its old subtype as an extra word),
    // then A's
    assert_eq!(t.signature("B").unwrap(), 0xB6DA_E5B5);
}

#[test]
fn a_change_of_any_part_changes_the_signature() {
    let base = table();
    let want = base.signature("B").unwrap();
    let edits: [fn(&mut ClassTable); 8] = [
        |t| t.classes[1].interfaces = 0,
        |t| t.classes[1].enums[0].items[1].value = 3,
        |t| t.classes[1].members[0].enumeration = None,
        |t| t.classes[1].members[1].subtype = MemberType::STRING_PTR,
        |t| t.classes[1].members[1].flags = 0,
        |t| t.classes[1].members[2].class = Some("B".into()),
        |t| t.classes[0].members[0].name = "y".into(),
        |t| t.classes[0].members[0].array_size = 2,
    ];
    for (i, edit) in edits.iter().enumerate() {
        let mut t = base.clone();
        edit(&mut t);
        // `B` as a struct of itself is a cycle a signature refuses; the rest change it
        match t.signature("B") {
            Ok(got) => assert_ne!(got, want, "edit {i}"),
            Err(e) => assert!(e.detail.contains("without bound"), "edit {i}: {e}"),
        }
    }
}

#[test]
fn layout_follows_the_rules_of_each_platform() {
    let t = table();
    // 32-bit: A.x at 0; B starts at A's 4 bytes, an interface takes a 4-byte table pointer, then e (1), p (4), s (4)
    let a = t.layout("A", Rules::POWERPC_32).unwrap();
    assert_eq!((a.size, a.alignment, a.offsets.clone()), (4, 4, vec![0]));
    let b = t.layout("B", Rules::POWERPC_32).unwrap();
    assert_eq!((b.size, b.offsets.clone()), (20, vec![8, 12, 16]));
    // 64-bit: the table pointer and the pointer are 8 bytes
    let b = t.layout("B", Rules::WINDOWS_64).unwrap();
    assert_eq!(
        (b.size, b.alignment, b.offsets.clone()),
        (40, 8, vec![16, 24, 32])
    );
    assert_eq!(t.member_offset("B", &["s", "x"], Rules::POWERPC_32).unwrap(), 16);
    assert_eq!(t.member_offset("B", &["x"], Rules::POWERPC_32).unwrap(), 0);
    assert!(t.member_offset("B", &["s", "z"], Rules::POWERPC_32).is_err());
    assert!(t.member_offset("B", &["e", "x"], Rules::POWERPC_32).is_err());
    assert!(t.member_offset("B", &[], Rules::POWERPC_32).is_err());
}

/// A derived class's first member goes in its parent's tail padding when the rules say so, and after the parent's size
/// when they do not.
#[test]
fn padding_is_reused_only_when_the_rules_say_so() {
    let mut a = class(
        "P",
        None,
        vec![member("a", MemberType::INT32), member("b", MemberType::INT8)],
    );
    a.interfaces = 0;
    let q = class("Q", Some("P"), vec![member("c", MemberType::INT8)]);
    let t = ClassTable { classes: vec![a, q] };
    let reuse = Rules {
        reuse_padding: true,
        ..Rules::WINDOWS_64
    };
    assert_eq!(t.layout("P", reuse).unwrap().size, 8);
    assert_eq!(t.layout("Q", reuse).unwrap().offsets, [5]);
    assert_eq!(t.layout("Q", Rules::WINDOWS_64).unwrap().offsets, [8]);
}

#[test]
fn flags_and_arrays_change_alignment_and_size() {
    let mut aligned = member("v", MemberType::REAL);
    aligned.flags = ALIGN_16;
    let mut array = member("w", MemberType::UINT16);
    array.array_size = 3;
    let c = class("C", None, vec![member("b", MemberType::BOOL), aligned, array]);
    let t = ClassTable { classes: vec![c] };
    let l = t.layout("C", Rules::POWERPC_32).unwrap();
    // b at 0, v 16-aligned at 16 (4 bytes), w (3 x 2) at 20; the class takes the 16-byte alignment: 32
    assert_eq!((l.offsets, l.size, l.alignment), (vec![0, 16, 20], 32, 16));
}

#[test]
fn a_table_that_cannot_be_laid_out_or_signed_is_refused() {
    let t = table();
    assert!(
        t.layout("missing", Rules::POWERPC_32)
            .unwrap_err()
            .detail
            .contains("no class missing")
    );
    assert!(t.signature("missing").is_err());
    // a type with no layout in the data
    let c = class("C", None, vec![member("f", MemberType(29))]);
    let t = ClassTable { classes: vec![c] };
    assert!(
        t.layout("C", Rules::POWERPC_32)
            .unwrap_err()
            .detail
            .contains("type 29 has no layout")
    );
    // a class that is its own parent, and one that holds itself
    let loop_parent = ClassTable {
        classes: vec![class("L", Some("L"), vec![])],
    };
    assert!(loop_parent.signature("L").is_err());
    assert!(loop_parent.layout("L", Rules::POWERPC_32).is_err());
    let mut s = member("me", MemberType::STRUCT);
    s.class = Some("S".into());
    let holds = ClassTable {
        classes: vec![class("S", None, vec![s])],
    };
    assert!(holds.signature("S").is_err());
    assert!(holds.layout("S", Rules::POWERPC_32).is_err());
    // a struct member without a class, and an enum stored in a type that is not an integer
    let c = class("D", None, vec![member("s", MemberType::STRUCT)]);
    assert!(
        ClassTable { classes: vec![c] }
            .layout("D", Rules::POWERPC_32)
            .is_err()
    );
    let mut e = member("e", MemberType::ENUM);
    e.subtype = MemberType::REAL;
    let c = class("E", None, vec![e]);
    assert!(
        ClassTable { classes: vec![c] }
            .layout("E", Rules::POWERPC_32)
            .is_err()
    );
}

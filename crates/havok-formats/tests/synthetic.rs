//! Hand-authored tagfile records exercise serialized references independently
//! of the production parser, including the reflected width of serialized ITEM references.
use havok_formats::Tagfile;
fn section(name: &[u8; 4], leaf: bool, bytes: Vec<u8>) -> Vec<u8> {
    let mut out = (((bytes.len() + 8) as u32) | if leaf { 0x40000000 } else { 0 })
        .to_be_bytes()
        .to_vec();
    out.extend(name);
    out.extend(bytes);
    out
}
fn packed(value: u32) -> Vec<u8> {
    if value < 128 {
        vec![value as u8]
    } else if value < 16384 {
        ((value as u16) | 0x8000).to_be_bytes().to_vec()
    } else {
        vec![0xc0 | ((value >> 16) as u8), (value >> 8) as u8, value as u8]
    }
}
/// A PTCH record: a type index and the DATA offsets of its stored references.
type Patch<'a> = (u32, &'a [u32]);
fn fixture() -> Vec<u8> {
    fixture_with(b"20160200", packed(3))
}
/// The fixture with another SDK version and Root's packed member counts (fields in the low 16 bits, reflection
/// properties above).
fn fixture_with(sdk: &[u8; 8], root_counts: Vec<u8>) -> Vec<u8> {
    fixture_patched(sdk, root_counts, &[])
}
/// The fixture with a PTCH section of `patches` (type index, DATA offsets of the stored references), as given.
fn fixture_patched(sdk: &[u8; 8], root_counts: Vec<u8>, patches: &[Patch]) -> Vec<u8> {
    let strings = b"Root\0uint\0Pointer\0Array\0".to_vec();
    let names = vec![5, 0, 0, 1, 0, 2, 0, 3, 0];
    // Root{u32 value @0; pointer @8; array @16}, uint, ptr<uint>, array<ptr>.
    let mut body = vec![1, 0, 41, 7, 32, 8];
    body.extend(root_counts);
    body.extend([0, 32, 0, 2, 1, 32, 8, 3, 2, 32, 16, 4]);
    body.extend([2, 0, 9]);
    body.extend(packed(0x8004));
    body.extend([4, 4]);
    body.extend([3, 0, 11, 6, 2, 8, 8]);
    body.extend([4, 0, 11, 8, 3, 16, 8]);
    let mut types = section(b"TSTR", true, strings);
    types.extend(section(b"TNA1", true, names));
    types.extend(section(b"FSTR", true, b"value\0pointer\0array\0".to_vec()));
    types.extend(section(b"TBDY", true, body));
    let mut data = vec![0u8; 56];
    data[0..4].copy_from_slice(&17u32.to_le_bytes());
    data[8..12].copy_from_slice(&2u32.to_le_bytes());
    data[16..20].copy_from_slice(&3u32.to_le_bytes());
    data[32..36].copy_from_slice(&91u32.to_le_bytes());
    data[40..48].copy_from_slice(&2u64.to_le_bytes());
    data[48..56].copy_from_slice(&2u64.to_le_bytes());
    let mut items = vec![0u8; 12];
    for (kind, offset, count) in [
        (0x10000001u32, 0u32, 1u32),
        (0x10000002, 32, 1),
        (0x20000003, 40, 2),
    ] {
        items.extend(kind.to_le_bytes());
        items.extend(offset.to_le_bytes());
        items.extend(count.to_le_bytes());
    }
    let mut root = section(b"SDKV", true, sdk.to_vec());
    root.extend(section(b"DATA", true, data));
    root.extend(section(b"TYPE", false, types));
    let mut index = section(b"ITEM", true, items);
    if !patches.is_empty() {
        let mut table = Vec::new();
        for (type_index, offsets) in patches {
            table.extend(type_index.to_le_bytes());
            table.extend(u32::try_from(offsets.len()).expect("a few offsets").to_le_bytes());
            for offset in *offsets {
                table.extend(offset.to_le_bytes());
            }
        }
        index.extend(section(b"PTCH", true, table));
    }
    root.extend(section(b"INDX", false, index));
    section(b"TAG0", false, root)
}
#[test]
fn pointer_arrays_keep_the_reflected_eight_byte_stride() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let root = tag.root().expect("root");
    assert_eq!(root.field("value").expect("value").unsigned().expect("uint"), 17);
    assert_eq!(
        root.field("pointer")
            .expect("pointer")
            .dereference()
            .expect("target")
            .unsigned()
            .expect("uint"),
        91
    );
    let array = root.field("array").expect("array").array().expect("array view");
    assert_eq!(array.len(), 2);
    for i in 0..2 {
        assert_eq!(
            array
                .get(i)
                .expect("entry")
                .dereference()
                .expect("target")
                .unsigned()
                .expect("uint"),
            91
        );
    }
    assert!(array.get(2).is_err());
    assert!(root.field("absent").is_err());
}
#[test]
fn all_truncations_are_errors() {
    let bytes = fixture();
    for n in 0..bytes.len() {
        assert!(Tagfile::read(&bytes[..n], None).is_err(), "prefix {n}");
    }
}
#[test]
fn oversized_item_and_bad_reference_fail() {
    let mut bytes = fixture();
    let last = bytes.len();
    bytes[last - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Tagfile::read(&bytes, None).is_err());
    let mut bytes = fixture();
    let data = bytes.windows(4).position(|b| b == b"DATA").expect("DATA") + 4;
    bytes[data + 8..data + 12].copy_from_slice(&99u32.to_le_bytes());
    let tag = Tagfile::read(&bytes, None).expect("container");
    assert!(
        tag.root()
            .expect("root")
            .field("pointer")
            .expect("field")
            .dereference()
            .is_err()
    );
}
#[test]
fn sdk_2018_member_counts_carry_property_counts() {
    let bytes = fixture_with(b"20180100", packed(0x0002_0003));
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    assert_eq!(&tag.version, b"20180100");
    let root = tag.root().expect("root");
    assert_eq!(root.field("value").expect("value").unsigned().expect("uint"), 17);
    assert_eq!(tag.types[1].fields.len(), 3);
    assert_eq!(tag.types[1].properties, 2);
    let old = fixture();
    let tag = Tagfile::read(&old, None).expect("fixture");
    assert_eq!(tag.types[1].properties, 0);
    assert!(Tagfile::read(&fixture_with(b"20170100", packed(3)), None).is_err());
}

/// A string index equal to the table's length is out of bounds, not the empty string the split after the last NUL would
/// make of it.
#[test]
fn a_string_index_at_the_end_of_the_table_is_an_error() {
    let mut bytes = fixture();
    assert!(Tagfile::read(&bytes, None).is_ok());
    // the type names section: 5 types (the null one counted), each a string index and no template parameters; the last
    // type's name is string 3 of the 4 the table holds: 4 names no string
    let names = [5u8, 0, 0, 1, 0, 2, 0, 3, 0];
    let at = bytes
        .windows(names.len())
        .position(|w| w == names)
        .expect("the names section");
    bytes[at + 7] = 4;
    let error = Tagfile::read(&bytes, None).unwrap_err().to_string();
    assert!(error.contains("string index out of bounds"), "{error}");
}

/// The file offset of the first section called `name`: its header, four bytes before the signature.
fn section_offset(bytes: &[u8], name: &[u8; 4]) -> usize {
    bytes.windows(4).position(|w| w == name).expect("section") - 4
}

/// An item index the table lacks is refused at the item table, saying which index and how many items there are.
#[test]
fn an_item_index_outside_the_table_names_the_table() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let error = tag.item(99).unwrap_err();
    assert_eq!(error.offset, section_offset(&bytes, b"ITEM"));
    assert!(error.detail.contains("99"), "{error}");
    assert!(error.detail.contains("4 items"), "{error}");
}

/// A dangling reference is reported where the reference is stored: a file offset, not an offset in DATA.
#[test]
fn a_dangling_reference_names_the_stored_reference_in_the_file() {
    let mut bytes = fixture();
    let data = section_offset(&bytes, b"DATA");
    bytes[data + 8 + 8..data + 8 + 12].copy_from_slice(&99u32.to_le_bytes());
    let tag = Tagfile::read(&bytes, None).expect("container");
    let error = tag
        .root()
        .unwrap()
        .field("pointer")
        .unwrap()
        .dereference()
        .unwrap_err();
    assert_eq!(error.offset, data + 8 + 8);
    assert!(error.detail.contains("99"), "{error}");
}

/// Errors of the typed views carry file offsets (DATA starts eight bytes after its section header), and say what was
/// asked for.
#[test]
fn value_errors_carry_file_offsets() {
    let bytes = fixture();
    let data = section_offset(&bytes, b"DATA") + 8;
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let root = tag.root().unwrap();
    let missing = root.field("absent").unwrap_err();
    assert_eq!(missing.offset, data);
    let array = root.field("array").unwrap().array().unwrap();
    let past = array.get(2).unwrap_err();
    // where element 2 would be: the item is at 40, its elements eight bytes each
    assert_eq!(past.offset, data + 40 + 16);
    assert!(
        past.detail.contains("index 2") && past.detail.contains("2 elements"),
        "{past}"
    );
    let not_a_class = root.field("value").unwrap().field("x").unwrap_err();
    assert_eq!(not_a_class.offset, data);
    assert!(not_a_class.detail.contains("class"), "{not_a_class}");
}

/// Elements of no size take no bytes however many there are: a count of 2^32 of them passes the extent check, and
/// readers then size their output from it. They are refused instead.
#[test]
fn repeated_zero_size_elements_are_refused() {
    let zero_size = |count: u32| {
        let mut bytes = fixture();
        // `uint`: its size and alignment bytes
        let body = [2u8, 0, 9, 0xc0, 0x80, 4, 4, 4];
        let at = bytes
            .windows(body.len())
            .position(|w| w == body)
            .expect("uint body");
        bytes[at + 6] = 0;
        // item 2 (type 2, offset 32): its count
        let item = [0x02u8, 0, 0, 0x10, 32, 0, 0, 0];
        let at = bytes.windows(item.len()).position(|w| w == item).expect("item 2") + 8;
        bytes[at..at + 4].copy_from_slice(&count.to_le_bytes());
        bytes
    };
    assert!(Tagfile::read(&zero_size(1), None).is_ok());
    let error = Tagfile::read(&zero_size(u32::MAX), None).unwrap_err();
    assert!(error.detail.contains("zero-size"), "{error}");
}

/// The SDKs whose tagfiles are read: 2015.1 (Sekiro's one clip), 2016.2 and 2018.1; the 2018.2 string, which no file
/// of either game has, is not.
#[test]
fn the_sdk_versions_are_2015_1_2016_2_and_2018_1() {
    assert_eq!(
        havok_formats::SDK_VERSIONS,
        [*b"20150100", *b"20160200", *b"20180100"]
    );
    for sdk in havok_formats::SDK_VERSIONS {
        let bytes = fixture_with(&sdk, packed(3));
        let tag = Tagfile::read(&bytes, None).expect("fixture");
        assert_eq!(tag.version, sdk);
        assert_eq!(
            tag.root().unwrap().field("value").unwrap().unsigned().unwrap(),
            17
        );
    }
    for other in [b"20180200", b"20140100", b"20170100"] {
        assert!(Tagfile::read(&fixture_with(other, packed(3)), None).is_err());
    }
}

/// The byte of item `i`'s record that holds its kind: the record starts 8 bytes into the item table's section and
/// takes 12, the kind is the fourth.
fn kind_byte(bytes: &[u8], i: usize) -> usize {
    section_offset(bytes, b"ITEM") + 8 + 12 * i + 3
}

/// An item's kind is the byte beside its type index: 0x10 (one object) or 0x20 (a run), and 0 on the null item; the
/// rest of the byte is no flags in any file.
#[test]
fn an_item_is_an_object_or_a_run_and_nothing_else() {
    use havok_formats::ItemKind;
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let kinds: Vec<ItemKind> = tag.items.iter().map(|i| i.kind).collect();
    assert_eq!(
        kinds,
        [ItemKind::Null, ItemKind::Object, ItemKind::Object, ItemKind::Run]
    );
    // either of the two kinds on any item after the null one
    for (item, byte) in [(1, 0x20), (3, 0x10)] {
        let mut changed = bytes.clone();
        changed[kind_byte(&bytes, item)] = byte;
        assert!(Tagfile::read(&changed, None).is_ok(), "item {item} as {byte:#x}");
    }
    for (item, byte) in [
        (1, 0x00),
        (1, 0x11),
        (1, 0x30),
        (2, 0x01),
        (3, 0x2F),
        (3, 0xF0),
        (0, 0x10),
        (0, 0x20),
    ] {
        let mut changed = bytes.clone();
        changed[kind_byte(&bytes, item)] = byte;
        let error = Tagfile::read(&changed, None).unwrap_err();
        assert_eq!(error.offset, kind_byte(&bytes, item), "item {item} as {byte:#x}");
        assert!(
            error
                .detail
                .contains(&format!("item {item}: kind byte {byte:#x}")),
            "{error}"
        );
    }
}

/// A pointer, string or array slot is the item index and zeros: the rest of an eight-byte pointer, and the size and
/// capacity words of a 16-byte `hkArray` (the loader fills them in).
#[test]
fn a_reference_slot_holds_only_its_item_index() {
    let bytes = fixture();
    let data = section_offset(&bytes, b"DATA") + 8;
    // the pointer member is DATA bytes 8 to 16, the array member 16 to 32
    for (member, slot, size) in [("pointer", 8, 8), ("array", 16, 16)] {
        for high in 4..size {
            let mut changed = bytes.clone();
            changed[data + slot + high] = 1;
            let tag = Tagfile::read(&changed, None).expect("the container reads");
            let value = tag.root().unwrap().field(member).unwrap();
            let error = if member == "pointer" {
                value.dereference().unwrap_err()
            } else {
                value.array().unwrap_err()
            };
            assert_eq!(error.offset, data + slot + 4, "{member} byte {high}");
            assert!(error.detail.contains(&format!("bytes 4..{size}")), "{error}");
        }
        // the index alone reads
        let tag = Tagfile::read(&bytes, None).expect("fixture");
        assert!(tag.root().unwrap().field(member).is_ok());
    }
}

/// The patch records of a tagfile: one a slot type (the pointer type 3, the array type 4 here), the types ascending
/// and each record's offsets strictly ascending. The offsets are the stored references: the root's pointer (8) and
/// array (16) and the two pointers of item 3 (40, 48).
#[test]
fn patch_records_ascend_by_type_and_by_offset() {
    let good: &[Patch] = &[(3, &[8, 40, 48]), (4, &[16])];
    let bytes = fixture_patched(b"20160200", packed(3), good);
    let tag = Tagfile::read(&bytes, None).expect("patched fixture");
    assert_eq!(tag.patches, vec![(3, vec![8, 40, 48]), (4, vec![16])]);
    // no PTCH section, or a record with no offsets, reads
    assert!(
        Tagfile::read(&fixture(), None)
            .expect("fixture")
            .patches
            .is_empty()
    );
    assert!(
        Tagfile::read(
            &fixture_patched(b"20160200", packed(3), &[(3, &[]), (4, &[16])]),
            None
        )
        .is_ok()
    );
    let refused: [(&[Patch], &str); 4] = [
        // the types descend, or one type has two records
        (&[(4, &[16]), (3, &[8])], "patch record of type 3 after type 4"),
        (&[(3, &[8]), (3, &[40])], "patch record of type 3 after type 3"),
        // the offsets of a record descend or repeat
        (&[(3, &[40, 8])], "patch offset 0x8 after 0x28"),
        (&[(3, &[8, 8])], "patch offset 0x8 after 0x8"),
    ];
    for (patches, want) in refused {
        let error = Tagfile::read(&fixture_patched(b"20160200", packed(3), patches), None).unwrap_err();
        assert!(error.detail.contains(want), "{want}: {error}");
    }
}

/// The fixture's items after a write: type, kind byte and count of each, and where DATA holds them.
fn written_items(bytes: &[u8]) -> Vec<(usize, u8, usize, usize)> {
    let tag = Tagfile::read(bytes, None).expect("written fixture");
    tag.items
        .iter()
        .skip(1)
        .map(|i| {
            let kind = match i.kind {
                havok_formats::ItemKind::Object => 0x10,
                _ => 0x20,
            };
            (i.type_index, kind, i.offset, i.count)
        })
        .collect()
}
#[test]
fn a_tagfile_is_written_in_the_layout_the_game_files_have() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let written = tag.image().expect("image").write().expect("write");
    // the fixture is not laid out as the game's files are (its DATA is not a multiple of 16 long), so the comparison of
    // a file with what is written from it can fail
    assert_ne!(written, bytes);
    // the root at 0; the run an array slot refers to on a multiple of 16; the object a pointer refers to after it,
    // on its type's alignment; DATA a multiple of 16 long
    assert_eq!(
        written_items(&written),
        [(1, 0x10, 0, 1), (2, 0x10, 48, 1), (3, 0x20, 32, 2)]
    );
    let back = Tagfile::read(&written, None).expect("read back");
    // one patch record a slot type, ascending: the three pointer slots (the root's and the array's two) and the array
    assert_eq!(back.patches, vec![(3, vec![8, 32, 40]), (4, vec![16])]);
    let root = back.root().expect("root");
    assert_eq!(root.field("value").expect("value").unsigned().expect("uint"), 17);
    assert_eq!(
        root.field("pointer")
            .expect("pointer")
            .dereference()
            .expect("target")
            .unsigned()
            .expect("uint"),
        91
    );
    assert_eq!(
        root.field("array")
            .expect("array")
            .array()
            .expect("array view")
            .len(),
        2
    );
    // what is written reads and writes again as itself
    assert_eq!(back.image().expect("image").write().expect("write"), written);
}
#[test]
fn an_edited_value_is_written_and_read_back_with_everything_else_unchanged() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let (item, within) = tag
        .item_at(tag.root().expect("root").field("value").expect("value").offset)
        .expect("the value is in an item");
    assert_eq!((item, within), (1, 0));
    let mut image = tag.image().expect("image");
    let before = image.write().expect("write");
    image.items[item].data[within] = 18;
    let after = image.write().expect("write");
    let differing: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
    assert_eq!(before.len(), after.len());
    assert_eq!(differing.len(), 1, "one byte differs");
    let back = Tagfile::read(&after, None).expect("read back");
    assert_eq!(
        back.root()
            .expect("root")
            .field("value")
            .expect("value")
            .unsigned()
            .expect("uint"),
        18
    );
}
#[test]
fn a_reference_can_be_pointed_at_a_new_item_and_the_items_are_renumbered() {
    let tag_bytes = fixture();
    let tag = Tagfile::read(&tag_bytes, None).expect("fixture");
    let mut image = tag.image().expect("image");
    let new = image.push_item(havok_formats::ImageItem {
        type_index: 2,
        kind: havok_formats::ItemKind::Object,
        count: 1,
        data: 5u32.to_le_bytes().to_vec(),
    });
    // an item nothing refers to is not written
    let unused = image.push_item(havok_formats::ImageItem {
        type_index: 2,
        kind: havok_formats::ItemKind::Object,
        count: 1,
        data: 9u32.to_le_bytes().to_vec(),
    });
    assert_eq!((new, unused), (4, 5));
    image.set_reference(1, 8, new).expect("the root's pointer slot");
    let written = image.write().expect("write");
    let back = Tagfile::read(&written, None).expect("read back");
    // root, the new object (found first: the pointer comes before the array), the array's run, the old object
    assert_eq!(back.items.len(), 5);
    assert_eq!(
        written_items(&written),
        [
            (1, 0x10, 0, 1),
            (2, 0x10, 48, 1),
            (3, 0x20, 32, 2),
            (2, 0x10, 52, 1)
        ]
    );
    let root = back.root().expect("root");
    let pointed =
        |v: havok_formats::Value<'_, '_>| v.dereference().expect("target").unsigned().expect("uint");
    assert_eq!(pointed(root.field("pointer").expect("pointer")), 5);
    let array = root.field("array").expect("array").array().expect("array view");
    assert_eq!(pointed(array.get(0).expect("first")), 91);
    assert_eq!(pointed(array.get(1).expect("second")), 91);
    // slots are not set past the item's data, or where there is none
    assert!(image.set_reference(1, 0, 2).is_err());
    assert!(image.set_reference(1, 400, 2).is_err());
    assert!(image.set_reference(9, 8, 2).is_err());
    assert!(image.set_string(1, 8, "x").is_err(), "a pointer is not a string");
}
#[test]
fn an_image_that_does_not_hold_together_is_refused_on_write() {
    let bytes = fixture();
    let tag = Tagfile::read(&bytes, None).expect("fixture");
    let image = tag.image().expect("image");
    let refused = |image: &havok_formats::TagfileImage, why: &str| {
        let e = image.write().expect_err(why).to_string();
        assert!(e.contains(why), "{e}");
    };
    let mut short = image.clone();
    short.items[1].data.pop();
    refused(&short, "bytes for");
    let mut wide = image.clone();
    wide.items[1].data[12] = 1;
    refused(&wide, "not zero");
    let mut far = image.clone();
    far.set_reference(1, 8, 99).expect("set");
    refused(&far, "refers to item 99");
    let mut run = image.clone();
    run.items[1].kind = havok_formats::ItemKind::Run;
    refused(&run, "not a single object");
    let mut null = image.clone();
    null.items.truncate(1);
    refused(&null, "no root");
}

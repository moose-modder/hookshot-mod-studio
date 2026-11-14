//! The packfile image and writer on hand-made images of the three layouts the readers know (version 7, 9 and 11): what
//! `write` lays out is worked out here by hand, and the hostile-input checks of the packfile reader run on the
//! written files. No game data.
#![allow(
    clippy::unwrap_used,
    reason = "a fixture that does not build is the test's failure"
)]
use havok_formats::packfile::image::{Block, ImageObject, NESTED_RUN_ALIGNMENT, PackfileImage, Slot, Target};
use havok_formats::packfile::{CLASS_SIGNATURES, Packfile, Rules};

mod hostile;
use hostile::{Lcg, hostile, use_everything};

const REFLECTION: [&str; 4] = ["hkClass", "hkClassMember", "hkClassEnum", "hkClassEnumItem"];

/// The signatures the version 7 reader holds the classes to, or made-up ones for a class it does not know.
fn signatures(extra: &[&str]) -> Vec<(String, u32)> {
    REFLECTION
        .iter()
        .chain(extra)
        .zip(0x1000_0001u32..)
        .map(|(n, s)| {
            let known = CLASS_SIGNATURES.iter().find(|(name, _)| name == n);
            ((*n).to_owned(), known.map_or(s, |&(_, signature)| signature))
        })
        .collect()
}

fn header_words(version: u32) -> ([u32; 2], Vec<u8>, &'static str, Rules) {
    match version {
        7 => (
            [u32::MAX, u32::MAX],
            Vec::new(),
            "Havok-7.1.0-r1",
            Rules::POWERPC_32,
        ),
        9 => ([0, u32::MAX], Vec::new(), "hk_2013.1.0-r1", Rules::WINDOWS_64),
        _ => (
            [0, 0x0010_0015],
            [&[0x14u8, 0][..], &[0; 14]].concat(),
            "hk_2014.1.0-r1",
            Rules::WINDOWS_64,
        ),
    }
}

fn u32_bytes(rules: Rules, v: u32) -> [u8; 4] {
    if rules.little_endian {
        v.to_le_bytes()
    } else {
        v.to_be_bytes()
    }
}

/// A root container whose one variant is named by `name`, with class name `hkpPhysicsData`, pointing to a physics data
/// object of 16 zero bytes; the way the games' files hold theirs.
fn scene(version: u32, name: &str) -> PackfileImage {
    let (filler, predicate_region, contents_version, rules) = header_words(version);
    let width = rules.pointer_size;
    // hkRootLevelContainer: namedVariants (an hkArray: pointer, size, capacity)
    let mut body = vec![0; width];
    body.extend(u32_bytes(rules, 1));
    body.extend(u32_bytes(rules, 0x8000_0001));
    // NamedVariant: name, className, variant: three pointers
    let string = |text: &str| {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        Block {
            bytes,
            slots: Vec::new(),
            align: NESTED_RUN_ALIGNMENT,
            packed: false,
        }
    };
    let elements = Block {
        bytes: vec![0; width * 3],
        slots: vec![
            Slot {
                offset: 0,
                target: Target::Run(string(name)),
            },
            Slot {
                offset: width,
                target: Target::Run(string("hkpPhysicsData")),
            },
            Slot {
                offset: width * 2,
                target: Target::Object(1),
            },
        ],
        align: 16,
        packed: false,
    };
    let root = Block {
        bytes: body,
        slots: vec![Slot {
            offset: 0,
            target: Target::Run(elements),
        }],
        align: 16,
        packed: false,
    };
    PackfileImage {
        version,
        rules,
        user_tag: 0,
        contents_version: contents_version.to_owned(),
        filler,
        predicate_region,
        signatures: signatures(&["hkRootLevelContainer", "hkpPhysicsData"]),
        types: Vec::new(),
        objects: vec![
            ImageObject {
                class: "hkRootLevelContainer".into(),
                block: root,
            },
            ImageObject {
                class: "hkpPhysicsData".into(),
                block: Block::run(vec![0; 16]),
            },
        ],
        root: 0,
    }
}

fn word(bytes: &[u8], at: usize, rules: Rules) -> u32 {
    let b: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
    if rules.little_endian {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    }
}

#[test]
fn the_three_layouts_are_written_and_read_back() {
    for version in [7, 9, 11] {
        let image = scene(version, "Physics Data");
        let bytes = image.write().unwrap();
        let pack = Packfile::read(&bytes).unwrap_or_else(|e| panic!("version {version}: {e}"));
        assert_eq!(pack.version, version);
        assert_eq!(pack.contents_version, image.contents_version);
        assert_eq!(pack.rules, image.rules);
        assert_eq!(pack.predicate_region(), &image.predicate_region[..]);
        // the classes: the four reflection classes first, then the root's, then the other
        let names: Vec<&str> = pack.class_names().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [REFLECTION.as_slice(), &["hkRootLevelContainer", "hkpPhysicsData"]].concat()
        );
        // reading what was written gives an image that writes the same bytes, and the same image again (its blocks hold
        // the padding the file does)
        let again = pack.image().unwrap();
        assert_eq!(again.write().unwrap(), bytes, "version {version}");
        assert_eq!(again.objects.len(), image.objects.len());
        let twice = Packfile::read(&bytes).unwrap().image().unwrap();
        assert_eq!(twice, again, "version {version}");
    }
}

/// The layout of the root, worked out by hand for version 7 (the games' own: the body is 12 bytes, padded to 16; the
/// variants follow on a 16-byte boundary at 0x10, 12 bytes; the first string follows at once at 0x1c and is padded to
/// 0x30, where the second starts and is padded to 0x40; the next object starts there) and for the 8-byte pointers of
/// version 9 and 11 (the body is 16 bytes; the variants at 0x10 are 24 bytes; the first string at 0x28, padded to
/// 0x40; the second at 0x40, padded to 0x50).
#[test]
fn runs_are_written_where_the_rules_put_them() {
    for (version, strings, next) in [
        (7, [0x1C, 0x30], 0x40),
        (9, [0x28, 0x40], 0x50),
        (11, [0x28, 0x40], 0x50),
    ] {
        let image = scene(version, "Physics Data");
        let bytes = image.write().unwrap();
        let pack = Packfile::read(&bytes).unwrap();
        let rules = image.rules;
        let data = pack.sections.iter().find(|s| s.tag == "__data__").unwrap();
        let at = |o: usize| data.start + o;
        assert_eq!(pack.c_string(strings[0]).unwrap(), "Physics Data");
        assert_eq!(pack.c_string(strings[1]).unwrap(), "hkpPhysicsData");
        // the fixups: variants, then the two strings, from the three pointer slots; the variant's object
        let width = rules.pointer_size;
        assert_eq!(pack.pointer(0).unwrap(), Some(0x10));
        assert_eq!(pack.pointer(0x10).unwrap(), Some(strings[0]));
        assert_eq!(pack.pointer(0x10 + width).unwrap(), Some(strings[1]));
        assert_eq!(pack.pointer(0x10 + 2 * width).unwrap(), Some(next));
        assert_eq!(
            pack.objects().iter().map(|o| o.offset).collect::<Vec<_>>(),
            [0, next]
        );
        // the sections: the data is the objects, 16-byte padded, then the three tables padded to 16
        assert_eq!(data.local, next + 16, "version {version}");
        let local_entries = 3;
        let global_entries = 1;
        let virtual_entries = 2;
        let pad = |bytes: usize| bytes.div_ceil(16) * 16;
        assert_eq!(data.global - data.local, pad(local_entries * 8));
        assert_eq!(data.virtual_fixups - data.global, pad(global_entries * 12));
        assert_eq!(data.exports - data.virtual_fixups, pad(virtual_entries * 12));
        assert_eq!(data.imports, data.exports);
        assert_eq!(data.end, data.exports);
        assert_eq!(bytes.len(), at(data.end));
        // a table that is not 16 bytes long ends in 0xFFFFFFFF words: three pairs are 24 bytes, so one pair of them
        let tail = at(data.global) - 8;
        assert_eq!(word(&bytes, tail, rules), u32::MAX);
        assert_eq!(word(&bytes, tail + 4, rules), u32::MAX);
    }
}

#[test]
fn editing_the_image_changes_the_file_the_way_the_data_says() {
    for version in [7, 9, 11] {
        let mut image = scene(version, "Physics Data");
        let before = image.write().unwrap();
        // a longer name: the string moves the class name's string and the object after it
        let Target::Run(elements) = &mut image.objects[0].block.slots[0].target else {
            panic!("the variants are a run");
        };
        let Target::Run(name) = &mut elements.slots[0].target else {
            panic!("the name is a run");
        };
        name.bytes = b"A much longer name for the variant\0".to_vec();
        let after = image.write().unwrap();
        assert_ne!(after, before);
        let pack = Packfile::read(&after).unwrap();
        let read = pack.image().unwrap();
        let Target::Run(elements) = &read.objects[0].block.slots[0].target else {
            panic!();
        };
        let Target::Run(name) = &elements.slots[0].target else {
            panic!();
        };
        assert!(name.bytes.starts_with(b"A much longer name for the variant\0"));
        // the 35 bytes and the zeros up to the next 16-byte boundary after the name's start
        assert_eq!(name.bytes.len(), if version == 7 { 36 } else { 40 });
        let Target::Run(class) = &elements.slots[1].target else {
            panic!();
        };
        assert!(class.bytes.starts_with(b"hkpPhysicsData\0"));
        // one more object, pointed to from the physics data, appears after it
        let mut grown = image.clone();
        grown.signatures.push(("hkpPhysicsSystem".into(), 0xFF72_4C17));
        grown.objects.push(ImageObject {
            class: "hkpPhysicsSystem".into(),
            block: Block::run(vec![1; 32]),
        });
        let width = grown.rules.pointer_size;
        grown.objects[1].block = Block {
            bytes: vec![0; width.max(16)],
            slots: vec![Slot {
                offset: 0,
                target: Target::Object(2),
            }],
            align: 16,
            packed: false,
        };
        let bytes = grown.write().unwrap();
        let pack = Packfile::read(&bytes).unwrap();
        assert_eq!(pack.objects().len(), 3);
        assert_eq!(
            pack.class_at(pack.objects()[2].offset).unwrap(),
            "hkpPhysicsSystem"
        );
        // an object nothing leads to is not written
        let mut orphan = grown.clone();
        orphan.objects[1].block.slots.clear();
        assert_eq!(
            Packfile::read(&orphan.write().unwrap()).unwrap().objects().len(),
            2
        );
    }
}

#[test]
fn an_image_that_cannot_be_written_is_refused() {
    let good = scene(7, "n");
    let refuse = |image: PackfileImage| image.write().unwrap_err().to_string();
    let mut bad = good.clone();
    bad.root = 9;
    assert!(refuse(bad).contains("object 9 does not exist"));
    let mut bad = good.clone();
    bad.objects[1].block.slots.push(Slot {
        offset: 0,
        target: Target::Object(9),
    });
    assert!(refuse(bad).contains("object 9 does not exist"));
    let mut bad = good.clone();
    bad.objects[1].class = "hkUnknown".into();
    assert!(refuse(bad).contains("hkUnknown has no signature"));
    let mut bad = good.clone();
    bad.objects[0].block.slots[0].offset = 100;
    assert!(refuse(bad).contains("leaves its block"));
    let mut bad = good.clone();
    bad.rules = Rules::WINDOWS_64;
    assert!(refuse(bad).contains("cannot be written"));
    let mut bad = good.clone();
    bad.version = 8;
    assert!(refuse(bad).contains("cannot be written"));
    // runs nested without end
    let mut deep = Block::run(vec![0; 16]);
    for _ in 0..20 {
        deep = Block {
            bytes: vec![0; 16],
            slots: vec![Slot {
                offset: 0,
                target: Target::Run(deep),
            }],
            align: 2,
            packed: false,
        };
    }
    let mut bad = good.clone();
    bad.objects[1].block = deep;
    assert!(refuse(bad).contains("nested more than"));
    let mut bad = good;
    bad.objects[0].block.slots[0].target = Target::Run(Block {
        bytes: vec![0; 16],
        slots: Vec::new(),
        align: 3,
        packed: false,
    });
    assert!(refuse(bad).contains("not a boundary"));
}

// --- hostile input -----------------------------------------------------------------------------------------------

#[test]
fn synthetic_files_survive_truncation_and_flipped_bytes() {
    for version in [7, 9, 11] {
        let bytes = scene(version, "Physics Data").write().unwrap();
        hostile(&format!("synthetic version {version}"), &bytes);
    }
}

#[test]
fn garbage_is_not_a_packfile() {
    let mut lcg = Lcg(7);
    for len in [0usize, 1, 7, 8, 63, 64, 65, 255, 1000, 4096] {
        let garbage: Vec<u8> = (0..len).map(|_| (lcg.next() & 0xFF) as u8).collect();
        let error = Packfile::read(&garbage).unwrap_err();
        if len >= 8 {
            assert!(
                error.detail.contains("not a Havok packfile"),
                "{len} bytes: {error}"
            );
        }
        use_everything(&garbage);
    }
    // the magic of a packfile and nothing else
    let mut bytes = vec![0x57, 0xE0, 0xE0, 0x57, 0x10, 0xC0, 0xC0, 0x10];
    bytes.resize(0x200, 0);
    let error = Packfile::read(&bytes).unwrap_err();
    assert!(error.detail.contains("unsupported packfile version 0"), "{error}");
    // tagfile and compendium magics are not packfiles
    for magic in [&b"\0\0\0\x20TAG0"[..], b"\0\0\0\x20TCM0"] {
        let mut bytes = magic.to_vec();
        bytes.resize(64, 0);
        assert!(
            Packfile::read(&bytes)
                .unwrap_err()
                .detail
                .contains("not a Havok packfile")
        );
    }
}

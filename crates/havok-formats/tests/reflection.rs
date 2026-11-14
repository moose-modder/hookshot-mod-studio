//! The class metadata of the eight Tony Hawk's SHRED packfiles that carry it, read into a class table: every stored
//! signature recomputes from the table, and the layout rules give every stored member offset and class size. Skips
//! without the packfiles (see tests/inputs/README.md); no game data is in the repository.
mod inputs;

use std::collections::BTreeMap;

use havok_formats::packfile::reflection::{ClassTable, MemberType};
use havok_formats::packfile::{Packfile, Rules};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

type Tables = Vec<(String, ClassTable)>;

fn tables() -> Result<Option<Tables>, Box<dyn std::error::Error>> {
    let query = Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles");
    let Some(dir) = resolve(query) else {
        return Ok(None);
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    let mut out = Vec::new();
    let mut read = 0;
    for path in files {
        let bytes = std::fs::read(&path)?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let pack = Packfile::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        read += 1;
        if let Some(table) = ClassTable::from_packfile(&pack).map_err(|e| format!("{name}: {e}"))? {
            out.push((name, table));
        }
    }
    expect_found(query, "packfiles", read);
    Ok(Some(out))
}

#[test]
fn eight_files_carry_their_classes() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tables) = tables()? else {
        return Ok(());
    };
    assert_eq!(tables.len(), 8);
    let mut members = 0;
    let mut enums = 0;
    let mut signed = 0;
    let mut kinds: BTreeMap<u8, usize> = BTreeMap::new();
    for (name, table) in &tables {
        assert_eq!(table.classes.len(), 68, "{name}");
        for class in &table.classes {
            members += class.members.len();
            enums += class.enums.len();
            signed += usize::from(class.signature.is_some());
            for m in &class.members {
                *kinds.entry(m.kind.0).or_default() += 1;
            }
        }
    }
    eprintln!(
        "members {members}, declared enums {enums}, listed signatures {signed}, member types {kinds:?}"
    );
    assert_eq!(signed, 8 * 12);
    Ok(())
}

#[test]
fn every_stored_signature_recomputes_from_the_table() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tables) = tables()? else {
        return Ok(());
    };
    let mut checked = 0;
    for (name, table) in &tables {
        for class in &table.classes {
            let Some(stored) = class.signature else { continue };
            let computed = table.signature(&class.name)?;
            assert_eq!(
                computed, stored,
                "{name}: {} {computed:#010x} against {stored:#010x}",
                class.name
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 96);
    Ok(())
}

#[test]
fn the_layout_rules_give_every_stored_offset_and_size() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tables) = tables()? else {
        return Ok(());
    };
    let mut classes = 0;
    let mut offsets = 0;
    let mut wrong = Vec::new();
    for (name, table) in &tables {
        for class in &table.classes {
            let layout = table.layout(&class.name, Rules::POWERPC_32)?;
            classes += 1;
            if Some(layout.size) != class.size {
                wrong.push(format!(
                    "{name}: {} size {} against {:?}",
                    class.name, layout.size, class.size
                ));
            }
            for (m, at) in class.members.iter().zip(&layout.offsets) {
                offsets += 1;
                if Some(*at) != m.offset {
                    wrong.push(format!(
                        "{name}: {}.{} at {at} against {:?}",
                        class.name, m.name, m.offset
                    ));
                }
            }
        }
    }
    eprintln!("{classes} classes, {offsets} member offsets");
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
    Ok(())
}

/// These 68 classes do not tell whether the rules reuse padding: no class derives from a parent whose last member ends
/// short of its size, so the layout is the same either way. (The files' header says the rules reuse it.)
#[test]
fn the_padding_rule_changes_nothing_in_these_classes() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tables) = tables()? else {
        return Ok(());
    };
    let without = Rules {
        reuse_padding: false,
        ..Rules::POWERPC_32
    };
    for (name, table) in &tables {
        for class in &table.classes {
            let with = table.layout(&class.name, Rules::POWERPC_32)?;
            let off = table.layout(&class.name, without)?;
            assert_eq!(
                (with.size, with.offsets),
                (off.size, off.offsets),
                "{name}: {}",
                class.name
            );
        }
    }
    let _ = MemberType::BOOL;
    Ok(())
}

/// The offsets `packfile::physics` states by hand, for the classes the metadata of the ragdoll files covers: the class
/// table gives the same ones (`class`, member path, offset).
const PHYSICS_OFFSETS: [(&str, &[&str], usize); 42] = [
    ("hkpRigidBody", &["collidable", "shape"], 0x10),
    ("hkpRigidBody", &["collidable", "shapeKey"], 0x14),
    ("hkpRigidBody", &["collidable", "broadPhaseHandle", "type"], 0x28),
    (
        "hkpRigidBody",
        &["collidable", "broadPhaseHandle", "objectQualityType"],
        0x2A,
    ),
    (
        "hkpRigidBody",
        &["collidable", "broadPhaseHandle", "collisionFilterInfo"],
        0x2C,
    ),
    ("hkpRigidBody", &["collidable", "allowedPenetrationDepth"], 0x5C),
    ("hkpRigidBody", &["name"], 0x74),
    ("hkpRigidBody", &["material", "responseType"], 0x8C),
    ("hkpRigidBody", &["material", "friction"], 0x90),
    ("hkpRigidBody", &["material", "restitution"], 0x94),
    ("hkpRigidBody", &["storageIndex"], 0xA8),
    ("hkpRigidBody", &["contactPointCallbackDelay"], 0xAA),
    ("hkpRigidBody", &["motion", "type"], 0xE8),
    ("hkpRigidBody", &["motion", "motionState", "transform"], 0xF0),
    (
        "hkpRigidBody",
        &["motion", "motionState", "sweptTransform", "centerOfMass0"],
        0x130,
    ),
    (
        "hkpRigidBody",
        &["motion", "motionState", "sweptTransform", "centerOfMass1"],
        0x140,
    ),
    (
        "hkpRigidBody",
        &["motion", "motionState", "sweptTransform", "rotation0"],
        0x150,
    ),
    (
        "hkpRigidBody",
        &["motion", "motionState", "sweptTransform", "rotation1"],
        0x160,
    ),
    (
        "hkpRigidBody",
        &["motion", "motionState", "sweptTransform", "centerOfMassLocal"],
        0x170,
    ),
    ("hkpRigidBody", &["motion", "motionState", "objectRadius"], 0x190),
    ("hkpRigidBody", &["motion", "motionState", "linearDamping"], 0x194),
    (
        "hkpRigidBody",
        &["motion", "motionState", "angularDamping"],
        0x198,
    ),
    ("hkpRigidBody", &["motion", "inertiaAndMassInv"], 0x1A0),
    ("hkpRigidBody", &["motion", "linearVelocity"], 0x1B0),
    ("hkpRigidBody", &["motion", "angularVelocity"], 0x1C0),
    ("hkpRigidBody", &["motion", "gravityFactor"], 0x1FE),
    ("hkpSphereShape", &["radius"], 0x10),
    ("hkpCapsuleShape", &["radius"], 0x10),
    ("hkpCapsuleShape", &["vertexA"], 0x20),
    ("hkpCapsuleShape", &["vertexB"], 0x30),
    ("hkpBoxShape", &["halfExtents"], 0x20),
    ("hkpConvexTranslateShape", &["childShape", "childShape"], 0x18),
    ("hkpConvexTranslateShape", &["translation"], 0x20),
    ("hkpConvexVerticesShape", &["aabbHalfExtents"], 0x20),
    ("hkpConvexVerticesShape", &["aabbCenter"], 0x30),
    ("hkpConvexVerticesShape", &["rotatedVertices"], 0x40),
    ("hkpConvexVerticesShape", &["numVertices"], 0x4C),
    ("hkpConvexVerticesShape", &["planeEquations"], 0x54),
    ("hkpConvexVerticesShape", &["connectivity"], 0x60),
    ("hkpConvexVerticesConnectivity", &["vertexIndices"], 0x08),
    ("hkpConvexVerticesConnectivity", &["numVerticesPerFace"], 0x14),
    ("hkpPhysicsSystem", &["active"], 0x40),
];

#[test]
fn the_offsets_physics_states_by_hand_are_the_metadata_s() -> Result<(), Box<dyn std::error::Error>> {
    let Some(tables) = tables()? else {
        return Ok(());
    };
    for (name, table) in &tables {
        for (class, path, want) in PHYSICS_OFFSETS {
            let got = table.member_offset(class, path, Rules::POWERPC_32)?;
            assert_eq!(got, want, "{name}: {class}.{}", path.join("."));
        }
        // the other things the hand-stated table and the views rely on
        let sizes = [
            ("hkpRigidBody", 0x220),
            ("hkpConvexVerticesShape", 0x70),
            ("hkpPhysicsSystem", 0x44),
        ];
        for (class, size) in sizes {
            assert_eq!(
                table.class(class).and_then(|c| c.size),
                Some(size),
                "{name}: {class}"
            );
        }
    }
    Ok(())
}

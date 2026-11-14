//! Real-data checks of the names of the enumerated members (`havok_formats::enums`), of the member declaration flags
//! (`DeclFlags`) and of the section flags (`SectionFlags`). Skips without the inputs (see tests/inputs/README.md); no
//! game data is included in the repository.
//!
//! The expected values come from censuses of the games' own files made for this check, independent of the code under
//! test: the class metadata of the eight reflection-bearing SHRED packfiles (the enumeration items themselves), a walk of
//! all 490 SHRED packfiles for the byte at each enumerated offset, and a walk of the `havok-enums` corpora (the smallest
//! tagfile of each game that shows each value found in a census of every distinct tagfile of the install: 19,127 of
//! Sekiro and 68,722 of Elden Ring) that counts the values through the generic value view, by class and member name.
mod inputs;

use std::collections::BTreeMap;

use havok_formats::animation::container;
use havok_formats::cloth;
use havok_formats::enums::{hcl, hka, hknp, hkp};
use havok_formats::packfile::Packfile;
use havok_formats::packfile::physics::{self, ExtendedMesh, MoppCode, RigidBody, Shape};
use havok_formats::packfile::reflection::ClassTable;
use havok_formats::{Compendium, DeclAccess, DeclFlags, Tagfile, compressed_mesh};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

type Failure = Box<dyn std::error::Error>;
type Counts = BTreeMap<String, u64>;
type Expected = &'static [(&'static str, u64)];

fn count(counts: &mut Counts, key: impl Into<String>) {
    *counts.entry(key.into()).or_default() += 1;
}

fn expected(list: Expected) -> Counts {
    list.iter().map(|&(k, n)| (k.to_owned(), n)).collect()
}

// ---------------------------------------------------------------------------------------------------- SHRED (7.1)

/// Files by name, with their bytes.
type Files = Vec<(String, Vec<u8>)>;

/// All 490 SHRED packfiles, by file name; `None` without them.
fn shred_files() -> Result<Option<Files>, Failure> {
    let query = Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles");
    let Some(dir) = resolve(query) else {
        return Ok(None);
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        out.push((name, std::fs::read(&path)?));
    }
    expect_found(query, "packfiles", out.len());
    Ok(Some(out))
}

/// The items of the enumeration `name` that class `class` of `table` declares, as (name, value).
fn items_of<'t>(table: &'t ClassTable, class: &str, name: &str) -> Option<Vec<(&'t str, i32)>> {
    let class = table.class(class)?;
    let e = class.enums.iter().find(|e| e.name == name)?;
    Some(e.items.iter().map(|i| (i.name.as_str(), i.value)).collect())
}

/// Every one of the eight SHRED files that carry their class metadata lists the three enumerations the crate takes its
/// `hkp` names from, with exactly the crate's variants and the enumeration's `*_MAX_ID` count after them. The items
/// are the census (`hkpMotion::MotionType` 9 items, `hkpMaterial::ResponseType` 5, `hkpWorldObject::BroadPhaseType` 5).
#[test]
fn shred_metadata_lists_the_enumerations_the_crate_names() -> Result<(), Failure> {
    let Some(files) = shred_files()? else {
        return Ok(());
    };
    macro_rules! want {
        ($t:ty, $max:literal) => {{
            let mut items: Vec<(String, i32)> = <$t>::ALL
                .iter()
                .map(|v| (v.name().to_owned(), v.value()))
                .collect();
            let next = items.last().map_or(0, |(_, v)| v + 1);
            items.push(($max.to_owned(), next));
            items
        }};
    }
    let motion = want!(hkp::MotionType, "MOTION_MAX_ID");
    let response = want!(hkp::ResponseType, "RESPONSE_MAX_ID");
    let broad_phase = want!(hkp::BroadPhaseType, "BROAD_PHASE_MAX_ID");
    assert_eq!(motion.len(), 9);
    assert_eq!(response.len(), 5);
    assert_eq!(broad_phase.len(), 5);
    let mut carriers = 0;
    for (name, bytes) in &files {
        let pack = Packfile::read(bytes).map_err(|e| format!("{name}: {e}"))?;
        let Some(table) = ClassTable::from_packfile(&pack).map_err(|e| format!("{name}: {e}"))? else {
            continue;
        };
        carriers += 1;
        for (class, enumeration, want) in [
            ("hkpMotion", "MotionType", &motion),
            ("hkpMaterial", "ResponseType", &response),
            ("hkpWorldObject", "BroadPhaseType", &broad_phase),
        ] {
            let items = items_of(&table, class, enumeration)
                .ok_or_else(|| format!("{name}: no {class}::{enumeration}"))?;
            let items: Vec<(String, i32)> = items.into_iter().map(|(n, v)| (n.to_owned(), v)).collect();
            assert_eq!(&items, want, "{name}: {class}::{enumeration}");
        }
    }
    assert_eq!(carriers, 8);
    Ok(())
}

/// The enumerated members of every rigid body, extended mesh, subpart, MOPP code and list child of the 490 SHRED
/// packfiles, read through the crate and named. The counts are the census of the byte at each enumerated offset
/// (12,789 bodies, 1,758 meshes, 6,445 subparts, 1,705 codes, 10 lists with 41 children): the tables the layout of
/// `packfile::physics` documents, summed over all files.
#[test]
fn shred_enumerated_members_have_the_censused_values() -> Result<(), Failure> {
    let Some(files) = shred_files()? else {
        return Ok(());
    };
    let mut counts = Counts::new();
    let (mut subparts, mut children) = (0u64, 0u64);
    for (name, bytes) in &files {
        let pack = Packfile::read(bytes).map_err(|e| format!("{name}: {e}"))?;
        for offset in physics::rigid_bodies(&pack) {
            let body = RigidBody::read(&pack, offset).map_err(|e| format!("{name}: {e}"))?;
            count(
                &mut counts,
                format!("motion {}", body.motion().ok_or("motion type")?.name()),
            );
            count(
                &mut counts,
                format!("response {}", body.response().ok_or("response type")?.name()),
            );
            count(
                &mut counts,
                format!("broad phase {}", body.broad_phase_type.name()),
            );
            let filter = body.filter();
            assert_eq!(
                (
                    filter.sub_system_id(),
                    filter.sub_system_dont_collide_with(),
                    filter.system_group()
                ),
                (0, 0, 0),
                "{name}: {:#x}",
                body.collision_filter_info
            );
            assert_eq!(filter.word(), body.collision_filter_info);
            count(&mut counts, format!("layer {}", filter.layer()));
        }
        for offset in pack.objects_of("hkpStorageExtendedMeshShape") {
            let mesh = ExtendedMesh::read(&pack, offset).map_err(|e| format!("{name}: {e}"))?;
            count(
                &mut counts,
                format!("welding {}", mesh.welding().ok_or("welding type")?.name()),
            );
            subparts += u64::try_from(mesh.subparts.len())?;
        }
        for offset in pack.objects_of("hkpMoppCode") {
            let code = MoppCode::read(&pack, offset).map_err(|e| format!("{name}: {e}"))?;
            count(
                &mut counts,
                format!("mopp {}", code.build().ok_or("build type")?.name()),
            );
        }
        for offset in pack.objects_of("hkpListShape") {
            let Shape::List { children: list } = Shape::read(&pack, offset)? else {
                return Err(format!("{name}: a list shape that is no list").into());
            };
            for child in list {
                assert_eq!(child.collision_filter_info, 0);
                children += 1;
            }
            count(&mut counts, "list");
        }
    }
    // the subparts' type, material index striding type and index striding type are refused unless they are the three the
    // census found in every subpart (triangles, 16-bit material indices, 32-bit indices); reading them is the check
    assert_eq!(subparts, 6_445);
    assert_eq!(children, 41);
    assert_eq!(
        counts,
        expected(&[
            ("broad phase BROAD_PHASE_ENTITY", 12_789),
            ("layer 0", 148),
            ("layer 11", 1_953),
            ("layer 15", 1),
            ("layer 18", 13),
            ("layer 2", 1_933),
            ("layer 7", 8_741),
            ("list", 10),
            ("mopp BUILT_WITH_CHUNK_SUBDIVISION", 1_705),
            ("motion MOTION_BOX_INERTIA", 10_372),
            ("motion MOTION_FIXED", 2_021),
            ("motion MOTION_SPHERE_INERTIA", 396),
            ("response RESPONSE_SIMPLE_CONTACT", 12_789),
            ("welding WELDING_TYPE_ANTICLOCKWISE", 1_758),
        ])
    );
    Ok(())
}

// ----------------------------------------------------------------------------------- Sekiro and Elden Ring tagfiles

/// The tagfiles and compendia of a game's `havok-enums` corpus; `None` without it.
struct Corpus {
    files: Vec<(String, Vec<u8>)>,
    compendia: Vec<Compendium>,
}

fn corpus(game: Game) -> Result<Option<Corpus>, Failure> {
    let query = Query::Oracle(OracleSet::Corpus, game, "havok-enums");
    let Some(dir) = resolve(query) else {
        return Ok(None);
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    let (mut files, mut compendia) = (Vec::new(), Vec::new());
    for path in paths {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        match path.extension().and_then(|e| e.to_str()) {
            Some("hkx") => files.push((name, std::fs::read(&path)?)),
            Some("compendium") => compendia.push(Compendium::read(&std::fs::read(&path)?)?),
            _ => {}
        }
    }
    expect_found(query, ".hkx files", files.len());
    Ok(Some(Corpus { files, compendia }))
}

impl Corpus {
    /// Each tagfile, read with its own types or the first compendium that has them.
    fn tagfiles(&self) -> Result<Vec<(&str, Tagfile<'_>)>, Failure> {
        self.files
            .iter()
            .map(|(name, bytes)| {
                std::iter::once(None)
                    .chain(self.compendia.iter().map(Some))
                    .find_map(|c| Tagfile::read(bytes, c).ok())
                    .map(|tag| (name.as_str(), tag))
                    .ok_or_else(|| Failure::from(format!("{name}: no type table reads it")))
            })
            .collect()
    }
}

/// Whether type `index` is `class` or derives from it.
fn derives(tag: &Tagfile<'_>, mut index: usize, class: &str) -> bool {
    for _ in 0..64 {
        let Some(t) = tag.types.get(index).filter(|_| index != 0) else {
            return false;
        };
        if t.name == class {
            return true;
        }
        index = t.parent;
    }
    false
}

fn sdk(tag: &Tagfile<'_>) -> String {
    String::from_utf8_lossy(&tag.version).into_owned()
}

/// Every element of every item of `tag` whose type derives from `class`: (class name of the element, element).
fn objects_of<'t, 'a>(
    tag: &'t Tagfile<'a>,
    class: &str,
) -> Result<Vec<(&'t str, havok_formats::Value<'t, 'a>)>, Failure> {
    let mut out = Vec::new();
    for i in 1..tag.items.len() {
        let Some(item) = tag.items.get(i) else { break };
        if !derives(tag, item.type_index, class) {
            continue;
        }
        let run = tag.item(i)?;
        for j in 0..run.len() {
            let v = run.get(j)?;
            out.push((v.type_name(), v));
        }
    }
    Ok(out)
}

/// What the `havok-enums` corpus of a game shows of the enumerations of the tagfile classes, counted through the
/// generic value view and named through the crate; and the expected counts, which are the census of the same files.
struct Want {
    game: Game,
    tagfiles: usize,
    shapes: Expected,
    bodies: Expected,
    cloth_shapes: Expected,
    platforms: Expected,
    animations: Expected,
    blends: Expected,
    sections: Expected,
    /// Reference frames (`hkaAnimatedReferenceFrame`), each with the unstored `frameType` reading 0.
    reference_frames: u64,
}

/// The tagfiles of the sekiro corpus.
const SEKIRO_TAGFILES: usize = 24;

/// sekiro: the `hknp` shape objects by SDK, class and stored `type`.
const SEKIRO_SHAPES: &[(&str, u64)] = &[
    ("20160200 fsnpCustomParamCompressedMeshShape 9", 1),
    ("20160200 hknpBoxShape 6", 4),
    ("20160200 hknpCapsuleShape 3", 4),
    ("20160200 hknpCompressedMeshShape 9", 1),
    ("20160200 hknpConvexPolytopeShape 1", 2),
    ("20160200 hknpConvexShape 0", 1),
    ("20160200 hknpExternMeshShape 10", 1),
    ("20160200 hknpSphereShape 2", 1),
    ("20160200 hknpTriangleShape 5", 1),
];

/// sekiro: the `hknp` bodies by SDK, motion type and the mass pattern.
const SEKIRO_BODIES: &[(&str, u64)] = &[
    ("20160200 Dynamic mass=-1", 1),
    ("20160200 Dynamic mass>0", 1),
    ("20160200 Keyframed mass=-1", 5),
    ("20160200 Keyframed mass=0", 2),
    ("20160200 Static mass=-1", 2),
    ("20160200 Static mass=0", 5),
];

/// sekiro: the `hcl` shape objects by SDK and class.
const SEKIRO_CLOTH_SHAPES: &[(&str, u64)] = &[
    ("20160200 hclCapsuleShape", 10),
    ("20160200 hclSphereShape", 1),
    ("20160200 hclTaperedCapsuleShape", 12),
];

/// sekiro: the `hclClothData` objects by SDK and `targetPlatform`.
const SEKIRO_PLATFORMS: &[(&str, u64)] = &[("20160200 2", 21)];

/// sekiro: the animations of the containers by SDK, class and kind.
const SEKIRO_ANIMATIONS: &[(&str, u64)] = &[
    ("20150100 hkaSplineCompressedAnimation SplineCompressed", 1),
    ("20160200 hkaInterleavedUncompressedAnimation Interleaved", 1),
    ("20160200 hkaQuantizedAnimation Quantized", 2),
    ("20160200 hkaSplineCompressedAnimation SplineCompressed", 4),
];

/// sekiro: the bindings of the containers by SDK and blend hint.
const SEKIRO_BLENDS: &[(&str, u64)] = &[
    ("20150100 Normal", 1),
    ("20160200 Additive1", 1),
    ("20160200 Additive2", 1),
    ("20160200 Normal", 5),
];

/// sekiro: the mesh sections by SDK, flags and whether they hold a custom primitive.
const SEKIRO_SECTIONS: &[(&str, u64)] = &[
    ("20160200 flags 0 custom false", 1),
    ("20160200 flags 1 custom true", 1),
];

/// The tagfiles of the elden corpus.
const ELDEN_TAGFILES: usize = 29;

/// elden: the `hknp` shape objects by SDK, class and stored `type`.
const ELDEN_SHAPES: &[(&str, u64)] = &[
    ("20160200 hknpExternMeshShape 10", 1),
    ("20180100 fsnpCustomParamCompressedMeshShape 9", 1),
    ("20180100 hknpBoxShape 6", 9),
    ("20180100 hknpCapsuleShape 3", 2),
    ("20180100 hknpCompressedMeshShape 9", 2),
    ("20180100 hknpConvexPolytopeShape 1", 31),
    ("20180100 hknpConvexShape 0", 1),
    ("20180100 hknpCylinderShape 4", 1),
    ("20180100 hknpExternMeshShape 10", 1),
    ("20180100 hknpSphereShape 2", 2),
    ("20180100 hknpTriangleShape 5", 1),
];

/// elden: the `hknp` bodies by SDK, motion type and the mass pattern.
const ELDEN_BODIES: &[(&str, u64)] = &[
    ("20160200 Static mass=-1", 1),
    ("20180100 Dynamic mass<0 other", 3),
    ("20180100 Dynamic mass=-1", 4),
    ("20180100 Dynamic mass>0", 28),
    ("20180100 Keyframed mass=-1", 2),
    ("20180100 Keyframed mass=0", 3),
    ("20180100 Static mass=-1", 2),
    ("20180100 Static mass=0", 7),
];

/// elden: the `hcl` shape objects by SDK and class.
const ELDEN_CLOTH_SHAPES: &[(&str, u64)] = &[
    ("20180100 hclCapsuleShape", 5),
    ("20180100 hclConvexGeometryShape", 2),
    ("20180100 hclPlaneShape", 1),
    ("20180100 hclSphereShape", 1),
    ("20180100 hclTaperedCapsuleShape", 4),
];

/// elden: the `hclClothData` objects by SDK and `targetPlatform`.
const ELDEN_PLATFORMS: &[(&str, u64)] = &[("20180100 2", 18)];

/// elden: the animations of the containers by SDK, class and kind.
const ELDEN_ANIMATIONS: &[(&str, u64)] = &[
    ("20160200 hkaInterleavedUncompressedAnimation Interleaved", 1),
    ("20180100 hkaInterleavedUncompressedAnimation Interleaved", 1),
    ("20180100 hkaSplineCompressedAnimation SplineCompressed", 3),
];

/// elden: the bindings of the containers by SDK and blend hint.
const ELDEN_BLENDS: &[(&str, u64)] = &[
    ("20160200 Normal", 1),
    ("20180100 Additive1", 1),
    ("20180100 Normal", 3),
];

/// elden: the mesh sections by SDK, flags and whether they hold a custom primitive.
const ELDEN_SECTIONS: &[(&str, u64)] = &[
    ("20180100 flags 0 custom false", 1),
    ("20180100 flags 1 custom true", 1),
];

const WANTS: [Want; 2] = [
    Want {
        game: Game::Sekiro,
        tagfiles: SEKIRO_TAGFILES,
        shapes: SEKIRO_SHAPES,
        bodies: SEKIRO_BODIES,
        cloth_shapes: SEKIRO_CLOTH_SHAPES,
        platforms: SEKIRO_PLATFORMS,
        animations: SEKIRO_ANIMATIONS,
        blends: SEKIRO_BLENDS,
        sections: SEKIRO_SECTIONS,
        reference_frames: 2,
    },
    Want {
        game: Game::Elden,
        tagfiles: ELDEN_TAGFILES,
        shapes: ELDEN_SHAPES,
        bodies: ELDEN_BODIES,
        cloth_shapes: ELDEN_CLOTH_SHAPES,
        platforms: ELDEN_PLATFORMS,
        animations: ELDEN_ANIMATIONS,
        blends: ELDEN_BLENDS,
        sections: ELDEN_SECTIONS,
        reference_frames: 1,
    },
];

/// `hknp` shapes: the `type` of each shape object is the enumeration's value for its class, and the counts are the
/// census. Bodies: the `motionType` is one of the three named values, only a dynamic body has a positive mass and a
/// keyframed one has mass 0 or -1 (the pattern the names were checked against).
#[test]
fn hknp_shapes_and_bodies_have_the_censused_values() -> Result<(), Failure> {
    for want in WANTS {
        let Some(corpus) = corpus(want.game)? else {
            continue;
        };
        let tagfiles = corpus.tagfiles()?;
        assert_eq!(tagfiles.len(), want.tagfiles);
        let (mut shapes, mut bodies) = (Counts::new(), Counts::new());
        for (name, tag) in &tagfiles {
            for (class, shape) in objects_of(tag, "hknpShape")? {
                let stored = shape.field("type")?.signed()?;
                let named = hknp::ShapeType::from_value(stored)
                    .ok_or_else(|| format!("{name}: shape type {stored}"))?;
                assert_eq!(hknp::ShapeType::of_class(class), Some(named), "{name}: {class}");
                count(&mut shapes, format!("{} {class} {stored}", sdk(tag)));
            }
            for (_, body) in objects_of(tag, "hknpBodyCinfo")? {
                let stored = body.field("motionType")?.signed()?;
                let motion = hknp::MotionType::from_value(stored)
                    .ok_or_else(|| format!("{name}: motion type {stored}"))?;
                let mass = body.field("mass")?.float()?;
                let pattern = if mass > 0.0 {
                    "mass>0"
                } else if mass == 0.0 {
                    "mass=0"
                } else if mass == -1.0 {
                    "mass=-1"
                } else {
                    "mass<0 other"
                };
                if mass > 0.0 {
                    assert_eq!(motion, hknp::MotionType::Dynamic, "{name}");
                }
                if motion == hknp::MotionType::Keyframed {
                    assert!(mass == 0.0 || mass == -1.0, "{name}: {mass}");
                }
                count(&mut bodies, format!("{} {motion:?} {pattern}", sdk(tag)));
            }
        }
        assert_eq!(shapes, expected(want.shapes), "{:?}", want.game);
        assert_eq!(bodies, expected(want.bodies), "{:?}", want.game);
    }
    Ok(())
}

/// `hcl`: the shapes' own `type` is not stored (0 in every shape) and the class names the shape type; every
/// `hclClothData` stores the platform 2, `X64`, and `Cloth::platform` and `Shape::shape_type` name them.
#[test]
fn hcl_shapes_and_platforms_have_the_censused_values() -> Result<(), Failure> {
    for want in WANTS {
        let Some(corpus) = corpus(want.game)? else {
            continue;
        };
        let (mut shapes, mut platforms) = (Counts::new(), Counts::new());
        let mut cloths = 0u64;
        for (name, tag) in corpus.tagfiles()? {
            for (class, shape) in objects_of(&tag, "hclShape")? {
                assert_eq!(shape.field("type")?.unsigned()?, 0, "{name}: {class}");
                assert!(hcl::ShapeType::of_class(class).is_some(), "{name}: {class}");
                count(&mut shapes, format!("{} {class}", sdk(&tag)));
            }
            for (_, data) in objects_of(&tag, "hclClothData")? {
                let stored = data.field("targetPlatform")?.signed()?;
                assert_eq!(hcl::Platform::from_value(stored), Some(hcl::Platform::X64));
                count(&mut platforms, format!("{} {stored}", sdk(&tag)));
            }
            if tag.types.iter().any(|t| t.name == "hclClothData") {
                for c in cloth::read(&tag)?.cloths {
                    assert_eq!(c.platform(), Some(hcl::Platform::X64), "{name}");
                    cloths += 1;
                    for sim in &c.sim_cloths {
                        for collidable in &sim.collidables {
                            assert!(
                                collidable.shape.shape_type().is_some(),
                                "{name}: {:?}",
                                collidable.shape
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(shapes, expected(want.cloth_shapes), "{:?}", want.game);
        assert_eq!(platforms, expected(want.platforms), "{:?}", want.game);
        assert_eq!(cloths, platforms.values().sum::<u64>(), "{:?}", want.game);
    }
    Ok(())
}

/// `hka`: each animation of a container has the type its class stores, and each binding a blend hint of 0, 1 or 2,
/// named by `Binding::blend`; a reference frame's unstored `frameType` reads 0. The counts are the census.
#[test]
fn hka_animations_and_bindings_have_the_censused_values() -> Result<(), Failure> {
    for want in WANTS {
        let Some(corpus) = corpus(want.game)? else {
            continue;
        };
        let (mut animations, mut blends) = (Counts::new(), Counts::new());
        let mut frames = 0u64;
        for (name, tag) in corpus.tagfiles()? {
            if tag.types.iter().any(|t| t.name == "hkaAnimationContainer") {
                let c = container(&tag).map_err(|e| format!("{name}: {e}"))?;
                for a in &c.animations {
                    count(&mut animations, format!("{} {} {:?}", sdk(&tag), a.class, a.kind));
                }
                for b in &c.bindings {
                    let hint = b
                        .blend()
                        .ok_or_else(|| format!("{name}: blend hint {}", b.blend_hint))?;
                    count(&mut blends, format!("{} {hint:?}", sdk(&tag)));
                    assert_eq!(
                        hint.name_in(tag.version).is_some(),
                        hint == hka::BlendHint::Normal || tag.version == *b"20180100"
                    );
                }
            }
            for (class, frame) in objects_of(&tag, "hkaAnimatedReferenceFrame")? {
                assert_eq!(frame.field("frameType")?.unsigned()?, 0, "{name}: {class}");
                assert_eq!(
                    hka::ReferenceFrameType::from_value(0),
                    Some(hka::ReferenceFrameType::Unknown)
                );
                frames += 1;
            }
        }
        assert_eq!(animations, expected(want.animations), "{:?}", want.game);
        assert_eq!(blends, expected(want.blends), "{:?}", want.game);
        assert_eq!(frames, want.reference_frames, "{:?}", want.game);
    }
    Ok(())
}

/// Section flags: the flag bit is set on exactly the sections that hold a custom primitive, and `layerData` is 0; the
/// counts are the census (the corpus holds the smallest mesh of each kind). Reading the mesh checks the rest.
#[test]
fn section_flags_follow_the_custom_primitives() -> Result<(), Failure> {
    for want in WANTS {
        let Some(corpus) = corpus(want.game)? else {
            continue;
        };
        let mut sections = Counts::new();
        for (name, tag) in corpus.tagfiles()? {
            for (_, data) in objects_of(&tag, "hknpCompressedMeshShapeData")? {
                let ranges = havok_formats::mesh_sections(data).map_err(|e| format!("{name}: {e}"))?;
                let mesh = compressed_mesh(data).map_err(|e| format!("{name}: {e}"))?;
                for (i, range) in ranges.iter().enumerate() {
                    assert_eq!(range.layer_data, 0, "{name}: section {i}");
                    let custom = mesh.custom.iter().any(|c| usize::try_from(c.key >> 8) == Ok(i));
                    count(
                        &mut sections,
                        format!("{} flags {} custom {custom}", sdk(&tag), range.flags.0),
                    );
                }
            }
        }
        assert_eq!(sections, expected(want.sections), "{:?}", want.game);
    }
    Ok(())
}

/// Member declaration flags: every member of every type table of the corpora has the data-field bit and no bit the
/// installs do not use; the access bits of members that the Havok 6.6 headers declare `public:` (neither bit),
/// `protected:` (bit 1) and `private:` (bit 2) are those; the not-serialisable bit is set on `frameType`, whose value
/// the animation test finds 0 in every reference frame.
#[test]
fn member_flags_name_the_access_the_headers_declare() -> Result<(), Failure> {
    /// (class, member, access in the 6.6 headers, whether the value is written): the headers' sections for the
    /// members both they and the type tables declare; the last four rows are from the data alone (no header
    /// declares `frameType`, `refCount` or `propertyBag` under these names, and `memSizeAndFlags` is `public:` in the
    /// header but private in the tables).
    const MEMBERS: [(&str, &str, DeclAccess, bool); 14] = [
        ("hkRootLevelContainer", "namedVariants", DeclAccess::Public, true),
        ("hkaAnimationContainer", "animations", DeclAccess::Public, true),
        ("hkaAnimationBinding", "blendHint", DeclAccess::Public, true),
        ("hkaAnimation", "type", DeclAccess::Protected, true),
        ("hkaAnimation", "extractedMotion", DeclAccess::Protected, true),
        (
            "hkaSplineCompressedAnimation",
            "numFrames",
            DeclAccess::Private,
            true,
        ),
        (
            "hkaSplineCompressedAnimation",
            "numBlocks",
            DeclAccess::Private,
            true,
        ),
        (
            "hkaSplineCompressedAnimation",
            "blockDuration",
            DeclAccess::Private,
            true,
        ),
        ("hkaSplineCompressedAnimation", "data", DeclAccess::Private, true),
        (
            "hkaSplineCompressedAnimation",
            "endian",
            DeclAccess::Private,
            true,
        ),
        (
            "hkaAnimatedReferenceFrame",
            "frameType",
            DeclAccess::Public,
            false,
        ),
        (
            "hkReferencedObject",
            "memSizeAndFlags",
            DeclAccess::Private,
            false,
        ),
        ("hkReferencedObject", "refCount", DeclAccess::Private, false),
        ("hkReferencedObject", "propertyBag", DeclAccess::Private, true),
    ];
    for want in WANTS {
        let Some(corpus) = corpus(want.game)? else {
            continue;
        };
        let mut seen = [0u64; MEMBERS.len()];
        let mut fields = 0u64;
        for (name, tag) in corpus.tagfiles()? {
            for ty in &tag.types {
                for field in &ty.fields {
                    let decl = field.decl_flags();
                    assert!(
                        decl.contains(DeclFlags::DATA_FIELD),
                        "{name}: {}::{}",
                        ty.name,
                        field.name
                    );
                    assert_eq!(decl.unseen(), 0, "{name}: {}::{}", ty.name, field.name);
                    assert!(decl.access().is_some(), "{name}: {}::{}", ty.name, field.name);
                    fields += 1;
                }
                for (i, (class, member, access, serialized)) in MEMBERS.iter().enumerate() {
                    if ty.name != *class {
                        continue;
                    }
                    let Some(field) = ty.fields.iter().find(|f| f.name == *member) else {
                        continue;
                    };
                    let decl = field.decl_flags();
                    assert_eq!(decl.access(), Some(*access), "{name}: {class}::{member}");
                    assert_eq!(decl.serialized(), *serialized, "{name}: {class}::{member}");
                    if let Some(n) = seen.get_mut(i) {
                        *n += 1;
                    }
                }
            }
        }
        assert!(fields > 1_000, "{:?}: {fields} fields", want.game);
        for ((class, member, ..), n) in MEMBERS.iter().zip(seen) {
            assert!(n > 0, "{:?}: no type table declares {class}::{member}", want.game);
        }
    }
    Ok(())
}

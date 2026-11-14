//! Real-data check of the cloth reader: every distinct tagfile with cloth of each install (149 Sekiro, 814 Elden
//! Ring), extracted from the user's own install (see tests/inputs/README.md); skips without them. No game data is
//! included in the repository.
//!
//! Every file reads without a refusal, so every particle, buffer, transform set, operator and sim cloth index the
//! reader checks is in range in both installs. The object counts equal those of a separate census that walks every
//! `hcl` object of the same files through the generic value view (classes, arrays and pointers, with no cloth
//! knowledge). Collidables are counted as the sim cloths reference them: a collidable object several sim cloths share
//! counts once for each.
mod inputs;

use havok_formats::cloth::{self, ConstraintKind, OperatorKind, Shape};
use havok_formats::{Compendium, Tagfile};
use inputs::{Game, OracleSet, Query, expect_found, resolve};
use std::collections::BTreeMap;

/// What a corpus holds: (name, count), sorted by name.
type Counts = BTreeMap<String, u64>;

/// Named counts as the test states them.
type Expected = &'static [(&'static str, u64)];

fn operator_name(kind: &OperatorKind) -> String {
    match kind {
        OperatorKind::Simulate { .. } => "Simulate".to_owned(),
        OperatorKind::MoveParticles { .. } => "MoveParticles".to_owned(),
        OperatorKind::ObjectSpaceSkin { components, .. } => format!("ObjectSpaceSkin {components:?}"),
        OperatorKind::BoneSpaceSkin { components, .. } => format!("BoneSpaceSkin {components:?}"),
        OperatorKind::ObjectSpaceMeshMeshDeform { components, .. } => {
            format!("ObjectSpaceMeshMeshDeform {components:?}")
        }
        OperatorKind::BoneSpaceMeshMeshDeform { components, .. } => {
            format!("BoneSpaceMeshMeshDeform {components:?}")
        }
        OperatorKind::SimpleMeshBoneDeform { .. } => "SimpleMeshBoneDeform".to_owned(),
        OperatorKind::MeshBoneDeform { .. } => "MeshBoneDeform".to_owned(),
        OperatorKind::CopyVertices { .. } => "CopyVertices".to_owned(),
        OperatorKind::GatherAllVertices { .. } => "GatherAllVertices".to_owned(),
        OperatorKind::GatherSomeVertices { .. } => "GatherSomeVertices".to_owned(),
        OperatorKind::InputConvert { .. } => "InputConvert".to_owned(),
        OperatorKind::OutputConvert { .. } => "OutputConvert".to_owned(),
        OperatorKind::UpdateAllVertexFrames { .. } => "UpdateAllVertexFrames".to_owned(),
        OperatorKind::UpdateSomeVertexFrames { .. } => "UpdateSomeVertexFrames".to_owned(),
        OperatorKind::Other { class } => format!("other {class}"),
    }
}

fn constraint_name(kind: &ConstraintKind) -> String {
    match kind {
        ConstraintKind::StandardLink(_) => "StandardLink".to_owned(),
        ConstraintKind::StretchLink(_) => "StretchLink".to_owned(),
        ConstraintKind::CompressibleLink(_) => "CompressibleLink".to_owned(),
        ConstraintKind::BendLink(_) => "BendLink".to_owned(),
        ConstraintKind::BendStiffness { .. } => "BendStiffness".to_owned(),
        ConstraintKind::LocalRange { .. } => "LocalRange".to_owned(),
        ConstraintKind::Transition { .. } => "Transition".to_owned(),
        ConstraintKind::VolumeMx { .. } => "VolumeMx".to_owned(),
        ConstraintKind::Other { class } => format!("other {class}"),
    }
}

fn shape_name(shape: &Shape) -> String {
    match shape {
        Shape::Sphere { .. } => "Sphere".to_owned(),
        Shape::Capsule { .. } => "Capsule".to_owned(),
        Shape::TaperedCapsule { .. } => "TaperedCapsule".to_owned(),
        Shape::Plane { .. } => "Plane".to_owned(),
        Shape::ConvexGeometry { .. } => "ConvexGeometry".to_owned(),
        Shape::Other { class } => format!("other {class}"),
    }
}

fn add(n: &mut Counts, key: impl Into<String>, count: usize) {
    *n.entry(key.into()).or_default() += u64::try_from(count).unwrap_or(u64::MAX);
}

fn count(physics: &cloth::Physics, n: &mut Counts) {
    for c in &physics.cloths {
        add(n, "cloth datas", 1);
        add(n, "buffers", c.buffers.len());
        add(n, "transform sets", c.transform_sets.len());
        add(n, "states", c.states.len());
        add(n, "state transitions", c.state_transitions.len());
        add(n, "cloth actions", c.actions.len());
        for o in &c.operators {
            add(n, format!("operator {}", operator_name(&o.kind)), 1);
        }
        for s in &c.sim_cloths {
            add(n, "sim cloths", 1);
            add(n, "particles", s.particles.len());
            add(n, "fixed particles", s.fixed_particles.len());
            add(n, "triangle indices", s.triangle_indices.len());
            add(n, "sim cloth actions", s.actions.len());
            add(n, "anti-pinch sets", s.anti_pinch_constraint_sets.len());
            add(
                n,
                "virtual collision points",
                usize::from(s.virtual_collision_points.points),
            );
            for col in &s.collidables {
                add(n, format!("collidable {}", shape_name(&col.shape)), 1);
            }
            for set in &s.constraint_sets {
                add(n, format!("constraint set {}", constraint_name(&set.kind)), 1);
                if set.constraint_id.is_some() {
                    add(n, "constraint sets with an id", 1);
                }
            }
        }
    }
}

/// The counts of each corpus, as the reader and the generic census find them.
fn expected(game: Game) -> Counts {
    let (common, own): (Expected, Expected) = match game {
        Game::Sekiro => (
            &[
                ("files", 149),
                ("cloth datas", 610),
                ("buffers", 3_286),
                ("transform sets", 610),
                ("states", 3_094),
                ("sim cloths", 638),
                ("particles", 194_248),
                ("fixed particles", 49_807),
                ("triangle indices", 638_742),
                ("virtual collision points", 810),
                ("constraint set StandardLink", 565),
                ("constraint set StretchLink", 634),
                ("constraint set CompressibleLink", 78),
                ("constraint set BendLink", 203),
                ("constraint set BendStiffness", 262),
                ("constraint set LocalRange", 592),
                ("constraint set Transition", 638),
                ("constraint set VolumeMx", 24),
                ("operator Simulate", 638),
                ("operator MoveParticles", 639),
                ("operator ObjectSpaceSkin P", 28),
                ("operator ObjectSpaceSkin PN", 443),
                ("operator ObjectSpaceSkin PNT", 629),
                ("operator BoneSpaceSkin P", 7),
                ("operator BoneSpaceSkin PN", 137),
                ("operator BoneSpaceSkin PNT", 90),
                ("operator ObjectSpaceMeshMeshDeform P", 52),
                ("operator ObjectSpaceMeshMeshDeform PN", 180),
                ("operator ObjectSpaceMeshMeshDeform PNT", 24),
                ("operator BoneSpaceMeshMeshDeform P", 13),
                ("operator BoneSpaceMeshMeshDeform PN", 234),
                ("operator BoneSpaceMeshMeshDeform PNT", 15),
                ("operator SimpleMeshBoneDeform", 197),
                ("operator CopyVertices", 944),
                ("operator GatherAllVertices", 515),
                ("operator InputConvert", 579),
                ("operator OutputConvert", 965),
                ("operator UpdateAllVertexFrames", 387),
                ("operator UpdateSomeVertexFrames", 54),
                ("collidable Capsule", 162),
                ("collidable TaperedCapsule", 255),
                ("collidable Sphere", 6),
            ],
            &[],
        ),
        Game::Elden => (
            &[
                ("files", 814),
                ("cloth datas", 2_692),
                ("buffers", 14_028),
                ("transform sets", 2_692),
                ("states", 13_659),
                ("sim cloths", 2_875),
                ("particles", 480_207),
                ("fixed particles", 89_966),
                ("triangle indices", 1_700_280),
                ("virtual collision points", 25_065),
                ("constraint set StandardLink", 2_096),
                ("constraint set StretchLink", 2_807),
                ("constraint set CompressibleLink", 733),
                ("constraint set BendLink", 268),
                ("constraint set BendStiffness", 2_340),
                ("constraint set LocalRange", 2_836),
                ("constraint set Transition", 2_856),
                ("constraint set VolumeMx", 326),
                ("operator Simulate", 2_875),
                ("operator MoveParticles", 2_906),
                ("operator ObjectSpaceSkin P", 28),
                ("operator ObjectSpaceSkin PN", 2_457),
                ("operator ObjectSpaceSkin PNT", 3_177),
                ("operator BoneSpaceSkin P", 7),
                ("operator BoneSpaceSkin PN", 351),
                ("operator BoneSpaceSkin PNT", 297),
                ("operator ObjectSpaceMeshMeshDeform P", 673),
                ("operator ObjectSpaceMeshMeshDeform PN", 1_157),
                ("operator ObjectSpaceMeshMeshDeform PNT", 29),
                ("operator BoneSpaceMeshMeshDeform P", 51),
                ("operator BoneSpaceMeshMeshDeform PN", 103),
                ("operator BoneSpaceMeshMeshDeform PNT", 2),
                ("operator SimpleMeshBoneDeform", 827),
                ("operator MeshBoneDeform", 41),
                ("operator CopyVertices", 3_265),
                ("operator GatherAllVertices", 2_944),
                ("operator GatherSomeVertices", 36),
                ("operator InputConvert", 1_487),
                ("operator OutputConvert", 3_597),
                ("operator UpdateAllVertexFrames", 1_744),
                ("operator UpdateSomeVertexFrames", 534),
                ("collidable Capsule", 2_653),
                ("collidable TaperedCapsule", 2_519),
                ("collidable ConvexGeometry", 117),
                ("collidable Sphere", 7),
                ("collidable Plane", 3),
            ],
            // only Elden Ring's hclConstraintSet stores constraintId
            &[("constraint sets with an id", 14_262)],
        ),
        // the PlayStation 3 game has no tagfiles and no cloth
        Game::Shred => (&[], &[]),
    };
    // neither install has state transitions, actions or anti-pinch sets: they count 0
    let zero = [
        ("state transitions", 0),
        ("cloth actions", 0),
        ("sim cloth actions", 0),
        ("anti-pinch sets", 0),
    ];
    common
        .iter()
        .chain(own)
        .chain(zero.iter())
        .map(|&(k, v)| (k.to_owned(), v))
        .collect()
}

#[test]
fn every_cloth_tagfile_of_both_installs_reads() -> Result<(), Box<dyn std::error::Error>> {
    for game in [Game::Sekiro, Game::Elden] {
        let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-cloth");
        let Some(dir) = resolve(corpus) else {
            continue;
        };
        // manifest.tsv: sha256, file, compendium ("-" for none), binder, member; one row per binder member
        let manifest = std::fs::read_to_string(dir.join("manifest.tsv"))?;
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        for line in manifest.lines().skip(1) {
            let columns: Vec<&str> = line.split('\t').collect();
            let [_, file, compendium, ..] = columns.as_slice() else {
                return Err(format!("manifest row {line:?}").into());
            };
            files.insert((*file).to_owned(), (*compendium).to_owned());
        }
        expect_found(corpus, "cloth tagfiles", files.len());
        let mut n = Counts::new();
        add(&mut n, "files", files.len());
        let mut refusals = Vec::new();
        for (file, compendium) in &files {
            let bytes = std::fs::read(dir.join(file))?;
            let compendium = match compendium.as_str() {
                "-" => None,
                c => Some(Compendium::read(&std::fs::read(dir.join(c))?)?),
            };
            let tag = Tagfile::read(&bytes, compendium.as_ref())?;
            match cloth::read(&tag) {
                Ok(physics) => count(&physics, &mut n),
                Err(e) => refusals.push(format!("{file}: {e}")),
            }
        }
        assert!(
            refusals.is_empty(),
            "{game:?}: {} refused: {refusals:#?}",
            refusals.len()
        );
        assert_eq!(n, expected(game), "{game:?}");
    }
    Ok(())
}

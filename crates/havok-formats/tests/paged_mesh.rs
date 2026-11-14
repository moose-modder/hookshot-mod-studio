//! Real-data check of the section page: the one Sekiro and the one Elden Ring tagfile of the install's low-resolution
//! map collision (`l` sets) with mesh-tree sections of page above 0, extracted from the user's own install (see
//! tests/inputs/README.md); skips without them. No game data is included in the repository.
//!
//! A section's triangles lie inside its `domain` (the box of its triangle vertices grown by the convex radius, which is
//! at least 1 cm in these meshes), so a vertex that lands outside it is another triangle's: a reader that ignores the
//! page takes the shared vertex at `entry` where the section means `page * 65536 + entry`. The counts are those of a
//! census of every map `l` binder of both installs, which decoded each triangle a second, independent way.
mod inputs;

use havok_formats::{Compendium, Tagfile, compressed_mesh, mesh_sections};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// How far outside a section's domain a decoded vertex may lie: float rounding of the domain's corners (the census's
/// worst overshoot of any domain by any triangle of either install is 15 micrometres).
const TOLERANCE: f32 = 1e-3;

/// A game's corpus folder, then what its one tagfile holds: sections of page above 0, and the triangles in them.
const SETS: [(Game, usize, usize); 2] = [(Game::Sekiro, 89, 6_980), (Game::Elden, 6_233, 507_001)];

#[test]
fn paged_sections_decode_inside_their_domains() -> Result<(), Box<dyn std::error::Error>> {
    for (game, want_sections, want_triangles) in SETS {
        let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-l");
        let Some(dir) = resolve(corpus) else {
            continue;
        };
        let mut files = std::fs::read_dir(dir)?
            .map(|f| f.map(|f| f.path()))
            .collect::<Result<Vec<_>, _>>()?;
        files.sort();
        let with = |ext: &str| {
            files
                .iter()
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .collect::<Vec<_>>()
        };
        let compendia = with("compendium")
            .iter()
            .map(|p| Compendium::read(&std::fs::read(p)?).map_err(Box::<dyn std::error::Error>::from))
            .collect::<Result<Vec<_>, _>>()?;
        let tagfiles = with("hkx");
        expect_found(corpus, ".hkx files", tagfiles.len());
        let (mut sections, mut paged_sections, mut triangles, mut paged_triangles, mut outside) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        for path in tagfiles {
            let bytes = std::fs::read(path)?;
            let tag = std::iter::once(None)
                .chain(compendia.iter().map(Some))
                .find_map(|c| Tagfile::read(&bytes, c).ok())
                .ok_or_else(|| format!("{}: no type table reads it", path.display()))?;
            let variants = tag.root()?.field("namedVariants")?.array()?;
            for i in 0..variants.len() {
                let physics = variants.get(i)?.field("variant")?.dereference()?;
                let systems = physics.field("systemDatas")?.array()?;
                for j in 0..systems.len() {
                    let bodies = systems.get(j)?.dereference()?.field("bodyCinfos")?.array()?;
                    for k in 0..bodies.len() {
                        let shape = bodies.get(k)?.field("shape")?.dereference()?;
                        if !shape.has_field("data") {
                            continue;
                        }
                        let data = shape.field("data")?.dereference()?;
                        let ranges = mesh_sections(data)?;
                        sections += ranges.len();
                        paged_sections += ranges.iter().filter(|s| s.page > 0).count();
                        for triangle in compressed_mesh(data)?.triangles {
                            let section = ranges
                                .get(usize::try_from(triangle.key >> 8)?)
                                .ok_or("a triangle key past the sections")?;
                            let [low, high] = section.domain;
                            let lies_outside = triangle.vertices.iter().any(|v| {
                                (0..3).any(|a| v[a] < low[a] - TOLERANCE || v[a] > high[a] + TOLERANCE)
                            });
                            outside += usize::from(lies_outside);
                            triangles += 1;
                            paged_triangles += usize::from(section.page > 0);
                        }
                    }
                }
            }
        }
        eprintln!(
            "{game:?}: {sections} sections, {paged_sections} paged, {triangles} triangles, {paged_triangles} in paged sections, {outside} outside their domain"
        );
        assert_eq!(
            paged_sections, want_sections,
            "{game:?}: sections of page above 0"
        );
        assert_eq!(paged_triangles, want_triangles, "{game:?}: triangles in them");
        assert_eq!(
            outside, 0,
            "{game:?}: triangles with a vertex outside their section's domain"
        );
    }
    Ok(())
}

//! The MOPP codes of Tony Hawk's SHRED (the PS3 packfiles, Havok 7.1.0-r1) on the walker, the query and the mesh check of
//! `havok_formats::mopp`: all 1,705 codes of the 490 level and ragdoll collision files. Skips without the corpus (see
//! tests/inputs/README.md); no game data is in the repository.
//!
//! The counts are those of an independent census of the same files (a Python reader of the packfiles with its own copy of
//! the command table, run before this code was written): every code walks to the end; the terminals of the 1,704 codes
//! over a mesh are exactly the mesh's shape keys without the degenerate triangles; no
//! terminal names a triangle the mesh lacks; and the query with a triangle's own box returns the triangle's key for all
//! 637,213 triangles a code has a terminal for.
mod inputs;

use std::collections::BTreeSet;

use havok_formats::mopp::{SUBPART_INDEX_BITS, check_mesh, query, shape_key, walk};
use havok_formats::packfile::Packfile;
use havok_formats::packfile::physics::{ExtendedMesh, MoppCode, Shape};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// The opcodes the 1,705 codes execute (the census's list): rescale 1-4, jumps 5 and 6, terminal offsets 9-0xB, splits
/// 0x10-0x1C and 0x20-0x25, bounds 0x26-0x28, terminals 0x30-0x4F and 0x50-0x53, and the absolute jump 0x70.
fn expected_opcodes() -> BTreeSet<u8> {
    let mut set = BTreeSet::new();
    set.extend(0x01..=0x06u8);
    set.extend(0x09..=0x0Bu8);
    set.extend(0x10..=0x1Cu8);
    set.extend(0x20..=0x28u8);
    set.extend(0x30..=0x53u8);
    set.insert(0x70);
    set
}

#[test]
fn every_shred_mopp_code_walks_and_agrees_with_its_mesh() -> Result<(), Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, Game::Shred, "havok-packfiles");
    let Some(dir) = resolve(corpus) else {
        return Ok(());
    };
    let mut files = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort();
    expect_found(corpus, "packfiles", files.len());
    let (mut codes, mut code_bytes, mut over_meshes, mut over_lists) = (0usize, 0usize, 0usize, 0usize);
    let (mut triangles, mut terminals, mut degenerate) = (0usize, 0usize, 0usize);
    let (mut unknown, mut degenerate_with_terminal, mut omitted) = (0usize, 0usize, Vec::new());
    let (mut queries, mut found, mut list_terminals) = (0usize, 0usize, 0usize);
    let mut opcodes = BTreeSet::new();
    for path in &files {
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let pack = Packfile::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
        for offset in pack.objects_of("hkpMoppBvTreeShape") {
            let Shape::Mopp { child, code } =
                Shape::read(&pack, offset).map_err(|e| format!("{name}: {e}"))?
            else {
                return Err(format!("{name}: a MOPP shape that is not one").into());
            };
            let code = MoppCode::read(&pack, code).map_err(|e| format!("{name}: {e}"))?;
            let walked = walk(&code.code).map_err(|e| format!("{name}: {e}"))?;
            codes += 1;
            code_bytes += code.code.len();
            opcodes.extend(walked.opcodes.keys().copied());
            if pack.class_at(child)? != "hkpStorageExtendedMeshShape" {
                over_lists += 1;
                list_terminals += walked.terminals.len();
                continue;
            }
            over_meshes += 1;
            let mesh = ExtendedMesh::read(&pack, child).map_err(|e| format!("{name}: {e}"))?;
            let check = check_mesh(&walked, &mesh, SUBPART_INDEX_BITS)?;
            triangles += check.triangles;
            terminals += check.terminals;
            degenerate += check.degenerate;
            unknown += check.unknown_terminals;
            degenerate_with_terminal += check.degenerate_with_terminal;
            omitted.push((name.clone(), check.omitted.len()));
            // the query with a triangle's own box finds the triangle
            for (subpart_index, subpart) in mesh.subparts.iter().enumerate() {
                for (triangle_index, triangle) in subpart.triangles.iter().enumerate() {
                    let key = shape_key(subpart_index, triangle_index, SUBPART_INDEX_BITS)
                        .ok_or("a key past its bits")?;
                    if !walked.terminals.contains(&key) {
                        continue;
                    }
                    let mut low = [f32::MAX; 3];
                    let mut high = [f32::MIN; 3];
                    for &i in triangle {
                        let v = subpart.vertices[i as usize];
                        for axis in 0..3 {
                            low[axis] = low[axis].min(v[axis]);
                            high[axis] = high[axis].max(v[axis]);
                        }
                    }
                    queries += 1;
                    found +=
                        usize::from(query(&code.code, code.offset, code.scale, [low, high])?.contains(&key));
                }
            }
        }
    }
    assert_eq!(codes, 1_705);
    assert_eq!(code_bytes, 5_917_936);
    assert_eq!((over_meshes, over_lists), (1_704, 1));
    // the one code over a list shape (FOXTROT's) has five terminals
    assert_eq!(list_terminals, 5);
    assert_eq!(opcodes, expected_opcodes());
    assert_eq!(triangles, 645_973);
    assert_eq!(terminals, 637_213);
    assert_eq!(degenerate, 8_760);
    assert_eq!(unknown, 0, "terminals that name no triangle");
    assert_eq!(degenerate_with_terminal, 0, "degenerate triangles the codes keep");
    // every key without a terminal is degenerate by the stock test (the five thin triangles the squared cross product alone
    // keeps are collinear by the identity in single precision)
    let leaving: Vec<_> = omitted.iter().filter(|(_, n)| *n > 0).collect();
    assert!(leaving.is_empty(), "{leaving:?}");
    assert_eq!((queries, found), (637_213, 637_213));
    Ok(())
}

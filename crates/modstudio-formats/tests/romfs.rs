//! Checks against a real game dump. They are ignored by default and never run in CI:
//!
//! ```text
//! MODSTUDIO_ROMFS=C:/path/to/romfs cargo test --release -p modstudio-formats --test romfs -- --ignored --nocapture
//! ```
//!
//! They print counts only, never file contents.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use modstudio_formats::romfs::Romfs;

fn romfs() -> Romfs {
    let root = std::env::var_os("MODSTUDIO_ROMFS").expect("set MODSTUDIO_ROMFS to a romfs folder");
    Romfs::open(PathBuf::from(root)).expect("open romfs")
}

fn files_with_suffix(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_with_suffix(&path, suffix, out);
        } else if path.to_string_lossy().ends_with(suffix) {
            out.push(path);
        }
    }
}

/// Runs `check` on every file, on all cores; returns the failures.
fn for_each_file(
    romfs: &Romfs,
    files: &[PathBuf],
    check: impl Fn(&mut modstudio_formats::zs::Decoder, &Path) -> Result<(), String> + Sync,
) -> Vec<(PathBuf, String)> {
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut decoder = romfs.decoder().expect("decoder");
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    if let Err(e) = check(&mut decoder, path) {
                        failures.lock().unwrap().push((path.clone(), e));
                    }
                }
            });
        }
    });
    failures.into_inner().unwrap()
}

fn report(what: &str, total: usize, failures: &[(PathBuf, String)], root: &Path) {
    println!("{what}: {} of {total} ok", total - failures.len());
    for (path, e) in failures.iter().take(20) {
        println!(
            "  {}: {e}",
            path.strip_prefix(root).unwrap_or(path).display()
        );
    }
}

#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn every_zs_file_decompresses() {
    let romfs = romfs();
    println!(
        "dictionaries: {:?}",
        romfs.dictionaries().names().collect::<Vec<_>>()
    );
    let mut files = Vec::new();
    files_with_suffix(romfs.root(), ".zs", &mut files);
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        romfs
            .read(decoder, path)
            .map(drop)
            .map_err(|e| e.to_string())
    });
    report("decompressed", files.len(), &failures, romfs.root());
    assert!(failures.is_empty());
}

#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn texture_survey() {
    use modstudio_formats::bntx::Bntx;
    use std::collections::BTreeMap;
    let romfs = romfs();
    let mut files = Vec::new();
    files_with_suffix(romfs.root(), ".bntx.zs", &mut files);
    let stats = Mutex::new(BTreeMap::<String, usize>::new());
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bntx = Bntx::parse(&data).map_err(|e| e.to_string())?;
        let mut s = stats.lock().unwrap();
        *s.entry(format!("textures per file: {}", bntx.textures.len()))
            .or_default() += 1;
        for t in &bntx.textures {
            *s.entry(format!("format {}", t.format.name())).or_default() += 1;
            *s.entry(format!(
                "tile mode {} dim {} surface dim {}",
                t.tile_mode, t.dim, t.surface_dim
            ))
            .or_default() += 1;
            *s.entry(format!("layers {} depth {}", t.array_length, t.depth))
                .or_default() += 1;
        }
        Ok(())
    });
    for (k, v) in stats.into_inner().unwrap() {
        println!("{v:>7}  {k}");
    }
    report("parsed", files.len(), &failures, romfs.root());
    assert!(failures.is_empty());
}

/// Our block-linear size of each mip level must match the space the file gives it (the next level's offset).
#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn mip_sizes_match_the_files() {
    use modstudio_formats::bntx::Bntx;
    use std::collections::BTreeMap;
    let romfs = romfs();
    let mut files = Vec::new();
    files_with_suffix(romfs.root(), ".bntx.zs", &mut files);
    let mismatches = Mutex::new(BTreeMap::<String, (usize, String)>::new());
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bntx = Bntx::parse(&data).map_err(|e| e.to_string())?;
        for t in &bntx.textures {
            let Ok(_) = t.surface(0) else { continue };
            for level in 0..t.mip_count.saturating_sub(1) {
                let ours = t.surface(level).unwrap().swizzled_size();
                let theirs = t.mip_offsets[level as usize + 1] - t.mip_offsets[level as usize];
                if ours != theirs {
                    let s = t.surface(level).unwrap();
                    let key = format!("{} level {level}", t.format.name());
                    let mut m = mismatches.lock().unwrap();
                    let e = m.entry(key).or_insert((0, String::new()));
                    if e.0 == 0 {
                        e.1 = format!(
                            "{}x{} -> {}x{} texels, block height {} (log2 {}), ours 0x{ours:x} file 0x{theirs:x}",
                            t.width,
                            t.height,
                            s.width,
                            s.height,
                            s.block_height,
                            t.block_height_log2
                        );
                    }
                    e.0 += 1;
                }
            }
        }
        Ok(())
    });
    let m = mismatches.into_inner().unwrap();
    for (k, (n, example)) in &m {
        println!("{n:>6}  {k}: e.g. {example}");
    }
    report("parsed", files.len(), &failures, romfs.root());
    assert!(m.is_empty());
}

/// The top mip level of the first layer of every texture decodes.
#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn every_texture_decodes() {
    use modstudio_formats::bntx::Bntx;
    use std::collections::BTreeMap;
    let romfs = romfs();
    let mut files = Vec::new();
    files_with_suffix(romfs.root(), ".bntx.zs", &mut files);
    let by_error = Mutex::new(BTreeMap::<String, usize>::new());
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bntx = Bntx::parse(&data).map_err(|e| e.to_string())?;
        for t in &bntx.textures {
            if let Err(e) = t.decode(0, 0) {
                let key = e
                    .to_string()
                    .split(": ")
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(": ");
                *by_error.lock().unwrap().entry(key).or_default() += 1;
                return Err(e.to_string());
            }
        }
        Ok(())
    });
    for (k, v) in by_error.into_inner().unwrap() {
        println!("{v:>6}  {k}");
    }
    report("decoded", files.len(), &failures, romfs.root());
}

fn external_strings(romfs: &Romfs) -> modstudio_formats::bfres::ExternalStrings {
    let mut decoder = romfs.decoder().unwrap();
    let data = romfs
        .read(&mut decoder, "Shader/ExternalBinaryString.bfres.zs")
        .unwrap();
    modstudio_formats::bfres::ExternalStrings::parse(&data).unwrap()
}

/// Every model parses, and every attribute of every vertex buffer and every LOD's indices read.
#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn model_survey() {
    use modstudio_formats::bfres::Bfres;
    use std::collections::BTreeMap;
    let romfs = romfs();
    let strings = external_strings(&romfs);
    println!("external strings: {}", strings.len());
    let mut files = Vec::new();
    files_with_suffix(&romfs.root().join("Model"), ".bfres.zs", &mut files);
    files.retain(|p| !p.to_string_lossy().ends_with(".anim.bfres.zs"));
    let stats = Mutex::new(BTreeMap::<String, usize>::new());
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bfres = Bfres::parse(&data, Some(&strings)).map_err(|e| e.to_string())?;
        let mut s = Vec::new();
        s.push(format!(
            "version {:?} flags 0x{:02x}",
            bfres.version, bfres.external_flags
        ));
        s.push(format!("models per file {}", bfres.models.len()));
        for m in &bfres.models {
            for vb in &m.vertex_buffers {
                for a in &vb.attributes {
                    s.push(format!("attribute {} {}", a.name, a.format.name()));
                    vb.read(a).map_err(|e| format!("{}: {e}", m.name))?;
                }
            }
            if m.shapes.iter().any(|sh| sh.is_meshlet()) {
                let file = path.file_name().unwrap().to_string_lossy().into_owned();
                let kind = file.split('_').next().unwrap_or("").to_owned();
                let all = m.shapes.iter().all(|sh| sh.is_meshlet());
                s.push(format!(
                    "meshlet shapes ({}): {kind}",
                    if all { "all" } else { "some" }
                ));
            }
            for sh in &m.shapes {
                s.push(format!(
                    "shape flags 0x{:x} skin {} keys {}",
                    sh.flags, sh.skin_count, sh.key_shape_count
                ));
                // Unskinned shapes bound to a bone that isn't at the origin (their vertices are in bone space).
                let b = &m.skeleton.bones[sh.bone];
                if sh.skin_count == 0
                    && !sh.meshes.is_empty()
                    && (b.parent.is_some() || b.translation != [0.0; 3])
                {
                    let file = path.file_name().unwrap().to_string_lossy().into_owned();
                    let kind = file.split('_').next().unwrap_or("").to_owned();
                    s.push(format!("shapes with skin 0 on a moved bone: {kind}"));
                }
                s.push(format!("lods {}", sh.meshes.len()));
                for (i, mesh) in sh.meshes.iter().enumerate() {
                    s.push(format!(
                        "mesh primitive {} index format {}",
                        mesh.primitive, mesh.index_format
                    ));
                    let idx = mesh
                        .indices()
                        .map_err(|e| format!("{} {}: {e}", m.name, sh.name))?;
                    let n = m.vertex_buffers[sh.vertex_buffer].vertex_count;
                    if let Some(bad) = idx.iter().find(|&&v| v >= n) {
                        return Err(format!(
                            "{} {} lod {i}: index {bad} of {n} vertices",
                            m.name, sh.name
                        ));
                    }
                }
            }
            for mat in &m.materials {
                if mat.textures.len() != mat.samplers.len() {
                    s.push("material: texture and sampler counts differ".to_owned());
                }
                if mat.name.starts_with("ExtStr_")
                    || mat.textures.iter().any(|t| t.starts_with("ExtStr_"))
                {
                    s.push("unresolved external string".to_owned());
                }
            }
        }
        let mut st = stats.lock().unwrap();
        for k in s {
            *st.entry(k).or_default() += 1;
        }
        Ok(())
    });
    for (k, v) in stats.into_inner().unwrap() {
        println!("{v:>8}  {k}");
    }
    report("models", files.len(), &failures, romfs.root());
}

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
                if let Some(mesh) = sh.meshes.first() {
                    let n = mesh.sub_meshes.len();
                    s.push(format!("sub meshes in LOD 0: {}", n.min(5)));
                    if n > 1 && n < 4 {
                        let file = path.file_name().unwrap().to_string_lossy().into_owned();
                        let index = m.shapes.iter().position(|x| std::ptr::eq(x, sh)).unwrap();
                        println!("multi sub-mesh: {file} shape {index} ({n})");
                    }
                }
                let expected: usize = sh.meshes.iter().map(|m| m.sub_meshes.len() + 1).sum();
                if !sh.meshes.is_empty() && sh.boundings.len() != expected {
                    s.push("bounding count differs".to_owned());
                }
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

/// Writing a model back without changes gives the same bytes; writing every shape back with its own decoded
/// geometry changes nothing but the recomputed bounds, and those only within float rounding.
#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn models_write_back_identically() {
    use modstudio_formats::bfres::Bfres;
    use modstudio_formats::bfres_write::{ShapeGeometry, write};
    use std::collections::{BTreeMap, BTreeSet};
    let romfs = romfs();
    let strings = external_strings(&romfs);
    let mut files = Vec::new();
    files_with_suffix(&romfs.root().join("Model"), ".bfres.zs", &mut files);
    files.retain(|p| !p.to_string_lossy().ends_with(".anim.bfres.zs"));
    if let Ok(filter) = std::env::var("MODSTUDIO_FILTER") {
        files.retain(|p| p.to_string_lossy().contains(&filter));
    }
    let stats = Mutex::new(BTreeMap::<String, usize>::new());
    let worst = Mutex::new((0f32, String::new()));
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bfres = Bfres::parse(&data, Some(&strings)).map_err(|e| e.to_string())?;
        let same = match write(&bfres, &BTreeMap::new()) {
            Ok(out) => out == data,
            Err(e) => {
                if std::env::var_os("MODSTUDIO_VERBOSE").is_some() {
                    println!("not written: {}: {e}", path.display());
                }
                *stats
                    .lock()
                    .unwrap()
                    .entry(format!("not written: {e}"))
                    .or_default() += 1;
                return Ok(());
            }
        };
        if !same {
            return Err("unchanged write differs".into());
        }
        // Every shape with its own geometry.
        let m = &bfres.models[0];
        let mut replace = BTreeMap::new();
        for (i, s) in m.shapes.iter().enumerate() {
            replace.insert(
                i,
                ShapeGeometry::from_shape(m, s).map_err(|e| e.to_string())?,
            );
        }
        let out = write(&bfres, &replace).map_err(|e| format!("own geometry: {e}"))?;
        if out.len() != data.len() {
            return Err(format!(
                "own geometry: {} bytes instead of {}",
                out.len(),
                data.len()
            ));
        }
        let mut bounds = BTreeSet::new();
        for s in &m.shapes {
            for k in 0..s.boundings.len() * 24 {
                bounds.insert(s.bounding_at + k);
            }
            for k in 0..s.spheres.len() * 16 {
                bounds.insert(s.radius_at + k);
            }
        }
        if let Some(at) = (0..data.len()).find(|&i| data[i] != out[i] && !bounds.contains(&i)) {
            return Err(format!(
                "own geometry: byte 0x{at:x} differs outside the bounds"
            ));
        }
        // Bounds: largest difference relative to the shape's size.
        let after = Bfres::parse(&out, Some(&strings)).map_err(|e| e.to_string())?;
        let mut file_worst = 0f32;
        for (a, b) in m.shapes.iter().zip(&after.models[0].shapes) {
            let scale = a
                .boundings
                .iter()
                .flat_map(|x| x.extent)
                .fold(1e-3f32, f32::max);
            for (x, y) in a.boundings.iter().zip(&b.boundings) {
                for c in 0..3 {
                    file_worst = file_worst.max((x.center[c] - y.center[c]).abs() / scale);
                    file_worst = file_worst.max((x.extent[c] - y.extent[c]).abs() / scale);
                }
            }
            for (x, y) in a.spheres.iter().zip(&b.spheres) {
                for c in 0..4 {
                    file_worst = file_worst.max((x[c] - y[c]).abs() / scale);
                }
            }
            if std::env::var_os("MODSTUDIO_VERBOSE").is_some() {
                let off = |p: &[f32], q: &[f32]| {
                    p.iter()
                        .zip(q)
                        .map(|(u, v)| (u - v).abs())
                        .fold(0f32, f32::max)
                        / scale
                };
                for (k, (x, y)) in a.boundings.iter().zip(&b.boundings).enumerate() {
                    if off(&x.center, &y.center).max(off(&x.extent, &y.extent)) > 1e-3 {
                        println!(
                            "{} skin {} box {k}: stored {x:?} computed {y:?}",
                            a.name, a.skin_count
                        );
                        break;
                    }
                }
                for (k, (x, y)) in a.spheres.iter().zip(&b.spheres).enumerate() {
                    if off(x, y) > 1e-3 {
                        println!(
                            "{} skin {} sphere {k} (of {}): stored {x:?} computed {y:?}",
                            a.name,
                            a.skin_count,
                            a.spheres.len()
                        );
                        break;
                    }
                }
            }
        }
        let bucket = match file_worst {
            w if w < 1e-5 => "bounds within 1e-5",
            w if w < 1e-3 => "bounds within 1e-3",
            w if w < 1e-1 => "bounds within 1e-1",
            _ => "bounds off by more than 10%",
        };
        *stats.lock().unwrap().entry(bucket.to_owned()).or_default() += 1;
        let mut w = worst.lock().unwrap();
        if file_worst > w.0 {
            *w = (
                file_worst,
                path.file_name().unwrap().to_string_lossy().into_owned(),
            );
        }
        Ok(())
    });
    for (k, v) in stats.into_inner().unwrap() {
        println!("{v:>6}  {k}");
    }
    println!("worst bounds: {:?}", worst.into_inner().unwrap());
    report("written back", files.len(), &failures, romfs.root());
    assert!(failures.is_empty());
}

/// How the game's textures set block height, flags and layout fields (to write new ones the same way).
#[test]
#[ignore = "needs a game dump (MODSTUDIO_ROMFS)"]
fn texture_layout_survey() {
    use modstudio_formats::bntx::Bntx;
    use std::collections::BTreeMap;
    let romfs = romfs();
    let mut files = Vec::new();
    files_with_suffix(&romfs.root().join("Tex"), ".bntx.zs", &mut files);
    let stats = Mutex::new(BTreeMap::<String, usize>::new());
    let failures = for_each_file(&romfs, &files, |decoder, path| {
        let data = romfs.read(decoder, path).map_err(|e| e.to_string())?;
        let bntx = Bntx::parse(&data).map_err(|e| e.to_string())?;
        let t = &bntx.textures[0];
        let Some((_, bh)) = t.format.block_size() else {
            return Ok(());
        };
        let rows = t.height.div_ceil(bh);
        // The usual rule (tegra_swizzle's block_height_mip0): rows + rows / 2 against 16 * 8, 8 * 8, ...
        let rule = match rows + rows / 2 {
            r if r >= 128 => 4,
            r if r >= 64 => 3,
            r if r >= 32 => 2,
            r if r >= 16 => 1,
            _ => 0,
        };
        let brti = data.as_slice();
        let at = u64::from_le_bytes(brti[0x28..0x30].try_into().unwrap()) as usize;
        let at = u64::from_le_bytes(brti[at..at + 8].try_into().unwrap()) as usize;
        let flags = brti[at + 0x10];
        let access = u32::from_le_bytes(brti[at + 0x20..at + 0x24].try_into().unwrap());
        let layout2 = u32::from_le_bytes(brti[at + 0x38..at + 0x3C].try_into().unwrap());
        let kind = format!("{:?}", t.format.encoding().unwrap()).to_lowercase();
        let mut s = stats.lock().unwrap();
        let rows_bucket = match rows {
            r if r <= 8 => "<=8",
            r if r <= 16 => "<=16",
            r if r <= 64 => "<=64",
            r if r <= 256 => "<=256",
            _ => ">256",
        };
        *s.entry(format!(
            "log2 {} vs rule {rule} (rows {rows_bucket})",
            t.block_height_log2
        ))
        .or_default() += 1;
        *s.entry(format!(
            "flags {flags} access 0x{access:x} layout2 0x{layout2:x} ({kind})"
        ))
        .or_default() += 1;
        Ok(())
    });
    for (k, v) in stats.into_inner().unwrap() {
        println!("{v:>7}  {k}");
    }
    report("textures", files.len(), &failures, romfs.root());
}

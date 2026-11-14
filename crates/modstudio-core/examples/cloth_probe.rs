//! How far the vendored havok-formats and havok-cloth get with the game's cloth files: each `Phive/Cloth/*.bphcl`
//! is split (Phive header), its tagfile read, its cloth data read, and every cloth stepped for a while with the
//! bones at rest. Files are grouped by where they stop (numbers in messages left out), with the first message of
//! each group in full.
//!
//!     cargo run --release -p modstudio-core --example cloth_probe -- <romfs> [<part of a name>] [--steps N] [--preview]
//!
//! `--preview`: each file through `modstudio_core::cloth::simulate` instead (the test motion), with how far its
//! bones get from their rest place in the moving model (a simulation that runs off shows as metres), and the
//! warnings, grouped.

use std::collections::BTreeMap;
use std::path::Path;

use havok_cloth::instance::ClothInstance;
use havok_formats::{Tagfile, cloth};
use modstudio_formats::phive::Phive;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(romfs) = args.first() else {
        eprintln!("usage: cloth_probe <romfs> [<part of a name>] [--steps N]");
        std::process::exit(2);
    };
    let steps: usize = args
        .iter()
        .position(|a| a == "--steps")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);
    let filter = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let preview = args.iter().any(|a| a == "--preview");
    let mut warnings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let dir = Path::new(romfs).join("Phive").join("Cloth");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "bphcl")
                && filter
                    .as_ref()
                    .is_none_or(|f| p.to_string_lossy().contains(f.as_str()))
        })
        .collect();
    files.sort();
    let mut groups: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let name = name.trim_end_matches(".Nin_Ounce_NVN2.bphcl").to_owned();
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let outcome = if preview {
            let (outcome, notes) = probe_preview(&bytes, &name);
            for n in notes {
                warnings.entry(n).or_default().push(name.clone());
            }
            outcome
        } else {
            probe(&bytes, steps)
        };
        let entry = groups
            .entry(without_numbers(&outcome))
            .or_insert_with(|| (outcome.clone(), Vec::new()));
        entry.1.push(name);
    }
    let mut sorted: Vec<_> = groups.into_values().collect();
    sorted.sort_by_key(|(_, names)| std::cmp::Reverse(names.len()));
    for (first, names) in sorted {
        println!(
            "{:4}  {first}\n      e.g. {}",
            names.len(),
            names.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
        );
    }
    // CLOTH_DEPTHS: every collidable some particles start more than 1 cm inside at rest, with the model's size
    if std::env::var_os("CLOTH_DEPTHS").is_some() {
        for path in &files {
            let bytes = std::fs::read(path).unwrap_or_default();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let name = name.trim_end_matches(".Nin_Ounce_NVN2.bphcl");
            let Ok(phive) = Phive::parse(&bytes) else {
                continue;
            };
            let Ok(tag) = Tagfile::read(phive.tagfile, None) else {
                continue;
            };
            let Ok(physics) = cloth::read(&tag) else {
                continue;
            };
            let Ok(container) = havok_formats::animation::container(&tag) else {
                continue;
            };
            for data in &physics.cloths {
                let rest: Vec<_> = data
                    .transform_sets
                    .iter()
                    .filter_map(|set| container.skeletons.iter().find(|s| s.name == set.name))
                    .map(modstudio_core::cloth::rest_pose)
                    .collect();
                if rest.len() != data.transform_sets.len() {
                    continue;
                }
                let size = rest
                    .iter()
                    .map(|r| modstudio_core::cloth::extent(r))
                    .fold(0.0, f32::max);
                for (sim, depths) in data
                    .sim_cloths
                    .iter()
                    .zip(modstudio_core::cloth::start_depths(data, &rest))
                {
                    let skeletons: Vec<_> = data
                        .transform_sets
                        .iter()
                        .filter_map(|set| container.skeletons.iter().find(|s| s.name == set.name))
                        .collect();
                    for (i, (c, d)) in sim.collidables.iter().zip(depths).enumerate() {
                        // the bone it's on, and whether that bone is a root (no parent)
                        let bone = sim.collidable_transform_map.as_ref().and_then(|m| {
                            let s = skeletons.get(m.transform_set as usize)?;
                            let b = usize::try_from(*m.transform_indices.get(i)?).ok()?;
                            Some((s.bones.get(b)?.name.clone(), *s.parents.get(b)? < 0))
                        });
                        let on = match &bone {
                            Some((b, true)) => format!("on ROOT {b}"),
                            Some((b, false)) => format!("on {b}"),
                            None => "on no bone".to_owned(),
                        };
                        if d > 0.01 || matches!(bone, Some((_, true))) {
                            println!(
                                "depth {:7.3} m ({:5.1}% of {:5.2} m)  {name} {} {} {on}",
                                d,
                                100.0 * d / size,
                                size,
                                data.name,
                                c.name
                            );
                        }
                    }
                }
            }
        }
    }
    for (note, names) in warnings {
        println!(
            "note ({}): {note}
      {}",
            names.len(),
            names.join(", ")
        );
    }
}

/// What the preview made of a file: its parts and bones, and how far a bone gets from its rest place in the moving
/// model; and its warnings, the part names left out.
fn probe_preview(bytes: &[u8], name: &str) -> (String, Vec<String>) {
    let p = match modstudio_core::cloth::simulate(bytes, name) {
        Ok(p) => p,
        Err(e) => return (format!("preview: {e}"), Vec::new()),
    };
    let mut far = 0f32;
    let mut worst = String::new();
    for b in &p.bones {
        for (f, w) in b.frames.iter().enumerate() {
            let m = &p.motion[f];
            // the motion is rigid: the model's place is its transpose rotation times (world - translation)
            let d: Vec<f32> = (0..3).map(|i| w[12 + i] - m[12 + i]).collect();
            let local: Vec<f32> = (0..3)
                .map(|c| (0..3).map(|i| m[c * 4 + i] * d[i]).sum())
                .collect();
            let off = (0..3)
                .map(|i| (local[i] - b.rest[12 + i]).powi(2))
                .sum::<f32>()
                .sqrt();
            if off > far {
                far = off;
                worst = format!("{} at frame {f}", b.name);
            }
        }
    }
    if far > 1.0 && std::env::var_os("CLOTH_FAR").is_some() {
        eprintln!("{name}: {far:.2} m, {worst}");
    }
    let notes = p
        .warnings
        .iter()
        .map(|w| {
            w.split_once(": ")
                .map_or(w.clone(), |(_, rest)| rest.to_owned())
        })
        .collect();
    // in words, so that files group by it
    let reach = match far {
        f if f < 0.25 => "bones stay within 25 cm of rest",
        f if f < 0.5 => "bones stay within 50 cm of rest",
        f if f < 1.0 => "bones stay within a metre of rest",
        f if f < 2.0 => "bones go up to two metres from rest",
        _ => "bones go further than two metres from rest (runs off?)",
    };
    (
        format!(
            "preview: {} part(s), {} bone(s), {reach}",
            p.parts.len(),
            p.bones.len()
        ),
        notes,
    )
}

/// Where a file stops, or what it ran.
fn probe(bytes: &[u8], steps: usize) -> String {
    let phive = match Phive::parse(bytes) {
        Ok(p) => p,
        Err(e) => return format!("phive: {e}"),
    };
    let tag = match Tagfile::read(phive.tagfile, None) {
        Ok(t) => t,
        Err(e) => return format!("tagfile: {e}"),
    };
    let physics = match cloth::read(&tag) {
        Ok(p) => p,
        Err(e) => return format!("cloth data: {e}"),
    };
    let mut sims = 0;
    let mut particles = 0;
    for data in &physics.cloths {
        let mut instance = match ClothInstance::new(data) {
            Ok(i) => i,
            Err(e) => return format!("instance: {e}"),
        };
        for frame in 0..steps {
            if let Err(e) = instance.step(1.0 / 30.0, &mut []) {
                return format!("step {frame}: {e}");
            }
        }
        for (sim, state) in data.sim_cloths.iter().zip(&instance.sims) {
            if let Some(i) = state.positions.iter().position(|p| !p.is_finite3()) {
                return format!("simulated: {} particle {i} not finite", sim.name);
            }
            sims += 1;
            particles += state.positions.len();
        }
    }
    format!(
        "ran {steps} steps: {} cloth data, {sims} simulated meshes, {particles} particles",
        physics.cloths.len()
    )
}

/// The message with every number (decimal, or hexadecimal after `0x`) as `#`.
fn without_numbers(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '0' && chars.get(i + 1) == Some(&'x') {
            i += 2;
            while chars.get(i).is_some_and(char::is_ascii_hexdigit) {
                i += 1;
            }
            out.push('#');
        } else if chars[i].is_ascii_digit() {
            while chars.get(i).is_some_and(char::is_ascii_digit) {
                i += 1;
            }
            out.push('#');
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

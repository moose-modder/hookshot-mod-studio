//! Real-data check of the spline block writer, the encoder and the animation writer on the animation tagfiles of the
//! corpus (the SDK 2015.1 clip of Sekiro, and the cutscene files of both games: see tests/inputs/README.md); skips
//! without them. No game data is included in the repository.
//!
//! The oracle is the games' own files: every stored animation's blocks are written back byte for byte; a re-encode of
//! its decoded frames with its own parameters gets its header bit for bit and stays near its frames; the file written
//! with its own animation put back is the file, and with the re-encoded one reads back as exactly that.
mod inputs;

use havok_formats::Tagfile;
use havok_formats::animation::{
    Animation, AnimationType, Container, QsTransform, Tracks, container, replace_animation,
};
use havok_formats::spline::{Absent, EncodeParams, SplineAnimation, SplineBlocks, TrackParams, encode};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

type Files = Vec<(String, Vec<u8>)>;

/// The tagfiles of a corpus set, labelled `<label>/<file name>`.
fn files(corpus: Query, label: &str) -> Result<Files, Box<dyn std::error::Error>> {
    let Some(dir) = resolve(corpus) else {
        return Ok(Vec::new());
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "hkx"));
    paths.sort();
    expect_found(corpus, ".hkx files", paths.len());
    let mut out = Vec::new();
    for p in paths {
        let name = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        out.push((format!("{label}/{name}"), std::fs::read(p)?));
    }
    Ok(out)
}

/// Every animation tagfile of the corpus that is configured: (label, bytes).
fn corpus() -> Result<Files, Box<dyn std::error::Error>> {
    let mut all = files(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-2015"),
        "sekiro/havok-2015",
    )?;
    all.extend(files(
        Query::Oracle(OracleSet::Corpus, Game::Sekiro, "havok-float"),
        "sekiro/havok-float",
    )?);
    all.extend(files(
        Query::Oracle(OracleSet::Corpus, Game::Elden, "havok-float"),
        "elden/havok-float",
    )?);
    Ok(all)
}

const STILL: QsTransform = QsTransform {
    translation: [0.0; 4],
    rotation: [0.0, 0.0, 0.0, 1.0],
    scale: [1.0, 1.0, 1.0, 0.0],
};

/// The animation's own parameters: each track's quantizations from its first block, tolerance 0.001, degree 3,
/// its frames a block, and the guard bytes for an SDK 2018.1 file.
fn own_params(a: &Animation, version: &[u8; 8]) -> Result<EncodeParams, Box<dyn std::error::Error>> {
    let stored = SplineBlocks::read(a)?;
    let Tracks::Spline(s) = &a.tracks else {
        return Err("not spline".into());
    };
    let first = stored.blocks.first().ok_or("no block")?;
    Ok(EncodeParams {
        max_frames_per_block: s.max_frames_per_block,
        transform: first
            .tracks
            .iter()
            .map(|t| TrackParams {
                translation_quantization: t.position.quantization,
                rotation_quantization: t.rotation.quantization,
                scale_quantization: t.scale.quantization,
                ..TrackParams::default()
            })
            .collect(),
        float: vec![TrackParams::default(); a.float_tracks],
        guard: version == b"20180100",
        ..EncodeParams::uniform(TrackParams::default(), 0, 0, false)
    })
}

/// The animation re-encoded from its own decoded frames, or `None` when it stores values no frame can be decoded from.
fn re_encoded(
    a: &Animation,
    version: &[u8; 8],
) -> Result<Option<(Animation, Animation)>, Box<dyn std::error::Error>> {
    let decoded = SplineAnimation::parse(a)?;
    if !decoded.unreadable.is_empty() {
        return Ok(None);
    }
    let frames = decoded.interleaved(&vec![STILL; a.transform_tracks], Absent::NEUTRAL)?;
    let source = Animation {
        class: "hkaInterleavedUncompressedAnimation".into(),
        kind: AnimationType::Interleaved,
        tracks: Tracks::Interleaved(frames),
        ..a.clone()
    };
    let out = encode(&source, &own_params(a, version)?)?;
    Ok(Some((source, out)))
}

#[test]
fn stored_blocks_write_back_byte_for_byte() -> Result<(), Box<dyn std::error::Error>> {
    let mut animations = 0;
    for (label, bytes) in corpus()? {
        let tag = Tagfile::read(&bytes, None)?;
        for a in &container(&tag)?.animations {
            let Tracks::Spline(s) = &a.tracks else { continue };
            let written = SplineBlocks::read(a)?.write()?;
            assert!(written.data == s.data, "{label}: the data differs");
            assert_eq!(written.block_offsets, s.block_offsets, "{label}");
            assert_eq!(written.float_block_offsets, s.float_block_offsets, "{label}");
            assert_eq!(
                written.mask_and_quantization_size, s.mask_and_quantization_size,
                "{label}"
            );
            animations += 1;
        }
    }
    eprintln!("{animations} spline animations written back byte for byte");
    Ok(())
}

#[test]
fn decoded_frames_re_encode_with_the_stored_header_and_near_the_frames()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut animations, mut frames) = (0, 0usize);
    let (mut position, mut rotation) = (0.0f64, 0.0f64);
    let (mut position_sum, mut rotation_sum) = (0.0f64, 0.0f64);
    for (label, bytes) in corpus()? {
        let tag = Tagfile::read(&bytes, None)?;
        for a in &container(&tag)?.animations {
            let Tracks::Spline(stock) = &a.tracks else {
                continue;
            };
            let Some((source, out)) = re_encoded(a, &tag.version)? else {
                continue;
            };
            let Tracks::Spline(new) = &out.tracks else {
                return Err("not spline".into());
            };
            // the header: the stored f32 arithmetic of the frame count and duration
            assert_eq!(
                (
                    new.num_frames,
                    new.num_blocks,
                    new.mask_and_quantization_size,
                    new.frame_duration.to_bits(),
                    new.block_duration.to_bits(),
                    new.block_inverse_duration.to_bits()
                ),
                (
                    stock.num_frames,
                    stock.num_blocks,
                    stock.mask_and_quantization_size,
                    stock.frame_duration.to_bits(),
                    stock.block_duration.to_bits(),
                    stock.block_inverse_duration.to_bits()
                ),
                "{label}"
            );
            let Tracks::Interleaved(src) = &source.tracks else {
                return Err("not interleaved".into());
            };
            let back = SplineAnimation::parse(&out)?
                .interleaved(&vec![STILL; a.transform_tracks], Absent::NEUTRAL)?;
            for (x, y) in src.transforms.iter().zip(&back.transforms) {
                let d = |u: [f32; 4], v: [f32; 4], n: usize| {
                    (0..n)
                        .map(|i| (f64::from(u[i]) - f64::from(v[i])).powi(2))
                        .sum::<f64>()
                        .sqrt()
                };
                let flip = if (0..4).map(|i| x.rotation[i] * y.rotation[i]).sum::<f32>() < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                let p = d(x.translation, y.translation, 3);
                let r = d(x.rotation, y.rotation.map(|c| c * flip), 4);
                position = position.max(p);
                rotation = rotation.max(r);
                position_sum += p;
                rotation_sum += r;
                frames += 1;
            }
            animations += 1;
        }
    }
    if frames == 0 {
        return Ok(());
    }
    let (position_mean, rotation_mean) = (position_sum / frames as f64, rotation_sum / frames as f64);
    eprintln!(
        "{animations} animations re-encoded: position error mean {position_mean:.3e} max {position:.3e}; rotation (4D) mean {rotation_mean:.3e} max {rotation:.3e}"
    );
    // the tolerance is 0.001 on every part; on average a frame comes back well within it
    assert!(position_mean < 1e-3 && rotation_mean < 1e-3);
    Ok(())
}

/// Everything of the container but the animation `index`.
fn others(
    c: &Container,
    index: usize,
) -> (
    Vec<havok_formats::animation::Skeleton>,
    Vec<havok_formats::animation::Binding>,
    Vec<Animation>,
) {
    let mut animations = c.animations.clone();
    animations.remove(index);
    (c.skeletons.clone(), c.bindings.clone(), animations)
}

#[test]
fn an_animation_is_written_into_its_file() -> Result<(), Box<dyn std::error::Error>> {
    let mut written = 0;
    for (label, bytes) in corpus()? {
        let tag = Tagfile::read(&bytes, None)?;
        let c = container(&tag)?;
        for (i, a) in c.animations.iter().enumerate() {
            if !matches!(a.tracks, Tracks::Spline(_)) {
                continue;
            }
            // its own animation put back: the file itself
            assert!(
                replace_animation(&tag, i, a)?.write()? == bytes,
                "{label}: put back, it differs"
            );
            // the re-encoded one: read back as itself, the rest of the container as it was
            let Some((_, new)) = re_encoded(a, &tag.version)? else {
                continue;
            };
            let out = replace_animation(&tag, i, &new)?.write()?;
            let back_tag = Tagfile::read(&out, None)?;
            let back = container(&back_tag)?;
            assert_eq!(back.animations[i], new, "{label}: animation {i}");
            assert_eq!(others(&back, i), others(&c, i), "{label}");
            written += 1;
        }
    }
    eprintln!("{written} re-encoded animations written into their files");
    Ok(())
}

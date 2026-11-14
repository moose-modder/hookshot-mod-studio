//! The spline encoder against `CompressAnim.exe` (Havok 2010.2.0-r1, the closed 32-bit compressor some modding tools
//! bundle): synthetic animations (made here with exact f32 arithmetic only, so every machine makes the same frames)
//! were given to it, and what it wrote is kept in tests/compress_anim/<case>.txt. Encoded with
//! [`EncodeParams::compress_anim`], every case has CompressAnim's header, layout, flags and knots, two come out byte for
//! byte and the others differ in the last bits of fitted values; encoded with the games' flavor, every stored part has
//! the same flags and knots.
//!
//! How the files were made: `COMPRESS_ANIM_INPUTS=<folder> cargo test --test compress_anim -- --ignored write_inputs`
//! writes each case's frames (every value the hex of its f32 bits); each was written as a Havok 2010 packfile (one
//! `hkaInterleavedUncompressedAnimation`, duration `(frames - 1) / 30` stored as an f32, bound to bones 0 to tracks - 1)
//! and compressed with the command line in the file's first line, `CompressAnim.exe <in> <out> <rotation code> 0.001`
//! (code 1 THREECOMP40, 2 THREECOMP48, 5 uncompressed; every other parameter the tool's default). Each file holds the
//! header fields of the `hkaSplineCompressedAnimation` it wrote (floats as the hex of their bits) and its `data` as hex.
//! They are the tool's output on inputs made here, not game data.
#![allow(
    clippy::unwrap_used,
    reason = "a fixture that does not parse or encode is the test's failure"
)]
use havok_formats::animation::{Animation, AnimationType, InterleavedData, QsTransform, Tracks};
use havok_formats::spline::{
    EncodeParams, Knots, QuaternionData, RotationQuantization, ScalarData, SplineBlocks, TrackParams,
    VectorData, encode,
};

/// A case: its name, its frames (`tracks` transforms a frame), and the rotation quantization CompressAnim was asked
/// for (with tolerance 0.001).
struct Case {
    name: &'static str,
    tracks: usize,
    frames: Vec<QsTransform>,
    code: u8,
}

fn normalized(q: [f32; 4]) -> [f32; 4] {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    q.map(|c| c / length)
}

fn transform(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> QsTransform {
    QsTransform {
        translation: [t[0], t[1], t[2], 0.0],
        rotation: r,
        scale: [s[0], s[1], s[2], 0.0],
    }
}

/// A smooth track at `t` in 0..1: a cubic position, a turning rotation, unit scale.
fn smooth(t: f32, k: f32) -> QsTransform {
    transform(
        [t * t * t * 2.0 - t * k, 0.5 + 0.25 * t, -0.125 * t * t],
        normalized([0.1 * t + 0.05 * k, 0.2 - 0.1 * t * t, 0.05 * k, 1.0]),
        [1.0, 1.0, 1.0],
    )
}

/// A small deterministic noise in -0.5..0.5 (a linear congruential sequence).
fn noise(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state >> 8) as f32 / 16_777_216.0 - 0.5
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let single: Vec<QsTransform> = (0..61).map(|f| smooth(f as f32 / 60.0, 1.0)).collect();
    for (name, code) in [("single-40", 1u8), ("single-48", 2), ("single-128", 5)] {
        out.push(Case {
            name,
            tracks: 1,
            frames: single.clone(),
            code,
        });
    }
    // four tracks: a static position; the 90 degree turn about x (its x and w are equal); the identity; a static
    // scale of 2
    let half = 0.5f32.sqrt();
    let mut statics = Vec::new();
    for f in 0..40 {
        let t = f as f32 / 39.0;
        statics.push(transform([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0]));
        statics.push(transform(
            [0.0, 0.0, 0.0],
            [half, 0.0, 0.0, half],
            [1.0, 1.0, 1.0],
        ));
        statics.push(smooth(t, 2.0));
        statics.push(transform([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [2.0, 2.0, 2.0]));
    }
    out.push(Case {
        name: "statics",
        tracks: 4,
        frames: statics,
        code: 1,
    });
    // 300 frames: two blocks
    out.push(Case {
        name: "blocks",
        tracks: 1,
        frames: (0..300).map(|f| smooth(f as f32 / 299.0, 0.5)).collect(),
        code: 1,
    });
    // noise larger than the tolerance: more control points than frames at degree 3, so degree 1
    let mut state = 7u32;
    let noisy = (0..50)
        .map(|f| {
            let t = f as f32 / 49.0;
            let mut x = smooth(t, 1.0);
            x.translation[0] += noise(&mut state) * 0.02;
            x.translation[2] += noise(&mut state) * 0.02;
            x
        })
        .collect();
    out.push(Case {
        name: "noisy",
        tracks: 1,
        frames: noisy,
        code: 1,
    });
    out
}

/// The duration the compressor's input file holds: `(frames - 1) / 30` computed in f64 and stored as an f32.
fn duration(frames: usize) -> f32 {
    ((frames - 1) as f64 / 30.0) as f32
}

fn source(case: &Case) -> Animation {
    Animation {
        class: "hkaInterleavedUncompressedAnimation".into(),
        kind: AnimationType::Interleaved,
        duration: duration(case.frames.len() / case.tracks),
        transform_tracks: case.tracks,
        float_tracks: 0,
        extracted_motion: None,
        annotation_tracks: Vec::new(),
        tracks: Tracks::Interleaved(InterleavedData {
            transforms: case.frames.clone(),
            floats: Vec::new(),
        }),
    }
}

fn quantization(code: u8) -> RotationQuantization {
    match code {
        1 => RotationQuantization::ThreeComp40,
        2 => RotationQuantization::ThreeComp48,
        _ => RotationQuantization::Uncompressed,
    }
}

/// What CompressAnim wrote for a case, as kept in tests/compress_anim/<name>.txt: `key value...` lines.
struct Written {
    duration: u32,
    num_frames: u32,
    num_blocks: u32,
    mask_and_quantization_size: u32,
    block_duration: u32,
    block_inverse_duration: u32,
    frame_duration: u32,
    block_offsets: Vec<u32>,
    float_block_offsets: Vec<u32>,
    data: Vec<u8>,
}

fn written(name: &str) -> Written {
    let path = format!("{}/tests/compress_anim/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let line = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(' ')))
            .unwrap_or_else(|| panic!("{name}: no {key}"))
    };
    let hex32 = |key: &str| u32::from_str_radix(line(key), 16).unwrap();
    let int = |key: &str| line(key).parse::<u32>().unwrap();
    let ints = |key: &str| {
        line(key)
            .split_whitespace()
            .map(|v| v.parse::<u32>().unwrap())
            .collect::<Vec<_>>()
    };
    let data = line("data");
    Written {
        duration: hex32("duration"),
        num_frames: int("numFrames"),
        num_blocks: int("numBlocks"),
        mask_and_quantization_size: int("maskAndQuantizationSize"),
        block_duration: hex32("blockDuration"),
        block_inverse_duration: hex32("blockInverseDuration"),
        frame_duration: hex32("frameDuration"),
        block_offsets: ints("blockOffsets"),
        float_block_offsets: ints("floatBlockOffsets"),
        data: (0..data.len() / 2)
            .map(|i| u8::from_str_radix(&data[2 * i..2 * i + 2], 16).unwrap())
            .collect(),
    }
}

/// The stored blocks of `ours`, and of `ours` with CompressAnim's data put in.
fn both(ours: &Animation, w: &Written) -> (SplineBlocks, SplineBlocks) {
    let mut theirs = ours.clone();
    if let Tracks::Spline(t) = &mut theirs.tracks {
        t.data = w.data.clone();
        t.block_offsets = w.block_offsets.clone();
        t.float_block_offsets = w.float_block_offsets.clone();
    }
    (
        SplineBlocks::read(ours).unwrap(),
        SplineBlocks::read(&theirs).unwrap(),
    )
}

fn rotation_knots(r: &QuaternionData) -> Option<Knots> {
    match r {
        QuaternionData::Spline { knots, .. } => Some(knots.clone()),
        _ => None,
    }
}

/// Every part of `a` has the flags and knots of the same part of `b`.
fn same_parts(a: &SplineBlocks, b: &SplineBlocks, name: &str) {
    assert_eq!(a.blocks.len(), b.blocks.len(), "{name}");
    for (x, y) in a.blocks.iter().zip(&b.blocks) {
        for (s, t) in x.tracks.iter().zip(&y.tracks) {
            assert_eq!(s.position.flags, t.position.flags, "{name}");
            assert_eq!(s.position.knots, t.position.knots, "{name}");
            assert_eq!(s.scale.flags, t.scale.flags, "{name}");
            assert_eq!(s.scale.knots, t.scale.knots, "{name}");
            assert_eq!(s.rotation.flags, t.rotation.flags, "{name}");
            assert_eq!(
                rotation_knots(&s.rotation.value),
                rotation_knots(&t.rotation.value),
                "{name}"
            );
        }
    }
}

/// Two stored vectors with the same flags differ only in the last bits of their values: bounds and statics within
/// `1e-6` (relative to 1 or their size), integers within 2 steps.
fn near_vector(a: &VectorData, b: &VectorData, name: &str) {
    let close = |x: f32, y: f32| (x - y).abs() <= 1e-6 * x.abs().max(1.0);
    for (x, y) in a.components.iter().zip(&b.components) {
        match (x, y) {
            (ScalarData::Static(x), ScalarData::Static(y)) => assert!(close(*x, *y), "{name}: {x} {y}"),
            (
                ScalarData::Spline {
                    min: m0,
                    max: x0,
                    values: v0,
                },
                ScalarData::Spline {
                    min: m1,
                    max: x1,
                    values: v1,
                },
            ) => {
                assert!(
                    close(*m0, *m1) && close(*x0, *x1),
                    "{name}: bounds {m0} {x0} {m1} {x1}"
                );
                assert!(
                    v0.iter().zip(v1).all(|(p, q)| p.abs_diff(*q) <= 2),
                    "{name}: {v0:?} {v1:?}"
                );
            }
            (x, y) => assert_eq!(x, y, "{name}"),
        }
    }
}

/// Encoded with CompressAnim's parameters and arithmetic ([`EncodeParams::compress_anim`]), every case has its header
/// and block layout bit for bit and the flags and knots of every part; `statics` (four tracks: static, a 90 degree turn
/// whose x and w are equal, a spline, a static scale) and `noisy` (degree 1) come out byte for byte. In the others a
/// spline's values differ in their last bits only, the fit being computed in f32 here and in the x87's extended
/// precision there: bounds within 1e-6, integers within 2 steps, quaternions within one step.
#[test]
fn compress_anim_flavor_writes_what_compress_anim_wrote() {
    let mut identical = Vec::new();
    for case in cases() {
        let w = written(case.name);
        let a = source(&case);
        assert_eq!(
            a.duration.to_bits(),
            w.duration,
            "{}: the input's duration",
            case.name
        );
        let params = EncodeParams::compress_anim(quantization(case.code), 0.001, case.tracks, 0);
        let out = encode(&a, &params).unwrap();
        let Tracks::Spline(d) = &out.tracks else {
            panic!("spline")
        };
        assert_eq!(
            (d.num_frames, d.num_blocks, d.mask_and_quantization_size),
            (w.num_frames, w.num_blocks, w.mask_and_quantization_size),
            "{}",
            case.name
        );
        assert_eq!(
            (
                d.block_duration.to_bits(),
                d.block_inverse_duration.to_bits(),
                d.frame_duration.to_bits()
            ),
            (w.block_duration, w.block_inverse_duration, w.frame_duration),
            "{}: header",
            case.name
        );
        assert_eq!(d.block_offsets, w.block_offsets, "{}", case.name);
        assert_eq!(d.float_block_offsets, w.float_block_offsets, "{}", case.name);
        if d.data == w.data {
            identical.push(case.name);
            continue;
        }
        let (ours, theirs) = both(&out, &w);
        same_parts(&ours, &theirs, case.name);
        for (x, y) in ours.blocks.iter().zip(&theirs.blocks) {
            for (s, t) in x.tracks.iter().zip(&y.tracks) {
                near_vector(&s.position, &t.position, case.name);
                near_vector(&s.scale, &t.scale, case.name);
                if let (QuaternionData::Spline { points: p, .. }, QuaternionData::Spline { points: q, .. }) =
                    (&s.rotation.value, &t.rotation.value)
                {
                    for (u, v) in p.iter().zip(q) {
                        let (u, v) = (u.unpack(), v.unpack());
                        let far = (0..4).map(|i| (u[i] - v[i]).abs()).fold(0.0f32, f32::max);
                        assert!(far < 4e-4, "{}: {u:?} {v:?}", case.name);
                    }
                }
            }
        }
    }
    assert_eq!(identical, ["statics", "noisy"]);
}

/// Encoded with the games' flavor, every part of every case has CompressAnim's flags and knots: the two differ in
/// arithmetic, not in what they decide to store.
#[test]
fn games_flavor_stores_the_same_parts() {
    for case in cases() {
        let w = written(case.name);
        let ours = encode(
            &source(&case),
            &EncodeParams::uniform(
                TrackParams::compress_anim(quantization(case.code), 0.001),
                case.tracks,
                0,
                false,
            ),
        )
        .unwrap();
        let (a, b) = both(&ours, &w);
        same_parts(&a, &b, case.name);
    }
}

/// Writes the cases as CompressAnim's inputs (see the module documentation); does nothing unless asked.
#[test]
#[ignore = "writes the compressor's inputs when COMPRESS_ANIM_INPUTS names a folder"]
fn write_inputs() {
    let Ok(dir) = std::env::var("COMPRESS_ANIM_INPUTS") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for case in cases() {
        let hex = |v: f32| format!("\"{:08x}\"", v.to_bits());
        let frames: Vec<String> = case
            .frames
            .chunks(case.tracks)
            .map(|frame| {
                let tracks: Vec<String> = frame
                    .iter()
                    .map(|x| {
                        let v = [
                            x.translation[0],
                            x.translation[1],
                            x.translation[2],
                            x.rotation[0],
                            x.rotation[1],
                            x.rotation[2],
                            x.rotation[3],
                            x.scale[0],
                            x.scale[1],
                            x.scale[2],
                        ];
                        format!("[{}]", v.map(hex).join(","))
                    })
                    .collect();
                format!("[{}]", tracks.join(","))
            })
            .collect();
        let json = format!(
            "{{\"frame_rate\": 30.0, \"tracks\": {}, \"frames\": [{}]}}",
            case.tracks,
            frames.join(",")
        );
        std::fs::write(format!("{dir}/{}-r{}.json", case.name, case.code), json).unwrap();
    }
}

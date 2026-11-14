//! Real-data check of float tracks and of non-finite rotation control points: five cutscene tagfiles (two Sekiro, three
//! Elden Ring) extracted from the user's own install (see tests/inputs/README.md); skips without them. No game data is
//! included in the repository.
//!
//! The expected values come from a census that walked every spline animation of both installs from the block layout
//! alone, without this crate: the camera's field of view (the one float track of each cutscene camera animation), the
//! control points and bounds of its spline blocks, and the one control point of four NaNs in each of the two Elden Ring
//! wing-bone rotation splines.
mod inputs;

use havok_formats::Tagfile;
use havok_formats::animation::{Animation, Container, Tracks, container};
use havok_formats::spline::{FloatTrack, Rotation, SplineAnimation};
use inputs::{Game, OracleSet, Query, expect_found, resolve};

/// A folder's tagfiles: file name and bytes.
type Files = Vec<(String, Vec<u8>)>;

/// The tagfiles of a game's `havok-float` folder, by file name.
fn files(game: Game) -> Result<Option<Files>, Box<dyn std::error::Error>> {
    let corpus = Query::Oracle(OracleSet::Corpus, game, "havok-float");
    let Some(dir) = resolve(corpus) else {
        return Ok(None);
    };
    let mut paths = std::fs::read_dir(dir)?
        .map(|f| f.map(|f| f.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    expect_found(corpus, ".hkx files", paths.len());
    paths
        .into_iter()
        .map(|p| {
            let name = p
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            Ok((name, std::fs::read(p)?))
        })
        .collect::<Result<Vec<_>, std::io::Error>>()
        .map(Some)
        .map_err(Into::into)
}

/// A file of `files` by the start of its name, read.
fn read<'a>(
    files: &'a [(String, Vec<u8>)],
    name: &str,
) -> Result<(Tagfile<'a>, Container), Box<dyn std::error::Error>> {
    let (_, bytes) = files
        .iter()
        .find(|(n, _)| n.starts_with(name))
        .ok_or_else(|| format!("no file {name}"))?;
    let tag = Tagfile::read(bytes, None)?;
    let c = container(&tag)?;
    Ok((tag, c))
}

fn parse(a: &Animation) -> SplineAnimation {
    SplineAnimation::parse(a).unwrap_or_else(|e| panic!("{e}"))
}

/// The spline block `b` of the float track of `s`, as (degree, control points, first, last).
fn spline(s: &SplineAnimation, b: usize) -> (u8, usize, f32, f32) {
    let Some(FloatTrack::Spline { knots, points }) = s.floats.get(b).and_then(|f| f.first()) else {
        panic!("block {b} is not a float spline: {:?}", s.floats.get(b));
    };
    (knots.degree, points.len(), points[0], points[points.len() - 1])
}

fn static_value(s: &SplineAnimation, b: usize) -> f32 {
    let Some(FloatTrack::Static(v)) = s.floats.get(b).and_then(|f| f.first()) else {
        panic!("block {b} is not a static float: {:?}", s.floats.get(b));
    };
    *v
}

/// The animation with float tracks of a container: its index, and that it is the only one.
fn float_animation(c: &Container) -> usize {
    let with: Vec<usize> = (0..c.animations.len())
        .filter(|&i| c.animations[i].float_tracks > 0)
        .collect();
    assert_eq!(with.len(), 1);
    with[0]
}

#[test]
fn sekiro_cutscene_cameras_have_a_field_of_view_track() -> Result<(), Box<dyn std::error::Error>> {
    let Some(files) = files(Game::Sekiro)? else {
        return Ok(());
    };
    // s11_00_0020, cut0010: two spline blocks of 4 control points, degree 3 (mask 0x12), the first ending where the
    // second begins (a seam of 4e-6 in a value of 0.93)
    let (_, c) = read(&files, "s11_00_0020")?;
    assert_eq!(c.animations.len(), 2);
    let i = float_animation(&c);
    assert_eq!(i, 1);
    let a = &c.animations[i];
    assert_eq!((a.transform_tracks, a.float_tracks), (1, 1));
    let s = parse(a);
    assert_eq!((s.floats.len(), s.num_frames), (2, 301));
    let (degree, points, first, last) = spline(&s, 0);
    assert_eq!((degree, points), (3, 4));
    assert_eq!(first, 0.691_111_207_008_361_8_f64 as f32);
    assert_eq!(last, 0.934_289_082_560_776_1_f64 as f32);
    let (degree, points, first, last) = spline(&s, 1);
    assert_eq!((degree, points), (3, 4));
    assert_eq!(first, 0.934_292_793_273_925_8_f64 as f32);
    assert_eq!(last, 0.950_021_564_960_479_7_f64 as f32);
    // the binding drives the one float slot of the camera skeleton, `cam0000_FOV`, whose reference value is 0
    let binding = c
        .bindings
        .iter()
        .find(|b| b.animation == Some(i))
        .ok_or("no binding")?;
    assert_eq!(binding.float_track_to_slot, [0]);
    let skeleton = c
        .skeletons
        .iter()
        .find(|k| k.name == binding.original_skeleton_name)
        .ok_or("no skeleton")?;
    assert_eq!(skeleton.float_slots, ["cam0000_FOV"]);
    assert_eq!(skeleton.reference_floats, [0.0]);
    // the whole clip: the first value at time 0, the last at its duration
    let end = a.duration;
    assert_eq!(s.sample_float(0, 0.0)?, first_of(&s, 0));
    assert!((s.sample_float(0, end)? - last).abs() < 1e-6);
    assert_eq!(s.floats_at(0.0, binding, skeleton)?, [first_of(&s, 0)]);

    // s11_01_0031, cut0000: a spline of 34 control points, then a static value
    let (_, c) = read(&files, "s11_01_0031")?;
    assert_eq!(c.animations.len(), 4);
    let i = float_animation(&c);
    assert_eq!(i, 3);
    let s = parse(&c.animations[i]);
    let (degree, points, first, last) = spline(&s, 0);
    assert_eq!((degree, points), (3, 34));
    assert_eq!(first, 0.889_096_402_402_072_f64 as f32);
    assert_eq!(last, 0.750_858_921_182_760_7_f64 as f32);
    assert_eq!(static_value(&s, 1), 0.750_860_810_279_846_2_f64 as f32);
    Ok(())
}

fn first_of(s: &SplineAnimation, b: usize) -> f32 {
    spline(s, b).2
}

#[test]
fn elden_ring_cutscene_cameras_have_a_field_of_view_track() -> Result<(), Box<dyn std::error::Error>> {
    let Some(files) = files(Game::Elden)? else {
        return Ok(());
    };
    // s13_00_0040, cut0040: one block, a degree 1 spline of 12 control points falling from 0.4975 to 0.4879
    let (_, c) = read(&files, "s13_00_0040")?;
    assert_eq!(c.animations.len(), 6);
    let i = float_animation(&c);
    assert_eq!(i, 1);
    let s = parse(&c.animations[i]);
    let (degree, points, first, last) = spline(&s, 0);
    assert_eq!((s.floats.len(), degree, points), (1, 1, 12));
    assert_eq!(first, 0.497_480_455_284_904_9_f64 as f32);
    assert_eq!(last, 0.487_926_423_542_265_2_f64 as f32);
    for a in &c.animations {
        if matches!(a.tracks, Tracks::Spline(_)) {
            parse(a);
        }
    }
    Ok(())
}

/// The control point of four NaNs, in the track of the wing bone `R_Wing_12`, of each of two cutscene animations.
#[test]
fn a_rotation_control_point_of_nan_is_stored_data() -> Result<(), Box<dyn std::error::Error>> {
    let Some(files) = files(Game::Elden)? else {
        return Ok(());
    };
    // (the file, its animations, the block and control point of the NaN, and the camera's field of view in each block)
    let cases: [(&str, usize, usize, usize, &[f64]); 2] = [
        (
            "s12_05_0020-cut0040",
            21,
            0,
            180,
            &[
                1.068_683_385_848_999,
                1.068_683_385_848_999,
                1.068_684_458_732_605,
            ],
        ),
        ("s12_05_0020-cut0060", 7, 1, 36, &[0.559_768_497_943_878_2; 3]),
    ];
    for (name, animations, block, point, fov) in cases {
        let (_, c) = read(&files, name)?;
        assert_eq!(c.animations.len(), animations, "{name}");
        // every spline animation of the tagfile reads, the 225-track one with the NaN among them
        let mut named = Vec::new();
        for a in &c.animations {
            let Tracks::Spline(_) = &a.tracks else { continue };
            let s = parse(a);
            named.extend(s.unreadable.iter().cloned().map(|u| (a.transform_tracks, u)));
        }
        assert_eq!(named.len(), 1, "{name}: {named:?}");
        let (tracks, u) = &named[0];
        assert_eq!(
            (*tracks, u.block, u.track, u.part, u.point),
            (225, block, 190, "rotation", Some(point)),
            "{name}"
        );

        let a = &c.animations[0];
        let s = parse(a);
        // the track drives `R_Wing_12`, a support bone of the skeleton the binding names
        let binding = c
            .bindings
            .iter()
            .find(|b| b.animation == Some(0))
            .ok_or("no binding")?;
        let skeleton = c
            .skeletons
            .iter()
            .find(|k| k.name == binding.original_skeleton_name)
            .ok_or("no skeleton")?;
        let bone = usize::try_from(binding.transform_track_to_bone[190])?;
        assert!(
            skeleton.bones[bone].name.ends_with("R_Wing_12"),
            "{}",
            skeleton.bones[bone].name
        );
        // the stored point is four quiet NaNs, 0x7FC00000 each, among finite unit quaternions
        let Rotation::Spline { knots, points } = &s.blocks[block][190].rotation else {
            return Err("not a rotation spline".into());
        };
        assert_eq!(points[point].map(f32::to_bits), [0x7FC0_0000; 4]);
        assert_eq!(
            points.iter().filter(|q| q.iter().any(|v| !v.is_finite())).count(),
            1,
            "{name}"
        );
        for q in points
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != point)
            .map(|(_, q)| q)
        {
            let norm = q.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>().sqrt();
            assert!((norm - 1.0).abs() < 1e-6, "{name}: {q:?}");
        }
        // a pose in a span that does not use the point reads; where the point counts, sampling says so
        let frame_time = |frame: u8| (block as f32 * 255.0 + f32::from(frame)) * s.frame_duration;
        let pose = |frame: u8| {
            s.sample(
                190,
                frame_time(frame),
                &skeleton.reference_pose[bone],
                havok_formats::spline::Absent::NEUTRAL,
            )
        };
        // a degree 1 spline interpolates its control points: point `i` is at knot `i + 1`
        let at_point = *knots.values.get(point + 1).ok_or("no knot")?;
        assert!(pose(at_point).is_err(), "{name}");
        assert!(pose(knots.values[1]).is_ok(), "{name}");
        // the camera's field of view in the same tagfile is static in every block
        let fov_animation = c
            .animations
            .iter()
            .position(|a| a.float_tracks > 0)
            .ok_or("no float track")?;
        let f = parse(&c.animations[fov_animation]);
        assert_eq!(f.floats.len(), fov.len());
        for (b, v) in fov.iter().enumerate() {
            assert_eq!(static_value(&f, b), *v as f32, "{name} block {b}");
        }
    }
    Ok(())
}

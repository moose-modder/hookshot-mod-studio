//! One bone's curves in an animation, raw: each curve's header fields, its frames, and the first words of its key
//! data as f32 and as u16 (for working out a key layout).
//!
//!     cargo run --release -p modstudio-formats --example anim_curve_dump -- <romfs> <anim file> <anim name> <bone>

use modstudio_formats::bfres::ExternalStrings;
use modstudio_formats::bfres_anim;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, file, name, bone] = args.as_slice() else {
        eprintln!("usage: anim_curve_dump <romfs> <anim file> <anim name> <bone>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let data = romfs.read(&mut d, file).unwrap();
    let anim = bfres_anim::find(&data, Some(&strings), name).unwrap();
    println!(
        "{name}: flags 0x{:x}, frames {}, curve count {}, baked size {}, {} bones",
        anim.flags,
        anim.frame_count,
        anim.curve_count,
        anim.baked_size,
        anim.bones.len()
    );
    let mut total = 0;
    for (i, b) in anim.bones.iter().enumerate() {
        let kinds = b
            .curves
            .iter()
            .map(|c| c.target & 0x30)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if i < 3 || b.name == *bone || (kinds >= 2 && i < 200) {
            println!(
                "    #{i} {:20} flags 0x{:08x} begin rotate {} translate {} base translate {} curve {} | curves {:?} (sum before: {total}) at 0x{:x}",
                b.name,
                b.flags,
                b.begin_rotate,
                b.begin_translate,
                b.begin_base_translate,
                b.begin_curve,
                b.curves
                    .iter()
                    .map(|c| format!("{:x}@{:x}", c.target, c.at))
                    .collect::<Vec<_>>(),
                b.at
            );
        }
        total += b.curves.len();
    }
    println!("    curves in all: {total}");
    // `*f32`: the first bone with a scalar curve stored as f32
    let b = if bone == "*f32" {
        anim.bones
            .iter()
            .find(|b| {
                b.curves
                    .iter()
                    .any(|c| (c.flags >> 2) & 3 == 0 && c.flags & 0x80 == 0)
            })
            .expect("a bone with an f32 scalar curve")
    } else {
        anim.bones.iter().find(|b| &b.name == bone).expect("bone")
    };
    println!(
        "{bone}: flags 0x{:x}, base rotation {:?}, base translation {:?}",
        b.flags, b.base_rotation, b.base_translation
    );
    let u64_at = |at: usize| u64::from_le_bytes(data[at..at + 8].try_into().unwrap()) as usize;
    let f32_at = |at: usize| f32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let u16_at = |at: usize| u16::from_le_bytes(data[at..at + 2].try_into().unwrap());
    for c in &b.curves {
        let frames_at = u64_at(c.at);
        let keys_at = u64_at(c.at + 8);
        let n = u16_at(c.at + 0x12) as usize;
        println!(
            "curve target 0x{:x} flags 0x{:x}: {n} keys, frames {}..{}, scale {} offset {} delta {}; frames at 0x{frames_at:x}, keys at 0x{keys_at:x} ({} bytes between)",
            c.target,
            c.flags,
            c.start_frame,
            c.end_frame,
            c.scale,
            c.offset,
            c.delta,
            keys_at as i64 - frames_at as i64
        );
        println!("    frames: {:?}", &c.frames[..c.frames.len().min(12)]);
        let floats: Vec<f32> = (0..32).map(|i| f32_at(keys_at + i * 4)).collect();
        println!(
            "    keys as f32: {:?}",
            floats
                .iter()
                .map(|v| (v * 10000.0).round() / 10000.0)
                .collect::<Vec<_>>()
        );
        let words: Vec<u16> = (0..32).map(|i| u16_at(keys_at + i * 2)).collect();
        println!("    keys as u16: {words:?}");
    }
}

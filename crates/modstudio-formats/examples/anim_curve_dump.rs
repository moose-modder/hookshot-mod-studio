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
    let b = anim.bones.iter().find(|b| &b.name == bone).expect("bone");
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

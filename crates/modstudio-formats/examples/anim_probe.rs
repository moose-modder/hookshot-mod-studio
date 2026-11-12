//! Do a model's animation files store its bones' rest translations? Counts, per bone of the model's skeleton,
//! where its local translation (three f32) occurs in the decompressed animation file. Found for nearly every
//! bone, many times: the animations hold each bone's own translation (absolute local transforms), so a bone
//! moved in the rest pose is put back where the game has it while an animation plays (local analysis for the
//! build: moved bones are reported, not written).
//!
//! `cargo run --release --example anim_probe -- <romfs> <model.bfres.zs> <anim.bfres.zs>`

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, model, anim] = args.as_slice() else {
        eprintln!("usage: anim_probe <romfs> <model> <anim>");
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
    let data = romfs.read(&mut d, model).expect("model");
    let bfres = Bfres::parse(&data, Some(&strings)).expect("bfres");
    let anim = romfs.read(&mut d, anim).expect("anim");
    let bones = &bfres.models[0].skeleton.bones;
    let (mut found, mut zero, mut total) = (0, 0, 0);
    let mut counts = Vec::new();
    for b in bones {
        let t = b.translation;
        if t.iter().all(|&x| x == 0.0) {
            zero += 1;
            continue;
        }
        total += 1;
        let pattern: Vec<u8> = t.iter().flat_map(|x| x.to_le_bytes()).collect();
        let n = anim
            .windows(12)
            .filter(|w| *w == pattern.as_slice())
            .count();
        if n > 0 {
            found += 1;
        }
        counts.push((n, b.name.clone()));
    }
    counts.sort();
    println!(
        "{} bones ({zero} at the origin); of the other {total}, {found} have their rest translation in the animation file",
        bones.len()
    );
    println!("fewest: {:?}", &counts[..counts.len().min(6)]);
    println!("most:   {:?}", &counts[counts.len().saturating_sub(4)..]);
}

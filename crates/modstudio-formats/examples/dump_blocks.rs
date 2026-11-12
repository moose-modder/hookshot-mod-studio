//! Writes a texture's top mip level as stored blocks in row order (unswizzled), for local debugging.
//!
//! `cargo run --release --example dump_blocks -- <romfs> <Tex/name.bntx.zs> <out.bin>`

use modstudio_formats::bntx::Bntx;
use modstudio_formats::romfs::Romfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [romfs, file, out] = args.as_slice() else {
        eprintln!("usage: dump_blocks <romfs> <file> <out>");
        std::process::exit(2);
    };
    let romfs = Romfs::open(romfs).expect("romfs");
    let data = romfs
        .read(&mut romfs.decoder().unwrap(), file)
        .expect("read");
    let bntx = Bntx::parse(&data).expect("bntx");
    let t = &bntx.textures[0];
    let blocks = t
        .surface(0)
        .unwrap()
        .deswizzle(t.level_bytes(0, 0).unwrap())
        .unwrap();
    std::fs::write(out, blocks).expect("write");
    println!("{} {}x{} {}", t.name, t.width, t.height, t.format.name());
}

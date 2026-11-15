//! An actor pack's files, and the BYML ones whose path matches a filter printed as text.
//!
//!     cargo run --release -p modstudio-core --example pack_dump -- <romfs> <actor> [path filter]

use modstudio_formats::byml::{self, Value};
use modstudio_formats::romfs::Romfs;
use modstudio_formats::sarc::Sarc;

fn show(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Map(entries) => {
            for (k, x) in entries {
                match x {
                    Value::Map(_) | Value::Array(_) => {
                        out.push_str(&format!("{pad}{k}:\n"));
                        show(x, depth + 1, out);
                    }
                    other => out.push_str(&format!("{pad}{k}: {other:?}\n")),
                }
            }
        }
        Value::Array(items) => {
            for (i, x) in items.iter().enumerate() {
                out.push_str(&format!("{pad}- [{i}]\n"));
                show(x, depth + 1, out);
            }
        }
        other => out.push_str(&format!("{pad}{other:?}\n")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, actor, rest @ ..] = args.as_slice() else {
        eprintln!("usage: pack_dump <romfs> <actor> [path filter]");
        std::process::exit(2);
    };
    let filter = rest.first().cloned().unwrap_or_default();
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let data = romfs
        .read(&mut d, format!("Pack/Actor/{actor}.pack.zs"))
        .unwrap();
    let sarc = Sarc::parse(&data).unwrap();
    for f in sarc.files() {
        let name = f.name.unwrap_or("?");
        if !name.contains(filter.as_str()) {
            continue;
        }
        println!("{name} ({} bytes)", f.data.len());
        if !filter.is_empty()
            && let Ok(v) = byml::parse(f.data)
        {
            let mut out = String::new();
            show(&v, 1, &mut out);
            print!("{out}");
        }
    }
}

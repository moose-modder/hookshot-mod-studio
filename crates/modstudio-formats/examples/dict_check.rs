//! `res_dict::build` against the game's own dictionaries: every model's bone dictionary and every animation file's
//! dictionary of animations, built again from their names and compared node by node (the bit tested, the two
//! links), and every name looked up.
//!
//!     cargo run --release -p modstudio-formats --example dict_check -- <romfs> [name filter]

use std::path::Path;

use modstudio_formats::bfres::{Bfres, ExternalStrings};
use modstudio_formats::bfres_anim;
use modstudio_formats::res_dict::{self, DictNode};
use modstudio_formats::romfs::Romfs;

fn read_dict(d: &[u8], at: usize) -> Vec<DictNode> {
    let count = i32::from_le_bytes(d[at + 4..at + 8].try_into().unwrap()) as usize;
    (0..=count)
        .map(|i| {
            let n = at + 8 + i * 16;
            DictNode {
                reference: u32::from_le_bytes(d[n..n + 4].try_into().unwrap()),
                left: u16::from_le_bytes(d[n + 4..n + 6].try_into().unwrap()),
                right: u16::from_le_bytes(d[n + 6..n + 8].try_into().unwrap()),
            }
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: dict_check <romfs> [name filter]");
        std::process::exit(2);
    };
    let filter = args.get(1).cloned().unwrap_or_default();
    let romfs = Romfs::open(root).expect("romfs");
    let mut d = romfs.decoder().unwrap();
    let strings = ExternalStrings::parse(
        &romfs
            .read(&mut d, "Shader/ExternalBinaryString.bfres.zs")
            .unwrap(),
    )
    .unwrap();
    let mut files: Vec<String> = std::fs::read_dir(Path::new(root).join("Model"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".bfres.zs") && n.contains(filter.as_str()))
        .collect();
    files.sort();
    let (mut same, mut differ, mut dicts, mut names_found, mut names) = (0, Vec::new(), 0, 0, 0);
    for f in &files {
        let Ok(data) = romfs.read(&mut d, format!("Model/{f}")) else {
            continue;
        };
        let lists: Vec<(String, usize, Vec<String>)> = if f.contains(".anim.") {
            let Ok(a) = bfres_anim::list(&data, Some(&strings)) else {
                continue;
            };
            let at = u64::from_le_bytes(data[0x60..0x68].try_into().unwrap()) as usize;
            if at == 0 {
                continue;
            }
            vec![(
                "animations".into(),
                at,
                a.into_iter().map(|x| x.name).collect(),
            )]
        } else {
            let Ok(b) = Bfres::parse(&data, Some(&strings)) else {
                continue;
            };
            b.models
                .iter()
                .map(|m| {
                    let at = u64::from_le_bytes(
                        data[m.skeleton.at + 8..m.skeleton.at + 16]
                            .try_into()
                            .unwrap(),
                    ) as usize;
                    (
                        format!("bones of {}", m.name),
                        at,
                        m.skeleton.bones.iter().map(|x| x.name.clone()).collect(),
                    )
                })
                .collect()
        };
        for (what, at, list) in lists {
            dicts += 1;
            let keys: Vec<&str> = list.iter().map(String::as_str).collect();
            let built = res_dict::build(&keys);
            let game = read_dict(&data, at);
            if built == game {
                same += 1;
            } else {
                let first = built.iter().zip(&game).position(|(a, b)| a != b);
                differ.push(format!(
                    "{f} ({what}, {} names): first differing node {first:?}: built {:?}, game {:?}",
                    keys.len(),
                    first.map(|i| built[i]),
                    first.map(|i| game[i])
                ));
            }
            for (i, k) in keys.iter().enumerate() {
                names += 1;
                if res_dict::find(&game, &keys, k) == Some(i) {
                    names_found += 1;
                }
            }
        }
    }
    println!(
        "{dicts} dictionaries: {same} built exactly as the game's, {} not; {names_found} of {names} names found through the game's dictionaries",
        differ.len()
    );
    for x in differ.iter().take(8) {
        println!("    {x}");
    }
}

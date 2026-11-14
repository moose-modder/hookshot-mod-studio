//! The game's cloth links (`cloth_links::scan`: which cloth files its actor packs give each model) next to the old
//! guess from names (a cloth file named after the model's group or the model): how long reading the packs takes,
//! which cloth files no actor gives, and where the links and the names disagree.
//!
//!     cargo run --release -p modstudio-core --example cloth_links -- <romfs> [--json out.json]

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use modstudio_core::{catalog, cloth_links};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: cloth_links <romfs> [--json out.json]");
        std::process::exit(2);
    };
    let root = Path::new(root);
    let t = Instant::now();
    let links = cloth_links::scan(root).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "read the packs in {:.1} s: {}",
        t.elapsed().as_secs_f32(),
        links.stamp
    );
    if let Some(i) = args.iter().position(|a| a == "--json") {
        let out = &args[i + 1];
        std::fs::write(out, serde_json::to_string_pretty(&links).expect("JSON")).expect("write");
    }
    let models: Vec<_> = catalog::list(root).expect("models");
    let model_paths: BTreeSet<&str> = models.iter().map(|m| m.path.as_str()).collect();
    let missing_models: Vec<&String> = links
        .models
        .keys()
        .filter(|m| !model_paths.contains(m.as_str()))
        .collect();
    println!(
        "{} model(s) with cloth from the packs; {} of them not in Model/: {:?}",
        links.models.len(),
        missing_models.len(),
        missing_models.iter().take(10).collect::<Vec<_>>()
    );
    let several: Vec<_> = links.models.iter().filter(|(_, l)| l.len() > 1).collect();
    println!("{} model(s) given more than one cloth file:", several.len());
    for (m, l) in several.iter().take(20) {
        println!("    {m}:");
        for c in l.iter() {
            println!(
                "        {} ({} actors: {})",
                c.cloth,
                c.actors.len(),
                c.actors
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    // every cloth file, and who has it
    let dir = root.join("Phive").join("Cloth");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("Phive/Cloth")
        .flatten()
        .map(|e| format!("Phive/Cloth/{}", e.file_name().to_string_lossy()))
        .filter(|n| n.ends_with(".bphcl"))
        .collect();
    files.sort();
    let linked: BTreeSet<&str> = links
        .models
        .values()
        .flatten()
        .map(|l| l.cloth.as_str())
        .collect();
    let unlinked: Vec<&String> = files
        .iter()
        .filter(|f| !linked.contains(f.as_str()))
        .collect();
    println!(
        "{} cloth file(s), {} given by no actor: {:?}",
        files.len(),
        unlinked.len(),
        unlinked
    );
    // the name guess, per model, against the links
    let (mut same, mut only_name, mut only_link, mut differ) =
        (0, Vec::new(), Vec::new(), Vec::new());
    for m in &models {
        let guess: Vec<String> = [m.group.as_str(), m.name.as_str()]
            .iter()
            .map(|n| format!("Phive/Cloth/{n}.Nin_Ounce_NVN2.bphcl"))
            .filter(|p| root.join(p).is_file())
            .collect();
        let guess = guess.first().cloned();
        let link = links.cloth_of(&m.path).first().map(|s| s.to_string());
        match (guess, link) {
            (None, None) => {}
            (Some(g), Some(l)) if g == l => same += 1,
            (Some(g), None) => only_name.push(format!("{} -> {g}", m.path)),
            (None, Some(l)) => only_link.push(format!("{} -> {l}", m.path)),
            (Some(g), Some(l)) => differ.push(format!("{}: name {g}, actors {l}", m.path)),
        }
    }
    println!("models whose cloth the name and the packs agree on: {same}");
    println!("found only by name ({}):", only_name.len());
    for x in &only_name {
        println!("    {x}");
    }
    println!("found only in the packs ({}):", only_link.len());
    for x in &only_link {
        println!("    {x}");
    }
    println!("name and packs differ ({}):", differ.len());
    for x in &differ {
        println!("    {x}");
    }
}

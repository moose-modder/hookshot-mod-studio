//! The game's actor links (`actor_links::scan`: the cloth and animation files its actor packs give each model): how
//! long reading the packs takes; for cloth, which files no actor gives and where the links and the old guess from
//! names (a cloth file named after the model's group or the model) disagree; for animations, how many files the
//! actors give and which animation files no actor gives.
//!
//!     cargo run --release -p modstudio-core --example actor_links -- <romfs> [--json out.json]

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use modstudio_core::{actor_links, catalog};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(root) = args.first() else {
        eprintln!("usage: actor_links <romfs> [--json out.json]");
        std::process::exit(2);
    };
    let root = Path::new(root);
    let t = Instant::now();
    let links = actor_links::scan(root).unwrap_or_else(|e| panic!("{e}"));
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
        "{} model(s) given cloth or animations; {} of them not in Model/: {:?}",
        links.models.len(),
        missing_models.len(),
        missing_models.iter().take(10).collect::<Vec<_>>()
    );

    // cloth
    let with_cloth = links
        .models
        .values()
        .filter(|l| !l.cloth.is_empty())
        .count();
    println!("cloth: {with_cloth} model(s)");
    for (m, l) in links.models.iter().filter(|(_, l)| l.cloth.len() > 1) {
        println!("    {m}, more than one cloth file:");
        for c in &l.cloth {
            println!("        {} ({} actors)", c.file, c.actors.len());
        }
    }
    let list = |dir: &str, suffix: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(root.join(dir))
            .expect("folder")
            .flatten()
            .map(|e| format!("{dir}/{}", e.file_name().to_string_lossy()))
            .filter(|n| n.ends_with(suffix))
            .collect();
        v.sort();
        v
    };
    let cloth_files = list("Phive/Cloth", ".bphcl");
    let linked: BTreeSet<&str> = links
        .models
        .values()
        .flat_map(|l| l.cloth.iter().map(|c| c.file.as_str()))
        .collect();
    let unlinked: Vec<&String> = cloth_files
        .iter()
        .filter(|f| !linked.contains(f.as_str()))
        .collect();
    println!(
        "    {} cloth file(s), {} given by no actor: {:?}",
        cloth_files.len(),
        unlinked.len(),
        unlinked
    );
    let (mut same, mut only_name, mut only_link, mut differ) =
        (0, Vec::new(), Vec::new(), Vec::new());
    for m in &models {
        let guess = [m.group.as_str(), m.name.as_str()]
            .iter()
            .map(|n| format!("Phive/Cloth/{n}.Nin_Ounce_NVN2.bphcl"))
            .find(|p| root.join(p).is_file());
        let link = links.cloth_of(&m.path).first().map(|s| s.to_string());
        match (guess, link) {
            (None, None) => {}
            (Some(g), Some(l)) if g == l => same += 1,
            (Some(g), None) => only_name.push(format!("{} -> {g}", m.path)),
            (None, Some(l)) => only_link.push(format!("{} -> {l}", m.path)),
            (Some(g), Some(l)) => differ.push(format!("{}: name {g}, actors {l}", m.path)),
        }
    }
    println!("    the name guess and the packs agree on {same} model(s)");
    for (what, list) in [
        ("found only by name", &only_name),
        ("found only in the packs", &only_link),
        ("name and packs differ", &differ),
    ] {
        println!("    {what} ({}):", list.len());
        for x in list {
            println!("        {x}");
        }
    }

    // animations
    let with_anims: Vec<_> = links
        .models
        .iter()
        .filter(|(_, l)| !l.anims.is_empty())
        .collect();
    let anim_files = list("Model", ".anim.bfres.zs");
    let given: BTreeSet<&str> = links
        .models
        .values()
        .flat_map(|l| l.anims.iter().map(|a| a.file.as_str()))
        .collect();
    println!(
        "animations: {} model(s) given {} of the {} animation files",
        with_anims.len(),
        given.len(),
        anim_files.len()
    );
    for (m, l) in with_anims
        .iter()
        .filter(|(m, _)| m.contains("Player_") || m.contains("GreatFairy"))
    {
        println!(
            "    {m}: {}",
            l.anims
                .iter()
                .map(|a| a.file.trim_start_matches("Model/"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // the files no actor gives: a model's own (named after its group) and cutscenes (`Dm_<scene>_<model>`)
    let groups: BTreeSet<&str> = models.iter().map(|m| m.group.as_str()).collect();
    let (mut own, mut cutscene, mut other) = (0, 0, Vec::new());
    for f in anim_files.iter().filter(|f| !given.contains(f.as_str())) {
        let stem = f
            .trim_start_matches("Model/")
            .trim_end_matches(".anim.bfres.zs");
        if groups.contains(stem) {
            own += 1;
        } else if stem.starts_with("Dm_") && groups.iter().any(|g| stem.ends_with(&format!("_{g}")))
        {
            cutscene += 1;
        } else {
            other.push(stem.to_owned());
        }
    }
    println!(
        "    given by no actor: {own} named after a model's group, {cutscene} cutscenes ending in a group's name, {} other: {:?}",
        other.len(),
        other.iter().take(25).collect::<Vec<_>>()
    );
}

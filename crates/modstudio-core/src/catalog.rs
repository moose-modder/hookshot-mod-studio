//! The models in a romfs, by kind, for picking one to mod. Files are `Model/<group>.<model>.bfres.zs`: most
//! groups hold one model of the same name (`Npc_Hylia_MN_001.Npc_Hylia_MN_001`), some several
//! (`Armor_Child_Mask_Bunny.Armor_Child_Mask_Bunny_NPC`, `..._Shop`).

use std::path::Path;

use serde::Serialize;

use crate::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelEntry {
    /// Path in the romfs: `Model/Npc_Hylia_MN_001.Npc_Hylia_MN_001.bfres.zs`.
    pub path: String,
    /// The model: `Npc_Hylia_MN_001`.
    pub name: String,
    /// The group it's filed under (often the same as `name`).
    pub group: String,
    pub category: &'static str,
    pub bytes: u64,
}

/// Kinds of model, in the order the app lists them. "Level" (map pieces, terrain, foliage: thousands of
/// files) is listed last; the app hides it unless asked.
pub const CATEGORIES: [&str; 10] = [
    "Characters",
    "Enemies",
    "Player",
    "Animals",
    "Armor",
    "Weapons",
    "Items",
    "Objects",
    "Other",
    "Level",
];

/// The kind of a model, from its name's prefix.
pub fn category(name: &str) -> &'static str {
    let prefix = name.split(['_', '.']).next().unwrap_or(name);
    match prefix {
        "Npc" => "Characters",
        "Enemy" => "Enemies",
        "Player" => "Player",
        "Animal" | "Horse" => "Animals",
        "Armor" => "Armor",
        "Weapon" => "Weapons",
        "Item" => "Items",
        "Obj" | "DgnObj" | "MinusObj" | "GateCloth" => "Objects",
        "Land" | "Terrain" | "Foliage" | "Cubemap" | "RiverRail" | "KakarikoVillage"
        | "KokiriForest" => "Level",
        _ => "Other",
    }
}

/// Every model in `<romfs>/Model`, sorted by name. Left out: animations (`.anim.`), the skeletons animations
/// use (`<name>_Animation.Skeleton`), and MeshCodec files (`.mc`).
pub fn list(romfs: &Path) -> Result<Vec<ModelEntry>> {
    let dir = romfs.join("Model");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).context(dir.display())?.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let lower = file.to_ascii_lowercase();
        if !lower.ends_with(".bfres.zs") || lower.contains(".anim.") {
            continue;
        }
        let (group, name) = split_name(&file);
        if group.ends_with("_Animation") {
            continue;
        }
        out.push(ModelEntry {
            path: format!("Model/{file}"),
            category: category(group),
            name: name.to_owned(),
            group: group.to_owned(),
            bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    out.sort_by_key(|m| m.name.to_lowercase());
    Ok(out)
}

/// The group and the model of a model file's name (`<group>.<model>.bfres.zs`).
pub fn split_name(file: &str) -> (&str, &str) {
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let parts: Vec<&str> = file.split('.').collect();
    match parts.as_slice() {
        [group, model, _, _, ..] => (group, model),
        [one, ..] => (one, one),
        [] => (file, file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_prefixes() {
        assert_eq!(category("Npc_Hylia_MN_001"), "Characters");
        assert_eq!(category("Enemy_Stalfos"), "Enemies");
        assert_eq!(category("Land_Field_01"), "Level");
        assert_eq!(category("Foliage.FoliageMoss"), "Level");
        assert_eq!(category("dammy"), "Other");
        for c in [
            "Npc", "Enemy", "Player", "Animal", "Armor", "Weapon", "Item", "Obj", "Land", "x",
        ] {
            assert!(CATEGORIES.contains(&category(&format!("{c}_A"))));
        }
    }

    #[test]
    fn lists_models_only() {
        let dir = std::env::temp_dir().join(format!("modstudio-catalog-{}", std::process::id()));
        let model = dir.join("Model");
        std::fs::create_dir_all(&model).unwrap();
        for f in [
            "Npc_B.Npc_B.bfres.zs",
            "Npc_a.Npc_a.bfres.zs",
            "Npc_a.Npc_a_Anim.anim.bfres.zs",
            "Npc_c.Npc_c.bfres.mc",
            "Enemy_X_Animation.Skeleton.bfres.zs",
            "Npc_d.Npc_d_Shop.bfres.zs",
            "notes.txt",
        ] {
            std::fs::write(model.join(f), b"x").unwrap();
        }
        let names: Vec<String> = list(&dir).unwrap().into_iter().map(|m| m.path).collect();
        assert_eq!(
            names,
            [
                "Model/Npc_a.Npc_a.bfres.zs",
                "Model/Npc_B.Npc_B.bfres.zs",
                "Model/Npc_d.Npc_d_Shop.bfres.zs"
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

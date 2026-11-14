//! What the game's actors give each model: its cloth files and its animation files, read from the actor packs
//! (`Pack/Actor/<actor>.pack.zs`).
//!
//! An actor's `Actor/<actor>.engine__actor__ActorParam.bgyml` names, under `Components`:
//! - its model info (`ModelInfoRef`): the model `Model/<ModelProjectName>.<FmdbName>.bfres.zs`;
//! - its physics (`PhysicsRef`), whose `ControllerSetPath` is a controller set: each of its `ClothList` paths,
//!   `Work/Phive/Cloth/<name>.phcl`, is the cloth file `Phive/Cloth/<name>.Nin_Ounce_NVN2.bphcl`;
//! - its animation (`AnimationRef`): each of its `AnimationResources` is an animation file
//!   `Model/<ModelProjectName>.anim.bfres.zs` (the player's bundles, an NPC's cutscene).
//!
//! Each of these files takes what it leaves out from its `$parent`, which is in the same pack.
//!
//! Most cloth files are named after their model, but not all: the Great Fairy's is `Npc_U_GreantFairy`, Koume uses
//! Kotake's, an actor can use another model's file. Those are found only here. Reading the eight thousand packs takes
//! a while, so the links are kept in `<cache>/actor_links.json`, and read again when the packs change. The packs read
//! are the game's (a mod's packs aren't).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use modstudio_formats::byml::{self, Value};
use modstudio_formats::romfs::Romfs;
use modstudio_formats::sarc::Sarc;
use serde::{Deserialize, Serialize};

use crate::{Context, Error, Result};

/// The cache file in the app's cache folder.
pub const CACHE_FILE: &str = "actor_links.json";
/// Bumped when what's read from the packs changes, so an older cache is read again.
const VERSION: u32 = 2;
/// How deep `$parent` chains may go (a file naming itself as its parent would loop).
const MAX_PARENTS: usize = 16;

/// What the game's actors give its models.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActorLinks {
    /// What the packs were when read: their count, total size and newest change, and [`VERSION`].
    pub stamp: String,
    /// Each model (`Model/<group>.<model>.bfres.zs`) an actor gives cloth or animations.
    pub models: BTreeMap<String, ModelLinks>,
}

/// A model's files, as the actors using it give them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLinks {
    /// Its cloth files (sorted).
    #[serde(default)]
    pub cloth: Vec<Link>,
    /// Its animation files (sorted).
    #[serde(default)]
    pub anims: Vec<Link>,
}

/// A file and the actors that give it to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// The file, in the romfs: `Phive/Cloth/<name>.Nin_Ounce_NVN2.bphcl`, `Model/<name>.anim.bfres.zs`.
    pub file: String,
    pub actors: Vec<String>,
}

/// The files of `links`, the one most actors give first.
fn most_given(links: &[Link]) -> Vec<&str> {
    let mut links: Vec<&Link> = links.iter().collect();
    links.sort_by(|a, b| {
        b.actors
            .len()
            .cmp(&a.actors.len())
            .then(a.file.cmp(&b.file))
    });
    links.iter().map(|l| l.file.as_str()).collect()
}

impl ActorLinks {
    fn of(&self, model: &str) -> Option<&ModelLinks> {
        self.models.get(&model.replace('\\', "/"))
    }

    /// The cloth files of `model` (a model's romfs path), the one most actors give first.
    pub fn cloth_of(&self, model: &str) -> Vec<&str> {
        self.of(model)
            .map(|l| most_given(&l.cloth))
            .unwrap_or_default()
    }

    /// The animation files the actors using `model` give it, the one most actors give first.
    pub fn anims_of(&self, model: &str) -> Vec<&str> {
        self.of(model)
            .map(|l| most_given(&l.anims))
            .unwrap_or_default()
    }

    /// The actors giving `model` the cloth file `cloth`.
    pub fn cloth_actors(&self, model: &str, cloth: &str) -> Vec<&str> {
        self.of(model)
            .and_then(|l| l.cloth.iter().find(|c| c.file == cloth))
            .map(|c| c.actors.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// The models the game gives the cloth file `cloth` (a romfs path).
    pub fn models_with(&self, cloth: &str) -> Vec<&str> {
        self.models
            .iter()
            .filter(|(_, links)| links.cloth.iter().any(|l| l.file == cloth))
            .map(|(m, _)| m.as_str())
            .collect()
    }
}

/// A path as a pack lists its files: `?Component/X.bgyml` (a component reference) and `Work/Actor/X.gyml` (a
/// parameter file, as the game's sources named it) are `Component/X.bgyml` and `Actor/X.bgyml`.
fn in_pack(reference: &str) -> String {
    let r = reference.trim_start_matches('?');
    let r = r.strip_prefix("Work/").unwrap_or(r);
    match r.strip_suffix(".gyml") {
        Some(stem) => format!("{stem}.bgyml"),
        None => r.to_owned(),
    }
}

/// A controller set's cloth path (`Work/Phive/Cloth/<name>.phcl`) as the romfs has the file.
fn cloth_file_of(path: &str) -> Option<String> {
    let p = path.strip_prefix("Work/").unwrap_or(path);
    let name = p.strip_prefix("Phive/Cloth/")?.strip_suffix(".phcl")?;
    (!name.is_empty()).then(|| format!("Phive/Cloth/{name}.Nin_Ounce_NVN2.bphcl"))
}

/// The value at `keys` in the parameter file `file` (a pack path), or in its parents'.
fn inherited(read: &dyn Fn(&str) -> Option<Value>, file: &str, keys: &[&str]) -> Option<Value> {
    let mut file = file.to_owned();
    for _ in 0..MAX_PARENTS {
        let doc = read(&file)?;
        let mut v = Some(&doc);
        for k in keys {
            v = v.and_then(|x| x.get(k));
        }
        if let Some(found) = v {
            return Some(found.clone());
        }
        file = in_pack(doc.get("$parent")?.as_str()?);
    }
    None
}

/// What one actor gives its model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorFiles {
    pub actor: String,
    /// `Model/<group>.<model>.bfres.zs`.
    pub model: String,
    /// Cloth files (romfs paths, sorted).
    pub cloth: Vec<String>,
    /// Animation files (romfs paths, sorted).
    pub anims: Vec<String>,
}

/// The model of `actor` and the cloth and animation files it gives it, its parameter files read through `read` (a
/// pack path to its document): `None` if it has no model, or gives it neither.
pub fn actor_files(read: &dyn Fn(&str) -> Option<Value>, actor: &str) -> Option<ActorFiles> {
    let actor_param = format!("Actor/{actor}.engine__actor__ActorParam.bgyml");
    let text = |v: Option<Value>| v.and_then(|v| v.as_str().map(str::to_owned));
    let component = |name: &str| {
        text(inherited(read, &actor_param, &["Components", name])).map(|r| in_pack(&r))
    };
    let model_info = component("ModelInfoRef")?;
    let fmdb = text(inherited(read, &model_info, &["FmdbName"])).filter(|s| !s.is_empty())?;
    let project = text(inherited(read, &model_info, &["ModelProjectName"]))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fmdb.clone());
    let mut cloth: Vec<String> = component("PhysicsRef")
        .and_then(|physics| text(inherited(read, &physics, &["ControllerSetPath"])))
        .and_then(|set| inherited(read, &in_pack(&set), &["ClothList"]))
        .and_then(|list| {
            list.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|e| e.str_at(&["Path"]).and_then(cloth_file_of))
                    .collect()
            })
        })
        .unwrap_or_default();
    let mut anims: Vec<String> = component("AnimationRef")
        .and_then(|param| inherited(read, &param, &["AnimationResources"]))
        .and_then(|list| {
            list.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|e| e.str_at(&["ModelProjectName"]))
                    .filter(|n| !n.is_empty())
                    .map(|n| format!("Model/{n}.anim.bfres.zs"))
                    .collect()
            })
        })
        .unwrap_or_default();
    for list in [&mut cloth, &mut anims] {
        list.sort();
        list.dedup();
    }
    (!cloth.is_empty() || !anims.is_empty()).then(|| ActorFiles {
        actor: actor.to_owned(),
        model: format!("Model/{project}.{fmdb}.bfres.zs"),
        cloth,
        anims,
    })
}

/// What the actor of the pack `data` (`<actor>.pack`, decompressed) gives its model.
pub fn pack_files(data: &[u8], actor: &str) -> Option<ActorFiles> {
    let sarc = Sarc::parse(data).ok()?;
    let files: HashMap<&str, &[u8]> = sarc
        .files()
        .iter()
        .filter_map(|f| f.name.map(|n| (n, f.data)))
        .collect();
    let read = |path: &str| files.get(path).and_then(|d| byml::parse(d).ok());
    actor_files(&read, actor)
}

fn packs_dir(game_romfs: &Path) -> PathBuf {
    game_romfs.join("Pack").join("Actor")
}

/// The actor packs and their stamp (count, total size, newest change, [`VERSION`]).
fn packs(game_romfs: &Path) -> Result<(Vec<PathBuf>, String)> {
    let dir = packs_dir(game_romfs);
    let mut list = Vec::new();
    let (mut size, mut newest) = (0u64, 0u64);
    for e in std::fs::read_dir(&dir).context(dir.display())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".pack.zs") {
            continue;
        }
        if let Ok(m) = e.metadata() {
            size += m.len();
            let changed = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            newest = newest.max(changed);
        }
        list.push(e.path());
    }
    list.sort();
    let stamp = format!("v{VERSION} {} packs, {size} bytes, {newest}", list.len());
    Ok((list, stamp))
}

/// Reads every actor pack of the game (a thread per core).
pub fn scan(game_romfs: &Path) -> Result<ActorLinks> {
    let (list, stamp) = packs(game_romfs)?;
    let romfs = Romfs::open(game_romfs)?;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let chunk = list.len().div_ceil(threads).max(1);
    let found: Vec<Result<Vec<ActorFiles>>> = std::thread::scope(|s| {
        let handles: Vec<_> = list
            .chunks(chunk)
            .map(|packs| {
                let romfs = &romfs;
                s.spawn(move || -> Result<Vec<ActorFiles>> {
                    let mut decoder = romfs.decoder()?;
                    let mut out = Vec::new();
                    for pack in packs {
                        let name = pack.file_name().unwrap_or_default().to_string_lossy();
                        let actor = name.trim_end_matches(".pack.zs").to_owned();
                        // a pack that doesn't read is left out, as one giving nothing
                        let Ok(data) = romfs.read(&mut decoder, pack) else {
                            continue;
                        };
                        out.extend(pack_files(&data, &actor));
                    }
                    Ok(out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(Error("a pack-reading thread panicked".into())))
            })
            .collect()
    });
    type Given = BTreeMap<String, BTreeSet<String>>;
    let mut models: BTreeMap<String, (Given, Given)> = BTreeMap::new();
    for part in found {
        for f in part? {
            let entry = models.entry(f.model.clone()).or_default();
            // only files the game has (a parameter file can name one that isn't shipped)
            for c in f.cloth.into_iter().filter(|c| game_romfs.join(c).is_file()) {
                entry.0.entry(c).or_default().insert(f.actor.clone());
            }
            for a in f.anims.into_iter().filter(|a| game_romfs.join(a).is_file()) {
                entry.1.entry(a).or_default().insert(f.actor.clone());
            }
        }
    }
    let links = |given: Given| -> Vec<Link> {
        given
            .into_iter()
            .map(|(file, actors)| Link {
                file,
                actors: actors.into_iter().collect(),
            })
            .collect()
    };
    let models = models
        .into_iter()
        .filter(|(_, (cloth, anims))| !cloth.is_empty() || !anims.is_empty())
        .map(|(model, (cloth, anims))| {
            (
                model,
                ModelLinks {
                    cloth: links(cloth),
                    anims: links(anims),
                },
            )
        })
        .collect();
    Ok(ActorLinks { stamp, models })
}

fn memo() -> &'static Mutex<HashMap<PathBuf, Arc<ActorLinks>>> {
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Arc<ActorLinks>>>> = OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The game's actor links: read once a run, from `<cache_dir>/actor_links.json` while it matches the packs, else
/// from the packs (and kept there). Without a cache folder (`None`) they're kept only for the run.
pub fn links(game_romfs: &Path, cache_dir: Option<&Path>) -> Result<Arc<ActorLinks>> {
    let key = game_romfs.to_path_buf();
    if let Some(l) = memo().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(l);
    }
    let (_, stamp) = packs(game_romfs)?;
    let cache = cache_dir.map(|d| d.join(CACHE_FILE));
    let cached = cache
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<ActorLinks>(&t).ok())
        .filter(|l| l.stamp == stamp);
    let links = match cached {
        Some(l) => l,
        None => {
            let l = scan(game_romfs)?;
            if let Some(path) = &cache {
                // a cache that can't be written only means reading the packs again next time
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Ok(text) = serde_json::to_string_pretty(&l) {
                    let _ = std::fs::write(path, text);
                }
                // the cache of the first version, superseded
                if let Some(dir) = path.parent() {
                    let _ = std::fs::remove_file(dir.join("cloth_links.json"));
                }
            }
            l
        }
    };
    let links = Arc::new(links);
    if let Ok(mut m) = memo().lock() {
        m.insert(key, links.clone());
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        )
    }
    fn s(v: &str) -> Value {
        Value::String(v.to_owned())
    }

    /// A pack like the Great Fairy's: the actor inherits its model, physics and animation from a base actor, its
    /// cloth file isn't named after its model, and its controller set's parent has no cloth.
    fn fairy() -> HashMap<String, Value> {
        let mut files = HashMap::new();
        files.insert(
            "Actor/Fairy_001.engine__actor__ActorParam.bgyml".to_owned(),
            map(&[
                (
                    "$parent",
                    s("Work/Actor/Fairy_Default.engine__actor__ActorParam.gyml"),
                ),
                (
                    "Components",
                    map(&[("VoiceRef", s("?Component/VoiceParam/V.bgyml"))]),
                ),
            ]),
        );
        files.insert(
            "Actor/Fairy_Default.engine__actor__ActorParam.bgyml".to_owned(),
            map(&[
                ("$parent", s("Work/Actor/Base.engine__actor__ActorParam.gyml")),
                (
                    "Components",
                    map(&[
                        (
                            "AnimationRef",
                            s("?Component/AnimationParam/Fairy.engine__component__AnimationParam.bgyml"),
                        ),
                        (
                            "ModelInfoRef",
                            s("?Component/ModelInfo/Npc_U_Fairy.engine__component__ModelInfo.bgyml"),
                        ),
                        (
                            "PhysicsRef",
                            s("?Component/Physics/Fairy.engine__component__PhysicsParam.bgyml"),
                        ),
                    ]),
                ),
            ]),
        );
        files.insert(
            "Actor/Base.engine__actor__ActorParam.bgyml".to_owned(),
            map(&[(
                "Components",
                map(&[("ModelInfoRef", s("?Component/ModelInfo/Dummy.bgyml"))]),
            )]),
        );
        files.insert(
            "Component/ModelInfo/Npc_U_Fairy.engine__component__ModelInfo.bgyml".to_owned(),
            map(&[
                ("FmdbName", s("Npc_U_Fairy")),
                ("ModelProjectName", s("Npc_U_Fairy")),
            ]),
        );
        files.insert(
            "Component/AnimationParam/Fairy.engine__component__AnimationParam.bgyml".to_owned(),
            map(&[(
                "AnimationResources",
                Value::Array(vec![
                    map(&[("ModelProjectName", s("Dm_Fountain_Npc_U_Fairy"))]),
                    map(&[("ModelProjectName", s(""))]),
                ]),
            )]),
        );
        files.insert(
            "Component/Physics/Fairy.engine__component__PhysicsParam.bgyml".to_owned(),
            map(&[
                (
                    "$parent",
                    s("Work/Component/Physics/Base.engine__component__PhysicsParam.gyml"),
                ),
                (
                    "ControllerSetPath",
                    s("Work/Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.gyml"),
                ),
            ]),
        );
        files.insert(
            "Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.bgyml".to_owned(),
            map(&[
                (
                    "$parent",
                    s("Work/Phive/ControllerSetParam/Base.phive__ControllerSetParam.gyml"),
                ),
                (
                    "ClothList",
                    Value::Array(vec![map(&[
                        ("Name", s("Default")),
                        ("Path", s("Work/Phive/Cloth/Npc_U_Faiiry.phcl")),
                    ])]),
                ),
            ]),
        );
        files.insert(
            "Phive/ControllerSetParam/Base.phive__ControllerSetParam.bgyml".to_owned(),
            map(&[("ShapeNamePathAry", Value::Array(vec![]))]),
        );
        files
    }

    #[test]
    fn an_actor_s_files_through_its_parents() {
        let files = fairy();
        let read = |p: &str| files.get(p).cloned();
        assert_eq!(
            actor_files(&read, "Fairy_001"),
            Some(ActorFiles {
                actor: "Fairy_001".to_owned(),
                model: "Model/Npc_U_Fairy.Npc_U_Fairy.bfres.zs".to_owned(),
                cloth: vec!["Phive/Cloth/Npc_U_Faiiry.Nin_Ounce_NVN2.bphcl".to_owned()],
                anims: vec!["Model/Dm_Fountain_Npc_U_Fairy.anim.bfres.zs".to_owned()],
            })
        );
        // the base actor alone has a dummy model info and gives nothing
        assert_eq!(actor_files(&read, "Base"), None);
        assert_eq!(actor_files(&read, "Missing"), None);
    }

    #[test]
    fn an_actor_without_cloth_still_gives_its_animations() {
        let mut files = fairy();
        files.insert(
            "Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.bgyml".to_owned(),
            map(&[(
                "$parent",
                s("Work/Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.gyml"),
            )]),
        );
        let read = |p: &str| files.get(p).cloned();
        let found = actor_files(&read, "Fairy_001").unwrap();
        assert!(found.cloth.is_empty());
        assert_eq!(found.anims, ["Model/Dm_Fountain_Npc_U_Fairy.anim.bfres.zs"]);
    }

    #[test]
    fn paths_as_packs_list_them() {
        assert_eq!(
            in_pack("?Component/X.engine__component__ModelInfo.bgyml"),
            "Component/X.engine__component__ModelInfo.bgyml"
        );
        assert_eq!(
            in_pack("Work/Actor/X.engine__actor__ActorParam.gyml"),
            "Actor/X.engine__actor__ActorParam.bgyml"
        );
        assert_eq!(
            cloth_file_of("Work/Phive/Cloth/Npc_A.phcl").as_deref(),
            Some("Phive/Cloth/Npc_A.Nin_Ounce_NVN2.bphcl")
        );
        assert_eq!(cloth_file_of("Work/Phive/Other/Npc_A.phcl"), None);
        assert_eq!(cloth_file_of("Work/Phive/Cloth/.phcl"), None);
    }

    #[test]
    fn the_file_most_actors_give_comes_first() {
        let link = |c: &str, actors: &[&str]| Link {
            file: c.to_owned(),
            actors: actors.iter().map(|a| (*a).to_owned()).collect(),
        };
        let mut links = ActorLinks::default();
        links.models.insert(
            "Model/A.A.bfres.zs".to_owned(),
            ModelLinks {
                cloth: vec![
                    link("Phive/Cloth/A_Alt.bphcl", &["x"]),
                    link("Phive/Cloth/A.bphcl", &["y", "z"]),
                ],
                anims: vec![link("Model/A_Animation.anim.bfres.zs", &["y"])],
            },
        );
        assert_eq!(
            links.cloth_of("Model\\A.A.bfres.zs"),
            ["Phive/Cloth/A.bphcl", "Phive/Cloth/A_Alt.bphcl"]
        );
        assert_eq!(
            links.anims_of("Model/A.A.bfres.zs"),
            ["Model/A_Animation.anim.bfres.zs"]
        );
        assert_eq!(
            links.cloth_actors("Model/A.A.bfres.zs", "Phive/Cloth/A.bphcl"),
            ["y", "z"]
        );
        assert_eq!(
            links.models_with("Phive/Cloth/A_Alt.bphcl"),
            ["Model/A.A.bfres.zs"]
        );
        assert!(links.cloth_of("Model/B.B.bfres.zs").is_empty());
    }
}

//! Which cloth files the game gives each model, read from its actor packs (`Pack/Actor/<actor>.pack.zs`).
//!
//! An actor's `Actor/<actor>.engine__actor__ActorParam.bgyml` names its model info (`Components.ModelInfoRef`: the
//! model `Model/<ModelProjectName>.<FmdbName>.bfres.zs`) and its physics (`Components.PhysicsRef`, whose
//! `ControllerSetPath` is a controller set; each of its `ClothList` paths, `Work/Phive/Cloth/<name>.phcl`, is the cloth
//! file `Phive/Cloth/<name>.Nin_Ounce_NVN2.bphcl`). Each of these files takes what it leaves out from its `$parent`,
//! which is in the same pack.
//!
//! Most cloth files are named after their model, but not all: the Great Fairy's is `Npc_U_GreantFairy`, Koume uses
//! Kotake's, an actor can use another model's file. Those are found only here. Reading the eight thousand packs takes
//! a while, so the links are kept in `<cache>/cloth_links.json`, and read again when the packs change. The packs read
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
pub const CACHE_FILE: &str = "cloth_links.json";
/// Bumped when what's read from the packs changes, so an older cache is read again.
const VERSION: u32 = 1;
/// How deep `$parent` chains may go (a file naming itself as its parent would loop).
const MAX_PARENTS: usize = 16;

/// The cloth files the game's actors give its models.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClothLinks {
    /// What the packs were when read: their count, total size and newest change, and [`VERSION`].
    pub stamp: String,
    /// Each model (`Model/<group>.<model>.bfres.zs`) with cloth: its cloth files (romfs paths, sorted), each with
    /// the actors giving it.
    pub models: BTreeMap<String, Vec<ClothLink>>,
}

/// One of a model's cloth files and the actors that give it to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClothLink {
    /// `Phive/Cloth/<name>.Nin_Ounce_NVN2.bphcl`.
    pub cloth: String,
    pub actors: Vec<String>,
}

impl ClothLinks {
    /// The cloth files of `model` (a model's romfs path), the one most actors give first.
    pub fn cloth_of(&self, model: &str) -> Vec<&str> {
        let key = model.replace('\\', "/");
        let mut links: Vec<&ClothLink> = self
            .models
            .get(&key)
            .map(|l| l.iter().collect())
            .unwrap_or_default();
        links.sort_by(|a, b| {
            b.actors
                .len()
                .cmp(&a.actors.len())
                .then(a.cloth.cmp(&b.cloth))
        });
        links.iter().map(|l| l.cloth.as_str()).collect()
    }

    /// The models the game gives the cloth file `cloth` (a romfs path).
    pub fn models_with(&self, cloth: &str) -> Vec<&str> {
        self.models
            .iter()
            .filter(|(_, links)| links.iter().any(|l| l.cloth == cloth))
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

/// The model and cloth files of `actor`, its parameter files read through `read` (a pack path to its document):
/// `None` if it has no model or no cloth.
pub fn actor_cloth(
    read: &dyn Fn(&str) -> Option<Value>,
    actor: &str,
) -> Option<(String, Vec<String>)> {
    let actor_param = format!("Actor/{actor}.engine__actor__ActorParam.bgyml");
    let text = |v: Option<Value>| v.and_then(|v| v.as_str().map(str::to_owned));
    let model_info = in_pack(&text(inherited(
        read,
        &actor_param,
        &["Components", "ModelInfoRef"],
    ))?);
    let physics = in_pack(&text(inherited(
        read,
        &actor_param,
        &["Components", "PhysicsRef"],
    ))?);
    let fmdb = text(inherited(read, &model_info, &["FmdbName"])).filter(|s| !s.is_empty())?;
    let project = text(inherited(read, &model_info, &["ModelProjectName"]))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fmdb.clone());
    let controllers = in_pack(&text(inherited(read, &physics, &["ControllerSetPath"]))?);
    let list = inherited(read, &controllers, &["ClothList"])?;
    let mut cloth: Vec<String> = list
        .as_array()?
        .iter()
        .filter_map(|e| e.str_at(&["Path"]).and_then(cloth_file_of))
        .collect();
    cloth.sort();
    cloth.dedup();
    (!cloth.is_empty()).then(|| (format!("Model/{project}.{fmdb}.bfres.zs"), cloth))
}

/// The model and cloth files of the actor of the pack `data` (`<actor>.pack`, decompressed).
pub fn pack_cloth(data: &[u8], actor: &str) -> Option<(String, Vec<String>)> {
    let sarc = Sarc::parse(data).ok()?;
    let files: HashMap<&str, &[u8]> = sarc
        .files()
        .iter()
        .filter_map(|f| f.name.map(|n| (n, f.data)))
        .collect();
    let read = |path: &str| files.get(path).and_then(|d| byml::parse(d).ok());
    actor_cloth(&read, actor)
}

/// What one actor gives: the actor, its model, its cloth files.
type Found = (String, String, Vec<String>);

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
pub fn scan(game_romfs: &Path) -> Result<ClothLinks> {
    let (list, stamp) = packs(game_romfs)?;
    let romfs = Romfs::open(game_romfs)?;
    let cloth_dir = game_romfs.join("Phive").join("Cloth");
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let chunk = list.len().div_ceil(threads).max(1);
    let found: Vec<Result<Vec<Found>>> = std::thread::scope(|s| {
        let handles: Vec<_> = list
            .chunks(chunk)
            .map(|packs| {
                let romfs = &romfs;
                s.spawn(move || -> Result<Vec<Found>> {
                    let mut decoder = romfs.decoder()?;
                    let mut out = Vec::new();
                    for pack in packs {
                        let name = pack.file_name().unwrap_or_default().to_string_lossy();
                        let actor = name.trim_end_matches(".pack.zs").to_owned();
                        // a pack that doesn't read is left out, as one without cloth
                        let Ok(data) = romfs.read(&mut decoder, pack) else {
                            continue;
                        };
                        if let Some((model, cloth)) = pack_cloth(&data, &actor) {
                            out.push((actor, model, cloth));
                        }
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
    let mut models: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for part in found {
        for (actor, model, cloth) in part? {
            for c in cloth {
                // only files the game has (a controller set can name one that isn't shipped)
                let file = c.rsplit('/').next().unwrap_or(&c);
                if cloth_dir.join(file).is_file() {
                    models
                        .entry(model.clone())
                        .or_default()
                        .entry(c)
                        .or_default()
                        .insert(actor.clone());
                }
            }
        }
    }
    let models = models
        .into_iter()
        .map(|(model, cloth)| {
            let links = cloth
                .into_iter()
                .map(|(cloth, actors)| ClothLink {
                    cloth,
                    actors: actors.into_iter().collect(),
                })
                .collect();
            (model, links)
        })
        .collect();
    Ok(ClothLinks { stamp, models })
}

fn memo() -> &'static Mutex<HashMap<PathBuf, Arc<ClothLinks>>> {
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, Arc<ClothLinks>>>> = OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// The game's cloth links: read once a run, from `<cache_dir>/cloth_links.json` while it matches the packs, else
/// from the packs (and kept there). Without a cache folder (`None`) they're kept only for the run.
pub fn links(game_romfs: &Path, cache_dir: Option<&Path>) -> Result<Arc<ClothLinks>> {
    let key = game_romfs.to_path_buf();
    if let Some(l) = memo().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(l);
    }
    let (_, stamp) = packs(game_romfs)?;
    let cache = cache_dir.map(|d| d.join(CACHE_FILE));
    let cached = cache
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<ClothLinks>(&t).ok())
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

    /// A pack like the Great Fairy's: the actor inherits its model and physics from a base actor, its cloth file
    /// isn't named after its model, and its controller set's parent has no cloth.
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
    fn an_actor_s_cloth_through_its_parents() {
        let files = fairy();
        let read = |p: &str| files.get(p).cloned();
        assert_eq!(
            actor_cloth(&read, "Fairy_001"),
            Some((
                "Model/Npc_U_Fairy.Npc_U_Fairy.bfres.zs".to_owned(),
                vec!["Phive/Cloth/Npc_U_Faiiry.Nin_Ounce_NVN2.bphcl".to_owned()]
            ))
        );
        // the base actor alone has a dummy model info and no physics
        assert_eq!(actor_cloth(&read, "Base"), None);
        assert_eq!(actor_cloth(&read, "Missing"), None);
    }

    #[test]
    fn no_cloth_list_or_a_parent_loop_is_none() {
        let mut files = fairy();
        files.insert(
            "Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.bgyml".to_owned(),
            map(&[(
                "$parent",
                s("Work/Phive/ControllerSetParam/Fairy.phive__ControllerSetParam.gyml"),
            )]),
        );
        let read = |p: &str| files.get(p).cloned();
        assert_eq!(actor_cloth(&read, "Fairy_001"), None);
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
    fn the_cloth_most_actors_give_comes_first() {
        let link = |c: &str, actors: &[&str]| ClothLink {
            cloth: c.to_owned(),
            actors: actors.iter().map(|a| (*a).to_owned()).collect(),
        };
        let mut links = ClothLinks::default();
        links.models.insert(
            "Model/A.A.bfres.zs".to_owned(),
            vec![
                link("Phive/Cloth/A_Alt.bphcl", &["x"]),
                link("Phive/Cloth/A.bphcl", &["y", "z"]),
            ],
        );
        assert_eq!(
            links.cloth_of("Model\\A.A.bfres.zs"),
            ["Phive/Cloth/A.bphcl", "Phive/Cloth/A_Alt.bphcl"]
        );
        assert!(links.cloth_of("Model/B.B.bfres.zs").is_empty());
    }
}

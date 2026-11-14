//! Hookshot Mod Studio: the app. The window is a web page (`app/src`); this side runs everything through
//! `modstudio-core`, the same code as the `modstudio` CLI, and sends long tasks' progress to the page as
//! `task` events.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use modstudio_core::build::{self, BuildReport};
use modstudio_core::catalog::{self, ModelEntry};
use modstudio_core::model::{self, Loaded};
use modstudio_core::progress::{Event, Progress};
use modstudio_core::project::{self, Project, ProjectFile};
use modstudio_core::settings::{Settings, Status};
use modstudio_core::{anim, cloth, mods, preview, tools};
use modstudio_formats::romfs::Romfs;
use serde::Serialize;
use tauri::ipc::Response;
use tauri::{AppHandle, Emitter, State};

/// Errors reach the page as their message.
type Result<T> = std::result::Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct App {
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    /// The game's romfs, opened once (reading its dictionaries takes a moment).
    romfs: Mutex<Option<Arc<Romfs>>>,
    /// Running tasks' cancel flags.
    tasks: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    next_task: AtomicU64,
    /// The 3D view's running cloth simulations, by id.
    cloths: Mutex<HashMap<u64, cloth::LiveCloth>>,
    next_cloth: AtomicU64,
    /// The 3D view's open animations, by id (the cloth follows one).
    clips: Mutex<HashMap<u64, Arc<anim::Clip>>>,
    next_clip: AtomicU64,
}

impl App {
    fn settings(&self) -> Settings {
        self.settings.lock().expect("settings lock").clone()
    }

    fn romfs(&self) -> Result<Arc<Romfs>> {
        let mut cached = self.romfs.lock().expect("romfs lock");
        if let Some(r) = cached.as_ref() {
            return Ok(r.clone());
        }
        let r = Arc::new(self.settings().romfs().map_err(err)?);
        *cached = Some(r.clone());
        Ok(r)
    }
}

#[derive(Clone, Serialize)]
struct TaskEvent {
    task: u64,
    #[serde(flatten)]
    event: Event,
}

/// Runs `work` on a worker thread with a [`Progress`] that sends `task` events to the page and stops when
/// [`cancel_task`] is called with `task`.
async fn run_task<T: Send + 'static>(
    app: AppHandle,
    state: &App,
    task: u64,
    work: impl FnOnce(&mut Progress) -> Result<T> + Send + 'static,
) -> Result<T> {
    let flag = Arc::new(AtomicBool::new(false));
    state
        .tasks
        .lock()
        .expect("tasks lock")
        .insert(task, flag.clone());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let sink_app = app.clone();
        let mut progress = Progress::new(move |event| {
            let _ = sink_app.emit("task", TaskEvent { task, event });
        })
        .with_cancel(&flag);
        work(&mut progress)
    })
    .await
    .map_err(err)?;
    state.tasks.lock().expect("tasks lock").remove(&task);
    result
}

// ---------------------------------------------------------------- settings

#[derive(Serialize)]
struct SettingsView {
    settings: Settings,
    path: PathBuf,
    status: Status,
    /// Blender installs found on this PC.
    blenders: Vec<PathBuf>,
}

fn settings_view(state: &App) -> SettingsView {
    let settings = state.settings();
    SettingsView {
        status: settings.status(),
        path: state.settings_path.clone(),
        blenders: tools::find_blender(),
        settings,
    }
}

#[tauri::command]
async fn get_settings(state: State<'_, App>) -> Result<SettingsView> {
    Ok(settings_view(&state))
}

#[tauri::command]
async fn save_settings(state: State<'_, App>, settings: Settings) -> Result<SettingsView> {
    settings.save(&state.settings_path).map_err(err)?;
    *state.settings.lock().expect("settings lock") = settings;
    *state.romfs.lock().expect("romfs lock") = None;
    Ok(settings_view(&state))
}

/// Settings for a Hookshot folder (its romfs and mods folders, and the Blender found if none is set), keeping
/// the rest of `current` (the projects folder is the user's to choose).
#[tauri::command]
fn settings_for_hookshot(dir: PathBuf, current: Settings) -> Settings {
    let fresh = Settings::for_hookshot(&dir);
    Settings {
        game_romfs: fresh.game_romfs,
        install_mods: fresh.install_mods,
        tools: modstudio_core::settings::Tools {
            blender: current.tools.blender.or(fresh.tools.blender),
        },
        ..current
    }
}

// ---------------------------------------------------------------- models

#[derive(Serialize)]
struct ModelList {
    models: Vec<ModelEntry>,
    categories: Vec<&'static str>,
    /// Model path -> the installed mods that replace it.
    modded: HashMap<String, Vec<String>>,
}

#[tauri::command]
async fn list_models(state: State<'_, App>) -> Result<ModelList> {
    let settings = state.settings();
    let models = catalog::list(&settings.game_romfs).map_err(err)?;
    let mut modded: HashMap<String, Vec<String>> = HashMap::new();
    for m in mods::list(&settings.install_mods).map_err(err)? {
        for path in m.models {
            modded.entry(path).or_default().push(m.folder.clone());
        }
    }
    Ok(ModelList {
        models,
        categories: catalog::CATEGORIES.to_vec(),
        modded,
    })
}

#[derive(Serialize)]
struct ModelInfo {
    meshes: Vec<model::MeshInfo>,
    materials: usize,
    bones: usize,
    textures: Vec<model::TextureInfo>,
}

#[tauri::command]
async fn model_info(state: State<'_, App>, path: String) -> Result<ModelInfo> {
    let romfs = state.romfs()?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut decoder = romfs.decoder().map_err(err)?;
        let loaded = Loaded::read(&romfs, &mut decoder, Path::new(&path)).map_err(err)?;
        let bfres = loaded.bfres().map_err(err)?;
        let mut inspection = model::inspect(&romfs, &mut decoder, &bfres).map_err(err)?;
        let first = inspection.models.swap_remove(0);
        Ok(ModelInfo {
            materials: first.materials.len(),
            bones: first.bones.len(),
            meshes: first.meshes,
            textures: inspection.textures,
        })
    })
    .await
    .map_err(err)?
}

/// A model as one `.glb` for the 3D view; with `overlay` (a mod's romfs folder), the mod's version.
#[tauri::command]
async fn model_preview(
    state: State<'_, App>,
    path: String,
    overlay: Option<PathBuf>,
) -> Result<Response> {
    let romfs = romfs_with(&state, overlay.as_deref())?;
    let glb = tauri::async_runtime::spawn_blocking(move || {
        preview::preview(&romfs, Path::new(&path), 1024).map(|p| p.glb)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    Ok(Response::new(glb))
}

/// The game's romfs, with a mod's `romfs` folder on top when one is given.
fn romfs_with(state: &App, overlay: Option<&Path>) -> Result<Arc<Romfs>> {
    match overlay {
        None => state.romfs(),
        Some(dir) => Ok(Arc::new(
            Romfs::open(&state.settings().game_romfs)
                .map_err(err)?
                .with_overlay(dir),
        )),
    }
}

/// Whether a model has cloth (as the romfs, with the overlay, has it).
#[tauri::command]
async fn has_cloth(state: State<'_, App>, path: String, overlay: Option<PathBuf>) -> Result<bool> {
    let romfs = romfs_with(&state, overlay.as_deref())?;
    let cache = state.settings().cache_dir();
    tauri::async_runtime::spawn_blocking(move || {
        cloth::find_cloth(&romfs, Some(&cache), &path).is_some()
    })
    .await
    .map_err(err)
}

/// How long a step of the 3D view's cloth simulation is.
const CLOTH_STEP: f32 = 1.0 / 60.0;
/// At most this many cloth simulations are kept (the oldest goes): a page that never closed its own doesn't pile up.
const CLOTH_SESSIONS: usize = 4;

/// A running cloth simulation of the 3D view's model.
#[derive(Serialize)]
struct ClothSession {
    id: u64,
    source: String,
    size: f32,
    parts: Vec<String>,
    /// The bones the cloth moves, with their rest place in the model.
    bones: Vec<cloth::LiveBone>,
    warnings: Vec<String>,
    /// Where they are now: settled at rest.
    places: Vec<[f32; 16]>,
    /// How long a step is.
    step: f32,
    /// The test motion, a place of the model for every step.
    test_motion: Vec<[f32; 16]>,
}

/// Starts simulating a model's cloth (as the romfs, with the overlay, has it), settled at rest; `None`: no cloth.
#[tauri::command]
async fn cloth_open(
    state: State<'_, App>,
    path: String,
    overlay: Option<PathBuf>,
) -> Result<Option<ClothSession>> {
    let romfs = romfs_with(&state, overlay.as_deref())?;
    let cache = state.settings().cache_dir();
    let Some(live) = tauri::async_runtime::spawn_blocking(move || {
        cloth::open_model(&romfs, Some(&cache), &path)
    })
    .await
    .map_err(err)?
    .map_err(err)?
    else {
        return Ok(None);
    };
    let id = state.next_cloth.fetch_add(1, Ordering::Relaxed);
    let session = ClothSession {
        id,
        source: live.source.clone(),
        size: live.size,
        parts: live.parts.clone(),
        bones: live.bones.clone(),
        warnings: live.warnings.clone(),
        places: live.places(),
        step: CLOTH_STEP,
        test_motion: cloth::test_motion_frames(live.size, 1.0 / CLOTH_STEP),
    };
    let mut cloths = state.cloths.lock().expect("cloths lock");
    while cloths.len() >= CLOTH_SESSIONS {
        let Some(&oldest) = cloths.keys().min() else {
            break;
        };
        cloths.remove(&oldest);
    }
    cloths.insert(id, live);
    Ok(Some(session))
}

/// An animation the cloth follows in a step: the clip (`anim_open`) and its frame at each step.
#[derive(serde::Deserialize)]
struct ClothAnim {
    clip: u64,
    frames: Vec<f32>,
    /// The clip starts here (a new one, or one started again): the cloth settles on its first frame's pose for a
    /// second first, rather than whipping after the jump.
    #[serde(default)]
    settle: bool,
}

/// How long the cloth settles on a clip's first pose ([`ClothAnim::settle`]), in steps.
const CLOTH_SETTLE_STEPS: usize = 60;

/// Steps a cloth simulation once for each place of the model in `models` (column-major 4x4s, at most 8), posed by
/// `anim` where given (the clip's frame for each step), and returns where its bones are then: 16 little-endian floats
/// each, in the order `cloth_open` listed them.
#[tauri::command]
async fn cloth_step(
    state: State<'_, App>,
    id: u64,
    models: Vec<[f32; 16]>,
    anim: Option<ClothAnim>,
) -> Result<Response> {
    let clip = match &anim {
        Some(a) => Some(
            state
                .clips
                .lock()
                .expect("clips lock")
                .get(&a.clip)
                .cloned()
                .ok_or_else(|| "this animation was closed".to_owned())?,
        ),
        None => None,
    };
    let mut cloths = state.cloths.lock().expect("cloths lock");
    let live = cloths
        .get_mut(&id)
        .ok_or_else(|| "this cloth simulation was closed".to_owned())?;
    match (&anim, &clip) {
        (Some(a), Some(clip)) => {
            if a.settle
                && let (Some(model), Some(&frame)) = (models.first(), a.frames.first())
            {
                let posed = clip.pose_by_name(frame);
                for _ in 0..CLOTH_SETTLE_STEPS {
                    live.step_posed(model, &|n| posed.get(n).copied(), CLOTH_STEP)
                        .map_err(err)?;
                }
            }
            for (i, model) in models.iter().take(8).enumerate() {
                let frame = a.frames.get(i).or(a.frames.last()).copied().unwrap_or(0.0);
                let posed = clip.pose_by_name(frame);
                live.step_posed(model, &|n| posed.get(n).copied(), CLOTH_STEP)
                    .map_err(err)?;
            }
        }
        _ => {
            for model in models.iter().take(8) {
                live.step(model, CLOTH_STEP).map_err(err)?;
            }
        }
    }
    let bytes: Vec<u8> = live
        .places()
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    Ok(Response::new(bytes))
}

/// Ends a cloth simulation.
#[tauri::command]
fn cloth_close(state: State<'_, App>, id: u64) {
    state.cloths.lock().expect("cloths lock").remove(&id);
}

/// At most this many animations are kept open (the oldest goes).
const CLIPS: usize = 4;

/// A model's animations (as the romfs, with the overlay, has its files): the ones its actors give it, its own, its
/// cutscenes.
#[tauri::command]
async fn anim_list(
    state: State<'_, App>,
    path: String,
    overlay: Option<PathBuf>,
) -> Result<Vec<anim::AnimEntry>> {
    let romfs = romfs_with(&state, overlay.as_deref())?;
    let cache = state.settings().cache_dir();
    tauri::async_runtime::spawn_blocking(move || anim::list(&romfs, Some(&cache), &path))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Opens animation `name` of `file` with the skeleton of the model `path`, sampled at every frame: the clip as
/// `anim::Clip::to_bytes` lays it out (its id in the header, for `cloth_step` and `anim_close`).
#[tauri::command]
async fn anim_open(
    state: State<'_, App>,
    path: String,
    overlay: Option<PathBuf>,
    file: String,
    name: String,
) -> Result<Response> {
    let romfs = romfs_with(&state, overlay.as_deref())?;
    let clip =
        tauri::async_runtime::spawn_blocking(move || anim::Clip::open(&romfs, &path, &file, &name))
            .await
            .map_err(err)?
            .map_err(err)?;
    let id = state.next_clip.fetch_add(1, Ordering::Relaxed);
    let bytes = clip.to_bytes(id);
    let mut clips = state.clips.lock().expect("clips lock");
    while clips.len() >= CLIPS {
        let Some(&oldest) = clips.keys().min() else {
            break;
        };
        clips.remove(&oldest);
    }
    clips.insert(id, Arc::new(clip));
    Ok(Response::new(bytes))
}

/// Closes an animation.
#[tauri::command]
fn anim_close(state: State<'_, App>, id: u64) {
    state.clips.lock().expect("clips lock").remove(&id);
}

// ---------------------------------------------------------------- projects

#[tauri::command]
async fn list_projects(state: State<'_, App>) -> Result<Vec<project::Summary>> {
    Ok(project::list(&state.settings()))
}

#[derive(Serialize)]
struct ProjectView {
    summary: project::Summary,
    data: ProjectFile,
    last_build: Option<BuildReport>,
    mod_romfs: PathBuf,
    work_dir: PathBuf,
    /// Whether the game has cloth for the model (it can be added to the scene to edit).
    has_cloth: bool,
}

#[tauri::command]
async fn get_project(state: State<'_, App>, file: PathBuf) -> Result<ProjectView> {
    let p = Project::load(&file).map_err(err)?;
    let settings = state.settings();
    Ok(ProjectView {
        summary: p.summary(&settings),
        last_build: p.last_build(),
        mod_romfs: p.mod_romfs(),
        work_dir: p.work_dir(),
        has_cloth: cloth::cloth_file(
            &settings.game_romfs,
            Some(&settings.cache_dir()),
            &p.data.model,
        )
        .is_some(),
        data: p.data,
    })
}

#[tauri::command]
async fn create_project(
    app: AppHandle,
    state: State<'_, App>,
    task: u64,
    model: String,
    folder: String,
) -> Result<project::Summary> {
    let settings = state.settings();
    run_task(app, &state, task, move |progress| {
        let p = project::create(&settings, &model, &folder, progress).map_err(err)?;
        Ok(p.summary(&settings))
    })
    .await
}

#[tauri::command]
async fn build_project(
    app: AppHandle,
    state: State<'_, App>,
    task: u64,
    file: PathBuf,
    install: bool,
    check: bool,
) -> Result<BuildReport> {
    let settings = state.settings();
    let p = Project::load(&file).map_err(err)?;
    let options = build::Options {
        install,
        check,
        skip_blender: false,
    };
    run_task(app, &state, task, move |progress| {
        Ok(build::build(&settings, &p, options, progress))
    })
    .await
}

/// Adds the model's cloth to the project's scene, to edit there (`cloth_edit::add_to_scene`).
#[tauri::command]
async fn add_cloth_to_scene(
    app: AppHandle,
    state: State<'_, App>,
    task: u64,
    file: PathBuf,
) -> Result<usize> {
    let settings = state.settings();
    let mut p = Project::load(&file).map_err(err)?;
    run_task(app, &state, task, move |progress| {
        modstudio_core::cloth_edit::add_to_scene(&settings, &mut p, progress)
            .map(|scene| scene.colliders.len())
            .map_err(err)
    })
    .await
}

#[tauri::command]
fn new_task_id(state: State<'_, App>) -> u64 {
    state.next_task.fetch_add(1, Ordering::Relaxed)
}

#[tauri::command]
fn cancel_task(state: State<'_, App>, task: u64) {
    if let Some(flag) = state.tasks.lock().expect("tasks lock").get(&task) {
        flag.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn open_in_blender(state: State<'_, App>, file: PathBuf) -> Result<()> {
    let p = Project::load(&file).map_err(err)?;
    let settings = state.settings();
    tools::open_in_blender(settings.blender().map_err(err)?, &p.blend()).map_err(err)
}

/// Shows a folder (or a file in its folder) in the system's file manager.
#[tauri::command]
fn show_in_folder(path: PathBuf) -> Result<()> {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        if path.is_file() {
            c.arg(format!("/select,{}", path.display()));
        } else {
            c.arg(&path);
        }
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(if path.is_file() {
            path.parent().unwrap_or(&path)
        } else {
            &path
        });
        c
    };
    cmd.spawn().map(|_| ()).map_err(err)
}

/// A picture the build made (the check's), for the page. Only files in the workspace.
#[tauri::command]
async fn read_image(state: State<'_, App>, path: PathBuf) -> Result<Response> {
    let workspace = state.settings().workspace;
    let real = path.canonicalize().map_err(err)?;
    let root = workspace.canonicalize().map_err(err)?;
    let is_png = real
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !real.starts_with(&root) || !is_png {
        return Err(format!(
            "{} isn't a picture in the workspace",
            path.display()
        ));
    }
    std::fs::read(&real).map(Response::new).map_err(err)
}

// ---------------------------------------------------------------- mods

#[derive(Serialize)]
struct ModsView {
    mods: Vec<mods::InstalledMod>,
    size_table: Option<PathBuf>,
    folder: PathBuf,
}

#[tauri::command]
async fn list_mods(state: State<'_, App>) -> Result<ModsView> {
    let settings = state.settings();
    Ok(ModsView {
        mods: mods::list(&settings.install_mods).map_err(err)?,
        size_table: mods::size_table(&settings.install_mods),
        folder: settings.install_mods,
    })
}

/// Turns a mod on or off and makes the size table for the enabled mods again.
#[tauri::command]
async fn set_mod_enabled(
    app: AppHandle,
    state: State<'_, App>,
    task: u64,
    folder: String,
    enabled: bool,
) -> Result<String> {
    let settings = state.settings();
    run_task(app, &state, task, move |progress| {
        let new = mods::set_enabled(&settings.install_mods, &folder, enabled).map_err(err)?;
        progress.step("Resource size table");
        rebuild_table(&settings, progress)?;
        Ok(new)
    })
    .await
}

#[tauri::command]
async fn rebuild_size_table(
    app: AppHandle,
    state: State<'_, App>,
    task: u64,
) -> Result<Option<PathBuf>> {
    let settings = state.settings();
    run_task(app, &state, task, move |progress| {
        progress.step("Resource size table");
        rebuild_table(&settings, progress)
    })
    .await
}

fn rebuild_table(settings: &Settings, progress: &mut Progress) -> Result<Option<PathBuf>> {
    mods::rebuild_size_table(settings, progress).map_err(err)
}

fn main() {
    let settings_path = Settings::default_path().unwrap_or_else(|| PathBuf::from("settings.json"));
    let settings = Settings::load(&settings_path).unwrap_or_default();
    tauri::Builder::default()
        .setup(|app| {
            let builder =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
                    .title("Hookshot Mod Studio")
                    .inner_size(1320.0, 860.0)
                    .min_inner_size(980.0, 620.0);
            // MODSTUDIO_WEBVIEW_ARGS: extra WebView2 switches, e.g. "--remote-debugging-port=9333" to check
            // the page with DevTools (Windows; not set, nothing changes).
            #[cfg(windows)]
            let builder = match std::env::var("MODSTUDIO_WEBVIEW_ARGS") {
                Ok(extra) => builder.additional_browser_args(&format!(
                    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection {extra}"
                )),
                Err(_) => builder,
            };
            builder.build()?;
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .manage(App {
            settings_path,
            settings: Mutex::new(settings),
            romfs: Mutex::new(None),
            tasks: Mutex::new(HashMap::new()),
            next_task: AtomicU64::new(1),
            cloths: Mutex::new(HashMap::new()),
            next_cloth: AtomicU64::new(1),
            clips: Mutex::new(HashMap::new()),
            next_clip: AtomicU64::new(1),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            settings_for_hookshot,
            list_models,
            model_info,
            model_preview,
            list_projects,
            get_project,
            create_project,
            build_project,
            add_cloth_to_scene,
            new_task_id,
            cancel_task,
            open_in_blender,
            has_cloth,
            cloth_open,
            cloth_step,
            cloth_close,
            anim_list,
            anim_open,
            anim_close,
            show_in_folder,
            read_image,
            list_mods,
            set_mod_enabled,
            rebuild_size_table,
        ])
        .run(tauri::generate_context!())
        .expect("the app's window");
}

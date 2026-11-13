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
use modstudio_core::{mods, preview, tools};
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

/// Settings for a Hookshot folder (its romfs and mods folders), keeping the rest of `current`.
#[tauri::command]
fn settings_for_hookshot(dir: PathBuf, current: Settings) -> Settings {
    let fresh = Settings::for_hookshot(&dir);
    Settings {
        game_romfs: fresh.game_romfs,
        install_mods: fresh.install_mods,
        workspace: if current.workspace.as_os_str().is_empty() {
            fresh.workspace
        } else {
            current.workspace
        },
        tools: modstudio_core::settings::Tools {
            blender: current.tools.blender.or(fresh.tools.blender),
            restbl: current.tools.restbl,
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
    let romfs = match &overlay {
        None => state.romfs()?,
        Some(dir) => Arc::new(
            Romfs::open(&state.settings().game_romfs)
                .map_err(err)?
                .with_overlay(dir),
        ),
    };
    let glb = tauri::async_runtime::spawn_blocking(move || {
        preview::preview(&romfs, Path::new(&path), 1024).map(|p| p.glb)
    })
    .await
    .map_err(err)?
    .map_err(err)?;
    Ok(Response::new(glb))
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
    let work = settings.cache_dir();
    std::fs::create_dir_all(&work).map_err(err)?;
    mods::rebuild_size_table(settings, &work, progress).map_err(err)
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
            new_task_id,
            cancel_task,
            open_in_blender,
            show_in_folder,
            read_image,
            list_mods,
            set_mod_enabled,
            rebuild_size_table,
        ])
        .run(tauri::generate_context!())
        .expect("the app's window");
}

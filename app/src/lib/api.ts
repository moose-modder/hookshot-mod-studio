// The app's Rust side (src-tauri/src/main.rs), typed. Field names follow the Rust structs' JSON.

import { invoke } from "@tauri-apps/api/core";

export interface Tools {
  blender?: string | null;
  restbl?: string | null;
}

export interface Settings {
  game: string;
  game_romfs: string;
  install_mods: string;
  workspace: string;
  tools: Tools;
  game_version?: string | null;
}

export interface Status {
  romfs: boolean;
  mods: boolean;
  workspace: boolean;
  blender: string | null;
  restbl: boolean;
  game_version: string | null;
  problems: string[];
}

export interface SettingsView {
  settings: Settings;
  path: string;
  status: Status;
  blenders: string[];
}

export interface ModelEntry {
  path: string;
  name: string;
  group: string;
  category: string;
  bytes: number;
}

export interface ModelList {
  models: ModelEntry[];
  categories: string[];
  modded: Record<string, string[]>;
}

export interface MeshInfo {
  name: string;
  material: string;
  vertices: number;
  skinCount: number;
  lods: number;
  triangles: number;
  lodTriangles: number[];
  meshlet?: boolean;
}

export interface TextureInfo {
  name: string;
  format: string;
  width: number;
  height: number;
  mips: number;
}

export interface ModelInfo {
  meshes: MeshInfo[];
  materials: number;
  bones: number;
  textures: TextureInfo[];
}

export interface ProjectSummary {
  file: string;
  dir: string;
  folder: string;
  name: string;
  model: string;
  model_name: string;
  blend: string;
  blend_saved: number | null;
  last_build: number | null;
  last_build_ok: boolean | null;
  installed: boolean | null;
  install_dir: string;
}

export interface MeshRow {
  name: string;
  status: string;
  material: string;
  game_vertices: number;
  vertices: number;
  triangles: number;
  skin_count: number;
  lods: number;
  joined?: string[];
}

export interface TextureRow {
  name: string;
  from: string;
  to: string;
  width: number;
  height: number;
}

export interface Difference {
  view: string;
  pose: string;
  kind: string;
  pixels: number;
  meshes: string[];
}

export interface CheckSummary {
  matches: boolean;
  differences: Difference[];
  images: string[];
  not_compared: string[];
}

export interface BuildReport {
  ok: boolean;
  error?: string | null;
  finished: number;
  seconds: number;
  install: boolean;
  meshes: MeshRow[];
  textures: TextureRow[];
  warnings: string[];
  check?: CheckSummary | null;
  files: string[];
  installed?: string | null;
  size_table?: string | null;
}

export interface ProjectFile {
  name: string;
  model: string;
  blend: string;
  prep: Record<string, unknown>;
  check?: Record<string, unknown>;
}

export interface ProjectView {
  summary: ProjectSummary;
  data: ProjectFile;
  last_build: BuildReport | null;
  mod_romfs: string;
  work_dir: string;
}

export interface InstalledMod {
  name: string;
  folder: string;
  enabled: boolean;
  files: number;
  models: string[];
  textures: number;
  own_size_table: boolean;
}

export interface ModsView {
  mods: InstalledMod[];
  size_table: string | null;
  folder: string;
}

export const api = {
  getSettings: () => invoke<SettingsView>("get_settings"),
  saveSettings: (settings: Settings) => invoke<SettingsView>("save_settings", { settings }),
  settingsForHookshot: (dir: string, current: Settings) =>
    invoke<Settings>("settings_for_hookshot", { dir, current }),
  listModels: () => invoke<ModelList>("list_models"),
  modelInfo: (path: string) => invoke<ModelInfo>("model_info", { path }),
  modelPreview: (path: string, overlay?: string) =>
    invoke<ArrayBuffer>("model_preview", { path, overlay: overlay ?? null }),
  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  getProject: (file: string) => invoke<ProjectView>("get_project", { file }),
  openInBlender: (file: string) => invoke<void>("open_in_blender", { file }),
  showInFolder: (path: string) => invoke<void>("show_in_folder", { path }),
  readImage: (path: string) => invoke<ArrayBuffer>("read_image", { path }),
  listMods: () => invoke<ModsView>("list_mods"),
};

/** A short, readable error from a failed command. */
export function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

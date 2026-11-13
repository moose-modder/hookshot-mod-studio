<script lang="ts">
  import {
    api,
    message,
    type ModelEntry,
    type ModelInfo,
    type ModelList,
    type ProjectSummary,
    type SettingsView,
  } from "../lib/api";
  import { bytes, count } from "../lib/format";
  import { nav, openProject } from "../lib/nav.svelte";
  import { Task } from "../lib/task.svelte";
  import TaskLog from "../lib/TaskLog.svelte";
  import Viewer from "../lib/Viewer.svelte";

  let { settings }: { settings: SettingsView } = $props();

  let list = $state<ModelList | null>(null);
  let listError = $state<string | null>(null);
  let projects = $state<ProjectSummary[]>([]);
  let search = $state("");
  let category = $state("Characters");
  let showLevel = $state(false);
  let limit = $state(400);

  let selected = $state<ModelEntry | null>(null);
  let info = $state<ModelInfo | null>(null);
  let infoError = $state<string | null>(null);
  let glb = $state<ArrayBuffer | null>(null);
  let previewError = $state<string | null>(null);
  let loadingPreview = $state(false);
  let showMeshes = $state(false);

  let folder = $state("");
  const task = new Task();

  const ready = $derived(settings.status.romfs);

  const filtered = $derived.by(() => {
    if (!list) return [];
    const words = search.toLowerCase().split(/\s+/).filter(Boolean);
    return list.models.filter(
      (m) =>
        (category === "All" ? m.category !== "Level" || showLevel : m.category === category) &&
        words.every((w) => m.name.toLowerCase().includes(w) || m.group.toLowerCase().includes(w)),
    );
  });

  const counts = $derived.by(() => {
    const c: Record<string, number> = {};
    for (const m of list?.models ?? []) c[m.category] = (c[m.category] ?? 0) + 1;
    return c;
  });

  const existing = $derived(selected ? projects.filter((p) => p.model === selected!.path) : []);
  const modded = $derived(selected ? (list?.modded[selected.path] ?? []) : []);
  const folderTaken = $derived(projects.some((p) => p.folder.toLowerCase() === folder.toLowerCase()));
  const folderValid = $derived(/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(folder));

  async function load() {
    if (!ready) return;
    try {
      [list, projects] = await Promise.all([api.listModels(), api.listProjects()]);
      listError = null;
    } catch (e) {
      listError = message(e);
    }
  }

  async function select(m: ModelEntry) {
    if (task.running) return;
    selected = m;
    folder = m.name;
    info = null;
    infoError = null;
    glb = null;
    previewError = null;
    loadingPreview = true;
    const path = m.path;
    api
      .modelInfo(path)
      .then((i) => {
        if (selected?.path === path) info = i;
      })
      .catch((e) => {
        if (selected?.path === path) infoError = message(e);
      });
    try {
      const data = await api.modelPreview(path);
      if (selected?.path === path) glb = data;
    } catch (e) {
      if (selected?.path === path) previewError = message(e);
    } finally {
      if (selected?.path === path) loadingPreview = false;
    }
  }

  async function create() {
    if (!selected) return;
    const summary = await task.run<ProjectSummary>(`New project ${folder}`, "create_project", {
      model: selected.path,
      folder,
    });
    if (summary) openProject(summary.file);
  }

  function keydown(e: KeyboardEvent) {
    if (!filtered.length || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    e.preventDefault();
    const i = selected ? filtered.findIndex((m) => m.path === selected!.path) : -1;
    const next = filtered[Math.max(0, Math.min(filtered.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
    if (next) {
      void select(next);
      document.getElementById(`m-${next.path}`)?.scrollIntoView({ block: "nearest" });
    }
  }

  $effect(() => {
    void nav.settingsVersion;
    void load();
  });
</script>

<div class="page">
  <header>
    <div class="title">
      <h1>Models</h1>
      <p class="muted">The game's models, from your Hookshot folder. Pick one to see it, then make a project to edit it in Blender.</p>
    </div>
    <div class="filters">
      <input type="search" placeholder="Search by name (e.g. Hylia, Impa, Fairy)" bind:value={search} onkeydown={keydown} />
      <div class="chips">
        {#each ["All", ...(list?.categories ?? []).filter((c) => c !== "Level" || showLevel)] as c (c)}
          <button class="chip" class:on={category === c} onclick={() => { category = c; limit = 400; }}>
            {c}
            {#if c !== "All" && counts[c]}<span class="n">{counts[c]}</span>{/if}
          </button>
        {/each}
        <label class="level"><input type="checkbox" bind:checked={showLevel} onchange={() => { if (!showLevel && category === "Level") category = "All"; }} /> Map pieces</label>
      </div>
    </div>
  </header>

  {#if !ready}
    <div class="notice warn">Choose your Hookshot folder in Settings first.</div>
  {:else if listError}
    <div class="notice bad">{listError}</div>
  {:else}
    <div class="body">
      <div class="list card" role="listbox" tabindex="0" onkeydown={keydown}>
        {#if !list}
          <div class="pad"><span class="spinner"></span></div>
        {:else if !filtered.length}
          <div class="pad muted">No model matches.</div>
        {:else}
          {#each filtered.slice(0, limit) as m (m.path)}
            <button
              id={`m-${m.path}`}
              class="item"
              class:sel={selected?.path === m.path}
              onclick={() => select(m)}
              role="option"
              aria-selected={selected?.path === m.path}
            >
              <span class="nm">{m.name}</span>
              {#if list.modded[m.path]}<span class="pill accent">modded</span>{/if}
              {#if projects.some((p) => p.model === m.path)}<span class="pill">project</span>{/if}
            </button>
          {/each}
          {#if filtered.length > limit}
            <button class="ghost more" onclick={() => (limit = Infinity)}>Show all {filtered.length.toLocaleString()}</button>
          {/if}
        {/if}
        <div class="foot muted">{count(filtered.length, "model")}</div>
      </div>

      <div class="detail">
        <div class="view">
          <Viewer {glb} loading={loadingPreview} error={previewError} />
        </div>
        {#if selected}
          <div class="info card">
            <div class="head">
              <div>
                <h2>{selected.name}</h2>
                <div class="muted meta">
                  {selected.category}
                  {#if selected.group !== selected.name}· in {selected.group}{/if}
                  · {bytes(selected.bytes)}
                  {#if info}
                    · {count(info.meshes.length, "mesh", "meshes")} · {count(info.bones, "bone")} · {count(info.textures.length, "texture")}
                  {/if}
                </div>
              </div>
              {#if modded.length}
                <span class="pill accent" title="An installed mod replaces this model">Modded by {modded.join(", ")}</span>
              {/if}
            </div>
            {#if infoError}<div class="notice bad">{infoError}</div>{/if}

            {#if existing.length}
              <div class="existing">
                {#each existing as p (p.file)}
                  <button onclick={() => openProject(p.file)}>Open project {p.folder} →</button>
                {/each}
              </div>
            {/if}

            {#if task.running || task.error || task.events.length}
              <TaskLog {task} />
            {:else}
              <div class="create">
                <label for="folder">Project name</label>
                <input id="folder" type="text" bind:value={folder} spellcheck="false" />
                <button class="primary" onclick={create} disabled={!folderValid || folderTaken || !settings.status.blender}>
                  Make a project
                </button>
              </div>
              <p class="muted small">
                {#if folderTaken}
                  There's a project called {folder} already.
                {:else if !folderValid}
                  Use letters, digits, _ and - for the name.
                {:else}
                  Exports the model and makes a Blender scene from it in your projects folder (about half a minute).
                {/if}
              </p>
            {/if}

            {#if info}
              <button class="ghost small toggle" onclick={() => (showMeshes = !showMeshes)}>
                {showMeshes ? "Hide" : "Show"} meshes
              </button>
              {#if showMeshes}
                <table>
                  <thead><tr><th>Mesh</th><th>Material</th><th class="num">Vertices</th><th class="num">Triangles</th><th class="num">LODs</th><th class="num">Skin</th></tr></thead>
                  <tbody>
                    {#each info.meshes as mesh (mesh.name)}
                      <tr>
                        <td>{mesh.name}{#if mesh.meshlet} <span class="pill warn">meshlets</span>{/if}</td>
                        <td>{mesh.material}</td>
                        <td class="num">{mesh.vertices.toLocaleString()}</td>
                        <td class="num">{mesh.triangles.toLocaleString()}</td>
                        <td class="num">{mesh.lods}</td>
                        <td class="num">{mesh.skinCount}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {/if}
            {/if}
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    display: flex;
    flex-direction: column;
    padding: 20px 24px;
    gap: 14px;
    min-height: 0;
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .title p {
    margin-top: 2px;
  }
  .filters {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .filters input[type="search"] {
    max-width: 520px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }
  .chip {
    padding: 4px 11px;
    border-radius: 999px;
    font-size: 13px;
  }
  .chip.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--text);
  }
  .chip .n {
    color: var(--muted);
    font-size: 11.5px;
    margin-left: 3px;
  }
  .level {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    color: var(--muted);
    font-size: 13px;
  }
  .body {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(240px, 300px) 1fr;
    gap: 14px;
    min-height: 0;
  }
  .list {
    display: flex;
    flex-direction: column;
    overflow: auto;
    padding: 4px;
    min-height: 0;
    outline: none;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 6px;
    text-align: left;
    background: transparent;
    border: none;
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 13px;
  }
  .item .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .item.sel {
    background: var(--accent-soft);
  }
  .more {
    margin: 6px;
  }
  .foot {
    margin-top: auto;
    padding: 8px;
    font-size: 12px;
    border-top: 1px solid var(--border);
  }
  .pad {
    padding: 16px;
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-height: 0;
    overflow: auto;
  }
  .view {
    flex: 1 0 340px;
    min-height: 340px;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 14px 16px;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 12px;
  }
  .meta {
    font-size: 13px;
    margin-top: 2px;
  }
  .existing {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .create {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .create label {
    color: var(--muted);
    font-size: 13px;
  }
  .create input {
    width: 260px;
  }
  .small {
    font-size: 12.5px;
  }
  .toggle {
    align-self: flex-start;
    padding-left: 0;
  }
</style>

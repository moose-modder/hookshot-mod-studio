<script lang="ts">
  import { onDestroy } from "svelte";
  import { api, message, type BuildReport, type ProjectView, type SettingsView } from "../lib/api";
  import { ago, base, count } from "../lib/format";
  import { go } from "../lib/nav.svelte";
  import { Task } from "../lib/task.svelte";
  import TaskLog from "../lib/TaskLog.svelte";
  import Viewer from "../lib/Viewer.svelte";

  let { file, settings }: { file: string; settings: SettingsView } = $props();

  let view = $state<ProjectView | null>(null);
  let error = $state<string | null>(null);
  let check = $state(true);
  let notice = $state<string | null>(null);
  const task = new Task();
  let showLog = $state(false);

  let tab = $state<"mod" | "game">("mod");
  let glb = $state<ArrayBuffer | null>(null);
  let previewError = $state<string | null>(null);
  let loadingPreview = $state(false);
  let images = $state<{ path: string; url: string }[]>([]);
  let bigImage = $state<string | null>(null);

  const report = $derived<BuildReport | null>(view?.last_build ?? null);
  const built = $derived(!!report?.ok);
  const stale = $derived(
    !!view?.summary.blend_saved && !!view?.summary.last_build && view.summary.blend_saved > view.summary.last_build,
  );
  const canBuild = $derived(settings.status.problems.length === 0);

  async function load() {
    try {
      view = await api.getProject(file);
      error = null;
      if (!view.last_build?.ok) tab = "game";
      void loadPreview();
      void loadImages();
    } catch (e) {
      error = message(e);
    }
  }

  async function loadPreview() {
    if (!view) return;
    const which = tab;
    glb = null;
    previewError = null;
    loadingPreview = true;
    try {
      const data = await api.modelPreview(view.data.model, which === "mod" ? view.mod_romfs : undefined);
      if (tab === which) glb = data;
    } catch (e) {
      if (tab === which) previewError = message(e);
    } finally {
      if (tab === which) loadingPreview = false;
    }
  }

  function dropImages() {
    for (const i of images) URL.revokeObjectURL(i.url);
    images = [];
  }

  async function loadImages() {
    dropImages();
    const paths = view?.last_build?.check?.images ?? [];
    const loaded = [];
    for (const path of paths) {
      try {
        const data = await api.readImage(path);
        loaded.push({ path, url: URL.createObjectURL(new Blob([data], { type: "image/png" })) });
      } catch {
        // (an older build's pictures may be gone)
      }
    }
    images = loaded;
  }

  onDestroy(dropImages);

  async function build(install: boolean) {
    if (!view) return;
    showLog = true;
    notice = null;
    const result = await task.run<BuildReport>(install ? "Build and install" : "Build", "build_project", {
      file,
      install,
      check,
    });
    if (result && !result.ok) task.error = result.error ?? "The build failed";
    await load();
    if (result?.ok) {
      tab = "mod";
      void loadPreview();
    }
  }

  async function openBlender() {
    notice = null;
    try {
      await api.openInBlender(file);
      notice = "Blender is opening the scene. Save it there (Ctrl+S), then come back and build.";
    } catch (e) {
      notice = message(e);
    }
  }

  function setTab(t: "mod" | "game") {
    if (tab === t) return;
    tab = t;
    void loadPreview();
  }

  $effect(() => {
    void file;
    void load();
  });

  const statusClass: Record<string, string> = { kept: "", replaced: "accent", removed: "bad" };
</script>

<div class="page">
  <button class="ghost small back" onclick={() => go("projects")}>← All projects</button>

  {#if error}
    <div class="notice bad">{error}</div>
  {:else if !view}
    <span class="spinner"></span>
  {:else}
    <header>
      <div>
        <h1>{view.data.name}</h1>
        <div class="muted meta">
          {view.summary.model_name} · scene saved {ago(view.summary.blend_saved)}
          {#if view.summary.installed === true}
            · <span class="okText">installed</span>
          {:else if view.summary.installed === false}
            · installed, turned off
          {/if}
        </div>
      </div>
      <div class="actions">
        <button class="ghost" onclick={() => api.showInFolder(view!.summary.dir)}>Show folder</button>
        <button onclick={openBlender} disabled={!settings.status.blender}>Open in Blender</button>
        <button onclick={() => build(false)} disabled={task.running || !canBuild}>Build</button>
        <button class="primary" onclick={() => build(true)} disabled={task.running || !canBuild}>Build and install</button>
      </div>
    </header>

    <div class="options">
      <label><input type="checkbox" bind:checked={check} disabled={task.running} /> Compare the result with the scene</label>
      {#if stale}<span class="pill warn">The scene changed since the last build</span>{/if}
      {#if !canBuild}<span class="pill warn">Finish the setup in Settings to build</span>{/if}
    </div>
    {#if notice}<div class="notice">{notice}</div>{/if}

    {#if showLog && (task.running || task.events.length || task.error)}
      <TaskLog {task} onclose={() => (showLog = false)} />
    {/if}

    <div class="body">
      <section class="preview">
        <div class="tabs">
          <button class="tab" class:on={tab === "mod"} disabled={!built} onclick={() => setTab("mod")}>Your mod</button>
          <button class="tab" class:on={tab === "game"} onclick={() => setTab("game")}>The game's model</button>
        </div>
        <div class="view">
          <Viewer {glb} loading={loadingPreview} error={previewError} empty="Build the mod to see it here" />
        </div>
      </section>

      <section class="report">
        {#if !report}
          <div class="card pad">
            <h2>Not built yet</h2>
            <ol class="steps muted">
              <li><strong>Open in Blender</strong> and edit the scene: shapes, materials, paint.</li>
              <li>Save it in Blender.</li>
              <li><strong>Build and install</strong>: the studio bakes your changes into the game's textures, rebuilds the model, checks it against your scene and puts it in Hookshot's mods folder.</li>
              <li>Restart Hookshot to see it.</li>
            </ol>
          </div>
        {:else}
          <div class="card pad summary">
            {#if report.ok}
              <div class="line"><span class="pill ok">Built</span> {ago(report.finished)} in {Math.round(report.seconds)} s</div>
              {#if report.installed}
                <div class="muted small">Installed to {report.installed}. Restart Hookshot to see it.</div>
              {:else}
                <div class="muted small">Not installed by this build.</div>
              {/if}
            {:else}
              <div class="line"><span class="pill bad">Failed</span> {ago(report.finished)}</div>
              <div class="notice bad">{report.error}</div>
            {/if}
          </div>

          {#if report.check}
            <div class="card pad">
              {#if report.check.matches}
                <div class="line"><span class="pill ok">✓</span> The result looks like your scene: four sides, at rest and in a test pose.</div>
              {:else}
                <div class="line"><span class="pill warn">!</span> The result looks different from your scene in {count(report.check.differences.length, "place")}:</div>
                <ul class="diffs">
                  {#each report.check.differences as d, i (i)}
                    <li><strong>{d.meshes.join(", ")}</strong> <span class="muted">{d.view}, {d.pose}: {d.kind}, {d.pixels} px</span></li>
                  {/each}
                </ul>
                {#if images.length}
                  <div class="muted small">Scene · result · differences in red:</div>
                  <div class="images">
                    {#each images as img (img.path)}
                      <button class="thumb" onclick={() => (bigImage = img.url)} title={base(img.path)}>
                        <img src={img.url} alt={base(img.path)} />
                      </button>
                    {/each}
                  </div>
                {/if}
              {/if}
              {#if report.check.not_compared.length}
                <div class="muted small">Not compared exactly: {report.check.not_compared.join(", ")}</div>
              {/if}
            </div>
          {/if}

          {#if report.warnings.length}
            <div class="card pad">
              <h3>Warnings</h3>
              <ul class="warnings">
                {#each report.warnings as w, i (i)}<li>{w}</li>{/each}
              </ul>
            </div>
          {/if}

          {#if report.meshes.length}
            <div class="card pad">
              <h3>Meshes</h3>
              <table>
                <thead><tr><th>Mesh</th><th></th><th class="num">Vertices</th><th class="num">LODs</th></tr></thead>
                <tbody>
                  {#each report.meshes as m (m.name)}
                    <tr>
                      <td>
                        {m.name}
                        {#if m.joined?.length}<div class="muted small">+ {m.joined.join(", ")}</div>{/if}
                      </td>
                      <td><span class="pill {statusClass[m.status] ?? ''}">{m.status}</span></td>
                      <td class="num">
                        {#if m.vertices !== m.game_vertices}<span class="muted">{m.game_vertices.toLocaleString()} →</span>{/if}
                        {m.vertices.toLocaleString()}
                      </td>
                      <td class="num">{m.lods}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}

          {#if report.textures.length}
            <div class="card pad">
              <h3>Textures changed</h3>
              <table>
                <tbody>
                  {#each report.textures as t (t.name)}
                    <tr><td>{t.name}</td><td class="muted">{t.width}×{t.height}</td><td class="muted">{t.from} → {t.to}</td></tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/if}
      </section>
    </div>
  {/if}
</div>

{#if bigImage}
  <button class="lightbox" onclick={() => (bigImage = null)} title="Close">
    <img src={bigImage} alt="Scene, result and differences" />
  </button>
{/if}

<style>
  .page {
    height: 100%;
    overflow: auto;
    padding: 16px 24px 24px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .back {
    align-self: flex-start;
    margin-left: -10px;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: flex-end;
    gap: 16px;
    flex-wrap: wrap;
  }
  .meta {
    margin-top: 2px;
  }
  .okText {
    color: var(--ok);
  }
  .actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .options {
    display: flex;
    gap: 14px;
    align-items: center;
    flex-wrap: wrap;
    font-size: 13px;
  }
  .options label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .body {
    display: grid;
    grid-template-columns: minmax(360px, 1.1fr) minmax(340px, 1fr);
    gap: 14px;
    align-items: start;
  }
  .preview {
    position: sticky;
    top: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .tabs {
    display: flex;
    gap: 4px;
  }
  .tab {
    background: transparent;
    border-color: transparent;
    color: var(--muted);
  }
  .tab.on {
    background: var(--panel-2);
    border-color: var(--border);
    color: var(--text);
  }
  .view {
    height: min(62vh, 620px);
  }
  .report {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }
  .pad {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .line {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .small {
    font-size: 12.5px;
  }
  .steps {
    margin: 0;
    padding-left: 20px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .steps strong {
    color: var(--text);
  }
  .diffs,
  .warnings {
    margin: 0;
    padding-left: 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
  }
  .warnings li {
    color: var(--warn);
  }
  .images {
    display: grid;
    grid-template-columns: 1fr;
    gap: 8px;
  }
  .thumb {
    padding: 0;
    border-radius: 8px;
    overflow: hidden;
    background: var(--panel-2);
  }
  .thumb img {
    display: block;
    width: 100%;
  }
  .lightbox {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.8);
    border: none;
    border-radius: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 32px;
    z-index: 10;
  }
  .lightbox img {
    max-width: 100%;
    max-height: 100%;
  }
</style>

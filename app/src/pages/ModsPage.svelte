<script lang="ts">
  import { api, message, type ModsView, type SettingsView } from "../lib/api";
  import { base, count } from "../lib/format";
  import { nav } from "../lib/nav.svelte";
  import { Task } from "../lib/task.svelte";
  import TaskLog from "../lib/TaskLog.svelte";

  let { settings }: { settings: SettingsView } = $props();

  let view = $state<ModsView | null>(null);
  let error = $state<string | null>(null);
  let changed = $state(false);
  const task = new Task();

  async function load() {
    try {
      view = await api.listMods();
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function toggle(folder: string, enabled: boolean) {
    const name = folder.replace(/^[-_]+/, "");
    await task.run<string>(`Turning ${name} ${enabled ? "on" : "off"}`, "set_mod_enabled", { folder, enabled });
    changed = true;
    await load();
  }

  async function rebuild() {
    await task.run("Resource size table", "rebuild_size_table", {});
    changed = true;
    await load();
  }

  $effect(() => {
    void nav.settingsVersion;
    void load();
  });
</script>

<div class="page">
  <header>
    <div>
      <h1>Installed mods</h1>
      <p class="muted">
        The mods in Hookshot's mods folder. Turning one off renames its folder with a leading <code>-</code>, which
        Hookshot skips. Hookshot reads its mods when it starts.
      </p>
    </div>
    <div class="actions">
      {#if view}<button class="ghost" onclick={() => api.showInFolder(view!.folder)}>Show folder</button>{/if}
      <button onclick={rebuild} disabled={task.running || !settings.status.restbl}>Rebuild size table</button>
    </div>
  </header>

  {#if changed && !task.running && !task.error}
    <div class="notice ok">Done. Restart Hookshot to load the change.</div>
  {/if}
  {#if task.running || task.error}
    <TaskLog {task} />
  {/if}

  {#if error}
    <div class="notice bad">{error}</div>
  {:else if !view}
    <span class="spinner"></span>
  {:else if !view.mods.length}
    <div class="card empty">
      <h2>No mods installed</h2>
      <p class="muted">Build a project with "Build and install" and it shows up here.</p>
    </div>
  {:else}
    <div class="card list">
      {#each view.mods as m (m.folder)}
        <div class="mod" class:off={!m.enabled}>
          <label class="switch" title={m.enabled ? "Turn off" : "Turn on"}>
            <input
              type="checkbox"
              checked={m.enabled}
              disabled={task.running}
              onchange={(e) => toggle(m.folder, (e.currentTarget as HTMLInputElement).checked)}
            />
            <span class="track"><span class="knob"></span></span>
          </label>
          <div class="what">
            <div class="name">{m.name}</div>
            <div class="muted small">
              {count(m.files, "file")}
              {#if m.models.length}· {m.models.map((p) => base(p).split(".")[1] ?? base(p)).join(", ")}{/if}
              {#if m.textures}· {count(m.textures, "texture")}{/if}
            </div>
          </div>
          {#if m.own_size_table}
            <span class="pill" title="Its own resource size table isn't used: the shared one covers every mod">own size table</span>
          {/if}
          <span class="pill" class:ok={m.enabled}>{m.enabled ? "on" : "off"}</span>
        </div>
      {/each}
    </div>
    <p class="muted small">
      {#if view.size_table}
        Shared resource size table: <code>{base(view.size_table)}</code> in <code>zzz-ResourceSizeTable</code>, made for every mod that's on.
      {:else}
        No shared resource size table: no mod that's on needs one.
      {/if}
    </p>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    overflow: auto;
    padding: 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 1000px;
  }
  header {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    align-items: flex-end;
  }
  header p {
    margin-top: 4px;
    max-width: 640px;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .list {
    display: flex;
    flex-direction: column;
  }
  .mod {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .mod:last-child {
    border-bottom: none;
  }
  .mod.off .name {
    color: var(--muted);
  }
  .what {
    flex: 1;
    min-width: 0;
  }
  .name {
    font-weight: 600;
  }
  .small {
    font-size: 12.5px;
  }
  .empty {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .switch {
    position: relative;
    display: inline-block;
    cursor: pointer;
  }
  .switch input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }
  .track {
    display: block;
    width: 38px;
    height: 22px;
    border-radius: 999px;
    background: var(--border);
    transition: background 0.15s;
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    transition: transform 0.15s;
  }
  .switch input:checked + .track {
    background: var(--ok);
  }
  .switch input:checked + .track .knob {
    transform: translateX(16px);
  }
  .switch input:focus-visible + .track {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .switch input:disabled + .track {
    opacity: 0.5;
  }
</style>

<script lang="ts">
  import { untrack } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, message, type Settings, type SettingsView } from "../lib/api";
  import { parent } from "../lib/format";

  let { view, onsaved }: { view: SettingsView; onsaved: (v: SettingsView) => void } = $props();

  // An editable copy; saved with Save.
  let draft = $state<Settings>(untrack(() => structuredClone($state.snapshot(view.settings))));
  let saving = $state(false);
  let error = $state<string | null>(null);
  let savedNote = $state(false);

  const hookshot = $derived(draft.game_romfs ? parent(draft.game_romfs) : "");
  const status = $derived(view.status);
  const ready = $derived(status.problems.length === 0);
  const changed = $derived(JSON.stringify($state.snapshot(draft)) !== JSON.stringify(view.settings));

  async function pickFolder(title: string): Promise<string | null> {
    const picked = await open({ directory: true, title });
    return typeof picked === "string" ? picked : null;
  }

  async function pickFile(title: string, name: string, extensions: string[]): Promise<string | null> {
    const picked = await open({ title, filters: extensions.length ? [{ name, extensions }] : [] });
    return typeof picked === "string" ? picked : null;
  }

  async function chooseHookshot() {
    const dir = await pickFolder("Your Hookshot folder (the one with hookshot.exe, romfs and mods)");
    if (dir) draft = await api.settingsForHookshot(dir, $state.snapshot(draft));
  }

  async function save() {
    saving = true;
    error = null;
    try {
      const clean = $state.snapshot(draft);
      clean.tools = {
        blender: clean.tools.blender || null,
        restbl: clean.tools.restbl || null,
      };
      const v = await api.saveSettings(clean);
      draft = structuredClone(v.settings);
      onsaved(v);
      savedNote = true;
      setTimeout(() => (savedNote = false), 2500);
    } catch (e) {
      error = message(e);
    } finally {
      saving = false;
    }
  }

  const exe = navigator.userAgent.includes("Windows") ? ["exe"] : [];
</script>

<div class="page">
  <header>
    <h1>Settings</h1>
    {#if !ready}
      <p class="muted">
        Point the studio at your Hookshot install, your Blender and the size table tool. Nothing from the game
        leaves your PC: models are read from your own Hookshot folder, and mods go into its mods folder.
      </p>
    {/if}
  </header>

  <section class="card form">
    <div class="row">
      <div class="label">
        <h3>Hookshot folder</h3>
        <p class="muted">Holds <code>romfs</code> (the game's files) and <code>mods</code>.</p>
      </div>
      <div class="field">
        <div class="value" class:empty={!hookshot}>{hookshot || "Not chosen"}</div>
        <button onclick={chooseHookshot}>Choose…</button>
      </div>
      <div class="check">{@render tick(status.romfs && status.mods)}</div>
    </div>

    <div class="row">
      <div class="label">
        <h3>Projects folder</h3>
        <p class="muted">Each project's Blender scene and build files.</p>
      </div>
      <div class="field">
        <input type="text" bind:value={draft.workspace} placeholder="C:\Users\you\HookshotModStudio" />
        <button onclick={async () => { const d = await pickFolder("Folder for projects"); if (d) draft.workspace = d; }}>Choose…</button>
      </div>
      <div class="check">{@render tick(status.workspace)}</div>
    </div>

    <div class="row">
      <div class="label">
        <h3>Blender</h3>
        <p class="muted">Version 5 or newer. {status.blender ?? ""}</p>
      </div>
      <div class="field">
        <input type="text" bind:value={draft.tools.blender} placeholder="blender.exe" />
        <button onclick={async () => { const f = await pickFile("Blender", "Blender", exe); if (f) draft.tools.blender = f; }}>Choose…</button>
      </div>
      <div class="check">{@render tick(!!status.blender)}</div>
    </div>
    {#if view.blenders.length > 1 || (view.blenders.length === 1 && view.blenders[0] !== draft.tools.blender)}
      <div class="found">
        Found:
        {#each view.blenders as b (b)}
          <button class="ghost small" onclick={() => (draft.tools.blender = b)}>{b}</button>
        {/each}
      </div>
    {/if}

    <div class="row">
      <div class="label">
        <h3>Size table tool</h3>
        <p class="muted">
          <code>restbl</code> from RESTBL Calculator: works out the game's resource size table for mods. (Built in later.)
        </p>
      </div>
      <div class="field">
        <input type="text" bind:value={draft.tools.restbl} placeholder="restbl.exe" />
        <button onclick={async () => { const f = await pickFile("restbl", "restbl", exe); if (f) draft.tools.restbl = f; }}>Choose…</button>
      </div>
      <div class="check">{@render tick(status.restbl)}</div>
    </div>

    <details>
      <summary class="muted">Advanced</summary>
      <div class="row">
        <div class="label"><h3>Game files (romfs)</h3></div>
        <div class="field"><input type="text" bind:value={draft.game_romfs} /></div>
        <div class="check">{@render tick(status.romfs)}</div>
      </div>
      <div class="row">
        <div class="label"><h3>Mods folder</h3></div>
        <div class="field"><input type="text" bind:value={draft.install_mods} /></div>
        <div class="check">{@render tick(status.mods)}</div>
      </div>
      <p class="muted small">Settings file: <code>{view.path}</code></p>
    </details>
  </section>

  {#if status.problems.length}
    <section class="notice warn">
      {#each status.problems as p (p)}<div>• {p}</div>{/each}
    </section>
  {:else}
    <section class="notice ok">
      Ready{status.game_version ? ` · game version ${status.game_version.split("").join(".")}` : ""} · {status.blender}
    </section>
  {/if}
  {#if error}<section class="notice bad">{error}</section>{/if}

  <footer>
    <button class="primary" onclick={save} disabled={saving || !changed}>
      {saving ? "Checking…" : "Save"}
    </button>
    {#if savedNote}<span class="muted">Saved</span>{/if}
  </footer>
</div>

{#snippet tick(ok: boolean)}
  {#if ok}
    <span class="pill ok" title="Found">✓</span>
  {:else}
    <span class="pill warn" title="Missing">!</span>
  {/if}
{/snippet}

<style>
  .page {
    height: 100%;
    overflow: auto;
    padding: 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 980px;
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .form {
    padding: 6px 16px;
  }
  .row {
    display: grid;
    grid-template-columns: 230px 1fr 34px;
    gap: 16px;
    align-items: center;
    padding: 12px 0;
    border-bottom: 1px solid var(--border);
  }
  details .row:last-of-type {
    border-bottom: none;
  }
  .label p {
    font-size: 12.5px;
  }
  .field {
    display: flex;
    gap: 8px;
    min-width: 0;
  }
  .field input {
    flex: 1;
  }
  .value {
    flex: 1;
    padding: 7px 10px;
    border-radius: 8px;
    background: var(--panel-2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .value.empty {
    color: var(--muted);
  }
  .check {
    text-align: center;
  }
  .found {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    align-items: center;
    padding: 6px 0 10px;
    border-bottom: 1px solid var(--border);
    color: var(--muted);
    font-size: 12.5px;
  }
  details {
    padding: 10px 0;
  }
  summary {
    cursor: pointer;
    padding: 4px 0;
  }
  .small {
    font-size: 12px;
    padding-top: 6px;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 12px;
  }
</style>

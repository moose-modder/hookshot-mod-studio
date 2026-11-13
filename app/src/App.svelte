<script lang="ts">
  import { onMount } from "svelte";
  import { api, message, type SettingsView } from "./lib/api";
  import { nav, go, type Page } from "./lib/nav.svelte";
  import ModelsPage from "./pages/ModelsPage.svelte";
  import ProjectsPage from "./pages/ProjectsPage.svelte";
  import ModsPage from "./pages/ModsPage.svelte";
  import SettingsPage from "./pages/SettingsPage.svelte";

  let view = $state<SettingsView | null>(null);
  let loadError = $state<string | null>(null);

  const pages: { id: Page; label: string; icon: string }[] = [
    { id: "models", label: "Models", icon: "M4 7l8-4 8 4-8 4-8-4zm0 5l8 4 8-4M4 17l8 4 8-4" },
    { id: "projects", label: "Projects", icon: "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" },
    { id: "mods", label: "Installed mods", icon: "M5 12l4 4L19 6" },
    { id: "settings", label: "Settings", icon: "M12 15a3 3 0 100-6 3 3 0 000 6zm7-3l2-1-2-4-2 1-2-1V5h-4v2l-2 1-2-1-2 4 2 1v0l-2 1 2 4 2-1 2 1v2h4v-2l2-1 2 1 2-4-2-1z" },
  ];

  async function load() {
    try {
      view = await api.getSettings();
      loadError = null;
    } catch (e) {
      loadError = message(e);
    }
  }

  function ready(v: SettingsView | null): boolean {
    return !!v && v.status.problems.length === 0;
  }

  onMount(async () => {
    await load();
    if (!ready(view)) nav.page = "settings";
  });

  function saved(v: SettingsView) {
    view = v;
    nav.settingsVersion++;
  }
</script>

<div class="shell">
  <aside>
    <div class="brand">
      <svg viewBox="0 0 32 32" width="28" height="28" aria-hidden="true">
        <rect x="1" y="1" width="30" height="30" rx="7" fill="#18222b" />
        <path d="M19 9v9a5 5 0 01-10 0" fill="none" stroke="#e9b949" stroke-width="3" stroke-linecap="round" />
        <circle cx="19" cy="6.2" r="2.2" fill="none" stroke="#e9b949" stroke-width="1.6" />
      </svg>
      <div>
        <div class="name">Hookshot</div>
        <div class="sub">Mod Studio</div>
      </div>
    </div>
    <nav>
      {#each pages as p (p.id)}
        <button class="ghost" class:active={nav.page === p.id} onclick={() => go(p.id)}>
          <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
            <path d={p.icon} fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round" stroke-linecap="round" />
          </svg>
          {p.label}
        </button>
      {/each}
    </nav>
    <div class="status">
      {#if view}
        {#if ready(view)}
          <div><span class="dot ok"></span>Game {view.status.game_version ? `v${view.status.game_version.split("").join(".")}` : ""}</div>
          <div><span class="dot ok"></span>{view.status.blender}</div>
        {:else}
          <button class="ghost small warnlink" onclick={() => go("settings")}>
            <span class="dot warn"></span>Setup needed
          </button>
        {/if}
      {:else if loadError}
        <div class="bad">{loadError}</div>
      {/if}
    </div>
  </aside>

  <main>
    {#if view}
      {#if nav.page === "models"}
        <ModelsPage settings={view} />
      {:else if nav.page === "projects"}
        <ProjectsPage settings={view} />
      {:else if nav.page === "mods"}
        <ModsPage settings={view} />
      {:else}
        <SettingsPage {view} onsaved={saved} />
      {/if}
    {:else if !loadError}
      <div class="loading"><span class="spinner"></span></div>
    {/if}
  </main>
</div>

<style>
  .shell {
    display: grid;
    grid-template-columns: 216px 1fr;
    height: 100%;
  }
  aside {
    display: flex;
    flex-direction: column;
    background: var(--sidebar);
    border-right: 1px solid var(--border);
    padding: 16px 10px;
    gap: 18px;
    min-height: 0;
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 8px;
  }
  .brand .name {
    font-weight: 700;
    font-size: 15px;
    line-height: 1.1;
  }
  .brand .sub {
    color: var(--muted);
    font-size: 12px;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav button {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    padding: 8px 10px;
    color: var(--muted);
  }
  nav button.active {
    background: var(--accent-soft);
    color: var(--text);
  }
  nav button.active svg {
    color: var(--accent);
  }
  .status {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .status .bad {
    color: var(--bad);
  }
  .warnlink {
    color: var(--warn);
    padding: 4px 0;
    text-align: left;
  }
  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    margin-right: 7px;
    vertical-align: 1px;
  }
  .dot.ok {
    background: var(--ok);
  }
  .dot.warn {
    background: var(--warn);
  }
  main {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .loading {
    display: flex;
    height: 100%;
    align-items: center;
    justify-content: center;
  }
</style>

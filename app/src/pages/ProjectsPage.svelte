<script lang="ts">
  import { api, message, type ProjectSummary, type SettingsView } from "../lib/api";
  import { ago } from "../lib/format";
  import { go, nav, openProject } from "../lib/nav.svelte";
  import ProjectDetail from "./ProjectDetail.svelte";

  let { settings }: { settings: SettingsView } = $props();

  let projects = $state<ProjectSummary[] | null>(null);
  let error = $state<string | null>(null);

  async function load() {
    try {
      projects = await api.listProjects();
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  $effect(() => {
    void nav.settingsVersion;
    if (!nav.project) void load();
  });
</script>

{#if nav.project}
  {#key nav.project}
    <ProjectDetail file={nav.project} {settings} />
  {/key}
{:else}
  <div class="page">
    <header>
      <h1>Projects</h1>
      <p class="muted">
        Each project is one model being modded: its Blender scene, and the mod built from it.
        Start one from <button class="link" onclick={() => go("models")}>Models</button>.
      </p>
    </header>
    {#if error}
      <div class="notice bad">{error}</div>
    {:else if !projects}
      <span class="spinner"></span>
    {:else if !projects.length}
      <div class="empty card">
        <h2>No projects yet</h2>
        <p class="muted">Pick a model, look at it, and make a project: the studio exports it and opens a Blender scene for you to edit.</p>
        <button class="primary" onclick={() => go("models")}>Browse models</button>
      </div>
    {:else}
      <div class="grid">
        {#each projects as p (p.file)}
          {@const stale = p.blend_saved && p.last_build && p.blend_saved > p.last_build}
          <button class="project card" onclick={() => openProject(p.file)}>
            <div class="top">
              <h2>{p.name}</h2>
              {#if p.installed === true}
                <span class="pill ok">installed</span>
              {:else if p.installed === false}
                <span class="pill">turned off</span>
              {/if}
            </div>
            <div class="muted model">{p.model_name}</div>
            <div class="facts">
              <div><span class="muted">Scene saved</span> {ago(p.blend_saved)}</div>
              <div>
                <span class="muted">Last build</span>
                {#if p.last_build}
                  {ago(p.last_build)}
                  {#if p.last_build_ok === false}<span class="pill bad">failed</span>{/if}
                {:else}
                  never
                {/if}
              </div>
            </div>
            {#if stale}<div class="pill warn">scene changed since the build</div>{/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .page {
    height: 100%;
    overflow: auto;
    padding: 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 14px;
  }
  .project {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
    padding: 16px;
    text-align: left;
    white-space: normal;
  }
  .project:hover {
    border-color: var(--accent);
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  .model {
    font-size: 13px;
    margin-top: -4px;
  }
  .facts {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 13px;
  }
  .facts .muted {
    display: inline-block;
    width: 92px;
  }
  .project .pill.warn {
    align-self: flex-start;
  }
  .empty {
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-start;
    max-width: 560px;
  }
</style>

// Which page shows, shared by the sidebar and the pages that link to each other.

export type Page = "models" | "projects" | "mods" | "settings";

export const nav = $state({
  page: "models" as Page,
  /** The open project's project.json, or null for the list. */
  project: null as string | null,
  /** Bumped when settings change, so pages load again. */
  settingsVersion: 0,
});

export function go(page: Page) {
  nav.page = page;
  if (page === "projects") nav.project = null;
}

export function openProject(file: string) {
  nav.page = "projects";
  nav.project = file;
}

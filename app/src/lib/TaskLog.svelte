<script lang="ts">
  // A running (or finished) task's steps and output, newest at the bottom, with Cancel while it runs.
  import type { Task } from "./task.svelte";

  let { task, onclose }: { task: Task; onclose?: () => void } = $props();
  let box: HTMLDivElement;

  // Follow the output while it comes, unless the user scrolled up.
  $effect(() => {
    void task.events.length;
    if (box && box.scrollHeight - box.scrollTop - box.clientHeight < 80) {
      queueMicrotask(() => (box.scrollTop = box.scrollHeight));
    }
  });

  const steps = $derived(task.events.filter((e) => e.kind === "step").length);
</script>

<section class="task">
  <header>
    <div class="title">
      {#if task.running}<span class="spinner"></span>{/if}
      <strong>{task.title}</strong>
      {#if task.running}
        <span class="muted">{task.cancelling ? "stopping…" : `step ${Math.max(steps, 1)}`}</span>
      {:else if task.error}
        <span class="pill bad">failed</span>
      {:else}
        <span class="pill ok">done</span>
      {/if}
    </div>
    {#if task.running}
      <button class="small" onclick={() => task.cancel()} disabled={task.cancelling}>Cancel</button>
    {:else if onclose}
      <button class="ghost small" onclick={onclose}>Hide</button>
    {/if}
  </header>
  <div class="log" bind:this={box}>
    {#each task.events as e, i (i)}
      {#if e.kind === "step"}
        <div class="step">{e.text}</div>
      {:else if e.kind === "warning"}
        <div class="line warn">⚠ {e.text}</div>
      {:else}
        <div class="line">{e.text}</div>
      {/if}
    {/each}
    {#if task.error}
      <div class="error">{task.error}</div>
    {/if}
  </div>
</section>

<style>
  .task {
    flex: none;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--panel);
    min-height: 0;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .log {
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.5;
    padding: 8px 12px 12px;
    overflow: auto;
    max-height: 340px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .step {
    margin-top: 8px;
    font-family: var(--sans);
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .line {
    color: var(--muted);
  }
  .warn {
    color: var(--warn);
  }
  .error {
    margin-top: 10px;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--bad-bg);
    color: var(--bad);
  }
</style>

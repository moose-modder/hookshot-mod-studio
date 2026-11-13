// A long task on the Rust side (a new project, a build, the size table): its progress arrives as `task`
// events, shown by TaskLog, and it can be cancelled.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "./api";

export interface LogEvent {
  kind: "step" | "line" | "warning";
  text: string;
}

interface TaskEventPayload extends LogEvent {
  task: number;
}

export class Task {
  id = 0;
  title = $state("");
  events = $state<LogEvent[]>([]);
  running = $state(false);
  cancelling = $state(false);
  error = $state<string | null>(null);

  /** Runs `command` with `args` (and the task's id); resolves to its result, or null if it failed. */
  async run<T>(title: string, command: string, args: Record<string, unknown>): Promise<T | null> {
    this.title = title;
    this.events = [];
    this.error = null;
    this.running = true;
    this.cancelling = false;
    this.id = await invoke<number>("new_task_id");
    const id = this.id;
    const unlisten = await listen<TaskEventPayload>("task", (e) => {
      if (e.payload.task === id) {
        this.events.push({ kind: e.payload.kind, text: e.payload.text });
      }
    });
    try {
      return await invoke<T>(command, { ...args, task: id });
    } catch (e) {
      this.error = message(e);
      return null;
    } finally {
      unlisten();
      this.running = false;
    }
  }

  cancel() {
    if (this.running) {
      this.cancelling = true;
      void invoke("cancel_task", { task: this.id });
    }
  }
}

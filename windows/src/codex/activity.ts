import { State } from "../core/state";

export interface ActivityEntry {
  id: string;
  kind: "message" | "tool" | "status";
  text: string;
  detail?: string;
  timestamp: number;
}

export interface ActivitySession {
  id: string;
  cwd: string;
  title: string;
  state: "working" | "thinking" | "finished" | "interrupted" | "idle";
  updatedAt: number;
  entries: ActivityEntry[];
}

export interface ActivitySnapshot { sessions: ActivitySession[] }

export function codexActivityStatus(state: ActivitySession["state"]): string {
  switch (state) {
    case "working": return "Working";
    case "thinking": return "Thinking";
    case "finished": return "Finished";
    case "interrupted": return "Interrupted";
    case "idle": return "Idle";
  }
}

export function codexProjectName(cwd: string): string {
  return cwd.split(/[\\/]/).filter(Boolean).at(-1) || cwd || "Local session";
}

class ActivityStore {
  sessions: ActivitySession[] = [];
  selectedId: string | null = null;
  loaded = false;
  error = "";
  private snapshotKey = "";

  get latest(): ActivitySession | null { return this.sessions[0] ?? null; }
  get latestEntry(): ActivityEntry | null { return this.latest?.entries.at(-1) ?? null; }
  get selected(): ActivitySession | null {
    return this.sessions.find((session) => session.id === this.selectedId) ?? null;
  }

  /** Backend snapshots are authoritative and already bounded by the monitor. */
  apply(snapshot: ActivitySnapshot): void {
    const active = (session: ActivitySession) => session.state === "working" || session.state === "thinking";
    const sessions = snapshot.sessions.map((session) => ({
      id: session.id, cwd: session.cwd, title: session.title,
      state: session.state, updatedAt: session.updatedAt,
      entries: session.entries.map((entry) => ({
        id: entry.id, kind: entry.kind, text: entry.text,
        detail: entry.detail, timestamp: entry.timestamp,
      })).sort((a, b) => a.timestamp - b.timestamp),
    })).sort((a, b) => Number(active(b)) - Number(active(a)) || b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
    const key = JSON.stringify(sessions);
    if (this.loaded && !this.error && key === this.snapshotKey) return;
    this.sessions = sessions;
    this.snapshotKey = key;
    this.loaded = true;
    this.error = "";
    if (!sessions.some((session) => session.id === this.selectedId)) {
      this.selectedId = sessions[0]?.id ?? null;
    }
    State.notify();
  }

  select(id: string): void {
    if (id === this.selectedId || !this.sessions.some((session) => session.id === id)) return;
    this.selectedId = id;
    State.notify();
  }

  setError(message: string): void {
    if (this.loaded && this.error === message) return;
    this.loaded = true;
    this.error = message;
    State.notify();
  }
}

export const CodexActivity = new ActivityStore();

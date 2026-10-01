// Local Codex session events → island state.

import { onEvent } from "../core/bridge";
import { Sound } from "../core/sound";
import { State } from "../core/state";
import type { Island } from "./island";

const CODEX_ID = "integration_codex";

let finishTimeout: number | null = null;
let currentSessionId = "";

interface SessionPayload {
  hook_event_name?: string;
  session_id?: string;
  cwd?: string;
  message?: string;
  /** UserPromptSubmit carries `prompt`; `message` belongs to Notification/Stop. */
  prompt?: string;
  tool_name?: string;
  tool_input?: Record<string, unknown>;
}

function lastPathComponent(p: string): string {
  const cleaned = p.replace(/[\\/]+$/, "");
  const idx = Math.max(cleaned.lastIndexOf("\\"), cleaned.lastIndexOf("/"));
  return idx >= 0 ? cleaned.slice(idx + 1) : cleaned;
}

/** Short labels for normalized tool activity. */
const TOOL_LABELS: Record<string, string> = {
  Bash: "Exécute",
  Read: "Lit",
  Write: "Écrit",
  Edit: "Modifie",
  Glob: "Cherche",
  Grep: "Recherche",
  WebSearch: "Recherche web",
  WebFetch: "Récupère",
  TodoWrite: "Tâches",
  Task: "Agent",
  LS: "Liste",
  MultiEdit: "Modifie",
  NotebookEdit: "Notebook",
};

function stepLabel(tool: string, input: Record<string, unknown>): string {
  const label = TOOL_LABELS[tool] ?? tool;
  const str = (k: string) => (typeof input[k] === "string" ? (input[k] as string) : null);
  const cmd = str("command");
  if (cmd) return `${label} · ${cmd.slice(0, 40)}`;
  const path = str("path");
  if (path) return `${label} · ${lastPathComponent(path)}`;
  const file = str("file_path");
  if (file) return `${label} · ${lastPathComponent(file)}`;
  const query = str("query");
  if (query) return `${label} · ${query.slice(0, 40)}`;
  return label;
}

function upsert(projectName: string, cwd: string) {
  const t = State.tasks.find((x) => x.id === CODEX_ID);
  if (!t) return;
  t.name = projectName;
  if (cwd) t.sessionCwd = cwd;
}

function clearSession() {
  const t = State.tasks.find((x) => x.id === CODEX_ID);
  if (!t) return;
  t.steps = [];
  t.stepIndex = 0;
  t.name = State.agentAppLabel;
  t.pillBadge = null;
  t.sessionId = null;
}

export async function registerSessionHandlers(island: Island) {
  await onEvent<SessionPayload>("codex-session", (payload) => handleSession(island, payload));
}

export function handleSession(island: Island, payload: SessionPayload) {
  if (State.paused) return;

  const name = payload.hook_event_name ?? "";
  if (["SessionStart", "UserPromptSubmit", "PreToolUse", "SessionEnd", "Stop", "StopFailure"].includes(name) && finishTimeout !== null) {
    window.clearTimeout(finishTimeout);
    finishTimeout = null;
  }
  if (payload.session_id && payload.session_id !== currentSessionId) {
    clearSession();
    currentSessionId = payload.session_id;
  }
  if (payload.session_id) {
    const task = State.tasks.find((task) => task.id === CODEX_ID);
    if (task) task.sessionId = payload.session_id;
  }
  const cwd = payload.cwd ?? "";
  const raw = lastPathComponent(cwd);
  const projectName = raw || "Session";
  const focused = State.focusId === CODEX_ID;

  /** Alerts force the island open; work events only reveal the compact island. */
  const surface = (view: Parameters<Island["alert"]>[0], isAlert: boolean) => {
    if (State.mode === "expanded") {
      if (isAlert && State.view !== "codex") island.setView(view);
    } else if (isAlert) {
      island.alert(view);
    } else if (State.mode === "hidden") {
      island.reveal();
    }
  };

  switch (name) {
    case "SessionStart":
      upsert(projectName, cwd);
      surface("overview", false);
      Sound.play("work");
      break;

    case "UserPromptSubmit": {
      upsert(projectName, cwd);
      State.updateTask(CODEX_ID, "thinking");
      // The field is `prompt`; reading `message` meant this step was always blank.
      const asked = payload.prompt ?? payload.message;
      if (asked) State.appendStep(CODEX_ID, asked.slice(0, 60));
      surface("overview", false);
      break;
    }

    case "PreToolUse": {
      upsert(projectName, cwd);
      State.updateTask(CODEX_ID, "working");
      const tool = payload.tool_name ?? "Tool";
      State.appendStep(CODEX_ID, stepLabel(tool, payload.tool_input ?? {}));
      surface("overview", false);
      break;
    }

    case "PostToolUse":
      State.updateTask(CODEX_ID, "working");
      break;

    case "PostToolUseFailure":
      State.updateTask(CODEX_ID, "working");
      State.appendStep(CODEX_ID, "⚠ failed");
      break;

    case "Notification": {
      const message = payload.message ?? "";
      const lower = message.toLowerCase();
      if (lower.includes("rate limit") || lower.includes("limite d")) {
        State.updateTask(CODEX_ID, "ratelimit");
        Sound.play("rate");
      } else if (message.endsWith("?")) {
        State.updateTask(CODEX_ID, "question");
        State.appendStep(CODEX_ID, message);
      }
      break;
    }

    case "Stop":
      State.updateTask(CODEX_ID, "finished");
      if (payload.message) State.appendStep(CODEX_ID, payload.message.slice(0, 60));
      Sound.play("finish");
      if (focused) surface("finished", true);
      else State.setPillBadge(CODEX_ID, "finished");
      finishTimeout = window.setTimeout(() => {
        finishTimeout = null;
        State.updateTask(CODEX_ID, "idle");
        State.setPillBadge(CODEX_ID, null);
      }, 5200);
      break;

    case "StopFailure":
      State.updateTask(CODEX_ID, "error");
      Sound.play("error");
      if (focused) surface("error", true);
      else State.setPillBadge(CODEX_ID, "error");
      break;

    case "SessionEnd":
      State.updateTask(CODEX_ID, "idle");
      clearSession();
      break;

    case "SubagentStart":
      State.appendStep(CODEX_ID, "+ subagent");
      break;

    case "SubagentStop":
      State.appendStep(CODEX_ID, "• subagent done");
      break;

    default:
      break;
  }
  State.notify();
}

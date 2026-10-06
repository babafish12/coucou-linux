// Development-only fixture: the real island with public updates supplied locally.
import "../src/style.css";
import { CodexActivity, type ActivitySnapshot } from "../src/codex/activity";
import { Bridge } from "../src/core/bridge";
import { State } from "../src/core/state";
import { Sound } from "../src/core/sound";
import { Island } from "../src/island/island";

const style = document.createElement("style");
style.textContent = `
  body { background: #252932; }
  #preview-controls { position: fixed; top: 345px; left: 24px; right: 24px; color: #d8dde5; font: 13px/1.7 system-ui; }
  #preview-controls p { color: #a4afbf; }
  #preview-controls button { background: #394556; border: 1px solid #607084; border-radius: 6px; color: white; padding: 5px 10px; margin: 10px 8px 10px 0; }
  #preview-controls label { white-space: nowrap; }
  #preview-status { display: block; }
`;
document.head.append(style);
const output = document.querySelector<HTMLOutputElement>("#preview-status")!;
const focusError = document.querySelector<HTMLInputElement>("#preview-focus-error")!;
let focusRequests = 0;
let sequence = 0;
let snapshot: ActivitySnapshot;

Bridge.focusWindow = async () => null;
Bridge.focusCodex = async (cwd) => {
  focusRequests++;
  output.textContent = `Fixture focus requests: ${focusRequests} · Project: ${cwd || "latest session"}`;
  if (focusError.checked) throw new Error("No Codex window found. Start Codex and try again.");
};
Bridge.openCodexSession = async (sessionId) => {
  output.textContent = `Fixture opened chat: ${sessionId}`;
  if (focusError.checked) throw new Error("The selected chat could not be opened. Focus Codex and try again.");
};
State.settings.activeIntegrations = [];
State.settings.soundEnabled = false;
State.settings.autoCloseInterval = 30;
Sound.setEnabled(false);
State.loadIntegrationTasks();
State.setFocus("integration_codex");

function apply() {
  CodexActivity.apply(snapshot);
  const latest = CodexActivity.latest;
  const task = State.tasks.find((task) => task.id === "integration_codex")!;
  task.state = latest?.state === "working" ? "working" : latest?.state === "finished" ? "finished" : "idle";
  task.steps = latest?.entries.map((entry) => entry.text) ?? [];
  task.stepIndex = Math.max(0, task.steps.length - 1);
  task.sessionCwd = latest?.cwd;
}

function reset() {
  const now = Date.now();
  snapshot = { sessions: [
    {
      id: "island-performance", cwd: "/home/steve/rnd/coucou", title: "Keep the compact island visible", state: "working", updatedAt: now,
      entries: [
        { id: "start", kind: "status", text: "Turn started", timestamp: now - 300_000 },
        { id: "message-1", kind: "message", text: "I found an idle timer that hides the island completely. I’m checking how Pause and auto-close use the same state transition.", timestamp: now - 240_000 },
        { id: "tool-1", kind: "tool", text: "Read island state machine", detail: "sed -n '1,220p' linux/src/island/fsm.ts", timestamp: now - 200_000 },
        { id: "message-2", kind: "message", text: "Auto-close should return to the compact island. I’ll keep explicit Pause behavior and add a regression test for several minutes of idle time.", timestamp: now - 160_000 },
        { id: "tool-2", kind: "tool", text: "Update idle visibility handling", detail: "linux/src/island/fsm.ts\nlinux/tests/island-fsm.test.mjs", timestamp: now - 120_000 },
        { id: "tool-3", kind: "tool", text: "Run island regression tests", detail: "node scripts/test-island-performance.mjs", timestamp: now - 60_000 },
        { id: "message-3", kind: "message", text: "The idle test now passes. I’m checking the release build and the launcher used by the application menu.", timestamp: now },
      ],
    },
    {
      id: "dashboard", cwd: "/home/steve/rnd/dashboard", title: "Fix chart labels", state: "finished", updatedAt: now - 600_000,
      entries: [
        { id: "dashboard-message", kind: "message", text: "The labels now use the same timezone as the report. The focused tests pass.", timestamp: now - 600_001 },
        { id: "dashboard-finished", kind: "status", text: "Turn completed", timestamp: now - 600_000 },
      ],
    },
  ] };
  apply();
}
reset();
const island = new Island(document.querySelector<HTMLElement>("#root")!);
island.applySettings();
island.alert("overview");

document.querySelector("#preview-home")!.addEventListener("click", () => island.alert("overview"));
document.querySelector("#preview-detail")!.addEventListener("click", () => island.alert("codex"));
document.querySelector("#preview-update")!.addEventListener("click", () => {
  const session = snapshot.sessions[0];
  if (!session) return;
  session.updatedAt = Date.now();
  session.entries.push({ id: `new-${++sequence}`, kind: "message", timestamp: session.updatedAt, text: `Public progress update ${sequence}: checking the new launcher against the release executable.` });
  apply();
});
document.querySelector("#preview-switch")!.addEventListener("click", () => {
  const session = snapshot.sessions[1];
  if (!session) return;
  session.state = "working";
  session.updatedAt = Date.now();
  session.entries.push({ id: `other-${++sequence}`, kind: "tool", timestamp: session.updatedAt, text: "Run chart label tests", detail: "npm test -- chart-labels" });
  apply();
});
document.querySelector("#preview-finished")!.addEventListener("click", () => {
  const session = snapshot.sessions[0];
  if (!session) return;
  session.state = "finished";
  session.updatedAt = Date.now();
  session.entries.push({ id: `done-${++sequence}`, kind: "status", timestamp: session.updatedAt, text: "Turn completed" });
  apply();
});
document.querySelector("#preview-empty")!.addEventListener("click", () => { snapshot = { sessions: [] }; apply(); });
document.querySelector("#preview-error")!.addEventListener("click", () => CodexActivity.setError("The local session monitor could not refresh. Showing the last received activity."));
document.querySelector("#preview-reset")!.addEventListener("click", reset);

// Development-only fixture for the real chat UI. Every bridge action is local.
import "../src/style.css";
import { State } from "../src/core/state";
import { Bridge, type ChatEvent } from "../src/core/bridge";
import { Sound } from "../src/core/sound";
import { Island } from "../src/island/island";

const style = document.createElement("style");
style.textContent = `
  body { background: #252932; }
  #preview-controls { position: fixed; top: 550px; left: 24px; right: 24px; z-index: 20; color: #d8dde5; font: 13px/1.7 system-ui; }
  #preview-controls p { color: #a4afbf; margin: 5px 0; }
  #preview-controls button { background: #394556; border: 1px solid #607084; border-radius: 6px; color: white; padding: 5px 10px; margin: 10px 8px 10px 0; }
  #preview-controls button:disabled { opacity: .45; }
  #preview-status { display: block; }
`;
document.head.append(style);

type Scenario = "stream" | "approval" | "failure" | "slow";
type Pending = {
  id: string;
  controller: AbortController;
  approve?: (allow: boolean) => void;
};
const output = document.querySelector<HTMLOutputElement>("#preview-status")!;
const scenarioButtons = document.querySelectorAll<HTMLButtonElement>("[data-scenario]");
let scenario: Scenario = "stream";
let pending: Pending | null = null;
let requests = 0;
let chunks = 0;
let decisions = 0;
let stopped = 0;

function report(message = "") {
  output.textContent = `Fixture requests: ${requests} · Stream chunks: ${chunks} · Decisions: ${decisions} · Stops: ${stopped}${message ? ` · ${message}` : ""}`;
  for (const button of scenarioButtons) button.disabled = pending !== null;
}

function wait(milliseconds: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(new Error("Fixture reply stopped.")); return; }
    const cancel = () => { window.clearTimeout(timer); reject(new Error("Fixture reply stopped.")); };
    const timer = window.setTimeout(() => {
      signal.removeEventListener("abort", cancel);
      resolve();
    }, milliseconds);
    signal.addEventListener("abort", cancel, { once: true });
  });
}

Bridge.chatSend = async (_query, _context, requestId, onProgress) => {
  const request: Pending = { id: requestId, controller: new AbortController() };
  pending = request;
  requests++;
  const currentScenario = scenario;
  const signal = request.controller.signal;
  const emit = (event: Omit<ChatEvent, "requestId">) => onProgress({ requestId, ...event });
  const metadata = { model: "gpt-6-luna", reasoningEffort: "low" };
  let text = "";
  report(currentScenario);
  try {
    emit({ kind: "status", text: "Fixture connected", ...metadata });
    await wait(160, signal);
    if (currentScenario === "approval") {
      const approved = new Promise<boolean>((resolve) => { request.approve = resolve; });
      emit({
        kind: "approval", approvalId: "fixture-command", text: "Run a local command? (Preview only)",
        command: "printf 'Hello from the preview\\n'", cwd: "/home/demo/project",
      });
      report("Choose Allow once or Deny in the chat");
      const timeout = new AbortController();
      const cancelTimeout = () => timeout.abort();
      signal.addEventListener("abort", cancelTimeout, { once: true });
      let allow: boolean;
      try {
        allow = await Promise.race([
          approved,
          wait(45_000, timeout.signal).then(() => { throw new Error("Fixture approval timed out."); }),
        ]);
      } finally {
        timeout.abort();
        signal.removeEventListener("abort", cancelTimeout);
        request.approve = undefined;
      }
      if (signal.aborted) throw new Error("Fixture reply stopped.");
      emit({ kind: "approvalResolved", approvalId: "fixture-command" });
      text = allow ? "Approval received. This preview runs no command. " : "Command denied. Nothing was run. ";
      emit({ kind: "delta", itemId: "fixture-answer", text, ...metadata });
    }
    const words = currentScenario === "failure"
      ? ["A partial reply ", "is already visible. "]
      : currentScenario === "slow"
        ? ["This ", "reply ", "arrives ", "slowly. ", "Press ", "Stop ", "to ", "keep ", "the ", "partial ", "answer."]
        : ["The first words ", "appear immediately ", "while the answer ", "is still arriving.\n\n", "Model and reasoning ", "stay visible throughout. ", "<b>HTML stays plain text.</b>"];
    for (const word of words) {
      await wait(currentScenario === "slow" ? 700 : 180, signal);
      text += word;
      chunks++;
      emit({ kind: "delta", itemId: "fixture-answer", text: word, ...metadata });
      report(currentScenario);
    }
    if (currentScenario === "failure") throw new Error("Fixture connection failed after a partial reply.");
    return { text, ...metadata };
  } finally {
    if (pending === request) pending = null;
    report("Ready");
  }
};
Bridge.chatApprove = async (requestId, approvalId, allow) => {
  if (pending?.id !== requestId || approvalId !== "fixture-command" || !pending.approve) {
    throw new Error("No matching fixture approval.");
  }
  decisions++;
  pending.approve(allow);
  report(allow ? "Allowed once" : "Denied");
};
Bridge.chatCancel = async (requestId) => {
  if (pending?.id !== requestId) return;
  stopped++;
  pending.controller.abort();
  report("Stop requested");
};
Bridge.chatReset = async () => { pending?.controller.abort(); return null; };
Bridge.focusWindow = async () => null;
Bridge.openCodexSession = async () => { report("Mock chat navigation"); };
Bridge.openUrl = async () => null;
Bridge.openSettingsWindow = async () => null;

State.settings.activeIntegrations = [];
State.settings.soundEnabled = false;
Sound.setEnabled(false);
State.loadIntegrationTasks();
const island = new Island(document.querySelector<HTMLElement>("#root")!);
island.applySettings();
island.alert("prompt");

const prompts: Record<Scenario, string> = {
  stream: "Show me a streamed answer.",
  approval: "Preview a local command approval.",
  failure: "Show a connection failure after some text.",
  slow: "Write slowly so I can stop you.",
};
for (const button of scenarioButtons) {
  button.addEventListener("click", () => {
    if (pending) return;
    scenario = button.dataset.scenario as Scenario;
    island.alert("prompt");
    const input = document.querySelector<HTMLInputElement>(".chat-input")!;
    input.value = prompts[scenario];
    document.querySelector<HTMLButtonElement>(".send-btn")!.click();
  });
}
document.querySelector("#preview-reset")!.addEventListener("click", () => {
  pending?.controller.abort();
  State.chatHistory = [];
  State.stateOverride = null;
  State.notify();
  island.alert("prompt");
});
report("Ready");

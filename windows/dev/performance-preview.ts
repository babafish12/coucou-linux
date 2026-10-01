// Development-only measurements around the real Island; no account or network APIs.
import "../src/style.css";
import { State } from "../src/core/state";
import { Bridge } from "../src/core/bridge";
import { Sound } from "../src/core/sound";
import { Island } from "../src/island/island";
import { Telegram, type TelegramChat, type TelegramMessage } from "../src/telegram/api";
import { selectInlineChat } from "../src/telegram/inline";
import { TelegramNotifications } from "../src/telegram/notifications";

const style = document.createElement("style");
style.textContent = `
  body { background: #252932; overflow: auto; }
  #performance-controls { position: relative; z-index: 20; margin: 350px 24px 24px; color: #d8dde5; font: 13px/1.6 system-ui; }
  #performance-controls h1 { font-size: 18px; }
  #performance-controls p { color: #a4afbf; max-width: 1000px; margin: 6px 0; }
  .performance-actions { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin: 14px 0; }
  #performance-controls button, #performance-controls select { background: #394556; border: 1px solid #607084; border-radius: 6px; color: white; padding: 6px 10px; font: inherit; }
  #performance-controls button:disabled, #performance-controls select:disabled { opacity: .45; }
  #performance-status { display: block; min-height: 24px; }
  .performance-table { overflow-x: auto; margin-top: 12px; }
  #performance-controls table { border-collapse: collapse; width: 100%; font-variant-numeric: tabular-nums; }
  #performance-controls th, #performance-controls td { padding: 7px 12px; border-bottom: 1px solid #ffffff18; text-align: right; white-space: nowrap; }
  #performance-controls th:first-child, #performance-controls td:first-child { text-align: left; }
  #performance-controls details { margin-top: 14px; }
  #performance-json { user-select: text; white-space: pre-wrap; font: 11px/1.5 monospace; }
`;
document.head.append(style);

const scenarios = ["overview", "overview-searching", "compact", "prompt", "telegram", "notification", "hidden"] as const;
type Scenario = typeof scenarios[number];
interface Sample {
  started: number;
  frames: number[];
  callbacks: number[];
  clears: Record<string, number>;
  beginPaths: number;
  pathConstructions: number;
  backgrounded: boolean;
}
interface Result {
  label: string;
  scenario: Scenario;
  elapsedMs: number;
  frames: number;
  appRafPerSecond: number;
  intervalP50Ms: number | null;
  intervalP95Ms: number | null;
  callbackCount: number;
  callbackTotalMs: number;
  callbackP95Ms: number | null;
  canvasClears: number;
  clearsByCanvas: Record<string, number>;
  beginPaths: number;
  pathConstructions: number;
  backgrounded: boolean;
  browserRafIntervalMs: number;
  warnings: string[];
  preparedMiniBots: { compact: number; overview: number };
  viewport: string;
  dpr: number;
  userAgent: string;
}
let activeSample: Sample | null = null;
let measuring = false;
const results: Result[] = [];
const label = new URLSearchParams(location.search).get("label") ?? "working tree";
const status = document.querySelector<HTMLOutputElement>("#performance-status")!;
const selection = document.querySelector<HTMLSelectElement>("#performance-scenario")!;

// Wrap only this fixture's app RAF callbacks; the measurement timer uses setTimeout.
const requestFrame = window.requestAnimationFrame.bind(window);
window.requestAnimationFrame = (callback) => requestFrame((timestamp) => {
  const sample = activeSample;
  if (!sample) { callback(timestamp); return; }
  if (sample.frames.at(-1) !== timestamp) sample.frames.push(timestamp);
  const started = performance.now();
  try { callback(timestamp); }
  finally { sample.callbacks.push(performance.now() - started); }
});

const canvasLabels = new WeakMap<HTMLCanvasElement, string>();
const context = CanvasRenderingContext2D.prototype;
const clearRect = context.clearRect;
context.clearRect = function (...args) {
  if (activeSample) {
    let name = canvasLabels.get(this.canvas);
    if (!name) {
      name = this.canvas.id || (this.canvas.closest("#mini-grid") ? "compact-mini" : "overview-mini");
      canvasLabels.set(this.canvas, name);
    }
    activeSample.clears[name] = (activeSample.clears[name] ?? 0) + 1;
  }
  clearRect.apply(this, args);
};
const beginPath = context.beginPath;
context.beginPath = function () {
  if (activeSample) activeSample.beginPaths++;
  beginPath.call(this);
};
window.Path2D = new Proxy(window.Path2D, {
  construct(target, args, newTarget) {
    if (activeSample) activeSample.pathConstructions++;
    return Reflect.construct(target, args, newTarget);
  },
});

// Override the bridges before constructing views. Even interactive sends stay local.
for (const key of Object.keys(Bridge)) {
  (Bridge as unknown as Record<string, () => Promise<null>>)[key] = async () => null;
}
Bridge.chatSend = async () => ({ text: "Local preview: no request was sent." });
const chats: TelegramChat[] = [
  { id: "101", title: "Mara", lastMessage: "Treffen wir uns am See?", unreadCount: 2 },
  { id: "202", title: "Design Team", lastMessage: "Die Animation ist bereit.", unreadCount: 1 },
];
const messages: TelegramMessage[] = Array.from({ length: 12 }, (_, index) => ({
  id: String(index + 1), date: 1_700_000_000 + index * 60,
  text: index % 2 ? "Ja, bis später!" : "Eine lokale Beispielnachricht für die Vorschau.",
  outgoing: index % 2 === 1, senderName: "Mara",
}));
for (const key of Object.keys(Telegram)) {
  (Telegram as unknown as Record<string, () => Promise<never>>)[key] = async () => { throw new Error("Account actions are disabled in this preview."); };
}
Telegram.chats = async () => chats.map((chat) => ({ ...chat }));
Telegram.history = async () => messages.map((message) => ({ ...message }));
State.agentProvider = "codex";
State.settings.activeIntegrations = ["integration_telegram", "integration_github", "integration_vercel", "integration_n8n"];
State.settings.soundEnabled = false;
State.settings.autoCloseInterval = 30;
Sound.setEnabled(false);
State.integrations.integration_telegram = {
  configured: true, loaded: true, error: null,
  data: { state: "ready", paused: false, accountName: "Local fixture", unreadCount: 3, unreadChatCount: 2 },
};
State.chatHistory = [
  { id: 1, role: "user", content: "Wie funktioniert diese Vorschau?" },
  { id: 2, role: "assistant", content: "Sie verwendet lokale Beispieldaten und die echte Inseloberfläche." },
];
State.loadIntegrationTasks();
const island = new Island(document.querySelector<HTMLElement>("#root")!);
island.applySettings();

function showScenario(scenario: Scenario) {
  island.fsm.cancelTimers();
  State.stateOverride = scenario === "overview-searching" ? "searching" : null;
  TelegramNotifications.current = null;
  State.setFocus(scenario === "telegram" ? "integration_telegram" : "integration_claude");
  if (scenario === "hidden") island.fsm.forceHidden();
  else if (scenario === "compact") island.collapse();
  else {
    if (scenario === "telegram") selectInlineChat(chats[0]);
    if (scenario === "notification") {
      TelegramNotifications.current = {
        id: "preview-notification", chat: chats[0], message: messages[0],
      };
    }
    island.alert(scenario === "notification" ? "telegram-notification" : scenario === "overview-searching" ? "overview" : scenario);
  }
  // Benchmarks keep each scenario steady, independently of auto-close settings.
  State.isPinned = true;
  island.fsm.pinned = true;
  island.fsm.cancelTimers();
  island.onCursor(0, 600);
  State.notify();
  selection.value = scenario;
}

const delay = (milliseconds: number) => new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));
const rounded = (value: number) => Math.round(value * 1000) / 1000;
function nextBrowserFrame(): Promise<number> {
  return new Promise((resolve, reject) => {
    const timeout = window.setTimeout(() => {
      window.cancelAnimationFrame(frame);
      reject(new Error("No browser frame for 5 seconds. Keep the preview visible, then retry."));
    }, 5_000);
    const frame = requestFrame((timestamp) => {
      window.clearTimeout(timeout);
      resolve(timestamp);
    });
  });
}
function miniBotCounts() {
  return {
    compact: document.querySelectorAll("#mini-grid canvas").length,
    overview: document.querySelectorAll(".overview .pills canvas").length,
  };
}
function measurementWarnings(scenario: Scenario, backgrounded: boolean, browserInterval: number, appRafPerSecond: number): string[] {
  const warnings: string[] = [];
  if (backgrounded) warnings.push("Tab was backgrounded; timing sample is invalid.");
  if (browserInterval > 100) warnings.push("Browser RAF probe is slow; possible throttling or overload. Do not compare timing results.");
  if (scenario === "overview-searching" && appRafPerSecond < 15) {
    warnings.push("Searching should animate continuously, but fewer than 15 app frames/s were delivered.");
  }
  return warnings;
}
function percentile(values: number[], fraction: number): number | null {
  if (!values.length) return null;
  const ordered = [...values].sort((a, b) => a - b);
  return rounded(ordered[Math.min(ordered.length - 1, Math.ceil(ordered.length * fraction) - 1)]);
}

function renderResults() {
  const body = document.querySelector<HTMLTableSectionElement>("#performance-results")!;
  body.replaceChildren();
  for (const result of results) {
    const row = body.insertRow();
    const values = [result.scenario + (result.warnings.length ? " · timing warning" : ""),
      result.appRafPerSecond.toFixed(1), result.intervalP95Ms == null ? "—" : `${result.intervalP95Ms} ms`,
      `${result.callbackTotalMs} ms`, result.canvasClears, result.beginPaths, result.pathConstructions];
    for (const value of values) row.insertCell().textContent = String(value);
    if (result.warnings.length) {
      const warning = body.insertRow().insertCell();
      warning.colSpan = values.length;
      warning.style.whiteSpace = "normal";
      warning.textContent = result.warnings.join(" ");
    }
  }
  document.querySelector<HTMLElement>("#performance-json")!.textContent = JSON.stringify(results, null, 2);
}

async function measureScenario(scenario: Scenario) {
  status.textContent = `${label} · ${scenario} · preparing both mini-bot groups…`;
  if (miniBotCounts().overview !== 4) {
    showScenario("overview");
    await nextBrowserFrame();
  }
  showScenario("compact");
  const compactStarted = performance.now();
  await nextBrowserFrame();
  const preparedMiniBots = miniBotCounts();
  if (preparedMiniBots.compact !== 4 || preparedMiniBots.overview !== 4) {
    throw new Error("Both groups must contain four mini bots before measuring. Reload the preview and retry.");
  }
  await delay(Math.max(0, 500 - (performance.now() - compactStarted)));
  showScenario(scenario);
  status.textContent = `${label} · ${scenario} · settling and checking browser frame delivery…`;
  const settlingStarted = performance.now();
  // Native probe frames bypass app instrumentation and finish before measurement.
  const firstBrowserFrame = await nextBrowserFrame();
  const browserRafInterval = await nextBrowserFrame() - firstBrowserFrame;
  await delay(Math.max(0, 1_000 - (performance.now() - settlingStarted)));
  if (document.hidden) throw new Error("Keep the preview tab visible, then retry.");
  status.textContent = `${label} · ${scenario} · measuring 3 seconds…`;
  const sample: Sample = {
    started: performance.now(), frames: [], callbacks: [], clears: {}, beginPaths: 0,
    pathConstructions: 0, backgrounded: false,
  };
  activeSample = sample;
  await delay(3_000);
  activeSample = null;
  const elapsed = performance.now() - sample.started;
  const appRafPerSecond = sample.frames.length * 1_000 / elapsed;
  const intervals = sample.frames.slice(1).map((timestamp, index) => timestamp - sample.frames[index]);
  results.push({
    label, scenario, elapsedMs: rounded(elapsed), frames: sample.frames.length,
    appRafPerSecond: rounded(appRafPerSecond),
    intervalP50Ms: percentile(intervals, .5), intervalP95Ms: percentile(intervals, .95),
    callbackCount: sample.callbacks.length, callbackTotalMs: rounded(sample.callbacks.reduce((a, b) => a + b, 0)),
    callbackP95Ms: percentile(sample.callbacks, .95),
    canvasClears: Object.values(sample.clears).reduce((a, b) => a + b, 0), clearsByCanvas: sample.clears,
    beginPaths: sample.beginPaths, pathConstructions: sample.pathConstructions, backgrounded: sample.backgrounded,
    browserRafIntervalMs: rounded(browserRafInterval), preparedMiniBots,
    warnings: measurementWarnings(scenario, sample.backgrounded, browserRafInterval, appRafPerSecond),
    viewport: `${window.innerWidth}×${window.innerHeight}`, dpr: window.devicePixelRatio, userAgent: navigator.userAgent,
  });
  renderResults();
}

async function run(all: boolean) {
  if (measuring) return;
  measuring = true;
  const controls = document.querySelectorAll<HTMLButtonElement | HTMLSelectElement>("#performance-controls button, #performance-controls select");
  controls.forEach((control) => { control.disabled = true; });
  try {
    if (all) results.length = 0;
    for (const scenario of all ? scenarios : [selection.value as Scenario]) await measureScenario(scenario);
    status.textContent = `${label} · complete${results.some((result) => result.warnings.length) ? "; check timing warnings below" : ""}. Canvas counts include offscreen canvases if the implementation draws them.`;
  } catch (error) { status.textContent = String(error); }
  finally {
    activeSample = null;
    measuring = false;
    controls.forEach((control) => { control.disabled = false; });
  }
}

// Pointer motion would intentionally wake the island and distort a steady sample.
window.addEventListener("mousemove", (event) => { if (measuring) event.stopImmediatePropagation(); }, true);
document.addEventListener("visibilitychange", () => { if (activeSample && document.hidden) activeSample.backgrounded = true; });
selection.addEventListener("change", () => showScenario(selection.value as Scenario));
document.querySelector("#performance-measure")!.addEventListener("click", () => void run(false));
document.querySelector("#performance-all")!.addEventListener("click", () => void run(true));
Object.assign(window, { coucouPerformance: { results, runAll: () => run(true), measure: () => run(false) } });
showScenario("overview");
status.textContent = `${label} · ready. No Telegram account, Codex request or integration polling is used.`;

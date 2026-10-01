// Thin wrapper over the Tauri commands/events. Every call is a no-op when the
// page is opened in a plain browser, so the island can be iterated on with
// `npm run dev` alone.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { Settings } from "./state";
import type { TelegramNotification } from "../telegram/api";
import type { ActivitySnapshot } from "../codex/activity";

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!IS_TAURI) return null;
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    console.error(`[coucou] ${cmd} failed`, err);
    return null;
  }
}

export interface BootInfo {
  settings: Settings;
  /** Logical screen rect of the monitor the island lives on. */
  screen: { x: number; y: number; width: number; height: number; scale: number };
  version: string;
}

export const Bridge = {
  boot: () => call<BootInfo>("boot"),
  codexMonitorReady: () => call<void>("codex_monitor_ready"),
  codexActivity: () => callOrThrow<ActivitySnapshot>("codex_activity"),
  focusCodex: (cwd?: string) => callOrThrow<void>("focus_codex_window", { cwd: cwd ?? null }),
  codexStatus: () => call<{ installed: boolean; loggedIn: boolean; message: string }>("codex_status"),
  codexModels: (refresh = false) => callOrThrow<CodexModel[]>("codex_models", { refresh }),
  codexSetPreferences: (model: string, reasoningEffort: string) =>
    callOrThrow<Settings>("codex_set_preferences", { model, reasoningEffort }),

  saveSettings: (settings: Settings) => call<void>("save_settings", { settings }),

  /** Shrink the window down to the invisible wake strip (hidden) or back to full. */
  setCollapsed: (collapsed: boolean) => call<void>("set_collapsed", { collapsed }),

  /**
   * Pushes the island shape in window coordinates. Rust flips click-through from
   * its own cursor poll, so the flag is never a frame behind a click.
   */
  setIslandRect: (x: number, y: number, width: number, height: number) =>
    call<void>("set_island_rect", { x, y, width, height }),

  /** Give the window keyboard focus (chat field) and take it away again. */
  focusWindow: (focused: boolean) => call<void>("focus_window", { focused }),

  reposition: () => call<void>("reposition"),

  openUrl: (url: string) => call<void>("open_url", { url }),

  /** Focus the exact monitored chat in Codex Desktop. */
  openCodexSession: (sessionId: string) => callOrThrow<void>("open_codex_session", { sessionId }),

  quit: () => call<void>("quit_app"),

  openSettingsWindow: () => call<void>("open_settings_window"),

  /** Writes to the application log in the XDG data directory. */
  log: (message: string) => call<void>("log_line", { message }),

  monitorStatus: () => call<MonitorStatus>("monitor_status"),

  // ── Chat, files, secrets ──────────────────────────────────────────────────
  chatWarmup: () => call<void>("chat_warmup"),
  /** Stream this turn over a dedicated channel, registered before invocation. */
  chatSend: (query: string, context: ChatContext | null, requestId: string, onProgress: (event: ChatEvent) => void) => {
    const progress = new Channel<ChatEvent>();
    progress.onmessage = onProgress;
    return callOrThrow<{ text: string; model?: string; reasoningEffort?: string }>("chat_send", { query, context, requestId, progress });
  },
  chatApprove: (requestId: string, approvalId: string, allow: boolean) =>
    callOrThrow<void>("chat_approve", { requestId, approvalId, allow }),
  chatCancel: (requestId: string) => callOrThrow<void>("chat_cancel", { requestId }),
  chatReset: () => call<void>("chat_reset"),
  /** Copies a dropped file into the inbox. */
  ingestFile: (path: string) => callOrThrow<DroppedFile>("ingest_file", { path }),
  /** Only ever tells you whether a key exists — never its value. */
  secretPresent: (key: string) => call<boolean>("secret_present", { key }),
  secretSet: (key: string, value: string) => callOrThrow<void>("secret_set", { key, value }),
  secretClear: (key: string) => callOrThrow<void>("secret_clear", { key }),

  // ── Integrations ──────────────────────────────────────────────────────────
  refreshIntegration: (id: string) => call<void>("refresh_integration", { id }),
  /** Cached Telegram status, unread totals and fresh notification previews. */
  telegramSummary: () => callOrThrow<TelegramSummary>("telegram_summary"),
  openTelegramWindow: () => callOrThrow<void>("open_telegram_window"),
  /** Opens the configured n8n instance in the browser. */
  openN8n: () => call<void>("open_n8n"),

  /** Tray → Pause. Stops the integration pollers, not just the island. */
  setPaused: (paused: boolean) => call<void>("set_paused", { paused }),
};

export interface IntegrationUpdate {
  id: string;
  data: Record<string, unknown>;
  error: string | null;
  event: { success: boolean; label: string; detail: string | null } | null;
}

export interface TelegramSummary {
  state: string;
  configured: boolean;
  runtimeAvailable: boolean;
  paused: boolean;
  accountName?: string | null;
  message?: string | null;
  error?: string | null;
  unreadCount: number | null;
  unreadChatCount: number | null;
  notifications?: TelegramNotification[];
}

export interface CodexModel {
  model: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  defaultReasoningEffort: string;
  supportedReasoningEfforts: { reasoningEffort: string; description: string }[];
}

export type ChatContext =
  | { kind: "file"; name: string; path: string }
  | { kind: "window"; appName: string; title: string; url?: string };

export interface ChatEvent {
  requestId: string;
  kind: "delta" | "message" | "status" | "approval" | "approvalResolved";
  text?: string;
  itemId?: string;
  model?: string;
  reasoningEffort?: string;
  approvalId?: string;
  command?: string;
  cwd?: string;
}

export interface DroppedFile {
  name: string;
  path: string;
  size: number;
}

export interface MonitorStatus {
  available: boolean;
  sessionsPath: string;
}

/** Same as `call`, but surfaces the error so the UI can show what went wrong. */
async function callOrThrow<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!IS_TAURI) throw new Error("not running inside Coucou");
  return invoke<T>(cmd, args);
}

export type BridgeEvent =
  | { name: "cursor"; payload: { x: number; y: number } }
  | { name: "tray"; payload: string }
  | { name: "codex-session"; payload: Record<string, unknown> }
  | { name: "screen-changed"; payload: null };

export interface DragDropPayload {
  type: "enter" | "over" | "drop" | "leave";
  paths?: string[];
  /** Physical pixels relative to the webview; absent on leave. */
  position?: { x: number; y: number };
}

/** Files dragged onto the island. Only reaches us when the window takes the mouse. */
export async function onDragDrop(handler: (e: DragDropPayload) => void) {
  if (!IS_TAURI) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    handler(event.payload as DragDropPayload);
  });
}

export async function onEvent<T>(name: string, handler: (payload: T) => void) {
  if (!IS_TAURI) return () => {};
  return listen<T>(name, (e) => handler(e.payload));
}

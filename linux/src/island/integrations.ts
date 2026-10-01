// Integration events → island state. A new item flips the pill to
// finished/error, badges it when
// the pill isn't focused, plays a sound, and clears itself after 60 s.

import { onEvent, Bridge, type IntegrationUpdate } from "../core/bridge";
import { Sound } from "../core/sound";
import { State } from "../core/state";
import type { Island } from "./island";
import { processTelegramNotifications } from "../telegram/notifications";

/** Which system keyring entry backs each pill. */
const KEY_FOR: Record<string, string> = {
  integration_stripe: "stripe-api-key",
  integration_github: "github-token",
  integration_vercel: "vercel-token",
  integration_n8n: "n8n-api-key",
  integration_resend: "resend-api-key",
  integration_notion: "notion-api-key",
  integration_calcom: "calcom-api-key",
};

const clearTimers = new Map<string, number>();
let telegramRefreshPending = false;
let telegramUnreadCount: number | null = null;
let telegramIsland: Island | null = null;

export function registerIntegrationHandlers(island: Island) {
  telegramIsland = island;
  void onEvent<IntegrationUpdate>("integration", (update) => handle(island, update));
  void refreshConfigured();
  window.setInterval(() => void refreshTelegramSummary(), 2_000);
}

/** Asks Rust which keys exist so the idle cards can say so. */
export async function refreshConfigured() {
  void refreshTelegramSummary();
  for (const [id, key] of Object.entries(KEY_FOR)) {
    const present = (await Bridge.secretPresent(key)) ?? false;
    const info = State.integrations[id] ?? { data: {}, error: null, loaded: false, configured: false };
    State.integrations[id] = { ...info, configured: present };
  }
  State.notify();
}

/** Poll only the cached TDLib summary, including while the chat window is closed. */
export async function refreshTelegramSummary() {
  const id = "integration_telegram";
  if (!State.settings.activeIntegrations.includes(id)) {
    telegramUnreadCount = null;
    if (telegramIsland) processTelegramNotifications(telegramIsland, []);
    return;
  }
  if (telegramRefreshPending) return;
  telegramRefreshPending = true;
  try {
    const summary = await Bridge.telegramSummary();
    if (!State.settings.activeIntegrations.includes(id)) {
      telegramUnreadCount = null;
      if (telegramIsland) processTelegramNotifications(telegramIsland, []);
      return;
    }
    const previous = State.integrations[id];
    const info = {
      data: { ...summary },
      error: summary.error ?? null,
      loaded: true,
      configured: summary.configured,
    };
    State.integrations[id] = info;
    const task = State.tasks.find((t) => t.id === id);
    const count = summary.state === "ready" && !summary.paused ? summary.unreadCount : null;
    if (task) {
      task.state = summary.error ? "error" : "idle";
      if (summary.error) {
        if (State.focusId !== id) task.pillBadge = "error";
      } else if (count === null || count === 0 || task.pillBadge === "error") {
        task.pillBadge = null;
      } else if (telegramUnreadCount !== null && count > telegramUnreadCount && State.focusId !== id) {
        // Totals can also change during sync. Only TDLib notification events
        // below may open a message preview.
        task.pillBadge = "finished";
      }
    }
    telegramUnreadCount = count;
    if (telegramIsland) processTelegramNotifications(telegramIsland, summary.notifications ?? []);
    if (JSON.stringify(previous) !== JSON.stringify(info)) State.notify();
  } catch {
    const previous = State.integrations[id];
    State.integrations[id] = {
      data: {},
      error: "Telegram status is unavailable. Open Telegram to reconnect.",
      loaded: false,
      configured: previous?.configured ?? false,
    };
    telegramUnreadCount = null;
    if (telegramIsland) processTelegramNotifications(telegramIsland, []);
    const task = State.tasks.find((t) => t.id === id);
    if (task) {
      task.state = "error";
      if (State.focusId !== id) task.pillBadge = "error";
    }
    State.notify();
  } finally {
    telegramRefreshPending = false;
  }
}

function handle(island: Island, update: IntegrationUpdate) {
  if (State.paused) return;

  const previous = State.integrations[update.id];
  State.integrations[update.id] = {
    data: update.error ? (previous?.data ?? {}) : update.data,
    error: update.error,
    loaded: update.error ? (previous?.loaded ?? false) : true,
    configured: previous?.configured ?? true,
  };

  const event = update.event;
  if (event) {
    const task = State.tasks.find((t) => t.id === update.id);
    if (task) {
      task.state = event.success ? "finished" : "error";
      task.steps = event.detail ? [event.label, event.detail] : [event.label];
      task.stepIndex = task.steps.length - 1;
      if (State.focusId !== update.id) {
        task.pillBadge = event.success ? "finished" : "error";
      }
      Sound.play(event.success ? "finish" : "error");
      // Show the compact island so the badge is seen,
      // but never steal the screen for a successful deploy.
      island.reveal();

      const existing = clearTimers.get(update.id);
      if (existing != null) window.clearTimeout(existing);
      clearTimers.set(
        update.id,
        window.setTimeout(() => {
          clearTimers.delete(update.id);
          const t = State.tasks.find((x) => x.id === update.id);
          if (!t || (t.state !== "finished" && t.state !== "error")) return;
          t.state = "idle";
          t.steps = [];
          t.stepIndex = 0;
          t.pillBadge = null;
          State.notify();
        }, 60_000),
      );
    }
  }

  State.notify();
}

import { State } from "../core/state";
import type { Island } from "../island/island";
import type { TelegramNotification } from "./api";
import { InlineTelegram, inlineTelegramReady, refreshInlineHistory } from "./inline";

export const TelegramNotifications = {
  current: null as TelegramNotification | null,
};

const seen = new Set<string>();
type NotificationIsland = Pick<Island, "showTelegramNotification" | "setView">;

/** TDLib supplies fresh notification events; unread totals are not notifications. */
export function processTelegramNotifications(island: NotificationIsland, notifications: readonly TelegramNotification[]) {
  let latest: TelegramNotification | undefined;
  for (const notification of notifications) {
    if (seen.has(notification.id)) continue;
    seen.add(notification.id);
    if (!notification.message.outgoing) latest = notification;
  }
  // The backend retains at most eight notifications for a minute.
  while (seen.size > 128) seen.delete(seen.values().next().value!);

  if (!inlineTelegramReady()) latest = undefined;
  if (!inlineTelegramReady() || !notifications.some((item) => item.id === TelegramNotifications.current?.id)) {
    TelegramNotifications.current = null;
  }
  if (latest && inlineTelegramReady()) {
    TelegramNotifications.current = latest;
    const task = State.tasks.find((item) => item.id === "integration_telegram");
    if (task) task.pillBadge = "finished";
    if (State.mode === "expanded" && State.view === "telegram" && InlineTelegram.selected?.id === latest.chat.id) {
      void refreshInlineHistory();
    }
    // Keep typed drafts, approval prompts and file operations on screen. Their
    // header offers the same preview without changing the active conversation.
    const safeView = ["overview", "empty", "greeting", "telegram-notification"].includes(State.view);
    if (!State.isPinned && !State.pendingApproval && !State.fileDragOver && !InlineTelegram.sending &&
        (State.mode !== "expanded" || safeView)) {
      island.showTelegramNotification();
    }
  }
  if (!TelegramNotifications.current && State.mode === "expanded" && State.view === "telegram-notification") {
    island.setView("overview");
  }
  State.notify();
}

export function dismissTelegramNotification() {
  TelegramNotifications.current = null;
  State.notify();
}

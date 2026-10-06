import { State } from "../core/state";
import type { Island } from "../island/island";
import type { TelegramNotification } from "./api";
import { InlineTelegram, inlineTelegramReady, refreshInlineHistory } from "./inline";

export const TelegramNotifications = {
  current: null as TelegramNotification | null,
  pending: [] as TelegramNotification[],
};

const seen = new Set<string>();
type NotificationIsland = Pick<Island, "showTelegramNotification" | "setView">;

/** TDLib supplies fresh notification events; unread totals are not notifications. */
export function processTelegramNotifications(island: NotificationIsland, notifications: readonly TelegramNotification[]) {
  const previous = TelegramNotifications.current;
  const previousPending = JSON.stringify(TelegramNotifications.pending);
  const ready = inlineTelegramReady();
  let changed = false;
  const fresh: TelegramNotification[] = [];
  for (const notification of notifications) {
    if (seen.has(notification.id)) continue;
    seen.add(notification.id);
    if (!notification.message.outgoing) fresh.push(notification);
  }
  // The backend retains at most eight notifications for a minute.
  while (seen.size > 128) seen.delete(seen.values().next().value!);

  const pendingIds = new Set(TelegramNotifications.pending.map((item) => item.id));
  for (const item of fresh) pendingIds.add(item.id);
  const pending = ready ? notifications.filter((item) => pendingIds.has(item.id) && !item.message.outgoing).slice(-8) : [];
  // Keep the selected preview stable while it is being read. Every message in a
  // burst stays available, including edits supplied under an existing ID.
  const latest = ready ? fresh.at(-1) : undefined;
  const reading = State.mode === "expanded" && State.view === "telegram-notification";
  const selected = pending.find((item) => item.id === previous?.id);
  const next = reading && selected ? selected : latest ?? selected ?? pending.at(-1) ?? null;
  TelegramNotifications.pending = pending;
  TelegramNotifications.current = previous && next && JSON.stringify(previous) === JSON.stringify(next) ? previous : next;
  if (latest && ready) {
    const task = State.tasks.find((item) => item.id === "integration_telegram");
    if (task && task.pillBadge !== "finished") {
      task.pillBadge = "finished";
      changed = true;
    }
    if (State.mode === "expanded" && State.view === "telegram" && fresh.some((item) => InlineTelegram.selected?.id === item.chat.id)) {
      void refreshInlineHistory();
    }
    // Keep typed drafts, approval prompts and file operations on screen. Their
    // header offers the same preview without changing the active conversation.
    const safeView = ["overview", "empty", "greeting", "telegram-notification"].includes(State.view);
    if (!(reading && selected) && !State.isPinned && !State.fileDragOver && !InlineTelegram.sending &&
        (State.mode !== "expanded" || safeView)) {
      island.showTelegramNotification();
    }
  }
  if (!TelegramNotifications.current && State.mode === "expanded" && State.view === "telegram-notification") {
    island.setView("overview");
    changed = true;
  }
  if (changed || previous !== TelegramNotifications.current || previousPending !== JSON.stringify(pending)) State.notify();
}

export function dismissTelegramNotification() {
  const current = TelegramNotifications.current;
  if (!current) return;
  const index = TelegramNotifications.pending.findIndex((item) => item.id === current.id);
  TelegramNotifications.pending = TelegramNotifications.pending.filter((item) => item.id !== current.id);
  TelegramNotifications.current = TelegramNotifications.pending[Math.min(index, TelegramNotifications.pending.length - 1)] ?? null;
  State.notify();
}

export function selectTelegramNotification(offset: number) {
  const index = TelegramNotifications.pending.findIndex((item) => item.id === TelegramNotifications.current?.id);
  const next = TelegramNotifications.pending[index + offset];
  if (!next || next === TelegramNotifications.current) return;
  TelegramNotifications.current = next;
  State.notify();
}

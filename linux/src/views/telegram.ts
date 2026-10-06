import "../telegram/inline.css";
import { State } from "../core/state";
import { Bridge } from "../core/bridge";
import { telegramStatusLabel, type TelegramChat, type TelegramNotification } from "../telegram/api";
import {
  InlineTelegram, inlineTelegramReady, refreshInlineChats, refreshInlineHistory,
  selectInlineChat, sendInlineMessage, startInlineTelegram,
} from "../telegram/inline";
import { h, svg, dot } from "./dom";
import { ICONS } from "./icons";
import type { ViewActions, ViewHost } from "./views";
import { TelegramNotifications, dismissTelegramNotification, selectTelegramNotification } from "../telegram/notifications";

export function buildTelegramNotification(actions: ViewActions): ViewHost {
  let displayedNotification: TelegramNotification | null = null;
  const title = h("b", { class: "tg-alert-title" });
  const sender = h("span", { class: "tg-alert-sender" });
  const time = h("time", { class: "tg-alert-time" });
  const count = h("span", { class: "tg-alert-count", role: "status" });
  const previous = h("button", { class: "tg-alert-nav", title: "Previous message", "aria-label": "Previous Telegram message", onclick: () => selectTelegramNotification(-1) }, svg(ICONS.chevronLeft, 12, { stroke: 2 }));
  const next = h("button", { class: "tg-alert-nav", title: "Next message", "aria-label": "Next Telegram message", onclick: () => selectTelegramNotification(1) }, svg(ICONS.chevronRight, 12, { stroke: 2 }));
  const text = h("div", { class: "tg-alert-text", tabindex: "0", role: "region", "aria-label": "New Telegram message text", "aria-live": "polite" });
  const reply = h("button", { class: "btn primary", text: "Reply", onclick: () => {
    const notification = displayedNotification;
    if (!notification || !inlineTelegramReady()) return;
    selectInlineChat(notification.chat);
    dismissTelegramNotification();
    actions.setFocus("integration_telegram");
    actions.setView("telegram");
  } });
  const dismiss = h("button", { class: "btn secondary", text: "Dismiss", onclick: () => {
    dismissTelegramNotification();
    if (!TelegramNotifications.current) actions.collapse();
  } });
  const body = h("div", { class: "tg-alert-body" },
    h("div", { class: "tg-alert-heading" }, dot("#59ACD8", 6), h("span", { text: "Telegram" }), count, previous, next),
    title, h("div", { class: "tg-alert-meta" }, sender, time), text, h("div", { class: "actions" }, reply, dismiss),
  );
  const el = h("div", { class: "view tg-notification" }, h("div", { class: "card tg-card" }, body));
  let shown = "";
  return {
    el,
    sync() {
      const notification = TelegramNotifications.current;
      displayedNotification = notification;
      title.textContent = notification?.chat.title ?? "";
      title.title = title.textContent;
      sender.textContent = notification?.message.senderName ?? "";
      sender.hidden = !sender.textContent || sender.textContent === title.textContent;
      const date = new Date((notification?.message.date ?? 0) * 1000);
      const validDate = Number.isFinite(date.getTime()) && date.getTime() > 0;
      time.textContent = validDate ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : "";
      if (validDate) { time.dateTime = date.toISOString(); time.title = date.toLocaleString(); }
      const total = TelegramNotifications.pending.length;
      const index = TelegramNotifications.pending.findIndex((item) => item.id === notification?.id);
      count.textContent = total > 1 ? `${index + 1} of ${total}` : "New message";
      previous.hidden = next.hidden = total < 2;
      previous.disabled = index <= 0;
      next.disabled = index < 0 || index >= total - 1;
      text.textContent = notification?.message.text || "New message";
      reply.disabled = !notification || !inlineTelegramReady();
      if (notification && shown !== notification.id) {
        shown = notification.id;
        text.scrollTop = 0;
        body.animate?.([{ opacity: 0, transform: "translateY(5px)" }, { opacity: 1, transform: "translateY(0)" }], {
          duration: matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 200, easing: "ease-out",
        });
      }
    },
  };
}

export function telegramPreview(chat: TelegramChat, onClick: () => void): HTMLButtonElement {
  const button = h("button", {
    class: "tg-preview", type: "button", onclick: onClick,
    "aria-label": `Open chat with ${chat.title}`, title: `${chat.title}: ${chat.lastMessage || "No messages yet"}`,
  },
  h("span", { class: "tg-preview-copy" },
    h("b", { text: chat.title }), h("span", { text: chat.lastMessage || "No messages yet" })),
  );
  if (chat.unreadCount > 0) button.append(h("span", {
    class: "tg-unread", text: chat.unreadCount > 99 ? "99+" : String(chat.unreadCount),
    "aria-label": `${chat.unreadCount} unread messages`,
  }));
  else button.append(h("span", { class: "tg-preview-arrow" }, svg(ICONS.chevronRight, 10, { stroke: 2 })));
  return button;
}

export function buildTelegram(actions: ViewActions): ViewHost {
  startInlineTelegram();
  const back = h("button", { class: "tg-back", title: "Back to Home", "aria-label": "Back to Home", onclick: () => actions.setView("overview") }, svg(ICONS.chevronLeft, 13, { stroke: 2 }));
  const title = h("b", { text: "Telegram" });
  const connection = h("span", { class: "tg-connection" });
  const refresh = h("button", { class: "tg-refresh", text: "Refresh", onclick: () => {
    void refreshInlineChats();
    void refreshInlineHistory();
  } });
  const chatList = h("nav", { class: "tg-chat-list", "aria-label": "Recent Telegram chats" });
  const sidebar = h("aside", { class: "tg-sidebar" }, h("span", { class: "tg-sidebar-label", text: "Recent chats" }), chatList);
  const history = h("div", { class: "tg-history", role: "region", "aria-label": "Telegram messages", tabindex: "0" });
  const historyStatus = h("div", { class: "tg-history-status", role: "status" });
  const composer = h("textarea", {
    class: "tg-input", rows: "1", maxlength: "8192", placeholder: "Write a reply…",
    "aria-label": "Reply to selected Telegram chat",
  });
  const send = h("button", { class: "tg-send", type: "submit", text: "Send" });
  const notice = h("span", { class: "tg-send-notice", role: "status", "aria-live": "polite" });
  const form = h("form", { class: "tg-composer" }, h("div", { class: "tg-compose-row" }, composer, send), notice);
  const conversation = h("section", { class: "tg-conversation" }, historyStatus, history, form);
  const el = h("div", { class: "view tg-view" }, h("div", { class: "card tg-card" },
    h("header", { class: "tg-head" }, back, dot("#59ACD8", 6), title, connection, refresh),
    h("div", { class: "tg-workspace" }, sidebar, conversation),
  ));
  let lastChat = "";
  let chatsKey = "";
  let messagesKey = "";

  function updateComposer() {
    const selected = InlineTelegram.selected;
    const connected = inlineTelegramReady();
    const length = [...composer.value.trim()].length;
    composer.disabled = !connected || !selected || InlineTelegram.sending;
    send.disabled = composer.disabled || length === 0 || length > 4096;
    send.textContent = InlineTelegram.sending ? "Sending…" : "Send";
    send.title = selected ? `Send to ${selected.title}` : "Choose a chat";
    notice.textContent = length > 4096 ? "Maximum 4096 characters" : InlineTelegram.sendNotice || "Text reply · click Send";
  }

  composer.addEventListener("input", () => {
    if (InlineTelegram.selected) InlineTelegram.drafts.set(InlineTelegram.selected.id, composer.value);
    InlineTelegram.sendNotice = "";
    updateComposer();
  });
  composer.addEventListener("pointerdown", () => void Bridge.focusWindow(true));
  form.addEventListener("submit", (event) => { event.preventDefault(); void sendInlineMessage(); });

  return {
    el,
    focus() { if (!composer.disabled) composer.focus({ preventScroll: true }); },
    sync() {
      const selected = InlineTelegram.selected;
      const connected = inlineTelegramReady();
      const data = State.integrations.integration_telegram?.data;
      title.textContent = selected?.title ?? "Telegram";
      title.title = title.textContent;
      connection.textContent = connected ? "Telegram" : telegramStatusLabel(String(data?.paused ? "paused" : data?.state ?? "disconnected"));
      refresh.disabled = !connected || InlineTelegram.sending;
      if (lastChat !== (selected?.id ?? "")) {
        lastChat = selected?.id ?? "";
        messagesKey = "";
        composer.value = selected ? InlineTelegram.drafts.get(selected.id) ?? "" : "";
        history.scrollTop = 0;
        conversation.animate?.([{ opacity: 0, transform: "translateY(6px)" }, { opacity: 1, transform: "translateY(0)" }], {
          duration: matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 220, easing: "ease-out",
        });
      } else if (selected && !InlineTelegram.drafts.has(selected.id)) {
        composer.value = "";
      }

      const key = JSON.stringify([InlineTelegram.chats, selected?.id, InlineTelegram.loaded, InlineTelegram.listError]);
      if (key !== chatsKey) {
        chatsKey = key;
        chatList.replaceChildren();
        for (const chat of InlineTelegram.chats) {
          const button = telegramPreview(chat, () => {
            selectInlineChat(chat);
            actions.blip();
            requestAnimationFrame(() => {
              if (State.mode === "expanded" && State.view === "telegram") composer.focus({ preventScroll: true });
            });
          });
          button.classList.toggle("selected", chat.id === selected?.id);
          button.setAttribute("aria-current", chat.id === selected?.id ? "true" : "false");
          chatList.append(button);
        }
        if (!InlineTelegram.chats.length) chatList.append(h("p", { class: "tg-empty", text: InlineTelegram.listError || (InlineTelegram.loaded ? "No recent chats" : connected ? "Loading chats…" : "Not connected") }));
        else if (InlineTelegram.listError) chatList.append(h("p", { class: "tg-empty", text: "Could not refresh chats" }));
      }
      historyStatus.textContent = InlineTelegram.historyError;
      historyStatus.hidden = !InlineTelegram.historyError;
      const nextKey = JSON.stringify([selected?.id, InlineTelegram.messages, InlineTelegram.loadingHistory, connected]);
      if (nextKey !== messagesKey) {
        const firstRender = messagesKey === "";
        const atBottom = history.scrollHeight - history.scrollTop - history.clientHeight < 40;
        const scrollTop = history.scrollTop;
        messagesKey = nextKey;
        history.replaceChildren();
        for (const message of InlineTelegram.messages) {
          const bubble = h("article", { class: `tg-message${message.outgoing ? " outgoing" : ""}` });
          if (!message.outgoing && message.senderName) bubble.append(h("b", { class: "tg-sender", text: message.senderName }));
          bubble.append(h("div", { class: "tg-message-text", text: message.text || "[Message without text]" }));
          const date = new Date(message.date * 1000);
          if (Number.isFinite(date.getTime())) bubble.append(h("time", { datetime: date.toISOString(), text: date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) }));
          history.append(bubble);
        }
        if (!InlineTelegram.messages.length) history.append(h("p", {
          class: "tg-empty", text: !connected ? "Connect Telegram in Settings to read and reply." : !selected ? "Choose a chat to read and reply." : InlineTelegram.loadingHistory ? "Loading messages…" : "No messages yet",
        }));
        if (firstRender || atBottom) history.scrollTop = history.scrollHeight;
        else history.scrollTop = scrollTop;
      }
      updateComposer();
    },
  };
}

import { State } from "../core/state";
import { Telegram, type TelegramChat, type TelegramMessage } from "./api";

/** Shared by the Home previews and the conversation inside the island. */
export const InlineTelegram = {
  chats: [] as TelegramChat[],
  loaded: false,
  listError: "",
  selected: null as TelegramChat | null,
  messages: [] as TelegramMessage[],
  loadingHistory: false,
  historyError: "",
  sending: false,
  sendNotice: "",
  drafts: new Map<string, string>(),
};

let started = false;
let visible = false;
let ready = false;
let accountName = "";
let historyTarget = "";
let session = 0;
let selection = 0;
let historyRequest = 0;
let listPending = false;
let pendingHistory = 0;
let timer: number | undefined;

export function inlineTelegramReady(): boolean {
  const info = State.integrations.integration_telegram;
  return State.settings.activeIntegrations.includes("integration_telegram") && !State.paused &&
    info?.data.state === "ready" && !info.data.paused && !info.error;
}

function errorText(error: unknown): string {
  return String(error).replace(/^Error:\s*/, "");
}

function clearSession(preserveDrafts = false) {
  session++;
  selection++;
  historyRequest++;
  InlineTelegram.chats = [];
  InlineTelegram.loaded = false;
  InlineTelegram.selected = null;
  InlineTelegram.messages = [];
  if (!preserveDrafts) InlineTelegram.drafts.clear();
  InlineTelegram.listError = InlineTelegram.historyError = InlineTelegram.sendNotice = "";
  InlineTelegram.loadingHistory = InlineTelegram.sending = false;
}

/** Content polling stops as soon as the island or Telegram view is hidden. */
export function startInlineTelegram() {
  if (started) return;
  started = true;
  State.subscribe(syncVisibility);
  syncVisibility();
}

function syncVisibility() {
  const info = State.integrations.integration_telegram;
  const nextReady = inlineTelegramReady();
  if (ready && !nextReady) clearSession(State.paused || !!info?.data.paused || !info?.loaded);
  if (!State.settings.activeIntegrations.includes("integration_telegram") ||
      ["unconfigured", "loggingOut", "phone", "code", "password", "email", "emailCode", "registration"].includes(String(info?.data.state))) {
    InlineTelegram.drafts.clear();
  }
  const nextAccount = typeof info?.data.accountName === "string" ? info.data.accountName : "";
  if (nextReady && nextAccount) {
    if (accountName && accountName !== nextAccount) clearSession();
    accountName = nextAccount;
  }
  ready = nextReady;
  const nextVisible = ready && State.mode === "expanded" &&
    (State.view === "telegram" || (State.view === "overview" && State.focusTask?.id === "integration_telegram"));
  const nextTarget = nextVisible && State.view === "telegram" ? InlineTelegram.selected?.id ?? "" : "";
  if (nextVisible !== visible) {
    visible = nextVisible;
    window.clearTimeout(timer);
    timer = undefined;
    if (visible) {
      void refreshInlineChats();
      scheduleRefresh();
    }
  }
  if (nextTarget !== historyTarget) {
    historyTarget = nextTarget;
    historyRequest++;
    if (nextTarget) void refreshInlineHistory();
  }
}

function scheduleRefresh() {
  timer = window.setTimeout(() => {
    if (!visible) return;
    void refreshInlineChats();
    void refreshInlineHistory();
    scheduleRefresh();
  }, 5_000);
}

export async function refreshInlineChats() {
  if (!visible || listPending || InlineTelegram.sending) return;
  const version = session;
  listPending = true;
  try {
    const chats = await Telegram.chats();
    if (version !== session || !ready) return;
    InlineTelegram.chats = chats.slice(0, 2);
    InlineTelegram.loaded = true;
    InlineTelegram.listError = "";
    const selected = chats.find((chat) => chat.id === InlineTelegram.selected?.id);
    if (selected) InlineTelegram.selected = selected;
  } catch (error) {
    if (version === session) InlineTelegram.listError = errorText(error);
  } finally {
    listPending = false;
    State.notify();
  }
}

export function selectInlineChat(chat: TelegramChat) {
  if (InlineTelegram.selected?.id === chat.id) return;
  selection++;
  historyRequest++;
  InlineTelegram.selected = chat;
  InlineTelegram.messages = [];
  InlineTelegram.loadingHistory = true;
  InlineTelegram.historyError = InlineTelegram.sendNotice = "";
  State.notify();
}

export async function refreshInlineHistory() {
  const chat = InlineTelegram.selected;
  if (!visible || State.view !== "telegram" || !chat || InlineTelegram.sending || pendingHistory !== 0) return;
  const version = session;
  const request = ++historyRequest;
  pendingHistory = request;
  try {
    const messages = await Telegram.history(chat.id);
    if (version !== session || request !== historyRequest || chat.id !== InlineTelegram.selected?.id) return;
    InlineTelegram.messages = messages;
    InlineTelegram.historyError = "";
  } catch (error) {
    if (version === session && request === historyRequest) InlineTelegram.historyError = errorText(error);
  } finally {
    if (pendingHistory === request) pendingHistory = 0;
    if (version === session && request === historyRequest) InlineTelegram.loadingHistory = false;
    State.notify();
    // A chat switch invalidates the result, but queues only the latest selection.
    if (request !== historyRequest) void refreshInlineHistory();
  }
}

/** Only called by an explicit Send action; failed/ambiguous sends are never retried. */
export async function sendInlineMessage() {
  const chat = InlineTelegram.selected;
  if (!chat || !inlineTelegramReady() || InlineTelegram.sending) return;
  const draft = InlineTelegram.drafts.get(chat.id) ?? "";
  const text = draft.trim();
  if (!text || [...text].length > 4096) return;
  const version = session;
  const selectedVersion = selection;
  InlineTelegram.sending = true;
  InlineTelegram.sendNotice = `Sending to ${chat.title}…`;
  historyRequest++;
  State.notify();
  try {
    const sent = await Telegram.send(chat.id, text);
    if (version !== session) return;
    if (InlineTelegram.drafts.get(chat.id) === draft) InlineTelegram.drafts.delete(chat.id);
    if (selectedVersion === selection && chat.id === InlineTelegram.selected?.id) {
      InlineTelegram.messages = [...InlineTelegram.messages.filter((message) => message.id !== sent.id), sent].slice(-50);
      InlineTelegram.sendNotice = "Sent · confirmed by Telegram";
      InlineTelegram.loadingHistory = false;
    }
  } catch (error) {
    if (version === session && selectedVersion === selection) {
      InlineTelegram.sendNotice = `${errorText(error)} Check the chat before retrying.`;
    }
  } finally {
    if (version === session) {
      InlineTelegram.sending = false;
    }
    State.notify();
    if (version === session) void refreshInlineHistory();
  }
}

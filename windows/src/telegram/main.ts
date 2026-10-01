import "./telegram.css";
import { Bridge } from "../core/bridge";
import { h, clear } from "../views/dom";
import { Telegram, telegramStatusLabel, type TelegramChat, type TelegramMessage, type TelegramStatus } from "./api";

const root = document.getElementById("telegram-root")!;
const account = h("div", { class: "telegram-account", text: "Your personal chats" });
const statusDot = h("i", { class: "dot" });
const statusText = h("span", { text: "Checking connection…" });
const notice = h("div", { class: "telegram-notice", role: "status", "aria-live": "polite" });
const content = h("main", { class: "telegram-content" });
const refresh = h("button", { id: "telegram-refresh", text: "Refresh" });
const disconnect = h("button", { text: "Disconnect", hidden: true });
const logout = h("button", { class: "danger", text: "Log out", hidden: true });
let status: TelegramStatus | null = null;
let authView = "";
let editingCredentials = false;
let busy = false;
let sending = false;
let polling = false;
let disposed = false;
let timer: ReturnType<typeof setTimeout> | undefined;
let selected: TelegramChat | null = null;
let selectionVersion = 0;
let historyRequest = 0;
let statusRequest = 0;
let connectionVersion = 0;
let loadingOlder = false;
let hasOlder = true;
let chats: TelegramChat[] = [];
let messages: TelegramMessage[] = [];
let list: HTMLElement | null = null;
let search: HTMLInputElement | null = null;
let messageList: HTMLElement | null = null;
let recipient: HTMLElement | null = null;
let composer: HTMLTextAreaElement | null = null;
let sendButton: HTMLButtonElement | null = null;
let sendStatus: HTMLElement | null = null;
let olderButton: HTMLButtonElement | null = null;
let chatsFingerprint = "";
let messagesFingerprint = "";
const drafts = new Map<string, string>();

function report(error: unknown) {
  clear(notice);
  notice.append(h("div", { class: "notice err", text: String(error).replace(/^Error:\s*/, "") }));
}

function clearNotice() { clear(notice); }

function updateBusy() {
  refresh.disabled = busy || polling;
  disconnect.disabled = busy || sending;
  logout.disabled = busy || sending;
  for (const element of content.querySelectorAll<HTMLInputElement | HTMLButtonElement>(".telegram-auth input, .telegram-auth button")) {
    element.disabled = busy;
  }
  updateSendButton();
}

async function action(run: () => Promise<unknown>) {
  if (busy || disposed) return;
  busy = true;
  clearNotice();
  updateBusy();
  try {
    await run();
    await updateStatus();
  } catch (error) {
    report(error);
  } finally {
    busy = false;
    updateBusy();
  }
}

function authCard(title: string, description: string): HTMLElement {
  const card = h("section", {}, h("h2", { text: title }), h("p", { text: description }));
  clear(content);
  content.append(h("div", { class: "telegram-auth" }, card));
  return card;
}

function credentialsForm() {
  const card = authCard("Connect your Telegram account", "Read your existing chats and reply here. First enter the application credentials from your Telegram account.");
  const apiId = h("input", {
    id: "telegram-api-id", type: "text", inputmode: "numeric", pattern: "[0-9]+",
    required: true, placeholder: "12345678", autocomplete: "off", maxlength: "10",
  });
  const apiHash = h("input", {
    id: "telegram-api-hash", type: "password", required: true, placeholder: "Your API hash",
    autocomplete: "off", spellcheck: "false", minlength: "32", maxlength: "32",
  });
  const form = h("form", {},
    h("label", { for: apiId.id, text: "API ID" }, apiId),
    h("label", { for: apiHash.id, text: "API hash" }, apiHash),
    h("button", { type: "submit", class: "primary", text: "Save and continue", id: "telegram-save-credentials" }),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const id = Number(apiId.value.trim());
    const hash = apiHash.value.trim();
    if (!Number.isSafeInteger(id) || id <= 0 || id > 2147483647 || !/^[a-f0-9]{32}$/i.test(hash)) {
      report("Enter the API ID and 32-character API hash from my.telegram.org.");
      return;
    }
    void action(async () => {
      await Telegram.configure(id, hash);
      apiHash.value = "";
      apiId.value = "";
      editingCredentials = false;
      authView = "";
      await Telegram.connect();
    });
  });
  card.append(
    h("button", { class: "text-link", text: "Open my.telegram.org → API development tools", onclick: () => void Bridge.openUrl("https://my.telegram.org/apps") }),
    form,
    h("p", { class: "hint", text: "Credentials stay in your system keyring. Telegram messages are not automatically shared with Codex." }),
  );
  if (status?.configured) card.append(h("button", { text: "Back", onclick: () => {
    editingCredentials = false; authView = ""; if (status) drawAuth(status);
  } }));
}

function signInForm(kind: "phone" | "code" | "password" | "email" | "emailCode") {
  const details = {
    phone: { title: "Sign in to Telegram", description: "Enter the phone number for your existing Telegram account, including the country code.", label: "Phone number", type: "tel", placeholder: "+41 79 123 45 67", button: "Continue" },
    code: { title: "Enter your Telegram login code", description: "Check Telegram on a device where you are already signed in, or the delivery method Telegram offers for your account.", label: "Login code", type: "text", placeholder: "Login code", button: "Verify code" },
    password: { title: "Two-step verification", description: "Enter the two-step verification password for your Telegram account.", label: "Password", type: "password", placeholder: "Telegram password", button: "Sign in" },
    email: { title: "Telegram needs an email address", description: "Enter the email address to use for the login verification requested by Telegram.", label: "Email address", type: "email", placeholder: "you@example.com", button: "Continue" },
    emailCode: { title: "Check your email", description: "Enter the verification code sent by Telegram to your email address.", label: "Email code", type: "text", placeholder: "Verification code", button: "Verify code" },
  }[kind];
  const card = authCard(details.title, details.description);
  const input = h("input", {
    id: `telegram-auth-${kind}`, type: details.type, required: true,
    placeholder: details.placeholder, autocomplete: "off", spellcheck: "false", maxlength: "256",
  });
  const form = h("form", {},
    h("label", { for: input.id, text: details.label }, input),
    h("button", { type: "submit", class: "primary", text: details.button }),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (busy) return;
    const value = kind === "password" ? input.value : input.value.trim();
    if (!value) return;
    void action(async () => {
      try { await Telegram.authenticate(kind, value); }
      finally { if (kind !== "phone" && kind !== "email") input.value = ""; }
    });
  });
  card.append(form);
  input.focus();
}

function drawAuth(next: TelegramStatus) {
  const view = next.paused ? "paused" : !next.runtimeAvailable ? "missing_runtime" : editingCredentials || !next.configured ? "credentials" : next.state;
  if (authView === view) return;
  authView = view;
  selected = null;
  selectionVersion++;
  list = null;
  composer = null;
  messageList = null;
  sendButton = null;
  messages = [];
  chats = [];
  messagesFingerprint = "";
  chatsFingerprint = "";
  if (view === "paused") {
    authCard("Telegram is paused", "Resume Coucou from its tray menu to reconnect Telegram.");
  } else if (view === "credentials") {
    credentialsForm();
  } else if (["phone", "code", "password", "email", "emailCode"].includes(view)) {
    signInForm(view as "phone" | "code" | "password" | "email" | "emailCode");
  } else if (view === "missing_runtime") {
    authCard("Telegram runtime is missing", "Coucou needs Telegram’s TDLib library to connect. Run the Telegram installer included with Coucou, then restart Coucou.")
      .append(h("p", { class: "path", text: "./scripts/install-telegram-linux.sh" }));
  } else if (["disconnected", "closed"].includes(view)) {
    const card = authCard("Telegram is disconnected", "Connect to sign in or reopen your saved Telegram session.");
    card.append(
      h("button", { class: "primary", text: "Connect Telegram", id: "telegram-connect", onclick: () => void action(() => Telegram.connect()) }),
      h("button", { text: "Change application credentials", onclick: () => { editingCredentials = true; authView = "credentials"; credentialsForm(); } }),
    );
  } else if (view === "registration") {
    authCard("An existing Telegram account is required", "Create your account in the official Telegram app, then disconnect here and sign in again.");
  } else {
    authCard(telegramStatusLabel(next.state), next.message || "Wait for Telegram to update the connection status. You can disconnect and try again if it does not continue.");
  }
}

function drawChatList() {
  if (!list) return;
  const filter = search?.value.trim().toLocaleLowerCase() ?? "";
  clear(list);
  const visible = chats.filter((chat) => chat.title.toLocaleLowerCase().includes(filter));
  for (const chat of visible) {
    const button = h("button", {
      class: `telegram-chat${selected?.id === chat.id ? " active" : ""}`,
      "aria-pressed": String(selected?.id === chat.id), "data-chat-id": chat.id,
    },
    h("span", { class: "telegram-chat-copy" },
      h("span", { class: "telegram-chat-title", text: chat.title }),
      h("span", { class: "telegram-chat-preview", text: chat.lastMessage || "No messages yet" }),
    ));
    if (chat.unreadCount > 0) button.append(h("span", { class: "telegram-unread", text: String(chat.unreadCount), "aria-label": `${chat.unreadCount} unread` }));
    button.addEventListener("click", () => void selectChat(chat));
    list.append(button);
  }
  if (!visible.length) list.append(h("p", { class: "telegram-empty", text: filter ? "No chats match your search." : "Your recent chats will appear here. Use Refresh to try again." }));
}

function updateSendButton() {
  if (sendButton) {
    sendButton.disabled = busy || sending || !selected || !composer?.value.trim() || status?.state !== "ready" || status.paused;
    sendButton.textContent = sending ? "Sending…" : "Send";
  }
  if (composer) composer.disabled = !selected || status?.state !== "ready" || status.paused;
  if (olderButton) {
    olderButton.hidden = !selected;
    olderButton.disabled = loadingOlder || !messages.length || !hasOlder || messages.length >= 300 || !!status?.paused;
    olderButton.textContent = loadingOlder ? "Loading…" : messages.length >= 300 ? "Showing 300 messages" : hasOlder ? "Load older messages" : "Start of conversation";
  }
}

function compareMessages(a: TelegramMessage, b: TelegramMessage) {
  return a.date - b.date || (BigInt(a.id) < BigInt(b.id) ? -1 : BigInt(a.id) > BigInt(b.id) ? 1 : 0);
}

function mergeMessages(older: TelegramMessage[], newer: TelegramMessage[]) {
  const all = new Map(older.map((message) => [message.id, message]));
  for (const message of newer) all.set(message.id, message);
  return [...all.values()].sort(compareMessages).slice(-300);
}

function drawMessages(next: TelegramMessage[]) {
  if (!messageList) return;
  const fingerprint = JSON.stringify(next);
  if (fingerprint === messagesFingerprint) return;
  messagesFingerprint = fingerprint;
  messages = next;
  const nearBottom = messageList.scrollHeight - messageList.scrollTop - messageList.clientHeight < 80;
  const previousTop = messageList.scrollTop;
  clear(messageList);
  for (const message of next) {
    const time = new Date(message.date * 1000);
    const validDate = Number.isFinite(time.getTime());
    const bubble = h("article", { class: `telegram-message${message.outgoing ? " outgoing" : ""}` });
    if (!message.outgoing && message.senderName) bubble.append(h("div", { class: "telegram-message-sender", text: message.senderName }));
    bubble.append(h("div", { class: "telegram-message-text", text: message.text || "[Message without text]" }));
    if (validDate) bubble.append(h("time", { datetime: time.toISOString(), text: time.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }) }));
    messageList.append(bubble);
  }
  if (!next.length) messageList.append(h("p", { class: "telegram-empty", text: "No messages in this chat yet." }));
  if (nearBottom) messageList.scrollTop = messageList.scrollHeight;
  else messageList.scrollTop = previousTop;
  updateSendButton();
}

async function loadHistory(forceScroll = false) {
  const chat = selected;
  if (!chat) return;
  const version = selectionVersion;
  const request = ++historyRequest;
  const result = await Telegram.history(chat.id);
  if (disposed || selected?.id !== chat.id || selectionVersion !== version || request !== historyRequest) return;
  // Keep earlier pages, but replace the refreshed range so deleted messages
  // disappear and edited messages use Telegram's current text.
  const earlier = result.length ? messages.filter((message) => compareMessages(message, result[0]) < 0) : [];
  drawMessages(mergeMessages(earlier, result));
  if (forceScroll && messageList) messageList.scrollTop = messageList.scrollHeight;
}

async function loadOlder() {
  if (!selected || !messages.length || loadingOlder || !hasOlder) return;
  const chatId = selected.id;
  const version = selectionVersion;
  loadingOlder = true;
  updateSendButton();
  try {
    const result = await Telegram.history(chatId, messages[0].id);
    if (disposed || selected?.id !== chatId || selectionVersion !== version) return;
    const existingIds = new Set(messages.map((message) => message.id));
    hasOlder = result.some((message) => !existingIds.has(message.id));
    const previousHeight = messageList?.scrollHeight ?? 0;
    const previousTop = messageList?.scrollTop ?? 0;
    drawMessages(mergeMessages(result, messages));
    if (messageList) messageList.scrollTop = previousTop + messageList.scrollHeight - previousHeight;
  } catch (error) { report(error); }
  finally { loadingOlder = false; updateSendButton(); }
}

async function selectChat(chat: TelegramChat) {
  if (selected?.id === chat.id) return;
  if (selected && composer) drafts.set(selected.id, composer.value);
  selected = chat;
  selectionVersion++;
  messages = [];
  hasOlder = true;
  messagesFingerprint = "";
  if (recipient) {
    clear(recipient);
    recipient.append(h("h2", { text: chat.title }), h("p", { text: "Messages are sent to this chat from your Telegram account." }));
  }
  if (messageList) { clear(messageList); messageList.append(h("p", { class: "telegram-empty", text: "Loading messages…" })); }
  if (composer) composer.value = drafts.get(chat.id) ?? "";
  if (sendStatus) sendStatus.textContent = "";
  updateSendButton();
  drawChatList();
  try { await loadHistory(true); } catch (error) { report(error); }
}

async function sendMessage(event: SubmitEvent) {
  event.preventDefault();
  if (!selected || !composer || sending || busy || status?.state !== "ready" || status.paused) return;
  const text = composer.value.trim();
  if (!text || [...text].length > 4096) { report("Messages must contain between 1 and 4096 characters."); return; }
  const chat = selected;
  const draft = composer.value;
  sending = true;
  clearNotice();
  updateBusy();
  if (sendStatus) sendStatus.textContent = `Sending to ${chat.title}…`;
  try {
    const sent = await Telegram.send(chat.id, text);
    historyRequest++;
    if (drafts.get(chat.id) === draft) drafts.delete(chat.id);
    if (selected?.id === chat.id && composer) {
      if (composer.value === draft) composer.value = "";
      drawMessages([...messages.filter((message) => message.id !== sent.id), sent]);
      if (messageList) messageList.scrollTop = messageList.scrollHeight;
      if (sendStatus) sendStatus.textContent = "Sent — confirmed by Telegram.";
    }
    try { await loadHistory(); } catch { /* The confirmed send remains visible; refresh can retry history. */ }
  } catch (error) {
    report(error);
    if (selected?.id === chat.id && sendStatus) sendStatus.textContent = "No confirmation received. Check the chat before trying again.";
  } finally {
    sending = false;
    updateBusy();
  }
}

function drawWorkspace() {
  if (authView === "ready") return;
  authView = "ready";
  clear(content);
  search = h("input", { class: "telegram-search", type: "text", placeholder: "Search recent chats", "aria-label": "Search chats" });
  search.addEventListener("input", drawChatList);
  list = h("div", { class: "telegram-chats", "aria-label": "Telegram chats" });
  recipient = h("div", { class: "telegram-recipient" }, h("h2", { text: "Choose a chat" }));
  olderButton = h("button", { id: "telegram-older", text: "Load older messages", hidden: true });
  olderButton.addEventListener("click", () => void loadOlder());
  messageList = h("div", { class: "telegram-messages", "aria-label": "Chat messages" }, h("p", { class: "telegram-empty", text: "Select a chat to read and reply." }));
  composer = h("textarea", { id: "telegram-message", placeholder: "Write a message…", "aria-label": "Message to selected Telegram chat", maxlength: "8192", disabled: true });
  composer.addEventListener("input", () => {
    if (selected && composer) drafts.set(selected.id, composer.value);
    if (drafts.size > 100) drafts.delete(drafts.keys().next().value!);
    updateSendButton();
  });
  sendButton = h("button", { type: "submit", class: "primary", id: "telegram-send", text: "Send", disabled: true });
  sendStatus = h("div", { class: "telegram-send-status", role: "status", "aria-live": "polite" });
  const form = h("form", { class: "telegram-composer" }, composer,
    h("div", { class: "telegram-composer-actions" }, h("span", { class: "hint", text: "Text only · Sent when you click Send" }), sendButton), sendStatus,
  );
  form.addEventListener("submit", (event) => void sendMessage(event));
  content.append(h("div", { class: "telegram-workspace" },
    h("aside", { class: "telegram-sidebar" }, search, h("div", { class: "telegram-list-info", text: "Up to 100 recent chats · use Refresh to update" }), list),
    h("div", { class: "telegram-conversation" }, recipient, h("div", { class: "telegram-history-actions" }, olderButton), messageList, form),
  ));
  drawChatList();
}

async function updateStatus() {
  const request = ++statusRequest;
  const next = await Telegram.status();
  if (disposed || request !== statusRequest) return;
  if (status?.state === "ready" && (next.state !== "ready" || next.accountName !== status.accountName)) connectionVersion++;
  if (status?.error && !next.error) clearNotice();
  status = next;
  statusText.textContent = next.paused ? "Paused" : telegramStatusLabel(next.state);
  statusDot.style.background = next.state === "ready" && !next.paused ? "#22c55e" : "#f5a524";
  account.textContent = next.accountName || "Your personal chats";
  const connected = !["disconnected", "closed"].includes(next.state);
  disconnect.hidden = !connected;
  logout.hidden = next.state !== "ready";
  if (next.error) report(next.error);
  if (next.state === "ready") drawWorkspace();
  else drawAuth(next);
  updateBusy();
}

async function poll(force = false) {
  if (polling || disposed || busy) return;
  polling = true;
  updateBusy();
  try {
    await updateStatus();
    if (status?.state === "ready" && !status.paused && (!document.hidden || force)) {
      const generation = connectionVersion;
      const result = await Telegram.chats();
      if (disposed || status?.state !== "ready" || generation !== connectionVersion) return;
      const fingerprint = JSON.stringify(result);
      if (fingerprint !== chatsFingerprint) {
        chatsFingerprint = fingerprint;
        chats = result;
        drawChatList();
      }
      if (!sending) await loadHistory();
    }
  } catch (error) {
    report(error);
  } finally {
    polling = false;
    updateBusy();
  }
}

function schedule() {
  timer = setTimeout(async () => {
    await poll();
    if (!disposed) schedule();
  }, status?.state === "ready" || document.hidden ? 5000 : 1500);
}

refresh.addEventListener("click", () => { clearNotice(); void poll(true); });
disconnect.addEventListener("click", () => void action(() => Telegram.disconnect()));
logout.addEventListener("click", () => {
  clear(notice);
  notice.append(h("div", { class: "notice warn telegram-logout" },
    h("span", { text: "Log out of this Telegram session on this laptop?" }),
    h("button", { class: "danger", text: "Log out", onclick: () => void action(async () => {
      await Telegram.logout();
      drafts.clear();
      chats = [];
      messages = [];
      selected = null;
      selectionVersion++;
      connectionVersion++;
    }) }),
    h("button", { text: "Cancel", onclick: clearNotice }),
  ));
});
window.addEventListener("beforeunload", () => { disposed = true; if (timer) clearTimeout(timer); });
document.addEventListener("visibilitychange", () => { if (!document.hidden) void poll(); });

clear(root);
root.append(h("header", { class: "telegram-header" },
  h("div", { class: "telegram-brand" }, h("span", { class: "telegram-badge", "aria-hidden": "true", text: "➤" }),
    h("div", {}, h("h1", { text: "Telegram" }), account)),
  h("div", { class: "telegram-status", role: "status" }, statusDot, statusText),
  h("div", { class: "telegram-actions" }, refresh, disconnect, logout),
), notice, content);
void poll().finally(schedule);

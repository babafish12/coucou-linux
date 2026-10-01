// Development-only fixture: exercise the real island without sending messages.
import "../src/style.css";
import { State } from "../src/core/state";
import { Bridge } from "../src/core/bridge";
import { Sound } from "../src/core/sound";
import { Island } from "../src/island/island";
import { Telegram, type TelegramChat, type TelegramMessage, type TelegramNotification } from "../src/telegram/api";
import { processTelegramNotifications } from "../src/telegram/notifications";

const style = document.createElement("style");
style.textContent = `
  body { background: #252932; }
  #preview-controls { position: fixed; top: 355px; left: 24px; right: 24px; z-index: 20; color: #d8dde5; font: 13px/1.7 system-ui; }
  #preview-controls p { color: #a4afbf; }
  #preview-controls button { background: #394556; border: 1px solid #607084; border-radius: 6px; color: white; padding: 5px 10px; margin: 10px 8px 10px 0; }
  #preview-controls label { white-space: nowrap; }
  #preview-status { display: block; }
`;
document.head.append(style);
const status = document.querySelector<HTMLOutputElement>("#preview-status")!;
const failure = document.querySelector<HTMLInputElement>("#preview-fail")!;
let chatReads = 0;
let historyReads = 0;
let sends = 0;
let windowsOpened = 0;
let focusRequests = 0;
let notification: TelegramNotification | undefined;
let sequence = 10;
const chats: TelegramChat[] = [
  { id: "101", title: "Mara", lastMessage: "Treffen wir uns um 18:30 am See?", unreadCount: 2 },
  { id: "202", title: "Design Team", lastMessage: "Jonas: Die neue Animation ist bereit ✨", unreadCount: 1 },
  { id: "303", title: "Third chat — should not appear", lastMessage: "Older chat", unreadCount: 0 },
];
const now = Math.floor(Date.now() / 1000);
const messages = new Map<string, TelegramMessage[]>([
  ["101", [
    { id: "1", text: "Hey! Hast du heute Abend Zeit?", date: now - 240, outgoing: false, senderName: "Mara" },
    { id: "2", text: "Ja, sehr gerne 😊", date: now - 120, outgoing: true },
    { id: "3", text: chats[0].lastMessage, date: now - 60, outgoing: false, senderName: "Mara" },
  ]],
  ["202", [
    { id: "4", text: "Die neue Animation ist bereit ✨", date: now - 300, outgoing: false, senderName: "Jonas" },
    { id: "5", text: "Auch lange Nachrichten bleiben im Verlauf lesbar. <b>HTML bleibt Text</b> und öffnet keine Inhalte.", date: now - 100, outgoing: false, senderName: "Lena" },
  ]],
]);
const delay = () => new Promise<void>((resolve) => window.setTimeout(resolve, 250));
function report() { status.textContent = `Chat reads: ${chatReads} · History reads: ${historyReads} · Fixture sends: ${sends} · Extra windows: ${windowsOpened} · Focus requests: ${focusRequests}`; }
Telegram.chats = async () => { chatReads++; report(); await delay(); return chats.map((chat) => ({ ...chat })); };
Telegram.history = async (id) => { historyReads++; report(); await delay(); return [...messages.get(id) ?? []]; };
Telegram.send = async (id, text) => {
  sends++; report(); await delay();
  if (failure.checked) throw new Error("Fixture send failed.");
  const message = { id: String(++sequence), text, date: Math.floor(Date.now() / 1000), outgoing: true };
  messages.get(id)?.push(message);
  const chat = chats.find((chat) => chat.id === id);
  if (chat) chat.lastMessage = text;
  return message;
};
Bridge.openTelegramWindow = async () => { windowsOpened++; report(); };
Bridge.focusWindow = async (focused) => { if (focused) focusRequests++; report(); return null; };
State.settings.activeIntegrations = ["integration_telegram", "integration_github"];
State.settings.soundEnabled = false;
State.settings.autoCloseInterval = 15;
Sound.setEnabled(false);
const summary = { state: "ready", paused: false, accountName: "Fixture account", unreadCount: 3, unreadChatCount: 2 };
State.integrations.integration_telegram = { configured: true, loaded: true, error: null, data: summary };
State.loadIntegrationTasks();
State.setFocus("integration_telegram");
const island = new Island(document.querySelector<HTMLElement>("#root")!);
island.applySettings();
island.alert("overview");
document.querySelector("#preview-home")!.addEventListener("click", () => island.alert("overview"));
document.querySelector("#preview-collapse")!.addEventListener("click", () => island.collapse());
document.querySelector("#preview-hidden")!.addEventListener("click", () => island.fsm.forceHidden());
document.querySelector("#preview-notify")!.addEventListener("click", () => {
  const chat = chats[0];
  const message = { id: String(++sequence), text: "Ich bin jetzt am See. Kommst du auch? 🌅", date: Math.floor(Date.now() / 1000), outgoing: false, senderName: "Mara" };
  messages.get(chat.id)?.push(message);
  chat.lastMessage = message.text;
  chat.unreadCount++;
  notification = { id: `fixture:${sequence}`, chat: { ...chat }, message };
  processTelegramNotifications(island, [notification]);
});
document.querySelector("#preview-repeat")!.addEventListener("click", () => {
  if (notification) processTelegramNotifications(island, [notification]);
});
document.querySelector("#preview-auto-close")!.addEventListener("click", () => {
  State.settings.autoCloseInterval = 3;
  island.applySettings();
});
document.querySelector("#preview-connection")!.addEventListener("click", (event) => {
  summary.state = summary.state === "ready" ? "disconnected" : "ready";
  (event.target as HTMLButtonElement).textContent = summary.state === "ready" ? "Disconnect" : "Reconnect";
  State.notify();
});
report();

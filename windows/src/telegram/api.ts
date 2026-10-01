import { invoke } from "@tauri-apps/api/core";

export interface TelegramStatus {
  state: string;
  configured: boolean;
  runtimeAvailable: boolean;
  paused: boolean;
  message?: string | null;
  error?: string | null;
  accountName?: string | null;
}

export interface TelegramChat {
  id: string;
  title: string;
  lastMessage: string;
  unreadCount: number;
}

export interface TelegramMessage {
  id: string;
  text: string;
  date: number;
  outgoing: boolean;
  senderName?: string | null;
}

export interface TelegramNotification {
  id: string;
  chat: TelegramChat;
  message: TelegramMessage;
}

export const Telegram = {
  status: () => invoke<TelegramStatus>("telegram_status"),
  configure: (apiId: number, apiHash: string) => invoke<void>("telegram_configure", { apiId, apiHash }),
  connect: () => invoke<void>("telegram_connect"),
  authenticate: (kind: "phone" | "code" | "password" | "email" | "emailCode", value: string) =>
    invoke<void>("telegram_authenticate", { kind, value }),
  chats: () => invoke<TelegramChat[]>("telegram_chats"),
  history: (chatId: string, fromMessageId?: string) => invoke<TelegramMessage[]>("telegram_history", { chatId, fromMessageId }),
  send: (chatId: string, text: string) => invoke<TelegramMessage>("telegram_send", { chatId, text }),
  disconnect: () => invoke<void>("telegram_disconnect"),
  logout: () => invoke<void>("telegram_logout"),
  openWindow: () => invoke<void>("open_telegram_window"),
};

export function telegramStatusLabel(state: string): string {
  return ({
    unconfigured: "Not connected",
    missing_runtime: "Telegram runtime is missing",
    disconnected: "Disconnected",
    connecting: "Connecting…",
    parameters: "Preparing your session…",
    phone: "Phone number required",
    code: "Login code required",
    password: "Two-step verification required",
    email: "Email address required",
    emailCode: "Email verification required",
    registration: "An existing Telegram account is required",
    ready: "Connected",
    paused: "Paused",
    loggingOut: "Logging out…",
    closing: "Disconnecting…",
    closed: "Disconnected",
    error: "Connection needs attention",
  } as Record<string, string>)[state] ?? "Finish signing in";
}

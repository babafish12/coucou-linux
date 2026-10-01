// Chat view — DOM port of PromptView / ChatBubble / TypingDotsView from
// IslandViewContent.swift.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type ChatContext } from "../core/bridge";
import { Sound } from "../core/sound";
import { State, type ChatMessage } from "../core/state";
import type { ViewHost } from "./views";

let nextId = 1;

function bubble(message: ChatMessage): HTMLElement {
  if (message.role === "user") {
    return h(
      "div",
      { class: "chat-row user" },
      h("div", { class: "bubble", text: message.content }),
    );
  }
  const metadata = message.model
    ? h("div", {
        class: "reply-config",
        text: `Requested: ${message.model}${message.reasoningEffort ? ` · ${message.reasoningEffort} reasoning` : ""}`,
        title: "Model and reasoning sent to Codex for this reply. Codex CLI does not report the provider's runtime model identity.",
      })
    : null;
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "reply" }, metadata, h("div", { text: message.content })),
  );
}

function typingDots(): HTMLElement {
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "typing" }, h("i"), h("i"), h("i")),
  );
}

/** The coloured chip showing what the question is about (a dropped file). */
function contextChip(label: string): HTMLElement {
  const chip = h("div", { class: "chip" }, h("i", { class: "chip-dot" }), h("span", { text: label }));
  requestAnimationFrame(() => chip.classList.add("settled"));
  return chip;
}

export function buildPrompt(onHeightChange: () => void): ViewHost {
  const chipRow = h("div", { class: "chip-row" });
  const log = h("div", { class: "chat-log" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: "Ask me anything…",
    spellcheck: "false",
  }) as HTMLInputElement;
  const send = h("button", { class: "send-btn", title: "Send" }, svg(ICONS.arrowUp, 11));
  const bar = h("div", { class: "chat-bar" }, input, send);

  const el = h(
    "div",
    { class: "view" },
    h("div", { class: "card wash chat-card" }, h("div", { class: "chat-body" }, chipRow, log, bar)),
  );
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(99,102,241,0.5)");

  let sending = false;
  let renderedCount = -1;
  let renderedHistory: ChatMessage[] | null = null;

  async function submit() {
    const query = input.value.trim();
    const file = State.droppedFile;
    if (!query || sending || (file && !file.path)) return;
    input.value = "";
    sending = true;
    Sound.play("send");

    const history = State.chatHistory;
    const message: ChatMessage = { id: nextId++, role: "user", content: query };
    history.push(message);
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();

    const context: ChatContext | null =
      file ? { kind: "file", name: file.name, path: file.path } : null;

    try {
      const reply = await Bridge.chatSend(query, context);
      if (State.chatHistory !== history) return;
      history.push({
        id: nextId++,
        role: "assistant",
        content: reply.text,
        model: reply.model,
        reasoningEffort: reply.reasoningEffort,
      });
      State.stateOverride = null;
      Sound.play("finish");
    } catch (err) {
      if (State.chatHistory !== history) return;
      if (history[history.length - 1] === message) history.pop();
      input.value = query;
      State.stateOverride = null;
      State.noteMessage = String(err).replace(/^Error:\s*/, "");
      State.view = "note";
      Sound.play("error");
    } finally {
      sending = false;
      State.notify();
      if (State.chatHistory === history) {
        onHeightChange();
        if (State.view === "prompt") input.focus();
      }
    }
  }

  // Dock windows need an explicit activation when returning from another app.
  input.addEventListener("pointerdown", () => void Bridge.focusWindow(true));
  send.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation(); // Escape closes the island, not the chat
  });

  return {
    el,
    sync() {
      const file = State.droppedFile;
      const wantChip = file?.name ?? "";
      if (chipRow.dataset.label !== wantChip) {
        chipRow.dataset.label = wantChip;
        clear(chipRow);
        if (wantChip) chipRow.append(contextChip(wantChip));
      }

      const thinking = State.stateOverride === "thinking";
      const count = State.chatHistory.length + (thinking ? 0.5 : 0);
      if (count !== renderedCount || renderedHistory !== State.chatHistory) {
        renderedCount = count;
        renderedHistory = State.chatHistory;
        clear(log);
        for (const m of State.chatHistory) log.append(bubble(m));
        if (thinking) log.append(typingDots());
        log.scrollTop = log.scrollHeight;
      }

      const copying = Boolean(file && !file.path);
      input.placeholder = copying ? "Copying attachment…" : State.chatHistory.length === 0 ? "Ask me anything…" : "Continue…";
      input.disabled = sending || copying;
      send.disabled = sending || copying;
    },
    focus() {
      input.focus();
      input.select();
    },
  };
}

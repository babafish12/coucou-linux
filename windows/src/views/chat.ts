// Chat view — DOM port of PromptView / ChatBubble / TypingDotsView from
// IslandViewContent.swift.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import { Bridge, type ChatContext, type ChatEvent } from "../core/bridge";
import { Sound } from "../core/sound";
import { State, type ChatMessage } from "../core/state";
import type { ViewHost } from "./views";

let nextId = 1;

function replyMetadata(message: ChatMessage): string {
  return message.model
    ? `Requested: ${message.model}${message.reasoningEffort ? ` · ${message.reasoningEffort} reasoning` : ""}`
    : "";
}

function bubble(message: ChatMessage): HTMLElement {
  if (message.role === "user") {
    return h(
      "div",
      { class: "chat-row user" },
      h("div", { class: "bubble", text: message.content }),
    );
  }
  const metadata = h("div", {
        class: "reply-config",
        text: replyMetadata(message),
        title: "Model and reasoning sent to Codex for this reply. Codex CLI does not report the provider's runtime model identity.",
      });
  return h(
    "div",
    { class: "chat-row" },
    h("div", { class: "reply" }, metadata, h("div", { class: "reply-text", text: message.content })),
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

export function buildPrompt(onHeightChange: () => void, onApproval: () => void = () => {}): ViewHost {
  const chipRow = h("div", { class: "chip-row" });
  const log = h("div", { class: "chat-log" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: "Ask me anything…",
    spellcheck: "false",
  }) as HTMLInputElement;
  const send = h("button", { class: "send-btn", title: "Send" }, svg(ICONS.arrowUp, 11));
  const stop = h("button", { class: "chat-stop", text: "Stop", title: "Stop the current reply", hidden: true });
  const status = h("div", { class: "chat-status", role: "status" });
  const approval = h("div", { class: "chat-approval", hidden: true });
  const bar = h("div", { class: "chat-bar" }, input, send, stop);

  const el = h(
    "div",
    { class: "view" },
    h("div", { class: "card wash chat-card" }, h("div", { class: "chat-body" }, chipRow, log, status, bar)),
  );
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(99,102,241,0.5)");

  type Request = {
    id: string;
    history: ChatMessage[];
    assistant: ChatMessage;
    itemId?: string;
    stopping: boolean;
  };
  let active: Request | null = null;
  const approvals = new Map<string, ChatEvent>();
  let renderedCount = -1;
  let renderedHistory: ChatMessage[] | null = null;
  const renderedReplies = new Map<number, HTMLElement>();
  let scrollToApproval = false;

  function current(request: Request): boolean {
    return active === request && State.chatHistory === request.history;
  }

  function renderApproval() {
    clear(approval);
    const entry = approvals.entries().next().value;
    approval.hidden = !entry;
    if (!entry || !active) return;
    const [approvalId, event] = entry;
    const request = active;
    const details = [event.text, event.command, event.cwd ? `Folder: ${event.cwd}` : null].filter(Boolean).join("\n\n");
    const allow = h("button", { class: "chat-allow", text: "Allow once" });
    const deny = h("button", { class: "chat-deny", text: "Deny" });
    const error = h("div", { class: "chat-status", role: "alert" });
    const decide = async (accepted: boolean) => {
      if (!current(request)) return;
      allow.disabled = deny.disabled = true;
      try {
        await Bridge.chatApprove(request.id, approvalId, accepted);
        if (!current(request)) return;
        approvals.delete(approvalId);
        renderApproval();
      } catch (err) {
        if (!current(request)) return;
        error.textContent = String(err).replace(/^Error:\s*/, "");
        allow.disabled = deny.disabled = false;
      }
    };
    allow.addEventListener("click", () => void decide(true));
    deny.addEventListener("click", () => void decide(false));
    approval.append(
      h("strong", { text: "Codex needs your approval" }),
      h("pre", { class: "chat-approval-details", text: details }),
      h("div", { class: "chat-approval-actions" }, deny, allow), error,
    );
  }

  function progress(request: Request, event: ChatEvent) {
    if (!current(request) || event.requestId !== request.id) return;
    if (event.model) request.assistant.model = event.model;
    if (event.reasoningEffort) request.assistant.reasoningEffort = event.reasoningEffort;
    switch (event.kind) {
      case "delta":
      case "message":
        if (event.itemId !== request.itemId) {
          request.assistant.content = "";
          request.itemId = event.itemId;
        }
        if (event.kind === "message") request.assistant.content = event.text ?? "";
        else request.assistant.content += event.text ?? "";
        status.textContent = "";
        break;
      case "status":
        status.textContent = event.text ?? "";
        break;
      case "approval":
        if (event.approvalId) approvals.set(event.approvalId, event);
        scrollToApproval = true;
        status.textContent = "";
        renderApproval();
        onApproval();
        break;
      case "approvalResolved":
        if (event.approvalId) approvals.delete(event.approvalId);
        renderApproval();
        break;
    }
    State.notify();
  }

  async function submit() {
    const query = input.value.trim();
    const file = State.droppedFile;
    if (!query || active || (file && !file.path)) return;
    input.value = "";
    Sound.play("send");

    const history = State.chatHistory;
    const message: ChatMessage = { id: nextId++, role: "user", content: query };
    const assistant: ChatMessage = { id: nextId++, role: "assistant", content: "" };
    const request: Request = { id: crypto.randomUUID(), history, assistant, stopping: false };
    active = request;
    approvals.clear();
    renderApproval();
    status.textContent = "Connecting to Codex…";
    history.push(message, assistant);
    State.stateOverride = "thinking";
    State.notify();
    onHeightChange();

    const context: ChatContext | null =
      file ? { kind: "file", name: file.name, path: file.path } : null;

    try {
      const reply = await Bridge.chatSend(query, context, request.id, (event) => progress(request, event));
      if (!current(request)) return;
      Object.assign(assistant, {
        content: reply.text,
        model: reply.model,
        reasoningEffort: reply.reasoningEffort,
      });
      State.stateOverride = null;
      status.textContent = "";
      Sound.play("finish");
    } catch (err) {
      if (!current(request)) return;
      if (!assistant.content) history.splice(history.indexOf(assistant), 1);
      State.stateOverride = null;
      // Actions may already have happened; keep the transcript and never retry automatically.
      status.textContent = request.stopping ? "Stopped." : String(err).replace(/^Error:\s*/, "");
      if (!request.stopping) Sound.play("error");
    } finally {
      if (active !== request) return;
      active = null;
      approvals.clear();
      renderApproval();
      State.notify();
      if (State.chatHistory === history) {
        onHeightChange();
        if (State.view === "prompt") input.focus();
      }
    }
  }

  stop.addEventListener("click", async () => {
    const request = active;
    if (!request || request.stopping) return;
    request.stopping = true;
    stop.disabled = true;
    status.textContent = "Stopping…";
    try {
      await Bridge.chatCancel(request.id);
    } catch (err) {
      if (!current(request)) return;
      request.stopping = false;
      stop.disabled = false;
      status.textContent = String(err).replace(/^Error:\s*/, "");
    }
  });

  // Dock windows need an explicit activation when returning from another app.
  input.addEventListener("pointerdown", () => void Bridge.focusWindow(true));
  send.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Escape") return; // Let the island's close handler run.
    if ((e as KeyboardEvent).key === "Enter") {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation();
  });

  return {
    el,
    sync() {
      if (active && State.chatHistory !== active.history) {
        active = null;
        approvals.clear();
        renderApproval();
        status.textContent = "";
      }
      const file = State.droppedFile;
      const wantChip = file?.name ?? "";
      if (chipRow.dataset.label !== wantChip) {
        chipRow.dataset.label = wantChip;
        clear(chipRow);
        if (wantChip) chipRow.append(contextChip(wantChip));
      }

      const thinking = active !== null && !active.assistant.content && approvals.size === 0;
      const count = State.chatHistory.length + (thinking ? 0.5 : 0);
      if (count !== renderedCount || renderedHistory !== State.chatHistory) {
        renderedCount = count;
        renderedHistory = State.chatHistory;
        clear(log);
        renderedReplies.clear();
        for (const m of State.chatHistory) {
          const row = bubble(m);
          log.append(row);
          if (m.role === "assistant") renderedReplies.set(m.id, row);
        }
        if (thinking) log.append(typingDots());
        log.append(approval);
        log.scrollTop = log.scrollHeight;
      } else {
        const follow = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
        for (const m of State.chatHistory) {
          const row = renderedReplies.get(m.id);
          const text = row?.querySelector(".reply-text");
          const metadata = row?.querySelector(".reply-config");
          if (text && text.textContent !== m.content) text.textContent = m.content;
          if (metadata) metadata.textContent = replyMetadata(m);
        }
        if (follow) log.scrollTop = log.scrollHeight;
      }
      if (scrollToApproval) {
        log.scrollTop = log.scrollHeight;
        scrollToApproval = false;
      }

      const copying = Boolean(file && !file.path);
      input.placeholder = copying ? "Copying attachment…" : State.chatHistory.length === 0 ? "Ask me anything…" : "Continue…";
      input.disabled = active !== null || copying;
      send.disabled = active !== null || copying;
      send.hidden = active !== null && State.agentProvider === "codex";
      stop.hidden = active === null || State.agentProvider !== "codex";
      stop.disabled = active?.stopping ?? false;
    },
    focus() {
      input.focus();
      input.select();
    },
  };
}

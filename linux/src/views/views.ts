// Island views using the shared Coucou visual design.

import { h, svg, clear, dot } from "./dom";
import { ICONS } from "./icons";
import { State, type AgentTask } from "../core/state";
import { washRGBA, type IslandViewName, type Wash } from "../core/layout";
import { createMiniBot, pruneMiniBots } from "../mochi/minibots";
import { buildPrompt } from "./chat";
import { buildChoose, buildUpload, buildUploading } from "./upload";
import { renderIntegrationCard, type IntegrationCardHooks } from "./integrations";
import { buildTelegram, buildTelegramNotification } from "./telegram";
import { InlineTelegram, selectInlineChat } from "../telegram/inline";
import { TelegramNotifications } from "../telegram/notifications";
import { CodexActivity, codexActivityStatus, codexProjectName } from "../codex/activity";
import { buildCodex } from "./codex";

export interface ViewActions {
  setView(v: IslandViewName): void;
  showTelegramNotification(): void;
  collapse(): void;
  setFocus(id: string): void;
  openSession(sessionId?: string): Promise<void>;
  focusCodex(cwd?: string): Promise<void>;
  /** The ↗ button: opens whatever the focused pill points at. */
  openTarget(): void;
  openUrl(url: string): void;
  toggleSound(): void;
  setVolume(v: number): void;
  setAutoClose(seconds: number): void;
  openSettingsWindow(): void;
  blip(): void;
}

export interface ViewHost {
  el: HTMLElement;
  sync(): void;
  /** Called when the view becomes active, for views with a text field. */
  focus?(): void;
  /** Called every frame while the view is on screen. */
  tick?(nowMs: number): void;
  /** True while the visible view needs another animation frame. */
  readonly animating?: boolean;
}

// ── Shared pieces ─────────────────────────────────────────────────────────────

function card(wash: Wash, ...children: (Node | string)[]): HTMLElement {
  const el = h("div", { class: wash ? "card wash" : "card" }, ...children);
  if (wash) el.style.setProperty("--wash", washRGBA(wash));
  return el;
}

function btn(
  label: string,
  kind: "primary" | "secondary",
  onClick: () => void,
  kbd?: string,
): HTMLButtonElement {
  return h(
    "button",
    { class: `btn ${kind}`, onclick: onClick },
    h("span", { text: label }),
    kbd ? h("span", { class: "kbd", text: kbd }) : null,
  );
}

/** AgentWho — coloured dot + task name + grey label. */
function agentWho(task: AgentTask | null, label: string): HTMLElement {
  const row = h("div", { class: "who-row" });
  if (task) {
    row.append(dot(task.color, 8), h("span", { class: "n", text: task.name }));
  }
  row.append(h("span", { text: label }));
  return row;
}

function stack(padLeft: number, padRight: number, ...children: Node[]): HTMLElement {
  const el = h("div", { class: "stack" }, ...children);
  el.style.padding = `4px ${padRight}px 4px ${padLeft}px`;
  return el;
}

// ── Header ────────────────────────────────────────────────────────────────────

export function buildHeader(actions: ViewActions): ViewHost {
  const tabHome = h("button", { class: "tab", title: "Overview", onclick: () => go("overview") }, svg(ICONS.house, 13));
  const tabChat = h("button", { class: "tab", title: "Ask", onclick: () => go("prompt") }, svg(ICONS.bubble, 13));
  const tabDrop = h("button", { class: "tab", title: "Drop", onclick: () => go("upload") }, svg(ICONS.plus, 13));

  const gearBtn = h("button", { title: "Settings", onclick: () => go("settings") }, svg(ICONS.gear, 14));
  const soundBtn = h("button", { title: "Mute", onclick: () => actions.toggleSound() }, svg(ICONS.speakerOn, 14));
  const telegramAlert = h("button", {
    class: "tg-alert-tab", "aria-label": "New Telegram message", hidden: true,
    onclick: () => actions.showTelegramNotification(),
  });

  function go(v: IslandViewName) {
    actions.blip();
    actions.setView(v);
  }

  const el = h(
    "div",
    { id: "header" },
    h("div", { class: "tabs" }, tabHome, tabChat, tabDrop),
    h("div", { class: "header-actions" }, telegramAlert, gearBtn, soundBtn),
  );

  return {
    el,
    sync() {
      const v = State.view;
      tabHome.classList.toggle("on", v === "overview" || v === "empty" || v === "codex" || v === "telegram" || v === "telegram-notification");
      const notification = TelegramNotifications.current;
      telegramAlert.hidden = !notification || v === "telegram-notification";
      telegramAlert.textContent = notification ? `Telegram · ${notification.chat.title}` : "";
      telegramAlert.title = notification ? `${notification.chat.title}: ${notification.message.text}` : "";
      tabChat.classList.toggle("on", v === "prompt");
      tabDrop.classList.toggle("on", v === "upload");
      gearBtn.classList.toggle("on", v === "settings");
      clear(gearBtn);
      gearBtn.append(svg(v === "settings" ? ICONS.gearFill : ICONS.gear, 14));
      clear(soundBtn);
      soundBtn.append(svg(State.settings.soundEnabled ? ICONS.speakerOn : ICONS.speakerOff, 14));
      el.style.opacity = v === "confused" ? "0" : "1";
    },
  };
}

// ── Overview ──────────────────────────────────────────────────────────────────

function buildOverview(actions: ViewActions): ViewHost {
  const leftBody = h("div", { class: "left-body" });
  const jump = h(
    "button",
    { class: "icon-btn jump", title: "Open", onclick: () => actions.openTarget() },
    svg(ICONS.arrowUpRight, 8),
  );
  const left = card(null, leftBody, jump);
  const pills = h("div", { class: "pills" });
  const right = card(null, pills);

  const el = h("div", { class: "view overview" },
    h("div", { class: "left" }, left),
    h("div", { class: "right" }, right),
  );

  let pillIds = "";
  let detailOpen = false;
  let lastFocus: string | null = null;
  let cardKey = "";

  const hooks: IntegrationCardHooks = {
    get detailOpen() {
      return detailOpen;
    },
    openDetail() {
      detailOpen = true;
      cardKey = "";
      State.notify();
    },
    closeDetail() {
      detailOpen = false;
      cardKey = "";
      State.notify();
    },
    openSettings: () => actions.openSettingsWindow(),
    openTelegram(chat) {
      if (chat) selectInlineChat(chat);
      actions.blip();
      actions.setView("telegram");
    },
  };

  return {
    el,
    sync() {
      const task = State.focusTask;
      if (task?.id !== lastFocus) {
        lastFocus = task?.id ?? null;
        detailOpen = false;
        cardKey = "";
      }

      const codex = task?.id === "integration_codex";
      if (codex) {
        const session = CodexActivity.latest;
        const entry = session?.entries.filter((entry) => entry.kind !== "status").at(-1) ?? CodexActivity.latestEntry;
        const key = JSON.stringify([session, CodexActivity.loaded, CodexActivity.error]);
        if (key !== cardKey) {
          cardKey = key;
          const active = session?.state === "thinking" || session?.state === "working";
          const summary = entry ? entry.kind === "tool" && entry.detail
            ? `${entry.text}: ${entry.detail.split("\n")[0]}` : entry.text
            : CodexActivity.error || (CodexActivity.loaded ? "No recent sessions" : "Loading activity…");
          const project = session ? codexProjectName(session.cwd) : "Activity";
          leftBody.replaceChildren(h("button", {
            class: "codex-home", type: "button", "aria-label": "Open Codex activity",
            onclick: () => {
              if (session) CodexActivity.select(session.id);
              actions.blip();
              actions.setView("codex");
            },
          },
          h("span", { class: "codex-home-head" }, dot(active ? "#34D399" : "#F5F6F8", 7),
            h("b", { text: "Codex" }), h("span", { text: project || "Activity" })),
          h("span", { class: "codex-home-task", text: session?.title || "Local sessions", title: session?.title || "" }),
          h("span", { class: "codex-home-summary", text: summary, title: summary }),
          h("span", { class: "codex-home-open", text: session ? `${codexActivityStatus(session.state)} · View activity ›` : "View activity ›" })));
        }
      } else if (task) {
        const info = State.integrations[task.id];
        const key = [
          task.id, detailOpen, task.state, task.steps.join("|"),
          info?.loaded, info?.error, info?.configured,
          JSON.stringify(info?.data ?? {}),
          task.id === "integration_telegram" ? JSON.stringify([InlineTelegram.chats, InlineTelegram.loaded, InlineTelegram.listError]) : "",
        ].join("~");
        if (key !== cardKey) {
          cardKey = key;
          clear(leftBody);
          leftBody.append(renderIntegrationCard(task, hooks));
        }
      }

      jump.style.display = detailOpen ? "none" : "";
      jump.title = codex ? "Focus Codex window" : "Open";
      jump.setAttribute("aria-label", jump.title);

      const others = State.otherTasks.slice(0, 4);
      const pillKey = others.map((t) => `${t.id}:${t.pillBadge ?? ""}`).join("|");
      if (pillKey !== pillIds) {
        pillIds = pillKey;
        clear(pills);
        for (const t of others) pills.append(buildPill(t, actions));
        pruneMiniBots();
      }
    },
  };
}

function buildPill(task: AgentTask, actions: ViewActions): HTMLElement {
  const label = task.id === "integration_codex" ? State.agentAppLabel : task.name;
  const canvas = createMiniBot(task, 24, "overview");
  const pill = h(
    "div",
    { class: "pill", onclick: () => actions.setFocus(task.id) },
    canvas,
    h("span", { class: "lbl", text: label }),
  );
  pill.style.borderColor = `${task.color}24`;
  pill.addEventListener("mouseenter", () => {
    pill.style.background = `${task.color}2e`;
    pill.style.borderColor = `${task.color}8c`;
    pill.style.boxShadow = `0 2px 10px ${task.color}59`;
    (pill.querySelector(".lbl") as HTMLElement).style.color = lighten(task.color, 0.3);
  });
  pill.addEventListener("mouseleave", () => {
    pill.style.background = "";
    pill.style.borderColor = `${task.color}24`;
    pill.style.boxShadow = "";
    (pill.querySelector(".lbl") as HTMLElement).style.color = "";
  });

  if (task.pillBadge) {
    const colors = { approval: "#F5A524", finished: "#22C55E", error: "#F4505E" } as const;
    const icons = { approval: ICONS.bang, finished: ICONS.check, error: ICONS.xmark } as const;
    const inner = h("i", { style: `background:${colors[task.pillBadge]}` }, svg(icons[task.pillBadge], 6, { stroke: task.pillBadge === "finished" ? 3 : 0 }));
    const badge = h("div", { class: "pill-badge" }, inner);
    badge.style.boxShadow = `0 0 4px ${colors[task.pillBadge]}99`;
    pill.append(badge);
  }
  return pill;
}

function lighten(hex: string, amount: number): string {
  const v = parseInt(hex.replace("#", ""), 16);
  const c = [(v >> 16) & 255, (v >> 8) & 255, v & 255].map((x) =>
    Math.min(255, Math.round(x + amount * 255)),
  );
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

// ── Empty ─────────────────────────────────────────────────────────────────────

function buildEmpty(actions: ViewActions): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 118px;flex-direction:row;align-items:center;gap:16px" },
    h(
      "div",
      { style: "display:flex;flex-direction:column;gap:5px" },
      h("div", { class: "title", text: "Nothing running right now." }),
      h("div", { class: "sub", text: "Drop a file or window, or ask me anything." }),
    ),
    h("div", { class: "grow" }),
    btn("Ask Codex", "primary", () => actions.setView("prompt")),
  );
  return { el: h("div", { class: "view" }, card(null, body)), sync() {} };
}

// ── Question ──────────────────────────────────────────────────────────────────

function buildQuestion(): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title" });
  const row = h("div", { class: "actions" });
  const el = h("div", { class: "view" }, card("cyan", stack(116, 16, who, title, row)));
  return {
    el,
    sync() {
      clear(who);
      who.append(agentWho(State.focusTask, `${State.agentLabel} is asking a question`));
      const task = State.focusTask;
      title.textContent = task?.steps.at(-1) ?? `${State.agentLabel} needs an answer.`;
      clear(row);
      row.append(h("div", { class: "sub", text: "Answer in your terminal — Coucou can't reply for you yet." }));
    },
  };
}

// ── Error ─────────────────────────────────────────────────────────────────────

function buildError(actions: ViewActions): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title", text: "Workflow stopped." });
  const detail = h("div", { class: "detail" });
  const row = h("div", { class: "actions" },
    btn("Retry", "primary", () => actions.setView(State.defaultView())),
    btn("Open in n8n", "secondary", () => actions.openUrl("")),
  );
  const el = h("div", { class: "view" }, card("red", stack(116, 16, who, title, detail, row)));
  return {
    el,
    sync() {
      const task = State.focusTask;
      clear(who);
      who.append(agentWho(task, task?.source === "n8n" ? "n8n" : State.agentLabel));
      title.textContent = task?.source === "n8n" ? "Workflow stopped." : "Session stopped on an error.";
      detail.textContent = task?.steps.at(-1) ?? "No detail available.";
    },
  };
}

// ── Finished ──────────────────────────────────────────────────────────────────

function buildFinished(actions: ViewActions): ViewHost {
  const who = h("div");
  const title = h("div", { class: "title", role: "status" });
  let error = "";
  const renderTitle = () => {
    title.textContent = error || State.focusTask?.steps.at(-1) || "Session finished";
    title.title = title.textContent;
    title.style.color = error ? "#F4505E" : "";
  };
  const open = btn("Open chat", "primary", async () => {
    const openingSessionId = State.focusTask?.sessionId;
    open.disabled = true;
    error = "";
    renderTitle();
    try {
      await actions.openSession();
    } catch (reason) {
      if (State.focusTask?.sessionId === openingSessionId) {
        error = String(reason instanceof Error ? reason.message : reason);
        renderTitle();
      }
    } finally {
      open.disabled = false;
    }
  });
  const row = h("div", { class: "actions" },
    open,
    btn("OK", "secondary", () => actions.collapse()),
  );
  const el = h("div", { class: "view" }, card("green", stack(116, 16, who, title, row)));
  let sessionId: string | null | undefined;
  return {
    el,
    sync() {
      clear(who);
      who.append(agentWho(State.focusTask, `${State.agentLabel} finished`));
      if (sessionId !== State.focusTask?.sessionId) error = "";
      sessionId = State.focusTask?.sessionId;
      renderTitle();
    },
  };
}

// ── Confused ──────────────────────────────────────────────────────────────────

function buildConfused(): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 128px" },
    h("div", { class: "title", text: "Too many hits at once." }),
    h("div", { class: "sub", text: "Give me a sec — back to work in three seconds." }),
  );
  return { el: h("div", { class: "view" }, card("pink", body)), sync() {} };
}

// ── Note ──────────────────────────────────────────────────────────────────────

function buildNote(): ViewHost {
  const title = h("div", { class: "title" });
  const el = h("div", { class: "view" }, card(null, h("div", { class: "stack", style: "padding:0 18px 0 98px" }, title)));
  return {
    el,
    sync() {
      title.textContent = State.noteMessage ?? "";
    },
  };
}

// ── In-island settings ────────────────────────────────────────────────────────

function buildSettings(actions: ViewActions): ViewHost {
  const soundSwitch = h("button", { class: "switch", onclick: () => actions.toggleSound() });
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    oninput: (e: Event) => actions.setVolume(Number((e.target as HTMLInputElement).value)),
  }) as HTMLInputElement;
  const autoLabel = h("span", {});
  const autoClosePresets = [1, 3, 5, 10, 15, 30];
  const segButtons = autoClosePresets.map((s) =>
    h("button", { onclick: () => actions.setAutoClose(s) }, `${s}s`),
  );
  const codexBadge = h("span", { class: "status-badge" });

  const rows = h(
    "div",
    { class: "settings-rows" },
    h("div", { class: "settings-row" }, soundSwitch, h("span", { text: "Sound" }), volume),
    h(
      "div",
      { class: "settings-row" },
      svg(ICONS.timer, 12),
      autoLabel,
      h("div", { class: "seg" }, ...segButtons),
    ),
    h(
      "div",
      { class: "settings-row", style: "gap:14px" },
      codexBadge,
      h("div", { class: "grow" }),
      h("button", {
        class: "link-btn",
        style: "color:#8e939c;font-size:11.5px",
        text: "All settings…",
        title: "Activation, auto-hide, integrations and Codex",
        onclick: () => actions.openSettingsWindow(),
      }),
    ),
  );

  const el = h("div", { class: "view" },
    card(null, h("div", { class: "stack", style: "padding:14px 16px 14px 84px" }, rows)));

  return {
    el,
    sync() {
      const s = State.settings;
      soundSwitch.classList.toggle("on", s.soundEnabled);
      volume.value = String(s.soundVolume);
      volume.style.opacity = s.soundEnabled ? "1" : "0.4";
      autoLabel.textContent = `Auto-close · ${s.autoCloseInterval}s`;
      segButtons.forEach((b, i) => b.classList.toggle("on", s.autoCloseInterval === autoClosePresets[i]));
      clear(codexBadge);
      codexBadge.append(
        dot(CodexActivity.loaded && !CodexActivity.error ? "#22C55E" : "#F4505E", 6),
        h("span", { text: State.agentLabel }),
      );
    },
  };
}

// ── Placeholders filled in later stages ───────────────────────────────────────

function buildPlaceholder(title: string, sub: string): ViewHost {
  const body = h(
    "div",
    { class: "stack", style: "padding:0 18px 0 118px" },
    h("div", { class: "title", text: title }),
    h("div", { class: "sub", text: sub }),
  );
  return { el: h("div", { class: "view" }, card(null, body)), sync() {} };
}

// ── Registry ──────────────────────────────────────────────────────────────────

export function buildViews(
  actions: ViewActions,
  onChatHeightChange: () => void,
): Map<IslandViewName, ViewHost> {
  const map = new Map<IslandViewName, ViewHost>();
  map.set("overview", buildOverview(actions));
  map.set("empty", buildEmpty(actions));
  map.set("question", buildQuestion());
  map.set("error", buildError(actions));
  map.set("finished", buildFinished(actions));
  map.set("confused", buildConfused());
  map.set("note", buildNote());
  map.set("settings", buildSettings(actions));
  map.set("prompt", buildPrompt(onChatHeightChange, () => actions.setView("prompt")));
  map.set("codex", buildCodex(actions));
  map.set("telegram", buildTelegram(actions));
  map.set("telegram-notification", buildTelegramNotification(actions));
  map.set("upload", buildUpload());
  map.set("uploading", buildUploading());
  map.set("choose", buildChoose(actions));
  // Reserved views for unfinished email and search flows.
  map.set("mail", buildPlaceholder("Sending by email isn't in this version.", ""));
  map.set("searching", buildPlaceholder(`${State.agentLabel} is searching…`, ""));
  map.set("result", buildPlaceholder("Result", ""));
  return map;
}

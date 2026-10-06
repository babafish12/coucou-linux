import "../codex/activity.css";
import { CodexActivity, codexActivityStatus, codexCurrentWork, codexProjectName, codexSessionActive, type ActivityEntry } from "../codex/activity";
import { h, svg, dot } from "./dom";
import { ICONS } from "./icons";
import type { ViewActions, ViewHost } from "./views";

function entryTime(timestamp: number): HTMLTimeElement {
  const date = new Date(timestamp);
  return Number.isFinite(date.getTime())
    ? h("time", { datetime: date.toISOString(), title: date.toLocaleString(), text: date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) })
    : h("time");
}

function activityRow(entry: ActivityEntry, expanded: boolean): HTMLElement {
  const row = h("article", { class: `codex-entry ${entry.kind}` });
  row.dataset.entryId = entry.id;
  const label = entry.kind === "tool" ? "Tool" : entry.kind === "message" ? "Codex" : "Status";
  row.append(h("div", { class: "codex-entry-heading" }, h("b", { text: label }), entryTime(entry.timestamp)),
    h("div", { class: "codex-entry-text", text: entry.text }));
  if (entry.detail) {
    const details = h("details", { class: "codex-entry-details" },
      h("summary", { text: "Show details" }), h("pre", { text: entry.detail }));
    details.dataset.entryId = entry.id;
    details.open = expanded;
    row.append(details);
  }
  return row;
}

export function buildCodex(actions: ViewActions): ViewHost {
  const back = h("button", {
    class: "codex-back", title: "Back to Home", "aria-label": "Back to Home",
    onclick: () => actions.setView("overview"),
  }, svg(ICONS.chevronLeft, 13, { stroke: 2 }));
  const state = h("span", { class: "codex-current-state" });
  const focusNotice = h("div", { class: "codex-notice", role: "status", "aria-live": "polite" });
  const focus = h("button", { class: "codex-focus", text: "Focus Codex", onclick: () => {
    focus.disabled = true;
    focusNotice.textContent = "";
    void Promise.resolve().then(() => actions.focusCodex(CodexActivity.selected?.cwd)).catch((error: unknown) => {
      focusNotice.textContent = error instanceof Error ? error.message : String(error);
    }).finally(() => { focus.disabled = false; });
  } });
  let opening = false;
  const open = h("button", { class: "codex-open", text: "Open chat", onclick: () => {
    const sessionId = CodexActivity.selected?.id;
    if (!sessionId || opening) return;
    opening = true;
    open.disabled = true;
    focusNotice.textContent = "";
    void Promise.resolve().then(() => actions.openSession(sessionId)).catch((error: unknown) => {
      if (CodexActivity.selected?.id === sessionId) focusNotice.textContent = error instanceof Error ? error.message : String(error);
    }).finally(() => { opening = false; open.disabled = !CodexActivity.selected; });
  } });
  const sessions = h("nav", { class: "codex-session-list", "aria-label": "Local Codex sessions" });
  const sidebarLabel = h("span", { class: "codex-sidebar-label" });
  const sidebar = h("aside", { class: "codex-sidebar" }, sidebarLabel, sessions);
  const title = h("b", { class: "codex-session-title" });
  const project = h("span", { class: "codex-project" });
  const monitorNotice = h("div", { class: "codex-notice", role: "status" });
  const current = h("div", { class: "codex-current", role: "region", "aria-label": "Current work", tabindex: "0" });
  const history = h("div", { class: "codex-history", role: "region", "aria-label": "Codex activity", tabindex: "0" });
  let showingHistory = false;
  const showHistory = (show: boolean) => {
    showingHistory = show;
    current.hidden = show;
    history.hidden = !show;
    nowButton.setAttribute("aria-pressed", String(!show));
    historyButton.setAttribute("aria-pressed", String(show));
    view.sync();
    actions.blip();
  };
  const nowButton = h("button", { class: "codex-mode", type: "button", text: "Current work", "aria-pressed": "true", onclick: () => showHistory(false) });
  const historyButton = h("button", { class: "codex-mode", type: "button", text: "Activity", "aria-pressed": "false", onclick: () => showHistory(true) });
  history.hidden = true;
  const conversation = h("section", { class: "codex-conversation" },
    h("div", { class: "codex-session-heading" }, title, project), monitorNotice,
    h("div", { class: "codex-modes", "aria-label": "Activity display" }, nowButton, historyButton), current, history, focusNotice);
  const el = h("div", { class: "view codex-view" }, h("div", { class: "card codex-card" },
    h("header", { class: "codex-head" }, back, dot("#dce4e0", 6), h("b", { text: "Codex" }), state, focus, open),
    h("div", { class: "codex-workspace" }, sidebar, conversation),
  ));
  let displayedId: string | null = null;
  let historyId: string | null = null;
  let listKey = "";
  let historyKey = "";
  let currentKey = "";
  let currentMessageId: string | undefined;
  const positions = new Map<string, { top: number; bottom: boolean; entryId?: string; offset?: number }>();
  const expandedEntries = new Map<string, Set<string>>();

  const view: ViewHost = {
    el,
    sync() {
      const selected = CodexActivity.selected;
      const nextId = selected?.id ?? null;
      const changingSession = displayedId !== nextId;
      const atBottom = history.scrollHeight - history.scrollTop - history.clientHeight < 32;
      if (historyId && showingHistory) {
        const firstVisible = [...history.querySelectorAll<HTMLElement>(".codex-entry")]
          .find((row) => row.offsetTop + row.offsetHeight > history.scrollTop);
        positions.set(historyId, {
          top: history.scrollTop, bottom: atBottom,
          entryId: firstVisible?.dataset.entryId,
          offset: firstVisible ? firstVisible.offsetTop - history.scrollTop : undefined,
        });
        const expanded = new Set<string>();
        for (const detail of history.querySelectorAll<HTMLDetailsElement>("details[open]")) {
          if (detail.dataset.entryId) expanded.add(detail.dataset.entryId);
        }
        expandedEntries.set(historyId, expanded);
      }
      for (const id of positions.keys()) {
        if (!CodexActivity.sessions.some((session) => session.id === id)) {
          positions.delete(id);
          expandedEntries.delete(id);
        }
      }
      title.textContent = selected?.title || (selected ? codexProjectName(selected.cwd) : "Session activity");
      title.title = title.textContent;
      project.textContent = selected?.cwd ?? "Public updates and tool actions";
      project.title = project.textContent;
      state.textContent = selected ? codexActivityStatus(selected.state) : "";
      state.dataset.state = selected?.state ?? "idle";
      monitorNotice.textContent = CodexActivity.error;
      open.disabled = opening || !selected;
      if (changingSession) focusNotice.textContent = "";
      const activeCount = CodexActivity.sessions.filter(codexSessionActive).length;
      sidebarLabel.textContent = `Local chats · ${activeCount} active`;
      const work = selected ? codexCurrentWork(selected) : {};
      const nextCurrentKey = JSON.stringify([nextId, selected?.state, work, CodexActivity.loaded, CodexActivity.error]);
      if (currentKey !== nextCurrentKey) {
        const sameMessage = !changingSession && currentMessageId === work.message?.id;
        const expanded = sameMessage && current.querySelector<HTMLDetailsElement>("details")?.open;
        const scrollTop = sameMessage ? current.scrollTop : 0;
        currentKey = nextCurrentKey;
        currentMessageId = work.message?.id;
        current.replaceChildren();
        if (selected) {
          const label = selected.state === "finished" ? "Result" : selected.state === "interrupted" ? "Last update · interrupted" : "Latest update";
          const message = work.message;
          const summary = h("article", { class: "codex-summary" },
            h("div", { class: "codex-summary-heading" }, h("b", { text: label }), message ? entryTime(message.timestamp) : null),
            h("p", { class: "codex-summary-text", text: message?.text || (codexSessionActive(selected)
              ? "Codex is working on this request. Its next progress update will appear here."
              : "No public progress update was recorded for this turn. Open the chat to continue.") }));
          if (message?.detail) {
            const details = h("details", { class: "codex-entry-details" },
              h("summary", { text: "Read full update" }), h("pre", { text: message.detail }));
            details.open = !!expanded;
            summary.append(details);
          }
          current.append(summary);
          if (work.tool) {
            const tool = work.tool;
            const running = selected.state === "working" && (!message || tool.timestamp >= message.timestamp);
            current.append(h("article", { class: "codex-current-action" },
              h("div", { class: "codex-summary-heading" }, h("b", { text: running ? "Current action" : "Last action" }), entryTime(tool.timestamp)),
              h("p", { text: tool.text })));
          }
        } else current.append(h("p", { class: "codex-empty", text: CodexActivity.error
          ? "Activity is unavailable. Open Codex to continue."
          : CodexActivity.loaded ? "Start a local Codex conversation to see what it is working on here." : "Loading local Codex activity…" }));
        current.scrollTop = scrollTop;
      }

      const nextListKey = JSON.stringify([CodexActivity.sessions.map((session) => [session.id, session.cwd, session.title, session.state]), nextId, CodexActivity.loaded]);
      if (nextListKey !== listKey) {
        listKey = nextListKey;
        const scrollTop = sessions.scrollTop;
        sessions.replaceChildren();
        let group = "";
        for (const session of CodexActivity.sessions) {
          const nextGroup = codexSessionActive(session) ? "Working now" : "Recent";
          if (group !== nextGroup) {
            group = nextGroup;
            sessions.append(h("span", { class: "codex-session-group", text: group }));
          }
          const button = h("button", {
            class: "codex-session", type: "button", title: `${session.title}\n${session.cwd}`,
            "aria-current": session.id === nextId ? "true" : "false",
            onclick: () => { CodexActivity.select(session.id); actions.blip(); },
          }, h("span", { class: "codex-session-meta" }, h("span", { text: codexProjectName(session.cwd) }),
          h("span", { class: "codex-session-state", "data-state": session.state, text: codexActivityStatus(session.state) })),
          h("b", { text: session.title || codexProjectName(session.cwd) }));
          button.classList.toggle("selected", session.id === nextId);
          sessions.append(button);
        }
        if (!CodexActivity.sessions.length) sessions.append(h("p", { class: "codex-empty", text: CodexActivity.loaded ? "No sessions yet" : "Loading sessions…" }));
        sessions.scrollTop = scrollTop;
      }

      const nextHistoryKey = JSON.stringify([nextId, selected?.entries, CodexActivity.loaded, CodexActivity.error]);
      if (showingHistory && nextHistoryKey !== historyKey) {
        const firstRender = !historyKey;
        historyKey = nextHistoryKey;
        const position = nextId ? positions.get(nextId) : undefined;
        const expanded = nextId ? expandedEntries.get(nextId) : undefined;
        history.replaceChildren();
        for (const entry of selected?.entries ?? []) history.append(activityRow(entry, expanded?.has(entry.id) ?? false));
        if (!selected?.entries.length) history.append(h("p", {
          class: "codex-empty",
          text: CodexActivity.error && !selected ? "Activity is unavailable. Open Codex to continue."
            : !CodexActivity.loaded ? "Loading local Codex activity…"
            : !selected ? "Start a local Codex conversation. Its public updates and tool actions will appear here."
            : "No public updates or tool actions recorded in this session yet.",
        }));
        if (historyId !== nextId ? !position || position.bottom : firstRender || atBottom) history.scrollTop = history.scrollHeight;
        else {
          const anchor = [...history.querySelectorAll<HTMLElement>(".codex-entry")]
            .find((row) => row.dataset.entryId === position?.entryId);
          history.scrollTop = anchor && position?.offset != null
            ? anchor.offsetTop - position.offset : position?.top ?? history.scrollTop;
        }
        historyId = nextId;
      }
      displayedId = nextId;
    },
  };
  return view;
}

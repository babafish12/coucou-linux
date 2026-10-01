import "../codex/activity.css";
import { CodexActivity, codexActivityStatus, codexProjectName, type ActivityEntry } from "../codex/activity";
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
  const sessions = h("nav", { class: "codex-session-list", "aria-label": "Local Codex sessions" });
  const sidebar = h("aside", { class: "codex-sidebar" }, h("span", { class: "codex-sidebar-label", text: "Local sessions" }), sessions);
  const title = h("b", { class: "codex-session-title" });
  const project = h("span", { class: "codex-project" });
  const monitorNotice = h("div", { class: "codex-notice", role: "status" });
  const history = h("div", { class: "codex-history", role: "region", "aria-label": "Codex activity", tabindex: "0" });
  const conversation = h("section", { class: "codex-conversation" },
    h("div", { class: "codex-session-heading" }, title, project), monitorNotice, history, focusNotice);
  const el = h("div", { class: "view codex-view" }, h("div", { class: "card codex-card" },
    h("header", { class: "codex-head" }, back, dot("#dce4e0", 6), h("b", { text: "Codex" }), state, focus),
    h("div", { class: "codex-workspace" }, sidebar, conversation),
  ));
  let displayedId: string | null = null;
  let listKey = "";
  let historyKey = "";
  const positions = new Map<string, { top: number; bottom: boolean; entryId?: string; offset?: number }>();
  const expandedEntries = new Map<string, Set<string>>();

  return {
    el,
    sync() {
      const selected = CodexActivity.selected;
      const nextId = selected?.id ?? null;
      const changingSession = displayedId !== nextId;
      const atBottom = history.scrollHeight - history.scrollTop - history.clientHeight < 32;
      if (displayedId) {
        const firstVisible = [...history.querySelectorAll<HTMLElement>(".codex-entry")]
          .find((row) => row.offsetTop + row.offsetHeight > history.scrollTop);
        positions.set(displayedId, {
          top: history.scrollTop, bottom: atBottom,
          entryId: firstVisible?.dataset.entryId,
          offset: firstVisible ? firstVisible.offsetTop - history.scrollTop : undefined,
        });
        const expanded = new Set<string>();
        for (const detail of history.querySelectorAll<HTMLDetailsElement>("details[open]")) {
          if (detail.dataset.entryId) expanded.add(detail.dataset.entryId);
        }
        expandedEntries.set(displayedId, expanded);
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

      const nextListKey = JSON.stringify([CodexActivity.sessions.map((session) => [session.id, session.cwd, session.title, session.state]), nextId, CodexActivity.loaded]);
      if (nextListKey !== listKey) {
        listKey = nextListKey;
        const scrollTop = sessions.scrollTop;
        sessions.replaceChildren();
        for (const session of CodexActivity.sessions) {
          const button = h("button", {
            class: "codex-session", type: "button", title: `${session.title}\n${session.cwd}`,
            "aria-current": session.id === nextId ? "true" : "false",
            onclick: () => { CodexActivity.select(session.id); actions.blip(); },
          }, h("b", { text: session.title || codexProjectName(session.cwd) }),
          h("span", { text: `${codexProjectName(session.cwd)} · ${codexActivityStatus(session.state)}` }));
          button.classList.toggle("selected", session.id === nextId);
          sessions.append(button);
        }
        if (!CodexActivity.sessions.length) sessions.append(h("p", { class: "codex-empty", text: CodexActivity.loaded ? "No sessions yet" : "Loading sessions…" }));
        sessions.scrollTop = scrollTop;
      }

      const nextHistoryKey = JSON.stringify([nextId, selected?.entries, CodexActivity.loaded, CodexActivity.error]);
      if (nextHistoryKey !== historyKey) {
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
        if (changingSession ? !position || position.bottom : firstRender || atBottom) history.scrollTop = history.scrollHeight;
        else {
          const anchor = [...history.querySelectorAll<HTMLElement>(".codex-entry")]
            .find((row) => row.dataset.entryId === position?.entryId);
          history.scrollTop = anchor && position?.offset != null
            ? anchor.offsetTop - position.offset : position?.top ?? history.scrollTop;
        }
      }
      displayedId = nextId;
    },
  };
}

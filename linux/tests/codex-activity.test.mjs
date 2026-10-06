import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

let instance = 0;
const entry = (id, timestamp = 1, text = `Update ${id}`, detail) => ({ id, timestamp, kind: "message", text, detail });
const session = (id, updatedAt = 1, entries = [entry(`${id}-1`, updatedAt)]) => ({ id, updatedAt, cwd: `/work/${id}`, title: `Task ${id}`, state: "working", entries });

async function setup() {
  assert.ok(process.env.CODEX_ACTIVITY_TEST_FIXTURE, "Run node scripts/test-codex-activity.mjs");
  const app = await import(`${pathToFileURL(process.env.CODEX_ACTIVITY_TEST_FIXTURE).href}?instance=${++instance}`);
  let notifications = 0;
  app.State.subscribe(() => notifications++);
  return { ...app, notifications: () => notifications };
}

test("loads the latest session and orders the public timeline chronologically", async () => {
  const { CodexActivity: activity } = await setup();
  activity.apply({ sessions: [session("old", 4), session("new", 10, [entry("late", 9), entry("early", 6)])] });
  assert.equal(activity.loaded, true);
  assert.equal(activity.selectedId, "new");
  assert.equal(activity.latest.id, "new");
  assert.deepEqual(activity.selected.entries.map((entry) => entry.id), ["early", "late"]);
  assert.equal(activity.latestEntry.id, "late");
});

test("equivalent snapshots and repeated selections cause no render notifications", async () => {
  const app = await setup();
  const first = session("first", 2);
  const second = session("second", 4);
  app.CodexActivity.apply({ sessions: [first, second] });
  app.CodexActivity.apply({ sessions: [structuredClone(second), structuredClone(first)] });
  app.CodexActivity.select("second");
  app.CodexActivity.select("missing");
  assert.equal(app.notifications(), 1);
});

test("retains the session being read while a different session becomes active", async () => {
  const { CodexActivity: activity } = await setup();
  activity.apply({ sessions: [session("first", 3), session("second", 1)] });
  activity.select("second");
  activity.apply({ sessions: [session("first", 9), session("second", 1), session("third", 12)] });
  assert.equal(activity.selected.id, "second");
  assert.equal(activity.latest.id, "third");
  assert.equal(activity.latestEntry.id, "third-1");
});

test("a recently finished session does not displace another session that is still working", async () => {
  const { CodexActivity: activity } = await setup();
  activity.apply({ sessions: [
    { ...session("done", 30), state: "finished" },
    { ...session("thinking", 10), state: "thinking" },
    session("working", 20),
  ] });
  assert.deepEqual(activity.sessions.map((session) => session.id), ["working", "thinking", "done"]);
  assert.equal(activity.latest.id, "working");
  assert.equal(activity.latestEntry.id, "working-1");
  activity.apply({ sessions: [
    { ...session("done", 30), state: "finished" },
    { ...session("thinking", 10), state: "thinking" },
    { ...session("working", 40), state: "finished" },
  ] });
  assert.equal(activity.latest.id, "thinking");
  assert.equal(activity.selected.id, "working", "the session being read remains selected after completion");
});

test("removing the selected session falls back to the latest retained session", async () => {
  const { CodexActivity: activity } = await setup();
  activity.apply({ sessions: [session("first", 3), session("second", 1)] });
  activity.apply({ sessions: [session("second", 1)] });
  assert.equal(activity.selected.id, "second");
  activity.apply({ sessions: [] });
  assert.equal(activity.selectedId, null);
  assert.equal(activity.selected, null);
  assert.equal(activity.latestEntry, null);
});

test("preserves retained entries and owns a copy of each snapshot", async () => {
  const { CodexActivity: activity } = await setup();
  const original = session("first", 3, [entry("one"), entry("two", 2)]);
  activity.apply({ sessions: [original] });
  original.entries[0].text = "Changed outside the store";
  original.entries.push(entry("three", 3));
  assert.equal(activity.selected.entries.length, 2);
  assert.equal(activity.selected.entries[0].text, "Update one");
  activity.apply({ sessions: [original] });
  assert.deepEqual(activity.selected.entries.map((entry) => entry.id), ["one", "two", "three"]);
});

test("real changes notify and a successful snapshot clears an error without losing selection", async () => {
  const app = await setup();
  app.CodexActivity.apply({ sessions: [session("first"), session("second", 2)] });
  app.CodexActivity.select("first");
  app.CodexActivity.setError("Monitor unavailable");
  app.CodexActivity.setError("Monitor unavailable");
  assert.equal(app.notifications(), 3);
  app.CodexActivity.apply({ sessions: [session("first"), session("second", 2)] });
  assert.equal(app.CodexActivity.error, "");
  assert.equal(app.CodexActivity.selectedId, "first");
  assert.equal(app.notifications(), 4);
  app.CodexActivity.apply({ sessions: [{ ...session("first"), state: "finished" }, session("second", 2)] });
  assert.equal(app.notifications(), 5);
});

class Element {
  constructor(tag) {
    this.tagName = tag;
    this.children = [];
    this.className = this.textContent = "";
    this.dataset = {};
    this.listeners = new Map();
    this.scrollTop = 0;
    this.clientHeight = 100;
    this.scrollHeight = 600;
    this.classList = { toggle: (name, on) => {
      const values = new Set(this.className.split(" ").filter(Boolean));
      if (on) values.add(name); else values.delete(name);
      this.className = [...values].join(" ");
    } };
  }
  setAttribute(name, value) { this[name === "class" ? "className" : name] = value; }
  get offsetTop() { return (this.parent?.children.indexOf(this) ?? 0) * 100; }
  get offsetHeight() { return 100; }
  append(...children) { for (const child of children) child.parent = this; this.children.push(...children); }
  replaceChildren(...children) { this.children = []; this.append(...children); }
  addEventListener(name, callback) { this.listeners.set(name, callback); }
  click() { this.listeners.get("click")?.({}); }
  querySelectorAll(selector) {
    const found = [];
    for (const child of this.children) {
      const matches = selector === "details[open]" ? child.tagName === "details" && child.open
        : selector.startsWith(".") ? child.className.split(" ").includes(selector.slice(1)) : child.tagName === selector;
      if (matches) found.push(child);
      found.push(...child.querySelectorAll(selector));
    }
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}

async function setupView(t, options = {}) {
  const previous = globalThis.document;
  globalThis.document = {
    createElement: (tag) => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
    createTextNode: (text) => Object.assign(new Element("text"), { textContent: text }),
  };
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; });
  const app = await setup();
  const focusCalls = [];
  const openCalls = [];
  const views = [];
  const view = app.buildCodex({
    setView: (value) => views.push(value), blip() {},
    focusCodex: options.focusCodex ?? (async (cwd) => { focusCalls.push(cwd); }),
    openSession: options.openSession ?? (async (id) => { openCalls.push(id); }),
  });
  return { ...app, view, focusCalls, openCalls, views, find: (selector) => view.el.querySelector(selector),
    showActivity: () => view.el.querySelectorAll(".codex-mode")[1].click(),
    showCurrent: () => view.el.querySelectorAll(".codex-mode")[0].click(),
  };
}

test("timeline renders log text literally and preserves scroll and expanded tool details", async (t) => {
  const app = await setupView(t);
  app.showActivity();
  const entries = [entry("one", 1, "<img src=x onerror=alert(1)>", "printf '<text>'")];
  app.CodexActivity.apply({ sessions: [session("first", 1, entries)] });
  app.view.sync();
  const history = app.find(".codex-history");
  assert.equal(app.find(".codex-entry-text").textContent, entries[0].text);
  assert.equal(app.find("img"), null);
  history.scrollTop = 120;
  history.querySelector("details").open = true;
  app.CodexActivity.apply({ sessions: [session("first", 2, [...entries, entry("two", 2)])] });
  app.view.sync();
  assert.equal(history.scrollTop, 120);
  assert.equal(history.querySelector("details").open, true);
  const firstRow = history.children[0];
  app.view.sync();
  assert.equal(history.children[0], firstRow, "unchanged state must not rebuild selectable text");
});

test("timeline follows new entries at the bottom and restores each session's reading position", async (t) => {
  const app = await setupView(t);
  app.showActivity();
  app.CodexActivity.apply({ sessions: [session("first", 2), session("second", 1)] });
  app.view.sync();
  const history = app.find(".codex-history");
  assert.equal(history.scrollTop, 600);
  history.scrollTop = 500;
  app.CodexActivity.apply({ sessions: [session("first", 3, [entry("first-1", 2), entry("first-2", 3)]), session("second", 1)] });
  app.view.sync();
  assert.equal(history.scrollTop, 600);
  history.scrollTop = 180;
  app.CodexActivity.select("second");
  app.view.sync();
  assert.equal(history.scrollTop, 600);
  app.CodexActivity.select("first");
  app.view.sync();
  assert.equal(history.scrollTop, 180);
});

test("session buttons select the actual session and Focus Codex uses its working directory", async (t) => {
  const app = await setupView(t);
  app.CodexActivity.apply({ sessions: [session("first", 2), session("second", 1)] });
  app.view.sync();
  app.view.el.querySelectorAll(".codex-session")[1].click();
  app.view.sync();
  assert.equal(app.CodexActivity.selectedId, "second");
  app.find(".codex-focus").click();
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(app.focusCalls, ["/work/second"]);
  app.find(".codex-back").click();
  assert.deepEqual(app.views, ["overview"]);
});

test("retention keeps the same visible entry when old timeline rows are removed", async (t) => {
  const app = await setupView(t);
  app.showActivity();
  const entries = Array.from({ length: 6 }, (_, index) => entry(String(index), index));
  app.CodexActivity.apply({ sessions: [session("first", 6, entries)] });
  app.view.sync();
  const history = app.find(".codex-history");
  history.scrollTop = 120;
  app.CodexActivity.apply({ sessions: [session("first", 7, [...entries.slice(1), entry("6", 6)])] });
  app.view.sync();
  assert.equal(history.scrollTop, 20, "the reader stays 20px into entry 1 after entry 0 is pruned");
});

test("focus failures stay visible and keep the focus action usable", async (t) => {
  const app = await setupView(t, { focusCodex: async () => { throw new Error("No Codex window found"); } });
  app.find(".codex-focus").click();
  assert.equal(app.find(".codex-focus").disabled, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(app.find(".codex-focus").disabled, false);
  assert.ok(app.view.el.querySelectorAll(".codex-notice").some((node) => node.textContent === "No Codex window found"));
});

test("loading, empty monitor, and failure states provide distinct instructions", async (t) => {
  const app = await setupView(t);
  app.showActivity();
  app.view.sync();
  assert.equal(app.find(".codex-history").children[0].textContent, "Loading local Codex activity…");
  app.CodexActivity.apply({ sessions: [] });
  app.view.sync();
  assert.match(app.find(".codex-history").children[0].textContent, /Start a local Codex conversation/);
  app.CodexActivity.setError("Could not read the local session cache");
  app.view.sync();
  assert.match(app.find(".codex-history").children[0].textContent, /Activity is unavailable/);
});

test("current work separates the latest progress from actions and past turns", async (t) => {
  const app = await setupView(t);
  const entries = [entry("old", 1, "The earlier change is finished"),
    { ...entry("start", 2, "Started working"), kind: "status" },
    entry("progress", 3, "Checking the missing notification sender"),
    { ...entry("tool", 4, "Run notification tests"), kind: "tool" }];
  app.CodexActivity.apply({ sessions: [session("first", 4, entries)] });
  app.view.sync();
  assert.equal(app.find(".codex-history").hidden, true);
  assert.equal(app.find(".codex-summary-text").textContent, entries[2].text);
  assert.equal(app.find(".codex-current-action").querySelector("b").textContent, "Current action");
  app.CodexActivity.apply({ sessions: [session("first", 5, [...entries,
    { ...entry("next-turn", 5, "Started working"), kind: "status" }])] });
  app.view.sync();
  assert.match(app.find(".codex-summary-text").textContent, /next progress update/);
  assert.equal(app.find(".codex-current-action"), null);
});

test("completed sessions retain their result and group below active chats", async (t) => {
  const app = await setupView(t);
  app.CodexActivity.apply({ sessions: [
    { ...session("done", 10, [entry("result", 9, "Notification sender names are now shown"),
      { ...entry("finish", 10, "Finished"), kind: "status" }]), state: "finished" },
    session("active", 4),
  ] });
  app.CodexActivity.select("done");
  app.view.sync();
  assert.equal(app.find(".codex-summary").querySelector("b").textContent, "Result");
  assert.equal(app.find(".codex-summary-text").textContent, "Notification sender names are now shown");
  assert.deepEqual(app.view.el.querySelectorAll(".codex-session-group").map((node) => node.textContent), ["Working now", "Recent"]);
});

test("opening a chat targets the selected session and disables duplicate requests", async (t) => {
  const app = await setupView(t);
  app.CodexActivity.apply({ sessions: [session("first", 2), session("second", 1)] });
  app.CodexActivity.select("second");
  app.view.sync();
  app.find(".codex-open").click();
  assert.equal(app.find(".codex-open").disabled, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(app.openCalls, ["second"]);
  assert.equal(app.find(".codex-open").disabled, false);
  app.CodexActivity.apply({ sessions: [] });
  app.view.sync();
  assert.equal(app.find(".codex-open").disabled, true);
});

test("chat navigation failures stay visible without blocking subsequent attempts", async (t) => {
  const app = await setupView(t, { openSession: async () => { throw new Error("Chat could not be opened"); } });
  app.CodexActivity.apply({ sessions: [session("first")] });
  app.view.sync();
  app.find(".codex-open").click();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(app.find(".codex-open").disabled, false);
  assert.ok(app.view.el.querySelectorAll(".codex-notice").some((node) => node.textContent === "Chat could not be opened"));
});

test("switching between current work and activity preserves the reader's position", async (t) => {
  const app = await setupView(t);
  const entries = Array.from({ length: 6 }, (_, index) => entry(String(index), index));
  app.CodexActivity.apply({ sessions: [session("first", 6, entries)] });
  app.showActivity();
  const history = app.find(".codex-history");
  history.scrollTop = 120;
  app.showCurrent();
  app.CodexActivity.apply({ sessions: [session("first", 7, [...entries, entry("6", 6)])] });
  app.view.sync();
  app.showActivity();
  assert.equal(history.scrollTop, 120);
  assert.equal(history.hidden, false);
  assert.equal(app.find(".codex-current").hidden, true);
});

test("new tool actions preserve an expanded progress update being read", async (t) => {
  const app = await setupView(t);
  const message = entry("progress", 1, "Checking notifications", "A longer public progress update.");
  app.CodexActivity.apply({ sessions: [session("first", 1, [message])] });
  app.view.sync();
  const current = app.find(".codex-current");
  current.querySelector("details").open = true;
  current.scrollTop = 80;
  app.CodexActivity.apply({ sessions: [session("first", 2, [message, { ...entry("tool", 2), kind: "tool" }])] });
  app.view.sync();
  assert.equal(current.querySelector("details").open, true);
  assert.equal(current.scrollTop, 80);
});

import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

let instance = 0;

/** Small DOM surface for the real Island constructor; no layout/paint emulation. */
class Element {
  constructor(tag) {
    this.tagName = tag;
    this.children = [];
    this.parentNode = null;
    this.className = this.id = this.textContent = this.value = "";
    this.style = { setProperty(name, value) { this[name] = value; } };
    this.dataset = {};
    this.listeners = new Map();
    this.scrollTop = 0;
    this.scrollHeight = this.clientHeight = 100;
    this.clientWidth = 720;
    this.classList = {
      contains: (name) => this.className.split(" ").includes(name),
      toggle: (name, force) => {
        const classes = new Set(this.className.split(" ").filter(Boolean));
        const on = force ?? !classes.has(name);
        if (on) classes.add(name); else classes.delete(name);
        this.className = [...classes].join(" ");
        return on;
      },
      add: (name) => this.classList.toggle(name, true),
      remove: (name) => this.classList.toggle(name, false),
    };
  }
  get firstChild() { return this.children[0] ?? null; }
  get isConnected() { return this.id === "root" || !!this.parentNode?.isConnected; }
  setAttribute(name, value) {
    if (name === "style") this.style.cssText = value;
    else this[name === "class" ? "className" : name] = value;
  }
  append(...children) {
    for (const child of children) {
      child.parentNode?.removeChild(child);
      child.parentNode = this;
      this.children.push(child);
    }
  }
  removeChild(child) {
    this.children.splice(this.children.indexOf(child), 1);
    child.parentNode = null;
  }
  replaceChildren(...children) {
    for (const child of [...this.children]) this.removeChild(child);
    this.append(...children);
  }
  querySelector(selector) {
    for (const child of this.children) {
      const matches = selector.startsWith(".") ? child.classList.contains(selector.slice(1))
        : selector.startsWith("#") ? child.id === selector.slice(1) : child.tagName === selector;
      if (matches) return child;
      const nested = child.querySelector(selector);
      if (nested) return nested;
    }
    return null;
  }
  addEventListener(name, callback) {
    const listeners = this.listeners.get(name) ?? [];
    listeners.push(callback);
    this.listeners.set(name, listeners);
  }
  dispatch(name, event = {}) { for (const callback of this.listeners.get(name) ?? []) callback(event); }
  focus() { this.focused = true; }
  select() { this.selected = true; }
  animate() {}
  getContext() { return { canvas: this, setTransform() {}, clearRect() {} }; }
}

export async function setupIsland(t, { integrations = [], autoClose = 30 } = {}) {
  assert.ok(process.env.ISLAND_TEST_FIXTURE, "Run node scripts/test-island-performance.mjs");
  let now = 10_000;
  let id = 0;
  const timers = new Map();
  const frames = new Map();
  const draws = [];
  const globals = ["window", "document", "performance", "setTimeout", "clearTimeout", "requestAnimationFrame", "matchMedia"];
  const previous = new Map(globals.map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const timeout = (callback, delay = 0) => {
    timers.set(++id, { at: now + delay, callback });
    return id;
  };
  const root = new Element("div");
  root.id = "root";
  Object.defineProperty(globalThis, "performance", { configurable: true, value: { now: () => now } });
  globalThis.setTimeout = timeout;
  globalThis.clearTimeout = (timer) => timers.delete(timer);
  globalThis.requestAnimationFrame = (callback) => {
    frames.set(++id, { at: now + 1000 / 60, callback });
    return id;
  };
  globalThis.matchMedia = () => ({ matches: false });
  globalThis.window = { setTimeout: timeout, clearTimeout, devicePixelRatio: 1, addEventListener() {} };
  globalThis.document = {
    createElement: (tag) => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
    createTextNode: (text) => Object.assign(new Element("text"), { textContent: text }),
  };
  t.after(() => {
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  t.mock.method(Math, "random", () => 0.5);
  const app = await import(`${pathToFileURL(process.env.ISLAND_TEST_FIXTURE).href}?island=${++instance}`);
  t.mock.method(app.BotEngine.prototype, "draw", function (ctx) { draws.push({ engine: this, canvas: ctx.canvas }); });
  app.State.settings.activeIntegrations = integrations;
  app.State.settings.autoCloseInterval = autoClose;
  app.State.settings.soundEnabled = false;
  app.Sound.setEnabled(false);
  app.State.loadIntegrationTasks();
  const island = new app.Island(root);
  const focus = [];
  app.Bridge.focusWindow = async (on) => { focus.push(on); return null; };
  return {
    ...app, island, root, draws, focus, frames, timers,
    now: () => now,
    advance(milliseconds) {
      const end = now + milliseconds;
      let iterations = 0;
      while (true) {
        const next = [...timers].map(([key, value]) => ({ key, ...value, map: timers }))
          .concat([...frames].map(([key, value]) => ({ key, ...value, map: frames })))
          .filter((value) => value.at <= end)
          .sort((a, b) => a.at - b.at || a.key - b.key)[0];
        if (!next) break;
        assert.ok(++iterations < 20_000, "scheduler must not spin");
        now = next.at;
        next.map.delete(next.key);
        next.callback(now);
      }
      now = end;
    },
  };
}

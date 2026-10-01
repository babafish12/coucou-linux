import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

let instance = 0;
const task = (id) => ({ id, color: "#2481B5", state: "idle" });

async function setup(t) {
  const fixture = process.env.ISLAND_TEST_FIXTURE;
  assert.ok(fixture, "Run with the island performance test runner");
  const controller = await import(`${pathToFileURL(fixture).href}?minibots=${++instance}`);
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const calls = { updates: [], draws: [], contexts: [], clears: [] };
  globalThis.window = { devicePixelRatio: 2 };
  globalThis.document = {
    createElement(tag) {
      const element = { style: {}, children: [], isConnected: false };
      element.append = (child) => { element.children.push(child); };
      if (tag === "canvas") {
        const context = {
          canvas: element,
          setTransform() {},
          clearRect() { calls.clears.push(element); },
        };
        element.getContext = (kind) => {
          assert.equal(kind, "2d");
          calls.contexts.push(element);
          return context;
        };
      }
      return element;
    },
  };
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
    if (previousDocument === undefined) delete globalThis.document;
    else globalThis.document = previousDocument;
  });
  t.mock.method(controller.BotEngine.prototype, "update", function (dt) {
    calls.updates.push({ engine: this, dt });
  });
  t.mock.method(controller.BotEngine.prototype, "draw", function (context, width, height) {
    calls.draws.push({ engine: this, canvas: context.canvas, width, height });
  });
  return {
    ...controller,
    calls,
    create(id, group) {
      const slot = controller.createMiniBot(task(id), group === "compact" ? 13 : 24, group);
      const canvas = slot.children[0];
      canvas.isConnected = true;
      return canvas;
    },
    resetCalls() { for (const values of Object.values(calls)) values.length = 0; },
  };
}

test("mini bots update and draw only the visible groups, including both during a fade", async (t) => {
  const app = await setup(t);
  const compact = Array.from({ length: 4 }, (_, index) => app.create(`compact-${index}`, "compact"));
  const overview = Array.from({ length: 4 }, (_, index) => app.create(`overview-${index}`, "overview"));
  assert.equal(app.miniBotCount(), 8);

  for (const [groups, canvases] of [
    [["compact"], compact],
    [["overview"], overview],
    [["compact", "overview"], [...compact, ...overview]],
    [[], []],
  ]) {
    app.resetCalls();
    assert.equal(app.tickMiniBots(1 / 60, new Set(groups)), canvases.length > 0);
    assert.deepEqual(app.calls.contexts, canvases);
    assert.deepEqual(app.calls.clears, canvases);
    assert.deepEqual(app.calls.draws.map((draw) => draw.canvas), canvases);
    assert.equal(app.calls.updates.length, canvases.length);
    assert.ok(app.calls.updates.every((update) => update.dt === 1 / 60));
  }
});

test("hidden mini bots retain state changes for the next visible frame", async (t) => {
  const app = await setup(t);
  const compact = app.create("same-task", "compact");
  const overview = app.create("same-task", "overview");
  app.syncMiniBotStates([{ ...task("same-task"), state: "sleeping", color: "#00FF80" }]);
  assert.equal(app.tickMiniBots(1 / 60, new Set()), false);
  assert.equal(app.calls.updates.length, 0);
  assert.equal(app.calls.draws.length, 0);

  app.tickMiniBots(1 / 60, new Set(["overview"]));
  assert.deepEqual(app.calls.draws.map((draw) => draw.canvas), [overview]);
  assert.equal(app.calls.draws[0].engine.state, "sleeping");
  assert.deepEqual(app.calls.draws[0].engine.bodyColor, [0, 1, 128 / 255]);
  app.resetCalls();
  app.tickMiniBots(1 / 60, new Set(["compact"]));
  assert.deepEqual(app.calls.draws.map((draw) => draw.canvas), [compact]);
  assert.equal(app.calls.draws[0].engine.state, "sleeping");
});

test("detached mini bots stop drawing immediately and pruning releases them", async (t) => {
  const app = await setup(t);
  const compact = app.create("compact", "compact");
  const overview = app.create("overview", "overview");
  compact.isConnected = false;
  assert.equal(app.tickMiniBots(1 / 60, new Set(["compact"])), false);
  assert.equal(app.calls.updates.length, 0);
  assert.equal(app.calls.contexts.length, 0);
  app.pruneMiniBots();
  assert.equal(app.miniBotCount(), 1);
  assert.equal(app.tickMiniBots(1 / 60, new Set(["compact", "overview"])), true);
  assert.deepEqual(app.calls.draws.map((draw) => draw.canvas), [overview]);
  overview.isConnected = false;
  app.pruneMiniBots();
  assert.equal(app.miniBotCount(), 0);
});

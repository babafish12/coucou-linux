import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

let instance = 0;

async function setup(t) {
  let now = 10_000;
  let nextTimer = 0;
  const timers = new Map();
  t.mock.method(performance, "now", () => now);
  t.mock.method(Math, "random", () => 0.5);
  t.mock.method(globalThis, "setTimeout", (callback, delay = 0) => {
    const id = ++nextTimer;
    timers.set(id, { callback, at: now + delay });
    return id;
  });
  t.mock.method(globalThis, "clearTimeout", (id) => timers.delete(id));

  const fixture = process.env.ISLAND_TEST_FIXTURE;
  assert.ok(fixture, "Run with node scripts/test-island-performance.mjs");
  const { BotEngine } = await import(`${pathToFileURL(fixture).href}?mochi=${++instance}`);

  function advance(milliseconds) {
    const end = now + milliseconds;
    while (true) {
      const next = [...timers].filter(([, timer]) => timer.at <= end)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      now = next[1].at;
      timers.delete(next[0]);
      next[1].callback();
    }
    now = end;
  }

  return {
    BotEngine,
    get now() { return now; },
    advance,
    frames(bot, milliseconds) {
      for (let remaining = milliseconds; remaining > 0;) {
        const dt = Math.min(10, remaining);
        advance(dt);
        bot.update(dt / 1000);
        remaining -= dt;
      }
    },
  };
}

test("a quiet bot sleeps until its blink deadline and then settles again", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  let wakes = 0;
  bot.onNeedsFrame = () => wakes++;
  assert.equal(bot.busy, false);
  const blinkAt = bot.nextWakeAt;
  assert.ok(blinkAt > clock.now);

  clock.advance(blinkAt - clock.now);
  bot.update(1 / 60);
  assert.equal(bot.busy, true);
  assert.ok(wakes > 0);
  clock.frames(bot, 250);
  assert.equal(bot.open, 1);
  assert.equal(bot.busy, false);
  assert.ok(bot.nextWakeAt > clock.now);
});

test("temporary eyes expire on their exact wake deadline without keeping RAF busy", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  let wakes = 0;
  bot.onNeedsFrame = () => wakes++;
  bot.triggerEmote("annoyed");
  assert.ok(wakes > 0);
  assert.equal(bot.busy, false);
  assert.equal(bot.nextWakeAt, clock.now + 800);
  clock.advance(800);
  bot.update(0);
  assert.equal(bot.eyeOverride, null);
  assert.ok(bot.nextWakeAt > clock.now);

  bot.setPermanentEmote("happy");
  assert.equal(bot.eyeOverrideUntil, Infinity);
  assert.equal(bot.busy, false);
  assert.ok(Number.isFinite(bot.nextWakeAt));
});

test("working dots remain animated after their entry tween and stop when hidden by morph", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  bot.setState("working");
  clock.frames(bot, 1_400);
  assert.equal(bot.badge.kind, "dots");
  assert.equal(bot.badgeS, 1);
  assert.equal(bot.busy, true);

  bot.morph = 1;
  assert.equal(bot.busy, false);
  bot.morph = 0;
  assert.equal(bot.busy, true);
  bot.badgeS = 0;
  assert.equal(bot.busy, false);
});

test("moving eyes and an active wave keep drawing without making idle permanent", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  bot.eyeOverride = "spiral";
  bot.eyeOverrideUntil = Infinity;
  assert.equal(bot.busy, true);
  bot.eyeOverride = "star";
  assert.equal(bot.busy, true);
  bot.morph = 1;
  bot.isChewing = true;
  assert.equal(bot.busy, false, "the box's static chewing eyes cover the rotating stars");
  bot.morph = 0;
  bot.isChewing = false;
  bot.eyeOverride = null;
  assert.equal(bot.busy, false);

  bot.waveStart = (clock.now + 400) / 1000;
  bot.waveUntil = (clock.now + 900) / 1000;
  assert.equal(bot.busy, false);
  assert.equal(bot.nextWakeAt, clock.now + 400);
  clock.advance(400);
  assert.equal(bot.busy, true);
  clock.advance(500);
  assert.equal(bot.busy, false);

  bot.setState("dizzy");
  clock.frames(bot, 1_800);
  assert.equal(bot.roll, 0);
  assert.equal(bot.busy, true);
});

test("input effects and delayed badge, particle and chewing changes wake their owner", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  let wakes = 0;
  bot.onNeedsFrame = () => wakes++;

  bot.slap();
  assert.ok(wakes > 0);
  wakes = 0;
  bot.triggerEmote("love");
  assert.ok(wakes > 0);
  wakes = 0;
  bot.emit("heart", 1);
  assert.ok(wakes > 0);

  bot.setState("working");
  wakes = 0;
  clock.advance(100);
  assert.ok(wakes > 0, "the delayed badge appearance must wake a stopped loop");
  bot.setBadge(null);
  wakes = 0;
  clock.advance(100);
  assert.ok(wakes > 0, "removing a badge also changes the drawing");

  bot.gulp();
  wakes = 0;
  clock.advance(460);
  assert.equal(bot.isChewing, true);
  assert.ok(wakes > 0);
  wakes = 0;
  clock.advance(800);
  assert.equal(bot.isChewing, false);
  assert.ok(wakes > 0);
});

test("the greeting's delayed direct eye change wakes after its last hand tween", async (t) => {
  const clock = await setup(t);
  const bot = new clock.BotEngine();
  let wakes = 0;
  bot.onNeedsFrame = () => wakes++;
  bot.greet();
  clock.advance(1_749);
  wakes = 0;
  clock.advance(1);
  assert.ok(wakes > 0);
  assert.equal(bot.eyeOverride, "happy");
  assert.equal(bot.eyeOverrideUntil * 1000, clock.now + 300);
});

test("repeated drawing reuses one body path while colors, size and morph remain live", async (t) => {
  const clock = await setup(t);
  let paths = 0;
  class CanvasPath {
    constructor() { paths++; }
    moveTo() {}
    lineTo() {}
    closePath() {}
  }
  const originalPath = Object.getOwnPropertyDescriptor(globalThis, "Path2D");
  Object.defineProperty(globalThis, "Path2D", { configurable: true, value: CanvasPath });
  t.after(() => {
    if (originalPath) Object.defineProperty(globalThis, "Path2D", originalPath);
    else delete globalThis.Path2D;
  });
  const fills = [];
  const context = new Proxy({
    fill(path) { if (path instanceof CanvasPath) fills.push({ path, color: this.fillStyle }); },
  }, {
    get(target, key) { return key in target ? target[key] : () => {}; },
  });
  const bot = new clock.BotEngine();
  bot.bodyColor = [1, 0, 0];
  bot.draw(context, 100, 140);
  const first = fills.at(-1);
  bot.bodyColor = [0, 0.5, 1];
  bot.yaw = 0.3;
  bot.sx = 1.1;
  bot.draw(context, 100, 140);
  assert.equal(paths, 1);
  assert.equal(fills.at(-1).path, first.path);
  assert.notEqual(fills.at(-1).color, first.color);

  bot.draw(context, 120, 160);
  assert.equal(paths, 2);
  bot.morph = 0.5;
  bot.draw(context, 120, 160);
  assert.equal(paths, 3);
  bot.draw(context, 120, 160);
  assert.equal(paths, 3);
  bot.resetMorph();
  bot.draw(context, 100, 140);
  assert.equal(paths, 4, "returning to an old size replaces the single cache entry");
});

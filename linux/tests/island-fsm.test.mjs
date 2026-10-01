import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

async function setup(t) {
  const fixture = process.env.ISLAND_TEST_FIXTURE;
  assert.ok(fixture, "Run with the island test runner to set ISLAND_TEST_FIXTURE");
  const { IslandStateMachine } = await import(pathToFileURL(fixture).href);
  let now = 10_000;
  let nextTimer = 0;
  const timers = new Map();
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const previousPerformance = Object.getOwnPropertyDescriptor(globalThis, "performance");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      setTimeout(callback, delay) {
        const id = ++nextTimer;
        timers.set(id, { callback, time: now + delay });
        return id;
      },
      clearTimeout(id) { timers.delete(id); },
    },
  });
  Object.defineProperty(globalThis, "performance", {
    configurable: true,
    value: { now: () => now },
  });
  t.after(() => {
    if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
    else delete globalThis.window;
    if (previousPerformance) Object.defineProperty(globalThis, "performance", previousPerformance);
    else delete globalThis.performance;
  });
  const fsm = new IslandStateMachine();
  const deadlines = [];
  const transitions = [];
  fsm.onDeadlineChanged = () => deadlines.push(fsm.homeCollapseDeadline);
  fsm.onTransition = (from, to) => transitions.push({ from, to, deadline: fsm.homeCollapseDeadline });
  return {
    fsm,
    deadlines,
    transitions,
    timers,
    advance(milliseconds) {
      const end = now + milliseconds;
      while (true) {
        const next = [...timers].filter(([, timer]) => timer.time <= end)
          .sort((a, b) => a[1].time - b[1].time)[0];
        if (!next) break;
        now = next[1].time;
        timers.delete(next[0]);
        next[1].callback();
      }
      now = end;
    },
  };
}

test("every auto-close preset exposes its real deadline and clears it before expiry transitions", async (t) => {
  for (const seconds of [1, 3, 5, 10, 15, 30]) {
    await t.test(`${seconds} seconds`, async (t) => {
      const { fsm, deadlines, transitions, timers, advance } = await setup(t);
      fsm.homeToPetitDelay = seconds;
      fsm.forceHome();
      assert.equal(fsm.homeCollapseDeadline, null);
      fsm.mouseLeft();
      assert.equal(fsm.homeCollapseDeadline, 10_000 + seconds * 1000);
      assert.deepEqual(deadlines, [fsm.homeCollapseDeadline]);
      advance(seconds * 1000 - 1);
      assert.equal(fsm.state, "home");
      advance(1);
      assert.equal(fsm.state, "petit");
      assert.equal(fsm.homeCollapseDeadline, null);
      assert.deepEqual(deadlines, [10_000 + seconds * 1000, null]);
      assert.deepEqual(transitions.at(-1), { from: "home", to: "petit", deadline: null });
      assert.equal(timers.size, 0);
    });
  }
});

test("restarting replaces the timeout and publishes only a changed deadline", async (t) => {
  const { fsm, deadlines, timers, advance } = await setup(t);
  fsm.homeToPetitDelay = 3;
  fsm.forceHome();
  fsm.mouseLeft();
  fsm.mouseLeft();
  assert.deepEqual(deadlines, [13_000], "a restart at the same instant has the same deadline");
  assert.equal(timers.size, 1);
  advance(1000);
  fsm.mouseLeft();
  assert.equal(fsm.homeCollapseDeadline, 14_000);
  assert.deepEqual(deadlines, [13_000, 14_000]);
  assert.equal(timers.size, 1);
  advance(2000);
  assert.equal(fsm.state, "home", "the replaced timeout must not collapse Home");
  advance(1000);
  assert.equal(fsm.state, "petit");
  assert.deepEqual(deadlines, [13_000, 14_000, null]);
});

test("hover cancels the deadline and leaving starts a full new countdown", async (t) => {
  const { fsm, deadlines, timers, advance } = await setup(t);
  fsm.homeToPetitDelay = 5;
  fsm.forceHome();
  fsm.mouseLeft();
  advance(2000);
  fsm.mouseEntered();
  fsm.mouseEntered();
  assert.equal(fsm.homeCollapseDeadline, null);
  assert.equal(timers.size, 0);
  assert.deepEqual(deadlines, [15_000, null]);
  advance(10_000);
  assert.equal(fsm.state, "home");
  fsm.mouseLeft();
  assert.equal(fsm.homeCollapseDeadline, 27_000);
  advance(4999);
  assert.equal(fsm.state, "home");
  advance(1);
  assert.equal(fsm.state, "petit");
});

test("hover opens hidden and compact islands immediately without a collapse timer", async (t) => {
  for (const initial of ["hidden", "petit"]) {
    await t.test(initial, async (t) => {
      const { fsm, transitions, timers, advance } = await setup(t);
      if (initial === "petit") fsm.reveal();
      fsm.mouseEntered();
      assert.equal(fsm.state, "home");
      assert.deepEqual(transitions.at(-1), { from: initial, to: "home", deadline: null });
      assert.equal(timers.size, 0);
      advance(60_000);
      assert.equal(fsm.state, "home");
      fsm.mouseLeft();
      advance(fsm.homeToPetitDelay * 1000);
      assert.equal(fsm.state, "petit");
    });
  }
});

test("explicit cancellation and forced transitions cannot leave a stale home deadline", async (t) => {
  const actions = {
    cancelTimers: "home",
    forceHome: "home",
    forcePetit: "petit",
    forceHidden: "hidden",
    launch: "coucou",
  };
  for (const [action, expectedState] of Object.entries(actions)) {
    await t.test(action, async (t) => {
      const { fsm, deadlines, timers, advance } = await setup(t);
      fsm.homeToPetitDelay = 1;
      fsm.forceHome();
      fsm.mouseLeft();
      fsm[action]();
      assert.equal(fsm.homeCollapseDeadline, null);
      assert.equal(timers.size, 0);
      assert.deepEqual(deadlines, [11_000, null]);
      fsm.cancelTimers();
      assert.deepEqual(deadlines, [11_000, null], "cancelling twice must not repeat notifications");
      advance(5000);
      assert.equal(fsm.state, expectedState);
    });
  }
});

test("pinned Home never schedules a collapse and can resume after unpinning", async (t) => {
  const { fsm, deadlines, timers, advance } = await setup(t);
  fsm.homeToPetitDelay = 3;
  fsm.forceHome();
  fsm.pinned = true;
  fsm.mouseLeft();
  assert.equal(fsm.homeCollapseDeadline, null);
  assert.equal(timers.size, 0);
  assert.deepEqual(deadlines, []);
  advance(5000);
  assert.equal(fsm.state, "home");

  fsm.pinned = false;
  fsm.mouseLeft();
  assert.equal(fsm.homeCollapseDeadline, 18_000);
  fsm.pinned = true;
  fsm.mouseLeft();
  assert.equal(fsm.homeCollapseDeadline, null);
  assert.equal(timers.size, 0);
  assert.deepEqual(deadlines, [18_000, null]);
  advance(5000);
  assert.equal(fsm.state, "home");

  fsm.pinned = false;
  fsm.mouseLeft();
  advance(3000);
  assert.equal(fsm.state, "petit");
});

test("the greeting settles into a compact island that remains visible while idle", async (t) => {
  const { fsm, deadlines, timers, advance } = await setup(t);
  fsm.launch();
  fsm.greetComplete();
  advance(600);
  assert.equal(fsm.state, "petit");
  fsm.mouseLeft();
  advance(5 * 60_000);
  assert.equal(fsm.state, "petit");
  assert.equal(timers.size, 0);
  assert.equal(fsm.homeCollapseDeadline, null);
  assert.deepEqual(deadlines, []);
});

test("revealing or closing to compact never schedules automatic hiding", async (t) => {
  const { fsm, timers, advance } = await setup(t);
  fsm.reveal();
  advance(5 * 60_000);
  assert.equal(fsm.state, "petit");
  fsm.mouseEntered();
  fsm.mouseLeft();
  advance(5 * 60_000);
  assert.equal(fsm.state, "petit");
  fsm.click();
  assert.equal(fsm.state, "home");
  fsm.forcePetit();
  fsm.mouseLeft();
  advance(5 * 60_000);
  assert.equal(fsm.state, "petit");
  assert.equal(timers.size, 0);
  fsm.forceHidden();
  assert.equal(fsm.state, "hidden", "explicit pause can still hide the island");
});

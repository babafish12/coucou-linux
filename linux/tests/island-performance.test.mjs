import assert from "node:assert/strict";
import test from "node:test";
import { setupIsland } from "./helpers/island-fixture.mjs";

const integrations = ["integration_github", "integration_notion", "integration_vercel", "integration_stripe"];

test("auto-close keeps the compact island visible and clickable after a long idle", async (t) => {
  const app = await setupIsland(t, { autoClose: 3 });
  app.island.alert("overview");
  app.advance(123_000);
  assert.equal(app.State.mode, "compact");
  assert.equal(app.island.fsm.state, "petit");
  assert.ok(app.island.width.value > 0 && app.island.height.value > 0);
  assert.equal(app.island.botCanvas.style.opacity, "1");
  app.island.islandEl.dispatch("mousedown", { clientX: 360, clientY: 20 });
  app.advance(500);
  assert.equal(app.State.mode, "expanded");
});

test("hover opens the compact island immediately and auto-close starts only after leaving", async (t) => {
  const app = await setupIsland(t, { autoClose: 1 });
  app.island.reveal();
  app.advance(1000);
  app.island.onCursor(360, 20);
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  app.advance(3000);
  assert.equal(app.State.mode, "expanded", "stationary hover must keep the island open");
  assert.deepEqual(app.focus, [], "hover must not take keyboard focus");
  app.island.onCursor(1000, 500);
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 1000);
  app.advance(1500);
  assert.equal(app.State.mode, "compact");
  app.island.onCursor(360, 20);
  assert.equal(app.State.mode, "expanded", "hover must reopen after auto-close");
});

test("the hidden wake strip opens Home without a click or premature countdown", async (t) => {
  const app = await setupIsland(t, { autoClose: 1 });
  app.island.wakeStrip.dispatch("mouseenter");
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  app.advance(3000);
  assert.equal(app.State.mode, "expanded");
  assert.deepEqual(app.focus, []);
  app.island.onCursor(1000, 500);
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 1000);
});

test("explicit collapse remains compact until the pointer leaves and re-enters", async (t) => {
  const app = await setupIsland(t);
  app.island.reveal();
  app.advance(1000);
  app.island.onCursor(360, 20);
  app.advance(1000);
  app.island.collapse();
  app.island.onCursor(360, 20);
  app.advance(1000);
  assert.equal(app.State.mode, "compact");
  app.island.onCursor(1000, 500);
  app.island.onCursor(360, 20);
  assert.equal(app.State.mode, "expanded");
});

test("hidden changes stay dirty without drawing, then reveal uses the latest state", async (t) => {
  const app = await setupIsland(t);
  app.State.updateTask("integration_codex", "working");
  app.advance(5000);
  assert.equal(app.frames.size, 0);
  assert.equal(app.draws.length, 0);
  assert.equal(app.island.dirty, true);
  app.island.reveal();
  app.advance(1000);
  assert.equal(app.island.engine.state, "working");
  assert.ok(app.draws.length > 0);
  assert.equal(app.island.dirty, false);
  app.island.fsm.forceHidden();
  app.advance(1000);
  const count = app.draws.length;
  app.State.updateTask("integration_codex", "sleeping");
  app.island.engine.emit("heart", 1);
  app.advance(10_000);
  assert.equal(app.frames.size, 0);
  assert.equal(app.island.wakeTimer, null);
  assert.equal(app.draws.length, count);
});

test("a settled island wakes for an actual click, hover love and a scheduled blink", async (t) => {
  const app = await setupIsland(t);
  app.island.alert("empty");
  app.advance(2000);
  assert.equal(app.frames.size, 0);
  app.island.islandEl.dispatch("mousedown", {
    clientX: (720 - app.island.width.value) / 2 + app.island.botCx.value,
    clientY: app.island.botCy.value,
  });
  assert.equal(app.frames.size, 1);
  app.advance(1200);
  app.island.botHovering = true;
  app.island.scheduleLove();
  const before = app.draws.length;
  app.advance(2000);
  assert.ok(app.island.engine.particles.some((particle) => particle.type === "heart"));
  assert.ok(app.draws.length > before);
  app.island.botHovering = false;
  app.advance(2500);
  assert.equal(app.frames.size, 0);
  assert.ok(app.island.wakeTimer != null);
  const blinkAt = app.island.engine.nextWakeAt;
  app.advance(blinkAt - app.now() + 40);
  assert.ok(app.island.engine.open < 1, "idle blinking does not depend on cursor events");
});

test("mini draws follow compact and overview fades and stop in conversation views", async (t) => {
  const app = await setupIsland(t, { integrations });
  app.island.reveal();
  app.advance(1500);
  app.island.alert("overview");
  app.advance(1500);
  app.draws.length = 0;
  app.advance(17);
  assert.equal(app.draws.filter(({ engine }) => engine.isMini).length, 4);
  app.island.setView("telegram");
  app.advance(500);
  app.draws.length = 0;
  app.advance(1000);
  assert.equal(app.draws.filter(({ engine }) => engine.isMini).length, 0);
  assert.equal(app.island.views.get("overview").el.classList.contains("rendering"), false);
  assert.equal(app.island.views.get("telegram").el.classList.contains("rendering"), true);
  app.island.collapse();
  app.advance(700);
  app.draws.length = 0;
  app.advance(17);
  assert.equal(app.draws.filter(({ engine }) => engine.isMini).length, 4);
  app.island.fsm.forceHidden();
  app.advance(1000);
  assert.equal(app.island.islandEl.classList.contains("rendering"), false);
  assert.equal(app.frames.size, 0);
});

test("the covered Mochi tracks upload inputs without drawing its canvas", async (t) => {
  const app = await setupIsland(t);
  t.mock.method(app.Bridge, "log", async () => null);
  t.mock.method(app.island.uploadCanvas, "draw", () => {});
  app.island.alert("overview");
  app.advance(1000);
  app.island.onDragDrop({ type: "enter" });
  app.island.onCursor(1000, 200);
  app.draws.length = 0;
  app.advance(2000);
  assert.equal(app.draws.length, 0, "covered main canvas must not draw");
  assert.equal(app.island.engine.lookX, app.island.lookX());
  assert.ok(app.island.engine.yaw > 0.5, "gaze should already follow the cursor during upload");
  assert.equal(app.island.engine.slotHTarget, 0.2);
  app.island.onDragDrop({ type: "leave" });
  app.island.setView("overview");
  app.advance(600);
  assert.ok(app.draws.some(({ engine }) => engine === app.island.engine));
  assert.equal(app.island.engine.slotHTarget, 0);
});

test("notification countdowns reset, hover cancels, and Telegram keeps open without stale deadlines", async (t) => {
  const app = await setupIsland(t, { autoClose: 3 });
  app.TelegramNotifications.current = {
    id: "1", chat: { id: "chat", title: "Fixture" },
    message: { id: "1", text: "Hello", outgoing: false, date: 1 },
  };
  app.island.showTelegramNotification();
  app.advance(1600);
  assert.equal(app.State.view, "telegram-notification");
  assert.ok(parseFloat(app.island.countdown.style.width) > 0);
  assert.deepEqual(app.focus, [], "notification must not take keyboard focus");
  app.TelegramNotifications.current = { ...app.TelegramNotifications.current, id: "2" };
  app.island.showTelegramNotification();
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 3000);
  app.advance(1600);
  assert.equal(app.State.mode, "expanded", "old deadline must not collapse the second notification");
  app.island.onCursor(360, 100);
  app.advance(40);
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  assert.equal(app.island.countdown.style.width, "0px");
  app.island.onCursor(1000, 500);
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 3000);
  app.island.setView("telegram");
  app.advance(4000);
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.island.fsm.pinned, true);
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  assert.equal(app.island.countdown.style.width, "0px");
  app.island.setView("overview");
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 3000);
  app.advance(3500);
  assert.equal(app.State.mode, "compact");
});

test("the one-second preset wakes for its countdown and collapses at the FSM deadline", async (t) => {
  const app = await setupIsland(t, { autoClose: 1 });
  app.island.alert("empty");
  const deadline = app.island.fsm.homeCollapseDeadline;
  app.advance(450);
  assert.ok(parseFloat(app.island.countdown.style.width) > 0);
  app.advance(deadline - app.now());
  assert.equal(app.State.mode, "compact");
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  app.advance(500);
  assert.equal(app.island.countdown.style.width, "0px");
});

test("hiding a Telegram conversation releases its pin before the next notification", async (t) => {
  const app = await setupIsland(t, { autoClose: 3 });
  app.island.alert("telegram");
  app.advance(1500);
  assert.equal(app.island.fsm.pinned, true);
  app.island.fsm.forceHidden();
  app.advance(500);
  assert.equal(app.island.fsm.pinned, false);
  assert.equal(app.island.wakeTimer, null);
  app.TelegramNotifications.current = {
    id: "1", chat: { id: "chat", title: "Fixture" },
    message: { id: "1", text: "Hello", outgoing: false, date: 1 },
  };
  app.island.showTelegramNotification();
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 3000);
  app.advance(3500);
  assert.equal(app.State.mode, "compact");
});

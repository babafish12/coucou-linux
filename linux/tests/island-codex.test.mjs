import assert from "node:assert/strict";
import test from "node:test";
import { setupIsland } from "./helpers/island-fixture.mjs";

const session = {
  id: "local-session", cwd: "/projects/coucou", title: "Fix the launcher",
  state: "working", updatedAt: 1000,
  entries: [{ id: "1", kind: "message", text: "Checking the installed launcher", timestamp: 1000 }],
};

test("Codex card opens persistent activity and Home restores auto-close", async (t) => {
  const app = await setupIsland(t, { autoClose: 3 });
  app.CodexActivity.apply({ sessions: [session] });
  app.island.alert("overview");
  app.advance(500);
  const home = app.island.views.get("overview").el;
  assert.equal(home.querySelector(".codex-home-summary").textContent, session.entries[0].text);
  app.CodexActivity.apply({ sessions: [{ ...session, state: "finished", entries: [
    ...session.entries, { id: "2", kind: "status", text: "Finished", timestamp: 2000 },
  ] }] });
  app.advance(17);
  assert.equal(home.querySelector(".codex-home-summary").textContent, session.entries[0].text,
    "completion must keep the useful last update on Home");
  home.querySelector(".codex-home").dispatch("click");
  app.advance(4000);
  assert.equal(app.State.view, "codex");
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  assert.equal(app.island.countdown.style.width, "0px");
  assert.deepEqual(app.focus, [], "read-only activity must not steal keyboard focus");
  assert.equal(app.island.views.get("overview").el.inert, true);
  app.island.alert("codex");
  app.island.fsm.mouseLeft();
  app.advance(4000);
  assert.equal(app.island.fsm.pinned, true, "reopening activity must retain its pin");
  assert.equal(app.State.mode, "expanded");
  app.island.views.get("codex").el.querySelector(".codex-back").dispatch("click");
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 3000);
  app.advance(3500);
  assert.equal(app.State.mode, "compact");
});

test("Codex corner arrow focuses the latest desktop session and reports failure in the detail view", async (t) => {
  const app = await setupIsland(t);
  app.CodexActivity.apply({ sessions: [session] });
  const focused = [];
  t.mock.method(app.Bridge, "focusCodex", async (cwd) => { focused.push(cwd); });
  app.island.alert("overview");
  app.advance(500);
  const arrow = app.island.views.get("overview").el.querySelector(".jump");
  assert.equal(arrow.title, "Focus Codex window");
  arrow.dispatch("click");
  await Promise.resolve();
  assert.deepEqual(focused, [session.cwd]);
  assert.equal(app.State.view, "overview");
  t.mock.method(app.Bridge, "focusCodex", async () => { throw new Error("No Codex window is open"); });
  arrow.dispatch("click");
  await Promise.resolve();
  assert.equal(app.State.view, "codex");
  assert.match(app.CodexActivity.error, /No Codex window/);
});

test("unchanged Codex snapshots do not wake rendering", async (t) => {
  const app = await setupIsland(t);
  app.CodexActivity.apply({ sessions: [session] });
  app.island.alert("codex");
  app.advance(2000);
  assert.equal(app.frames.size, 0);
  app.CodexActivity.apply({ sessions: [session] });
  assert.equal(app.frames.size, 0);
  app.island.fsm.forceHidden();
  app.advance(1000);
  app.CodexActivity.apply({ sessions: [{ ...session, updatedAt: 2000, state: "finished" }] });
  app.advance(1000);
  assert.equal(app.frames.size, 0);
});

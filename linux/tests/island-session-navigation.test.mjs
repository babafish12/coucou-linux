import assert from "node:assert/strict";
import test from "node:test";
import { setupIsland } from "./helpers/island-fixture.mjs";

const sessionA = "01957a3b-832b-7000-9bdc-13d031ff6a42";
const sessionB = "01957a3b-832b-7000-9bdc-13d031ff6a43";

function sessionEvent(app, name, sessionId) {
  app.handleSession(app.island, {
    hook_event_name: name,
    session_id: sessionId,
    cwd: "/home/user/project",
  });
}

test("finished Codex sessions open the exact chat and release island focus", async (t) => {
  const app = await setupIsland(t);
  const opened = [];
  app.Bridge.openCodexSession = async (id) => { opened.push(id); };
  sessionEvent(app, "UserPromptSubmit", sessionA);
  sessionEvent(app, "Stop", sessionA);
  app.advance(400);
  assert.equal(app.State.focusTask.sessionId, sessionA);
  const button = app.island.views.get("finished").el.querySelector("button");
  assert.equal(button.querySelector("span").textContent, "Open chat");
  button.dispatch("click");
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(opened, [sessionA]);
  assert.equal(app.State.mode, "compact");
});

test("session navigation updates on switches and clears on session end", async (t) => {
  const app = await setupIsland(t);
  sessionEvent(app, "UserPromptSubmit", sessionA);
  sessionEvent(app, "Stop", sessionA);
  sessionEvent(app, "UserPromptSubmit", sessionB);
  assert.equal(app.State.focusTask.sessionId, sessionB);
  sessionEvent(app, "Stop", sessionB);
  app.advance(5300);
  assert.equal(app.State.focusTask.sessionId, sessionB, "completion timeout must retain the chat link");
  sessionEvent(app, "SessionEnd", sessionB);
  assert.equal(app.State.focusTask.sessionId, null);
});

test("a failed chat launch remains visible and offers a retry", async (t) => {
  const app = await setupIsland(t);
  app.Bridge.openCodexSession = async () => { throw new Error("No Codex link handler"); };
  sessionEvent(app, "UserPromptSubmit", sessionA);
  sessionEvent(app, "Stop", sessionA);
  app.advance(400);
  const view = app.island.views.get("finished");
  const button = view.el.querySelector("button");
  button.dispatch("click");
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(app.State.mode, "expanded");
  assert.equal(button.disabled, false);
  assert.equal(view.el.querySelector(".title").textContent, "No Codex link handler");
  view.sync();
  assert.equal(view.el.querySelector(".title").textContent, "No Codex link handler");
});

test("a delayed launch cannot collapse a different completed session", async (t) => {
  const app = await setupIsland(t);
  let resolveLaunch;
  app.Bridge.openCodexSession = () => new Promise((resolve) => { resolveLaunch = resolve; });
  sessionEvent(app, "UserPromptSubmit", sessionA);
  sessionEvent(app, "Stop", sessionA);
  app.advance(400);
  app.island.views.get("finished").el.querySelector("button").dispatch("click");
  sessionEvent(app, "UserPromptSubmit", sessionB);
  sessionEvent(app, "Stop", sessionB);
  resolveLaunch();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.State.focusTask.sessionId, sessionB);
});

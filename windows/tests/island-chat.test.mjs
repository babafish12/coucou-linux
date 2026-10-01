import assert from "node:assert/strict";
import test from "node:test";
import { setupIsland } from "./helpers/island-fixture.mjs";

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

async function setupChat(t, options = {}) {
  const app = await setupIsland(t, options);
  app.State.agentProvider = "codex";
  const requests = [];
  const decisions = [];
  const cancellations = [];
  app.Bridge.chatSend = (query, context, requestId, progress) => new Promise((resolve, reject) => {
    requests.push({ query, context, requestId, resolve, reject,
      emit: (event) => progress({ requestId, ...event }),
    });
  });
  app.Bridge.chatApprove = async (...args) => { decisions.push(args); };
  app.Bridge.chatCancel = async (requestId) => { cancellations.push(requestId); };
  app.island.setView("prompt");
  app.advance(500);
  const view = app.island.views.get("prompt");
  const find = (selector) => view.el.querySelector(selector);
  const input = find(".chat-input");
  const send = find(".send-btn");
  return {
    ...app, requests, decisions, cancellations, find, input, send,
    submit(query) {
      input.value = query;
      send.dispatch("click");
      app.advance(30);
      return requests.at(-1);
    },
  };
}

test("Codex tokens and request metadata render before the final reply arrives", async (t) => {
  const app = await setupChat(t);
  const request = app.submit("  Say hello  ");
  assert.equal(request.query, "Say hello");
  assert.equal(request.context, null);
  assert.ok(request.requestId);
  assert.equal(app.input.disabled, true);
  assert.equal(app.send.hidden, true);
  assert.equal(app.find(".chat-stop").hidden, false);
  assert.equal(app.find(".chat-status").textContent, "Connecting to Codex…");
  assert.ok(app.find(".typing"));

  request.emit({ kind: "delta", itemId: "answer", text: "Hello", model: "gpt-6-luna", reasoningEffort: "low" });
  app.advance(30);
  assert.equal(app.find(".reply-text").textContent, "Hello");
  assert.equal(app.find(".reply-config").textContent, "Requested: gpt-6-luna · low reasoning");
  assert.equal(app.find(".typing"), null);
  assert.equal(app.input.disabled, true, "tokens must appear while the request is still pending");
  request.emit({ kind: "delta", itemId: "answer", text: " there" });
  request.emit({ kind: "delta", itemId: "answer", text: "wrong request", requestId: "another-turn" });
  app.advance(30);
  assert.equal(app.find(".reply-text").textContent, "Hello there");
  request.emit({ kind: "message", itemId: "answer", text: "Hello there!" });
  app.advance(30);
  assert.equal(app.find(".reply-text").textContent, "Hello there!", "completed messages replace their streamed text");
  request.resolve({ text: "Hello there!", model: "gpt-6-luna", reasoningEffort: "low" });
  await settle();
  app.advance(30);
  assert.equal(app.State.chatHistory.length, 2);
  assert.equal(app.State.stateOverride, null);
  assert.equal(app.input.disabled, false);
  assert.equal(app.find(".chat-stop").hidden, true);
});

test("resetting history isolates the new turn from old tokens, approvals and completion", async (t) => {
  const app = await setupChat(t);
  const old = app.submit("Old request");
  old.emit({ kind: "approval", approvalId: "old-approval", command: "old command" });
  const oldAllow = app.find(".chat-allow");
  app.State.chatHistory = [];
  app.State.stateOverride = null;
  app.State.notify();
  app.advance(30);
  assert.equal(app.find(".chat-approval").hidden, true);
  const current = app.submit("New request");
  assert.notEqual(old.requestId, current.requestId);
  old.emit({ kind: "delta", itemId: "old", text: "stale text" });
  old.emit({ kind: "status", text: "stale status" });
  old.emit({ kind: "approval", approvalId: "late-approval", command: "late command" });
  oldAllow.dispatch("click");
  old.resolve({ text: "stale final" });
  await settle();
  app.advance(30);
  assert.deepEqual(app.State.chatHistory.map(({ content }) => content), ["New request", ""]);
  assert.deepEqual(app.decisions, []);
  assert.equal(app.find(".chat-status").textContent, "Connecting to Codex…");
  assert.equal(app.find(".chat-approval").hidden, true);
  assert.equal(app.input.disabled, true);
  assert.equal(app.State.stateOverride, "thinking");
  current.emit({ kind: "delta", itemId: "new", text: "Current answer" });
  current.resolve({ text: "Current answer" });
  await settle();
  app.advance(30);
  assert.equal(app.find(".reply-text").textContent, "Current answer");
  assert.equal(app.input.disabled, false);
});

test("tool approvals wait for an explicit allow or deny click and retain their request scope", async (t) => {
  const app = await setupChat(t);
  const request = app.submit("Inspect my laptop");
  request.emit({ kind: "approval", approvalId: "inspect", text: "Read system information", command: "uname -a", cwd: "/home/user" });
  request.emit({ kind: "approval", approvalId: "write", text: "Write a file", command: "touch note.txt" });
  app.advance(1000);
  assert.deepEqual(app.decisions, [], "receiving or displaying approval requests must not approve tools");
  assert.equal(app.find(".chat-approval").hidden, false);
  assert.equal(app.find(".chat-approval-details").textContent, "Read system information\n\nuname -a\n\nFolder: /home/user");
  app.find(".chat-allow").dispatch("click");
  await settle();
  assert.deepEqual(app.decisions, [[request.requestId, "inspect", true]]);
  assert.equal(app.find(".chat-approval-details").textContent, "Write a file\n\ntouch note.txt");
  app.find(".chat-deny").dispatch("click");
  await settle();
  assert.deepEqual(app.decisions, [[request.requestId, "inspect", true], [request.requestId, "write", false]]);
  assert.equal(app.find(".chat-approval").hidden, true);
  request.resolve({ text: "Inspected; write denied." });
  await settle();
});

test("the chat stays open past auto-close while manual collapse remains available", async (t) => {
  const app = await setupChat(t, { autoClose: 1 });
  app.island.onCursor(1000, 500);
  app.advance(3000);
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  app.island.collapse();
  app.advance(500);
  assert.equal(app.State.mode, "compact");
  assert.equal(app.island.fsm.pinned, false);
  app.island.setView("overview");
  assert.equal(app.island.fsm.homeCollapseDeadline, app.now() + 1000);
  app.advance(1500);
  assert.equal(app.State.mode, "compact", "leaving chat must restore normal auto-close");
});

test("a pending approval reopens a collapsed chat and stays visible until a decision", async (t) => {
  const app = await setupChat(t, { autoClose: 1 });
  const request = app.submit("Inspect the laptop");
  app.island.collapse();
  app.advance(500);
  assert.equal(app.State.mode, "compact");
  request.emit({ kind: "approval", approvalId: "shell", command: "uname -a" });
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.State.view, "prompt");
  app.island.onCursor(1000, 500);
  app.advance(3000);
  assert.equal(app.State.mode, "expanded");
  assert.equal(app.find(".chat-approval").hidden, false);
  assert.equal(app.island.fsm.homeCollapseDeadline, null);
  assert.deepEqual(app.decisions, []);
  app.find(".chat-deny").dispatch("click");
  await settle();
  request.resolve({ text: "Inspection denied." });
  await settle();
});

test("an approval after streamed text scrolls into view even when its card increases log height", async (t) => {
  const app = await setupChat(t);
  const request = app.submit("Inspect the laptop");
  request.emit({ kind: "delta", itemId: "answer", text: "I will inspect the system next." });
  app.advance(30);
  const log = app.find(".chat-log");
  log.scrollHeight = 500;
  log.clientHeight = 100;
  log.scrollTop = 400;
  request.emit({ kind: "approval", approvalId: "shell", command: "uname -a" });
  // Model the layout growth caused by renderApproval before the next DOM sync.
  log.scrollHeight = 700;
  app.advance(30);
  assert.equal(log.scrollTop, log.scrollHeight, "the new approval must be scrolled into view");
  assert.equal(app.find(".chat-approval").hidden, false);
  assert.deepEqual(app.decisions, []);
  request.resolve({ text: "No command executed." });
  await settle();
});

test("resolved approvals disappear and failed decisions remain visible for an explicit retry", async (t) => {
  const app = await setupChat(t);
  const request = app.submit("Run a command");
  request.emit({ kind: "approval", approvalId: "resolved", command: "pwd" });
  request.emit({ kind: "approvalResolved", approvalId: "resolved" });
  assert.equal(app.find(".chat-approval").hidden, true);
  assert.deepEqual(app.decisions, []);
  request.emit({ kind: "approval", approvalId: "retry", command: "ls" });
  app.Bridge.chatApprove = async (...args) => {
    app.decisions.push(args);
    throw new Error("Approval channel unavailable");
  };
  app.find(".chat-allow").dispatch("click");
  await settle();
  const approval = app.find(".chat-approval");
  assert.equal(approval.hidden, false);
  assert.equal(approval.querySelector(".chat-status").textContent, "Approval channel unavailable");
  assert.equal(app.find(".chat-allow").disabled, false);
  assert.equal(app.find(".chat-deny").disabled, false);
  app.advance(1000);
  assert.equal(app.decisions.length, 1, "failed decisions must not retry automatically");
  request.reject(new Error("Turn failed"));
  await settle();
});

test("Escape in the composer reaches the island close handler", async (t) => {
  const app = await setupChat(t);
  let stopped = false;
  app.input.dispatch("keydown", { key: "Escape", stopPropagation() { stopped = true; } });
  assert.equal(stopped, false, "Escape must bubble to the window's close handler");
  app.input.dispatch("keydown", { key: "a", stopPropagation() { stopped = true; } });
  assert.equal(stopped, true, "ordinary typing stays inside the composer");
});

test("Stop cancels only the active request once and preserves partial output", async (t) => {
  const app = await setupChat(t);
  const request = app.submit("Inspect the project");
  request.emit({ kind: "delta", itemId: "answer", text: "I checked the files." });
  app.advance(30);
  const stop = app.find(".chat-stop");
  stop.dispatch("click");
  stop.dispatch("click");
  await settle();
  assert.deepEqual(app.cancellations, [request.requestId]);
  assert.equal(stop.disabled, true);
  assert.equal(app.find(".chat-status").textContent, "Stopping…");
  request.reject(new Error("Codex turn interrupted"));
  await settle();
  app.advance(30);
  assert.deepEqual(app.State.chatHistory.map(({ content }) => content), ["Inspect the project", "I checked the files."]);
  assert.equal(app.find(".chat-status").textContent, "Stopped.");
  assert.equal(app.find(".reply-text").textContent, "I checked the files.");
  assert.equal(app.input.disabled, false);
  assert.equal(stop.hidden, true);
  assert.equal(app.requests.length, 1);
});

test("failed turns preserve user and partial assistant text without retrying an action", async (t) => {
  for (const partial of ["", "The first command completed."]) {
    await t.test(partial ? "after partial output" : "before output", async (t) => {
      const app = await setupChat(t);
      const request = app.submit("Perform the task");
      if (partial) request.emit({ kind: "delta", itemId: "answer", text: partial });
      request.reject(new Error("Connection lost"));
      await settle();
      app.advance(1000);
      assert.deepEqual(app.State.chatHistory.map(({ content }) => content), partial ? ["Perform the task", partial] : ["Perform the task"]);
      assert.equal(app.find(".bubble").textContent, "Perform the task");
      assert.equal(app.find(".reply-text")?.textContent ?? "", partial);
      assert.equal(app.find(".chat-status").textContent, "Connection lost");
      assert.equal(app.State.stateOverride, null);
      assert.equal(app.input.disabled, false);
      assert.equal(app.input.value, "");
      assert.equal(app.requests.length, 1);
    });
  }
});

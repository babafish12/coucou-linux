import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const alice = { id: "100", title: "Alice", lastMessage: "Hello", unreadCount: 1 };
const bob = { id: "200", title: "Bob", lastMessage: "Lunch?", unreadCount: 0 };
const charlie = { id: "300", title: "Charlie", lastMessage: "Later", unreadCount: 2 };
const message = (id, text) => ({ id, text, date: 1, outgoing: false });
let instance = 0;

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function flush() {
  // Settle API promises, their finally blocks, and resulting state notifications.
  await new Promise((resolve) => setImmediate(resolve));
}

async function setup(t, { visible = true, chats = [alice, bob, charlie] } = {}) {
  const fixture = process.env.TELEGRAM_TEST_FIXTURE;
  assert.ok(fixture, "Run with node scripts/test-telegram-inline.mjs");
  const controller = await import(`${pathToFileURL(fixture).href}?instance=${++instance}`);
  const { State, Telegram } = controller;
  let now = 0;
  let nextTimer = 0;
  const timers = new Map();
  const previousWindow = globalThis.window;
  globalThis.window = {
    setTimeout(callback, delay) {
      const id = ++nextTimer;
      timers.set(id, { callback, time: now + delay });
      return id;
    },
    clearTimeout(id) { timers.delete(id); },
  };
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  });
  const calls = { chats: 0, history: [], send: [] };
  Telegram.chats = async () => { calls.chats++; return chats; };
  Telegram.history = async (chatId) => { calls.history.push(chatId); return []; };
  Telegram.send = async (chatId, text) => {
    calls.send.push({ chatId, text });
    return { ...message("sent-1", text), outgoing: true };
  };
  State.settings.activeIntegrations = ["integration_telegram"];
  State.integrations.integration_telegram = {
    data: { state: "ready", paused: false, accountName: "account-a" },
    error: null,
    loaded: true,
    configured: true,
  };
  State.tasks = [{ id: "integration_telegram" }];
  State.mode = visible ? "expanded" : "hidden";
  State.view = "overview";

  return {
    ...controller,
    calls,
    async start() { controller.startInlineTelegram(); await flush(); },
    async open(chat) {
      State.view = "telegram";
      controller.selectInlineChat(chat);
      State.notify();
      await flush();
    },
    async advance(milliseconds) {
      const end = now + milliseconds;
      while (true) {
        const next = [...timers].filter(([, timer]) => timer.time <= end)
          .sort((a, b) => a[1].time - b[1].time)[0];
        if (!next) break;
        now = next[1].time;
        timers.delete(next[0]);
        next[1].callback();
        await flush();
      }
      now = end;
    },
  };
}

test("polls only visible Telegram content and shows the top two chats", async (t) => {
  const app = await setup(t, { visible: false });
  await app.start();
  await app.advance(15_000);
  assert.equal(app.calls.chats, 0);

  app.State.mode = "expanded";
  app.State.notify();
  await flush();
  assert.deepEqual(app.InlineTelegram.chats, [alice, bob]);
  assert.equal(app.calls.chats, 1);
  app.startInlineTelegram();
  await app.advance(5_000);
  assert.equal(app.calls.chats, 2, "starting twice must not duplicate polling");

  await app.open(alice);
  assert.deepEqual(app.calls.history, [alice.id]);
  app.State.mode = "hidden";
  app.State.notify();
  await app.advance(15_000);
  assert.equal(app.calls.chats, 2);
  assert.deepEqual(app.calls.history, [alice.id]);

  app.State.mode = "expanded";
  app.State.view = "overview";
  app.State.tasks = [{ id: "integration_claude" }];
  app.State.notify();
  await app.advance(10_000);
  assert.equal(app.calls.chats, 2, "other integrations must not poll Telegram content");
});

test("does not duplicate chat-list or history requests while a refresh is pending", async (t) => {
  const app = await setup(t);
  const list = deferred();
  const history = deferred();
  app.Telegram.chats = () => { app.calls.chats++; return list.promise; };
  app.Telegram.history = (id) => { app.calls.history.push(id); return history.promise; };
  await app.start();
  await app.refreshInlineChats();
  await app.advance(10_000);
  assert.equal(app.calls.chats, 1);
  list.resolve([alice, bob]);
  await flush();

  await app.open(alice);
  await app.refreshInlineHistory();
  await app.advance(10_000);
  assert.deepEqual(app.calls.history, [alice.id]);
  history.resolve([message("a1", "Current history")]);
  await flush();
  assert.equal(app.InlineTelegram.loadingHistory, false);
  assert.equal(app.InlineTelegram.messages[0].text, "Current history");
});

test("a late history response cannot replace the newly selected conversation", async (t) => {
  const app = await setup(t);
  const first = deferred();
  const second = deferred();
  app.Telegram.history = (id) => id === alice.id ? first.promise : second.promise;
  await app.start();
  await app.open(alice);
  await app.open(bob);
  second.resolve([message("b1", "For Bob")]);
  await flush();
  first.resolve([message("a1", "For Alice")]);
  await flush();
  assert.equal(app.InlineTelegram.selected.id, bob.id);
  assert.deepEqual(app.InlineTelegram.messages, [message("b1", "For Bob")]);
  assert.equal(app.InlineTelegram.loadingHistory, false);
});

test("rapid chat switches keep one history request active and load the final selection", async (t) => {
  const app = await setup(t);
  const first = deferred();
  app.Telegram.history = (id) => {
    app.calls.history.push(id);
    return id === alice.id ? first.promise : Promise.resolve([message("c1", "For Charlie")]);
  };
  await app.start();
  await app.open(alice);
  await app.open(bob);
  await app.open(charlie);
  assert.deepEqual(app.calls.history, [alice.id]);
  first.resolve([message("a1", "For Alice")]);
  await flush();
  assert.deepEqual(app.calls.history, [alice.id, charlie.id]);
  assert.equal(app.InlineTelegram.selected.id, charlie.id);
  assert.deepEqual(app.InlineTelegram.messages, [message("c1", "For Charlie")]);
});

test("sending stays bound to the original chat and cannot submit twice", async (t) => {
  const app = await setup(t);
  const sent = deferred();
  app.Telegram.send = (chatId, text) => {
    app.calls.send.push({ chatId, text });
    return sent.promise;
  };
  await app.start();
  await app.open(alice);
  app.InlineTelegram.drafts.set(alice.id, "  Reply to Alice  ");
  const pending = app.sendInlineMessage();
  await app.open(bob);
  app.InlineTelegram.drafts.set(bob.id, "Reply to Bob");
  await app.sendInlineMessage();
  assert.deepEqual(app.calls.send, [{ chatId: alice.id, text: "Reply to Alice" }]);
  sent.resolve({ ...message("a2", "Reply to Alice"), outgoing: true });
  await pending;
  assert.equal(app.InlineTelegram.drafts.has(alice.id), false);
  assert.equal(app.InlineTelegram.drafts.get(bob.id), "Reply to Bob");
  assert.deepEqual(app.InlineTelegram.messages, [], "Alice’s reply must not appear in Bob’s chat");
  assert.equal(app.InlineTelegram.sending, false);
});

test("send confirmation clears only the submitted draft and keeps later edits", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  // Keep the subsequent refresh pending while inspecting the send confirmation.
  const history = deferred();
  app.Telegram.history = () => history.promise;
  app.InlineTelegram.drafts.set(alice.id, "First reply");
  await app.sendInlineMessage();
  assert.equal(app.InlineTelegram.drafts.has(alice.id), false);
  assert.equal(app.InlineTelegram.messages[0].text, "First reply");

  const sent = deferred();
  app.Telegram.send = () => sent.promise;
  app.InlineTelegram.drafts.set(alice.id, "Second reply");
  const pending = app.sendInlineMessage();
  app.InlineTelegram.drafts.set(alice.id, "Next draft typed while sending");
  sent.resolve({ ...message("sent-2", "Second reply"), outgoing: true });
  await pending;
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Next draft typed while sending");
  assert.deepEqual(app.InlineTelegram.messages.map((item) => item.text), ["First reply", "Second reply"]);
  history.resolve(app.InlineTelegram.messages);
  await flush();
});

test("a failed send keeps the draft and is never automatically retried", async (t) => {
  const app = await setup(t);
  app.Telegram.send = async (chatId, text) => {
    app.calls.send.push({ chatId, text });
    throw new Error("Connection interrupted");
  };
  await app.start();
  await app.open(alice);
  app.InlineTelegram.drafts.set(alice.id, "Keep this reply");
  await app.sendInlineMessage();
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Keep this reply");
  assert.match(app.InlineTelegram.sendNotice, /Connection interrupted/);
  assert.equal(app.InlineTelegram.sending, false);
  await app.advance(15_000);
  assert.equal(app.calls.send.length, 1);
});

test("late history cannot remove a confirmed send or duplicate its message", async (t) => {
  const app = await setup(t);
  const history = deferred();
  const refreshed = deferred();
  app.Telegram.history = () => {
    app.calls.history.push(alice.id);
    return app.calls.history.length === 1 ? history.promise : refreshed.promise;
  };
  await app.start();
  await app.open(alice);
  const confirmed = { ...message("sent-1", "Confirmed"), outgoing: true };
  app.InlineTelegram.messages = [confirmed];
  app.InlineTelegram.drafts.set(alice.id, "Confirmed");
  await app.sendInlineMessage();
  history.resolve([message("old", "History from before sending")]);
  await flush();
  assert.deepEqual(app.InlineTelegram.messages, [confirmed]);
  refreshed.resolve([confirmed]);
  await flush();
  assert.deepEqual(app.InlineTelegram.messages, [confirmed]);
});

test("disconnect clears private content and ignores old list, history and send results", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  const history = deferred();
  const list = deferred();
  const sent = deferred();
  app.Telegram.history = () => history.promise;
  app.Telegram.chats = () => list.promise;
  app.Telegram.send = () => sent.promise;
  const pendingHistory = app.refreshInlineHistory();
  const pendingList = app.refreshInlineChats();
  app.InlineTelegram.drafts.set(alice.id, "Private draft");
  const pendingSend = app.sendInlineMessage();

  app.State.integrations.integration_telegram.data.state = "disconnected";
  app.State.notify();
  assert.deepEqual(app.InlineTelegram.chats, []);
  assert.deepEqual(app.InlineTelegram.messages, []);
  assert.equal(app.InlineTelegram.selected, null);
  assert.equal(app.InlineTelegram.drafts.size, 0);
  assert.equal(app.InlineTelegram.sending, false);
  assert.equal(app.InlineTelegram.loaded, false);

  history.resolve([message("old-history", "Old private history")]);
  list.resolve([alice, bob]);
  sent.resolve({ ...message("old-send", "Private draft"), outgoing: true });
  await Promise.all([pendingHistory, pendingList, pendingSend]);
  assert.deepEqual(app.InlineTelegram.chats, []);
  assert.deepEqual(app.InlineTelegram.messages, []);
  assert.equal(app.InlineTelegram.sendNotice, "");
  assert.equal(app.InlineTelegram.drafts.size, 0);

  app.Telegram.chats = async () => [charlie];
  app.State.integrations.integration_telegram.data.state = "ready";
  app.State.notify();
  await flush();
  assert.deepEqual(app.InlineTelegram.chats, [charlie]);
});

test("empty and oversized messages are rejected before any send", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  for (const draft of [" \n ", "🙂".repeat(4097)]) {
    app.InlineTelegram.drafts.set(alice.id, draft);
    await app.sendInlineMessage();
    assert.equal(app.InlineTelegram.drafts.get(alice.id), draft);
  }
  assert.equal(app.calls.send.length, 0);
  app.InlineTelegram.drafts.set(alice.id, "🙂".repeat(4096));
  await app.sendInlineMessage();
  assert.equal(app.calls.send.length, 1, "Telegram limits Unicode characters, not UTF-16 code units");
});

test("pause and transient status failures preserve drafts while stopping content polling", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  app.InlineTelegram.drafts.set(alice.id, "Unfinished reply");
  app.State.paused = true;
  app.State.notify();
  const before = app.calls.chats;
  await app.advance(15_000);
  assert.equal(app.calls.chats, before);
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Unfinished reply");

  app.State.paused = false;
  app.State.notify();
  await flush();
  await app.open(alice);
  const status = app.State.integrations.integration_telegram;
  status.loaded = false;
  status.error = "Temporary status failure";
  app.State.notify();
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Unfinished reply");
  status.loaded = true;
  status.error = null;
  app.State.notify();
  await flush();
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Unfinished reply");
});

test("account replacement clears the previous account’s conversation and drafts", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  app.InlineTelegram.messages = [message("private", "Private history")];
  app.InlineTelegram.drafts.set(alice.id, "Private draft");
  app.State.integrations.integration_telegram.data.accountName = "account-b";
  app.State.notify();
  assert.equal(app.InlineTelegram.selected, null);
  assert.equal(app.InlineTelegram.drafts.size, 0);
  assert.deepEqual(app.InlineTelegram.messages, []);
  assert.deepEqual(app.InlineTelegram.chats, []);
});

test("auth loss clears drafts even when the island was already paused", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  app.InlineTelegram.drafts.set(alice.id, "Private draft");
  app.State.paused = true;
  app.State.notify();
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Private draft");
  app.State.integrations.integration_telegram.data.state = "loggingOut";
  app.State.notify();
  assert.equal(app.InlineTelegram.drafts.size, 0);
});

test("disabling Telegram clears drafts even when it was already paused", async (t) => {
  const app = await setup(t);
  await app.start();
  await app.open(alice);
  app.InlineTelegram.drafts.set(alice.id, "Private draft");
  app.State.paused = true;
  app.State.notify();
  app.State.settings.activeIntegrations = [];
  app.State.notify();
  assert.equal(app.InlineTelegram.drafts.size, 0);
});

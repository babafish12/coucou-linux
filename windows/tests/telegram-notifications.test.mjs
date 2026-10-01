import assert from "node:assert/strict";
import test from "node:test";
import { pathToFileURL } from "node:url";

const alice = { id: "100", title: "Alice", lastMessage: "Hello", unreadCount: 1 };
const bob = { id: "200", title: "Bob", lastMessage: "Lunch?", unreadCount: 0 };
let instance = 0;

function notification(id, chat = alice, outgoing = false) {
  return {
    id,
    chat,
    message: { id: `message-${id}`, text: `Message ${id}`, date: 1, outgoing, senderName: chat.title },
  };
}

async function flush() {
  await new Promise((resolve) => setImmediate(resolve));
}

async function setup(t) {
  const fixture = process.env.TELEGRAM_TEST_FIXTURE;
  assert.ok(fixture, "Run with node scripts/test-telegram-inline.mjs");
  const controller = await import(`${pathToFileURL(fixture).href}?notifications=${++instance}`);
  const { State, Telegram, TelegramNotifications } = controller;
  const previousWindow = globalThis.window;
  globalThis.window = { setTimeout: () => 1, clearTimeout() {} };
  t.after(() => {
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  });
  State.settings.activeIntegrations = ["integration_telegram"];
  State.integrations.integration_telegram = {
    data: { state: "ready", paused: false, accountName: "account-a" },
    error: null,
    loaded: true,
    configured: true,
  };
  State.tasks = [{ id: "integration_claude", pillBadge: null }, { id: "integration_telegram", pillBadge: null }];
  State.focusId = "integration_claude";
  State.mode = "hidden";
  State.view = "overview";
  const calls = { show: [], views: [], history: [], notifications: 0 };
  t.after(State.subscribe(() => { calls.notifications++; }));
  const island = {
    showTelegramNotification() { calls.show.push(TelegramNotifications.current); },
    setView(view) { calls.views.push(view); State.view = view; },
  };
  Telegram.chats = async () => [alice, bob];
  Telegram.history = async (id) => { calls.history.push(id); return []; };
  Telegram.send = async () => { assert.fail("Notifications must never send messages"); };
  Telegram.openWindow = async () => { assert.fail("Notifications must stay inside the island"); };
  return {
    ...controller,
    calls,
    process: (items) => controller.processTelegramNotifications(island, items),
  };
}

test("unchanged empty notification polls do not wake the hidden island", async (t) => {
  for (const enabled of [false, true]) {
    await t.test(enabled ? "ready" : "disabled", async (t) => {
      const app = await setup(t);
      if (!enabled) app.State.settings.activeIntegrations = [];
      for (let poll = 0; poll < 30; poll++) app.process([]);
      assert.equal(app.calls.notifications, 0);
      assert.deepEqual(app.calls.show, []);
      assert.deepEqual(app.calls.views, []);
    });
  }
});

test("a notification batch surfaces only its newest incoming message without changing chat selection", async (t) => {
  const app = await setup(t);
  const first = notification("1");
  const latest = notification("2", bob);
  const outgoing = notification("3", alice, true);
  app.InlineTelegram.selected = alice;
  app.InlineTelegram.drafts.set(alice.id, "My unfinished reply");
  app.process([first, latest, outgoing]);
  assert.deepEqual(app.calls.show, [latest]);
  assert.equal(app.TelegramNotifications.current, latest);
  assert.equal(app.State.tasks[1].pillBadge, "finished");
  assert.equal(app.State.focusId, "integration_claude");
  assert.equal(app.InlineTelegram.selected, alice);
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "My unfinished reply");
  assert.equal(app.calls.notifications, 1);
});

test("polling an existing notification and dismissing it do not replay the alert", async (t) => {
  const app = await setup(t);
  const first = notification("1");
  app.process([first]);
  assert.equal(app.calls.notifications, 1);
  for (let poll = 0; poll < 30; poll++) app.process([structuredClone(first)]);
  assert.equal(app.calls.show.length, 1);
  assert.equal(app.calls.notifications, 1);
  app.dismissTelegramNotification();
  assert.equal(app.calls.notifications, 2);
  app.dismissTelegramNotification();
  app.process([first]);
  assert.equal(app.TelegramNotifications.current, null);
  assert.equal(app.calls.show.length, 1);
  assert.equal(app.calls.notifications, 2);
  const second = notification("2", bob);
  app.process([first, second]);
  assert.deepEqual(app.calls.show, [first, second]);
  assert.equal(app.calls.notifications, 3);
});

test("outgoing messages never trigger an incoming-message preview", async (t) => {
  const app = await setup(t);
  app.process([notification("1", alice, true)]);
  assert.equal(app.TelegramNotifications.current, null);
  assert.deepEqual(app.calls.show, []);
  assert.equal(app.State.tasks[1].pillBadge, null);
  assert.equal(app.calls.notifications, 0);
});

test("expanded interaction views retain their content and expose the new notification for explicit opening", async (t) => {
  for (const view of ["prompt", "telegram", "upload", "uploading", "choose", "approval", "question", "settings", "searching", "result"]) {
    await t.test(view, async (t) => {
      const app = await setup(t);
      app.State.mode = "expanded";
      app.State.view = view;
      app.InlineTelegram.selected = alice;
      app.InlineTelegram.drafts.set(alice.id, "Keep typing here");
      const incoming = notification("new", bob);
      app.process([incoming]);
      assert.deepEqual(app.calls.show, []);
      assert.deepEqual(app.calls.views, []);
      assert.equal(app.State.view, view);
      assert.equal(app.TelegramNotifications.current, incoming);
      assert.equal(app.InlineTelegram.selected, alice);
      assert.equal(app.InlineTelegram.drafts.get(alice.id), "Keep typing here");
    });
  }
});

test("idle island views allow the incoming-message preview", async (t) => {
  for (const view of ["overview", "empty", "greeting", "telegram-notification"]) {
    await t.test(view, async (t) => {
      const app = await setup(t);
      app.State.mode = "expanded";
      app.State.view = view;
      app.process([notification("new")]);
      assert.equal(app.calls.show.length, 1);
    });
  }
});

test("a collapsed island may show a notification even when its last view was a composer", async (t) => {
  for (const mode of ["hidden", "compact"]) {
    await t.test(mode, async (t) => {
      const app = await setup(t);
      app.State.mode = mode;
      app.State.view = "telegram";
      app.InlineTelegram.selected = alice;
      app.InlineTelegram.drafts.set(alice.id, "Saved draft");
      app.process([notification("new", bob)]);
      assert.equal(app.calls.show.length, 1);
      assert.equal(app.InlineTelegram.selected, alice);
      assert.equal(app.InlineTelegram.drafts.get(alice.id), "Saved draft");
    });
  }
});

test("approval, drag and send activity also block previews outside their usual views", async (t) => {
  const blockers = {
    pinned: (app) => { app.State.isPinned = true; },
    approval: (app) => { app.State.pendingApproval = { requestId: "pending" }; },
    drag: (app) => { app.State.fileDragOver = true; },
    sending: (app) => { app.InlineTelegram.sending = true; },
  };
  for (const [name, block] of Object.entries(blockers)) {
    await t.test(name, async (t) => {
      const app = await setup(t);
      app.State.mode = "expanded";
      app.State.view = "overview";
      block(app);
      app.process([notification("new")]);
      assert.equal(app.calls.show.length, 0);
      assert.ok(app.TelegramNotifications.current);
    });
  }
});

test("disabled, paused or disconnected Telegram does not show alerts or replay them after recovery", async (t) => {
  const unavailable = {
    disabled: (app) => { app.State.settings.activeIntegrations = []; },
    paused: (app) => { app.State.paused = true; },
    "telegram paused": (app) => { app.State.integrations.integration_telegram.data.paused = true; },
    disconnected: (app) => { app.State.integrations.integration_telegram.data.state = "disconnected"; },
    error: (app) => { app.State.integrations.integration_telegram.error = "Unavailable"; },
  };
  for (const [name, block] of Object.entries(unavailable)) {
    await t.test(name, async (t) => {
      const app = await setup(t);
      block(app);
      const incoming = notification("new");
      app.process([incoming]);
      assert.equal(app.TelegramNotifications.current, null);
      assert.equal(app.calls.show.length, 0);
      app.State.settings.activeIntegrations = ["integration_telegram"];
      app.State.paused = false;
      app.State.integrations.integration_telegram.data = { state: "ready", paused: false };
      app.State.integrations.integration_telegram.error = null;
      app.process([incoming]);
      assert.equal(app.calls.show.length, 0, "messages received during a pause must stay quiet afterwards");
    });
  }
});

test("an incoming message in the open conversation refreshes history without interrupting the draft", async (t) => {
  const app = await setup(t);
  app.State.mode = "expanded";
  app.State.view = "telegram";
  app.InlineTelegram.selected = alice;
  app.InlineTelegram.drafts.set(alice.id, "Reply in progress");
  app.startInlineTelegram();
  await flush();
  app.calls.history.length = 0;
  app.process([notification("new", alice)]);
  await flush();
  assert.deepEqual(app.calls.history, [alice.id]);
  assert.deepEqual(app.calls.show, []);
  assert.equal(app.State.view, "telegram");
  assert.equal(app.InlineTelegram.drafts.get(alice.id), "Reply in progress");
  app.calls.history.length = 0;
  app.process([notification("other", bob)]);
  await flush();
  assert.deepEqual(app.calls.history, []);
  assert.equal(app.InlineTelegram.selected, alice);
});

test("expired notifications clear the preview and return to Home", async (t) => {
  const app = await setup(t);
  app.process([notification("new")]);
  app.State.mode = "expanded";
  app.State.view = "telegram-notification";
  app.process([]);
  assert.equal(app.TelegramNotifications.current, null);
  assert.deepEqual(app.calls.views, ["overview"]);
  assert.equal(app.calls.notifications, 2);
  app.process([]);
  assert.equal(app.calls.notifications, 2);
});

test("an empty notification view still notifies when returning to Home", async (t) => {
  const app = await setup(t);
  app.State.mode = "expanded";
  app.State.view = "telegram-notification";
  app.process([]);
  assert.deepEqual(app.calls.views, ["overview"]);
  assert.equal(app.calls.notifications, 1);
  app.process([]);
  assert.equal(app.calls.notifications, 1);
});

test("notification expiration does not navigate away from an active conversation", async (t) => {
  const app = await setup(t);
  app.process([notification("new")]);
  app.State.mode = "expanded";
  app.State.view = "telegram";
  app.process([]);
  assert.equal(app.TelegramNotifications.current, null);
  assert.deepEqual(app.calls.views, []);
  assert.equal(app.State.view, "telegram");
  assert.equal(app.calls.notifications, 2);
});

// Entry point: boot the bridge, wire the island, start the greeting.

import "./style.css";
import { Bridge, IS_TAURI, onEvent } from "./core/bridge";
import { Sound } from "./core/sound";
import { State, type Settings } from "./core/state";
import { Island } from "./island/island";
import { registerSessionHandlers } from "./island/sessions";
import { registerIntegrationHandlers, refreshConfigured } from "./island/integrations";
import { CodexActivity, type ActivitySnapshot } from "./codex/activity";

async function main() {
  const root = document.getElementById("root");
  if (!root) return;

  void Sound.preload();

  const boot = await Bridge.boot();
  if (boot) {
    State.settings = { ...State.settings, ...boot.settings };
  }
  const island = new Island(root);
  void Bridge.chatWarmup();
  island.applySettings();
  State.loadIntegrationTasks();

  await onEvent<{ x: number; y: number }>("cursor", ({ x, y }) => island.onCursor(x, y));

  /** Pause has to reach Rust too, or the pollers keep calling out. */
  const setPaused = (on: boolean) => {
    if (State.paused === on) return;
    State.paused = on;
    void Bridge.setPaused(on);
    if (!on) void Bridge.codexMonitorReady();
  };

  await onEvent<string>("tray", (what) => {
    switch (what) {
      case "settings":
        setPaused(false);
        island.alert("settings");
        break;
      case "open":
        setPaused(false);
        island.alert(State.defaultView());
        break;
      case "pause":
        setPaused(!State.paused);
        if (State.paused) island.fsm.forceHidden();
        else island.reveal();
        break;
    }
  });

  await onEvent<null>("screen-changed", () => void Bridge.reposition());

  // The settings window writes preferences; apply them here without a restart.
  await onEvent<Settings>("settings-changed", (s) => {
    State.settings = { ...State.settings, ...s };
    island.applySettings();
    State.loadIntegrationTasks();
    void refreshConfigured();
  });

  await registerSessionHandlers(island);
  let activityReceived = false;
  await onEvent<ActivitySnapshot>("codex-activity", (snapshot) => {
    activityReceived = true;
    CodexActivity.apply(snapshot);
  });
  registerIntegrationHandlers(island);

  island.launch();
  await Bridge.codexMonitorReady();
  try {
    const snapshot = await Bridge.codexActivity();
    if (!activityReceived) CodexActivity.apply(snapshot);
  } catch (error) {
    if (!activityReceived) CodexActivity.setError(String(error));
  }

  // In a plain browser there is no wake strip behind the cursor: make the whole
  // page wake the island so the visuals can be checked with `npm run dev`.
  if (!IS_TAURI) {
    document.addEventListener("click", () => Sound.resume(), { once: true });
  }
}

void main();

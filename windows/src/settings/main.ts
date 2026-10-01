// Settings window — the place where anything that writes to disk is confirmed.
// Stage 2 covers the Claude Code hooks and the general preferences; API keys and
// integrations land here too in a later stage.

import "./settings.css";
import { Bridge, onEvent, type CodexModel, type HookStatus } from "../core/bridge";
import { DEFAULT_SETTINGS, State, type Settings } from "../core/state";
import { h, clear } from "../views/dom";
import { Telegram, telegramStatusLabel } from "../telegram/api";

let settings: Settings = { ...DEFAULT_SETTINGS };
let version = "";
let syncIntegrationControls = () => {};

const root = document.getElementById("settings-root")!;

async function save() {
  await Bridge.saveSettings(settings);
}

// ── Reusable bits ─────────────────────────────────────────────────────────────

function toggle(on: boolean, onChange: (v: boolean) => void): HTMLElement {
  const el = h("button", { class: on ? "switch on" : "switch", "aria-pressed": on });
  el.addEventListener("click", () => {
    const next = !el.classList.contains("on");
    el.classList.toggle("on", next);
    onChange(next);
  });
  return el;
}

function statusDot(ok: boolean): HTMLElement {
  return h("i", { class: "dot", style: `background:${ok ? "#22c55e" : "#f4505e"}` });
}

function renderDiff(text: string): HTMLElement {
  const box = h("div", { class: "diff" });
  for (const line of text.split("\n")) {
    const cls = line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
    box.append(h("div", { class: cls, text: line }));
  }
  return box;
}

// ── Claude Code section ───────────────────────────────────────────────────────

function claudeSection(status: HookStatus): HTMLElement {
  const body = h("div", { style: "display:flex;flex-direction:column;gap:12px" });
  const section = h(
    "section",
    {},
    h("h2", {}, statusDot(status.installed), h("span", { text: "Claude Code" })),
    body,
  );

  const rebuild = async () => {
    const fresh = await Bridge.hooksStatus();
    if (fresh) Object.assign(status, fresh);
    clear(body);
    draw();
    const head = section.querySelector("h2")!;
    clear(head);
    head.append(statusDot(status.installed), h("span", { text: "Claude Code" }));
  };

  function draw() {
    body.append(
      h("div", {
        class: "hint",
        text: status.installed
          ? "Coucou is hooked into your Claude Code sessions. Tool calls, questions and permission requests show up in the island, and you can answer them there."
          : "Install the hooks to see your Claude Code sessions in the island and approve permissions without leaving what you are doing.",
      }),
      h("div", { class: "row" },
        h("label", { text: "settings.json" }),
        h("span", { class: "path", text: status.settingsPath }),
      ),
      h("div", { class: "row" },
        h("label", { text: "Relay" }),
        h("span", { class: "path", text: status.hookPath }),
        statusDot(status.hookReady),
      ),
    );

    if (!status.hookReady) {
      body.append(h("div", {
        class: "notice warn",
        text: "coucou-hook.exe is not in place yet. Restart Coucou; if it still fails, build it with `cargo build -p coucou-hook`.",
      }));
    }

    const actions = h("div", { class: "row" });
    const install = h("button", {
      class: "primary",
      text: status.installed ? "Reinstall hooks…" : "Install hooks…",
      onclick: () => showPreview(true),
    });
    // Writing hook commands that point at a relay which isn't there would give
    // every Claude Code session a broken hook and nothing to show for it.
    if (!status.hookReady) {
      install.disabled = true;
      install.title = "The relay isn't installed yet.";
    }
    actions.append(install);
    if (status.installed) {
      actions.append(h("button", {
        class: "danger",
        text: "Uninstall hooks…",
        onclick: () => showPreview(false),
      }));
    }
    body.append(actions);
  }

  async function showPreview(install: boolean) {
    let preview;
    try {
      preview = await Bridge.hooksPreview(install);
    } catch (err) {
      // An unreadable or invalid settings.json stops here rather than being
      // treated as empty and written over.
      clear(body);
      body.append(
        h("div", { class: "notice err", text: String(err).replace(/^Error:\s*/, "") }),
        h("div", { class: "row" }, h("button", {
          text: "Back",
          onclick: () => { clear(body); draw(); },
        })),
      );
      return;
    }
    if (!preview) return;
    clear(body);
    body.append(
      h("div", {
        class: "hint",
        text: install
          ? "This is exactly what will change in your settings.json. Your own hooks are left untouched."
          : "This removes Coucou's entries only. Your own hooks are left untouched.",
      }),
      renderDiff(preview.diff),
      h("div", { class: "row" },
        h("span", { class: "path", text: `Backup → ${preview.backup}` }),
      ),
    );
    const confirm = h("button", {
      class: install ? "primary" : "danger",
      text: install ? "Back up and write" : "Back up and remove",
    });
    confirm.addEventListener("click", async () => {
      confirm.disabled = true;
      try {
        const backup = await Bridge.hooksApply(install, preview.fingerprint);
        clear(body);
        body.append(h("div", {
          class: "notice ok",
          text: `Done. Previous settings saved as ${backup}. Open a new Claude Code session to pick the hooks up.`,
        }));
        window.setTimeout(() => void rebuild(), 2600);
      } catch (err) {
        confirm.disabled = false;
        body.append(h("div", { class: "notice err", text: `Could not write: ${String(err)}` }));
      }
    });
    body.append(h("div", { class: "row" }, confirm, h("button", {
      text: "Cancel",
      onclick: () => { clear(body); draw(); },
    })));
  }

  draw();
  return section;
}

// ── Claude API section ────────────────────────────────────────────────────────

const MODELS: [string, string][] = [
  ["claude-opus-5", "Claude Opus 5"],
  ["claude-sonnet-5", "Claude Sonnet 5"],
  ["claude-haiku-4-5", "Claude Haiku 4.5"],
];

function apiSection(hasKey: boolean): HTMLElement {
  const dot = statusDot(hasKey);
  const state = h("span", { class: "hint", text: hasKey ? "Key saved in the Windows Credential Manager." : "No key yet — the chat needs one." });

  const field = h("input", {
    type: "password",
    placeholder: hasKey ? "••••••••••••  (stored)" : "sk-ant-...",
    style: "flex:1 1 auto;min-width:0",
    autocomplete: "off",
    spellcheck: "false",
  }) as HTMLInputElement;

  const saveBtn = h("button", { class: "primary", text: "Save key" });
  const clearBtn = h("button", { class: "danger", text: "Remove" });
  const feedback = h("div", {});

  async function refresh() {
    const present = (await Bridge.secretPresent("anthropic-api-key")) ?? false;
    dot.style.background = present ? "#22c55e" : "#f4505e";
    state.textContent = present
      ? "Key saved in the Windows Credential Manager."
      : "No key yet — the chat needs one.";
    field.placeholder = present ? "••••••••••••  (stored)" : "sk-ant-...";
    clearBtn.style.display = present ? "" : "none";
  }

  saveBtn.addEventListener("click", async () => {
    const value = field.value.trim();
    if (!value) return;
    clear(feedback);
    try {
      await Bridge.secretSet("anthropic-api-key", value);
      field.value = "";
      feedback.append(h("div", { class: "notice ok", text: "Saved. It never touches disk." }));
      await refresh();
    } catch (err) {
      feedback.append(h("div", { class: "notice err", text: `Could not save: ${String(err)}` }));
    }
  });

  clearBtn.addEventListener("click", async () => {
    clear(feedback);
    try {
      await Bridge.secretClear("anthropic-api-key");
      feedback.append(h("div", { class: "notice ok", text: "Key removed." }));
      await refresh();
    } catch (err) {
      feedback.append(h("div", { class: "notice err", text: `Could not remove: ${String(err)}` }));
    }
  });

  const model = h("select", {}) as HTMLSelectElement;
  for (const [id, label] of MODELS) model.append(h("option", { value: id, text: label }));
  if (!MODELS.some(([id]) => id === settings.model)) {
    model.append(h("option", { value: settings.model, text: settings.model }));
  }
  model.value = settings.model;
  model.addEventListener("change", () => {
    settings.model = model.value;
    void save();
  });

  clearBtn.style.display = hasKey ? "" : "none";

  return h(
    "section",
    {},
    h("h2", {}, dot, h("span", { text: "Claude" })),
    state,
    h("div", { class: "row" }, h("label", { text: "API key" }), field, saveBtn, clearBtn),
    h("div", { class: "row" }, h("label", { text: "Model" }), model),
    feedback,
  );
}

// Codex uses the local CLI account; no API key or hook installation is needed.
function codexSection(status: HookStatus): HTMLElement {
  let models: CodexModel[] = [];
  let loading = false;
  let saving = false;
  const dot = statusDot(false);
  const account = h("div", { class: "hint", text: "Checking your Codex login…" });
  const model = h("select", { id: "codex-model", style: "flex:1;min-width:0" }) as HTMLSelectElement;
  const effort = h("select", { id: "codex-reasoning", style: "flex:1;min-width:0" }) as HTMLSelectElement;
  const modelHint = h("div", { class: "hint", "aria-live": "polite" });
  const effortHint = h("div", { class: "hint", "aria-live": "polite" });
  const feedback = h("div", { "aria-live": "polite", role: "status" });
  const refresh = h("button", { text: "Refresh models" });

  const effortName = (value: string) => ({
    none: "None", minimal: "Minimal", low: "Low", medium: "Medium", high: "High",
    xhigh: "Extra high", max: "Maximum", ultra: "Ultra",
  }[value] ?? value);
  const selectedModel = () => models.find((entry) => model.value ? entry.model === model.value : entry.isDefault);

  function busy() {
    model.disabled = loading || saving || models.length === 0;
    effort.disabled = loading || saving || !selectedModel();
    refresh.disabled = loading || saving;
    refresh.textContent = loading ? "Loading models…" : "Refresh models";
  }

  function describeEffort() {
    const selected = selectedModel();
    const value = effort.value || selected?.defaultReasoningEffort;
    effortHint.textContent = selected?.supportedReasoningEfforts.find((entry) => entry.reasoningEffort === value)?.description
      ?? "Choose a model to see its supported reasoning levels.";
  }

  function drawEfforts(preferred: string) {
    const selected = selectedModel();
    clear(effort);
    effort.append(h("option", {
      value: "", text: selected?.defaultReasoningEffort
        ? `Model default (${effortName(selected.defaultReasoningEffort)})` : "Model default",
    }));
    for (const option of selected?.supportedReasoningEfforts ?? []) {
      effort.append(h("option", { value: option.reasoningEffort, text: effortName(option.reasoningEffort) }));
    }
    if (preferred && !selected?.supportedReasoningEfforts.some((entry) => entry.reasoningEffort === preferred)) {
      effort.append(h("option", { value: preferred, text: `${effortName(preferred)} (unavailable)`, disabled: true }));
    }
    effort.value = preferred;
    modelHint.textContent = selected?.description ?? (settings.model
      ? "Your saved model is unavailable. Choose another model or refresh the list."
      : "Models and reasoning levels come from your Codex installation.");
    describeEffort();
    busy();
  }

  function drawModels() {
    clear(model);
    const recommended = models.find((entry) => entry.isDefault);
    model.append(h("option", {
      value: "", text: recommended ? `Codex default (${recommended.displayName})` : "Codex default",
      disabled: models.length > 0 && !recommended,
    }));
    for (const entry of models) {
      model.append(h("option", { value: entry.model, text: entry.displayName }));
    }
    if (settings.model && !models.some((entry) => entry.model === settings.model)) {
      model.append(h("option", { value: settings.model, text: `${settings.model} (unavailable)`, disabled: true }));
    }
    model.value = settings.model;
    drawEfforts(settings.reasoningEffort);
  }

  async function loadModels(force: boolean) {
    loading = true;
    busy();
    clear(feedback);
    try {
      models = await Bridge.codexModels(force);
      drawModels();
    } catch (error) {
      feedback.append(h("div", { class: "notice err", text: String(error).replace(/^Error:\s*/, "") }));
    } finally {
      loading = false;
      busy();
    }
  }

  async function saveSelection(reset: boolean) {
    saving = true;
    busy();
    clear(feedback);
    feedback.append(h("div", { class: "hint", text: "Saving…" }));
    try {
      settings = await Bridge.codexSetPreferences(model.value, effort.value);
      clear(feedback);
      feedback.append(h("div", { class: "notice ok", text: reset
        ? "Saved. Reasoning was reset to the model default because the previous level is unsupported. Applies to your next reply."
        : "Saved. Applies to your next reply." }));
    } catch (error) {
      clear(feedback);
      feedback.append(h("div", { class: "notice err", text: String(error).replace(/^Error:\s*/, "") }));
    } finally {
      saving = false;
      drawModels();
    }
  }

  model.addEventListener("change", () => {
    const selected = selectedModel();
    const previous = settings.reasoningEffort;
    const supported = !previous || selected?.supportedReasoningEfforts.some((entry) => entry.reasoningEffort === previous);
    drawEfforts(supported ? previous : "");
    void saveSelection(!supported);
  });
  effort.addEventListener("change", () => { describeEffort(); void saveSelection(false); });
  refresh.addEventListener("click", () => void loadModels(true));

  drawModels();
  void loadModels(false);
  void Bridge.codexStatus().then((result) => {
    dot.style.background = result?.loggedIn ? "#22c55e" : "#f4505e";
    account.textContent = result?.message ?? "Could not check Codex. Run codex login in a terminal.";
  });

  return h("section", {},
    h("h2", {}, dot, h("span", { text: "Codex" })),
    account,
    h("div", { class: "hint", text: "Chat uses your existing Codex login. Model and reasoning choices apply to chat in Coucou." }),
    h("div", { class: "row" }, h("label", { for: "codex-model", text: "Model" }), model),
    modelHint,
    h("div", { class: "row" }, h("label", { for: "codex-reasoning", text: "Reasoning" }), effort),
    effortHint,
    h("div", { class: "row" }, refresh, h("span", { class: "hint", text: "Refresh after changing your Codex account." })),
    feedback,
    h("div", { class: "row" }, statusDot(status.installed), h("span", { text: status.installed ? "Local session monitor active" : "Waiting for local Codex sessions" })),
    h("div", { class: "path", text: status.settingsPath }),
    h("div", { class: "hint", text: "Shows local CLI and desktop activity, tool names and completion. Permission requests stay in Codex. Remote, cloud and ephemeral sessions are not monitored. Your Codex configuration is unchanged." }),
  );
}

// ── Integrations section ──────────────────────────────────────────────────────

function telegramSection(): HTMLElement {
  const dot = statusDot(false);
  const summary = h("div", { class: "hint", text: "Checking Telegram…" });
  const feedback = h("div", { role: "status", "aria-live": "polite" });
  const open = h("button", { id: "open-telegram", class: "primary", text: "Connect Telegram" });
  open.addEventListener("click", async () => {
    open.disabled = true;
    clear(feedback);
    try {
      if (!settings.activeIntegrations.includes("integration_telegram")) {
        if (settings.activeIntegrations.length < MAX_ACTIVE) {
          settings.activeIntegrations = [...settings.activeIntegrations, "integration_telegram"];
          await save();
          syncIntegrationControls();
        } else {
          feedback.append(h("div", { class: "notice warn", text: "All four Home slots are in use. Turn off another integration below to show Telegram on Home." }));
        }
      }
      await Telegram.openWindow();
    }
    catch (error) { feedback.append(h("div", { class: "notice err", text: String(error).replace(/^Error:\s*/, "") })); }
    finally { open.disabled = false; }
  });
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  async function refresh() {
    try {
      const status = await Telegram.status();
      if (disposed) return;
      dot.style.background = status.state === "ready" && !status.paused ? "#22c55e" : "#f5a524";
      summary.textContent = !status.runtimeAvailable ? "Install the Telegram runtime to connect. Open Telegram for details."
        : !status.configured ? "Connect your personal Telegram account to read existing chats and reply."
        : `${status.paused ? "Paused" : telegramStatusLabel(status.state)}${status.accountName ? ` · ${status.accountName}` : ""}`;
      open.textContent = status.configured ? "Open Telegram" : "Connect Telegram";
    } catch (error) {
      summary.textContent = `Could not check Telegram: ${String(error).replace(/^Error:\s*/, "")}`;
    }
    if (!disposed) timer = setTimeout(() => void refresh(), 5000);
  }
  window.addEventListener("beforeunload", () => { disposed = true; if (timer) clearTimeout(timer); }, { once: true });
  void refresh();
  return h("section", { id: "telegram-settings" },
    h("h2", {}, dot, h("span", { text: "Telegram" })),
    summary,
    h("div", { class: "hint", text: "Preview two recent chats on Home and reply directly in the island. Messages are only sent when you click Send. Open Telegram here for setup, the full list, and older messages." }),
    h("div", { class: "row" }, open),
    feedback,
  );
}

interface IntegrationDef {
  id: string;
  name: string;
  color: string;
  /** Credential Manager keys, in the order they are shown. */
  fields: { key: string; label: string; placeholder: string; secret: boolean }[];
}

const INTEGRATIONS: IntegrationDef[] = [
  { id: "integration_stripe", name: "Stripe", color: "#0570DE",
    fields: [{ key: "stripe-api-key", label: "Secret key", placeholder: "sk_live_…", secret: true }] },
  { id: "integration_github", name: "GitHub", color: "#F4505E",
    fields: [{ key: "github-token", label: "Token", placeholder: "ghp_…", secret: true }] },
  { id: "integration_vercel", name: "Vercel", color: "#7C5CFF",
    fields: [{ key: "vercel-token", label: "Token", placeholder: "…", secret: true }] },
  { id: "integration_n8n", name: "n8n", color: "#F29B38",
    fields: [
      { key: "n8n-url", label: "Instance URL", placeholder: "https://n8n.example.com", secret: false },
      { key: "n8n-api-key", label: "API key", placeholder: "…", secret: true },
    ] },
  { id: "integration_resend", name: "Resend", color: "#22C55E",
    fields: [{ key: "resend-api-key", label: "API key", placeholder: "re_…", secret: true }] },
  { id: "integration_notion", name: "Notion", color: "#8C8C8C",
    fields: [{ key: "notion-api-key", label: "Integration token", placeholder: "ntn_…", secret: true }] },
  { id: "integration_calcom", name: "Cal.com", color: "#C9956A",
    fields: [{ key: "calcom-api-key", label: "API key", placeholder: "cal_…", secret: true }] },
];

const MAX_ACTIVE = 4;

function integrationsSection(present: Record<string, boolean>): HTMLElement {
  const note = h("div", { class: "hint" });
  const list = h("div", { style: "display:flex;flex-direction:column;gap:14px" });
  const switches = new Map<string, HTMLButtonElement>();

  function updateNote() {
    const used = settings.activeIntegrations.length;
    note.textContent = `Pick up to ${MAX_ACTIVE} pills to show on Home next to Mochi — ${used}/${MAX_ACTIVE} in use. Keys are stored in the system credential store.`;
  }

  const integrations = State.agentProvider === "codex"
    ? [{ id: "integration_telegram", name: "Telegram", color: "#2481B5", fields: [] }, ...INTEGRATIONS]
    : INTEGRATIONS;
  syncIntegrationControls = () => {
    for (const [id, sw] of switches) {
      const active = settings.activeIntegrations.includes(id);
      sw.classList.toggle("on", active);
      sw.setAttribute("aria-pressed", String(active));
      sw.disabled = !active && settings.activeIntegrations.length >= MAX_ACTIVE;
    }
    updateNote();
  };

  for (const def of integrations) {
    const active = settings.activeIntegrations.includes(def.id);
    const sw = h("button", { id: `toggle-${def.id}`, class: active ? "switch on" : "switch", "aria-label": `Show ${def.name} on Home`, "aria-pressed": active });
    switches.set(def.id, sw);
    sw.addEventListener("click", () => {
      const on = settings.activeIntegrations.includes(def.id);
      if (on) {
        settings.activeIntegrations = settings.activeIntegrations.filter((x) => x !== def.id);
      } else {
        if (settings.activeIntegrations.length >= MAX_ACTIVE) return;
        settings.activeIntegrations = [...settings.activeIntegrations, def.id];
      }
      syncIntegrationControls();
      void save();
    });

    const rows = h("div", { style: "display:flex;flex-direction:column;gap:6px;flex:1 1 auto;min-width:0" });
    if (def.id === "integration_telegram") {
      rows.append(h("div", { class: "hint", text: "Uses your Telegram login above. Shows two recent chats with message previews and unread counts; select a chat to reply in the island. Reconnects your saved session while enabled." }));
    }
    for (const field of def.fields) {
      const input = h("input", {
        type: field.secret ? "password" : "text",
        placeholder: present[field.key] ? "••••••••  (stored)" : field.placeholder,
        autocomplete: "off",
        spellcheck: "false",
        style: "flex:1 1 auto;min-width:0",
      }) as HTMLInputElement;
      const saveBtn = h("button", { text: "Save" });
      const dotEl = statusDot(present[field.key] ?? false);
      saveBtn.addEventListener("click", async () => {
        const value = input.value.trim();
        try {
          await Bridge.secretSet(field.key, value);
          present[field.key] = value.length > 0;
          input.value = "";
          input.placeholder = value ? "••••••••  (stored)" : field.placeholder;
          dotEl.style.background = value ? "#22c55e" : "#f4505e";
        } catch {
          dotEl.style.background = "#f5a524";
        }
      });
      rows.append(
        h("div", { class: "row" },
          h("label", { style: "min-width:104px", text: field.label }),
          input, saveBtn, dotEl,
        ),
      );
    }

    list.append(
      h("div", { style: "display:flex;gap:12px;align-items:flex-start" },
        h("div", { style: "display:flex;align-items:center;gap:8px;min-width:132px;padding-top:4px" },
          sw,
          h("i", { class: "dot", style: `background:${def.color}` }),
          h("span", { style: "font-size:12.5px", text: def.name }),
        ),
        rows,
      ),
    );
  }

  syncIntegrationControls();
  return h("section", {}, h("h2", {}, h("span", { text: "Integrations" })), note, list);
}

// ── General section ───────────────────────────────────────────────────────────

function generalSection(): HTMLElement {
  const volume = h("input", {
    type: "range", min: "0", max: "0.2", step: "0.005",
    value: String(settings.soundVolume),
  }) as HTMLInputElement;
  volume.addEventListener("input", () => {
    settings.soundVolume = Number(volume.value);
    void save();
  });

  const autoClose = h("input", {
    id: "auto-close", type: "number", min: "0.1", max: "120", step: "0.1",
    value: String(settings.autoCloseInterval),
    style: "width:72px",
  }) as HTMLInputElement;
  autoClose.addEventListener("change", () => {
    const seconds = autoClose.valueAsNumber;
    settings.autoCloseInterval = Math.max(0.1, Math.min(120,
      Number.isFinite(seconds) ? seconds : settings.autoCloseInterval));
    autoClose.value = String(settings.autoCloseInterval);
    void save();
  });

  const screen = h("select", {}) as HTMLSelectElement;
  screen.append(
    h("option", { value: "primary", text: "Main display" }),
    h("option", { value: "cursor", text: "Display under the cursor" }),
  );
  screen.value = settings.screen;
  screen.addEventListener("change", () => {
    settings.screen = screen.value as Settings["screen"];
    void save();
  });

  return h(
    "section",
    {},
    h("h2", {}, h("span", { text: "General" })),
    h("div", { class: "row" },
      h("label", { text: "Sound" }),
      toggle(settings.soundEnabled, (v) => { settings.soundEnabled = v; void save(); }),
      volume,
    ),
    h("div", { class: "row" },
      h("label", { for: "auto-close", text: "Auto-close" }),
      autoClose,
      h("span", { class: "hint", text: "seconds after you leave the island" }),
    ),
    h("div", { class: "row" },
      h("label", { text: "Island lives on" }),
      screen,
    ),
    h("div", { class: "row" },
      h("label", { text: "Launch at startup" }),
      toggle(settings.autostart, (v) => { settings.autostart = v; void save(); }),
    ),
  );
}

// ── Boot ──────────────────────────────────────────────────────────────────────

async function main() {
  const boot = await Bridge.boot();
  if (boot) {
    settings = { ...settings, ...boot.settings };
    version = boot.version;
    State.agentProvider = boot.agentProvider;
  }
  const status = (await Bridge.hooksStatus()) ?? {
    installed: false, settingsPath: "", hookPath: "", hookReady: false,
  };

  const hasKey = State.agentProvider === "claude" && ((await Bridge.secretPresent("anthropic-api-key")) ?? false);

  const keys = [
    "stripe-api-key", "github-token", "vercel-token",
    "n8n-url", "n8n-api-key", "resend-api-key", "notion-api-key", "calcom-api-key",
  ];
  const present: Record<string, boolean> = {};
  for (const k of keys) present[k] = (await Bridge.secretPresent(k)) ?? false;

  clear(root);
  root.append(
    h("h1", {}, h("span", { text: "Coucou" }), h("span", { class: "version", text: version })),
    ...(State.agentProvider === "codex" ? [codexSection(status), telegramSection()] : [claudeSection(status), apiSection(hasKey)]),
    integrationsSection(present),
    generalSection(),
    h("div", {
      class: "hint",
      text: "No telemetry. Network requests only go to the services you configure yourself.",
    }),
  );

  void onEvent<Settings>("settings-changed", (s) => {
    settings = { ...settings, ...s };
    syncIntegrationControls();
  });
}

void main();

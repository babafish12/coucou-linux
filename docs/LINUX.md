# Linux + Codex

The Tauri application in `windows/` now also builds on Linux. The existing Mochi
character, sounds and island UI are shared. Windows keeps its Claude integration;
Linux uses the locally installed Codex CLI and its existing login.

## Install for your user

Requirements: Node 20+, npm, Rust/Cargo, Python 3, GTK 3, WebKit2GTK 4.1,
D-Bus and AppIndicator. On Arch Linux, the system packages are:

```sh
sudo pacman -S --needed base-devel rust nodejs npm python pkgconf gtk3 webkit2gtk-4.1 libayatana-appindicator librsvg xorg-xwayland
```

This command installs system packages and requires administrator access. Skip it
when these dependencies are already present. Codex must also be on your PATH:

```sh
codex login
codex login status
./scripts/install-linux.sh --autostart
~/.local/bin/coucou
```

The installer builds a release executable and installs into your home directory.
It does not need sudo, modify Codex configuration, or replace existing settings.
Changed installation files get a dated backup. Omit `--autostart` if you do not
want Coucou to start when you sign in. If a release build already exists, use
`--skip-build`. Run the same installer again after updating the source.

Use **Coucou** in your applications menu or run `coucou`. Start `coucou --settings`
to open settings, including when Coucou is already running. Quit through its
system tray menu. Hover the top centre of the main display to reveal Mochi.

## Codex behavior

- **Activity:** watches recent local `CODEX_HOME/sessions` JSONL logs incrementally
  (`~/.codex/sessions` by default). Shows active work, tool names and completion;
  concurrent sessions are represented by the most recently active session.
  Prompts, command arguments and tool outputs are not copied into the activity UI.
- **Approvals:** answer permissions and questions in Codex itself. Reading a log
  does not provide an approval channel, so Coucou never displays an Allow button
  for a monitored Codex session.
- **Chat:** runs `codex exec --json` with your saved account. No Anthropic key or
  separate OpenAI API key is required. Turns use read-only sandboxing, disabled
  shell tools/integrations/hooks, and ephemeral execution. User configuration is
  not loaded for these chat processes. Model and reasoning choices come from
  Coucou's settings, with defaults supplied by the Codex model catalog.
  This chat answers questions; it does not perform agent actions on your files.
- **Attachments:** UTF-8 text/code up to 200 KB and supported images up to 10 MB.
  PDF and other binary formats report an unsupported-format error. Files are
  copied into Coucou's inbox; originals are not modified.
- **Scope:** local desktop/interactive CLI sessions only. Cloud, remote, subagent,
  `codex exec` and ephemeral sessions are not shown. The rollout format is a
  Codex implementation detail and may require updates after a CLI upgrade.

The CLI integration follows the official
[Codex non-interactive documentation](https://learn.chatgpt.com/docs/non-interactive-mode).
Optional service integrations stay disabled initially. Their credentials use the
Linux Secret Service (for example GNOME Keyring or KeePassXC), never a plaintext
fallback. Codex manages its own authentication; Coucou does not read its tokens.

## Model and reasoning settings

Open **Settings → Codex** to choose a model and its reasoning level. Coucou loads
the catalog from the installed CLI through `codex app-server` and `model/list`.
The short-lived helper uses local standard input/output and starts no chat.
Model names, available reasoning levels, and defaults are read from the catalog
instead of a fixed list. See the official [app-server documentation](https://learn.chatgpt.com/docs/app-server#models).

- **Codex default** follows the catalog's recommended model.
- **Model default** follows that model's recommended reasoning effort.
- Switching models retains the selected effort when supported, otherwise resets
  it to Model default and shows a message.
- **Refresh models** reloads the list. Missing models or invalid saved selections
  are shown as unavailable and must be changed; chat does not silently substitute
  a different model.

Preferences persist in Coucou's `settings.json`. The chosen model and effort are
validated and passed to each chat turn using `--model` and
`-c 'model_reasoning_effort="…"'`. They affect the next Coucou reply, including in
an existing chat, without changing Codex settings or other Codex sessions.
A catalog entry does not guarantee that a particular request will be allowed by
your account; request errors are shown in the chat.

Each new chat reply displays **Requested: model · reasoning** using the resolved
values passed by the backend to that turn's Codex process. Changing preferences
does not relabel earlier replies. A model's generated answer to “Which model
are you?” can be wrong; use the request label to inspect Coucou's selection.
The CLI's JSON response does not independently confirm the provider's runtime
model identity, so the label reports the request rather than making that claim.

For the feasibility of reading and replying to existing Telegram chats, see
[Telegram connector](TELEGRAM.md). That connector is not implemented yet.

## Linux window support

The native GTK window uses X11, including XWayland on Hyprland. The app selects
`GDK_BACKEND=x11` when DISPLAY is available unless you explicitly selected a
backend. Native Wayland does not provide arbitrary global window positioning.
An XWayland session is therefore required for the positioned island.

The window has a dock hint and stays above regular windows. Its actual native
width and height follow the visible island, including during animations. This
avoids a large transparent input area on compositors such as Hyprland that select
XWayland windows by their full bounds rather than their X11 input mask. While
hidden, only the 240×6 wake strip remains and cursor polling stops.
The island sits at the monitor's top edge, centred horizontally, including when
a panel reserves space along that edge. On Hyprland it applies temporary window
properties to prevent background blur and keep the island on all workspaces.
Mochi's eyes use Hyprland's global pointer position, so they keep following the
mouse across workspace changes and while native Wayland applications have focus.
The compositor is queried on a background thread while the island is visible;
other desktops or unavailable Hyprland IPC fall back to GTK pointer tracking.
It does not edit your Hyprland configuration or reserve screen space. Other
compositors may apply their own dock, panel, workspace or fullscreen policies.
If DankMaterialShell has a transparent bar with an empty centre, enable its
**Click Through** option so the bar's empty area does not intercept island clicks.
DMS 1.6.2 still leaves a roughly four-pixel input strip at the exact centre when
the middle widget group is empty; adjacent island controls remain clickable.

## Paths

| Item | Default path |
| --- | --- |
| Launcher | `~/.local/bin/coucou` |
| Executable | `~/.local/lib/coucou/coucou` |
| Application entry | `~/.local/share/applications/Coucou.desktop` |
| Optional autostart | `~/.config/autostart/Coucou.desktop` |
| Preferences | `~/.config/coucou/settings.json` |
| Local log and inbox | `~/.local/share/coucou/` |

XDG_CONFIG_HOME and XDG_DATA_HOME are honored for preferences and data. Neither
Claude's settings nor Codex's settings are changed. Turn off startup in Coucou's
settings, or remove only its `Coucou.desktop` autostart entry to disable it.

## Development and checks

```sh
cd windows
npm ci
npm run build
cargo test --workspace
npm run tauri -- build --no-bundle
npm run tauri -- dev
```

`npm run build` verifies TypeScript and produces the UI assets. Cargo tests cover
Codex event parsing, bounded log reading, concurrent sessions, chat responses,
attachments and window input regions. A full visual check still needs a real
Linux desktop. The macOS Swift project is unchanged.

# coucou-linux

**A Linux desktop companion for local Codex sessions, based on [Coucou](https://github.com/Louis-CFM/coucou).**

Mochi lives at the top of your screen, shows Codex activity, and lets you chat or
ask questions about files using your existing Codex login. No Claude account,
Anthropic key, or separate OpenAI API key is required for the Linux integration.

![Linux](https://img.shields.io/badge/Linux-Arch%20%2B%20Hyprland-1793D1?logo=archlinux&logoColor=white)
![Tauri 2](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
[![Code license: MIT](https://img.shields.io/badge/code-MIT-green)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/babafish12/coucou-linux?style=social)](https://github.com/babafish12/coucou-linux)

This fork focuses on Linux and Codex. It has been tested on Arch Linux with
Hyprland using XWayland. The installed application is still called **Coucou** and
its launcher is `coucou`.

## Install on Linux

### Requirements

- Node.js 20+, npm, Rust/Cargo, Python 3, Git, and native build tools.
- GTK 3, WebKit2GTK 4.1, D-Bus, and AppIndicator development packages.
- An X11 session, or XWayland on a Wayland desktop such as Hyprland.
- Codex CLI installed on your `PATH`, with a working login.

On Arch Linux, install the system dependencies with:

```sh
sudo pacman -S --needed base-devel git rust nodejs npm python pkgconf gtk3 webkit2gtk-4.1 libayatana-appindicator librsvg xorg-xwayland
```

This changes system packages and requires administrator access. Skip it if the
dependencies are already installed. The Coucou installer itself runs as your
normal user.

### Build, install, and start

```sh
git clone https://github.com/babafish12/coucou-linux.git
cd coucou-linux

codex login status
# If you are not signed in yet, run: codex login

./scripts/install-linux.sh --autostart
~/.local/bin/coucou
```

The installer builds the application, installs it under `~/.local/`, and adds a
**Coucou** application menu entry. `--autostart` also starts it when you sign in;
omit that option if you prefer to launch it manually. Existing preferences are
preserved, and changed installation files receive dated backups.

For paths, configuration, and more setup details, see the
[Linux guide](docs/LINUX.md).

## Features

- **Local Codex activity:** shows work, tool names, and completion from local
  interactive CLI and desktop sessions. With concurrent sessions, the most
  recently active session is shown.
- **Chat with your Codex account:** ask questions from the island using the
  locally installed CLI and its saved login.
- **Model and reasoning selectors:** choose from the installed Codex catalog,
  with supported reasoning levels and defaults for each model. Each reply
  shows the model and reasoning requested for that turn.
- **Telegram on Home:** see the top two chats from Telegram's main list, with
  their latest message and unread counts. Select a chat to read and send text
  replies directly in the animated island, without opening another window.
  New notifications open a message preview with a direct reply action.
- **File context:** drop UTF-8 text or code files up to 200 KB, or supported images
  up to 10 MB, then ask questions about them. Original files are not modified.
- **A window that fits the island:** the native window follows the visible
  island's dimensions, including during animations. Collapsing it also shrinks
  the actual window, so it does not retain a large invisible area that blocks
  the application behind it.
- **Mochi animations and sounds:** the character and island UI are shared with
  the original project.
- **Optional integrations:** service integrations start disabled. Their
  credentials use Linux Secret Service, such as GNOME Keyring or KeePassXC.
- **Per-user installation:** application menu entry, optional autostart, and XDG
  settings and data paths.

## Use it

Hover near the top centre of your main display to reveal the island, then click
to open it. Start a local Codex session to see its activity, open chat to ask a
question, or drop a supported file onto the island for context.

Open settings from the system tray or run:

```sh
~/.local/bin/coucou --settings
```

Settings include sounds, startup, display selection, optional integrations, and
Codex model and reasoning dropdowns. Choose **Codex default** and **Model default**
to follow the catalog's defaults, or select explicit values. Choices apply to
the next chat reply. **Refresh models** reloads the catalog after an account or
CLI change. Quit from the system tray menu.

### Connect Telegram

Install the optional Telegram runtime once:

```sh
./scripts/install-telegram-linux.sh
```

This builds official TDLib and installs it under `~/.local/lib/coucou/` without
sudo. See [Telegram setup](docs/TELEGRAM.md) for build dependencies. Restart
Coucou, then open **Settings → Telegram → Connect Telegram**, choose **Telegram…**
in the tray, or run `coucou --telegram`.

Enter your Telegram API ID and API hash from
[my.telegram.org](https://my.telegram.org/apps), then complete the phone number,
login code, and any two-step verification requested by Telegram. Credentials
stay in the system keyring. Messages go to the selected chat only when you click
**Send**; they are not automatically passed to Codex.

Enable **Settings → Integrations → Telegram** to show its pill on Home. Opening
Telegram from Settings enables it automatically when a Home slot is available.
Home shows the top two chats in Telegram's main-list order. Select either one to
read its latest 50 messages and reply directly in the island. Visible chat lists
and the selected conversation refresh about every five seconds; connection status
and unread totals continue updating from the client's cache in the background.
Use Settings or the tray menu for the separate Telegram window, which provides
account setup, the full recent-chat list, and older messages. An enabled Telegram
integration reconnects your saved session when Coucou starts. Up to four optional
integrations can be shown alongside Codex.

### Current limits

- Approvals and permission questions stay in Codex; the activity monitor cannot
  approve them from the island.
- Built-in chat uses ephemeral, read-only Codex processes with shell tools
  disabled. It answers questions and does not perform agent actions on files.
- Cloud, remote, subagent, `codex exec`, and ephemeral sessions are not monitored.
  Monitoring reads local session logs, whose format may change with Codex updates.
- PDF and other unsupported binary attachments report an error.
- The positioned island requires X11/XWayland. Native Wayland positioning is not
  supported. The island sits at the top edge in the centre of the display; when hidden, a
  small 240×6 wake strip remains. Other compositors may apply different window
  policies. See [Linux window support](docs/LINUX.md#linux-window-support).

## Update an existing installation

From your checkout, pull the latest source and run the installer again:

```sh
git pull --ff-only
./scripts/install-linux.sh
```

Quit the running app through its tray menu and launch **Coucou** again to use the
new build. Reinstalling without `--autostart` leaves an existing startup entry
unchanged; disable startup in settings if you no longer want it.

## Development and contributing

The Linux and Windows Tauri application lives in `windows/` for historical
reasons. It uses Rust for the native backend and TypeScript/Canvas for the UI.
Linux selects the Codex backend; Windows retains its Claude integration. The
original macOS Swift project lives in `NotchBuddy/`.

After installing the dependencies above:

```sh
cd windows
npm ci
npm run build
cargo test --workspace
npm run tauri -- dev
```

To build a Linux release executable without a distribution package:

```sh
npm run tauri -- build --no-bundle
```

`npm run build` checks TypeScript and builds the UI. Cargo tests cover Codex
events, session monitoring, chat responses, attachments, and window input
regions. Window geometry and clicks reaching background applications also need
verification on a real desktop.

Send Linux improvements through
[pull requests](https://github.com/babafish12/coucou-linux/pulls). For window
fixes, include the compositor, X11/Wayland session type, display scaling, and
steps to reproduce. Keep changes focused and include the relevant build or test
results.
The inherited [contribution guide](CONTRIBUTING.md) covers the macOS project.

## Original project and credits

Original Coucou, Mochi, artwork, animations, and sounds by
[Louis Raillé](https://louisraille.fr). Linux and Codex adaptations are maintained
in [babafish12/coucou-linux](https://github.com/babafish12/coucou-linux).
This is an independent fork.

For upstream macOS and Windows downloads and documentation, visit
[Louis-CFM/coucou](https://github.com/Louis-CFM/coucou).

## License

- **Source code:** [MIT](LICENSE); retain the copyright notice.
- **Coucou and Mochi names, character, icons, sounds, and media:** © Louis Raillé,
  with separate terms in [LICENSE-ASSETS.md](LICENSE-ASSETS.md). These assets are
  not covered by the MIT license. Read those terms before distributing an app
  or derivative; they require permission to ship with the upstream branding and
  assets, or replacing them with your own.

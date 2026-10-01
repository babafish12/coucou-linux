# coucou-linux application

The Linux Tauri application uses Rust for the native backend and
TypeScript/Canvas for the island UI. It runs with X11 or XWayland and connects to
your locally installed Codex CLI.

See the [main README](../README.md) for features and installation,
[Linux setup](../docs/LINUX.md) for desktop support and paths, and
[contributing](../CONTRIBUTING.md) for development checks.

```sh
npm ci
npm run tauri -- dev
```

Build the release executable with:

```sh
npm run tauri -- build --no-bundle
```

The executable is `target/release/coucou`. Use `../scripts/install-linux.sh`
from this directory to install it for your user, including an application menu
entry and an optional startup entry.

## Layout

- `src/` — island, Canvas character, Codex activity, chat and Telegram UI.
- `src-tauri/` — Rust backend, GTK/X11 window, Codex client and service pollers.
- `public/sounds/` — Coucou sound assets served and bundled by Vite.
- `dev/` — local UI previews with fixtures.
- `tests/`, `scripts/test-*.mjs` — frontend checks.

Logs stay in `$XDG_DATA_HOME/coucou/` (default `~/.local/share/coucou/`).
Preferences use `$XDG_CONFIG_HOME/coucou/` (default `~/.config/coucou/`), and
credentials use Linux Secret Service. See the [asset terms](../LICENSE-ASSETS.md)
for the original Mochi artwork and sounds.

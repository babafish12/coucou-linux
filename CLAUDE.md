# coucou-linux — guide for AI coding agents

Coucou is a Linux desktop companion for local Codex sessions. The app uses
Tauri 2, a Rust backend and a TypeScript/Canvas frontend. It targets X11 and
XWayland, including Hyprland.

## Where things are

- `linux/src/` — island state, views, Mochi rendering, settings and Telegram UI.
- `linux/src-tauri/src/` — native window, Codex monitor/chat, secrets and integrations.
- `linux/public/sounds/` — original Coucou sound assets.
- `linux/dev/`, `linux/tests/` — browser fixtures and frontend regression tests.
- `scripts/` — per-user Linux installer, launcher and Telegram runtime installer.
- `docs/LINUX.md`, `docs/TELEGRAM.md` — setup and supported behavior.
- `docs/SPEC.md`, `docs/INTEGRATIONS.md` — architecture and integration entry points.

## Build and checks

```sh
cd linux
npm ci
npm run build
node scripts/test-island-performance.mjs
node scripts/test-codex-activity.mjs
node scripts/test-telegram-inline.mjs
cargo test --workspace --locked
npm run tauri -- build --no-bundle
```

Run `python3 scripts/test-coucou-launcher.py` from the repository root for
installer/launcher changes. Use `npm run tauri -- dev` from `linux/` for native
manual checks. See `CONTRIBUTING.md` for the review checklist.

## Rules

- Inspect the affected code and Git status before editing; preserve unrelated work.
- Keep existing style, avoid unnecessary refactors and justify new dependencies.
- Secrets use Linux Secret Service. Never put credentials in settings, logs or Git.
- Codex manages authentication; Coucou must not read its tokens or edit its config.
- No telemetry. Network calls only to services the user configured or invoked.
- Keep local activity monitoring read-only. Built-in chat approvals and Telegram
  sends require explicit user actions.
- Do not run animation loops while hidden. Preserve native window bounds and
  input-region behavior, and verify changes on an X11/XWayland desktop.
- Keep `fr.louisraille.coucou` for existing settings and keyring compatibility.
- Preserve original attribution and the separate terms in `LICENSE-ASSETS.md`.

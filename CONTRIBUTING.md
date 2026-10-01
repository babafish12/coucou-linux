# Contributing to coucou-linux

This fork develops the Linux desktop app with local Codex and optional service
integrations. Start with [Linux setup](docs/LINUX.md) for system dependencies.

## Getting started

```sh
cd linux
npm ci
npm run tauri -- dev
```

The native backend is in `linux/src-tauri/src/`; the TypeScript/Canvas frontend
is in `linux/src/`. Sounds are in `linux/public/sounds/` and are copied into the
frontend build by Vite. See [architecture](docs/SPEC.md) and
[integrations](docs/INTEGRATIONS.md) for the main entry points.

## Changes and checks

Keep changes focused and follow the existing Rust and TypeScript style. Avoid
new dependencies unless necessary. From `linux/`, run the checks relevant to
your change:

```sh
npm run build
node scripts/test-island-performance.mjs
node scripts/test-codex-activity.mjs
node scripts/test-telegram-inline.mjs
cargo test --workspace --locked
npm run tauri -- build --no-bundle
```

Installer and launcher changes also need the following check from the repository
root:

```sh
python3 scripts/test-coucou-launcher.py
```

Frontend previews under `linux/dev/` exercise UI fixtures without accounts or
real messages. Window geometry, focus, tray behavior and click-through require a
real X11/XWayland desktop. Include the compositor, session type, display scaling
and reproduction steps when reporting a window bug.

## Privacy and behavior

- Store integration credentials in Linux Secret Service; never in preferences,
  logs, frontend state or Git.
- Codex handles its own login. Do not read or copy its authentication tokens.
- No telemetry. Make network requests only for features and services the user
  configured or explicitly invoked.
- Keep monitored Codex sessions read-only. Approvals for the built-in chat and
  sending Telegram messages require an explicit user action.
- Preserve user settings and create dated backups when replacing installation
  files. Do not edit Codex configuration.
- Stop animation work when hidden and avoid unnecessary background polling.
  Check performance with the existing tests and a real desktop.

## Pull requests

Describe the concrete problem, resulting behavior and checks performed. Include
a screenshot or short recording for visual changes. Preserve attribution to
[Louis Raillé and the original Coucou project](https://github.com/Louis-CFM/coucou).
The [MIT code license](LICENSE) and [separate asset terms](LICENSE-ASSETS.md) both
apply; the artwork is not covered by the code license.

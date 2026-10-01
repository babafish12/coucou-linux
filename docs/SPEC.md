# Linux architecture and island behavior

The implementation is a Tauri 2 app with a Rust backend and TypeScript/Canvas
frontend. This guide describes the Linux application; the source and regression
tests define the detailed geometry and timing.

## Main components

| Component | Source | Responsibility |
| --- | --- | --- |
| Native application | [`lib.rs`](../linux/src-tauri/src/lib.rs) | Tauri commands, windows and background services |
| Linux island | [`island_linux.rs`](../linux/src-tauri/src/island_linux.rs) | GTK/X11 positioning, input regions, pointer tracking and window bounds |
| Shared frontend state | [`state.ts`](../linux/src/core/state.ts) | Tasks, views, settings and integration state |
| Island controller | [`island.ts`](../linux/src/island/island.ts), [`fsm.ts`](../linux/src/island/fsm.ts) | View transitions, hover, auto-close and rendering |
| Geometry | [`layout.ts`](../linux/src/core/layout.ts) | Logical dimensions, view sizes and character placement |
| Character | [`mochi/`](../linux/src/mochi/) | Canvas character, mini characters, greeting and animation lifecycle |
| File drop | [`upload/`](../linux/src/upload/), [`files.rs`](../linux/src-tauri/src/files.rs) | Drop animation and attachment copies |
| Sound | [`sound.ts`](../linux/src/core/sound.ts), [`public/sounds/`](../linux/public/sounds/) | Local sound playback and original sound assets |

Codex and external-service entry points are listed in
[the integration guide](INTEGRATIONS.md).

## Window and layout

The island is positioned at the top centre of the selected display. It uses
X11, including XWayland, with a native window that follows the visible island
through expansion and collapse. This prevents transparent areas from blocking
applications behind it. Native Wayland positioning is not supported.

The logical layout has three modes:

| Mode | Visible size | Behavior |
| --- | --- | --- |
| Hidden | No visible island; native 240 × 6 wake strip | Hover wakes the island |
| Compact | 288 × 32 | Mochi and the integration indicators remain visible |
| Expanded | 640 wide, height chosen by the view | Home, Codex activity, chat, files or Telegram |

Use `layout.ts` for current per-view dimensions. The frontend keeps a virtual
720 × 320 layout canvas, while the native window crops to the visible area.
Display scaling and native window coordinates must remain consistent.

## Interaction rules

- Hover opens the island immediately. Auto-close returns to the compact island
  when the pointer leaves; conversation views stay open until closed or changed.
- Pause can hide the island explicitly. Hidden views must stop their rendering
  loops and avoid unnecessary background activity.
- Mochi follows the pointer, reacts to clicks and long hovers, and animates with
  the active session state. The mini characters share its rendering engine.
- View changes coordinate the island bounds, character placement, content and
  integration indicators. Do not recreate render loops on every state update.
- A file drop runs the upload animation and copies the attachment into Coucou's
  inbox. It does not modify the original. Unsupported formats report an error.
- Monitored Codex activity is read-only. Approval controls belong to Coucou's
  built-in chat; monitored-session approvals stay in Codex.
- Telegram notifications respect the active view. A new message must not replace
  a conversation, draft or approval the user is handling.

See [Linux behavior and limits](LINUX.md) and [Telegram](TELEGRAM.md) for the
user-facing flows. Character artwork, animations, icons and sounds retain the
original Coucou attribution and [separate asset terms](../LICENSE-ASSETS.md).

## Verification

The frontend suites in `linux/tests/` use controlled clocks and DOM fixtures to
check state transitions, rendering, idle wakeups, chat, navigation and Telegram.
The previews in `linux/dev/` show the same flows without accounts or live actions.
See [contributing](../CONTRIBUTING.md) for commands.

Native window behavior still needs a real X11/XWayland desktop. Verify display
scaling, top-edge positioning, resize animations, keyboard focus, pointer tracking
and clicks reaching applications behind the island. JavaScript fixture timings
do not measure native WebKit or compositor CPU/GPU work.

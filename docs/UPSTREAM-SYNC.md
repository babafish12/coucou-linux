# Upstream review — 2026-10-06

Reviewed [Louis-CFM/coucou](https://github.com/Louis-CFM/coucou) after a fresh
`git fetch upstream`, from the fork baseline
[`d3bc42d`](https://github.com/Louis-CFM/coucou/commit/d3bc42d1522f52cf33e37baa085eb75643c4b90d)
through
[`03e3446`](https://github.com/Louis-CFM/coucou/commit/03e3446),
including all 92 new commits and the 0.1.1–0.1.8 release notes. The Linux fork's
starting commit for this review was `d5b8388`.

This update selectively ports applicable behavior. It does not merge the
upstream Git history: upstream still places its shared Windows/Linux app under
`windows/`, while this fork intentionally removed that tree and the macOS app in
favor of `linux/`. Merging those directories back would introduce a second app
with different settings, chat authentication and session monitoring.

## Ported in this update

| Upstream change | Linux adaptation |
| --- | --- |
| [`bef6558`](https://github.com/Louis-CFM/coucou/commit/bef6558) — private Linux data/config directories and logs | `private_fs.rs` creates app directories with `0700`; settings and logs use `0600`, including files written by older versions. Rewriting settings preserves an existing file symlink. |
| [`1f6e0d0`](https://github.com/Louis-CFM/coucou/commit/1f6e0d0) — safe local log output | `log.rs` escapes ASCII control characters, including newlines and terminal escape characters; the existing approximately 1 MB log limit remains. |
| [`bef6558`](https://github.com/Louis-CFM/coucou/commit/bef6558) — keep dropped files behind a private data directory | `files.rs` also makes the inbox private and copies attachments with `0600`. The destination is reserved with `create_new`, so collisions and symlinks cannot overwrite an existing file. Exhausting available names returns an error. |
| [`9fe8294`](https://github.com/Louis-CFM/coucou/commit/9fe8294) — local settings and pill shortcuts | While a chat has keyboard focus, `Ctrl+,` opens settings and `Ctrl+1`–`Ctrl+5` selects the corresponding Home integration. `Meta` is accepted too. Shortcuts require an expanded, unpaused island and never interrupt a pinned approval. The resting Home view keeps its existing native focus behavior. |

The filesystem helper changes only Coucou-owned directories/files. Tests use
temporary directories and leave the user's inbox and preferences alone.

## Already covered by this fork

| Upstream change | Existing equivalent or deliberate adaptation |
| --- | --- |
| [`eb1241f`](https://github.com/Louis-CFM/coucou/commit/eb1241f) — pause hidden-view animations | `linux/src/style.css` pauses animations outside rendering views; the island also stops animation work when hidden. |
| [`7ce5480`](https://github.com/Louis-CFM/coucou/commit/7ce5480) — shrink hidden Linux windows to the wake strip | `island_linux.rs` already fits the native window and input region to the island and uses the 240×6 hidden wake strip. |
| [`2481d77`](https://github.com/Louis-CFM/coucou/commit/2481d77) — do not focus the resting X11 island; show on every workspace | The fork already has X11 focus/input hints and Hyprland workspace handling. Those adaptations must be retained. |
| [`e98c182`](https://github.com/Louis-CFM/coucou/commit/e98c182) — avoid a 60 Hz display watcher when no global pointer is available | The fork parks its poller while hidden and throttles monitor metadata. Its visible X11/Hyprland pointer loop still supplies the global position for Mochi and workspace-aware hover behavior. Upstream's no-global-pointer Wayland loop is not a replacement for it. |
| [`593eda3`](https://github.com/Louis-CFM/coucou/commit/593eda3), [`3186879`](https://github.com/Louis-CFM/coucou/commit/3186879) — synchronize external open/close and retain approval cards | The Linux FSM uses explicit `forceHome`, `forcePetit`, `forceHidden` transitions and pins pending chat approvals. The hover/click and idle-hide settings in this update extend that FSM. |
| [`1f5f86a`](https://github.com/Louis-CFM/coucou/commit/1f5f86a), [`d7fc1c7`](https://github.com/Louis-CFM/coucou/commit/d7fc1c7) — usable settings window | The Linux settings window is already resizable and opened separately from the island. |
| [`4c61595`](https://github.com/Louis-CFM/coucou/commit/4c61595) — live model catalog | The fork already loads Codex models and their supported reasoning levels from the installed Codex CLI. Anthropic's catalog/defaults would replace the wrong provider. |
| [`d7dc8aa`](https://github.com/Louis-CFM/coucou/commit/d7dc8aa), [`c73c79d`](https://github.com/Louis-CFM/coucou/commit/c73c79d) — prevent arbitrary link schemes | The backend already restricts integration links to `http://` and `https://`. Exact Codex-chat navigation is a separate command that validates the session UUID before constructing its `codex://` link. |
| [`1f6e0d0`](https://github.com/Louis-CFM/coucou/commit/1f6e0d0) — omit sensitive integration data from logs | Linux integration logs already omit n8n response bodies, full service URLs and commands. |

## Feature additions that require a separate Linux implementation

These were reviewed and are not included in this port. They are not available in
upstream's TypeScript/Rust UI simply by merging files.

| Feature / upstream commits | Assessment for this fork |
| --- | --- |
| Markdown answers; Ollama and LM Studio — [`2566843`](https://github.com/Louis-CFM/coucou/commit/2566843) | Markdown rendering is useful for a later chat improvement. The implementation uses SwiftUI and needs a new safe DOM renderer here. Local model providers would add account/model behavior beyond the fork's existing Codex login. |
| GitHub PR/CI/review monitoring and contribution grid — [`daa4bec`](https://github.com/Louis-CFM/coucou/commit/daa4bec), [`28d045c`](https://github.com/Louis-CFM/coucou/commit/28d045c), [`86fbb79`](https://github.com/Louis-CFM/coucou/commit/86fbb79) | The current user's only enabled optional integration is Telegram; GitHub is disabled. This would require porting the Swift GraphQL queries, SHA-aware alert logic and several card views, plus validating token permissions and partial API failures. The current basic GitHub card remains. |
| Live file diffs and completion summaries — [`0540306`](https://github.com/Louis-CFM/coucou/commit/0540306), [`d5d0e49`](https://github.com/Louis-CFM/coucou/commit/d5d0e49) | Upstream derives diffs from Claude edit hooks. The fork reads Codex rollout events instead; a full diff viewer needs a Codex-specific source and bounded storage. The current task improves the readable session/activity overview independently. |
| Remaining keyboard shortcuts — [`9fe8294`](https://github.com/Louis-CFM/coucou/commit/9fe8294) | The small local settings/integration subset is ported above. Global shortcuts use macOS Carbon APIs; Linux needs a compositor/portal integration and conflict handling, particularly on Hyprland. Diff, window-capture and desktop-Mochi shortcuts also depend on features this fork does not implement. |
| Native Wayland overlay — [`1000f44`](https://github.com/Louis-CFM/coucou/commit/1000f44), [`4175ebc`](https://github.com/Louis-CFM/coucou/commit/4175ebc) | Relevant future option, but adds gtk-layer-shell and replaces window placement, input, pointer and focus handling. This fork's tested X11/XWayland and Hyprland behavior is retained. A native Wayland migration requires desktop testing of its own. |
| Mochi wardrobe, new greeting, desktop companion — [`b5d2242`](https://github.com/Louis-CFM/coucou/commit/b5d2242), [`f789a2a`](https://github.com/Louis-CFM/coucou/commit/f789a2a), [`2b02b91`](https://github.com/Louis-CFM/coucou/commit/2b02b91) | Optional visual features. The production implementation is Swift; desktop Mochi also depends on macOS window capture, drag and lifecycle APIs. |
| Sidebar settings — [`7df46f2`](https://github.com/Louis-CFM/coucou/commit/7df46f2) | SwiftUI reorganization, with no shared TypeScript view to import. The requested Linux behavior settings are added to the existing settings UI. |

## Platform/provider-specific changes left out

- Codex hook installer/approvals
  ([`123e713`](https://github.com/Louis-CFM/coucou/commit/123e713)),
  Cursor/main-agent selection
  ([`5332f9e`](https://github.com/Louis-CFM/coucou/commit/5332f9e)), third-party
  agent routing, Gemini and Antigravity hooks: upstream installs hook files and
  uses a local relay. This fork deliberately monitors local Codex sessions
  read-only and leaves Codex configuration untouched. Its built-in chat handles
  approvals through the app-server.
- Claude usage gauges and multiple-choice questions
  ([`ad44a2f`](https://github.com/Louis-CFM/coucou/commit/ad44a2f),
  [`c767db9`](https://github.com/Louis-CFM/coucou/commit/c767db9)): depend on
  Claude status-line and question hooks, not Codex session events.
- Multi-provider API-key chat and the pill catalog
  ([`adfef6b`](https://github.com/Louis-CFM/coucou/commit/adfef6b)): would add
  separate providers and credentials to a fork centered on the existing Codex
  account.
- iPhone, CloudKit, widgets, Live Activities, remote approvals/instructions,
  Siri/Spotlight/Focus, Liquid Glass, Apple Music, App Store signing and
  TestFlight documentation: use Apple-only APIs. They are not Linux features.
- Windows MSI/NSIS packaging, macOS release scripts and Linux distribution
  workflows targeting `windows/`: incompatible with the fork's current
  per-user installer and source layout. AppImage GStreamer registry isolation
  (`4c55ab8`) does not apply to its installed native executable.
- Telegram: upstream contains no Telegram implementation or new Telegram
  changes in this reviewed range. Detection and presentation fixes in this
  update are implemented locally against the fork's TDLib client.

## Verification

The port has focused Cargo regressions for private directory/file permissions,
existing settings symlinks, escaped log control characters, collision-safe
attachment copying and attachment retention. Run from `linux/`:

```sh
cargo test --workspace --locked private_fs::tests
cargo test --workspace --locked log::tests
cargo test --workspace --locked files::tests
node scripts/test-island-performance.mjs
```

The island fixture tests also cover shortcut-to-integration mapping and verify
that shortcuts leave pending approvals intact.

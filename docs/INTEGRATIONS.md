# Linux integrations

This guide maps the integrations in the current Linux app to their source.
Check the service's current official API documentation when changing requests;
existing endpoints and response formats are implementation details.

## Codex

Coucou uses the installed Codex CLI and the user's existing login. It does not
read authentication tokens or modify Codex configuration.

| Feature | Source | Boundary |
| --- | --- | --- |
| Activity monitoring | [`codex_monitor.rs`](../linux/src-tauri/src/codex_monitor.rs) | Reads recent local session logs incrementally; no approval channel |
| Built-in chat | [`codex_chat.rs`](../linux/src-tauri/src/codex_chat.rs) | Local app-server connection, ephemeral chat and streamed replies |
| Model catalog | [`codex_models.rs`](../linux/src-tauri/src/codex_models.rs) | Queries available models and reasoning levels from the installed CLI |
| Session navigation | [`codex_navigation.rs`](../linux/src-tauri/src/codex_navigation.rs), [`codex_window.rs`](../linux/src-tauri/src/codex_window.rs) | Focuses Codex or opens a validated session link |
| Attachments | [`files.rs`](../linux/src-tauri/src/files.rs) | Copies supported files into the local inbox |
| Activity interface | [`src/codex/`](../linux/src/codex/) | Session selection and public activity summaries |

The activity view excludes internal reasoning and raw tool outputs. Built-in
chat uses its own workspace and approval flow: **Allow once**, **Deny** and
**Stop** require explicit user actions. Stopping a turn does not undo completed
actions. See [Linux setup](LINUX.md) for supported session types, attachment
limits and the chat sandbox.

## Optional service integrations

Optional integrations start disabled. Configure credentials in Settings and
enable up to four Home integrations alongside Codex. Credentials are stored in
Linux Secret Service using [`secrets.rs`](../linux/src-tauri/src/secrets.rs),
not in `settings.json` or frontend state. There is no plaintext fallback.

[`integrations.rs`](../linux/src-tauri/src/integrations.rs) owns the HTTP pollers.
The current background intervals are:

| Integration | Main data | Interval |
| --- | --- | --- |
| n8n | Recent workflow executions | 15 seconds |
| Vercel | Deployments | 30 seconds |
| Stripe | Balance and recent charges | 30 seconds |
| Resend | Recent emails | 60 seconds |
| GitHub | User and repositories | 5 minutes |
| Cal.com | Upcoming bookings | 5 minutes |
| Notion | Search results/pages | 5 minutes |

Startup polling is staggered. Background pollers skip disabled integrations and
a paused app; each poller also requires its configured credentials. Explicit
Refresh buttons request a one-shot update. The first successful load populates
cards silently, then new events can update badges and sounds. Requests have a
10-second timeout and failures appear in the integration UI.

The backend emits integration data to
[`src/island/integrations.ts`](../linux/src/island/integrations.ts), with detail
views in [`src/views/integrations.ts`](../linux/src/views/integrations.ts).
Preserve the opt-in behavior and avoid turning repeated poll results into
repeated alerts.

## Telegram

Telegram uses the optional official TDLib runtime through
[`telegram.rs`](../linux/src-tauri/src/telegram.rs), with UI in
[`src/telegram/`](../linux/src/telegram/). It has its own personal-account login
and does not use the Codex account. Credentials and the database encryption key
use the system keyring.

Visible island chat views refresh their lists and messages; collapsed views do
not fetch those lists. Cached status and notification handling continue in the
background when connected. Sending a reply requires **Send** for the selected
chat. Messages are not automatically supplied to Codex.

See [Telegram setup](TELEGRAM.md) for runtime installation, login, session data,
notification privacy and current feature limits.

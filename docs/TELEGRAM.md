# Telegram connector: existing chats

Status: researched, not implemented or connected to an account.

Reading existing personal Telegram chats and replying from Coucou is technically
possible using Telegram's official [TDLib client library](https://core.telegram.org/tdlib/getting-started).
TDLib provides account authorization, chat lists, message history, updates, and
message sending. The [Bot API](https://core.telegram.org/bots/faq#what-messages-will-my-bot-get)
does not provide access to a user's existing private conversations.

## Requirements

- A TDLib runtime (`libtdjson`) on Linux. It is an additional dependency and was
  not installed on the laptop when this was checked.
- Telegram application credentials (`api_id` and `api_hash`), obtained through
  [Telegram's application setup](https://core.telegram.org/api/obtaining_api_id).
- A separate sign-in to the user's Telegram account, including any requested
  login code and two-step verification. A Codex login does not authorize Telegram.

## Fit with Coucou

The existing integration system can display a Telegram pill with unread counts
and recent activity. Unlike the current HTTP pollers, this connector needs a
persistent client session and live TDLib updates. Rust should own that session
and expose narrowly scoped commands to the TypeScript UI for listing chats,
loading history, and sending a reply.

Chat history and the reply composer would fit in a separate normal window,
opened from the island, so the island can retain its compact native dimensions.
Account credentials and the database encryption key belong in Secret Service;
local Telegram data belongs in Coucou's XDG data directory.

Sending should require an explicit user action for the selected chat. Telegram
messages should not automatically become Codex context; reading and replying can
work independently of AI chat. The connection also needs disconnect/logout,
expired-session handling, and pause behavior that stops network activity.

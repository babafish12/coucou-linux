# Telegram personal-account connector

Coucou's Linux app lets you read existing cloud chats and send text replies from
your own account directly in the island. For initial setup, open **Settings →
Telegram → Connect Telegram**, choose **Telegram…** from the tray menu, or run
`coucou --telegram` to open the separate Telegram window.
It uses Telegram's official [TDLib client library](https://core.telegram.org/tdlib/getting-started),
with a separate Telegram login. Your Codex login does not grant Telegram access.

Enable **Settings → Integrations → Telegram** to add the blue Telegram pill to
Home alongside Codex, GitHub, Stripe and the other integrations. Opening Telegram
from Settings enables it automatically if fewer than four optional integrations
are active. Existing selections are never removed automatically.

The Home card shows the top two chats in Telegram's main-list order, with each
chat's latest message and unread count. Select a chat to expand the island with
its latest 50 messages and a reply composer. The island keeps its animated
transitions, and reading or replying does not open a second window. Click
**Send** to send a text reply to the selected chat.

New Telegram notifications automatically expand the island with the chat name
and message preview. **Reply** opens that conversation inside the island;
**Dismiss** closes the preview. The preview does not take keyboard focus and
uses the existing auto-close interval, pausing while the mouse is over it.
While you are writing, handling an approval, or using another active view, a
**New Telegram message** button appears in the header instead of replacing your
work. Telegram's muted chats and preview-privacy settings are respected.
Existing notifications restored at startup or reconnection stay quiet.

Chat lists and the selected conversation refresh about every five seconds while
their island view is visible. The collapsed or hidden island does not fetch chat
lists or histories. Connection status and unread message/chat totals still update
from the client's cache every two seconds, including while the Telegram window
is closed. Fresh notification previews arrive through the same cached update;
the cache keeps at most eight for up to a minute and is cleared on disconnect,
pause, or logout. Those totals cover Telegram's main list and remain unknown until TDLib
supplies them; they are not estimated from the loaded chats. Telegram messages
are never automatically passed to Codex.

Use Settings or the tray menu to open the separate Telegram window for account
setup, the full recent-chat list, and loading older messages.

## Install the runtime

TDLib is optional: Codex chat works without it. The helper builds the official
source at the revision pinned in the script and installs only into your home
directory. Build dependencies are Git, CMake, Ninja, Clang, gperf, Python 3,
pkg-config, OpenSSL, and zlib development files. On Arch Linux:

```sh
sudo pacman -S --needed git cmake ninja clang gperf python pkgconf openssl zlib
./scripts/install-telegram-linux.sh
```

The first command installs system packages and needs administrator access; skip
it when the dependencies are already present. The helper itself needs no sudo.
Compilation takes several minutes and defaults to four parallel jobs; use
`COUCOU_BUILD_JOBS=2 ./scripts/install-telegram-linux.sh` on a low-memory laptop.
Restart Coucou afterwards. Installed files:

- `~/.local/lib/coucou/libtdjson.so`
- `~/.local/lib/coucou/TDLib-LICENSE.txt`
- `~/.local/lib/coucou/TDLib-revision.txt`

The build cache is under `$XDG_CACHE_HOME/coucou/tdlib/` (by default
`~/.cache/coucou/tdlib/`). Existing runtime files receive dated backups. Coucou
can also load a compatible system `libtdjson.so`; installing the pinned runtime
avoids differences between TDLib API revisions.

## Sign in

1. Open [my.telegram.org → API development tools](https://my.telegram.org/apps)
   and obtain your application's `api_id` and `api_hash`. Follow
   [Telegram's instructions](https://core.telegram.org/api/obtaining_api_id).
2. Open Coucou's Telegram window. Enter those values into **API ID** and
   **API hash**, then click **Save and continue**.
3. Enter your existing account's phone number, including its country code.
4. Enter the login code and any password or email verification Telegram asks for.
5. Choose a chat in the island or Telegram window, read its messages, type a
   reply, and click **Send**.

No bot token is needed. A bot cannot access your existing private conversations.
Do not put API credentials, login codes, or passwords into a GitHub issue or
Codex chat; enter them in the local connection form.

## Behavior and limits

- Home previews the top two chats from Telegram's main list. The separate
  Telegram window lists up to 100 recent chats, and search filters this loaded
  list. Archived chats and custom folders are not currently shown.
- The island shows up to 50 latest messages in the selected chat. In the separate
  Telegram window, **Load older messages** loads additional pages, retaining up
  to 300 messages in the current view.
- Replies are text only (up to 4096 characters). Media captions and placeholders
  are shown; downloading attachments, calls, and creating accounts are not supported.
- Existing secret chats from another Telegram client are not available. This
  connector uses cloud chats and does not enable new secret chats.
- The selected chat is named above the composer. Only clicking **Send** sends a
  message. Coucou waits for TDLib's send-success update before showing a
  confirmation; it does not automatically retry an ambiguous or failed send.
  A timeout can still be followed by delivery: refresh the chat before retrying.
- Telegram message text is never automatically sent to Codex. This connector
  provides an inbox and reply composer that you control; Codex does not control it.
- **Disconnect** closes the network connection while preserving the local login.
  Use **Connect Telegram** to reopen it. An enabled Home integration reconnects
  an existing saved session on startup; enabling it also resumes that session.
  Manual disconnect stays disconnected until you connect again, re-enable the
  Home integration, or restart Coucou. Turning the Home integration off closes
  its connection; you can still connect manually from the Telegram window.
  **Log out** ends this client's Telegram authorization.
- Pausing Coucou closes the Telegram client and stops network activity. Resuming
  reconnects a client that was connected before the pause. Closing only the
  Telegram window leaves its connection active until disconnect, pause, or exit.

## Local storage and troubleshooting

API credentials and a randomly generated database encryption key are stored in
Linux Secret Service, such as GNOME Keyring or KeePassXC. No plaintext credential
fallback is used. Telegram's local database lives under
`$XDG_DATA_HOME/coucou/telegram/` (normally `~/.local/share/coucou/telegram/`), with
private directory permissions. Login codes and two-step verification passwords
are not saved by Coucou. TDLib logging is disabled to avoid sensitive log data.

- **Runtime missing:** run the installer above, then restart Coucou.
- **Cannot save credentials:** unlock your Secret Service keyring. A locked or
  unavailable keyring also prevents reopening the encrypted database; Coucou
  does not replace a missing database key for an existing session.
- **Invalid API credentials:** disconnect, choose **Change application
  credentials**, and copy the API ID/hash from your Telegram application again.
- **Sign-in needs attention:** read the error in the connection window. Codes
  expire and Telegram may impose retry limits; do not repeatedly submit them.

Automated tests cover authentication state mapping, identifiers, message bounds,
update handling, cancellation, and send success/failure. Native UI checks can
exercise sign-in screens and composer behavior with fixture responses. A real
account is still required to verify that account's login, history and delivery.

For the inline controller's polling, chat-switch, draft and notification regressions, run
`cd linux && node scripts/test-telegram-inline.mjs`. With Vite running, open
`http://127.0.0.1:1420/dev/telegram-preview.html` to exercise the actual island
with local fixture chats, including send failures and incoming notifications.
This preview never connects
to Telegram or sends real messages, and is not included in production builds.

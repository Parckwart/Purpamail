# Purpamail

A fast, full-featured desktop mail client for Linux, written in Rust.

Purpamail brings the complete feature set and the familiar three-pane layout of a
classic desktop mail app to Linux: a mailbox sidebar with favorites and smart
mailboxes, a conversation-aware message list, a rich message viewer and a
proper rich-text composer — in its own purple-accented look, with light and dark
themes. It works with any IMAP, POP3 and SMTP provider and keeps everything in a
local SQLite store, so it is fast and fully usable offline.

## Features

**Accounts & sync**
- IMAP (with IDLE push), POP3 and SMTP; SSL/TLS, STARTTLS, password and OAuth 2.0 bearer tokens
- Add Account assistant with automatic server settings for common providers, manual setup, and a connection test
- Offline work: every change is applied locally first and replayed to the server later
- Connection Doctor, Activity window and connection log
- "On This Computer" local mailboxes; import from mbox / Maildir / `.eml`, export to mbox

**Reading**
- Columns layout (list beside the message) or classic layout (sortable table above the message)
- Conversations: grouped threads with expandable rows and a stacked conversation view, including related messages from other mailboxes
- HTML email rendering (tables, CSS subset, inline images), plain text with colored quote levels
- Remote content blocked by default; tracking pixels never load
- Data detectors for links, addresses, phone numbers and dates
- Calendar invitations with Accept / Maybe / Decline
- Attachment previews, Open, Save, Save All, Remove Attachments
- On-device message and conversation summaries
- Raw source, all headers, print, export as PDF, save as `.eml` / text / PDF

**Organizing**
- Favorites bar and sidebar favorites (Ctrl+1 … Ctrl+9)
- All Inboxes / Drafts / Sent / Junk / Trash / Archive across accounts
- VIPs, seven nameable flag colors, Flagged mailboxes per color
- Smart mailboxes with any/all criteria
- Rules: conditions on senders, recipients, subject, content, headers, dates, contacts, VIPs, priority, attachments …; actions such as move, copy, flag, color, highlight, mark read, junk, delete, archive, reply, forward, redirect, notify, play sound, run a command, set category, mute
- Adaptive junk filter (trainable) plus server spam headers, blocked senders
- Mail categories: Primary, Transactions, Updates, Promotions
- Remind Me, Follow Up suggestions, mute conversations, one-click unsubscribe
- Full text search with `from:`, `to:`, `subject:`, `body:`, `is:unread`, `is:flagged`, `has:attachment`, `-word`; save searches as smart mailboxes
- Filters (unread, flagged, to me, cc me, attachments, VIPs, today), sorting by any column
- Undo for moves, deletes and flag changes

**Writing**
- Rich text or plain text, format bar, fonts, colors, lists, quote levels, links, inline images
- Address autocompletion from contacts and previous recipients, address tokens
- Reply, Reply All, Forward, Forward as Attachment, Redirect, Send Again
- Signatures per account, templates (stationery), priority, read receipts
- Send Later (scheduled sending), Undo Send, automatic drafts, Outbox
- Spell checking with system hunspell dictionaries
- "Forgot the attachment?" and missing-subject checks

**Desktop integration**
- Desktop notifications and sounds (freedesktop), unread count in the window title
- Single instance: `mailto:` links and `.eml` files open in the running window
- `.desktop` file, icon and AppStream metadata; can be set as the default mail app
- Contacts with vCard import/export

## Building

Requirements: a Rust toolchain (1.85+), a C compiler (for the bundled SQLite) and
OpenSSL development headers (`libssl-dev` / `openssl-devel`). At runtime
Purpamail needs an X11 or Wayland session with OpenGL.

```sh
cargo build --release
./target/release/purpamail            # start
./target/release/purpamail --demo     # start with sample mail to explore
```

Install for the current user (binary, launcher, icon):

```sh
./install.sh
xdg-mime default org.purpamail.Purpamail.desktop x-scheme-handler/mailto   # optional
```

## Files

| What | Where |
|------|-------|
| Settings | `~/.config/purpamail/settings.toml` |
| Passwords | `~/.config/purpamail/secrets.toml` (mode 0600) |
| Mail store | `~/.local/share/purpamail/mail.db` |

`PURPAMAIL_CONFIG_DIR` and `PURPAMAIL_DATA_DIR` override these locations.

## Keyboard shortcuts

| Action | Shortcut |
|--------|----------|
| New message | Ctrl+N |
| Get all new mail | Ctrl+Shift+N |
| Reply / Reply All / Forward | Ctrl+R / Ctrl+Shift+R / Ctrl+Shift+F |
| Send | Ctrl+Shift+D or Ctrl+Enter |
| Archive | Ctrl+Shift+A |
| Delete | Delete |
| Mark read / unread | Ctrl+Shift+U |
| Flag | Ctrl+Shift+L |
| Junk | Ctrl+Shift+J |
| Search | Ctrl+Alt+F |
| Favorite mailbox 1–9 | Ctrl+1 … Ctrl+9 |
| Undo | Ctrl+Z |
| Preferences | Ctrl+, |

Help › Keyboard Shortcuts lists all of them.

## Architecture

```
src/core   mail logic: data model, SQLite store, MIME parsing and building,
           operations, rules, junk filter, search, categories, summaries,
           calendar, vCard, mbox, PDF, spell checking
src/net    IMAP / POP3 / SMTP clients and the background sync engine
src/ui     egui interface: main window, sidebar, message list, viewer,
           HTML renderer, composer, preferences, dialogs
```

The interface never talks to servers directly: it changes the local store
through `core::ops` (which queues server operations) and sends commands to the
sync engine, which reports back with events.

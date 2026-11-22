# Mod Panel + Purge — Design

Date: 2026-11-22
Status: Draft for approval
Builds on: `docs/superpowers/specs/2026-11-19-moderation-design.md` (step 1a, implemented on branch `feat/moderation-1a`)
Approved mockup: `.superpowers/brainstorm/1226-1795342774/content/panel-v4.html` (git-ignored; open it in a browser to see every screen described below)

## 1. Intent

Moderators can already do everything with slash commands. The owner wants a second way: a permanent **button panel** in a chosen mod channel, so a mod in the middle of an incident can click through instead of remembering command syntax. Example the owner gave: someone is spamming; the mod opens the panel channel, clicks *Members*, clicks *Kick*, picks the person, types "Spamming links", done.

The panel is a mirror of the slash commands, not a new system. Every button ends in exactly the same code path as its slash command: same permission checks, same DMs, same cases, same logs.

This change also adds one new moderation feature, **Purge**, available both as a slash command and on the panel.

## 2. Goals, non-goals, anti-goals

**Goals**
- A panel message that an admin posts into a channel and that keeps working across bot restarts.
- Every step-1a moderation action reachable from the panel (list in §5).
- Purge by message count or by time window, as `/purge` and on the panel.
- Each screen explains its buttons in one line each (owner requirement).

**Non-goals**
- Replacing slash commands. They stay exactly as they are.
- Step 1b/1c features (lockdown, AutoMod, raid, verification, Standby, modmail, appeals). Their panel buttons are added when those features are built.
- A one-pop-up "member list inside the form" (Option A in brainstorming). serenity 0.12.5 cannot parse the reply Discord sends for it; the owner chose the picker-then-pop-up flow (Option B).
- A web dashboard.

**Anti-goals (failure even if it "works")**
- A panel button that lets someone do what the matching slash command would refuse (any permission, hierarchy or protected-role bypass).
- The panel or its replies visible to people who did not click (every reply after the panel itself is ephemeral).
- Buttons that silently stop working after a restart.
- Purge deleting messages outside the channel it was run in, or more than the mod asked for.

## 3. Constraints

- Same stack and host constraints as step 1a (Rust, poise 0.7.1 / serenity 0.12.5, sqlx PostgreSQL, Wispbyte 512 MB RAM). No new crates unless unavoidable.
- serenity 0.12.5 pop-ups (modals) hold **text inputs only**. Anything that needs a member, role or channel list happens in the ephemeral message *before* the pop-up opens, using Discord's user / role / channel select menus.
- Discord limits: 5 buttons per row, 5 rows per message, 25 options per select, modal title ≤ 45 chars, embed description ≤ 4096, field value ≤ 1024. Interactions must be answered within 3 seconds (defer if the work is slower).
- **Restart safety:** no in-memory state for a flow. Everything a later step needs (which action, which member, which channel) travels inside the component `custom_id` (max 100 chars), e.g. `mp:act:kick` → `mp:pick:kick` → modal `mp:modal:kick:<user_id>`. A mod who leaves a flow half-way and comes back after a restart can continue it.
- Discord shows modal answers and select results only to the bot; nothing in a flow is posted publicly except what the action itself posts (e.g. the mod-log entry).

## 4. The panel message

- Posted by `/config panel <channel>` (Manage Server only, like all `/config`). Discord's nesting rule (command > group > subcommand) is respected: `panel` is a direct subcommand of `config`.
- The bot stores the channel and message id per guild. Running `/config panel` again deletes the old panel message (if it still exists) and posts a fresh one, so there is only ever one.
- The panel is an ordinary public message in that channel; who can see the channel is controlled by the admin's channel permissions. Clicking any button is checked by the bot (§6), so seeing the panel grants nothing.
- Content (from the mockup): embed titled **Moderation panel**, gold colour bar, one sentence ("Pick what you need. Every reply is visible only to you and follows the same rules as the slash commands."), then one line per category. Five buttons: **👤 Members**, **🏷️ Names & voice**, **🧹 Channels**, **📁 Records** (blurple), **⚙️ Config** (grey).

## 5. Category replies

Clicking a category answers with an **ephemeral** message: an embed (colour bar per category, title, one sentence, then every button with a one-line description, grouped under small field headings) and the buttons, grouped into rows by seriousness: grey = light, red = removes or deletes, green = undoes. Every category reply has a **← Back** button that returns to the category list (ephemeral copy of the panel's buttons). Exact wording is in the mockup; the builder may adjust wording to fit limits.

### 5.1 👤 Members (red bar)
| Button | Style | Flow | Runs |
|---|---|---|---|
| ⚠️ Warn | grey | member select → modal: reason (required) | `/warn` |
| 🔇 Timeout | grey | member select → modal: duration (required, e.g. `1h`, max 28d), reason | `/timeout` |
| 👢 Kick | red | member select → modal: reason | `/kick` |
| ⏳ Tempban | red | member select → modal: duration (required), delete-message days (0–7, default 0), reason | `/tempban` |
| 🔨 Ban | red | member select → modal: delete-message days (0–7, default 0), reason | `/ban` |
| 🔊 Untimeout | green | member select → modal: reason | `/untimeout` |
| ♻️ Unban | green | modal: user ID (required), reason — banned users are not in the member list | `/unban` |

### 5.2 🏷️ Names & voice
| Button | Flow | Runs |
|---|---|---|
| ✏️ Set nickname | member select → modal: nickname (≤ 32), reason | `/nick set` |
| 🧹 Reset nickname | member select → modal: reason | `/nick reset` |
| 📴 Disconnect | member select → modal: reason | `/voice disconnect` |
| 🔇 Server mute / 🔈 Unmute | member select → modal: reason | `/voice mute` / `unmute` |
| ➡️ Move | member select → the same ephemeral message is edited to a voice-channel select (its `custom_id` carries the member id) → modal: reason | `/voice move` |

### 5.3 🧹 Channels (blurple bar)
| Button | Flow | Runs |
|---|---|---|
| 🧹 Purge (red) | modal: **how much** (required: a number like `50`, or a time like `10m`, `2h`), only from member (optional user ID), reason | `/purge` (new, §7) |

*My call:* the panel's own channel is rarely the one being spammed, so the Purge pop-up is preceded by a **channel select** (text channels only, nothing pre-selected) and the purge runs in the channel the mod picks. (`/purge` itself runs in the channel where it is typed.) Footer line: "More channel tools (lockdown) arrive with the next update."

### 5.4 📁 Records (grey bar)
| Button | Flow | Runs |
|---|---|---|
| 📜 History | member select → result embed (ephemeral) | `/history` |
| 🔍 View case | modal: case number → result | `/case view` |
| ✏️ Edit reason | modal: case number, new reason | `/case reason` |
| 🗑️ Remove case (red) | modal: case number | `/case remove` |
| 📝 Add note | member select → modal: note (≤ 1000) | `/note add` |
| 🗒️ Notes | member select → result | `/note list` |
| 👁️ View note | modal: note id → result | `/note view` |
| ❌ Delete note (red) | modal: note id | `/note remove` |

### 5.5 ⚙️ Config (light grey bar, Manage Server only)
Same description pattern as the other categories, groups *People*, *Messages & logs*, *Rules*.

| Button | Flow | Runs |
|---|---|---|
| 🛡️ Mod roles | role select (multi, pre-filled with current roles, up to 25); the selection is saved as soon as the mod closes the select, no separate Save button (*my call*: a multi-role selection doesn't fit in a `custom_id`, and saving on select keeps it restart-safe) | `/config modroles` |
| 🔒 Protected | same as Mod roles | `/config protected` |
| 📋 Log channels | string select of the 8 log categories (each option shows its current channel) → channel select (text/news) or a Disable button | `/config logs set|disable` |
| ✉️ DM messages | string select of the 5 DM kinds → modal pre-filled with the current text (≤ 1500), plus a "Show moderator name" on/off toggle button | `/config dm set|reset|showmod` |
| 📈 Ladder | embed listing current rules + buttons Add rule (modal: warn count, action, duration) and Remove rule (modal: warn count) | `/config ladder` |
| ⏱️ Warn decay | buttons On (modal: days 1–365) / Off, current state shown | `/config decay` |
| 🚫 Name filter | buttons On / Off, Words (modal: comma-separated list, replaces the list), Hoist characters (modal), Fallback name (modal); every change validated like the slash commands | `/config nickfilter`, `/config nickwords` |

## 6. Permissions and behaviour rules (all flows)

- **Each click is authorised like the matching slash command, at the moment of the final action** (after the modal is submitted), using the same `authorize` / `require_member` logic and the same Manage Server rule for Config. Slash `/config` is also gated by Discord's `default_member_permissions`; the panel has no such Discord-side gate, so every Config panel action must run the explicit Manage Server / Administrator / owner check itself. The category buttons themselves may open for anyone; an unauthorised mod is refused with the same plain message the slash command gives, before any DM or Discord action.
- Category replies, pickers, pop-ups and results are ephemeral. The confirmation is the same text the slash command replies with (e.g. "Case #12: Kick applied to @user.").
- Errors reply ephemerally with the same `UserError` messages as the slash commands; unexpected errors with the generic message, logged.
- A stale or malformed `custom_id` (e.g. from a panel posted by an older version) answers "This panel is out of date — ask an admin to run /config panel again." and does nothing else.
- Interactions from other guilds are ignored.

## 7. Purge

New slash command and the panel button share one implementation.

`/purge amount:<text> [member:<user>] [reason:<text>]` in the current channel. Required permission: **Manage Messages** (or a mod role, per the step-1a permission model). Bot needs Manage Messages and Read Message History in that channel.

- `amount` is either a whole number **1–1000** (delete the last N messages, not counting the command itself) or a duration in the existing duration format (`10m`, `2h`, `1d`; **max 14 days**) meaning "every message newer than that". *My call:* cap at 1000 messages per purge to bound time and API calls; a time-window purge also stops at 1000.
- `member` optional: only that member's messages count toward N / are deleted.
- Discord can only bulk-delete messages younger than 14 days. Older messages in range are skipped and the reply says how many were skipped. Pinned messages are skipped (*my call*) and counted the same way.
- Deletes in batches of up to 100 via bulk delete; a batch of exactly 1 uses single delete.
- Reply (ephemeral): "Deleted N messages in #channel." plus skipped counts if any.
- Logged to the **mod_actions** log channel as an embed (moderator, channel, amount asked, deleted, skipped, member filter, reason). It is **not** a case: it targets a channel, not a member. The existing message-log category will also record the bulk delete; that is fine.
- No DM.

## 8. Restart safety and storage

- New columns on `guild_settings` (or a small table): panel channel id, panel message id.
- Component handling lives in the event handler (`InteractionCreate` for component and modal interactions); poise only handles slash commands. Routing is by the `mp:` `custom_id` prefix.
- No in-memory flow state (§3).

## 9. Testing

- Unit tests: `custom_id` encode/decode round-trips for every flow (including max-length IDs fitting 100 chars); purge `amount` parsing (number, duration, out-of-range, 0, > 1000, > 14d, junk); purge batch planning (ages and pinned → delete vs skip, batches of ≤ 100, single delete for 1); the panel and category embeds stay within Discord limits (≤ 5 rows, ≤ 5 buttons per row, field lengths).
- DB test for storing/replacing the panel message id.
- Manual checks on the test server: every button end to end; a non-mod clicking a Members action is refused before any DM; restart the bot mid-flow and finish it; `/config panel` twice leaves one panel; purge by count, by time, with member filter, with >14-day-old messages present.

## 10. Left to the builder

- Module layout (suggested: `src/panel/` with one file per category plus a router), exact `custom_id` scheme within the 100-char limit.
- How to share code between poise commands and component handlers (extracting each command body into a function taking `&serenity::Context`, `&Data`, the invoking member and the inputs is the expected shape).
- Exact wording of descriptions and replies within the mockup's meaning; emoji choice.
- Whether category replies edit the previous ephemeral message or send a new one (editing is preferred to avoid clutter).

## 11. Decisions marked "my call" (for owner review)

Mod/protected role selects save on select (no Save button); Move picks member then voice channel in two edits of one message; Purge picks its target channel with a channel select (the panel channel is rarely the spammed one); purge cap 1000 messages and 14-day window; pinned messages skipped by purge; purge logged but not a case; Unban asks for a user ID; ladder/word-list editing via pop-ups; stale panel buttons answer with "ask an admin to re-post"; `/config panel` replaces the previous panel.

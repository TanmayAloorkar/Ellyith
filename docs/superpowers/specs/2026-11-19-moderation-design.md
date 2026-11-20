# All-In-One Discord Bot — Moderation (Milestone 1) Design

Date: 2026-11-19
Status: Draft for approval
Source feature list: `C:\Users\TANMAY\Downloads\# All-In-One Discord Bot.md` (the "spec")

## 1. Intent

The owner wants one bot, written in Rust, that eventually covers every feature in the spec (moderation, utility, engagement, social alerts, fun, art) for their own server, replacing the usual mix of third-party bots. The owner has handed the whole build to us.

The spec is roughly 80 features, so it is being built section by section. **This document covers only Section 1, Moderation**, which ships first in three steps:

- **1a** — core moderation actions, cases, history, auto punishment, logging.
- **1b** — verification, AutoMod, link protection, raid protection, alt detection, quarantine, lockdown, health check.
- **1c** — Standby Staff Mode, staff availability, staff handoff, modmail, appeals, modmail blacklist.

Each step must be usable on the live server on its own before the next starts. Later sections (utility, engagement, etc.) will get their own design documents; the architecture here must leave room for them as new modules without restructuring.

## 2. Context and constraints

- **One server.** The bot is private to the owner's server (small: under ~500 members, few existing bots, no data to migrate). It is not a public bot. *Builder's call (mine):* still key all config and records by `guild_id` so a private test server works with the same binary.
- **Language:** Rust. Use existing crates; do not port discord.js. Chosen stack (my call): `serenity` (gateway, REST, cache) + `poise` (slash commands) on `tokio`, `sqlx` with migrations for the database. Write our own code only where no crate exists.
- **Host:** Wispbyte free tier, which offers a "Rust (compiled binaries)" runtime with **512 MB RAM and 1 GB disk**, plus a host-managed **PostgreSQL** database.
  - Build on the owner's Windows PC, not on the host (compiling serenity needs far more than 512 MB). Produce a static Linux binary (`x86_64-unknown-linux-musl`; e.g. via `cargo-zigbuild` or WSL) and upload it. Confirm the host architecture with `uname -m` on first deploy; switch target if it is ARM.
  - Steady-state memory must stay well under 512 MB. Every in-memory cache must be bounded.
  - Disk is small: no large local files. All state lives in PostgreSQL.
- **Configuration:** slash commands only for v1. A web dashboard is a later phase; keeping config in PostgreSQL tables (not files) is what makes that possible later.
- **Commands:** slash commands only. No prefix commands.
- **Secrets:** bot token and database URL come from environment variables, never committed.
- **Gateway intents:** privileged Server Members and Message Content (both must be enabled in the Discord developer portal), plus Guilds, Guild Messages, Guild Moderation, Guild Voice States, Guild Invites, Direct Messages. Reaction and presence intents are not needed for this milestone.

## 3. Goals, non-goals, anti-goals

**Goals**
- Every moderation feature in the spec's Section 1 works on the owner's server, configured entirely through slash commands.
- Nothing time-based is lost on restart: tempban expiry, Standby expiry, quarantine state all survive the bot going down.
- Every action, automatic or manual, is recorded as a case and (if enabled) logged.

**Non-goals (this milestone)**
- Web dashboard.
- Prefix commands.
- Discord's native AutoMod (the owner chose bot-side filters only).
- Image captcha verification.
- Public/multi-server operation, sharding.
- Any feature from spec Sections 2–6.
- Importing data from other bots.

**Anti-goals (failure even if it "works")**
- The bot punishes someone a moderator could not legitimately punish (role hierarchy or protected roles bypassed).
- A tempban never lifts, or a sub-staff member keeps elevated permissions after Standby expires, because the bot restarted.
- Raid responses that kick or ban real people without an admin having explicitly enabled those responses.
- Message content stored long-term in the database.
- Memory growth that eventually kills the bot on the 512 MB host.

## 4. Permissions model (applies to all steps)

- **Moderator roles:** configured list. A user may run a mod command if they hold a configured moderator role **or** the Discord permission matching that command (e.g. Ban Members for `/ban`). Owner and Administrator users may run everything.
- **Hierarchy check (my call):** the bot refuses to act on a target whose highest role is equal to or above the invoking moderator's highest role, or above the bot's own role. Owner is never actionable.
- **Protected roles:** configured list. Members holding one cannot be warned, timed out, kicked or banned by moderators. Owner and Administrator users bypass this.
- Auto punishment and AutoMod respect protected roles too (my call: protected members are exempt from automatic actions).
- **Staff** (owner's decision): moderator roles, Administrator users, and sub-staff roles (§7) — always, not only during Standby. Staff can view `/history`, mod notes, the status board, handoff notes and modmail, and are the people who appear on the status board and get pinged. Viewing access is not action access: sub-staff can only *act* through Standby.
- **Standby command subset is a hard limit** (my call): a member whose only staff standing comes from a sub-staff role may run only the admin-chosen Standby commands, and only while Standby is active. The "matching Discord permission" path in the first bullet does not apply to them, even though the temporary Standby role carries real permissions.

## 5. Step 1a — Core moderation

### Commands
- `/warn`, `/timeout` (Discord native timeout; reject durations over 28 days), `/untimeout`, `/kick`, `/ban` (optional message-deletion window), `/tempban` (duration; automatic unban), `/unban`.
- Nicknames: `/nick set`, `/nick reset`.
- Voice: disconnect, server-mute/unmute, move member to another voice channel.
- Cases: `/case view <id>`, `/case reason <id> <text>`, `/case remove <id>`.
- `/history @user` (staff): all cases, including removed ones (marked).
- `/note add|list|remove @user` (staff only, never shown to the member).
- `/mywarnings` (any member): ephemeral reply listing their own active cases; never shows notes or removed cases.
- Config commands for everything below.

### Cases
- Every manual or automatic action creates a case: sequential case number per guild, type, target, moderator (or "AutoMod"/"System"), reason, duration, created time, expiry if any.
- **Removal is a soft delete:** the case stays in the database marked removed, with who removed it and when. Removed cases do not count toward auto punishment and do not show in `/mywarnings`.

### Tempban expiry
- Stored in the database. A scheduler task checks for due expirations periodically and at startup, so expirations missed while offline are processed when the bot comes back.

### Auto punishment
- **Configurable ladder** of rules: "at N active warnings → action", where action is timeout (with duration), kick, tempban (with duration) or ban.
- Default ladder: 3 warns → 1 hour timeout; 5 warns → kick; 7 warns → ban.
- **Warn decay**, can be turned on or off (default off), with a configurable number of days. Decayed warnings stay in history but no longer count toward the ladder.
- The ladder triggers on warnings from both moderators and AutoMod. The resulting action is its own case attributed to "Auto punishment".
- A rule fires when the active-warning count *reaches* its N (my call), so going from 4 to 5 fires the "5" rule once; it does not re-fire on later warnings unless another rule's N is reached.

### Nickname enforcement
- Mods can set/reset nicknames.
- Automatic reset of bad nicknames on join and on change: names matching the filtered words list (this list is introduced in 1a and later shared with the 1b AutoMod word filter), names starting with hoisting characters (configurable set, default includes `!`), and names with no mentionable characters. Reset to a configurable fallback (e.g. "Moderated Nickname"). This is enforcement on content only; there is no per-member nickname lock.

### Member DMs
- Before applying a warn/timeout/kick/ban/tempban, the bot DMs the member using a configurable template per action type. Placeholders include action, reason, duration, server name, case number.
- Showing the moderator's name is a setting, **off by default**.
- If the DM fails (DMs closed), the action still proceeds; the case notes the DM failed.
- The DM includes an **Appeal** button once step 1c ships (see §7). Design the DM so the button can be added without changing 1a's flow.

### Logging
Each category can be turned on or off and given its own channel. A mod may enable just one category or all of them.

| Category | Contents |
|---|---|
| Mod actions | every case created, edited, removed |
| Message edits/deletes | before/after content, author, channel; bulk deletes |
| Joins/leaves | member, account age, (1b: invite used, alt score) |
| Member role/nickname changes | what changed and who did it (from audit log) |
| Channel create/edit/delete | what changed and who did it |
| Role create/edit/delete | what changed and who did it (my addition) |
| Voice join/leave/move | (my addition) |
| External mod actions | bans/kicks/timeouts done outside the bot, read from the audit log (my addition) |

- Message content is kept **only in a bounded in-memory cache** (my call: cap by count, e.g. last 10,000 messages, and age, e.g. 24 hours) so edit/delete logs can show the old content. It is never written to the database.

## 6. Step 1b — AutoMod, raids, verification

### Verification
- Admins pick the method: **button** (click "Verify") or **button + quiz** (staff-defined questions in a modal; configurable whether answers are auto-checked or sent to staff — builder's call on the default).
- When verification is enabled, new members get the quarantine role on join (reason: verification). Passing verification removes it and gives the configured roles. Existing members are not affected when verification is switched on (my call).
- Unverified and quarantined members share one access mechanism (see Quarantine).

### AutoMod (bot-side only)
- Rules: word/phrase filter (plain words and regex), message rate spam, duplicate messages, excessive caps, mass mentions, emoji spam.
- Each rule has its own action (delete, delete + warn, timeout with duration) and its own exempt roles and channels.
- AutoMod warnings feed the 1a ladder.
- Messages are visible briefly before deletion; this is accepted.
- Regex rules must be guarded against pathological patterns (use the `regex` crate, which has no catastrophic backtracking, and cap pattern size).

### Link protection
- **All links blocked except allowlisted domains.** Exempt roles and channels configurable. Action is configurable like other AutoMod rules (default: delete).

### Raid protection
- **Trigger** (my call): a join-rate burst only (default 10 joins in 10 seconds, configurable). Suspicious-account signals on their own do not start raid mode; they go to alt detection.
- **Who counts as a raid joiner** (my call): the joins inside the window that tripped the burst, plus everyone who joins while raid mode is active.
- **Optional filter** (my call): admins may set criteria (account age below N days, default avatar) so kick/ban apply only to raid joiners who also match them; the others are quarantined instead.
- Responses, any combination chosen by admins and run together: **alert staff**, **quarantine joiners**, **auto lockdown**, **kick joiners**, **ban joiners**. Default: alert + quarantine only (kick/ban must be turned on explicitly — see anti-goals).
- **Raid mode ends** (my call) on `/raid end`, or automatically after 10 minutes (configurable) with no new burst. Ending lifts the auto lockdown and posts the list of raid-quarantined members for batch review. Raid state is stored in the database so a restart neither forgets nor extends it.

### Alt detection
- Score each join from signals: account age, default avatar, username/display name similar to a recently banned user, joined via the same invite shortly after a ban (requires invite tracking: cache invite use counts and diff on join).
- Admins set the score threshold and choose the response: **flag to staff** or **auto-quarantine**.
- Signal weights are the builder's call; make them adjustable in config.

### Quarantine
- A quarantine role that hides all channels except one review channel (and the verification channel). **The bot writes the overwrites itself** (my call): a View Channel deny for the quarantine role on every channel and category, an allow on the review/verification channels, and the same deny added whenever a channel is created. This works regardless of the server's existing layout, because quarantined members hold no other roles.
- On entry the bot saves and removes the member's other roles; on release it restores them (my call).
- Release depends on the reason:
  - verification pending/failed → released automatically when verification is passed;
  - raid or alt → staff **Approve** / **Deny** buttons (Deny = kick by default, configurable to ban).
- Quarantine state is in the database and re-applied if the member leaves and rejoins.

### Lockdown
- Admins configure the set of lockdown channels. `/lockdown start|end` affects those channels only. On start, the bot denies Send Messages (and thread/reaction sending, builder's call) for @everyone **and for every role with an explicit Send Messages allow on that channel**, except staff roles, since an allow on another role would otherwise beat the @everyone deny (my call). Prior overwrites are saved in the database so `end` restores them exactly. Optional message posted in each locked channel (my call).

### Health check
- `/healthcheck`, on demand only. Reports: dangerous permissions on @everyone and other roles, bot role position too low to moderate, number of Administrator roles/members, 2FA moderation requirement off, low verification level, missing log/mod channels in bot config.

## 7. Step 1c — Staff tools, modmail, appeals

### Standby Staff Mode
- Admins configure the **sub-staff roles**. Members with those roles may activate Standby **at any time** (whether or not regular staff are available). Admins may also activate it. Regular moderators may not.
- While active, sub-staff get **both**:
  - access to an admin-chosen set of the bot's mod commands, and
  - a temporary role with real Discord permissions (admin-configured).
- Ends on expiry (default 1 hour; the activator may choose a duration within an admin-set maximum) or `/standby off`.
- Expiry is stored in the database and processed at startup if missed, so the temporary role never outlives the session.
- **Activation logging:** every activation/deactivation is logged with who, when, duration, reason, and how it ended.

### Staff availability
- `/status available|busy|unavailable`.
- A **live status board**: one message in a staff channel that the bot keeps updated.
- Modmail, appeals and raid alerts **ping only staff marked available**; if nobody is available, ping the moderator role (my call).

### Staff handoff
- `/handoff add <note>`, `/handoff resolve <id>`, `/handoff list`. Open notes appear on the status board. When a staff member sets themselves available, the bot privately shows them the open notes (my call).

### Modmail
- A member DMs the bot → a **forum post** opens in a staff forum channel, tagged *open*.
- Member messages are relayed into the post automatically.
- **Staff messages are relayed only through `/reply`** (owner's decision). Plain messages in the post stay internal so staff can discuss the case. `/anon-reply` sends without the staff member's name (my call). `/modmail close` closes the post (tag *closed*) and notifies the member.
- **Blacklist:** `/modmail block|unblock @user`. Blocked users get one notice, then are ignored.

### Appeals
- The punishment DM from 1a carries an **Appeal** button (bans, tempbans, timeouts). It is sent before the ban so the DM channel exists.
- The button opens a modal form; answers go to the modmail forum as a post tagged *appeal*, with **Accept** / **Deny** buttons.
- Accept lifts the ban/timeout and **soft-removes only the appealed case** (owner's decision). Earlier warnings keep counting toward the ladder. Every outcome is DMed to the member and recorded as a case.
- One appeal per punishment, with a cooldown before re-appealing after a denial (my call; duration configurable).
- Blacklisted users cannot appeal.
- Known gap: members with DMs closed never receive the button. A separate "appeals server" fallback is out of scope for this milestone.

## 8. Cross-cutting

- **Scheduler:** one background task that polls the database for due jobs (tempban expiry, Standby expiry, appeal cooldowns) and runs at startup. Warn decay is computed at query time, not by a job.
- **Restart safety:** any state that matters after a restart lives in PostgreSQL.
- **Errors to users:** command failures (missing permission, hierarchy, Discord API error) reply ephemerally with a plain reason. Unexpected errors are logged to stdout with context.
- **Module layout:** one module per feature area so Sections 2–6 add modules without restructuring.

## 9. Testing

- Unit tests for pure logic: duration parsing, hierarchy/protected-role checks, ladder evaluation with and without decay, alt scoring, raid window detection, AutoMod rule matching, template rendering, lockdown overwrite save/restore.
- Database code tested against a local PostgreSQL.
- Each step is accepted by running it on a private test server before deploying to the live server.

## 10. Left to the builder

- Exact crate versions, project/module layout, database schema details.
- Scheduler polling interval.
- Default alt-signal weights and threshold; default AutoMod thresholds.
- Wording of default DM templates and log embeds.
- Message cache exact caps (within the memory constraint).
- Quiz auto-check vs staff review default.

## 11. Decisions marked "my call" (for owner review)

Key all data by `guild_id`; serenity + poise + sqlx stack; PostgreSQL over SQLite; build locally to a musl binary; mod permission = role **or** matching Discord permission; hierarchy check; protected members exempt from automatic actions; extra log categories (roles, voice, external actions); in-memory-only message cache; `/raid end` batch review; restore roles after quarantine; lockdown channel message; Standby default 1 hour; fallback ping to mod role; handoff notes shown on going available; modmail anonymous reply; appeal cooldown; raid kick/ban off by default; ladder fires on reaching N; Standby command subset as a hard limit; raid triggered by join burst only, with joiners = burst + everyone during raid mode, optional criteria filter for kick/ban, and auto-end after 10 quiet minutes; bot-written quarantine overwrites; existing members unaffected by enabling verification; lockdown also denies roles with explicit allows.

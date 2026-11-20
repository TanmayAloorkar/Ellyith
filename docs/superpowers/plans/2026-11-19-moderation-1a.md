# Moderation Step 1a (Core Moderation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Rust Discord bot that runs on the owner's server with step 1a of the moderation design: slash-command warn/timeout/kick/ban/tempban/unban, cases with soft delete, history, notes, `/mywarnings`, an auto-punishment ladder with optional warn decay, nickname set/reset and bad-name enforcement, voice moderation, configurable member DMs, and per-category logging.

**Architecture:** One binary crate with a library half (`src/lib.rs`) so logic is testable. Pure logic (durations, permission checks, ladder, templates, nickname rules, message cache, audit-log categorising) lives in plain functions with unit tests. Database access lives in small repository modules over `sqlx::PgPool`, tested with `#[sqlx::test]` against a local PostgreSQL. Discord-facing code (poise commands, the event handler, the scheduler) stays thin and calls into those layers; it is verified by hand on a private test server.

**Tech Stack:** Rust (stable), `poise` + `serenity` (use the latest poise release and the serenity version it depends on; at writing time poise 0.6 / serenity 0.12; API names below are from those), `tokio`, `sqlx` (PostgreSQL, runtime queries), `chrono`, `tracing`, `dotenvy`. Cross-compiled with `cargo-zigbuild` to `x86_64-unknown-linux-musl`.

**Spec:** `docs/superpowers/specs/2026-11-19-moderation-design.md` (sections 2, 3, 4, 5, 8, 9 apply to this plan; 6 and 7 are later steps).

## Global Constraints

- Host is Wispbyte: **512 MB RAM, 1 GB disk**, managed **PostgreSQL**. Every in-memory cache must be bounded. No large local files; all state in PostgreSQL.
- **Slash commands only.** No prefix commands.
- Mutes use **Discord timeout**; reject any timeout duration **over 28 days**.
- Bot token, database URL and guild ID come from **environment variables** (`DISCORD_TOKEN`, `DATABASE_URL`, `GUILD_ID`). `.env` is git-ignored and never committed.
- **Message content is never written to the database.** Only the bounded in-memory cache holds it.
- Every table is keyed by `guild_id`. Discord IDs are stored as `BIGINT` (`i64`, via `id.get() as i64`).
- Use runtime `sqlx::query` / `query_as` only, not the `query!` macros (no build-time database needed, simpler cross-compile). *(plan-level call)*
- TLS via rustls only (no OpenSSL), so the musl cross-compile needs no C TLS library.
- Protected-role members, the owner and Administrators are exempt from automatic actions (ladder, nickname filter).
- Removed cases are soft-deleted: kept, marked with remover and time, excluded from counts and `/mywarnings`.

## Review Focus

1. **A tempban lifted early** (via `/unban` or Discord's own unban) must cancel the pending auto-unban; a scheduled unban that hits "Unknown Ban" marks the job done instead of retrying forever. Test in Task 9.
2. **A target who is not in the server**: `/warn`, `/ban`, `/tempban`, `/unban` work by user; `/timeout`, `/untimeout`, `/kick`, `/nick`, `/voice` reply "That user is not in this server." Test in Task 4 (`can_target` with no member).
3. **The bot cannot act** (its role is too low): refuse *before* the DM is sent, with a plain reason, and create no case. Test in Task 4 (`TargetNotBelowBot`).
4. **Very long reasons** (over 512 chars for the audit-log reason, 1024 for an embed field, 2000 for a DM) are truncated on a character boundary, never causing an API error or panic. Test in Task 2.
5. **Nickname reset loop**: a fallback nickname that itself breaks the filter, or a filtered word that matches the fallback, is rejected at config time. Test in Task 11.

---

## File structure

```
Cargo.toml
.gitignore                      target/, .env
.env.example                    DISCORD_TOKEN=, DATABASE_URL=, GUILD_ID=, RUST_LOG=info
migrations/
  0001_settings.sql             guild_settings, ladder_rules          (Task 3)
  0002_cases.sql                cases, case_counters                  (Task 5)
  0003_log_channels.sql         log_channels                          (Task 6)
  0004_dm_templates.sql         dm_templates                          (Task 7)
  0005_jobs.sql                 scheduled_jobs                        (Task 9)
  0006_notes.sql                notes                                 (Task 10)
src/
  main.rs                       env, pool, migrate, framework, intents, scheduler spawn
  lib.rs                        Data, Error, Context, UserError, module tree, all_commands()
  text.rs                       truncate()                                         (Task 2)
  duration.rs                   parse_duration(), format_duration(), MAX_TIMEOUT    (Task 2)
  settings.rs                   GuildSettings repo                                 (Task 3)
  events.rs                     single poise event handler, dispatches to modules (Task 1, grows)
  scheduler.rs                  job loop                                           (Task 9)
  moderation/
    mod.rs
    ladder.rs                   LadderRule, rule_for_count(), validate_rule(), repo (Task 3)
    perms.rs                    MemberFacts, Denied, can_moderate(), can_target(), authorize() (Task 4)
    cases.rs                    CaseKind, Actor, NewCase, Case, repo               (Task 5)
    templates.rs                render(), default_template(), repo                 (Task 7)
    actions.rs                  execute(), record_case(), apply_ladder()           (Task 8)
    jobs.rs                     scheduled job repo                                 (Task 9)
    notes.rs                    notes repo                                         (Task 10)
    nickname.rs                 check_name(), enforce()                            (Task 11)
    commands/
      mod.rs
      config.rs                 /config … (grows across tasks)
      punish.rs                 /warn /timeout /untimeout /kick /ban /tempban /unban (Tasks 8, 9)
      cases.rs                  /case /history /note /mywarnings                    (Task 10)
      nick.rs                   /nick                                               (Task 11)
      voice.rs                  /voice                                              (Task 12)
  logging/
    mod.rs                      LogCategory, repo, send_log(), case_embed()          (Task 6)
    cache.rs                    MessageCache                                         (Task 13)
    messages.rs                 edit/delete/bulk-delete logs                         (Task 13)
    audit.rs                    categorize(), audit-log/join/leave/voice logs        (Task 14)
tests/
  settings.rs  cases.rs  logs.rs  templates.rs  jobs.rs  notes.rs   (#[sqlx::test] DB tests)
```

Shared types in `src/lib.rs` (every task uses these):

```rust
#[derive(Clone)]
pub struct Data {
    pub db: sqlx::PgPool,
    pub msg_cache: std::sync::Arc<logging::cache::MessageCache>, // added in Task 13; before that, Data holds only db
}
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// An error whose message is safe and meant to be shown to the command user.
#[derive(Debug)]
pub struct UserError(pub String);   // impl Display + std::error::Error
```

DB tests use `#[sqlx::test(migrations = "./migrations")] async fn name(pool: PgPool)`; they need `DATABASE_URL` set to a local Postgres role allowed to create databases. Run all tests with `cargo test`.

---

### Task 1: Toolchain, project skeleton, bot online

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `.env.example`, `src/main.rs`, `src/lib.rs`, `src/events.rs`, `migrations/` (empty dir with `.gitkeep`)

**Interfaces:**
- Produces: `Data`, `Error`, `Context`, `UserError` (above); `pub fn all_commands() -> Vec<poise::Command<Data, Error>>` in `lib.rs` (later tasks append); `pub async fn event_handler(ctx: &serenity::Context, event: &serenity::FullEvent, framework: poise::FrameworkContext<'_, Data, Error>, data: &Data) -> Result<(), Error>` in `events.rs` (later tasks add match arms).

- [ ] **Step 1: Install tools (ask the owner before installing anything)**

Nothing is installed on the PC. Install: `rustup` (stable, MSVC host), PostgreSQL 16+ for Windows (local, for tests), `zig` and `cargo install cargo-zigbuild` (for Task 15), then `rustup target add x86_64-unknown-linux-musl`.
Verify: `cargo --version`, `psql --version`, `zig version`, `cargo zigbuild --version` all print versions.

- [ ] **Step 2: Create the Discord application (owner does this in the browser)**

In the Discord developer portal: create an application and bot, enable the **Server Members** and **Message Content** privileged intents, copy the token. Invite it to a **private test server** with permissions: View Audit Log, Manage Nicknames, Kick Members, Ban Members, Moderate Members, Move Members, Mute Members, Manage Messages, Send Messages, Embed Links, View Channels. Put `DISCORD_TOKEN`, `GUILD_ID` (test server), `DATABASE_URL` in `.env`.

- [ ] **Step 3: `git init`, `.gitignore` (`/target`, `.env`), `Cargo.toml`**

Package name `aio-bot`, edition 2021. Dependencies: `poise`, `tokio` (`macros`, `rt-multi-thread`), `sqlx` (`runtime-tokio`, `tls-rustls`, `postgres`, `chrono`, `migrate`), `chrono`, `tracing`, `tracing-subscriber` (`env-filter`), `dotenvy`. `[profile.release] strip = true, lto = "thin"`.

- [ ] **Step 4: Write `main.rs`**

Load `.env` (ignore if missing), init tracing from `RUST_LOG`, read the three env vars (exit with a clear message naming the missing one), `PgPoolOptions::new().max_connections(5)`, `sqlx::migrate!("./migrations").run(&pool)`, build the poise framework with `all_commands()`, `event_handler`, and an `on_error` that: for a `UserError` replies ephemerally with its text; otherwise logs with `tracing::error!` and replies ephemerally "Something went wrong. The error was logged.". In `setup`, register commands with `poise::builtins::register_in_guild` for `GUILD_ID`. Intents: `GUILDS | GUILD_MEMBERS | GUILD_MESSAGES | MESSAGE_CONTENT | GUILD_MODERATION | GUILD_VOICE_STATES`.

- [ ] **Step 5: Add a `/ping` command in `lib.rs` that replies "Pong"; run the bot**

Run: `cargo run`
Expected: log line showing the bot connected; `/ping` in the test server replies "Pong".

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "chore: project skeleton, bot connects and answers /ping"
```

---

### Task 2: Text truncation and duration parsing

**Files:**
- Create: `src/text.rs`, `src/duration.rs` (unit tests inline)

**Interfaces:**
- Produces: `pub fn truncate(s: &str, max_chars: usize) -> String` (appends `…` when cut; result ≤ `max_chars` chars). `pub fn parse_duration(s: &str) -> Result<chrono::Duration, UserError>`, `pub fn format_duration(d: chrono::Duration) -> String`, `pub const MAX_TIMEOUT_SECS: i64 = 28 * 24 * 3600;`

- [ ] **Step 1: Write failing tests**

```rust
// text.rs
#[test] fn short_is_unchanged() { assert_eq!(truncate("abc", 5), "abc"); }
#[test] fn cuts_with_ellipsis() { assert_eq!(truncate("abcdef", 4), "abc…"); }
#[test] fn multibyte_never_panics() { assert_eq!(truncate("ééééé", 3), "éé…"); }

// duration.rs
#[test] fn parses_units() {
    assert_eq!(parse_duration("90s").unwrap().num_seconds(), 90);
    assert_eq!(parse_duration("1h30m").unwrap().num_seconds(), 5400);
    assert_eq!(parse_duration("2D").unwrap().num_seconds(), 172_800);
    assert_eq!(parse_duration("1w").unwrap().num_seconds(), 604_800);
}
#[test] fn rejects_bad() {
    for s in ["", "0m", "10", "5x", "h", "-1h", "99999999999999w"] { assert!(parse_duration(s).is_err(), "{s}"); }
}
#[test] fn formats() {
    assert_eq!(format_duration(chrono::Duration::seconds(5400)), "1h 30m");
    assert_eq!(format_duration(chrono::Duration::days(7)), "7d");
}
```

- [ ] **Step 2: Run `cargo test` — expect compile failure (functions missing)**

- [ ] **Step 3: Implement** — units `s m h d w`, case-insensitive, concatenated number+unit pairs, no spaces required (trim outer whitespace); zero total or overflow is an error. Error text: `Invalid duration "<input>". Use forms like 30m, 2h, 1d12h, 1w.`

- [ ] **Step 4: Run `cargo test` — expect PASS**

- [ ] **Step 5: Commit** — `feat: duration parsing and safe text truncation`

---

### Task 3: Guild settings and ladder rules

**Files:**
- Create: `migrations/0001_settings.sql`, `src/settings.rs`, `src/moderation/mod.rs`, `src/moderation/ladder.rs`, `tests/settings.rs`

**Interfaces:**
- Produces:
  - `pub struct GuildSettings { pub guild_id: i64, pub mod_roles: Vec<i64>, pub protected_roles: Vec<i64>, pub dm_show_moderator: bool, pub warn_decay_enabled: bool, pub warn_decay_days: i32, pub filtered_words: Vec<String>, pub hoist_chars: String, pub nick_fallback: String, pub nick_filter_enabled: bool }` (`sqlx::FromRow`, `Clone`)
  - `pub async fn get_or_create(db: &PgPool, guild_id: i64) -> Result<GuildSettings, sqlx::Error>` — on first creation, also inserts the default ladder, in one transaction.
  - `pub async fn save(db: &PgPool, s: &GuildSettings) -> Result<(), sqlx::Error>` — full-row update.
  - `pub enum LadderAction { Timeout, Kick, Tempban, Ban }` stored as `'timeout'|'kick'|'tempban'|'ban'`; `pub struct LadderRule { pub warn_count: i32, pub action: LadderAction, pub duration_secs: Option<i64> }`
  - `pub fn rule_for_count(rules: &[LadderRule], active_warns: i64) -> Option<&LadderRule>`
  - `pub fn validate_rule(r: &LadderRule) -> Result<(), UserError>`
  - `pub async fn list_rules(db, guild_id) -> Result<Vec<LadderRule>, sqlx::Error>` (ordered by `warn_count`), `set_rule(db, guild_id, &LadderRule)` (upsert on `(guild_id, warn_count)`), `remove_rule(db, guild_id, warn_count) -> Result<bool, _>`

Schema: `guild_settings` with the columns above and defaults `mod_roles '{}'`, `protected_roles '{}'`, `dm_show_moderator false`, `warn_decay_enabled false`, `warn_decay_days 30`, `filtered_words '{}'`, `hoist_chars '!'`, `nick_fallback 'Moderated Nickname'`, `nick_filter_enabled true`. `ladder_rules(guild_id BIGINT, warn_count INT CHECK (warn_count >= 1), action TEXT CHECK (action IN (...)), duration_secs BIGINT NULL, PRIMARY KEY (guild_id, warn_count))`.

- [ ] **Step 1: Failing tests**

```rust
// ladder.rs unit tests
let rules = default_rules();  // pub fn default_rules() -> Vec<LadderRule>
assert_eq!(rule_for_count(&rules, 3).map(|r| r.action), Some(LadderAction::Timeout));
assert_eq!(rules[0].duration_secs, Some(3600));
assert_eq!(rule_for_count(&rules, 4), None);          // fires only on reaching N
assert_eq!(rule_for_count(&rules, 5).map(|r| r.action), Some(LadderAction::Kick));
assert_eq!(rule_for_count(&rules, 7).map(|r| r.action), Some(LadderAction::Ban));
// validate_rule
assert!(validate_rule(&LadderRule{warn_count:2, action:LadderAction::Timeout, duration_secs:None}).is_err());
assert!(validate_rule(&LadderRule{warn_count:2, action:LadderAction::Timeout, duration_secs:Some(MAX_TIMEOUT_SECS+1)}).is_err());
assert!(validate_rule(&LadderRule{warn_count:2, action:LadderAction::Kick, duration_secs:Some(60)}).is_err());
assert!(validate_rule(&LadderRule{warn_count:0, action:LadderAction::Ban, duration_secs:None}).is_err());
assert!(validate_rule(&LadderRule{warn_count:2, action:LadderAction::Tempban, duration_secs:Some(86400)}).is_ok());

// tests/settings.rs
#[sqlx::test(migrations = "./migrations")]
async fn first_get_creates_defaults_and_ladder(pool: PgPool) {
    let s = get_or_create(&pool, 1).await.unwrap();
    assert!(!s.warn_decay_enabled); assert_eq!(s.warn_decay_days, 30); assert_eq!(s.hoist_chars, "!");
    assert_eq!(list_rules(&pool, 1).await.unwrap().len(), 3);
}
#[sqlx::test(migrations = "./migrations")]
async fn ladder_not_reseeded_after_admin_clears_it(pool: PgPool) {
    get_or_create(&pool, 1).await.unwrap();
    for n in [3, 5, 7] { remove_rule(&pool, 1, n).await.unwrap(); }
    get_or_create(&pool, 1).await.unwrap();
    assert!(list_rules(&pool, 1).await.unwrap().is_empty());
}
#[sqlx::test(migrations = "./migrations")]
async fn save_round_trips(pool: PgPool) { /* set mod_roles=[10,11], decay on 14d; save; reload; assert equal */ }
```

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement migration, `settings.rs`, `ladder.rs`** — seed only when `INSERT … ON CONFLICT DO NOTHING` actually inserted the settings row.
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Commit** — `feat: guild settings and auto-punishment ladder storage`

---

### Task 4: Permission checks and `/config modroles|protected`

**Files:**
- Create: `src/moderation/perms.rs`, `src/moderation/commands/mod.rs`, `src/moderation/commands/config.rs`
- Modify: `src/lib.rs` (register `/config`)

**Interfaces:**
- Consumes: `GuildSettings`, `get_or_create`, `save` (Task 3).
- Produces:
  - `pub struct MemberFacts { pub user_id: u64, pub is_owner: bool, pub permissions: serenity::Permissions, pub role_ids: Vec<u64>, pub top_role_position: u16 }`
  - `pub enum Denied { NotModerator, TargetIsSelf, TargetIsOwner, TargetProtected, TargetNotBelowYou, TargetNotBelowBot }` with `pub fn message(&self) -> &'static str`
  - `pub fn can_moderate(invoker: &MemberFacts, s: &GuildSettings, required: Permissions) -> bool` — owner, or `ADMINISTRATOR`, or `permissions.contains(required)`, or holds a `mod_roles` role.
  - `pub fn can_target(invoker: &MemberFacts, target: Option<&MemberFacts>, bot: &MemberFacts, s: &GuildSettings) -> Result<(), Denied>` — `None` target (not in server) is `Ok`. Order: self, owner, protected (bypassed by owner/admin invoker), target top ≥ invoker top (unless invoker is owner), target top ≥ bot top.
  - `pub fn is_exempt_from_auto(m: &MemberFacts, s: &GuildSettings) -> bool` — owner, admin, or protected role.
  - `pub async fn member_facts(ctx: &serenity::Context, guild_id: GuildId, member: &Member) -> Result<MemberFacts, Error>` — from the guild cache: owner check, guild-level permissions, highest role position.
  - `pub async fn authorize(ctx: Context<'_>, required: Permissions, target: Option<UserId>) -> Result<GuildSettings, Error>` — fetches invoker, bot and (if in server) target facts; returns `UserError(denied.message())` on refusal.
  - `pub async fn require_member(ctx: Context<'_>, user: UserId) -> Result<Member, Error>` — `UserError("That user is not in this server.")` if absent.

- [ ] **Step 1: Failing unit tests (pure)**

```rust
fn facts(id: u64, top: u16) -> MemberFacts { MemberFacts { user_id: id, is_owner: false, permissions: Permissions::empty(), role_ids: vec![], top_role_position: top } }
// can_moderate
assert!(!can_moderate(&facts(1,5), &s, Permissions::BAN_MEMBERS));
assert!(can_moderate(&MemberFacts{permissions: Permissions::BAN_MEMBERS, ..facts(1,5)}, &s, Permissions::BAN_MEMBERS));
assert!(can_moderate(&MemberFacts{role_ids: vec![42], ..facts(1,5)}, &GuildSettings{mod_roles: vec![42], ..s.clone()}, Permissions::BAN_MEMBERS));
// can_target
let bot = facts(99, 10);
assert_eq!(can_target(&facts(1,5), None, &bot, &s), Ok(()));                       // Review Focus 2
assert_eq!(can_target(&facts(1,5), Some(&facts(1,5)), &bot, &s), Err(Denied::TargetIsSelf));
assert_eq!(can_target(&facts(1,5), Some(&facts(2,5)), &bot, &s), Err(Denied::TargetNotBelowYou));
assert_eq!(can_target(&facts(1,20), Some(&facts(2,12)), &bot, &s), Err(Denied::TargetNotBelowBot)); // Review Focus 3
let prot = GuildSettings{protected_roles: vec![7], ..s.clone()};
assert_eq!(can_target(&facts(1,5), Some(&MemberFacts{role_ids: vec![7], ..facts(2,1)}), &bot, &prot), Err(Denied::TargetProtected));
let admin = MemberFacts{permissions: Permissions::ADMINISTRATOR, ..facts(1,5)};
assert_eq!(can_target(&admin, Some(&MemberFacts{role_ids: vec![7], ..facts(2,1)}), &bot, &prot), Ok(()));
assert_eq!(can_target(&facts(1,5), Some(&MemberFacts{is_owner: true, ..facts(2,1)}), &bot, &s), Err(Denied::TargetIsOwner));
```
(`s` = a `GuildSettings` with defaults; add `impl GuildSettings { pub fn defaults(guild_id: i64) -> Self }` in Task 3's file if convenient.)

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement `perms.rs`**
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Add `/config`** — parent command with `default_member_permissions = "MANAGE_GUILD"` and a check requiring Manage Server (or owner). Subcommand groups `modroles add|remove|list <role>` and `protected add|remove|list <role>`; each loads settings, edits the `Vec`, saves, replies ephemerally. *(plan-level call: `/config` is Manage Server only.)*
- [ ] **Step 6: Manual check** — on the test server, `/config modroles add @Mod` then `/config modroles list` shows it; a member without Manage Server cannot run `/config`.
- [ ] **Step 7: Commit** — `feat: moderator/protected roles and permission checks`

---

### Task 5: Cases repository

**Files:**
- Create: `migrations/0002_cases.sql`, `src/moderation/cases.rs`, `tests/cases.rs`

**Interfaces:**
- Produces:
  - `pub enum CaseKind { Warn, Timeout, Untimeout, Kick, Ban, Tempban, Unban, NickSet, NickReset, VoiceDisconnect, VoiceMute, VoiceUnmute, VoiceMove }` with `as_str()` (snake_case) / `FromStr`, and `pub fn label(&self) -> &'static str` ("Warning", "Timeout", …).
  - `pub enum Actor { Moderator(i64), AutoPunishment, System }` → columns `actor TEXT ('moderator'|'auto_punishment'|'system')`, `moderator_id BIGINT NULL`.
  - `pub struct NewCase { pub guild_id: i64, pub kind: CaseKind, pub target_id: i64, pub actor: Actor, pub reason: Option<String>, pub duration_secs: Option<i64>, pub expires_at: Option<DateTime<Utc>>, pub dm_failed: bool }`
  - `pub struct Case { pub guild_id, pub case_number: i32, pub kind, pub target_id, pub actor, pub reason, pub duration_secs, pub created_at: DateTime<Utc>, pub expires_at, pub dm_failed, pub removed_at: Option<DateTime<Utc>>, pub removed_by: Option<i64> }`
  - `pub async fn reserve_number(db, guild_id) -> Result<i32, sqlx::Error>` — atomic: `INSERT INTO case_counters (guild_id, next) VALUES ($1, 2) ON CONFLICT (guild_id) DO UPDATE SET next = case_counters.next + 1 RETURNING next - 1`.
  - `pub async fn insert(db, number: i32, c: &NewCase) -> Result<Case, sqlx::Error>`; `pub async fn create(db, c: &NewCase) -> Result<Case, _>` (reserve + insert).
  - `get(db, guild_id, number) -> Option<Case>`, `set_reason(db, guild_id, number, reason) -> Option<Case>`, `soft_remove(db, guild_id, number, by: i64) -> Option<Case>` (None if missing **or already removed**), `history(db, guild_id, target_id, include_removed: bool) -> Vec<Case>` (newest first), `active_warn_count(db, guild_id, target_id, decay_days: Option<i32>) -> i64`.

- [ ] **Step 1: Failing DB tests (`tests/cases.rs`)**

```rust
#[sqlx::test(migrations = "./migrations")]
async fn numbers_are_sequential_per_guild(pool: PgPool) {
    assert_eq!(create(&pool, &warn(1, 100)).await.unwrap().case_number, 1);
    assert_eq!(create(&pool, &warn(1, 100)).await.unwrap().case_number, 2);
    assert_eq!(create(&pool, &warn(2, 100)).await.unwrap().case_number, 1);
}
#[sqlx::test(migrations = "./migrations")]
async fn soft_remove_hides_from_count_but_not_history(pool: PgPool) {
    create(&pool, &warn(1, 100)).await.unwrap(); create(&pool, &warn(1, 100)).await.unwrap();
    assert!(soft_remove(&pool, 1, 1, 555).await.unwrap().is_some());
    assert!(soft_remove(&pool, 1, 1, 555).await.unwrap().is_none());     // already removed
    assert_eq!(active_warn_count(&pool, 1, 100, None).await.unwrap(), 1);
    assert_eq!(history(&pool, 1, 100, true).await.unwrap().len(), 2);
    assert_eq!(history(&pool, 1, 100, false).await.unwrap().len(), 1);
}
#[sqlx::test(migrations = "./migrations")]
async fn decay_excludes_old_warns(pool: PgPool) {
    create(&pool, &warn(1, 100)).await.unwrap();
    sqlx::query("UPDATE cases SET created_at = now() - interval '40 days' WHERE case_number = 1").execute(&pool).await.unwrap();
    create(&pool, &warn(1, 100)).await.unwrap();
    assert_eq!(active_warn_count(&pool, 1, 100, Some(30)).await.unwrap(), 1);
    assert_eq!(active_warn_count(&pool, 1, 100, None).await.unwrap(), 2);
}
#[sqlx::test(migrations = "./migrations")]
async fn only_warns_count(pool: PgPool) { /* one Warn + one Kick → active_warn_count == 1 */ }
```
(`warn(guild, target)` = test helper building a `NewCase` with `kind: Warn, actor: Moderator(1)`.)

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement migration + `cases.rs`** — primary key `(guild_id, case_number)`; index on `(guild_id, target_id)`.
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Commit** — `feat: case storage with numbering, soft delete and warn decay`

---

### Task 6: Log channels and mod-action logging

**Files:**
- Create: `migrations/0003_log_channels.sql`, `src/logging/mod.rs`, `tests/logs.rs`
- Modify: `src/moderation/commands/config.rs` (add `logs` group)

**Interfaces:**
- Consumes: `Case`, `CaseKind::label` (Task 5); `truncate` (Task 2).
- Produces:
  - `pub enum LogCategory { ModActions, Messages, JoinsLeaves, MemberUpdates, Channels, Roles, Voice, ExternalMod }`, `as_str()` → `"mod_actions" "messages" "joins_leaves" "member_updates" "channels" "roles" "voice" "external_mod"`, plus `ALL: [LogCategory; 8]` and poise `ChoiceParameter` derive for the command option.
  - `set_channel(db, guild_id, cat, channel_id: i64)`, `clear(db, guild_id, cat) -> bool`, `list(db, guild_id) -> Vec<(LogCategory, i64)>`, `channel_for(db, guild_id, cat) -> Option<i64>`
  - `pub async fn send_log(http: &serenity::Http, db: &PgPool, guild_id: GuildId, cat: LogCategory, embed: CreateEmbed)` — returns `()`; no-op if the category is off; send errors are logged with `tracing::warn!`, never propagated.
  - `pub enum CaseEvent { Created, ReasonEdited, Removed { by: i64 } }`; `pub fn case_embed(c: &Case, ev: CaseEvent) -> CreateEmbed` — title `Case #N · <label>`, fields target (mention + id), moderator (mention or "Auto punishment"/"System"), reason (truncated 1024, "No reason given" if none), duration if any, "DM failed" note if `dm_failed`.

- [ ] **Step 1: Failing DB test** — `set_channel` then `channel_for` returns it; set again (upsert) replaces; `clear` returns true then `channel_for` is `None`; categories are independent.
- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement**; `log_channels(guild_id, category TEXT, channel_id BIGINT, PRIMARY KEY (guild_id, category))`.
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Add `/config logs set <category> <channel>`, `/config logs disable <category>`, `/config logs list`.**
- [ ] **Step 6: Commit** — `feat: per-category log channels`

---

### Task 7: Member DM templates

**Files:**
- Create: `migrations/0004_dm_templates.sql`, `src/moderation/templates.rs`, `tests/templates.rs`
- Modify: `src/moderation/commands/config.rs` (add `dm` group)

**Interfaces:**
- Consumes: `CaseKind` (Task 5), `truncate` (Task 2).
- Produces:
  - `pub struct TemplateVars<'a> { pub action: &'a str, pub reason: &'a str, pub duration: Option<&'a str>, pub server: &'a str, pub case_number: i32, pub moderator: Option<&'a str> }` — `moderator` is `None` when `dm_show_moderator` is off.
  - `pub fn default_template(kind: CaseKind) -> &'static str` — defined for Warn, Timeout, Kick, Ban, Tempban only.
  - `pub fn render(template: &str, v: &TemplateVars) -> String` — replaces `{action} {reason} {duration} {server} {case} {moderator}`; `{duration}` → "" when None; `{moderator}` → "Staff" when None; if `moderator` is Some and the template lacks `{moderator}`, appends `\nModerator: <name>`; result truncated to 2000 chars.
  - `pub fn dm_kinds() -> &'static [CaseKind]` → `[Warn, Timeout, Kick, Ban, Tempban]`.
  - repo: `get(db, guild_id, kind) -> Option<String>`, `set(db, guild_id, kind, text)`, `reset(db, guild_id, kind)`.

Default texts (exact):
- Warn: `You were warned in **{server}**.\nReason: {reason}\nCase #{case}`
- Timeout: `You were timed out in **{server}** for {duration}.\nReason: {reason}\nCase #{case}`
- Kick: `You were kicked from **{server}**.\nReason: {reason}\nCase #{case}`
- Ban: `You were banned from **{server}**.\nReason: {reason}\nCase #{case}`
- Tempban: `You were banned from **{server}** for {duration}.\nReason: {reason}\nCase #{case}`

- [ ] **Step 1: Failing unit tests**

```rust
let v = TemplateVars { action: "Warning", reason: "spam", duration: None, server: "Den", case_number: 4, moderator: None };
assert_eq!(render(default_template(CaseKind::Warn), &v), "You were warned in **Den**.\nReason: spam\nCase #4");
assert_eq!(render("{moderator}", &v), "Staff");
assert_eq!(render("Hi", &TemplateVars{moderator: Some("Tan"), ..v}), "Hi\nModerator: Tan");
assert_eq!(render("{reason}", &TemplateVars{reason: &"x".repeat(3000), ..v}).chars().count(), 2000);
```
Plus a DB test: `get` is `None`, `set` then `get` returns text, `reset` then `None`.

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement**; `dm_templates(guild_id, kind TEXT, template TEXT, PRIMARY KEY (guild_id, kind))`.
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Add `/config dm template set <kind> <text>`, `/config dm template reset <kind>`, `/config dm template view <kind>`, `/config dm showmod <on|off>`.** `<kind>` limited to `dm_kinds()`; `set` rejects text over 1500 chars.
- [ ] **Step 6: Commit** — `feat: configurable punishment DM templates`

---

### Task 8: Punishment pipeline, punishment commands, ladder

**Files:**
- Create: `src/moderation/actions.rs`, `src/moderation/commands/punish.rs`
- Modify: `src/moderation/commands/config.rs` (add `ladder` and `decay` groups), `src/lib.rs` (register)

**Interfaces:**
- Consumes: Tasks 2–7.
- Produces:
  - `pub struct ActionRequest { pub guild_id: GuildId, pub target: UserId, pub kind: CaseKind, pub actor: Actor, pub reason: Option<String>, pub duration: Option<chrono::Duration>, pub ban_delete_days: u8 }`
  - `pub async fn execute(ctx: &serenity::Context, data: &Data, req: ActionRequest) -> Result<Case, Error>`
  - `pub async fn record_case(ctx: &serenity::Context, data: &Data, number: Option<i32>, c: NewCase) -> Result<Case, Error>` — insert (reserving a number if `None`) and `send_log(ModActions, case_embed(.., Created))`.
  - `pub async fn apply_ladder(ctx: &serenity::Context, data: &Data, guild_id: GuildId, target: UserId) -> Result<Option<Case>, Error>`

`execute` order (callers have already run `authorize`):
1. If `kind ∈ dm_kinds()`: `reserve_number`, render the guild's template (or default) with `dm_show_moderator`, send the DM; DM failure → `dm_failed = true`, continue.
2. Apply on Discord, with the audit-log reason `truncate(reason, 512)`: Warn → nothing; Timeout → `disable_communication_until` now+duration; Untimeout → `enable_communication`; Kick; Ban (`ban_delete_days` 0–7); Unban. (Tempban arrives in Task 9.) On Discord error: return `UserError` with a plain reason ("I don't have permission to do that." for 403), **create no case**.
3. `record_case`.
4. If `kind == Warn`: `apply_ladder`; its failure is logged to ModActions as text, not returned to the moderator as an error.

`apply_ladder`: load settings; if target is a member and `is_exempt_from_auto` → `Ok(None)`; `active_warn_count` with `decay_days = warn_decay_enabled.then_some(warn_decay_days)`; `rule_for_count`; map the rule to an `ActionRequest` with `Actor::AutoPunishment`, reason `Reached N warnings`, and call `execute` (use `Box::pin` for the async recursion). A rule never maps to `Warn`, so there is no loop.

Commands (all `guild_only`, ephemeral confirmation "Case #N: <label> applied to <user>." with "(DM could not be delivered)" when `dm_failed`):

| Command | Options | Required permission | Target must be member |
|---|---|---|---|
| `/warn` | user, reason | MODERATE_MEMBERS | no |
| `/timeout` | user, duration, reason? | MODERATE_MEMBERS | yes; duration ≤ 28d |
| `/untimeout` | user, reason? | MODERATE_MEMBERS | yes |
| `/kick` | user, reason? | KICK_MEMBERS | yes |
| `/ban` | user, reason?, delete_days? (0–7, default 0) | BAN_MEMBERS | no |
| `/unban` | user (accepts ID), reason? | BAN_MEMBERS | no |

Config: `/config ladder set <warns> <action> [duration]` (validated by `validate_rule`), `/config ladder remove <warns>`, `/config ladder list`; `/config decay on <days 1–365>`, `/config decay off`.

- [ ] **Step 1: Implement `actions.rs`, `punish.rs`, config groups**
- [ ] **Step 2: `cargo build` — expect success, no warnings**
- [ ] **Step 3: Manual test on the test server** (use an alt account as target) — each expected result must hold:
  - `/warn alt spam` → DM received, case #1, mod-log embed (if `mod_actions` set).
  - 3rd `/warn` → a second case "Timeout · Auto punishment", alt timed out 1h.
  - `/timeout alt 29d` → refused with the duration error, no case.
  - Target a user ranked at or above you → refused with `TargetNotBelowYou` text, **no DM received** (Review Focus 3).
  - Move the bot's role below the alt's → `/kick alt` refused with `TargetNotBelowBot`, no DM, no case.
  - `/ban <ID of a user not in the server>` works; `/kick` on them replies "That user is not in this server." (Review Focus 2).
  - Give the alt a protected role → `/warn` refused; then from an Administrator account it works but the ladder does not fire.
  - `/warn` with a 3000-char reason → succeeds; log and DM truncated (Review Focus 4).
- [ ] **Step 4: Commit** — `feat: punishment commands with DMs, cases, logging and auto-punishment ladder`

---

### Task 9: Tempban and the scheduler

**Files:**
- Create: `migrations/0005_jobs.sql`, `src/moderation/jobs.rs`, `src/scheduler.rs`, `tests/jobs.rs`
- Modify: `src/moderation/actions.rs` (Tempban, cancel on Ban/Unban), `src/moderation/commands/punish.rs` (`/tempban`), `src/events.rs` (`GuildBanRemoval`), `src/main.rs` (spawn scheduler in `setup`)

**Interfaces:**
- Consumes: `execute`, `record_case` (Task 8).
- Produces:
  - `pub enum JobKind { Unban }` (`'unban'`); `pub struct Job { pub id: i64, pub guild_id: i64, pub kind: JobKind, pub target_id: i64, pub run_at: DateTime<Utc> }`
  - `schedule(db, guild_id, kind, target_id, run_at) -> i64`, `due(db, now: DateTime<Utc>) -> Vec<Job>`, `mark_done(db, id)`, `cancel(db, guild_id, kind, target_id) -> u64` (rows affected)
  - `pub async fn run(ctx: serenity::Context, data: Data)` — loop on `tokio::time::interval(30s)` (first tick is immediate, so missed jobs run at startup).

Schema: `scheduled_jobs(id BIGSERIAL PK, guild_id BIGINT, kind TEXT, target_id BIGINT, run_at TIMESTAMPTZ, done BOOLEAN NOT NULL DEFAULT false)`, partial index on `run_at WHERE NOT done`.

Behaviour:
- `execute(Tempban)`: ban, then `cancel` existing unban jobs for the target, then `schedule(Unban, now + duration)`; case gets `expires_at`.
- `execute(Ban)` and `execute(Unban)`: `cancel` pending unban jobs for the target (Review Focus 1).
- `GuildBanRemoval` event: `cancel` pending unban jobs for that user.
- Scheduler, per due job: unban with audit reason "Tempban expired"; on success `record_case(Unban, Actor::System, reason "Tempban expired")` then `mark_done`; on HTTP 404 / Unknown Ban → `mark_done` without a case; on other errors leave it for the next tick and `tracing::warn!`.

- [ ] **Step 1: Failing DB tests**

```rust
#[sqlx::test(migrations = "./migrations")]
async fn due_returns_only_past_undone(pool: PgPool) {
    let now = Utc::now();
    schedule(&pool, 1, JobKind::Unban, 100, now - Duration::minutes(1)).await.unwrap();
    schedule(&pool, 1, JobKind::Unban, 101, now + Duration::hours(1)).await.unwrap();
    let d = due(&pool, now).await.unwrap();
    assert_eq!(d.len(), 1); assert_eq!(d[0].target_id, 100);
    mark_done(&pool, d[0].id).await.unwrap();
    assert!(due(&pool, now).await.unwrap().is_empty());
}
#[sqlx::test(migrations = "./migrations")]
async fn cancel_removes_pending_for_target_only(pool: PgPool) {
    let past = Utc::now() - Duration::minutes(1);
    schedule(&pool, 1, JobKind::Unban, 100, past).await.unwrap();
    schedule(&pool, 1, JobKind::Unban, 101, past).await.unwrap();
    assert_eq!(cancel(&pool, 1, JobKind::Unban, 100).await.unwrap(), 1);
    let d = due(&pool, Utc::now()).await.unwrap();
    assert_eq!(d.len(), 1); assert_eq!(d[0].target_id, 101);
}
```

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement** (`cancel` sets `done = true`).
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Add `/tempban user duration reason?` (BAN_MEMBERS, target need not be a member).**
- [ ] **Step 6: Manual test** — `/tempban alt 2m` → banned, unbanned within ~2.5 min, Unban case by System logged. `/tempban alt 5m`, stop the bot, wait 6 min, start it → unbanned right after startup. `/tempban alt 10m` then `/unban alt` → no second unban case later. `/tempban alt 10m` then unban via Discord's UI → same.
- [ ] **Step 7: Commit** — `feat: tempbans with restart-safe scheduled unban`

---

### Task 10: Case management, history, notes, `/mywarnings`

**Files:**
- Create: `migrations/0006_notes.sql`, `src/moderation/notes.rs`, `src/moderation/commands/cases.rs`, `tests/notes.rs`
- Modify: `src/lib.rs` (register)

**Interfaces:**
- Consumes: cases repo (Task 5), `case_embed`/`send_log` (Task 6), `authorize` (Task 4).
- Produces: `pub struct Note { pub id: i64, pub guild_id: i64, pub target_id: i64, pub author_id: i64, pub content: String, pub created_at: DateTime<Utc> }`; `add(db, guild_id, target_id, author_id, content) -> Note`, `list(db, guild_id, target_id) -> Vec<Note>` (newest first), `remove(db, guild_id, id) -> bool` (hard delete; plan-level call — notes are not cases).

Commands (staff = `can_moderate` with MODERATE_MEMBERS; all replies ephemeral):
- `/case view <number>`; `/case reason <number> <text>` (logs `ReasonEdited`); `/case remove <number>` (soft delete, logs `Removed`; "Case not found or already removed." otherwise).
- `/history <user>` — newest 25 cases including removed ones (marked "Removed by @x"), header with total count and current active warning count (decay applied).
- `/note add <user> <text ≤ 1000 chars>`, `/note list <user>`, `/note remove <id>`.
- `/mywarnings` — any member; own non-removed cases (kind, reason, date; moderator shown only if `dm_show_moderator`), plus "Active warnings: N". Never notes, never removed cases.

- [ ] **Step 1: Failing DB test** — `add` twice, `list` returns 2 newest-first, `remove` true then false, notes from another guild not listed.
- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement repo and commands**
- [ ] **Step 4: `cargo test` — expect PASS; manual check** — `/case remove` on a warn lowers the active count shown by `/history` and hides it from the alt's `/mywarnings`; `/note list` is refused for a non-staff member.
- [ ] **Step 5: Commit** — `feat: case management, history, mod notes and /mywarnings`

---

### Task 11: Nicknames — commands and bad-name enforcement

**Files:**
- Create: `src/moderation/nickname.rs`, `src/moderation/commands/nick.rs`
- Modify: `src/moderation/commands/config.rs` (`nickfilter` group), `src/events.rs` (`GuildMemberAddition`, `GuildMemberUpdate`)

**Interfaces:**
- Consumes: `record_case` (Task 8), `is_exempt_from_auto`, `member_facts` (Task 4).
- Produces:
  - `pub enum NickViolation { FilteredWord, Hoisting, Unmentionable }`
  - `pub fn check_name(name: &str, filtered_words: &[String], hoist_chars: &str) -> Option<NickViolation>` — filtered word = case-insensitive substring; hoisting = first char is in `hoist_chars`; unmentionable = no ASCII alphanumeric character.
  - `pub fn validate_filter_config(fallback: &str, filtered_words: &[String], hoist_chars: &str) -> Result<(), UserError>` — the fallback must pass `check_name`.
  - `pub async fn enforce(ctx: &serenity::Context, data: &Data, member: &Member) -> Result<(), Error>` — skips if `nick_filter_enabled` is off, if the member is a bot, or `is_exempt_from_auto`; checks the display name (nick, else global name, else username); on violation sets the nick to `nick_fallback` and records a `NickReset` case with `Actor::System` and reason naming the violation.

- [ ] **Step 1: Failing unit tests**

```rust
let words = vec!["badword".to_string()];
assert_eq!(check_name("Tanmay", &words, "!"), None);
assert_eq!(check_name("!Tanmay", &words, "!"), Some(NickViolation::Hoisting));
assert_eq!(check_name("xBADWORDx", &words, "!"), Some(NickViolation::FilteredWord));
assert_eq!(check_name("ｔａｎ", &words, "!"), Some(NickViolation::Unmentionable));
assert_eq!(check_name("★★★", &words, "!"), Some(NickViolation::Unmentionable));
// Review Focus 5
assert!(validate_filter_config("Moderated Nickname", &words, "!").is_ok());
assert!(validate_filter_config("!Renamed", &words, "!").is_err());
assert!(validate_filter_config("Moderated Nickname", &["nick".to_string()], "!").is_err());
```

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement `nickname.rs`; wire `enforce` into the two events** (on update, only when the display name changed).
- [ ] **Step 4: `cargo test` — expect PASS**
- [ ] **Step 5: Commands** — `/nick set <user> <nickname ≤ 32>` and `/nick reset <user>` (MANAGE_NICKNAMES, member required, `authorize`); each records a `NickSet`/`NickReset` case (no DM). `/config nickfilter on|off`, `/config nickfilter words add|remove|list <word>`, `/config nickfilter hoist <chars>`, `/config nickfilter fallback <name>` — every change runs `validate_filter_config` before saving.
- [ ] **Step 6: Manual test** — alt sets nick `!aaa` → reset to "Moderated Nickname" with a case; adding the word `nick` to the filter is refused.
- [ ] **Step 7: Commit** — `feat: nickname commands and bad-name enforcement`

---

### Task 12: Voice moderation

**Files:**
- Create: `src/moderation/commands/voice.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `authorize`, `require_member` (Task 4), `record_case` (Task 8).

Commands (target must be a member **and** in a voice channel, else `UserError("That user is not in a voice channel.")`, read from the guild cache voice states):
- `/voice disconnect <user> [reason]` — MOVE_MEMBERS → `VoiceDisconnect`
- `/voice mute <user> [reason]` / `/voice unmute <user> [reason]` — MUTE_MEMBERS → `VoiceMute` / `VoiceUnmute` (server mute)
- `/voice move <user> <channel: voice only> [reason]` — MOVE_MEMBERS → `VoiceMove` (reason auto-includes the destination)

Each records a case (no DM).

- [ ] **Step 1: Implement**
- [ ] **Step 2: `cargo build` — expect success**
- [ ] **Step 3: Manual test** — each command on the alt in voice works and is logged; each on the alt out of voice gives the "not in a voice channel" reply and no case.
- [ ] **Step 4: Commit** — `feat: voice moderation commands`

---

### Task 13: Message cache and message logs

**Files:**
- Create: `src/logging/cache.rs`, `src/logging/messages.rs`
- Modify: `src/lib.rs` (`Data.msg_cache`), `src/main.rs` (construct), `src/events.rs` (`Message`, `MessageUpdate`, `MessageDelete`, `MessageDeleteBulk`)

**Interfaces:**
- Produces:
  - `pub struct CachedMessage { pub author_id: u64, pub channel_id: u64, pub content: String, pub attachment_urls: Vec<String> }`
  - `pub struct MessageCache` — `pub fn new(cap: usize, ttl: std::time::Duration) -> Self`; `insert(&self, id: u64, m: CachedMessage)`; `get(&self, id: u64) -> Option<CachedMessage>`; `update_content(&self, id: u64, content: String) -> Option<String>` (returns old content); `remove(&self, id: u64) -> Option<CachedMessage>`. Time-taking variants `insert_at`/`get_at`/`remove_at(.., now: Instant)` for tests. Interior `std::sync::Mutex<(HashMap<u64, (Instant, CachedMessage)>, VecDeque<u64>)>`; inserting past `cap` evicts the oldest; entries older than `ttl` read as absent.
  - Production values: `MessageCache::new(10_000, Duration::from_secs(24 * 3600))`.

Logging (category `Messages`; bots and DMs ignored):
- `Message` → `insert`.
- `MessageUpdate` → if content changed: log before (from cache, or "Not cached") / after, author, channel, jump link; `update_content`.
- `MessageDelete` → `remove`; log content (truncated 1024) or "Content not cached", author if known, channel.
- `MessageDeleteBulk` → one embed: count, channel, and up to 10 cached messages as `author: content` lines (each truncated to 100 chars).

- [ ] **Step 1: Failing unit tests**

```rust
let c = MessageCache::new(2, Duration::from_secs(60));
let t0 = Instant::now();
c.insert_at(1, msg("a"), t0); c.insert_at(2, msg("b"), t0); c.insert_at(3, msg("c"), t0);
assert!(c.get_at(1, t0).is_none());                          // evicted (cap)
assert_eq!(c.get_at(3, t0).unwrap().content, "c");
assert!(c.get_at(3, t0 + Duration::from_secs(61)).is_none()); // expired (ttl)
assert_eq!(c.update_content(2, "B".into()), Some("b".into()));
assert_eq!(c.remove(2).unwrap().content, "B");
assert!(c.remove(2).is_none());
```

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement cache, then message log handlers**
- [ ] **Step 4: `cargo test` — expect PASS; manual check** — with `/config logs set messages #logs`: edit and delete a message → both logged with old content; delete a message sent before the bot started → "Content not cached".
- [ ] **Step 5: Commit** — `feat: bounded message cache and message edit/delete logs`

---

### Task 14: Join/leave, voice and audit-log driven logs

**Files:**
- Create: `src/logging/audit.rs`
- Modify: `src/events.rs` (`GuildMemberAddition`, `GuildMemberRemoval`, `VoiceStateUpdate`, `GuildAuditLogEntryCreate`)

**Interfaces:**
- Consumes: `send_log` (Task 6).
- Produces: `pub fn categorize(action: &audit_log::Action, changes: &[audit_log::Change], executor_is_bot: bool) -> Option<LogCategory>`:
  - member nickname update, member role update → `MemberUpdates`
  - channel create/update/delete and channel overwrite create/update/delete → `Channels`
  - role create/update/delete → `Roles`
  - member kick, ban add, ban remove, or member update containing a `communication_disabled_until` change → `ExternalMod` **only if `!executor_is_bot`**; `None` if the bot did it (it is already in ModActions)
  - anything else → `None`

Logs:
- `JoinsLeaves`: join (mention, id, account created date and age in days), leave (mention, id, roles they had if cached).
- `Voice`: join channel / leave channel / move A → B.
- Audit categories: embed with action name, executor mention, target, and each change as `key: old → new` (truncated).

- [ ] **Step 1: Failing unit tests** (construct serenity audit-log `Action`/`Change` values)

```rust
assert_eq!(categorize(&Action::Member(MemberAction::Kick), &[], false), Some(LogCategory::ExternalMod));
assert_eq!(categorize(&Action::Member(MemberAction::Kick), &[], true), None);
assert_eq!(categorize(&Action::Member(MemberAction::RoleUpdate), &[], true), Some(LogCategory::MemberUpdates));
assert_eq!(categorize(&Action::Role(RoleAction::Create), &[], false), Some(LogCategory::Roles));
assert_eq!(categorize(&Action::Channel(ChannelAction::Delete), &[], false), Some(LogCategory::Channels));
// member update carrying a timeout change, by a human → ExternalMod; carrying only a nick change → MemberUpdates
```

- [ ] **Step 2: `cargo test` — expect FAIL**
- [ ] **Step 3: Implement `categorize` and the four event handlers** (executor-is-bot: compare the entry's `user_id` with `ctx.cache.current_user().id`).
- [ ] **Step 4: `cargo test` — expect PASS; manual check** — with every category set: join/leave the alt, hop voice channels, rename a channel, create a role, change the alt's nick and roles, and ban the alt from Discord's UI → each appears in its own channel; a ban via `/ban` appears only in mod actions.
- [ ] **Step 5: Commit** — `feat: join/leave, voice, member, channel, role and external-action logs`

---

### Task 15: Release build and deploy to Wispbyte

**Files:**
- Create: `DEPLOY.md` (the steps below, as run)

- [ ] **Step 1: Build** — `cargo zigbuild --release --target x86_64-unknown-linux-musl`. Expected: `target/x86_64-unknown-linux-musl/release/aio-bot` exists.
- [ ] **Step 2: Wispbyte setup (owner, in the panel)** — create the server with the **Rust** runtime; create the **PostgreSQL** database; set `DISCORD_TOKEN`, `DATABASE_URL` (Wispbyte's Postgres), `GUILD_ID` (**live** server), `RUST_LOG=info` as environment variables; upload the binary; set the startup command to run it.
- [ ] **Step 3: Start and verify** — the console shows migrations applied and the bot connected. If it fails with `Exec format error`, the host is ARM: rebuild with `--target aarch64-unknown-linux-musl` and re-upload.
- [ ] **Step 4: Live smoke test** — invite the bot to the live server with the Task 1 permissions, move its role above members it must moderate, then run: `/ping`, `/config logs set mod_actions #mod-log`, `/warn` an alt, `/case remove` it.
- [ ] **Step 5: Check memory** — panel shows RAM well under 512 MB after an hour.
- [ ] **Step 6: Commit** — `docs: deployment steps`

---

## Coverage check (spec §4, §5, §8 → tasks)

Mod roles / matching permission / owner-admin: 4 · hierarchy + protected + admin bypass: 4 · protected exempt from auto: 8, 11 · warn/timeout/untimeout/kick/ban/unban: 8 · 28-day cap: 2, 3, 8 · tempban + restart safety: 9 · nick set/reset + bad-name reset: 11 · voice: 12 · case view/reason/remove, soft delete: 5, 10 · history incl. removed: 10 · notes: 10 · `/mywarnings`: 10 · ladder default 3/5/7 + reach-N semantics: 3, 8 · warn decay toggle: 5, 8 · DM templates + showmod default off + DM-failed proceeds: 7, 8 · appeal button: deferred to 1c (DM built as a single send, button added later) · 8 log categories each with own channel: 6, 13, 14 · message content memory-only, bounded: 13 · scheduler at startup: 9 · ephemeral user-facing errors: 1 · module per feature area: file structure.

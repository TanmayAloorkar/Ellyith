//! AutoMod engine: checks home-guild messages against the cached rules, then deletes,
//! punishes (through `actions::execute`), posts a short-lived notice and logs once.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildId, Http, Message,
    MessageId, MessageUpdateEvent, UserId,
};
use sqlx::PgPool;

use super::matchers::{self, CompiledWords};
use super::normalize::fold_for_repeat;
use super::rules::{RuleAction, RuleConfig, RuleKind};
use super::store;
use super::window::{Seen, Windows};
use crate::duration::format_duration;
use crate::logging::cache::MAX_CONTENT_CHARS;
use crate::logging::{send_log, LogCategory};
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::perms::{is_absent, is_exempt_from_auto, member_facts, MemberFacts};
use crate::reply::{self, Tone};
use crate::settings::{self, GuildSettings};
use crate::text::truncate;
use crate::{Data, Error};

const NOTICE_GAP: Duration = Duration::from_secs(10);
const INVITE_TTL: Duration = Duration::from_secs(3600);
/// Rule windows are capped at 300 s when saved.
const MAX_WINDOW: Duration = Duration::from_secs(300);
const PRUNE_EVERY: Duration = Duration::from_secs(60);
const UNKNOWN_MESSAGE: isize = 10008;
const UNKNOWN_INVITE: isize = 10006;

/// One guild's rule settings, compiled once per load.
pub struct GuildRules {
    pub rules: Vec<RuleConfig>,
    pub words: CompiledWords,
    pub sites: Vec<String>,
}

pub struct AutoMod {
    cache: RwLock<HashMap<u64, Arc<GuildRules>>>,
    /// Bumped by `invalidate`, so a load that raced a save is not cached.
    generation: AtomicU64,
    windows: Windows,
    last_prune: Mutex<Instant>,
    notices: Mutex<HashMap<(u64, u64), Instant>>,
    invites: Mutex<HashMap<String, (bool, Instant)>>,
    scam: RwLock<HashSet<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}
fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

impl Default for AutoMod {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoMod {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            generation: AtomicU64::new(0),
            windows: Windows::new(5000, MAX_WINDOW),
            last_prune: Mutex::new(Instant::now()),
            notices: Mutex::new(HashMap::new()),
            invites: Mutex::new(HashMap::new()),
            scam: RwLock::new(HashSet::new()),
        }
    }

    /// Cached rule settings; loaded from the DB on first use.
    pub async fn rules(&self, db: &PgPool, guild: u64) -> Result<Arc<GuildRules>, sqlx::Error> {
        let cached = read(&self.cache).get(&guild).cloned();
        if let Some(r) = cached {
            return Ok(r);
        }
        let gen = self.generation.load(Ordering::SeqCst);
        let g = guild as i64;
        let (rules, words, sites) = (store::get_rules(db, g).await?, store::list_words(db, g).await?, store::list_sites(db, g).await?);
        let r = Arc::new(GuildRules { rules, words: CompiledWords::new(&words), sites });
        let mut c = write(&self.cache);
        if self.generation.load(Ordering::SeqCst) == gen {
            c.insert(guild, r.clone());
        }
        Ok(r)
    }

    pub fn invalidate(&self, guild: u64) {
        let mut c = write(&self.cache);
        self.generation.fetch_add(1, Ordering::SeqCst);
        c.remove(&guild);
    }

    pub fn set_scam(&self, set: HashSet<String>) {
        *write(&self.scam) = set;
    }

    /// True when this member may get a notice now (one per member per 10 s).
    fn notice_due(&self, guild: u64, user: u64, now: Instant) -> bool {
        let mut n = lock(&self.notices);
        n.retain(|_, t| now.saturating_duration_since(*t) < NOTICE_GAP);
        if n.contains_key(&(guild, user)) {
            return false;
        }
        n.insert((guild, user), now);
        true
    }

    fn maybe_prune(&self, now: Instant) {
        let mut last = lock(&self.last_prune);
        if now.saturating_duration_since(*last) >= PRUNE_EVERY {
            *last = now;
            drop(last);
            self.windows.prune(now, MAX_WINDOW);
        }
    }

    /// Multi-message rules: count this message, then check each window.
    fn window_hits(&self, guild: u64, user: u64, r: &GuildRules, m: &MsgFacts, attachments: usize, now: Instant) -> Vec<Hit> {
        let live: Vec<&RuleConfig> = r.rules.iter().filter(|c| c.kind.is_multi_message() && applies(c, m)).collect();
        if live.is_empty() {
            return vec![];
        }
        self.windows.push(guild, user, Seen { channel: m.channel, message: m.message, at: now, folded: fold_for_repeat(&m.content), attachments });
        live.into_iter()
            .filter_map(|c| {
                let (t, w) = (c.threshold.max(1) as usize, Duration::from_secs(c.window_secs.clamp(1, 300) as u64));
                let ids = match c.kind {
                    RuleKind::FastMessages => self.windows.check_fast(guild, user, t, w, now),
                    RuleKind::RepeatedMessages => self.windows.check_repeat(guild, user, t, w, c.percent.clamp(0, 100) as u8, now),
                    _ => self.windows.check_files(guild, user, t, w, now),
                }?;
                Some(hit(c, ids))
            })
            .collect()
    }

    /// Any code pointing to another server, or to no server at all (Unknown Invite).
    async fn foreign_invite(&self, http: &Http, guild: GuildId, codes: &[String]) -> bool {
        // shortcut: only the first 5 codes per message are resolved; raise if spammers pad with own invites.
        for code in codes.iter().take(5) {
            let cached = {
                let mut m = lock(&self.invites);
                m.retain(|_, (_, t)| t.elapsed() < INVITE_TTL);
                m.get(code).map(|(h, _)| *h)
            };
            let is_hit = match cached {
                Some(h) => h,
                None => {
                    let h = match http.get_invite(code, false, false, None).await {
                        Ok(inv) => inv.guild.is_some_and(|g| g.id != guild),
                        Err(e) if discord_code(&e) == Some(UNKNOWN_INVITE) => true,
                        // Network errors and rate limits are not hits, and not cached.
                        Err(e) => {
                            tracing::debug!("invite {code} lookup failed: {e}");
                            continue;
                        }
                    };
                    lock(&self.invites).insert(code.clone(), (h, Instant::now()));
                    h
                }
            };
            if is_hit {
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub kind: RuleKind,
    pub action: RuleAction,
    pub timeout_secs: i64,
    pub delete: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: RuleAction,
    pub timeout_secs: i64,
    pub rules: Vec<RuleKind>,
    pub delete: Vec<(u64, u64)>,
}

/// Strongest action wins, the longer timeout on a tie; every rule named, every message deleted.
pub fn decide(hits: &[Hit]) -> Option<Decision> {
    let best = hits.iter().max_by_key(|h| (h.action.strength(), h.timeout_secs))?;
    let (mut rules, mut delete) = (Vec::new(), Vec::new());
    for h in hits {
        if !rules.contains(&h.kind) {
            rules.push(h.kind);
        }
        for d in &h.delete {
            if !delete.contains(d) {
                delete.push(*d);
            }
        }
    }
    Some(Decision { action: best.action, timeout_secs: best.timeout_secs, rules, delete })
}

/// Owner, Administrators, protected roles and mod roles are never touched.
pub fn exempt(f: &MemberFacts, s: &GuildSettings) -> bool {
    is_exempt_from_auto(f, s) || f.role_ids.iter().any(|r| s.mod_roles.contains(&(*r as i64)))
}

/// The message as the rules see it.
pub struct MsgFacts {
    pub channel: u64,
    /// The parent channel when `channel` is a thread or forum post.
    pub parent: Option<u64>,
    pub message: u64,
    pub roles: Vec<u64>,
    pub content: String,
    pub user_mentions: usize,
    pub role_mentions: usize,
}

fn applies(c: &RuleConfig, m: &MsgFacts) -> bool {
    c.enabled
        && !c.ignored_channels.contains(&(m.channel as i64))
        && !m.parent.is_some_and(|p| c.ignored_channels.contains(&(p as i64)))
        && !m.roles.iter().any(|r| c.ignored_roles.contains(&(*r as i64)))
}

fn hit(c: &RuleConfig, delete: Vec<(u64, u64)>) -> Hit {
    Hit { kind: c.kind, action: c.action, timeout_secs: c.timeout_secs, delete }
}

/// Single-message rules, except invites (those need the API).
pub fn evaluate(r: &GuildRules, m: &MsgFacts, scam: &HashSet<String>) -> Vec<Hit> {
    let text = m.content.as_str();
    let hosts = matchers::extract_hosts(text);
    let n = |v: i32| v.max(0) as usize;
    r.rules
        .iter()
        .filter(|c| applies(c, m))
        .filter(|c| match c.kind {
            RuleKind::BannedWords => r.words.hit(text).is_some(),
            // Any @everyone/@here attempt counts, whatever the threshold.
            RuleKind::MassMentions => {
                matchers::mention_count(m.user_mentions, m.role_mentions, text) >= n(c.threshold).max(1)
                    || matchers::mention_count(0, 0, text) > 0
            }
            RuleKind::EmojiSpam => matchers::emoji_count(text) > n(c.threshold),
            RuleKind::AllCaps => {
                matchers::caps_percent(text, n(c.threshold)).is_some_and(|p| i32::from(p) >= c.percent)
            }
            RuleKind::Links => hosts.iter().any(|h| !matchers::link_allowed(h, &r.sites)),
            RuleKind::ScamLinks => hosts.iter().any(|h| matchers::host_in_set(h, scam)),
            RuleKind::WallsOfText => matchers::line_count(text) > n(c.threshold) || text.chars().count() > n(c.extra),
            RuleKind::SpoilerSpam => matchers::spoiler_count(text) > n(c.threshold),
            RuleKind::Zalgo => matchers::zalgo(text, n(c.threshold).max(1) as u32, c.percent.clamp(0, 100) as u8),
            RuleKind::Invites | RuleKind::FastMessages | RuleKind::RepeatedMessages | RuleKind::FileSpam => false,
        })
        .map(|c| hit(c, vec![(m.channel, m.message)]))
        .collect()
}

fn outcome_sentence(action: RuleAction, timeout_secs: i64) -> Option<String> {
    match action {
        RuleAction::Delete => None,
        RuleAction::DeleteWarn => Some("You've been warned.".into()),
        RuleAction::Timeout => Some(format!("You've been timed out for {}.", format_duration(chrono::Duration::seconds(timeout_secs)))),
        RuleAction::Kick => Some("You've been kicked.".into()),
        RuleAction::Ban => Some("You've been banned.".into()),
    }
}

/// Channel notice body; the punishment sentence only when the punishment went through.
pub fn notice_text(user: u64, rules: &[RuleKind], action: RuleAction, timeout_secs: i64, punished: bool) -> String {
    let names: Vec<String> = rules.iter().map(|k| format!("**{}**", k.label())).collect();
    let noun = if names.len() == 1 { "rule" } else { "rules" };
    let mut s = format!("<@{user}>, that broke the {} {noun}.", names.join(", "));
    if let Some(o) = outcome_sentence(action, timeout_secs).filter(|_| punished) {
        s.push('\n');
        s.push_str(&o);
    }
    s
}

/// Mod log "Action" line: what was done, with the case number or the failure.
pub fn action_line(action: RuleAction, timeout_secs: i64, result: &Result<Option<i32>, String>) -> String {
    let what = match action {
        RuleAction::Timeout => format!("Timeout {}", format_duration(chrono::Duration::seconds(timeout_secs))),
        a => a.label().to_string(),
    };
    match result {
        Ok(Some(n)) => format!("{what} · case #{n}"),
        Ok(None) => what,
        Err(e) => format!("{what} · Action failed: {e}"),
    }
}

fn labels(rules: &[RuleKind]) -> String {
    rules.iter().map(|k| k.label()).collect::<Vec<_>>().join(", ")
}

fn punishment(action: RuleAction) -> Option<CaseKind> {
    match action {
        RuleAction::Delete => None,
        RuleAction::DeleteWarn => Some(CaseKind::Warn),
        RuleAction::Timeout => Some(CaseKind::Timeout),
        RuleAction::Kick => Some(CaseKind::Kick),
        RuleAction::Ban => Some(CaseKind::Ban),
    }
}

fn discord_code(e: &serenity::Error) -> Option<isize> {
    match e {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) => Some(r.error.code),
        _ => None,
    }
}

/// Parent of a thread (forum posts included), from the cache.
fn parent_of(ctx: &serenity::Context, guild: GuildId, channel: ChannelId) -> Option<u64> {
    let g = ctx.cache.guild(guild)?;
    g.threads.iter().find(|t| t.id == channel).and_then(|t| t.parent_id).map(|p| p.get())
}

fn roles_of(member: Option<&serenity::PartialMember>) -> Vec<u64> {
    member.map(|m| m.roles.iter().map(|r| r.get()).collect()).unwrap_or_default()
}

pub async fn on_message(ctx: &serenity::Context, data: &Data, msg: &Message) {
    let Some(guild) = msg.guild_id else { return };
    if guild != data.home_guild || msg.author.bot {
        return;
    }
    let m = MsgFacts {
        channel: msg.channel_id.get(),
        parent: parent_of(ctx, guild, msg.channel_id),
        message: msg.id.get(),
        roles: roles_of(msg.member.as_deref()),
        content: msg.content.clone(),
        user_mentions: msg.mentions.len(),
        role_mentions: msg.mention_roles.len(),
    };
    run(ctx, data, guild, msg.author.id, m, Some(msg.attachments.len())).await
}

/// Edits run the single-message rules only.
pub async fn on_edit(ctx: &serenity::Context, data: &Data, ev: &MessageUpdateEvent) {
    let (Some(guild), Some(author), Some(content)) = (ev.guild_id, ev.author.as_ref(), ev.content.as_ref()) else { return };
    if guild != data.home_guild || author.bot {
        return;
    }
    // Link unfurls re-send unchanged content; only real edits are checked.
    if data.msg_cache.get(ev.id.get()).is_some_and(|o| o.content == truncate(content, MAX_CONTENT_CHARS)) {
        return;
    }
    let roles = match ev.member.as_ref().and_then(|m| m.as_deref()) {
        Some(pm) => roles_of(Some(pm)),
        None => ctx.cache.guild(guild).and_then(|g| g.members.get(&author.id).map(|m| m.roles.iter().map(|r| r.get()).collect())).unwrap_or_default(),
    };
    let m = MsgFacts {
        channel: ev.channel_id.get(),
        parent: parent_of(ctx, guild, ev.channel_id),
        message: ev.id.get(),
        roles,
        content: content.clone(),
        user_mentions: ev.mentions.as_ref().map_or(0, Vec::len),
        role_mentions: ev.mention_roles.as_ref().map_or(0, Vec::len),
    };
    run(ctx, data, guild, author.id, m, None).await
}

/// `attachments` is Some for new messages only: windows count new messages.
async fn run(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, m: MsgFacts, attachments: Option<usize>) {
    let am = &data.automod;
    let r = match am.rules(&data.db, guild.get()).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("automod rules load failed: {e}");
            return;
        }
    };
    if !r.rules.iter().any(|c| c.enabled) {
        return;
    }
    let mut hits = evaluate(&r, &m, &read(&am.scam));
    if let Some(n) = attachments {
        let now = Instant::now();
        am.maybe_prune(now);
        hits.extend(am.window_hits(guild.get(), user.get(), &r, &m, n, now));
    }
    let invites = r
        .rules
        .iter()
        .find(|c| c.kind == RuleKind::Invites && applies(c, &m))
        .map(|c| (c, matchers::invite_codes(&m.content)))
        .filter(|(_, codes)| !codes.is_empty());
    if hits.is_empty() && invites.is_none() {
        return;
    }
    // Exemption needs the DB and the member, so it runs only once something matched.
    match is_exempt(ctx, data, guild, user).await {
        Ok(false) => {}
        Ok(true) => return,
        Err(e) => {
            tracing::warn!("automod exemption check failed for {user}: {e}");
            return;
        }
    }
    if let Some((c, codes)) = invites {
        if am.foreign_invite(&ctx.http, guild, &codes).await {
            hits.push(hit(c, vec![(m.channel, m.message)]));
        }
    }
    if let Some(d) = decide(&hits) {
        act(ctx, data, guild, user, &m, d).await;
    }
}

async fn is_exempt(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId) -> Result<bool, Error> {
    let s = settings::get_or_create(&data.db, guild.get() as i64).await?;
    let member = match guild.member(ctx, user).await {
        Ok(m) => m,
        Err(e) if is_absent(&e) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    Ok(exempt(&member_facts(ctx, guild, &member).await?, &s))
}

/// Delete first, then punish; one notice and one log entry per trigger.
async fn act(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, m: &MsgFacts, d: Decision) {
    delete_all(&ctx.http, &d.delete).await;
    let result = match punishment(d.action) {
        None => Ok(None),
        Some(kind) => execute(
            ctx,
            data,
            ActionRequest {
                guild_id: guild,
                target: user,
                kind,
                actor: Actor::AutoPunishment,
                reason: Some(format!("AutoMod: {}", labels(&d.rules))),
                duration: (kind == CaseKind::Timeout).then(|| chrono::Duration::seconds(d.timeout_secs)),
                ban_delete_days: 0,
                // The AutoMod log entry carries the case number: one entry per trigger.
                log_case: false,
            },
        )
        .await
        .map(|c| c.map(|c| c.case_number))
        .map_err(|e| e.to_string()),
    };
    if data.automod.notice_due(guild.get(), user.get(), Instant::now()) {
        let text = notice_text(user.get(), &d.rules, d.action, d.timeout_secs, result.is_ok());
        send_notice(ctx.http.clone(), ChannelId::new(m.channel), text).await;
    }
    let embed = reply::done(
        Tone::Safety,
        &format!("🤖 AutoMod · {}", labels(&d.rules)),
        &[
            ("Member", format!("<@{user}> ({user})")),
            ("Channel", format!("<#{}>", m.channel)),
            ("Action", action_line(d.action, d.timeout_secs, &result)),
            ("Message", truncate(&m.content, 200)),
        ],
        None,
    );
    send_log(&ctx.http, &data.db, guild, LogCategory::ModActions, embed).await;
}

/// Bulk delete per channel when 2+; messages already gone are skipped silently.
async fn delete_all(http: &Http, ids: &[(u64, u64)]) {
    let mut by_channel: HashMap<u64, Vec<MessageId>> = HashMap::new();
    for (c, msg) in ids {
        by_channel.entry(*c).or_default().push(MessageId::new(*msg));
    }
    for (c, msgs) in by_channel {
        let ch = ChannelId::new(c);
        let r = if msgs.len() == 1 { ch.delete_message(http, msgs[0]).await } else { ch.delete_messages(http, &msgs).await };
        if let Err(e) = r {
            if discord_code(&e) != Some(UNKNOWN_MESSAGE) {
                tracing::warn!("automod delete in channel {c} failed: {e}");
            }
        }
    }
}

async fn send_notice(http: Arc<Http>, channel: ChannelId, text: String) {
    let embed = CreateEmbed::new()
        .colour(Tone::Safety.colour())
        .title("⚠️ Message removed")
        .description(text)
        .footer(CreateEmbedFooter::new("This notice disappears in 10 seconds"));
    let msg = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
    match channel.send_message(&http, msg).await {
        Ok(sent) => {
            tokio::spawn(async move {
                tokio::time::sleep(NOTICE_GAP).await;
                if let Err(e) = sent.delete(&http).await {
                    tracing::debug!("automod notice delete failed: {e}");
                }
            });
        }
        Err(e) => tracing::warn!("automod notice in channel {channel} failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::Permissions;
    use RuleAction::*;

    fn h(kind: RuleKind, action: RuleAction, timeout_secs: i64, delete: &[(u64, u64)]) -> Hit {
        Hit { kind, action, timeout_secs, delete: delete.to_vec() }
    }

    #[test]
    fn strongest_action_wins() {
        let d = decide(&[h(RuleKind::BannedWords, DeleteWarn, 600, &[(1, 10)]), h(RuleKind::Links, Timeout, 600, &[(1, 10)])]).unwrap();
        assert_eq!((d.action, d.timeout_secs), (Timeout, 600));
        assert_eq!(d.rules, vec![RuleKind::BannedWords, RuleKind::Links]);
        assert_eq!(d.delete, vec![(1, 10)]);

        let d = decide(&[h(RuleKind::Links, Timeout, 600, &[(1, 10)]), h(RuleKind::ScamLinks, Timeout, 3600, &[(1, 10)])]).unwrap();
        assert_eq!((d.action, d.timeout_secs), (Timeout, 3600));

        let d = decide(&[h(RuleKind::BannedWords, DeleteWarn, 600, &[(1, 10)]), h(RuleKind::MassMentions, DeleteWarn, 600, &[(1, 10)])]).unwrap();
        assert_eq!(d.action, DeleteWarn); // one action, so one warning
        assert_eq!(d.rules.len(), 2);

        let d = decide(&[h(RuleKind::FastMessages, Delete, 600, &[(1, 1), (2, 2)]), h(RuleKind::Links, Delete, 600, &[(2, 2), (3, 3)])]).unwrap();
        assert_eq!(d.delete, vec![(1, 1), (2, 2), (3, 3)]);
        assert_eq!(decide(&[]), None);
    }

    fn facts(owner: bool, perms: Permissions, roles: &[u64]) -> MemberFacts {
        MemberFacts { user_id: 5, is_owner: owner, permissions: perms, role_ids: roles.to_vec(), top_role_position: 1 }
    }

    #[test]
    fn exempt_rules() {
        let s = GuildSettings { mod_roles: vec![42], protected_roles: vec![7], ..GuildSettings::defaults(1) };
        assert!(exempt(&facts(true, Permissions::empty(), &[]), &s));
        assert!(exempt(&facts(false, Permissions::ADMINISTRATOR, &[]), &s));
        assert!(exempt(&facts(false, Permissions::empty(), &[42]), &s));
        assert!(exempt(&facts(false, Permissions::empty(), &[7]), &s));
        assert!(!exempt(&facts(false, Permissions::SEND_MESSAGES, &[3]), &s));
    }

    #[test]
    fn notice_text_sentences() {
        let r = [RuleKind::RepeatedMessages];
        assert_eq!(notice_text(9, &r, Delete, 0, true), "<@9>, that broke the **Repeated messages** rule.");
        assert_eq!(notice_text(9, &r, DeleteWarn, 0, true), "<@9>, that broke the **Repeated messages** rule.\nYou've been warned.");
        assert_eq!(notice_text(9, &r, Timeout, 600, true), "<@9>, that broke the **Repeated messages** rule.\nYou've been timed out for 10m.");
        assert_eq!(notice_text(9, &r, Kick, 0, true), "<@9>, that broke the **Repeated messages** rule.\nYou've been kicked.");
        assert_eq!(notice_text(9, &r, Ban, 0, true), "<@9>, that broke the **Repeated messages** rule.\nYou've been banned.");
        // Punishment failed: only the removal.
        assert_eq!(notice_text(9, &r, Ban, 0, false), "<@9>, that broke the **Repeated messages** rule.");
        assert_eq!(
            notice_text(9, &[RuleKind::BannedWords, RuleKind::Links], Timeout, 3600, true),
            "<@9>, that broke the **Banned words**, **Links** rules.\nYou've been timed out for 1h."
        );
    }

    #[test]
    fn action_lines() {
        assert_eq!(action_line(Timeout, 600, &Ok(Some(12))), "Timeout 10m · case #12");
        assert_eq!(action_line(Delete, 0, &Ok(None)), "Delete");
        assert_eq!(action_line(DeleteWarn, 0, &Err("nope".into())), "Delete + warn · Action failed: nope");
    }

    #[test]
    fn one_notice_per_ten_seconds() {
        let (am, t) = (AutoMod::new(), Instant::now());
        assert!(am.notice_due(1, 2, t));
        assert!(!am.notice_due(1, 2, t + Duration::from_secs(9)));
        assert!(am.notice_due(1, 3, t + Duration::from_secs(9)));
        assert!(am.notice_due(1, 2, t + Duration::from_secs(10)));
    }

    fn on(kind: RuleKind) -> RuleConfig {
        RuleConfig { enabled: true, ..kind.defaults() }
    }

    #[test]
    fn evaluate_respects_rules_and_ignores() {
        let r = GuildRules {
            rules: vec![on(RuleKind::Links), on(RuleKind::MassMentions), RuleConfig { ignored_channels: vec![77], ..on(RuleKind::AllCaps) }],
            words: CompiledWords::new(&[]),
            sites: vec!["youtube.com".into()],
        };
        let m = |channel: u64, content: &str| MsgFacts { channel, parent: None, message: 1, roles: vec![], content: content.into(), user_mentions: 0, role_mentions: 0 };
        let kinds = |m: &MsgFacts| evaluate(&r, m, &HashSet::new()).into_iter().map(|h| h.kind).collect::<Vec<_>>();
        assert_eq!(kinds(&m(1, "see https://sub.youtube.com/x")), vec![]);
        assert_eq!(kinds(&m(1, "see evil.io now @everyone")), vec![RuleKind::Links, RuleKind::MassMentions]);
        assert_eq!(kinds(&m(1, "THIS IS ALL SHOUTING NOW")), vec![RuleKind::AllCaps]);
        assert_eq!(kinds(&m(77, "THIS IS ALL SHOUTING NOW")), vec![]);
        let ignored_role = MsgFacts { roles: vec![5], ..m(1, "evil.io") };
        let r2 = GuildRules { rules: vec![RuleConfig { ignored_roles: vec![5], ..on(RuleKind::Links) }], words: CompiledWords::new(&[]), sites: vec![] };
        assert!(evaluate(&r2, &ignored_role, &HashSet::new()).is_empty());
        // A thread under an ignored channel is ignored too.
        assert_eq!(kinds(&MsgFacts { parent: Some(77), ..m(9, "THIS IS ALL SHOUTING NOW") }), vec![]);
        assert_eq!(kinds(&MsgFacts { parent: Some(8), ..m(9, "THIS IS ALL SHOUTING NOW") }), vec![RuleKind::AllCaps]);
        // Discord's own links and GIFs never trip Links.
        assert_eq!(kinds(&m(1, "https://media.tenor.com/x.gif https://cdn.discordapp.com/a.png discord.gg/abc")), vec![]);
        // Custom emoji names are not shouting.
        assert_eq!(kinds(&m(1, "<:KEKW:1><:OMEGALUL:2><:POGGERS:3>")), vec![]);
    }
}

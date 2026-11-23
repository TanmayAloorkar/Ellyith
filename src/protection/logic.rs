//! Pure protection decisions: no Discord, no DB, time is injected.
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use poise::serenity_prelude::Permissions;

use super::store::{ProtectionSettings, TrustKind};
use crate::automod::normalize::{fold, similarity};

/// Sliding window of recent joins for one guild.
#[derive(Default)]
pub struct JoinWindow(VecDeque<Instant>);

impl JoinWindow {
    /// Record a join; true when `joins` joins sit inside the last `secs` seconds.
    pub fn push(&mut self, now: Instant, joins: usize, secs: u64) -> bool {
        let span = Duration::from_secs(secs);
        while self.0.front().is_some_and(|t| now.saturating_duration_since(*t) > span) {
            self.0.pop_front();
        }
        self.0.push_back(now);
        while self.0.len() > joins.max(1) {
            self.0.pop_front();
        }
        self.0.len() >= joins
    }

    pub fn recent(&self) -> usize {
        self.0.len()
    }
}

/// Raid kick/ban hits everyone when the filter is empty, else members matching any listed criterion.
pub fn raid_kick_allowed(filter: &[String], filter_days: i32, account_age_days: i64, default_avatar: bool) -> bool {
    filter.is_empty()
        || filter.iter().any(|f| match f.as_str() {
            "age" => account_age_days < i64::from(filter_days),
            "avatar" => default_avatar,
            _ => false,
        })
}

pub struct AltInput {
    pub account_age_days: i64,
    pub default_avatar: bool,
    pub name_matches: Option<String>,
    pub invite_after_ban: bool,
}

pub struct AltScore {
    pub total: i32,
    /// (label, detail, points)
    pub lines: Vec<(&'static str, String, i32)>,
}

pub fn alt_score(s: &ProtectionSettings, i: &AltInput) -> AltScore {
    let mut lines = Vec::new();
    let age = if i.account_age_days < i64::from(s.alt_days) { s.alt_pts_age } else { 0 };
    lines.push(("Account age", format!("{} days old", i.account_age_days), age));
    match &i.name_matches {
        Some(n) => lines.push(("Name", format!("looks like banned user {n}"), s.alt_pts_name)),
        None => lines.push(("Name", "no match".into(), 0)),
    }
    lines.push(("Avatar", if i.default_avatar { "default" } else { "custom" }.into(), if i.default_avatar { s.alt_pts_avatar } else { 0 }));
    if i.invite_after_ban {
        lines.push(("Invite", "joined via an invite a banned user used".into(), s.alt_pts_invite));
    }
    AltScore { total: lines.iter().map(|l| l.2).sum(), lines }
}

/// Similar folded names (85+), both at least 4 chars so short names never match.
pub fn name_similar(a: &str, b: &str) -> bool {
    fold(a).chars().count() >= 4 && fold(b).chars().count() >= 4 && similarity(a, b) >= 85
}

pub fn gate_blocks(enabled: bool, days: i32, account_age_days: i64) -> bool {
    enabled && account_age_days < i64::from(days)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOutcome {
    Raid,
    Gate,
    Alt,
    Verify,
    Pass,
}

/// Bots pass. Otherwise the first true flag wins: Raid, Gate, Alt, Verify.
/// A gate that only flags (not quarantines) still returns `Gate`; the caller then runs
/// verification as well, since a flag is not a quarantine.
pub fn decide_join(raid_active_or_burst: bool, gate: bool, alt_over: bool, verify_on: bool, is_bot: bool) -> JoinOutcome {
    if is_bot {
        JoinOutcome::Pass
    } else if raid_active_or_burst {
        JoinOutcome::Raid
    } else if gate {
        JoinOutcome::Gate
    } else if alt_over {
        JoinOutcome::Alt
    } else if verify_on {
        JoinOutcome::Verify
    } else {
        JoinOutcome::Pass
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NukeKind {
    Ban,
    Kick,
    ChannelDelete,
    RoleDelete,
    Webhook,
    DangerousPerm,
}

impl NukeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ban => "bans",
            Self::Kick => "kicks",
            Self::ChannelDelete => "channel deletes",
            Self::RoleDelete => "role deletes",
            Self::Webhook => "webhooks made",
            Self::DangerousPerm => "dangerous permissions given",
        }
    }
}

pub fn nuke_limit(s: &ProtectionSettings, k: NukeKind) -> i32 {
    match k {
        NukeKind::Ban => s.nuke_bans,
        NukeKind::Kick => s.nuke_kicks,
        NukeKind::ChannelDelete => s.nuke_chan_del,
        NukeKind::RoleDelete => s.nuke_role_del,
        NukeKind::Webhook => s.nuke_webhooks,
        NukeKind::DangerousPerm => s.nuke_perms,
    }
}

const MAX_KEYS: usize = 5_000;

#[derive(Default)]
pub struct NukeCounter(Mutex<HashMap<(u64, u64, NukeKind), VecDeque<Instant>>>);

impl NukeCounter {
    /// `Some(count)` the first time `limit` actions land in `window`; the key then resets.
    pub fn record(&self, guild: u64, user: u64, kind: NukeKind, now: Instant, window: Duration, limit: i32) -> Option<usize> {
        let limit = usize::try_from(limit).ok().filter(|l| *l > 0)?;
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let key = (guild, user, kind);
        if !map.contains_key(&key) && map.len() >= MAX_KEYS {
            map.retain(|_, v| v.back().is_some_and(|t| now.saturating_duration_since(*t) <= window));
            // shortcut: drop new keys while full of live ones, raise only if real guilds ever hit 5000 live actors
            if map.len() >= MAX_KEYS {
                return None;
            }
        }
        let q = map.entry(key).or_default();
        while q.front().is_some_and(|t| now.saturating_duration_since(*t) > window) {
            q.pop_front();
        }
        q.push_back(now);
        let n = q.len();
        if n >= limit {
            map.remove(&key);
            Some(n)
        } else {
            None
        }
    }
}

pub fn is_trusted(user: u64, owner: u64, bot: u64, member_roles: &[u64], trusted: &[(TrustKind, i64)]) -> bool {
    user == owner
        || user == bot
        || trusted.iter().any(|(k, id)| match k {
            TrustKind::User => *id as u64 == user,
            TrustKind::Role => member_roles.contains(&(*id as u64)),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PunishSource {
    Command,
    RaidBulk,
    Automatic,
}

/// Only a moderator's own command counts; bot-driven bans/kicks never trip anti-nuke.
pub fn counts_toward_nuke(src: PunishSource) -> bool {
    src == PunishSource::Command
}

pub const DANGEROUS: Permissions = Permissions::ADMINISTRATOR
    .union(Permissions::BAN_MEMBERS)
    .union(Permissions::KICK_MEMBERS)
    .union(Permissions::MANAGE_GUILD)
    .union(Permissions::MANAGE_ROLES)
    .union(Permissions::MANAGE_CHANNELS)
    .union(Permissions::MANAGE_WEBHOOKS);

pub fn gained_dangerous(old: Permissions, new: Permissions) -> bool {
    new.intersects(DANGEROUS) && !(new & DANGEROUS).difference(old).is_empty()
}

/// The invite code used by a join: exactly one code went up, or exactly one new code has uses >= 1.
pub fn invite_diff(before: &HashMap<String, u64>, after: &HashMap<String, u64>) -> Option<String> {
    let mut hit = after.iter().filter(|(c, n)| match before.get(*c) {
        Some(b) => **n > *b,
        None => **n >= 1,
    });
    let first = hit.next()?;
    hit.next().is_none().then(|| first.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ProtectionSettings {
        ProtectionSettings {
            guild_id: 1, raid_enabled: true, raid_joins: 10, raid_secs: 10,
            raid_responses: vec!["alert".into(), "quarantine".into()], raid_filter: vec![], raid_filter_days: 7, raid_end_mins: 10,
            alt_enabled: true, alt_days: 7, alt_pts_age: 40, alt_pts_avatar: 20, alt_pts_name: 40, alt_pts_invite: 30,
            alt_threshold: 60, alt_response: "flag".into(), gate_enabled: false, gate_days: 7, gate_action: "quarantine".into(),
            nuke_enabled: true, nuke_window_secs: 10, nuke_bans: 3, nuke_kicks: 3, nuke_chan_del: 2, nuke_role_del: 2,
            nuke_webhooks: 3, nuke_perms: 1,
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn burst_trips_at_threshold() {
        let t0 = Instant::now();
        let mut w = JoinWindow::default();
        for i in 0..9 {
            assert!(!w.push(t0 + Duration::from_millis(i * 1000 / 2), 10, 10));
        }
        assert!(w.push(t0 + Duration::from_secs(5), 10, 10));
        assert!(w.recent() <= 10);
        let mut w = JoinWindow::default();
        for i in 0..40 {
            assert!(!w.push(t0 + Duration::from_secs(i * 3), 10, 10));
        }
    }

    #[test]
    fn raid_filter() {
        assert!(raid_kick_allowed(&[], 7, 999, false));
        assert!(raid_kick_allowed(&s(&["age"]), 7, 3, false));
        assert!(!raid_kick_allowed(&s(&["age"]), 7, 30, false));
        assert!(raid_kick_allowed(&s(&["age", "avatar"]), 7, 30, true));
    }

    #[test]
    fn alt_score_defaults() {
        let st = settings();
        let a = alt_score(&st, &AltInput { account_age_days: 3, default_avatar: false, name_matches: Some("x".into()), invite_after_ban: false });
        assert_eq!(a.total, 80);
        assert_eq!(a.lines.iter().map(|l| l.2).collect::<Vec<_>>(), vec![40, 40, 0]);
        let b = alt_score(&st, &AltInput { account_age_days: 30, default_avatar: false, name_matches: None, invite_after_ban: false });
        assert_eq!(b.total, 0);
        let c = alt_score(&st, &AltInput { account_age_days: 30, default_avatar: true, name_matches: None, invite_after_ban: true });
        assert_eq!(c.total, 50);
    }

    #[test]
    fn name_similarity() {
        assert!(name_similar("toxicguy", "toxicguy_2"));
        assert!(name_similar("t0xicguy", "toxicguy"));
        assert!(!name_similar("alice", "bob"));
        assert!(!name_similar("ab", "ab"));
    }

    #[test]
    fn gate_check() {
        assert!(gate_blocks(true, 7, 3));
        assert!(!gate_blocks(true, 7, 7));
        assert!(!gate_blocks(false, 7, 3));
    }

    #[test]
    fn join_order_single_quarantine() {
        assert_eq!(decide_join(false, false, true, true, false), JoinOutcome::Alt);
        assert_eq!(decide_join(true, true, true, true, false), JoinOutcome::Raid);
        assert_eq!(decide_join(true, true, true, true, true), JoinOutcome::Pass);
        assert_eq!(decide_join(false, false, false, false, false), JoinOutcome::Pass);
        assert_eq!(decide_join(false, true, true, true, false), JoinOutcome::Gate);
        assert_eq!(decide_join(false, false, false, true, false), JoinOutcome::Verify);
    }

    #[test]
    fn nuke_counter() {
        let c = NukeCounter::default();
        let t0 = Instant::now();
        let w = Duration::from_secs(10);
        let k = NukeKind::Ban;
        assert_eq!(c.record(1, 2, k, t0, w, 3), None);
        assert_eq!(c.record(1, 2, k, t0 + Duration::from_secs(1), w, 3), None);
        assert_eq!(c.record(1, 2, k, t0 + Duration::from_secs(2), w, 3), Some(3));
        assert_eq!(c.record(1, 2, k, t0 + Duration::from_secs(2), w, 3), None);
        for i in 0..3 {
            assert_eq!(c.record(1, 9, k, t0 + Duration::from_secs(i * 15), w, 3), None);
        }
    }

    #[test]
    fn trusted_never_counted() {
        let tr = [(TrustKind::User, 5), (TrustKind::Role, 77)];
        assert!(is_trusted(1, 1, 2, &[], &tr));
        assert!(is_trusted(2, 1, 2, &[], &tr));
        assert!(is_trusted(5, 1, 2, &[], &tr));
        assert!(is_trusted(6, 1, 2, &[77], &tr));
        assert!(!is_trusted(6, 1, 2, &[78], &tr));
        assert!(!is_trusted(77, 1, 2, &[], &tr));
    }

    #[test]
    fn bulk_raid_actions_not_counted() {
        assert!(!counts_toward_nuke(PunishSource::RaidBulk));
        assert!(!counts_toward_nuke(PunishSource::Automatic));
        assert!(counts_toward_nuke(PunishSource::Command));
    }

    #[test]
    fn dangerous_gain() {
        let none = Permissions::empty();
        assert!(gained_dangerous(none, Permissions::ADMINISTRATOR));
        assert!(!gained_dangerous(none, Permissions::SEND_MESSAGES));
        assert!(!gained_dangerous(Permissions::BAN_MEMBERS, none));
        assert!(!gained_dangerous(Permissions::BAN_MEMBERS, Permissions::BAN_MEMBERS | Permissions::SEND_MESSAGES));
    }

    #[test]
    fn invite_diff_ambiguous() {
        let m = |v: &[(&str, u64)]| v.iter().map(|(k, n)| (k.to_string(), *n)).collect::<HashMap<_, _>>();
        let before = m(&[("a", 1), ("b", 2)]);
        assert_eq!(invite_diff(&before, &m(&[("a", 2), ("b", 2)])), Some("a".into()));
        assert_eq!(invite_diff(&before, &m(&[("a", 2), ("b", 3)])), None);
        assert_eq!(invite_diff(&before, &before), None);
        assert_eq!(invite_diff(&before, &m(&[("a", 1), ("b", 2), ("c", 1)])), Some("c".into()));
    }
}

//! Raid mode: start, per-raider responses, the live alert, bulk kick/ban, and the end-of-raid review.
use super::logic::raid_kick_allowed;
use super::store::{self, RaidRow, RaiderRow};
use super::EMOJI_PROTECTION;
use crate::duration::format_duration;
use crate::logging::{send_log_message, LogCategory};
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::perms::is_absent;
use crate::panel::id::{PanelId, ProtId};
use crate::reply::{self, Tone};
use crate::safety::quarantine::{self, QuarantineReason};
use crate::safety::{lockdown, store as safety_store};
use crate::{settings, Data, Error, UserError};
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, EditMessage, GuildId, Member, MessageId, RoleId, UserId,
};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

pub const REASON: &str = "Raid protection";
const NOT_ON: &str = "Raid mode is not on.";
const ALREADY_ON: &str = "Raid mode is already on.";
const EDIT_GAP: Duration = Duration::from_secs(5);
const PAGE: usize = 25;
const SHOWN: usize = 10;

/// What raid mode has done so far, shown on the alert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Done {
    pub quarantined: usize,
    pub kicked: usize,
    pub banned: usize,
    pub lockdown: usize,
}

/// In-memory state of a guild's active raid.
/// shortcut: lost on restart (counts restart from 0, raiders stay in the DB); persist if owners ask.
#[derive(Debug, Default)]
pub struct RaidStats {
    pub title: String,
    pub done: Done,
    joins: VecDeque<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndPlan {
    pub lift_lockdown: bool,
}

/// Only lift a lockdown this raid started; a moderator's own lockdown stays.
pub fn end_plan(row: &RaidRow) -> EndPlan {
    EndPlan { lift_lockdown: row.started_lockdown }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiderAction {
    Punish(CaseKind),
    Quarantine,
}

/// Kick/ban when chosen and the filter matches; otherwise quarantine when chosen, or when the filter spared them.
pub fn raider_action(responses: &[String], kick_allowed: bool) -> Option<RaiderAction> {
    let has = |r: &str| responses.iter().any(|x| x == r);
    let punish = if has("ban") {
        Some(CaseKind::Ban)
    } else if has("kick") {
        Some(CaseKind::Kick)
    } else {
        None
    };
    match punish {
        Some(k) if kick_allowed => Some(RaiderAction::Punish(k)),
        Some(_) => Some(RaiderAction::Quarantine),
        None if has("quarantine") => Some(RaiderAction::Quarantine),
        None => None,
    }
}

pub fn alert_lines(count: usize, recent: usize, done: &Done) -> Vec<(&'static str, String)> {
    let parts: Vec<String> = [(done.quarantined, "Quarantined"), (done.kicked, "Kicked"), (done.banned, "Banned")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, l)| format!("{l} {n}"))
        .chain((done.lockdown > 0).then(|| format!("Lockdown on {} channels", done.lockdown)))
        .collect();
    let done = if parts.is_empty() { "Nothing yet".into() } else { parts.join(" · ") };
    vec![("Raiders so far", format!("{count} ({recent} in the last minute)")), ("Done", done)]
}

/// Page `page` of the approve select (25 per page), and whether a later page exists.
pub fn review_page(raiders: &[u64], page: usize) -> (Vec<u64>, bool) {
    let start = page.saturating_mul(PAGE);
    (raiders.iter().skip(start).take(PAGE).copied().collect(), raiders.len() > start.saturating_add(PAGE))
}

/// Largest unit only ("2h", "3d").
fn age(created: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let d = format_duration((now - created).max(chrono::Duration::zero()));
    d.split(' ').next().unwrap_or("0s").to_string()
}

/// Body of "Raid mode ended": the summary, up to 10 raiders, then "… N more".
pub fn ended_text(lifted: bool, waiting: &[RaiderRow], now: DateTime<Utc>) -> String {
    let n = waiting.len();
    let still = if n == 1 { "1 raider is still in quarantine.".to_string() } else { format!("{n} raiders are still in quarantine.") };
    let mut lines = vec![if lifted { format!("Lockdown lifted. {still}") } else { still }];
    if n > 0 {
        lines.push(String::new());
    }
    for r in waiting.iter().take(SHOWN) {
        let avatar = if r.default_avatar { " · default avatar" } else { "" };
        lines.push(format!("<@{}> account {} old{avatar}", r.user_id, age(r.account_created, now)));
    }
    if n > SHOWN {
        lines.push(format!("… {} more", n - SHOWN));
    }
    lines.join("\n")
}

/// Claims the next alert edit for `guild`: `Some(delay)` to edit after `delay`, `None` when one is already queued.
fn edit_slot(last: Option<Instant>, now: Instant) -> (Option<Duration>, Instant) {
    match last {
        Some(t) if t > now => (None, t),
        Some(t) if now.duration_since(t) < EDIT_GAP => (Some(t + EDIT_GAP - now), t + EDIT_GAP),
        _ => (Some(Duration::ZERO), now),
    }
}

fn already() -> Error {
    UserError(ALREADY_ON.into()).into()
}

fn not_on() -> Error {
    UserError(NOT_ON.into()).into()
}

pub(super) fn button(id: ProtId, label: &str, emoji: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(PanelId::Protection(id).encode()).label(label).emoji(serenity::ReactionType::Unicode(emoji.into())).style(style)
}

fn alert_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        button(ProtId::RaidEnd, "End raid mode", "✅", ButtonStyle::Success),
        button(ProtId::RaidBulk(false), "Kick all raiders", "👢", ButtonStyle::Secondary),
        button(ProtId::RaidBulk(true), "Ban all raiders", "🔨", ButtonStyle::Danger),
    ])]
}

fn alert_embed(title: &str, count: usize, recent: usize, done: &Done, ends: Option<(DateTime<Utc>, i32)>) -> CreateEmbed {
    let mut lines = alert_lines(count, recent, done);
    let footer = match ends {
        Some((at, mins)) => {
            lines.push(("Ends", format!("<t:{}:R>, {mins} minutes after the last burst", at.timestamp())));
            "Updates live while raid mode is on"
        }
        None => "Raid mode ended",
    };
    reply::done(Tone::Safety, title, &lines, Some(footer))
}

fn with_stats<T>(data: &Data, guild: GuildId, f: impl FnOnce(&mut RaidStats) -> T) -> T {
    let mut m = data.protection.raids.lock().unwrap_or_else(|e| e.into_inner());
    f(m.entry(guild.get()).or_insert_with(|| RaidStats { title: format!("{EMOJI_PROTECTION} Raid mode is on"), ..Default::default() }))
}

/// (title, recent joins, done) for the alert; drops joins older than a minute.
fn snapshot(data: &Data, guild: GuildId) -> (String, usize, Done) {
    with_stats(data, guild, |s| {
        while s.joins.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            s.joins.pop_front();
        }
        (s.title.clone(), s.joins.len(), s.done)
    })
}

pub async fn start(ctx: &serenity::Context, data: &Data, guild: GuildId, by: Actor) -> Result<RaidRow, Error> {
    let gid = guild.get() as i64;
    if store::active_raid(&data.db, gid).await?.is_some() {
        return Err(already());
    }
    let s = store::get_settings(&data.db, gid).await?;
    let ends = Utc::now() + chrono::Duration::minutes(s.raid_end_mins.into());
    // The unique index settles a race between two starts.
    let mut raid = store::start_raid(&data.db, gid, ends).await.map_err(|e| match e.as_database_error().and_then(|d| d.code()) {
        Some(c) if c == "23505" => already(),
        _ => e.into(),
    })?;
    let title = match by {
        Actor::Moderator(id) => {
            let name = UserId::new(id as u64).to_user(ctx).await.map(|u| u.name).unwrap_or_else(|_| "staff".into());
            format!("{EMOJI_PROTECTION} Raid mode started by @{name}")
        }
        _ => format!("{EMOJI_PROTECTION} Raid detected"),
    };
    let mut done = Done::default();
    if s.raid_responses.iter().any(|r| r == "lockdown") {
        match lockdown::start(ctx, data, guild, Actor::System, None, Some(REASON.into())).await {
            Ok(r) => {
                store::set_raid_lockdown(&data.db, raid.id, true).await?;
                raid.started_lockdown = true;
                done.lockdown = r.done.len();
            }
            Err(e) => tracing::warn!("raid lockdown in {guild} failed: {e}"),
        }
    }
    data.protection.raids.lock().unwrap_or_else(|e| e.into_inner()).insert(guild.get(), RaidStats { title: title.clone(), done, joins: VecDeque::new() });
    // The alert is always posted (it carries the controls); the mod-role ping only when "alert" is chosen.
    let embed = alert_embed(&title, 0, 0, &done, Some((ends, s.raid_end_mins)));
    let mut msg = CreateMessage::new().embed(embed).components(alert_buttons());
    if s.raid_responses.iter().any(|r| r == "alert") {
        let roles = settings::get_or_create(&data.db, gid).await?.mod_roles;
        if !roles.is_empty() {
            let content: Vec<String> = roles.iter().map(|r| format!("<@&{r}>")).collect();
            msg = msg.content(content.join(" ")).allowed_mentions(CreateAllowedMentions::new().roles(roles.iter().map(|r| RoleId::new(*r as u64))));
        }
    }
    if let Some(m) = send_log_message(&ctx.http, &data.db, guild, LogCategory::ModActions, msg).await {
        store::set_raid_alert(&data.db, raid.id, m.channel_id.get() as i64, m.id.get() as i64).await?;
        raid.alert_channel = Some(m.channel_id.get() as i64);
        raid.alert_message = Some(m.id.get() as i64);
    }
    Ok(raid)
}

/// A new burst during raid mode: the auto-end restarts from now.
pub async fn extend(data: &Data, raid: &RaidRow) -> Result<(), Error> {
    let s = store::get_settings(&data.db, raid.guild_id).await?;
    store::extend_raid(&data.db, raid.id, Utc::now() + chrono::Duration::minutes(s.raid_end_mins.into())).await?;
    Ok(())
}

/// Runs the chosen responses on one raider, records them, and refreshes the alert.
/// Returns true when the member was quarantined, kicked or banned. Failures are logged, never returned.
pub async fn on_raider(ctx: &serenity::Context, data: &Data, guild: GuildId, member: &Member, raid: &RaidRow) -> bool {
    let s = match store::get_settings(&data.db, raid.guild_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("raid settings for {guild} failed: {e}");
            return false;
        }
    };
    let created = DateTime::from_timestamp(member.user.created_at().unix_timestamp(), 0).unwrap_or_else(Utc::now);
    let default_avatar = member.user.avatar.is_none();
    let allowed = raid_kick_allowed(&s.raid_filter, s.raid_filter_days, (Utc::now() - created).num_days(), default_avatar);
    let user = member.user.id;
    let handled = match raider_action(&s.raid_responses, allowed) {
        Some(RaiderAction::Punish(kind)) => {
            let req = ActionRequest { guild_id: guild, target: user, kind, actor: Actor::AutoPunishment, reason: Some(REASON.into()), duration: None, ban_delete_days: 0, log_case: true };
            match execute(ctx, data, req).await {
                Ok(_) => {
                    with_stats(data, guild, |st| if kind == CaseKind::Ban { st.done.banned += 1 } else { st.done.kicked += 1 });
                    true
                }
                Err(e) => {
                    tracing::warn!("raid {} of {user} failed: {e}", kind.as_str());
                    false
                }
            }
        }
        Some(RaiderAction::Quarantine) => match quarantine::enter(ctx, data, guild, member, QuarantineReason::Raid, Actor::AutoPunishment, Some(REASON.into()), None).await {
            Ok(()) => {
                with_stats(data, guild, |st| st.done.quarantined += 1);
                true
            }
            Err(e) => {
                tracing::warn!("raid quarantine of {user} failed: {e}");
                false
            }
        },
        None => false,
    };
    if let Err(e) = store::add_raider(&data.db, raid.id, user.get() as i64, created, default_avatar).await {
        tracing::warn!("recording raider {user} failed: {e}");
    }
    with_stats(data, guild, |st| st.joins.push_back(Instant::now()));
    edit_alert_soon(ctx, data, guild);
    handled
}

/// At most one alert edit per 5 s per guild; a skipped edit is queued so the last raider still shows.
fn edit_alert_soon(ctx: &serenity::Context, data: &Data, guild: GuildId) {
    let delay = {
        let mut m = data.protection.alert_edit.lock().unwrap_or_else(|e| e.into_inner());
        let (delay, at) = edit_slot(m.get(&guild.get()).copied(), Instant::now());
        m.insert(guild.get(), at);
        delay
    };
    let Some(delay) = delay else { return };
    let (ctx, data) = (ctx.clone(), data.clone());
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        if let Err(e) = edit_alert(&ctx, &data, guild).await {
            tracing::warn!("raid alert edit in {guild} failed: {e}");
        }
    });
}

async fn edit_alert(ctx: &serenity::Context, data: &Data, guild: GuildId) -> Result<(), Error> {
    let Some(raid) = store::active_raid(&data.db, guild.get() as i64).await? else { return Ok(()) };
    let (Some(ch), Some(msg)) = (raid.alert_channel, raid.alert_message) else { return Ok(()) };
    let count = store::raiders(&data.db, raid.id).await?.len();
    let mins = store::get_settings(&data.db, raid.guild_id).await?.raid_end_mins;
    let (title, recent, done) = snapshot(data, guild);
    let embed = alert_embed(&title, count, recent, &done, Some((raid.ends_at, mins)));
    ChannelId::new(ch as u64).edit_message(ctx, MessageId::new(msg as u64), EditMessage::new().embed(embed)).await?;
    Ok(())
}

/// Raiders still in quarantine for a raid, in user id order.
pub async fn waiting(data: &Data, guild: GuildId, raid_id: i64) -> Result<Vec<RaiderRow>, Error> {
    let q: HashSet<i64> = safety_store::quarantine_list(&data.db, guild.get() as i64)
        .await?
        .into_iter()
        .filter(|r| r.reason == QuarantineReason::Raid.as_str())
        .map(|r| r.user_id)
        .collect();
    Ok(store::raiders(&data.db, raid_id).await?.into_iter().filter(|r| q.contains(&r.user_id)).collect())
}

/// The approve select for `page` (falls back to the first page once it empties), page buttons, Approve all and Deny the rest.
pub fn review_rows(ctx: &serenity::Context, guild: GuildId, raid_id: i64, waiting: &[u64], page: usize) -> Vec<CreateActionRow> {
    if waiting.is_empty() {
        return vec![];
    }
    let (mut ids, mut more) = review_page(waiting, page);
    let mut page = page;
    if ids.is_empty() {
        page = 0;
        (ids, more) = review_page(waiting, 0);
    }
    let options: Vec<CreateSelectMenuOption> = ids
        .iter()
        .map(|id| {
            let name = ctx.cache.guild(guild).and_then(|g| g.members.get(&UserId::new(*id)).map(|m| m.user.name.clone())).unwrap_or_else(|| id.to_string());
            CreateSelectMenuOption::new(name, id.to_string())
        })
        .collect();
    let n = options.len() as u8;
    let menu = CreateSelectMenu::new(PanelId::Protection(ProtId::ReviewApprove(raid_id, page)).encode(), CreateSelectMenuKind::String { options })
        .placeholder("Pick members to approve (let in)")
        .min_values(1)
        .max_values(n);
    let mut buttons = vec![];
    if more {
        buttons.push(button(ProtId::ReviewPage(raid_id, page + 1), "Next page", "➡️", ButtonStyle::Secondary));
    } else if page > 0 {
        buttons.push(button(ProtId::ReviewPage(raid_id, 0), "First page", "⬅️", ButtonStyle::Secondary));
    }
    if waiting.len() > PAGE {
        buttons.push(button(ProtId::ReviewApproveAll(raid_id), "Approve all", "✅", ButtonStyle::Success));
    }
    buttons.push(button(ProtId::ReviewDenyRest(raid_id), "Deny the rest", "⛔", ButtonStyle::Danger));
    vec![CreateActionRow::SelectMenu(menu), CreateActionRow::Buttons(buttons)]
}

pub async fn end(ctx: &serenity::Context, data: &Data, guild: GuildId, by: Actor) -> Result<CreateEmbed, Error> {
    let gid = guild.get() as i64;
    let raid = store::active_raid(&data.db, gid).await?.ok_or_else(not_on)?;
    // Claim the end first, so the tick and a button press can't both end it.
    if !store::end_raid(&data.db, raid.id).await? {
        return Err(not_on());
    }
    let (title, recent, done) = snapshot(data, guild);
    data.protection.raids.lock().unwrap_or_else(|e| e.into_inner()).remove(&guild.get());
    data.protection.alert_edit.lock().unwrap_or_else(|e| e.into_inner()).remove(&guild.get());
    let lifted = end_plan(&raid).lift_lockdown
        && match lockdown::end(ctx, data, guild, by).await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("lifting raid lockdown in {guild} failed: {e}");
                false
            }
        };
    if let (Some(ch), Some(msg)) = (raid.alert_channel, raid.alert_message) {
        let count = store::raiders(&data.db, raid.id).await?.len();
        let edit = EditMessage::new().embed(alert_embed(&title, count, recent, &done, None)).components(vec![]);
        if let Err(e) = ChannelId::new(ch as u64).edit_message(ctx, MessageId::new(msg as u64), edit).await {
            tracing::warn!("closing raid alert in {guild} failed: {e}");
        }
    }
    let waiting = waiting(data, guild, raid.id).await?;
    let embed = reply::done(Tone::Safety, "✅ Raid mode ended", &[], None).description(ended_text(lifted, &waiting, Utc::now()));
    let ids: Vec<u64> = waiting.iter().map(|r| r.user_id as u64).collect();
    let msg = CreateMessage::new().embed(embed.clone()).components(review_rows(ctx, guild, raid.id, &ids, 0));
    send_log_message(&ctx.http, &data.db, guild, LogCategory::ModActions, msg).await;
    Ok(embed)
}

/// Ends every raid whose auto-end time has passed. Called by the scheduler.
pub async fn tick(ctx: &serenity::Context, data: &Data) {
    let due = match store::due_raids(&data.db, Utc::now()).await {
        Ok(d) => d,
        Err(e) => return tracing::warn!("loading due raids failed: {e}"),
    };
    for raid in due {
        let guild = GuildId::new(raid.guild_id as u64);
        if let Err(e) = end(ctx, data, guild, Actor::System).await {
            tracing::warn!("auto-ending raid {} failed: {e}", raid.id);
        }
    }
}

/// Kicks or bans every raider still in the guild, 1 s apart.
/// PunishSource::RaidBulk: goes straight to `actions::execute`, so it never reaches the anti-nuke counter.
pub async fn bulk(ctx: &serenity::Context, data: &Data, guild: GuildId, kick: bool, by: Actor) -> Result<CreateEmbed, Error> {
    let gid = guild.get() as i64;
    let raid = store::active_raid(&data.db, gid).await?.ok_or_else(not_on)?;
    let kind = if kick { CaseKind::Kick } else { CaseKind::Ban };
    let (mut done, mut failed, mut first) = (0, 0, true);
    for r in store::raiders(&data.db, raid.id).await? {
        let user = UserId::new(r.user_id as u64);
        match guild.member(ctx, user).await {
            Ok(_) => {}
            Err(e) if is_absent(&e) => continue,
            Err(e) => {
                tracing::warn!("raid bulk: reading {user} failed: {e}");
                failed += 1;
                continue;
            }
        }
        if !first {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        first = false;
        let req = ActionRequest { guild_id: guild, target: user, kind, actor: by, reason: Some(REASON.into()), duration: None, ban_delete_days: 0, log_case: true };
        match execute(ctx, data, req).await {
            Ok(_) => {
                done += 1;
                // Gone from the server: their raid quarantine row would only clutter the review.
                if let Err(e) = safety_store::quarantine_delete(&data.db, gid, r.user_id).await {
                    tracing::warn!("raid bulk: clearing quarantine of {user} failed: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("raid bulk {} of {user} failed: {e}", kind.as_str());
                failed += 1;
            }
        }
    }
    let title = if kick { format!("👢 Kicked {done} raiders") } else { format!("🔨 Banned {done} raiders") };
    let mut lines = vec![("Reason", REASON.to_string())];
    if failed > 0 {
        lines.push(("Couldn't", format!("{failed} raiders (see the log)")));
    }
    Ok(reply::done(Tone::Safety, &title, &lines, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raid(started_lockdown: bool) -> RaidRow {
        RaidRow { id: 1, guild_id: 1, started_at: Utc::now(), ends_at: Utc::now(), alert_channel: None, alert_message: None, started_lockdown }
    }

    #[test]
    fn raid_end_lifts_only_own_lockdown() {
        assert_eq!(end_plan(&raid(true)), EndPlan { lift_lockdown: true });
        assert_eq!(end_plan(&raid(false)), EndPlan { lift_lockdown: false });
    }

    #[test]
    fn alert_text() {
        let done = Done { quarantined: 14, lockdown: 5, ..Default::default() };
        let l = alert_lines(14, 6, &done);
        assert_eq!(l[0], ("Raiders so far", "14 (6 in the last minute)".to_string()));
        assert_eq!(l[1], ("Done", "Quarantined 14 · Lockdown on 5 channels".to_string()));
        assert_eq!(alert_lines(0, 0, &Done::default())[1].1, "Nothing yet");
        assert_eq!(alert_lines(3, 3, &Done { kicked: 2, banned: 1, ..Default::default() })[1].1, "Kicked 2 · Banned 1");
    }

    #[test]
    fn review_pages_over_25() {
        let ids: Vec<u64> = (1..=30).collect();
        let (p0, more0) = review_page(&ids, 0);
        assert_eq!((p0.len(), more0, p0[0]), (25, true, 1));
        let (p1, more1) = review_page(&ids, 1);
        assert_eq!((p1, more1), (vec![26, 27, 28, 29, 30], false));
        assert_eq!(review_page(&ids[..25], 0), ((1..=25).collect(), false));
        assert_eq!(review_page(&ids, usize::MAX), (vec![], false));
    }

    #[test]
    fn raider_action_order() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(raider_action(&v(&["alert", "quarantine"]), true), Some(RaiderAction::Quarantine));
        assert_eq!(raider_action(&v(&["alert"]), true), None);
        assert_eq!(raider_action(&v(&["kick", "ban"]), true), Some(RaiderAction::Punish(CaseKind::Ban)));
        assert_eq!(raider_action(&v(&["kick"]), true), Some(RaiderAction::Punish(CaseKind::Kick)));
        // The filter spares them: quarantined instead, even when quarantine was not chosen.
        assert_eq!(raider_action(&v(&["kick"]), false), Some(RaiderAction::Quarantine));
    }

    #[test]
    fn ended_text_lists_ten() {
        let now = Utc::now();
        let row = |id: i64, avatar: bool| RaiderRow { raid_id: 1, user_id: id, account_created: now - chrono::Duration::hours(2), default_avatar: avatar };
        let rows: Vec<RaiderRow> = (1..=14).map(|i| row(i, i % 2 == 1)).collect();
        let t = ended_text(true, &rows, now);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "Lockdown lifted. 14 raiders are still in quarantine.");
        assert_eq!(lines[2], "<@1> account 2h old · default avatar");
        assert_eq!(lines[3], "<@2> account 2h old");
        assert_eq!(lines.len(), 2 + 10 + 1);
        assert_eq!(*lines.last().unwrap(), "… 4 more");
        assert_eq!(ended_text(false, &rows[..1], now).lines().next(), Some("1 raider is still in quarantine."));
        assert_eq!(ended_text(false, &[], now), "0 raiders are still in quarantine.");
    }

    #[test]
    fn alert_edits_throttled() {
        let t0 = Instant::now();
        assert_eq!(edit_slot(None, t0), (Some(Duration::ZERO), t0));
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(edit_slot(Some(t0), t1), (Some(Duration::from_secs(3)), t0 + EDIT_GAP));
        // Already queued: skip.
        assert_eq!(edit_slot(Some(t0 + EDIT_GAP), t1), (None, t0 + EDIT_GAP));
        let t2 = t0 + Duration::from_secs(6);
        assert_eq!(edit_slot(Some(t0), t2), (Some(Duration::ZERO), t2));
    }
}

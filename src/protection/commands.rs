//! `/raid`, `/alts`, `/newaccounts`, `/antinuke`. Each subcommand is a `*_core` the panel reuses.
use super::store::{self, ProtectionSettings, TrustKind};
use super::{raid, EMOJI_ALTS, EMOJI_GATE, EMOJI_NUKE, EMOJI_RAID};
use crate::invoker::Invoker;
use crate::moderation::cases::Actor;
use crate::moderation::perms::{authorize, require_manage_server};
use crate::reply::{self, Tone};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateEmbed, Permissions, Role, User};

pub const RAID_RESPONSES: [&str; 5] = ["alert", "quarantine", "kick", "ban", "lockdown"];
pub const RAID_FILTERS: [&str; 2] = ["age", "avatar"];
pub const ALT_RESPONSES: [&str; 2] = ["flag", "quarantine"];
pub const GATE_ACTIONS: [&str; 4] = ["flag", "quarantine", "kick", "ban"];

fn user_err(s: &str) -> Error {
    UserError(s.into()).into()
}

/// Out-of-range numbers are refused with this one sentence.
pub fn in_range(v: i32, lo: i32, hi: i32) -> Result<(), UserError> {
    if (lo..=hi).contains(&v) {
        Ok(())
    } else {
        Err(UserError(format!("Must be between {lo} and {hi}.")))
    }
}

/// Checks `v` and stores it; flags `changed`. `None` leaves the slot alone.
fn put(slot: &mut i32, v: Option<i32>, lo: i32, hi: i32, changed: &mut bool) -> Result<(), UserError> {
    if let Some(v) = v {
        in_range(v, lo, hi)?;
        *slot = v;
        *changed = true;
    }
    Ok(())
}

fn flag(slot: &mut bool, v: Option<bool>, changed: &mut bool) {
    if let Some(v) = v {
        *slot = v;
        *changed = true;
    }
}

fn nothing() -> UserError {
    UserError("Nothing to change.".into())
}

fn pick(v: &str, allowed: &[&str]) -> Result<String, UserError> {
    if allowed.contains(&v) {
        Ok(v.to_string())
    } else {
        Err(UserError(format!("Choose one of: {}.", allowed.join(", "))))
    }
}

async fn require_owner(inv: &Invoker<'_>) -> Result<(), Error> {
    let owner = inv.ctx.cache.guild(inv.guild_id).map(|g| g.owner_id);
    if owner == Some(inv.user.id) {
        Ok(())
    } else {
        Err(user_err("Only the server owner can do that."))
    }
}

fn gid(inv: &Invoker<'_>) -> i64 {
    inv.guild_id.get() as i64
}

async fn load(inv: &Invoker<'_>) -> Result<ProtectionSettings, Error> {
    Ok(store::get_settings(&inv.data.db, gid(inv)).await?)
}

async fn save(inv: &Invoker<'_>, s: &ProtectionSettings) -> Result<(), Error> {
    Ok(store::save_settings(&inv.data.db, s).await?)
}

fn onoff(on: bool) -> String {
    if on { "On".into() } else { "Off".into() }
}

fn plural(n: i32, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

// ---- lines shown by status and save replies (same text the panel pages show) ----

pub fn raid_lines(s: &ProtectionSettings) -> Vec<(&'static str, String)> {
    let filter = if s.raid_filter.is_empty() {
        "everyone".to_string()
    } else {
        s.raid_filter
            .iter()
            .map(|f| match f.as_str() {
                "age" => format!("account under {}", plural(s.raid_filter_days, "day")),
                "avatar" => "default avatar".into(),
                o => o.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" or ")
    };
    let responses = if s.raid_responses.is_empty() { "none".into() } else { s.raid_responses.join(", ") };
    vec![
        ("Raid mode", onoff(s.raid_enabled)),
        ("Trigger", format!("{} joins in {}s", s.raid_joins, s.raid_secs)),
        ("Responses", responses),
        ("Kick/ban filter", filter),
        ("Auto-end", format!("after {} min", s.raid_end_mins)),
    ]
}

pub fn alt_lines(s: &ProtectionSettings) -> Vec<(&'static str, String)> {
    vec![
        ("Alt detection", onoff(s.alt_enabled)),
        ("Young account", format!("under {}", plural(s.alt_days, "day"))),
        ("Points", format!("age {} · avatar {} · name {} · invite {}", s.alt_pts_age, s.alt_pts_avatar, s.alt_pts_name, s.alt_pts_invite)),
        ("Threshold", format!("{} points", s.alt_threshold)),
        ("Response", s.alt_response.clone()),
    ]
}

pub fn gate_lines(s: &ProtectionSettings) -> Vec<(&'static str, String)> {
    vec![("New account gate", onoff(s.gate_enabled)), ("Minimum age", plural(s.gate_days, "day")), ("Action", s.gate_action.clone())]
}

pub fn nuke_lines(s: &ProtectionSettings, trusted: usize) -> Vec<(&'static str, String)> {
    vec![
        ("Anti-nuke", onoff(s.nuke_enabled)),
        ("Window", format!("{}s", s.nuke_window_secs)),
        (
            "Limits",
            format!(
                "bans {} · kicks {} · channel deletes {} · role deletes {} · webhooks {} · dangerous permissions {}",
                s.nuke_bans, s.nuke_kicks, s.nuke_chan_del, s.nuke_role_del, s.nuke_webhooks, s.nuke_perms
            ),
        ),
        ("Trusted", trusted.to_string()),
    ]
}

fn saved(emoji: &str, what: &str, lines: Vec<(&'static str, String)>) -> CreateEmbed {
    reply::done(Tone::Safety, &format!("{emoji} {what} saved"), &lines, None)
}

// ---- pure edits ----

pub fn apply_raid(s: &mut ProtectionSettings, enabled: Option<bool>, joins: Option<i32>, secs: Option<i32>, filter_days: Option<i32>, end_mins: Option<i32>) -> Result<(), UserError> {
    let mut c = false;
    flag(&mut s.raid_enabled, enabled, &mut c);
    put(&mut s.raid_joins, joins, 2, 100, &mut c)?;
    put(&mut s.raid_secs, secs, 1, 120, &mut c)?;
    put(&mut s.raid_filter_days, filter_days, 0, 365, &mut c)?;
    put(&mut s.raid_end_mins, end_mins, 1, 1440, &mut c)?;
    if c { Ok(()) } else { Err(nothing()) }
}

#[derive(Default, Clone, Copy)]
pub struct Points {
    pub age: Option<i32>,
    pub avatar: Option<i32>,
    pub name: Option<i32>,
    pub invite: Option<i32>,
}

pub fn apply_alts(s: &mut ProtectionSettings, enabled: Option<bool>, days: Option<i32>, threshold: Option<i32>) -> Result<(), UserError> {
    let mut c = false;
    flag(&mut s.alt_enabled, enabled, &mut c);
    put(&mut s.alt_days, days, 0, 365, &mut c)?;
    put(&mut s.alt_threshold, threshold, 1, 400, &mut c)?;
    if c { Ok(()) } else { Err(nothing()) }
}

pub fn apply_points(s: &mut ProtectionSettings, p: Points) -> Result<(), UserError> {
    let mut c = false;
    put(&mut s.alt_pts_age, p.age, 0, 100, &mut c)?;
    put(&mut s.alt_pts_avatar, p.avatar, 0, 100, &mut c)?;
    put(&mut s.alt_pts_name, p.name, 0, 100, &mut c)?;
    put(&mut s.alt_pts_invite, p.invite, 0, 100, &mut c)?;
    if c { Ok(()) } else { Err(nothing()) }
}

#[derive(Default, Clone, Copy)]
pub struct NukeLimits {
    pub window: Option<i32>,
    pub bans: Option<i32>,
    pub kicks: Option<i32>,
    pub channel_deletes: Option<i32>,
    pub role_deletes: Option<i32>,
    pub webhooks: Option<i32>,
    pub perms: Option<i32>,
}

pub fn apply_limits(s: &mut ProtectionSettings, l: NukeLimits) -> Result<(), UserError> {
    let mut c = false;
    put(&mut s.nuke_window_secs, l.window, 1, 300, &mut c)?;
    put(&mut s.nuke_bans, l.bans, 1, 50, &mut c)?;
    put(&mut s.nuke_kicks, l.kicks, 1, 50, &mut c)?;
    put(&mut s.nuke_chan_del, l.channel_deletes, 1, 50, &mut c)?;
    put(&mut s.nuke_role_del, l.role_deletes, 1, 50, &mut c)?;
    put(&mut s.nuke_webhooks, l.webhooks, 1, 50, &mut c)?;
    put(&mut s.nuke_perms, l.perms, 1, 50, &mut c)?;
    if c { Ok(()) } else { Err(nothing()) }
}

// ---- raid ----

pub async fn raid_status_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let s = load(inv).await?;
    let active = store::active_raid(&inv.data.db, gid(inv)).await?;
    let mut lines = raid_lines(&s);
    if let Some(r) = active {
        lines.insert(0, ("Active raid", format!("started <t:{}:R>, ends <t:{}:R>", r.started_at.timestamp(), r.ends_at.timestamp())));
    }
    Ok(reply::done(Tone::Safety, &format!("{EMOJI_RAID} Raid mode"), &lines, None))
}

pub async fn raid_start_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let r = raid::start(inv.ctx, inv.data, inv.guild_id, Actor::Moderator(inv.user.id.get() as i64)).await?;
    Ok(reply::done(Tone::Safety, &format!("{EMOJI_RAID} Raid mode started"), &[("Auto-end", format!("<t:{}:R>", r.ends_at.timestamp()))], None))
}

pub async fn raid_end_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    raid::end(inv.ctx, inv.data, inv.guild_id, Actor::Moderator(inv.user.id.get() as i64)).await
}

pub async fn raid_settings_core(inv: &Invoker<'_>, enabled: Option<bool>, joins: Option<i32>, secs: Option<i32>, filter_days: Option<i32>, end_mins: Option<i32>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    apply_raid(&mut s, enabled, joins, secs, filter_days, end_mins)?;
    save(inv, &s).await?;
    Ok(saved(EMOJI_RAID, "Raid settings", raid_lines(&s)))
}

pub async fn raid_responses_core(inv: &Invoker<'_>, response: &str, add: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let r = pick(response, &RAID_RESPONSES)?;
    let mut s = load(inv).await?;
    toggle(&mut s.raid_responses, r, add, &RAID_RESPONSES);
    save(inv, &s).await?;
    Ok(saved(EMOJI_RAID, "Raid responses", raid_lines(&s)))
}

pub async fn raid_filter_core(inv: &Invoker<'_>, filter: &str, add: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let f = pick(filter, &RAID_FILTERS)?;
    let mut s = load(inv).await?;
    toggle(&mut s.raid_filter, f, add, &RAID_FILTERS);
    save(inv, &s).await?;
    Ok(saved(EMOJI_RAID, "Raid filter", raid_lines(&s)))
}

/// Adds or removes `v`, keeping the list in `order`.
fn toggle(list: &mut Vec<String>, v: String, add: bool, order: &[&str]) {
    list.retain(|x| *x != v);
    if add {
        list.push(v);
    }
    list.sort_by_key(|x| order.iter().position(|y| y == x));
}

// ---- alts ----

pub async fn alts_settings_core(inv: &Invoker<'_>, enabled: Option<bool>, days: Option<i32>, threshold: Option<i32>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    apply_alts(&mut s, enabled, days, threshold)?;
    save(inv, &s).await?;
    Ok(saved(EMOJI_ALTS, "Alt settings", alt_lines(&s)))
}

pub async fn alts_points_core(inv: &Invoker<'_>, p: Points) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    apply_points(&mut s, p)?;
    save(inv, &s).await?;
    Ok(saved(EMOJI_ALTS, "Alt points", alt_lines(&s)))
}

pub async fn alts_response_core(inv: &Invoker<'_>, response: &str) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    s.alt_response = pick(response, &ALT_RESPONSES)?;
    save(inv, &s).await?;
    Ok(saved(EMOJI_ALTS, "Alt response", alt_lines(&s)))
}

// ---- new accounts ----

pub async fn gate_enabled_core(inv: &Invoker<'_>, on: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    s.gate_enabled = on;
    save(inv, &s).await?;
    Ok(reply::done(Tone::Safety, &format!("{EMOJI_GATE} New account gate turned {}", if on { "on" } else { "off" }), &gate_lines(&s), None))
}

pub async fn gate_settings_core(inv: &Invoker<'_>, days: Option<i32>, action: Option<&str>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    let mut c = false;
    put(&mut s.gate_days, days, 0, 365, &mut c)?;
    if let Some(a) = action {
        s.gate_action = pick(a, &GATE_ACTIONS)?;
        c = true;
    }
    if !c {
        return Err(nothing().into());
    }
    save(inv, &s).await?;
    Ok(saved(EMOJI_GATE, "Gate settings", gate_lines(&s)))
}

// ---- anti-nuke ----

async fn trusted_count(inv: &Invoker<'_>) -> Result<usize, Error> {
    Ok(store::trusted(&inv.data.db, gid(inv)).await?.len())
}

pub async fn nuke_enabled_core(inv: &Invoker<'_>, on: bool) -> Result<CreateEmbed, Error> {
    if on {
        require_manage_server(inv).await?;
    } else {
        require_owner(inv).await?;
    }
    let mut s = load(inv).await?;
    s.nuke_enabled = on;
    save(inv, &s).await?;
    let n = trusted_count(inv).await?;
    Ok(reply::done(Tone::Safety, &format!("{EMOJI_NUKE} Anti-nuke turned {}", if on { "on" } else { "off" }), &nuke_lines(&s, n), None))
}

pub async fn nuke_limits_core(inv: &Invoker<'_>, l: NukeLimits) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    apply_limits(&mut s, l)?;
    save(inv, &s).await?;
    let n = trusted_count(inv).await?;
    Ok(saved(EMOJI_NUKE, "Anti-nuke limits", nuke_lines(&s, n)))
}

pub async fn trust_core(inv: &Invoker<'_>, kind: TrustKind, id: i64, add: bool) -> Result<CreateEmbed, Error> {
    require_owner(inv).await?;
    let g = gid(inv);
    let changed = if add { store::trust_add(&inv.data.db, g, kind, id).await? } else { store::trust_remove(&inv.data.db, g, kind, id).await? };
    let shown = match kind {
        TrustKind::User => format!("<@{id}>"),
        TrustKind::Role => format!("<@&{id}>"),
    };
    let note = match (add, changed) {
        (true, true) => "Now trusted",
        (true, false) => "Already trusted",
        (false, true) => "No longer trusted",
        (false, false) => "Was not trusted",
    };
    Ok(reply::done(Tone::Safety, &format!("{EMOJI_NUKE} Trusted list updated"), &[(note, shown)], None))
}

/// Owner gate first, then exactly one of user or role.
pub async fn trust_pick_core(inv: &Invoker<'_>, user: Option<i64>, role: Option<i64>, add: bool) -> Result<CreateEmbed, Error> {
    require_owner(inv).await?;
    let (k, id) = match (user, role) {
        (Some(u), None) => (TrustKind::User, u),
        (None, Some(r)) => (TrustKind::Role, r),
        _ => return Err(user_err("Give a user or a role, not both.")),
    };
    trust_core(inv, k, id, add).await
}

pub async fn trust_list_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let all = store::trusted(&inv.data.db, gid(inv)).await?;
    let of = |k: TrustKind, p: &str| all.iter().filter(|(x, _)| *x == k).map(|(_, id)| format!("<{p}{id}>")).collect::<Vec<_>>();
    Ok(reply::list(
        Tone::Safety,
        &format!("{EMOJI_NUKE} Trusted · {}", all.len()),
        Some("The owner and this bot are always trusted. Trusted members are never stripped."),
        &[("Members", of(TrustKind::User, "@")), ("Roles", of(TrustKind::Role, "@&"))],
    ))
}

// ---- slash wiring ----

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum RaidResponseChoice {
    #[name = "Alert mods"]
    Alert,
    #[name = "Quarantine raiders"]
    Quarantine,
    #[name = "Kick raiders"]
    Kick,
    #[name = "Ban raiders"]
    Ban,
    #[name = "Lock the server"]
    Lockdown,
}

impl RaidResponseChoice {
    fn key(self) -> &'static str {
        RAID_RESPONSES[self as usize]
    }
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum AltResponseChoice {
    #[name = "Flag for staff"]
    Flag,
    #[name = "Quarantine"]
    Quarantine,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum GateActionChoice {
    #[name = "Flag for staff"]
    Flag,
    #[name = "Quarantine"]
    Quarantine,
    #[name = "Kick"]
    Kick,
    #[name = "Ban"]
    Ban,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum AddRemove {
    #[name = "add"]
    Add,
    #[name = "remove"]
    Remove,
}

/// Raid mode: burst detection and responses.
#[poise::command(slash_command, guild_only, rename = "raid", subcommands("raid_status", "raid_start", "raid_end", "raid_settings", "raid_responses", "raid_filter"))]
pub async fn raid(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show raid mode settings and whether a raid is active.
#[poise::command(slash_command, guild_only, rename = "status")]
async fn raid_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_status_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Start raid mode now.
#[poise::command(slash_command, guild_only, rename = "start")]
async fn raid_start(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_start_core(&Invoker::from_poise(ctx)?).await?).await
}

/// End raid mode now.
#[poise::command(slash_command, guild_only, rename = "end")]
async fn raid_end(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_end_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Change when raid mode starts and ends.
#[poise::command(slash_command, guild_only, rename = "settings")]
async fn raid_settings(
    ctx: Context<'_>,
    #[description = "Turn automatic raid detection on or off"] enabled: Option<bool>,
    #[description = "Joins that trigger it (2-100)"] joins: Option<i32>,
    #[description = "Within this many seconds (1-120)"] secs: Option<i32>,
    #[description = "Account age for the kick/ban filter, in days (0-365)"] filter_days: Option<i32>,
    #[description = "Minutes until it ends by itself (1-1440)"] end_mins: Option<i32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_settings_core(&Invoker::from_poise(ctx)?, enabled, joins, secs, filter_days, end_mins).await?).await
}

/// Add or remove a raid response.
#[poise::command(slash_command, guild_only, rename = "responses")]
async fn raid_responses(ctx: Context<'_>, response: RaidResponseChoice, action: AddRemove) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_responses_core(&Invoker::from_poise(ctx)?, response.key(), matches!(action, AddRemove::Add)).await?).await
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum RaidFilterChoice {
    #[name = "New account"]
    Age,
    #[name = "Default avatar"]
    Avatar,
}

/// Add or remove a kick/ban filter: only matching raiders are kicked or banned.
#[poise::command(slash_command, guild_only, rename = "filter")]
async fn raid_filter(ctx: Context<'_>, filter: RaidFilterChoice, action: AddRemove) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, raid_filter_core(&Invoker::from_poise(ctx)?, RAID_FILTERS[filter as usize], matches!(action, AddRemove::Add)).await?).await
}

/// Alt detection: score joins against banned users.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "alts", subcommands("alts_settings", "alts_points", "alts_response"))]
pub async fn alts(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn alt detection on or off, or change its thresholds.
#[poise::command(slash_command, guild_only, rename = "settings")]
async fn alts_settings(
    ctx: Context<'_>,
    #[description = "Turn alt detection on or off"] enabled: Option<bool>,
    #[description = "Accounts younger than this many days score age points (0-365)"] days: Option<i32>,
    #[description = "Points needed to act (1-400)"] threshold: Option<i32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, alts_settings_core(&Invoker::from_poise(ctx)?, enabled, days, threshold).await?).await
}

/// Change how many points each signal adds.
#[poise::command(slash_command, guild_only, rename = "points")]
async fn alts_points(
    ctx: Context<'_>,
    #[description = "Young account (0-100)"] age: Option<i32>,
    #[description = "Default avatar (0-100)"] avatar: Option<i32>,
    #[description = "Name like a banned user (0-100)"] name: Option<i32>,
    #[description = "Joined with a banned user's invite (0-100)"] invite: Option<i32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, alts_points_core(&Invoker::from_poise(ctx)?, Points { age, avatar, name, invite }).await?).await
}

/// Choose what happens when the threshold is reached.
#[poise::command(slash_command, guild_only, rename = "response")]
async fn alts_response(ctx: Context<'_>, response: AltResponseChoice) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let r = match response {
        AltResponseChoice::Flag => "flag",
        AltResponseChoice::Quarantine => "quarantine",
    };
    reply::send(ctx, alts_response_core(&Invoker::from_poise(ctx)?, r).await?).await
}

/// New account gate: act on very new accounts.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "newaccounts", subcommands("gate_on", "gate_off", "gate_settings"))]
pub async fn newaccounts(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn the gate on.
#[poise::command(slash_command, guild_only, rename = "on")]
async fn gate_on(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, gate_enabled_core(&Invoker::from_poise(ctx)?, true).await?).await
}

/// Turn the gate off.
#[poise::command(slash_command, guild_only, rename = "off")]
async fn gate_off(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, gate_enabled_core(&Invoker::from_poise(ctx)?, false).await?).await
}

/// Change the minimum account age and what happens to younger accounts.
#[poise::command(slash_command, guild_only, rename = "settings")]
async fn gate_settings(ctx: Context<'_>, #[description = "Minimum account age in days (0-365)"] days: Option<i32>, action: Option<GateActionChoice>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let a = action.map(|a| match a {
        GateActionChoice::Flag => "flag",
        GateActionChoice::Quarantine => "quarantine",
        GateActionChoice::Kick => "kick",
        GateActionChoice::Ban => "ban",
    });
    reply::send(ctx, gate_settings_core(&Invoker::from_poise(ctx)?, days, a).await?).await
}

/// Anti-nuke: strip members who delete or ban too fast.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", rename = "antinuke", subcommands("nuke_on", "nuke_off", "nuke_limits", "nuke_trust"))]
pub async fn antinuke(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn anti-nuke on.
#[poise::command(slash_command, guild_only, rename = "on")]
async fn nuke_on(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, nuke_enabled_core(&Invoker::from_poise(ctx)?, true).await?).await
}

/// Turn anti-nuke off (server owner only).
#[poise::command(slash_command, guild_only, rename = "off")]
async fn nuke_off(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, nuke_enabled_core(&Invoker::from_poise(ctx)?, false).await?).await
}

/// Change the anti-nuke window and limits.
#[poise::command(slash_command, guild_only, rename = "limits")]
#[allow(clippy::too_many_arguments)]
async fn nuke_limits(
    ctx: Context<'_>,
    #[description = "Window in seconds (1-300)"] window: Option<i32>,
    #[description = "Bans allowed in the window (1-50)"] bans: Option<i32>,
    #[description = "Kicks allowed in the window (1-50)"] kicks: Option<i32>,
    #[description = "Channel deletes allowed (1-50)"] channel_deletes: Option<i32>,
    #[description = "Role deletes allowed (1-50)"] role_deletes: Option<i32>,
    #[description = "Webhook creations allowed (1-50)"] webhooks: Option<i32>,
    #[description = "Dangerous permission grants allowed (1-50)"] perms: Option<i32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let l = NukeLimits { window, bans, kicks, channel_deletes, role_deletes, webhooks, perms };
    reply::send(ctx, nuke_limits_core(&Invoker::from_poise(ctx)?, l).await?).await
}

/// Members and roles anti-nuke never acts on.
#[poise::command(slash_command, guild_only, rename = "trust", subcommands("trust_add", "trust_remove", "trust_list"))]
async fn nuke_trust(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Trust a member or a role (server owner only).
#[poise::command(slash_command, guild_only, rename = "add")]
async fn trust_add(ctx: Context<'_>, #[description = "Member to trust"] user: Option<User>, #[description = "Role to trust"] role: Option<Role>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, trust_pick_core(&Invoker::from_poise(ctx)?, user.map(|u| u.id.get() as i64), role.map(|r| r.id.get() as i64), true).await?).await
}

/// Stop trusting a member or a role (server owner only).
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn trust_remove(ctx: Context<'_>, #[description = "Member to stop trusting"] user: Option<User>, #[description = "Role to stop trusting"] role: Option<Role>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, trust_pick_core(&Invoker::from_poise(ctx)?, user.map(|u| u.id.get() as i64), role.map(|r| r.id.get() as i64), false).await?).await
}

/// Show the trusted list.
#[poise::command(slash_command, guild_only, rename = "list")]
async fn trust_list(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, trust_list_core(&Invoker::from_poise(ctx)?).await?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> ProtectionSettings {
        ProtectionSettings {
            guild_id: 1, raid_enabled: false, raid_joins: 10, raid_secs: 10,
            raid_responses: vec!["alert".into(), "quarantine".into()], raid_filter: vec![], raid_filter_days: 7, raid_end_mins: 10,
            alt_enabled: false, alt_days: 7, alt_pts_age: 40, alt_pts_avatar: 20, alt_pts_name: 40, alt_pts_invite: 30,
            alt_threshold: 60, alt_response: "flag".into(), gate_enabled: false, gate_days: 7, gate_action: "quarantine".into(),
            nuke_enabled: false, nuke_window_secs: 10, nuke_bans: 3, nuke_kicks: 3, nuke_chan_del: 2, nuke_role_del: 2, nuke_webhooks: 3, nuke_perms: 1,
        }
    }

    #[test]
    fn range_refusal() {
        assert!(in_range(2, 2, 100).is_ok() && in_range(100, 2, 100).is_ok());
        assert_eq!(in_range(1, 2, 100).unwrap_err().0, "Must be between 2 and 100.");
        assert_eq!(in_range(101, 2, 100).unwrap_err().0, "Must be between 2 and 100.");
    }

    #[test]
    fn ranges_per_setting() {
        let mut s = defaults();
        assert!(apply_raid(&mut s, None, Some(1), None, None, None).is_err());
        assert!(apply_raid(&mut s, None, None, Some(121), None, None).is_err());
        assert!(apply_raid(&mut s, None, None, None, Some(366), None).is_err());
        assert!(apply_raid(&mut s, None, None, None, None, Some(0)).is_err());
        assert_eq!(s, defaults(), "a refused edit changes nothing");
        apply_raid(&mut s, Some(true), Some(20), Some(30), Some(0), Some(1440)).unwrap();
        assert_eq!((s.raid_enabled, s.raid_joins, s.raid_secs, s.raid_filter_days, s.raid_end_mins), (true, 20, 30, 0, 1440));
        assert_eq!(apply_raid(&mut s, None, None, None, None, None).unwrap_err().0, "Nothing to change.");

        assert!(apply_alts(&mut s, None, Some(366), None).is_err());
        assert!(apply_alts(&mut s, None, None, Some(0)).is_err());
        assert!(apply_alts(&mut s, None, None, Some(401)).is_err());
        apply_alts(&mut s, Some(true), Some(0), Some(400)).unwrap();
        assert!(apply_points(&mut s, Points { name: Some(101), ..Default::default() }).is_err());
        apply_points(&mut s, Points { age: Some(0), invite: Some(100), ..Default::default() }).unwrap();
        assert_eq!((s.alt_pts_age, s.alt_pts_invite, s.alt_pts_name), (0, 100, 40));

        assert!(apply_limits(&mut s, NukeLimits { bans: Some(0), ..Default::default() }).is_err());
        assert!(apply_limits(&mut s, NukeLimits { perms: Some(51), ..Default::default() }).is_err());
        assert!(apply_limits(&mut s, NukeLimits { window: Some(301), ..Default::default() }).is_err());
        apply_limits(&mut s, NukeLimits { window: Some(300), webhooks: Some(50), ..Default::default() }).unwrap();
        assert_eq!((s.nuke_window_secs, s.nuke_webhooks, s.nuke_bans), (300, 50, 3));
    }

    #[test]
    fn choices_are_checked() {
        assert!(pick("ban", &RAID_RESPONSES).is_ok());
        assert!(pick("nuke", &RAID_RESPONSES).is_err());
        assert!(pick("kick", &ALT_RESPONSES).is_err());
        assert!(pick("kick", &GATE_ACTIONS).is_ok());
        use RaidResponseChoice as R;
        for (c, k) in [(R::Alert, "alert"), (R::Quarantine, "quarantine"), (R::Kick, "kick"), (R::Ban, "ban"), (R::Lockdown, "lockdown")] {
            assert_eq!(c.key(), k);
        }
        assert!(pick("age", &RAID_FILTERS).is_ok() && pick("ban", &RAID_FILTERS).is_err());
        assert_eq!(RAID_FILTERS[RaidFilterChoice::Avatar as usize], "avatar");
    }

    #[test]
    fn toggling_keeps_order() {
        let mut l = vec!["avatar".to_string()];
        toggle(&mut l, "age".into(), true, &RAID_FILTERS);
        assert_eq!(l, ["age", "avatar"]);
        toggle(&mut l, "age".into(), true, &RAID_FILTERS);
        assert_eq!(l, ["age", "avatar"]);
        toggle(&mut l, "age".into(), false, &RAID_FILTERS);
        assert_eq!(l, ["avatar"]);
        toggle(&mut l, "age".into(), false, &RAID_FILTERS);
        assert_eq!(l, ["avatar"]);
    }

    #[test]
    fn status_lines() {
        let s = defaults();
        let l = raid_lines(&s);
        assert_eq!(l[1].1, "10 joins in 10s");
        assert_eq!(l[2].1, "alert, quarantine");
        assert_eq!(l[3].1, "everyone");
        assert_eq!(l[4].1, "after 10 min");
        assert_eq!(alt_lines(&s)[2].1, "age 40 · avatar 20 · name 40 · invite 30");
        assert_eq!(gate_lines(&s)[1].1, "7 days");
        assert_eq!(nuke_lines(&s, 2)[3].1, "2");
        let mut f = defaults();
        f.raid_filter = vec!["age".into(), "avatar".into()];
        assert_eq!(raid_lines(&f)[3].1, "account under 7 days or default avatar");
    }
}

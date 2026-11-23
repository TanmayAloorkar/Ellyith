use crate::duration::format_duration;
use crate::invoker::Invoker;
use crate::logging::{send_log_message, LogCategory};
use crate::moderation::actions::{discord_error, record_or_log};
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::commands::punish::punish;
use crate::moderation::commands::{log_footer, username};
use crate::moderation::perms::{authorize, is_absent, member_facts, require_manage_server, require_member};
use crate::panel::id::PanelId;
use crate::reply::{self, Tone};
use crate::safety::logic::{roles_to_save, RoleInfo};
use crate::safety::store::{self, QuarantineRow, SafetySettings};
use crate::text::truncate;
use crate::{Data, Error, UserError};
use chrono::Utc;
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, CreateActionRow, CreateButton, CreateEmbed, CreateMessage, EditMember, EditMessage,
    EditRole, Guild, GuildChannel, GuildId, Member, MessageId, PermissionOverwrite, PermissionOverwriteType, Permissions,
    ReactionType, Role, RoleId, UserId,
};
use std::str::FromStr;

pub const NOT_SET_UP: &str = "Set up quarantine first in Config → Quarantine.";
const NOT_QUARANTINED: &str = "That member is not in quarantine.";
const APPROVE_EMOJI: &str = "✅";
const DENY_EMOJI: &str = "⛔";
const OPEN_ALLOW: Permissions =
    Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES).union(Permissions::READ_MESSAGE_HISTORY);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    Verification,
    Manual,
    Raid,
    Alt,
}

impl QuarantineReason {
    const ALL: [Self; 4] = [Self::Verification, Self::Manual, Self::Raid, Self::Alt];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Manual => "manual",
            Self::Raid => "raid",
            Self::Alt => "alt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Verification => "Waiting for verification",
            Self::Manual => "Quarantined by staff",
            Self::Raid => "Raid",
            Self::Alt => "Possible alt",
        }
    }
}

impl FromStr for QuarantineReason {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|r| r.as_str() == s).ok_or_else(|| format!("unknown quarantine reason {s}"))
    }
}

pub fn reason_label(raw: &str) -> &str {
    raw.parse::<QuarantineReason>().map(|r| r.label()).unwrap_or(raw)
}

/// Time since a unix timestamp, largest unit only ("3d", "5h").
fn ago(unix: i64) -> String {
    let d = format_duration(chrono::Duration::seconds((Utc::now().timestamp() - unix).max(0)));
    d.split(' ').next().unwrap_or("0s").to_string()
}

fn stamp() -> String {
    Utc::now().format("%H:%M").to_string()
}

pub fn require_setup(s: &SafetySettings, guild: &Guild) -> Result<(RoleId, ChannelId), UserError> {
    let role = s.quarantine_role_id.map(|r| RoleId::new(r as u64)).filter(|r| guild.roles.contains_key(r));
    let ch = s.review_channel_id.map(|c| ChannelId::new(c as u64)).filter(|c| guild.channels.contains_key(c));
    role.zip(ch).ok_or_else(|| UserError(NOT_SET_UP.into()))
}

fn setup_in(ctx: &serenity::Context, guild_id: GuildId, s: &SafetySettings) -> Result<(RoleId, ChannelId), Error> {
    let g = ctx.cache.guild(guild_id).ok_or("guild not in cache")?;
    Ok(require_setup(s, &g)?)
}

pub(super) async fn bot_top(ctx: &serenity::Context, guild_id: GuildId) -> Result<u16, Error> {
    let id = ctx.cache.current_user().id;
    let bot = guild_id.member(ctx, id).await?;
    Ok(member_facts(ctx, guild_id, &bot).await?.top_role_position)
}

pub fn validate_role(role: &Role, everyone: Permissions, bot_top: u16) -> Result<(), UserError> {
    let refuse = |m: &str| Err(UserError(m.into()));
    if role.managed {
        return refuse("That role is managed by an integration.");
    }
    if role.position >= bot_top {
        return refuse("That role is not below mine.");
    }
    if !(role.permissions - everyone).is_empty() {
        return refuse("That role has its own permissions; pick one without any.");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn enter(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    member: &Member,
    reason: QuarantineReason,
    actor: Actor,
    note: Option<String>,
    answers: Option<Vec<String>>,
) -> Result<(), Error> {
    let (gid, uid) = (guild_id.get() as i64, member.user.id.get() as i64);
    let s = store::get_settings(&data.db, gid).await?;
    let (role, _) = setup_in(ctx, guild_id, &s)?;
    let top = bot_top(ctx, guild_id).await?;
    let infos: Vec<RoleInfo> = {
        let g = ctx.cache.guild(guild_id).ok_or("guild not in cache")?;
        member
            .roles
            .iter()
            .filter(|r| **r != role)
            .filter_map(|r| g.roles.get(r))
            .map(|r| RoleInfo { id: r.id.get(), position: r.position, managed: r.managed })
            .collect()
    };
    let (saved, kept) = roles_to_save(&infos, top);
    let row = QuarantineRow {
        guild_id: gid,
        user_id: uid,
        reason: reason.as_str().into(),
        saved_roles: saved.iter().map(|r| *r as i64).collect(),
        answers,
        created_at: Utc::now(),
        review_channel_id: None,
        review_message_id: None,
    };
    // Row first: the saved roles are on disk before Discord removes them.
    if !store::quarantine_insert(&data.db, &row).await? {
        return Err(UserError("Already in quarantine.".into()).into());
    }
    let why = note.unwrap_or_else(|| reason.label().into());
    let roles: Vec<RoleId> = kept.into_iter().map(RoleId::new).chain([role]).collect();
    let audit = truncate(&why, 512);
    let edit = EditMember::new().roles(roles).audit_log_reason(&audit);
    if let Err(e) = guild_id.edit_member(ctx, member.user.id, edit).await {
        store::quarantine_delete(&data.db, gid, uid).await?;
        return Err(discord_error(e));
    }
    let case = NewCase {
        guild_id: gid,
        kind: CaseKind::Quarantine,
        target_id: uid,
        actor,
        reason: Some(why),
        duration_secs: None,
        expires_at: None,
        dm_failed: false,
    };
    record_or_log(ctx, data, None, case).await?;
    if reason != QuarantineReason::Verification || row.answers.is_some() {
        // Already quarantined: a missing card is not worth failing over (Waiting review still lists them).
        if let Err(e) = post_review_card(ctx, data, guild_id, &row).await {
            tracing::warn!("posting review card for {uid} failed: {e}");
        }
    }
    Ok(())
}

async fn actor_name(ctx: &serenity::Context, actor: Actor) -> String {
    match actor {
        Actor::Moderator(id) => UserId::new(id as u64).to_user(ctx).await.map(|u| format!("@{}", u.name)).unwrap_or_else(|_| "staff".into()),
        _ => "System".into(),
    }
}

/// Ids to give back, or a refusal naming every existing role the bot can no longer assign.
fn restore_plan(existing: &[(RoleInfo, String)], bot_top: u16) -> Result<Vec<u64>, UserError> {
    let blocked: Vec<String> = existing.iter().filter(|(r, _)| r.managed || r.position >= bot_top).map(|(_, n)| format!("@{n}")).collect();
    if !blocked.is_empty() {
        return Err(UserError(format!(
            "I can't give back {} because it is not below my role (or is managed by an integration). Move my role higher, then try again.",
            blocked.join(", ")
        )));
    }
    Ok(existing.iter().map(|(r, _)| r.id).collect())
}

pub async fn release(ctx: &serenity::Context, data: &Data, guild_id: GuildId, user: UserId, actor: Actor) -> Result<bool, Error> {
    let (gid, uid) = (guild_id.get() as i64, user.get() as i64);
    let Some(row) = store::quarantine_get(&data.db, gid, uid).await? else { return Ok(false) };
    let s = store::get_settings(&data.db, gid).await?;
    match guild_id.member(ctx, user).await {
        Ok(m) => {
            let q = s.quarantine_role_id.map(|r| RoleId::new(r as u64));
            let verified: &[i64] = if row.reason == QuarantineReason::Verification.as_str() { &s.verify_roles } else { &[] };
            let top = bot_top(ctx, guild_id).await?;
            let existing: Vec<(RoleInfo, String)> = {
                let g = ctx.cache.guild(guild_id).ok_or("guild not in cache")?;
                // Roles deleted from the server are skipped.
                row.saved_roles
                    .iter()
                    .chain(verified)
                    .filter_map(|r| g.roles.get(&RoleId::new(*r as u64)))
                    .map(|r| (RoleInfo { id: r.id.get(), position: r.position, managed: r.managed }, r.name.clone()))
                    .collect()
            };
            let restore = restore_plan(&existing, top)?;
            let mut roles: Vec<RoleId> =
                m.roles.iter().copied().filter(|r| Some(*r) != q).chain(restore.into_iter().map(RoleId::new)).collect();
            roles.sort();
            roles.dedup();
            let edit = EditMember::new().roles(roles).audit_log_reason("Released from quarantine");
            guild_id.edit_member(ctx, user, edit).await.map_err(discord_error)?;
        }
        // Left the server: nothing to restore, the row just goes.
        Err(e) if is_absent(&e) => {}
        Err(e) => return Err(e.into()),
    }
    if !store::quarantine_delete(&data.db, gid, uid).await? {
        return Ok(false);
    }
    let case = NewCase {
        guild_id: gid,
        kind: CaseKind::QuarantineRelease,
        target_id: uid,
        actor,
        reason: None,
        duration_secs: None,
        expires_at: None,
        dm_failed: false,
    };
    record_or_log(ctx, data, None, case).await?;
    let footer = match actor {
        Actor::Moderator(_) => format!("{APPROVE_EMOJI} Approved by {} · {}", actor_name(ctx, actor).await, stamp()),
        _ => format!("{APPROVE_EMOJI} Verified · {}", stamp()),
    };
    mark_card(ctx, data, &row, footer).await;
    Ok(true)
}

pub async fn deny(inv: &Invoker<'_>, user: UserId) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, Some(user)).await?;
    let (gid, uid) = (inv.guild_id.get() as i64, user.get() as i64);
    let row = store::quarantine_get(&inv.data.db, gid, uid).await?.ok_or_else(|| UserError(NOT_QUARANTINED.into()))?;
    let s = store::get_settings(&inv.data.db, gid).await?;
    let kind = if s.deny_action == "ban" { CaseKind::Ban } else { CaseKind::Kick };
    let present = match inv.guild_id.member(inv.ctx, user).await {
        Ok(_) => true,
        Err(e) if is_absent(&e) => false,
        Err(e) => return Err(e.into()),
    };
    let by = format!("{DENY_EMOJI} Denied by @{} · {}", inv.user.name, stamp());
    // Claim the row first so a second Deny click does nothing.
    if !store::quarantine_delete(&inv.data.db, gid, uid).await? {
        return Err(UserError("Already handled.".into()).into());
    }
    if !present && kind == CaseKind::Kick {
        mark_card(inv.ctx, inv.data, &row, format!("{by} · left the server")).await;
        let name = username(inv, user).await;
        let lines = [("Note", "They had already left the server.".to_string())];
        return Ok(reply::done(Tone::Safety, &format!("{DENY_EMOJI} Denied {name}"), &lines, log_footer(inv).await.as_deref()));
    }
    let reply = match punish(inv, kind, user, Some("Quarantine denied".into()), None, 0).await {
        Ok(r) => r,
        Err(e) => {
            // Put the row back so the saved roles are never lost.
            store::quarantine_insert(&inv.data.db, &row).await?;
            return Err(e);
        }
    };
    mark_card(inv.ctx, inv.data, &row, by).await;
    Ok(reply)
}

pub fn review_card(row: &QuarantineRow, username: &str, account_age: &str, joined: &str, deny_action: &str) -> (CreateEmbed, Vec<CreateActionRow>) {
    let mut lines = vec![("Why", reason_label(&row.reason).to_string()), ("Account age", account_age.into()), ("Joined", joined.into())];
    if let Some(a) = &row.answers {
        // Keep the whole Answers line within reply::done's 1024-char line cap.
        let per = (1000 / a.len().max(1)).saturating_sub(6);
        let text: Vec<String> = a.iter().enumerate().map(|(i, x)| format!("{}. {}", i + 1, truncate(x, per))).collect();
        lines.push(("Answers", format!("\n{}", text.join("\n"))));
    }
    let footer = if deny_action == "ban" { "Deny bans them (set to kick in Config)" } else { "Deny kicks them (set to ban in Config)" };
    let embed = reply::done(Tone::Safety, &format!("🚧 {username} quarantined"), &lines, Some(footer));
    let user = UserId::new(row.user_id as u64);
    let buttons = vec![
        CreateButton::new(PanelId::Review(true, user).encode()).label("Approve").emoji(ReactionType::Unicode(APPROVE_EMOJI.into())).style(ButtonStyle::Success),
        CreateButton::new(PanelId::Review(false, user).encode()).label("Deny").emoji(ReactionType::Unicode(DENY_EMOJI.into())).style(ButtonStyle::Danger),
    ];
    (embed, vec![CreateActionRow::Buttons(buttons)])
}

pub fn handled_card(row_title: &str, why: &str, footer: &str) -> CreateEmbed {
    reply::done(Tone::Safety, row_title, &[("Why", why.to_string())], Some(footer))
}

/// The review card for a quarantined member, with live account details.
pub async fn card_for(ctx: &serenity::Context, data: &Data, guild_id: GuildId, row: &QuarantineRow) -> Result<(CreateEmbed, Vec<CreateActionRow>), Error> {
    let s = store::get_settings(&data.db, row.guild_id).await?;
    let user = UserId::new(row.user_id as u64);
    let (name, age, joined) = match guild_id.member(ctx, user).await {
        Ok(m) => {
            let joined = m.joined_at.map_or_else(|| "unknown".into(), |t| format!("{} ago", ago(t.unix_timestamp())));
            (m.user.name.clone(), ago(m.user.created_at().unix_timestamp()), joined)
        }
        Err(_) => {
            let u = user.to_user(ctx).await?;
            (u.name.clone(), ago(u.created_at().unix_timestamp()), "left the server".into())
        }
    };
    Ok(review_card(row, &name, &age, &joined, &s.deny_action))
}

pub async fn post_review_card(ctx: &serenity::Context, data: &Data, guild_id: GuildId, row: &QuarantineRow) -> Result<(), Error> {
    let (embed, rows) = card_for(ctx, data, guild_id, row).await?;
    let msg = CreateMessage::new().embed(embed).components(rows);
    if let Some(m) = send_log_message(&ctx.http, &data.db, guild_id, LogCategory::ModActions, msg).await {
        store::quarantine_set_review(&data.db, row.guild_id, row.user_id, m.channel_id.get() as i64, m.id.get() as i64).await?;
    }
    Ok(())
}

pub async fn mark_card(ctx: &serenity::Context, _data: &Data, row: &QuarantineRow, footer: String) {
    let (Some(ch), Some(msg)) = (row.review_channel_id, row.review_message_id) else { return };
    let (ch, msg) = (ChannelId::new(ch as u64), MessageId::new(msg as u64));
    // Card (or its channel) deleted: nothing to mark.
    let Ok(m) = ch.message(ctx, msg).await else { return };
    let title = m.embeds.first().and_then(|e| e.title.clone()).unwrap_or_else(|| "🚧 Quarantine".into());
    let edit = EditMessage::new().embed(handled_card(&title, reason_label(&row.reason), &footer)).components(vec![]);
    if let Err(e) = ch.edit_message(ctx, msg, edit).await {
        tracing::warn!("marking review card {msg} failed: {e}");
    }
}

pub async fn quarantine_core(inv: &Invoker<'_>, user: UserId, reason: String) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, Some(user)).await?;
    let member = require_member(inv, user).await?;
    let actor = Actor::Moderator(inv.user.id.get() as i64);
    enter(inv.ctx, inv.data, inv.guild_id, &member, QuarantineReason::Manual, actor, Some(reason.clone()), None).await?;
    let title = format!("🚧 Quarantined {}", member.user.name);
    Ok(reply::done(Tone::Safety, &title, &[("Reason", reason)], log_footer(inv).await.as_deref()))
}

pub async fn release_core(inv: &Invoker<'_>, user: UserId) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, Some(user)).await?;
    if !release(inv.ctx, inv.data, inv.guild_id, user, Actor::Moderator(inv.user.id.get() as i64)).await? {
        return Err(UserError(NOT_QUARANTINED.into()).into());
    }
    let title = format!("{APPROVE_EMOJI} Released {}", username(inv, user).await);
    Ok(reply::done(Tone::Safety, &title, &[("Roles", "Restored".into())], log_footer(inv).await.as_deref()))
}

pub async fn list_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let rows = store::quarantine_list(&inv.data.db, inv.guild_id.get() as i64).await?;
    let lines: Vec<String> = rows
        .iter()
        .take(25)
        .map(|r| format!("<@{}> · {} · {} ago", r.user_id, reason_label(&r.reason), ago(r.created_at.timestamp())))
        .collect();
    let body = if lines.is_empty() { "Nobody is in quarantine.".to_string() } else { lines.join("\n") };
    Ok(reply::list(Tone::Safety, "📋 Waiting review", Some(&body), &[]))
}

/// View Channel deny for `role` everywhere, except an allow on the review and verify channels.
async fn sync(ctx: &serenity::Context, guild_id: GuildId, role: RoleId, s: &SafetySettings) -> Result<(usize, Vec<String>), Error> {
    let channels: Vec<(ChannelId, String)> = {
        let g = ctx.cache.guild(guild_id).ok_or("guild not in cache")?;
        g.channels.values().map(|c| (c.id, c.name.clone())).collect()
    };
    let open = [s.review_channel_id, s.verify_channel_id];
    let (mut updated, mut failed) = (0, Vec::new());
    for (id, name) in channels {
        let is_open = open.contains(&Some(id.get() as i64));
        let ow = PermissionOverwrite {
            allow: if is_open { OPEN_ALLOW } else { Permissions::empty() },
            deny: if is_open { Permissions::empty() } else { Permissions::VIEW_CHANNEL },
            kind: PermissionOverwriteType::Role(role),
        };
        match id.create_permission(&ctx.http, ow).await {
            Ok(()) => updated += 1,
            Err(e) => {
                tracing::warn!("quarantine sync on channel {id} failed: {e}");
                failed.push(format!("#{name}"));
            }
        }
    }
    Ok((updated, failed))
}

pub fn sync_lines(updated: usize, failed: Vec<String>) -> Vec<(&'static str, String)> {
    let mut lines = vec![("Updated", format!("{updated} channels"))];
    if !failed.is_empty() {
        let more = failed.len().saturating_sub(10);
        let mut names = failed.into_iter().take(10).collect::<Vec<_>>().join(", ");
        if more > 0 {
            names += &format!(" +{more}");
        }
        lines.push(("Couldn't edit", names));
    }
    lines
}

fn sync_reply(title: &str, updated: usize, failed: Vec<String>) -> CreateEmbed {
    reply::done(Tone::Safety, title, &sync_lines(updated, failed), None)
}

pub async fn setup_sync(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    let (updated, failed) = setup_sync_counts(inv).await?;
    Ok(sync_reply("🚧 Channel permissions updated", updated, failed))
}

/// Same sync as `setup_sync`, returning the counts so config replies can carry them.
pub async fn setup_sync_counts(inv: &Invoker<'_>) -> Result<(usize, Vec<String>), Error> {
    require_manage_server(inv).await?;
    let s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    let role = {
        let g = inv.ctx.cache.guild(inv.guild_id).ok_or("guild not in cache")?;
        s.quarantine_role_id.map(|r| RoleId::new(r as u64)).filter(|r| g.roles.contains_key(r))
    }
    .ok_or_else(|| UserError(NOT_SET_UP.into()))?;
    sync(inv.ctx, inv.guild_id, role, &s).await
}

/// Release removes only the current quarantine role, so it can't change while anyone holds the old one.
/// `new` None = a role about to be created.
pub fn role_change_check(current: Option<i64>, new: Option<i64>, anyone_quarantined: bool) -> Result<(), UserError> {
    if anyone_quarantined && (new.is_none() || current != new) {
        return Err(UserError("Release everyone in quarantine before changing the quarantine role.".into()));
    }
    Ok(())
}

pub async fn require_role_change_ok(inv: &Invoker<'_>, new: Option<i64>) -> Result<(), Error> {
    let gid = inv.guild_id.get() as i64;
    let current = store::get_settings(&inv.data.db, gid).await?.quarantine_role_id;
    let anyone = !store::quarantine_list(&inv.data.db, gid).await?.is_empty();
    Ok(role_change_check(current, new, anyone)?)
}

pub async fn create_role(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    require_role_change_ok(inv, None).await?;
    let top = bot_top(inv.ctx, inv.guild_id).await?;
    let builder = EditRole::new().name("Quarantined").permissions(Permissions::empty()).audit_log_reason("Quarantine setup");
    let role = inv.guild_id.create_role(inv.ctx, builder).await.map_err(discord_error)?;
    // Directly below the bot's top role; Discord shifts the roles in between down one.
    inv.guild_id.edit_role_position(&inv.ctx.http, role.id, top.saturating_sub(1).max(1)).await.map_err(discord_error)?;
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.quarantine_role_id = Some(role.id.get() as i64);
    store::save_settings(&inv.data.db, &s).await?;
    let (updated, failed) = sync(inv.ctx, inv.guild_id, role.id, &s).await?;
    Ok(sync_reply("🚧 Created role Quarantined", updated, failed))
}

pub async fn on_channel_create(ctx: &serenity::Context, data: &Data, channel: &GuildChannel) {
    let s = match store::get_settings(&data.db, channel.guild_id.get() as i64).await {
        Ok(s) => s,
        Err(e) => return tracing::warn!("quarantine settings lookup failed: {e}"),
    };
    let Some(role) = s.quarantine_role_id else { return };
    let ow = PermissionOverwrite {
        allow: Permissions::empty(),
        deny: Permissions::VIEW_CHANNEL,
        kind: PermissionOverwriteType::Role(RoleId::new(role as u64)),
    };
    if let Err(e) = channel.id.create_permission(&ctx.http, ow).await {
        tracing::warn!("quarantine overwrite on new channel {} failed: {e}", channel.id);
    }
}

/// True when the member is (now) in quarantine, so other join checks should skip them.
pub async fn on_member_join(ctx: &serenity::Context, data: &Data, member: &Member) -> bool {
    match join(ctx, data, member).await {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("quarantine on join failed for {}: {e}", member.user.id);
            false
        }
    }
}

async fn join(ctx: &serenity::Context, data: &Data, member: &Member) -> Result<bool, Error> {
    let (gid, uid) = (member.guild_id.get() as i64, member.user.id.get() as i64);
    let s = store::get_settings(&data.db, gid).await?;
    if store::quarantine_get(&data.db, gid, uid).await?.is_some() {
        if let Some(r) = s.quarantine_role_id {
            ctx.http.add_member_role(member.guild_id, member.user.id, RoleId::new(r as u64), Some("Still in quarantine")).await?;
        }
        return Ok(true);
    }
    // Bots cannot press Verify; an unfinished setup lets members in rather than failing every join.
    if s.verify_mode == "off" || member.user.bot || setup_in(ctx, member.guild_id, &s).is_err() {
        return Ok(false);
    }
    enter(ctx, data, member.guild_id, member, QuarantineReason::Verification, Actor::System, None, None).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn sync_lines_lists_failures_only_when_present() {
        let l = super::sync_lines(5, vec![]);
        assert_eq!(l, vec![("Updated", "5 channels".to_string())]);
        let failed: Vec<String> = (0..12).map(|i| format!("#c{i}")).collect();
        let l = super::sync_lines(3, failed);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].0, "Couldn't edit");
        assert!(l[1].1.ends_with("#c9 +2"), "{}", l[1].1);
        assert!(!l[1].1.contains("#c10"));
    }

    use super::*;

    #[test]
    fn reason_round_trip() {
        for r in [QuarantineReason::Verification, QuarantineReason::Manual, QuarantineReason::Raid, QuarantineReason::Alt] {
            assert_eq!(r.as_str().parse::<QuarantineReason>(), Ok(r));
        }
        assert_eq!(QuarantineReason::Alt.as_str(), "alt");
        assert_eq!(QuarantineReason::Verification.label(), "Waiting for verification");
        assert!("nope".parse::<QuarantineReason>().is_err());
    }

    fn role(position: u16, managed: bool, permissions: Permissions) -> Role {
        serde_json::from_value(serde_json::json!({
            "id": "5", "color": 0, "colors": { "primary_color": 0 }, "hoist": false, "managed": managed,
            "name": "q", "permissions": permissions.bits().to_string(), "position": position,
        }))
        .unwrap()
    }

    #[test]
    fn validate_role_rules() {
        let everyone = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES;
        assert_eq!(validate_role(&role(3, true, Permissions::empty()), everyone, 10).unwrap_err().0, "That role is managed by an integration.");
        assert_eq!(validate_role(&role(10, false, Permissions::empty()), everyone, 10).unwrap_err().0, "That role is not below mine.");
        assert_eq!(
            validate_role(&role(3, false, Permissions::KICK_MEMBERS), everyone, 10).unwrap_err().0,
            "That role has its own permissions; pick one without any."
        );
        assert!(validate_role(&role(3, false, Permissions::VIEW_CHANNEL), everyone, 10).is_ok());
    }

    #[test]
    fn role_change_refused_while_quarantined() {
        let msg = "Release everyone in quarantine before changing the quarantine role.";
        assert_eq!(role_change_check(Some(1), Some(2), true).unwrap_err().0, msg);
        assert_eq!(role_change_check(Some(1), None, true).unwrap_err().0, msg);
        assert_eq!(role_change_check(None, None, true).unwrap_err().0, msg);
        assert!(role_change_check(Some(1), Some(1), true).is_ok());
        assert!(role_change_check(Some(1), Some(2), false).is_ok());
        assert!(role_change_check(Some(1), None, false).is_ok());
    }

    #[test]
    fn restore_plan_refuses_unassignable() {
        let r = |id, position, managed, n: &str| (RoleInfo { id, position, managed }, n.to_string());
        assert_eq!(restore_plan(&[r(1, 3, false, "A"), r(2, 4, false, "B")], 10).unwrap(), [1, 2]);
        let err = restore_plan(&[r(1, 3, false, "A"), r(2, 10, false, "High"), r(3, 2, true, "Bot")], 10).unwrap_err().0;
        assert!(err.contains("@High, @Bot") && !err.contains("@A") && err.contains("Move my role higher"), "{err}");
    }

    fn row(answers: Option<Vec<String>>) -> QuarantineRow {
        QuarantineRow {
            guild_id: 1,
            user_id: 42,
            reason: "manual".into(),
            saved_roles: vec![],
            answers,
            created_at: chrono::Utc::now(),
            review_channel_id: None,
            review_message_id: None,
        }
    }

    #[test]
    fn review_card_shape() {
        let (e, rows) = review_card(&row(None), "newuser123", "3d", "5m ago", "kick");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["title"], "🚧 newuser123 quarantined");
        assert_eq!(v["footer"]["text"], "Deny kicks them (set to ban in Config)");
        let desc = v["description"].as_str().unwrap();
        assert!(desc.contains("Quarantined by staff") && desc.contains("3d") && desc.contains("5m ago"), "{desc}");
        assert!(!desc.contains("Answers"));
        let r = serde_json::to_value(&rows).unwrap();
        let ids: Vec<&str> = r[0]["components"].as_array().unwrap().iter().map(|b| b["custom_id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["mp:rv:a:42", "mp:rv:d:42"]);

        let (e, _) = review_card(&row(Some(vec!["blue".into(), "x".repeat(500)])), "newuser123", "3d", "5m ago", "kick");
        let desc = serde_json::to_value(&e).unwrap()["description"].as_str().unwrap().to_string();
        assert!(desc.contains("**Answers**") && desc.contains("blue"), "{desc}");
    }
}

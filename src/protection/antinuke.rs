//! Anti-nuke: per-person limits on destructive actions, read from the audit log and the bot's own commands.
use super::logic::{counts_toward_nuke, gained_dangerous, is_trusted, nuke_limit, NukeHit, NukeKind, PunishSource};
use super::store::{self, TrustKind};
use super::{raid, EMOJI_NUKE};
use crate::logging::{send_log_message, LogCategory};
use crate::moderation::actions::{discord_error, execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::perms::is_absent;
use crate::panel::id::ProtId;
use crate::reply::{self, Tone};
use crate::safety::logic::{roles_to_save, RoleInfo};
use crate::safety::quarantine::bot_top;
use crate::text::truncate;
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity,
    audit_log::{Action, AuditLogEntry, ChannelAction, Change, MemberAction, RoleAction, WebhookAction},
    ButtonStyle, CreateActionRow, CreateAllowedMentions, CreateEmbed, CreateMessage, EditMember, GuildId, Permissions, RoleId, UserId,
};
use std::time::{Duration, Instant};

pub const HIERARCHY: &str = "Could not remove roles: their top role is above the bot's";
const FOOTER: &str = "The bot can't bring deleted channels back.";
pub const NOT_ALLOWED: &str = "Only the owner or a trusted member can do that.";

/// The watched kind for an audit action; `perm_change` is (old, new) permissions for role edits and role grants.
pub fn kind_for(action: &Action, perm_change: Option<(Permissions, Permissions)>) -> Option<NukeKind> {
    let dangerous = perm_change.is_some_and(|(old, new)| gained_dangerous(old, new));
    match action {
        Action::Member(MemberAction::BanAdd) => Some(NukeKind::Ban),
        Action::Member(MemberAction::Kick) => Some(NukeKind::Kick),
        Action::Channel(ChannelAction::Delete) => Some(NukeKind::ChannelDelete),
        Action::Role(RoleAction::Delete) => Some(NukeKind::RoleDelete),
        Action::Webhook(WebhookAction::Create) => Some(NukeKind::Webhook),
        Action::Role(RoleAction::Create | RoleAction::Update) | Action::Member(MemberAction::RoleUpdate) if dangerous => Some(NukeKind::DangerousPerm),
        _ => None,
    }
}

/// A bot command's kick or ban counts only when its source is a moderator's own command (spec §10.1).
pub fn nuke_kind(src: PunishSource, kind: CaseKind) -> Option<NukeKind> {
    match kind {
        _ if !counts_toward_nuke(src) => None,
        CaseKind::Kick => Some(NukeKind::Kick),
        CaseKind::Ban | CaseKind::Tempban => Some(NukeKind::Ban),
        _ => None,
    }
}

/// "deleted 3 channels in 6 seconds (limit 2)".
pub fn what(kind: NukeKind, count: usize, span: Duration, limit: i32) -> String {
    let (verb, one, many) = match kind {
        NukeKind::Ban => ("banned", "member", "members"),
        NukeKind::Kick => ("kicked", "member", "members"),
        NukeKind::ChannelDelete => ("deleted", "channel", "channels"),
        NukeKind::RoleDelete => ("deleted", "role", "roles"),
        NukeKind::Webhook => ("made", "webhook", "webhooks"),
        NukeKind::DangerousPerm => ("gave dangerous permissions", "time", "times"),
    };
    let noun = if count == 1 { one } else { many };
    let secs = span.as_millis().div_ceil(1000).max(1);
    let when = if count > 1 { format!(" in {secs} second{}", if secs == 1 { "" } else { "s" }) } else { String::new() };
    format!("{verb} {count} {noun}{when} (limit {limit})")
}

/// Field label for the names collected with a hit.
fn names_label(kind: NukeKind) -> &'static str {
    match kind {
        NukeKind::ChannelDelete | NukeKind::RoleDelete => "Deleted",
        _ => "Targets",
    }
}

pub fn alert_embed(user: UserId, what: &str, done: &str, names: Option<(&str, &[String])>, removed: &[i64]) -> CreateEmbed {
    let mut lines = vec![("Done", done.to_string())];
    if let Some((label, n)) = names.filter(|(_, n)| !n.is_empty()) {
        lines.push((label, truncate(&n.join(", "), 1024)));
    }
    let roles = if removed.is_empty() { "None".into() } else { removed.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(", ") };
    lines.push(("Removed roles", truncate(&roles, 1024)));
    let body: Vec<String> = lines.iter().map(|(l, v)| format!("**{l}** {v}")).collect();
    reply::done(Tone::Safety, &format!("{EMOJI_NUKE} Anti-nuke triggered"), &[], Some(FOOTER)).description(format!("<@{user}> {what}\n{}", body.join("\n")))
}

fn buttons(strip: i64, name: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        raid::button(ProtId::NukeRestore(strip), "Give roles back", "↩️", ButtonStyle::Secondary),
        raid::button(ProtId::NukeBan(strip), &format!("Ban @{name}"), "🔨", ButtonStyle::Danger),
    ])]
}

/// Owner, this bot, or on the trusted list (directly or by role).
pub async fn trusted(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId) -> Result<bool, Error> {
    let owner = ctx.cache.guild(guild).map(|g| g.owner_id).ok_or("guild not in cache")?;
    let bot = ctx.cache.current_user().id;
    if user == owner || user == bot {
        return Ok(true);
    }
    let list = store::trusted(&data.db, guild.get() as i64).await?;
    let roles: Vec<u64> = if list.iter().any(|(k, _)| *k == TrustKind::Role) {
        match guild.member(ctx, user).await {
            Ok(m) => m.roles.iter().map(|r| r.get()).collect(),
            Err(e) if is_absent(&e) => vec![],
            Err(e) => return Err(e.into()),
        }
    } else {
        vec![]
    };
    Ok(is_trusted(user.get(), owner.get(), bot.get(), &roles, &list))
}

/// Count one action; strip the actor when it reaches the limit. Off, or a trusted actor: nothing.
async fn count(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, kind: NukeKind, name: Option<String>) -> Result<(), Error> {
    let s = store::get_settings(&data.db, guild.get() as i64).await?;
    if !s.nuke_enabled || trusted(ctx, data, guild, user).await? {
        return Ok(());
    }
    let limit = nuke_limit(&s, kind);
    let window = Duration::from_secs(s.nuke_window_secs.max(1) as u64);
    let Some(NukeHit { count, span, names }) = data.protection.nuke.record(guild.get(), user.get(), kind, Instant::now(), window, limit, name) else {
        return Ok(());
    };
    strip(ctx, data, guild, user, what(kind, count, span, limit), Some((names_label(kind), names))).await
}

/// After a successful kick or ban from a moderator's own command or panel action. Failures are logged only.
pub async fn count_command(ctx: &serenity::Context, data: &Data, guild: GuildId, moderator: UserId, kind: NukeKind, target: UserId) {
    if let Err(e) = count(ctx, data, guild, moderator, kind, Some(format!("<@{target}>"))).await {
        tracing::warn!("anti-nuke command count in {guild} failed: {e}");
    }
}

fn perm_change(ctx: &serenity::Context, guild: GuildId, entry: &AuditLogEntry) -> Option<(Permissions, Permissions)> {
    let changes = entry.changes.as_deref().unwrap_or_default();
    match entry.action {
        Action::Role(_) => changes.iter().find_map(|c| match c {
            Change::Permissions { old, new } => Some((old.unwrap_or_default(), new.unwrap_or_default())),
            _ => None,
        }),
        // A role grant: what the added roles carry counts as newly given.
        Action::Member(MemberAction::RoleUpdate) => {
            let g = ctx.cache.guild(guild)?;
            let added = changes.iter().filter_map(|c| match c {
                Change::RolesAdded { new: Some(r), .. } => Some(r),
                _ => None,
            });
            let perms = added.flatten().filter_map(|r| g.roles.get(&r.id)).fold(Permissions::empty(), |a, r| a | r.permissions);
            Some((Permissions::empty(), perms))
        }
        _ => None,
    }
}

/// What the entry touched, by name where the audit log has it.
fn entry_name(kind: NukeKind, entry: &AuditLogEntry) -> Option<String> {
    let changes = entry.changes.as_deref().unwrap_or_default();
    let name = |old: bool| {
        changes.iter().find_map(|c| match c {
            Change::Name { old: o, new: n } => if old { o.clone() } else { n.clone() },
            _ => None,
        })
    };
    let target = entry.target_id.map(|t| t.get());
    match kind {
        NukeKind::ChannelDelete => name(true).map(|n| format!("#{n}")),
        NukeKind::RoleDelete => name(true).map(|n| format!("@{n}")),
        NukeKind::Webhook => name(false),
        NukeKind::DangerousPerm if matches!(entry.action, Action::Role(_)) => target.map(|t| format!("<@&{t}>")),
        NukeKind::Ban | NukeKind::Kick | NukeKind::DangerousPerm => target.map(|t| format!("<@{t}>")),
    }
}

pub async fn on_entry(ctx: &serenity::Context, data: &Data, guild: GuildId, entry: &AuditLogEntry) {
    // The bot's own kicks and bans were counted against the moderator who ran the command.
    if entry.user_id == ctx.cache.current_user().id {
        return;
    }
    if let Err(e) = handle_entry(ctx, data, guild, entry).await {
        tracing::warn!("anti-nuke on audit entry {} in {guild} failed: {e}", entry.id);
    }
}

async fn handle_entry(ctx: &serenity::Context, data: &Data, guild: GuildId, entry: &AuditLogEntry) -> Result<(), Error> {
    let actor = entry.user_id;
    if let Action::Member(MemberAction::BotAdd) = entry.action {
        if !store::get_settings(&data.db, guild.get() as i64).await?.nuke_enabled || trusted(ctx, data, guild, actor).await? {
            return Ok(());
        }
        let Some(bot) = entry.target_id.map(|t| UserId::new(t.get())) else { return Ok(()) };
        let what = match guild.kick_with_reason(ctx, bot, "Anti-nuke: added by an untrusted member").await {
            Ok(()) => format!("added the bot <@{bot}>, which was kicked"),
            Err(e) => format!("added the bot <@{bot}> (kicking it failed: {e})"),
        };
        return strip(ctx, data, guild, actor, what, None).await;
    }
    let Some(kind) = kind_for(&entry.action, perm_change(ctx, guild, entry)) else { return Ok(()) };
    count(ctx, data, guild, actor, kind, entry_name(kind, entry)).await
}

/// Roles to remove and keep, and the Done text when the edit succeeds.
/// Discord checks only the roles being changed, so everything below the bot goes even when a higher role stays.
pub fn plan_strip(roles: &[RoleInfo], bot_top: u16) -> (Vec<u64>, Vec<u64>, String) {
    let (removable, kept) = roles_to_save(roles, bot_top);
    let blocked = roles.iter().any(|r| r.position >= bot_top);
    let done = match (removable.len(), blocked) {
        (0, true) => HIERARCHY.to_string(),
        (0, false) => "No roles to remove.".to_string(),
        (n, true) => format!("Removed {n} roles; could not remove the rest: their top role is above the bot's"),
        (n, false) => format!("Removed {n} roles."),
    };
    (removable, kept, done)
}

/// Strip the roles; returns (done, removed, strip id, name for the Ban button).
async fn strip_roles(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, what: &str) -> Result<(String, Vec<i64>, i64, String), Error> {
    let gid = guild.get() as i64;
    let top = bot_top(ctx, guild).await?;
    let m = match guild.member(ctx, user).await {
        Ok(m) => m,
        Err(e) if is_absent(&e) => {
            let name = user.to_user(ctx).await.map(|u| u.name).unwrap_or_else(|_| user.to_string());
            let id = store::record_strip(&data.db, gid, user.get() as i64, &[], what).await?;
            return Ok(("They are no longer in the server.".into(), vec![], id, name));
        }
        Err(e) => return Err(e.into()),
    };
    let infos: Vec<RoleInfo> = {
        let g = ctx.cache.guild(guild).ok_or("guild not in cache")?;
        m.roles.iter().filter_map(|r| g.roles.get(r)).map(|r| RoleInfo { id: r.id.get(), position: r.position, managed: r.managed }).collect()
    };
    let (removable, kept, done) = plan_strip(&infos, top);
    let removed: Vec<i64> = removable.iter().map(|r| *r as i64).collect();
    // Stored before Discord removes them, so they can always be given back.
    let id = store::record_strip(&data.db, gid, user.get() as i64, &removed, what).await?;
    let stuck = !kept.is_empty();
    let (done, removed) = if removed.is_empty() {
        (done, removed)
    } else {
        let audit = truncate(&format!("Anti-nuke: {what}"), 512);
        let edit = EditMember::new().roles(kept.into_iter().map(RoleId::new)).audit_log_reason(&audit);
        match guild.edit_member(ctx, user, edit).await {
            Ok(_) => (done, removed),
            Err(e) => {
                tracing::warn!("anti-nuke strip of {user} in {guild} failed: {e}");
                (format!("Could not remove roles: {e}"), vec![])
            }
        }
    };
    // A bot keeps its managed integration role (and its permissions): it is kicked, as the bot-add guard does.
    let done = if m.user.bot && (stuck || removed.is_empty()) {
        match guild.kick_with_reason(ctx, user, "Anti-nuke: bot whose roles could not be removed").await {
            Ok(()) => format!("{done} The bot was kicked."),
            Err(e) => format!("{done} Kicking the bot failed: {e}"),
        }
    } else {
        done
    };
    Ok((done, removed, id, m.user.name))
}

/// Remove every role below the bot's top role, store them, and post the alert pinging the owner.
/// The alert is posted even when stripping fails; without a stored strip it has no buttons.
pub async fn strip(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, what: String, names: Option<(&str, Vec<String>)>) -> Result<(), Error> {
    let (done, removed, rows) = match strip_roles(ctx, data, guild, user, &what).await {
        Ok((done, removed, id, name)) => (done, removed, buttons(id, &name)),
        Err(e) => {
            tracing::warn!("anti-nuke strip of {user} in {guild} failed: {e}");
            (format!("Could not remove roles: {e}"), vec![], vec![])
        }
    };
    let owner = ctx.cache.guild(guild).map(|g| g.owner_id).ok_or("guild not in cache")?;
    let names = names.as_ref().map(|(l, n)| (*l, n.as_slice()));
    let msg = CreateMessage::new()
        .content(format!("<@{owner}>"))
        .embed(alert_embed(user, &what, &done, names, &removed))
        .components(rows)
        .allowed_mentions(CreateAllowedMentions::new().users([owner]));
    send_log_message(&ctx.http, &data.db, guild, LogCategory::ModActions, msg).await;
    Ok(())
}

/// Give roles back (`ban` false) or ban (`ban` true) from an anti-nuke alert.
pub async fn press(ctx: &serenity::Context, data: &Data, guild: GuildId, presser: UserId, strip_id: i64, ban: bool) -> Result<CreateEmbed, Error> {
    if !trusted(ctx, data, guild, presser).await? {
        return Err(UserError(NOT_ALLOWED.into()).into());
    }
    let row = store::get_strip(&data.db, strip_id).await?.filter(|r| r.guild_id == guild.get() as i64).ok_or_else(|| UserError("That alert is out of date.".into()))?;
    let target = UserId::new(row.user_id as u64);
    let user = ("User", format!("<@{target}>"));
    if ban {
        let req = ActionRequest {
            guild_id: guild,
            target,
            kind: CaseKind::Ban,
            actor: Actor::Moderator(presser.get() as i64),
            reason: Some(format!("Anti-nuke: {}", row.reason)),
            duration: None,
            ban_delete_days: 0,
            log_case: true,
        };
        execute(ctx, data, req).await?;
        return Ok(reply::done(Tone::Safety, "🔨 Banned", &[user], None));
    }
    // Claim first so two presses can't both restore.
    if !store::mark_strip_restored(&data.db, strip_id).await? {
        return Err(UserError("Roles were already given back.".into()).into());
    }
    let res: Result<(), Error> = async {
        let m = guild.member(ctx, target).await.map_err(|e| if is_absent(&e) { UserError("They are no longer in the server.".into()).into() } else { Error::from(e) })?;
        let mut roles: Vec<RoleId> = {
            let g = ctx.cache.guild(guild).ok_or("guild not in cache")?;
            // Roles deleted since are skipped.
            m.roles.iter().copied().chain(row.roles.iter().map(|r| RoleId::new(*r as u64)).filter(|r| g.roles.contains_key(r))).collect()
        };
        roles.sort();
        roles.dedup();
        guild.edit_member(ctx, target, EditMember::new().roles(roles).audit_log_reason("Anti-nuke: roles given back")).await.map_err(discord_error)?;
        Ok(())
    }
    .await;
    if let Err(e) = res {
        // Release the claim so the stored roles can still be given back.
        store::unmark_strip_restored(&data.db, strip_id).await?;
        return Err(e);
    }
    Ok(reply::done(Tone::Safety, "↩️ Roles given back", &[user, ("Roles", row.roles.len().to_string())], None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_kind_mapping() {
        let none = None;
        assert_eq!(kind_for(&Action::Member(MemberAction::BanAdd), none), Some(NukeKind::Ban));
        assert_eq!(kind_for(&Action::Member(MemberAction::Kick), none), Some(NukeKind::Kick));
        assert_eq!(kind_for(&Action::Channel(ChannelAction::Delete), none), Some(NukeKind::ChannelDelete));
        assert_eq!(kind_for(&Action::Role(RoleAction::Delete), none), Some(NukeKind::RoleDelete));
        assert_eq!(kind_for(&Action::Webhook(WebhookAction::Create), none), Some(NukeKind::Webhook));
        let gain = Some((Permissions::empty(), Permissions::ADMINISTRATOR));
        let harmless = Some((Permissions::empty(), Permissions::SEND_MESSAGES));
        for a in [Action::Role(RoleAction::Create), Action::Role(RoleAction::Update), Action::Member(MemberAction::RoleUpdate)] {
            assert_eq!(kind_for(&a, gain), Some(NukeKind::DangerousPerm));
            assert_eq!(kind_for(&a, harmless), None);
            assert_eq!(kind_for(&a, none), None);
        }
        // Already held: not newly given.
        assert_eq!(kind_for(&Action::Role(RoleAction::Update), Some((Permissions::BAN_MEMBERS, Permissions::BAN_MEMBERS))), None);
        assert_eq!(kind_for(&Action::Member(MemberAction::BotAdd), gain), None);
        assert_eq!(kind_for(&Action::Channel(ChannelAction::Create), none), None);
    }

    #[test]
    fn only_commands_count() {
        // Every source other than a moderator's own command (deny, raid bulk, AutoMod, gate, alt) never counts.
        for kind in [CaseKind::Kick, CaseKind::Ban, CaseKind::Tempban] {
            assert_eq!(nuke_kind(PunishSource::Automatic, kind), None);
            assert_eq!(nuke_kind(PunishSource::RaidBulk, kind), None);
        }
        assert_eq!(nuke_kind(PunishSource::Command, CaseKind::Kick), Some(NukeKind::Kick));
        assert_eq!(nuke_kind(PunishSource::Command, CaseKind::Ban), Some(NukeKind::Ban));
        assert_eq!(nuke_kind(PunishSource::Command, CaseKind::Tempban), Some(NukeKind::Ban));
        assert_eq!(nuke_kind(PunishSource::Command, CaseKind::Warn), None);
        assert_eq!(nuke_kind(PunishSource::Command, CaseKind::Timeout), None);
    }

    #[test]
    fn strip_plan() {
        let r = |id, position, managed| RoleInfo { id, position, managed };
        // All removable; a managed role below the bot stays but is not a hierarchy failure.
        let (rm, kept, done) = plan_strip(&[r(1, 2, false), r(2, 3, false), r(3, 1, true)], 5);
        assert_eq!((rm, kept, done.as_str()), (vec![1, 2], vec![3], "Removed 2 roles."));
        // Partial: a role below the bot still goes when a higher one stays.
        let (rm, kept, done) = plan_strip(&[r(1, 2, false), r(2, 7, false)], 5);
        assert_eq!((rm, kept, done.as_str()), (vec![1], vec![2], "Removed 1 roles; could not remove the rest: their top role is above the bot's"));
        // None removable.
        let (rm, _, done) = plan_strip(&[r(2, 7, false), r(3, 5, false)], 5);
        assert!(rm.is_empty());
        assert_eq!(done, HIERARCHY);
        // No roles.
        let (rm, _, done) = plan_strip(&[], 5);
        assert!(rm.is_empty());
        assert_eq!(done, "No roles to remove.");
    }

    #[test]
    fn strip_roles_below_bot() {
        let r = |id, position, managed| RoleInfo { id, position, managed };
        let (removable, kept) = roles_to_save(&[r(1, 2, false), r(2, 5, false), r(3, 1, true), r(4, 9, false)], 5);
        assert_eq!(removable, vec![1]);
        assert_eq!(kept, vec![2, 3, 4]);
    }

    #[test]
    fn what_text() {
        assert_eq!(what(NukeKind::ChannelDelete, 3, Duration::from_millis(5_200), 2), "deleted 3 channels in 6 seconds (limit 2)");
        assert_eq!(what(NukeKind::Ban, 2, Duration::ZERO, 2), "banned 2 members in 1 second (limit 2)");
        assert_eq!(what(NukeKind::DangerousPerm, 1, Duration::ZERO, 1), "gave dangerous permissions 1 time (limit 1)");
    }

    #[test]
    fn alert_text() {
        let names = vec!["#general".to_string(), "#rules".to_string()];
        let e = serde_json::to_value(alert_embed(UserId::new(5), "deleted 2 channels in 3 seconds (limit 2)", "Removed 1 roles.", Some(("Deleted", &names)), &[9])).unwrap();
        assert_eq!(e["title"], "💣 Anti-nuke triggered");
        assert_eq!(e["footer"]["text"], FOOTER);
        assert_eq!(e["description"], "<@5> deleted 2 channels in 3 seconds (limit 2)\n**Done** Removed 1 roles.\n**Deleted** #general, #rules\n**Removed roles** <@&9>");
        let e = serde_json::to_value(alert_embed(UserId::new(5), "x", HIERARCHY, None, &[])).unwrap();
        assert_eq!(e["description"], format!("<@5> x\n**Done** {HIERARCHY}\n**Removed roles** None"));
    }
}

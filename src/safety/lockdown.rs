//! Lockdown: only staff can talk in the configured channels; `end` restores every overwrite exactly.
use crate::invoker::Invoker;
use crate::logging::{send_log, LogCategory};
use crate::moderation::cases::Actor;
use crate::moderation::commands::log_footer;
use crate::moderation::perms::{member_facts, require_manage_server, require_member};
use crate::reply::{self, Tone};
use crate::safety::logic::{lockdown_overwrites, restore_plan};
use crate::safety::store::{self, SavedOverwrite};
use crate::{settings, Context, Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ChannelId, CreateEmbed, CreateMessage, GuildChannel, GuildId, LightMethod, PermissionOverwriteType, Permissions, Request,
    RoleId, Route, TargetId, UserId,
};
use std::sync::Mutex;

const MAX_CHANNELS: usize = 25;
// Discord channel types (raw JSON `type`).
const VOICE: u64 = 2;
const STAGE: u64 = 13;
const FORUM: u64 = 15;
const MEDIA: u64 = 16;

pub struct LockdownResult {
    pub done: Vec<ChannelId>,
    pub skipped: Vec<ChannelId>,
}

/// "<#1>, <#2>, <#3> +2"
pub fn channel_summary(ids: &[ChannelId]) -> String {
    if ids.is_empty() {
        return "none".into();
    }
    let shown: Vec<String> = ids.iter().take(3).map(|c| format!("<#{c}>")).collect();
    match ids.len().saturating_sub(3) {
        0 => shown.join(", "),
        n => format!("{} +{n}", shown.join(", ")),
    }
}

pub fn precheck(active: bool, channels: &[i64]) -> Result<(), UserError> {
    if active {
        return Err(UserError("A lockdown is already active.".into()));
    }
    if channels.is_empty() {
        return Err(UserError("No lockdown channels are set. Add them in Config → Lockdown.".into()));
    }
    Ok(())
}

/// The `message` slash option: omitted = the saved default, `none` (or blank) = post nothing.
pub fn message_arg(arg: Option<String>, default: String) -> Option<String> {
    match arg {
        None => Some(default),
        Some(m) if m.trim().is_empty() || m.trim().eq_ignore_ascii_case("none") => None,
        Some(m) => Some(m.trim().to_string()),
    }
}

// Guilds with a start/end running: a second start would save the already-locked overwrites over the originals.
static BUSY: Mutex<Vec<GuildId>> = Mutex::new(Vec::new());

struct Busy(GuildId);

impl Busy {
    fn take(g: GuildId) -> Result<Self, UserError> {
        let mut b = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        if b.contains(&g) {
            return Err(UserError("A lockdown change is already running.".into()));
        }
        b.push(g);
        Ok(Self(g))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock().unwrap_or_else(|e| e.into_inner()).retain(|g| *g != self.0);
    }
}

fn kind(o: &SavedOverwrite) -> PermissionOverwriteType {
    if o.is_member {
        PermissionOverwriteType::Member(UserId::new(o.target_id as u64))
    } else {
        PermissionOverwriteType::Role(RoleId::new(o.target_id as u64))
    }
}

/// Overwrites and channel type from raw channel JSON. serenity's typed `Permissions` drops bits it
/// doesn't know (newer than 1<<50), which would make the restore inexact.
pub fn parse_channel(v: &serde_json::Value) -> (Vec<SavedOverwrite>, u64) {
    let ows = v["permission_overwrites"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let num = |k: &str| -> Option<i64> { o[k].as_str()?.parse::<u64>().ok().map(|n| n as i64) };
            Some(SavedOverwrite { target_id: num("id")?, is_member: o["type"].as_u64()? == 1, allow: num("allow")?, deny: num("deny")? })
        })
        .collect();
    (ows, v["type"].as_u64().unwrap_or(0))
}

/// Current overwrites and channel type; Ok(None) when the channel is gone.
/// Read from Discord, not the cache: `end` right after `start` must see the overwrites `start` added.
async fn fetch(ctx: &serenity::Context, ch: ChannelId) -> Result<Option<(Vec<SavedOverwrite>, u64)>, Error> {
    match ctx.http.fire::<serde_json::Value>(Request::new(Route::Channel { channel_id: ch }, LightMethod::Get)).await {
        Ok(v) => Ok(Some(parse_channel(&v))),
        // Unknown Channel (10003): deleted.
        Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r))) if r.error.code == 10003 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Sets every overwrite in `set` that differs from `current`, then deletes `del`.
/// Returns how many calls succeeded and the first error.
async fn apply(
    ctx: &serenity::Context,
    ch: ChannelId,
    current: &[SavedOverwrite],
    set: &[SavedOverwrite],
    del: &[SavedOverwrite],
) -> (usize, Option<serenity::Error>) {
    let mut n = 0;
    for o in set.iter().filter(|o| !current.contains(o)) {
        let body = serde_json::json!({"allow": (o.allow as u64).to_string(), "deny": (o.deny as u64).to_string(), "type": u8::from(o.is_member)});
        if let Err(e) = ctx.http.create_permission(ch, TargetId::new(o.target_id as u64), &body, None).await {
            return (n, Some(e));
        }
        n += 1;
    }
    for o in del {
        if let Err(e) = ch.delete_permission(ctx, kind(o)).await {
            return (n, Some(e));
        }
        n += 1;
    }
    (n, None)
}

fn notice(title: &str, text: Option<&str>) -> CreateMessage {
    let mut e = CreateEmbed::new().colour(Tone::Channels.colour()).title(title);
    if let Some(t) = text {
        e = e.description(t);
    }
    CreateMessage::new().embed(e)
}

fn actor_field(actor: Actor) -> String {
    match actor {
        Actor::Moderator(id) => format!("<@{id}>"),
        _ => "System".into(),
    }
}

pub async fn start(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: GuildId,
    actor: Actor,
    message: Option<String>,
    reason: Option<String>,
) -> Result<LockdownResult, Error> {
    let _busy = Busy::take(guild_id)?;
    let gid = guild_id.get() as i64;
    let s = store::get_settings(&data.db, gid).await?;
    precheck(store::lockdown_active(&data.db, gid).await?, &s.lockdown_channels)?;
    let mods: Vec<u64> = settings::get_or_create(&data.db, gid).await?.mod_roles.iter().map(|r| *r as u64).collect();
    let bot = ctx.cache.current_user().id.get();
    let mut r = LockdownResult { done: vec![], skipped: vec![] };
    for &ch in &s.lockdown_channels {
        let id = ChannelId::new(ch as u64);
        let (current, kind) = match fetch(ctx, id).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                r.skipped.push(id);
                continue;
            }
            Err(e) => {
                tracing::warn!("reading {id} for lockdown failed: {e}");
                r.skipped.push(id);
                continue;
            }
        };
        // Saved before any edit, so `end` can always put the channel back.
        store::lockdown_save_channel(&data.db, gid, ch, &current, false).await?;
        let target = lockdown_overwrites(&current, guild_id.get(), &mods, bot, matches!(kind, VOICE | STAGE));
        if let (applied, Some(e)) = apply(ctx, id, &current, &target, &[]).await {
            tracing::warn!("locking {id} failed: {e}");
            // Nothing changed: forget it. Partly changed: keep it so `end` restores it.
            if applied == 0 {
                store::lockdown_clear_channel(&data.db, gid, ch).await?;
            }
            r.skipped.push(id);
            continue;
        }
        // Forum and media channels can't take a plain message.
        if let Some(m) = message.as_ref().filter(|_| !matches!(kind, FORUM | MEDIA)) {
            match id.send_message(ctx, notice("🔒 Channel locked", Some(m))).await {
                Ok(_) => store::lockdown_save_channel(&data.db, gid, ch, &current, true).await?,
                Err(e) => tracing::warn!("posting the lock message in {id} failed: {e}"),
            }
        }
        r.done.push(id);
    }
    if r.done.is_empty() {
        let mut msg = format!("Couldn't lock any channel. {}", channel_summary(&r.skipped));
        // A channel that failed part-way keeps its saved state; `end` puts it back.
        if store::lockdown_active(&data.db, gid).await? {
            msg.push_str(" Some were changed part-way: run /lockdown end to undo them.");
        }
        return Err(UserError(msg).into());
    }
    let mut log = CreateEmbed::new()
        .title("Lockdown · no case")
        .field("Channels", r.done.len().to_string(), true)
        .field("Moderator", actor_field(actor), true);
    if !r.skipped.is_empty() {
        log = log.field("Couldn't lock", channel_summary(&r.skipped), true);
    }
    let why = reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given");
    log = log.field("Reason", why.chars().take(1024).collect::<String>(), false);
    send_log(&ctx.http, &data.db, guild_id, LogCategory::ModActions, log).await;
    Ok(r)
}

pub async fn end(ctx: &serenity::Context, data: &Data, guild_id: GuildId, actor: Actor) -> Result<LockdownResult, Error> {
    let _busy = Busy::take(guild_id)?;
    let gid = guild_id.get() as i64;
    let rows = store::lockdown_load(&data.db, gid).await?;
    if rows.is_empty() {
        return Err(UserError("No lockdown is active.".into()).into());
    }
    let mut r = LockdownResult { done: vec![], skipped: vec![] };
    for (ch, posted, saved) in rows {
        let id = ChannelId::new(ch as u64);
        // A deleted channel has nothing left to restore. On any failure the saved state stays, so a later `end` can retry.
        let current = match fetch(ctx, id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("reading {id} for unlock failed: {e}");
                r.skipped.push(id);
                continue;
            }
        };
        if let Some((current, _)) = current {
            let (set, del) = restore_plan(&saved, &current);
            if let (_, Some(e)) = apply(ctx, id, &current, &set, &del).await {
                tracing::warn!("unlocking {id} failed: {e}");
                r.skipped.push(id);
                continue;
            }
            if posted {
                if let Err(e) = id.send_message(ctx, notice("🔓 Channel unlocked", None)).await {
                    tracing::warn!("posting the unlock message in {id} failed: {e}");
                }
            }
        }
        store::lockdown_clear_channel(&data.db, gid, ch).await?;
        r.done.push(id);
    }
    let mut log = CreateEmbed::new()
        .title("Lockdown ended · no case")
        .field("Channels", r.done.len().to_string(), true)
        .field("Moderator", actor_field(actor), true);
    if !r.skipped.is_empty() {
        log = log.field("Couldn't unlock", channel_summary(&r.skipped), true);
    }
    send_log(&ctx.http, &data.db, guild_id, LogCategory::ModActions, log).await;
    Ok(r)
}

/// Manage Channels as a Discord permission (owner and Administrators pass); a mod role alone is not enough.
async fn require_manage_channels(inv: &Invoker<'_>) -> Result<(), Error> {
    let m = require_member(inv, inv.user.id).await?;
    let f = member_facts(inv.ctx, inv.guild_id, &m).await?;
    if f.is_owner || f.permissions.contains(Permissions::ADMINISTRATOR) || f.permissions.contains(Permissions::MANAGE_CHANNELS) {
        return Ok(());
    }
    Err(UserError("You need Manage Channels to use lockdown.".into()).into())
}

fn actor_of(inv: &Invoker<'_>) -> Actor {
    Actor::Moderator(inv.user.id.get() as i64)
}

pub async fn start_core(inv: &Invoker<'_>, message: Option<String>, reason: Option<String>) -> Result<CreateEmbed, Error> {
    require_manage_channels(inv).await?;
    let r = start(inv.ctx, inv.data, inv.guild_id, actor_of(inv), message, reason.clone()).await?;
    let why = reason.filter(|r| !r.is_empty()).unwrap_or_else(|| "No reason given".into());
    let mut lines = vec![("Channels", channel_summary(&r.done)), ("Reason", why)];
    if !r.skipped.is_empty() {
        lines.push(("Couldn't lock", channel_summary(&r.skipped)));
    }
    Ok(reply::done(Tone::Channels, "🔒 Lockdown started", &lines, log_footer(inv).await.as_deref()))
}

pub async fn end_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_channels(inv).await?;
    let r = end(inv.ctx, inv.data, inv.guild_id, actor_of(inv)).await?;
    let mut lines = vec![("Channels", channel_summary(&r.done))];
    if !r.skipped.is_empty() {
        lines.push(("Couldn't unlock", channel_summary(&r.skipped)));
    }
    Ok(reply::done(Tone::Channels, "🔓 Lockdown ended", &lines, log_footer(inv).await.as_deref()))
}

pub async fn set_channel_core(inv: &Invoker<'_>, channel: ChannelId, add: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    let id = channel.get() as i64;
    if add && !s.lockdown_channels.contains(&id) {
        if s.lockdown_channels.len() >= MAX_CHANNELS {
            return Err(UserError("At most 25 lockdown channels.".into()).into());
        }
        s.lockdown_channels.push(id);
    } else if !add {
        s.lockdown_channels.retain(|c| *c != id);
    }
    store::save_settings(&inv.data.db, &s).await?;
    let title = if add { "🔒 Lockdown channel added" } else { "🔒 Lockdown channel removed" };
    let lines = [("Channel", format!("<#{channel}>")), ("Lockdown channels", s.lockdown_channels.len().to_string())];
    Ok(reply::done(Tone::Config, title, &lines, None))
}

/// Replace the whole lockdown channel list (panel multi-select).
pub async fn set_channels_core(inv: &Invoker<'_>, channels: &[ChannelId]) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    if channels.len() > MAX_CHANNELS {
        return Err(UserError("At most 25 lockdown channels.".into()).into());
    }
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.lockdown_channels = channels.iter().map(|c| c.get() as i64).collect();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(reply::done(Tone::Config, "🔒 Lockdown channels saved", &[("Lockdown channels", channel_summary(channels))], None))
}

pub async fn list_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    let chans: Vec<String> = s.lockdown_channels.iter().map(|c| format!("<#{c}>")).collect();
    let intro = chans.is_empty().then_some("No lockdown channels set.");
    Ok(reply::list(Tone::Config, "🔒 Lockdown", intro, &[("Channels", chans), ("Message", vec![s.lockdown_message])]))
}

pub async fn set_message_core(inv: &Invoker<'_>, text: String) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 1000 {
        return Err(UserError("Message must be 1 to 1000 characters.".into()).into());
    }
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.lockdown_message = text.into();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(reply::done(Tone::Config, "🔒 Lockdown message saved", &[("Text", s.lockdown_message.clone())], None))
}

/// Lock channels so only staff can talk.
#[poise::command(slash_command, guild_only, subcommands("cmd_start", "cmd_end"), default_member_permissions = "MANAGE_CHANNELS")]
pub async fn lockdown(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Lock every lockdown channel.
#[poise::command(slash_command, guild_only, rename = "start")]
async fn cmd_start(
    ctx: Context<'_>,
    #[description = "Posted in each channel (default: the saved message; none = post nothing)"]
    #[max_length = 1000]
    message: Option<String>,
    #[max_length = 500] reason: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    let default = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?.lockdown_message;
    reply::send(ctx, start_core(&inv, message_arg(message, default), reason).await?).await
}

/// Put every locked channel back exactly as it was.
#[poise::command(slash_command, guild_only, rename = "end")]
async fn cmd_end(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, end_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Lockdown settings.
#[poise::command(slash_command, guild_only, rename = "lockdown", subcommands("cfg_add", "cfg_remove", "cfg_list", "cfg_message"))]
pub async fn lockdown_config(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a channel to lockdown.
#[poise::command(slash_command, guild_only, rename = "add")]
async fn cfg_add(ctx: Context<'_>, #[channel_types("Text", "News", "Voice", "Stage", "Forum")] channel: GuildChannel) -> Result<(), Error> {
    reply::send(ctx, set_channel_core(&Invoker::from_poise(ctx)?, channel.id, true).await?).await
}

/// Remove a channel from lockdown.
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn cfg_remove(ctx: Context<'_>, channel: GuildChannel) -> Result<(), Error> {
    reply::send(ctx, set_channel_core(&Invoker::from_poise(ctx)?, channel.id, false).await?).await
}

/// Show lockdown channels and message.
#[poise::command(slash_command, guild_only, rename = "list")]
async fn cfg_list(ctx: Context<'_>) -> Result<(), Error> {
    reply::send(ctx, list_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Default message posted in each locked channel.
#[poise::command(slash_command, guild_only, rename = "message")]
async fn cfg_message(ctx: Context<'_>, #[max_length = 1000] text: String) -> Result<(), Error> {
    reply::send(ctx, set_message_core(&Invoker::from_poise(ctx)?, text).await?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary() {
        let ids: Vec<ChannelId> = (1..=5).map(ChannelId::new).collect();
        assert_eq!(channel_summary(&[]), "none");
        assert_eq!(channel_summary(&ids[..3]), "<#1>, <#2>, <#3>");
        assert_eq!(channel_summary(&ids), "<#1>, <#2>, <#3> +2");
    }

    #[test]
    fn prechecks() {
        assert_eq!(precheck(true, &[1]).unwrap_err().0, "A lockdown is already active.");
        assert_eq!(precheck(false, &[]).unwrap_err().0, "No lockdown channels are set. Add them in Config → Lockdown.");
        assert!(precheck(false, &[1]).is_ok());
    }

    #[test]
    fn parses_raw_overwrites() {
        let hi = (1u64 << 51) | 2048;
        let v = serde_json::json!({"type": 2, "permission_overwrites": [
            {"id": "7", "type": 0, "allow": hi.to_string(), "deny": (1u64 << 52).to_string()},
            {"id": "9", "type": 1, "allow": "0", "deny": "2048"},
        ]});
        let (ows, kind) = parse_channel(&v);
        assert_eq!(kind, 2);
        assert_eq!(
            ows,
            vec![
                SavedOverwrite { target_id: 7, is_member: false, allow: hi as i64, deny: 1 << 52 },
                SavedOverwrite { target_id: 9, is_member: true, allow: 0, deny: 2048 },
            ]
        );
    }

    #[test]
    fn message_option() {
        let d = || "default".to_string();
        assert_eq!(message_arg(None, d()), Some("default".into()));
        assert_eq!(message_arg(Some("None".into()), d()), None);
        assert_eq!(message_arg(Some(" hi ".into()), d()), Some("hi".into()));
    }
}

use crate::moderation::actions::{discord_error, record_or_log};
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::commands::{action_reply, detail_lines};
use crate::reply::{self, Tone};
use crate::invoker::Invoker;
use crate::moderation::perms::{authorize, member_facts, require_member};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{ChannelId, CreateEmbed, EditMember, GuildChannel, Permissions, User, UserId};

/// Voice moderation.
#[poise::command(slash_command, guild_only, subcommands("voice_disconnect", "voice_mute", "voice_unmute", "voice_move"))]
pub async fn voice(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Disconnect a member from voice.
#[poise::command(slash_command, guild_only, rename = "disconnect")]
pub async fn voice_disconnect(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, user, CaseKind::VoiceDisconnect, None, reason).await
}

/// Server-mute a member in voice.
#[poise::command(slash_command, guild_only, rename = "mute")]
pub async fn voice_mute(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, user, CaseKind::VoiceMute, None, reason).await
}

/// Remove a member's server voice mute.
#[poise::command(slash_command, guild_only, rename = "unmute")]
pub async fn voice_unmute(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, user, CaseKind::VoiceUnmute, None, reason).await
}

/// Move a member to another voice channel.
#[poise::command(slash_command, guild_only, rename = "move")]
pub async fn voice_move(
    ctx: Context<'_>,
    user: User,
    #[channel_types("Voice", "Stage")] channel: GuildChannel,
    reason: Option<String>,
) -> Result<(), Error> {
    run(ctx, user, CaseKind::VoiceMove, Some(channel.id), reason).await
}

/// Permission needed by both the moderator and the bot.
fn required_permission(kind: CaseKind) -> Permissions {
    match kind {
        CaseKind::VoiceMute | CaseKind::VoiceUnmute => Permissions::MUTE_MEMBERS,
        _ => Permissions::MOVE_MEMBERS,
    }
}

fn case_reason(dest: Option<ChannelId>, reason: Option<String>) -> Option<String> {
    match (dest, reason) {
        (Some(d), Some(r)) => Some(format!("Moved to <#{d}>: {r}")),
        (Some(d), None) => Some(format!("Moved to <#{d}>")),
        (None, r) => r,
    }
}

async fn run(ctx: Context<'_>, user: User, kind: CaseKind, dest: Option<ChannelId>, reason: Option<String>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    reply::send(ctx, voice_action(&inv, user.id, kind, dest, reason).await?).await
}

/// Disconnect, mute, unmute or move (`dest` required for move). Authorises like the slash command.
pub async fn voice_action(inv: &Invoker<'_>, user: UserId, kind: CaseKind, dest: Option<ChannelId>, reason: Option<String>) -> Result<CreateEmbed, Error> {
    if kind == CaseKind::VoiceMove && dest.is_none() {
        return Err(UserError("Choose a voice channel to move them to.".into()).into());
    }
    if !matches!(kind, CaseKind::VoiceDisconnect | CaseKind::VoiceMute | CaseKind::VoiceUnmute | CaseKind::VoiceMove) {
        return Err(UserError(format!("{} is not a voice action.", kind.label())).into());
    }
    let need = required_permission(kind);
    authorize(inv, need, Some(user)).await?;
    require_member(inv, user).await?;
    let guild_id = inv.guild_id;
    let current = inv
        .ctx
        .cache
        .guild(guild_id)
        .and_then(|g| g.voice_states.get(&user).and_then(|v| v.channel_id))
        .ok_or_else(|| UserError("That user is not in a voice channel.".into()))?;
    if dest == Some(current) {
        return Err(UserError("That user is already in that channel.".into()).into());
    }
    let bot_id = inv.ctx.cache.current_user().id;
    let bot_m = require_member(inv, bot_id).await?;
    let bot = member_facts(inv.ctx, guild_id, &bot_m).await?;
    if !bot.permissions.contains(need) {
        return Err(UserError("I don't have permission to do that.".into()).into());
    }
    let audit = format!("Voice action by {}", inv.user.name);
    let edit = match kind {
        CaseKind::VoiceDisconnect => EditMember::new().disconnect_member(),
        CaseKind::VoiceMute => EditMember::new().mute(true),
        CaseKind::VoiceUnmute => EditMember::new().mute(false),
        _ => EditMember::new().voice_channel(dest.expect("move has a destination")),
    };
    guild_id.edit_member(inv.ctx, user, edit.audit_log_reason(&audit)).await.map_err(discord_error)?;
    let case = record_or_log(
        inv.ctx,
        inv.data,
        None,
        NewCase {
            guild_id: guild_id.get() as i64,
            kind,
            target_id: user.get() as i64,
            actor: Actor::Moderator(inv.user.id.get() as i64),
            reason: case_reason(dest, reason.clone()),
            duration_secs: None,
            expires_at: None,
            dm_failed: false,
        },
    )
    .await?;
    let mut lines = Vec::new();
    if let Some(d) = dest {
        lines.push(("Channel", format!("<#{d}>")));
    }
    lines.extend(detail_lines(reason.as_deref(), None));
    action_reply(inv, Tone::NamesVoice, kind, user, lines, case.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_by_kind() {
        assert_eq!(required_permission(CaseKind::VoiceMute), Permissions::MUTE_MEMBERS);
        assert_eq!(required_permission(CaseKind::VoiceUnmute), Permissions::MUTE_MEMBERS);
        assert_eq!(required_permission(CaseKind::VoiceMove), Permissions::MOVE_MEMBERS);
        assert_eq!(required_permission(CaseKind::VoiceDisconnect), Permissions::MOVE_MEMBERS);
    }

    #[test]
    fn move_reason_includes_destination() {
        let c = ChannelId::new(5);
        assert_eq!(case_reason(Some(c), Some("x".into())).as_deref(), Some("Moved to <#5>: x"));
        assert_eq!(case_reason(Some(c), None).as_deref(), Some("Moved to <#5>"));
        assert_eq!(case_reason(None, None), None);
    }
}

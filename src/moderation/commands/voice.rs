use crate::moderation::actions::{discord_error, record_or_log};
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::commands::config::ephemeral;
use crate::invoker::Invoker;
use crate::moderation::perms::{authorize, member_facts, require_member};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{ChannelId, EditMember, GuildChannel, Permissions, User};

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
    let need = required_permission(kind);
    let inv = Invoker::from_poise(ctx)?;
    authorize(&inv, need, Some(user.id)).await?;
    require_member(&inv, user.id).await?;
    let guild_id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
    let current = ctx
        .cache()
        .guild(guild_id)
        .and_then(|g| g.voice_states.get(&user.id).and_then(|v| v.channel_id))
        .ok_or_else(|| UserError("That user is not in a voice channel.".into()))?;
    if dest == Some(current) {
        return Err(UserError("That user is already in that channel.".into()).into());
    }
    let bot_id = ctx.cache().current_user().id;
    let bot_m = require_member(&inv, bot_id).await?;
    let bot = member_facts(ctx.serenity_context(), guild_id, &bot_m).await?;
    if !bot.permissions.contains(need) {
        return Err(UserError("I don't have permission to do that.".into()).into());
    }
    let audit = format!("Voice action by {}", ctx.author().name);
    let edit = match kind {
        CaseKind::VoiceDisconnect => EditMember::new().disconnect_member(),
        CaseKind::VoiceMute => EditMember::new().mute(true),
        CaseKind::VoiceUnmute => EditMember::new().mute(false),
        _ => EditMember::new().voice_channel(dest.expect("move has a destination")),
    };
    guild_id.edit_member(ctx, user.id, edit.audit_log_reason(&audit)).await.map_err(discord_error)?;
    let case = record_or_log(
        ctx.serenity_context(),
        ctx.data(),
        None,
        NewCase {
            guild_id: guild_id.get() as i64,
            kind,
            target_id: user.id.get() as i64,
            actor: Actor::Moderator(ctx.author().id.get() as i64),
            reason: case_reason(dest, reason),
            duration_secs: None,
            expires_at: None,
            dm_failed: false,
        },
    )
    .await?;
    match case {
        Some(c) => ephemeral(ctx, format!("Case #{}: {} for <@{}>.", c.case_number, c.kind.label(), user.id)).await,
        None => ephemeral(ctx, format!("{} applied to <@{}>. Logged, no case.", kind.label(), user.id)).await,
    }
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

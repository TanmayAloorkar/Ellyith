use crate::moderation::actions::record_case;
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::commands::config::ephemeral;
use crate::moderation::nickname::check_name;
use crate::moderation::perms::{authorize, require_member};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{EditMember, Permissions, User};

/// Change member nicknames.
#[poise::command(slash_command, guild_only, subcommands("nick_set", "nick_reset"))]
pub async fn nick(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set a member's nickname.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn nick_set(ctx: Context<'_>, user: User, nickname: String) -> Result<(), Error> {
    if nickname.chars().count() > 32 {
        return Err(UserError("Nicknames can be at most 32 characters.".into()).into());
    }
    apply(ctx, user, Some(nickname)).await
}

/// Remove a member's nickname.
#[poise::command(slash_command, guild_only, rename = "reset")]
pub async fn nick_reset(ctx: Context<'_>, user: User) -> Result<(), Error> {
    apply(ctx, user, None).await
}

async fn apply(ctx: Context<'_>, user: User, nick: Option<String>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let s = authorize(ctx, Permissions::MANAGE_NICKNAMES, Some(user.id)).await?;
    require_member(ctx, user.id).await?;
    if let Some(v) = nick.as_deref().filter(|_| s.nick_filter_enabled).and_then(|n| check_name(n, &s.filtered_words, &s.hoist_chars)) {
        return Err(UserError(format!("That nickname {}; the filter would reset it.", v.describe())).into());
    }
    let guild_id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
    let reason = format!("Nickname by {}", ctx.author().name);
    guild_id
        .edit_member(ctx, user.id, EditMember::new().nickname(nick.as_deref().unwrap_or("")).audit_log_reason(&reason))
        .await?;
    let (kind, note) = match &nick {
        Some(n) => (CaseKind::NickSet, Some(format!("Set to: {n}"))),
        None => (CaseKind::NickReset, None),
    };
    let case = record_case(
        ctx.serenity_context(),
        ctx.data(),
        None,
        NewCase {
            guild_id: guild_id.get() as i64,
            kind,
            target_id: user.id.get() as i64,
            actor: Actor::Moderator(ctx.author().id.get() as i64),
            reason: note,
            duration_secs: None,
            expires_at: None,
            dm_failed: false,
        },
    )
    .await?;
    ephemeral(ctx, format!("Case #{}: {} for <@{}>.", case.case_number, case.kind.label(), user.id)).await
}

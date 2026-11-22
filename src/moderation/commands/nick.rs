use crate::invoker::Invoker;
use crate::moderation::actions::record_or_log;
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::commands::{action_reply, detail_lines};
use crate::moderation::nickname::check_name;
use crate::moderation::perms::{authorize, require_member};
use crate::reply::{self, Tone};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateEmbed, EditMember, Permissions, User, UserId};

/// Change member nicknames.
#[poise::command(slash_command, guild_only, subcommands("nick_set", "nick_reset"))]
pub async fn nick(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set a member's nickname.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn nick_set(ctx: Context<'_>, user: User, nickname: String, reason: Option<String>) -> Result<(), Error> {
    run(ctx, user, Some(nickname), reason).await
}

/// Remove a member's nickname.
#[poise::command(slash_command, guild_only, rename = "reset")]
pub async fn nick_reset(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, user, None, reason).await
}

async fn run(ctx: Context<'_>, user: User, nick: Option<String>, reason: Option<String>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    reply::send(ctx, set_nick(&inv, user.id, nick, reason).await?).await
}

/// `None` resets the nickname. Authorises like the slash command.
pub async fn set_nick(inv: &Invoker<'_>, user: UserId, nick: Option<String>, reason: Option<String>) -> Result<CreateEmbed, Error> {
    if nick.as_deref().is_some_and(|n| n.chars().count() > 32) {
        return Err(UserError("Nicknames can be at most 32 characters.".into()).into());
    }
    let s = authorize(inv, Permissions::MANAGE_NICKNAMES, Some(user)).await?;
    require_member(inv, user).await?;
    if let Some(v) = nick.as_deref().filter(|_| s.nick_filter_enabled).and_then(|n| check_name(n, &s.filtered_words, &s.hoist_chars)) {
        return Err(UserError(format!("That nickname {}; the filter would reset it.", v.describe())).into());
    }
    let audit = match reason.as_deref().filter(|r| !r.is_empty()) {
        Some(r) => format!("Nickname by {}: {r}", inv.user.name),
        None => format!("Nickname by {}", inv.user.name),
    };
    inv.guild_id
        .edit_member(inv.ctx, user, EditMember::new().nickname(nick.as_deref().unwrap_or("")).audit_log_reason(&audit))
        .await?;
    let kind = if nick.is_some() { CaseKind::NickSet } else { CaseKind::NickReset };
    let note = match (&nick, reason.as_deref().filter(|r| !r.is_empty())) {
        (Some(n), Some(r)) => Some(format!("Set to: {n}. {r}")),
        (Some(n), None) => Some(format!("Set to: {n}")),
        (None, r) => r.map(str::to_string),
    };
    let _ = record_or_log(
        inv.ctx,
        inv.data,
        None,
        NewCase {
            guild_id: inv.guild_id.get() as i64,
            kind,
            target_id: user.get() as i64,
            actor: Actor::Moderator(inv.user.id.get() as i64),
            reason: note,
            duration_secs: None,
            expires_at: None,
            dm_failed: false,
        },
    )
    .await?;
    let mut lines = Vec::new();
    if let Some(n) = &nick {
        lines.push(("Nickname", n.clone()));
    }
    lines.extend(detail_lines(reason.as_deref(), None));
    action_reply(inv, Tone::NamesVoice, kind, user, lines, None).await
}

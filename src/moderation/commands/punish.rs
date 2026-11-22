use crate::duration::{parse_duration, MAX_TIMEOUT_SECS};
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::commands::config::ephemeral;
use crate::invoker::Invoker;
use crate::moderation::perms::{authorize, require_member};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{Permissions, User};

struct Spec {
    required: Permissions,
    kind: CaseKind,
    needs_member: bool,
}

async fn run(
    ctx: Context<'_>,
    spec: Spec,
    user: &User,
    reason: Option<String>,
    duration: Option<chrono::Duration>,
    ban_delete_days: u8,
) -> Result<(), Error> {
    // Defer first: the DM and Discord calls can exceed the 3 s response window.
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    authorize(&inv, spec.required, Some(user.id)).await?;
    if spec.needs_member {
        require_member(&inv, user.id).await?;
    }
    let guild_id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
    let case = execute(
        ctx.serenity_context(),
        ctx.data(),
        ActionRequest {
            guild_id,
            target: user.id,
            kind: spec.kind,
            actor: Actor::Moderator(ctx.author().id.get() as i64),
            reason,
            duration,
            ban_delete_days,
        },
    )
    .await?;
    match case {
        Some(c) => {
            let note = if c.dm_failed { " (DM could not be delivered)" } else { "" };
            ephemeral(ctx, format!("Case #{}: {} applied to <@{}>.{note}", c.case_number, c.kind.label(), user.id)).await
        }
        None => ephemeral(ctx, format!("{} applied to <@{}>. Logged, no case.", spec.kind.label(), user.id)).await,
    }
}

/// Warn a member.
#[poise::command(slash_command, guild_only)]
pub async fn warn(ctx: Context<'_>, user: User, reason: String) -> Result<(), Error> {
    let spec = Spec { required: Permissions::MODERATE_MEMBERS, kind: CaseKind::Warn, needs_member: false };
    run(ctx, spec, &user, Some(reason), None, 0).await
}

/// Time a member out.
#[poise::command(slash_command, guild_only)]
pub async fn timeout(ctx: Context<'_>, user: User, duration: String, reason: Option<String>) -> Result<(), Error> {
    let d = parse_duration(&duration)?;
    if d.num_seconds() > MAX_TIMEOUT_SECS {
        return Err(UserError("Timeout can be at most 28 days.".into()).into());
    }
    let spec = Spec { required: Permissions::MODERATE_MEMBERS, kind: CaseKind::Timeout, needs_member: true };
    run(ctx, spec, &user, reason, Some(d), 0).await
}

/// Remove a member's timeout.
#[poise::command(slash_command, guild_only)]
pub async fn untimeout(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    let spec = Spec { required: Permissions::MODERATE_MEMBERS, kind: CaseKind::Untimeout, needs_member: true };
    run(ctx, spec, &user, reason, None, 0).await
}

/// Kick a member.
#[poise::command(slash_command, guild_only)]
pub async fn kick(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    let spec = Spec { required: Permissions::KICK_MEMBERS, kind: CaseKind::Kick, needs_member: true };
    run(ctx, spec, &user, reason, None, 0).await
}

/// Ban a user, even if they are not in the server.
#[poise::command(slash_command, guild_only)]
pub async fn ban(
    ctx: Context<'_>,
    user: User,
    reason: Option<String>,
    #[description = "Days of messages to delete (0-7)"] delete_days: Option<u8>,
) -> Result<(), Error> {
    let days = delete_days.unwrap_or(0);
    if days > 7 {
        return Err(UserError("delete_days must be between 0 and 7.".into()).into());
    }
    let spec = Spec { required: Permissions::BAN_MEMBERS, kind: CaseKind::Ban, needs_member: false };
    run(ctx, spec, &user, reason, None, days).await
}

/// Ban a user for a limited time, then unban automatically.
#[poise::command(slash_command, guild_only)]
pub async fn tempban(
    ctx: Context<'_>,
    user: User,
    duration: String,
    reason: Option<String>,
    #[description = "Days of messages to delete (0-7)"] delete_days: Option<u8>,
) -> Result<(), Error> {
    let d = parse_duration(&duration)?;
    // Reject up front what `Utc::now() + d` cannot represent.
    if chrono::Utc::now().checked_add_signed(d).is_none() {
        return Err(UserError("That duration is too long.".into()).into());
    }
    let days = delete_days.unwrap_or(0);
    if days > 7 {
        return Err(UserError("delete_days must be between 0 and 7.".into()).into());
    }
    let spec = Spec { required: Permissions::BAN_MEMBERS, kind: CaseKind::Tempban, needs_member: false };
    run(ctx, spec, &user, reason, Some(d), days).await
}

/// Unban a user (paste their ID).
#[poise::command(slash_command, guild_only)]
pub async fn unban(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    let spec = Spec { required: Permissions::BAN_MEMBERS, kind: CaseKind::Unban, needs_member: false };
    run(ctx, spec, &user, reason, None, 0).await
}

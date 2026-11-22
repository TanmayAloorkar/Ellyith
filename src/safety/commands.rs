//! `/quarantine …` and `/config quarantine …`.
use crate::invoker::Invoker;
use crate::moderation::perms::require_manage_server;
use crate::reply::{self, Tone};
use crate::safety::quarantine::{bot_top, create_role, list_core, quarantine_core, release_core, setup_sync, setup_sync_counts, sync_lines, validate_role};
use crate::safety::store;
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{ChannelId, CreateEmbed, GuildChannel, Role, RoleId, User};

pub async fn set_role_core(inv: &Invoker<'_>, role: RoleId) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let top = bot_top(inv.ctx, inv.guild_id).await?;
    {
        let g = inv.ctx.cache.guild(inv.guild_id).ok_or("guild not in cache")?;
        let r = g.roles.get(&role).ok_or_else(|| UserError("That role no longer exists.".into()))?;
        let everyone = g.roles.get(&RoleId::new(inv.guild_id.get())).map(|e| e.permissions).unwrap_or_default();
        validate_role(r, everyone, top)?;
    }
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.quarantine_role_id = Some(role.get() as i64);
    store::save_settings(&inv.data.db, &s).await?;
    let (updated, failed) = setup_sync_counts(inv).await?;
    let mut lines = vec![("Role", format!("<@&{}>", role.get()))];
    lines.extend(sync_lines(updated, failed));
    Ok(reply::done(Tone::Config, "🚧 Quarantine role saved", &lines, None))
}

pub async fn set_review_channel_core(inv: &Invoker<'_>, channel: ChannelId) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.review_channel_id = Some(channel.get() as i64);
    store::save_settings(&inv.data.db, &s).await?;
    let mut lines = vec![("Channel", format!("<#{}>", channel.get()))];
    // Without a role there is nothing to sync yet.
    if s.quarantine_role_id.is_some() {
        let (updated, failed) = setup_sync_counts(inv).await?;
        lines.extend(sync_lines(updated, failed));
    }
    Ok(reply::done(Tone::Config, "🚧 Review channel saved", &lines, None))
}

pub async fn set_deny_core(inv: &Invoker<'_>, ban: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?;
    s.deny_action = if ban { "ban" } else { "kick" }.into();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(reply::done(Tone::Config, "🚧 Deny action saved", &[("Deny", s.deny_action.clone())], None))
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum DenyChoice {
    #[name = "kick"]
    Kick,
    #[name = "ban"]
    Ban,
}

/// Quarantine members and review them.
#[poise::command(slash_command, guild_only, subcommands("add", "release", "list"), default_member_permissions = "MODERATE_MEMBERS")]
pub async fn quarantine(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Put a member in quarantine.
#[poise::command(slash_command, guild_only)]
async fn add(ctx: Context<'_>, user: User, #[max_length = 500] reason: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, quarantine_core(&Invoker::from_poise(ctx)?, user.id, reason).await?).await
}

/// Release a member from quarantine.
#[poise::command(slash_command, guild_only)]
async fn release(ctx: Context<'_>, user: User) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, release_core(&Invoker::from_poise(ctx)?, user.id).await?).await
}

/// List quarantined members.
#[poise::command(slash_command, guild_only)]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, list_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Quarantine settings.
#[poise::command(slash_command, guild_only, rename = "quarantine", subcommands("cfg_role", "cfg_create_role", "cfg_channel", "cfg_deny", "cfg_sync"))]
pub async fn quarantine_config(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Use an existing role as the quarantine role.
#[poise::command(slash_command, guild_only, rename = "role")]
async fn cfg_role(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, set_role_core(&Invoker::from_poise(ctx)?, role.id).await?).await
}

/// Create the quarantine role.
#[poise::command(slash_command, guild_only, rename = "create-role")]
async fn cfg_create_role(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, create_role(&Invoker::from_poise(ctx)?).await?).await
}

/// Where review cards for quarantined members go.
#[poise::command(slash_command, guild_only, rename = "channel")]
async fn cfg_channel(ctx: Context<'_>, #[channel_types("Text")] channel: GuildChannel) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, set_review_channel_core(&Invoker::from_poise(ctx)?, channel.id).await?).await
}

/// What Deny does to a member.
#[poise::command(slash_command, guild_only, rename = "deny")]
async fn cfg_deny(ctx: Context<'_>, action: DenyChoice) -> Result<(), Error> {
    reply::send(ctx, set_deny_core(&Invoker::from_poise(ctx)?, matches!(action, DenyChoice::Ban)).await?).await
}

/// Re-apply quarantine permissions on every channel.
#[poise::command(slash_command, guild_only, rename = "sync")]
async fn cfg_sync(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, setup_sync(&Invoker::from_poise(ctx)?).await?).await
}

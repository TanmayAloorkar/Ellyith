use crate::logging::{self, LogCategory};
use crate::settings::{self, GuildSettings};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateAllowedMentions, GuildChannel, Role};

/// Manage Server only (owner and Administrators pass too).
async fn manage_guild(ctx: Context<'_>) -> Result<bool, Error> {
    let Some(guild_id) = ctx.guild_id() else { return Ok(false) };
    let Some(m) = ctx.author_member().await else { return Ok(false) };
    let ok = ctx
        .serenity_context()
        .cache
        .guild(guild_id)
        .is_some_and(|g| g.owner_id == m.user.id || g.member_permissions(&m).manage_guild());
    Ok(ok)
}

/// Configure the bot.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("modroles", "protected", "logs"),
    default_member_permissions = "MANAGE_GUILD",
    check = "manage_guild"
)]
pub async fn config(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Roles that may use moderation commands.
#[poise::command(slash_command, guild_only, subcommands("modroles_add", "modroles_remove", "modroles_list"))]
pub async fn modroles(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Roles exempt from moderation and automatic actions.
#[poise::command(slash_command, guild_only, subcommands("protected_add", "protected_remove", "protected_list"))]
pub async fn protected(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a moderator role.
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn modroles_add(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, role, |s| &mut s.mod_roles, true, "moderator").await
}

/// Remove a moderator role.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn modroles_remove(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, role, |s| &mut s.mod_roles, false, "moderator").await
}

/// List moderator roles.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn modroles_list(ctx: Context<'_>) -> Result<(), Error> {
    list(ctx, |s| &s.mod_roles, "Moderator roles").await
}

/// Add a protected role.
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn protected_add(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, role, |s| &mut s.protected_roles, true, "protected").await
}

/// Remove a protected role.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn protected_remove(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, role, |s| &mut s.protected_roles, false, "protected").await
}

/// List protected roles.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn protected_list(ctx: Context<'_>) -> Result<(), Error> {
    list(ctx, |s| &s.protected_roles, "Protected roles").await
}

fn guild_id(ctx: Context<'_>) -> Result<i64, Error> {
    let id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
    Ok(id.get() as i64)
}

async fn edit(
    ctx: Context<'_>,
    role: Role,
    field: fn(&mut GuildSettings) -> &mut Vec<i64>,
    add: bool,
    kind: &str,
) -> Result<(), Error> {
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    let id = role.id.get() as i64;
    let v = field(&mut s);
    let msg = if add {
        if v.contains(&id) {
            format!("{} is already a {kind} role.", role.name)
        } else {
            v.push(id);
            format!("Added {} as a {kind} role.", role.name)
        }
    } else if let Some(i) = v.iter().position(|r| *r == id) {
        v.remove(i);
        format!("Removed {} from {kind} roles.", role.name)
    } else {
        format!("{} is not a {kind} role.", role.name)
    };
    settings::save(db, &s).await?;
    ctx.send(poise::CreateReply::default().content(msg).ephemeral(true)).await?;
    Ok(())
}

async fn list(ctx: Context<'_>, field: fn(&GuildSettings) -> &Vec<i64>, title: &str) -> Result<(), Error> {
    let s = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?;
    let ids = field(&s);
    let body = if ids.is_empty() {
        "none".to_string()
    } else {
        ids.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ")
    };
    // Empty allowed_mentions: show roles without pinging them.
    ctx.send(
        poise::CreateReply::default()
            .content(format!("{title}: {body}"))
            .ephemeral(true)
            .allowed_mentions(CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

/// Per-category log channels.
#[poise::command(slash_command, guild_only, subcommands("logs_set", "logs_disable", "logs_list"))]
pub async fn logs(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Send a log category to a channel.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn logs_set(ctx: Context<'_>, category: LogCategory, channel: GuildChannel) -> Result<(), Error> {
    logging::set_channel(&ctx.data().db, guild_id(ctx)?, category, channel.id.get() as i64).await?;
    ephemeral(ctx, format!("{} logs go to <#{}>.", category.as_str(), channel.id)).await
}

/// Stop logging a category.
#[poise::command(slash_command, guild_only, rename = "disable")]
pub async fn logs_disable(ctx: Context<'_>, category: LogCategory) -> Result<(), Error> {
    let msg = if logging::clear(&ctx.data().db, guild_id(ctx)?, category).await? {
        format!("{} logs disabled.", category.as_str())
    } else {
        format!("{} logs were not enabled.", category.as_str())
    };
    ephemeral(ctx, msg).await
}

/// Show log channels.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn logs_list(ctx: Context<'_>) -> Result<(), Error> {
    let rows = logging::list(&ctx.data().db, guild_id(ctx)?).await?;
    let body = if rows.is_empty() {
        "none".to_string()
    } else {
        rows.iter().map(|(c, ch)| format!("{}: <#{ch}>", c.as_str())).collect::<Vec<_>>().join("\n")
    };
    ephemeral(ctx, format!("Log channels:\n{body}")).await
}

async fn ephemeral(ctx: Context<'_>, msg: String) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(msg).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

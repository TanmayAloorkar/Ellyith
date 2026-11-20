use crate::logging::{case_embed, send_log, CaseEvent, LogCategory};
use crate::moderation::cases::{self, Actor, Case};
use crate::moderation::commands::config::ephemeral;
use crate::moderation::notes;
use crate::moderation::perms::authorize;
use crate::settings;
use crate::text::truncate;
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, GuildId, Permissions, User};

const MAX_LINES: usize = 25;
const MAX_LINE: usize = 150;

fn guild(ctx: Context<'_>) -> Result<GuildId, Error> {
    ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()).into())
}

/// Embed descriptions allow 4096 chars, so 26 lines of at most 150 fit; embeds never ping.
async fn embed_reply(ctx: Context<'_>, embed: CreateEmbed) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

fn lines_embed(title: &str, lines: Vec<String>) -> CreateEmbed {
    let body = lines.iter().map(|l| truncate(l, MAX_LINE)).collect::<Vec<_>>().join("\n");
    CreateEmbed::new().title(title).description(body)
}

fn reason_of(c: &Case) -> &str {
    c.reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given")
}

/// Inspect and manage cases.
#[poise::command(slash_command, guild_only, subcommands("case_view", "case_reason", "case_remove"))]
pub async fn case(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show a case.
#[poise::command(slash_command, guild_only, rename = "view")]
pub async fn case_view(ctx: Context<'_>, #[min = 1] number: i32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?;
    let Some(c) = cases::get(&ctx.data().db, g.get() as i64, number).await? else {
        return ephemeral(ctx, "Case not found.".into()).await;
    };
    let mut e = case_embed(&c, CaseEvent::Created);
    if let Some(by) = c.removed_by {
        e = e.footer(CreateEmbedFooter::new(format!("Removed by <@{by}>")));
    }
    embed_reply(ctx, e).await
}

/// Change a case's reason.
#[poise::command(slash_command, guild_only, rename = "reason")]
pub async fn case_reason(ctx: Context<'_>, #[min = 1] number: i32, #[max_length = 1000] text: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?;
    let Some(c) = cases::set_reason(&ctx.data().db, g.get() as i64, number, Some(text)).await? else {
        return ephemeral(ctx, "Case not found.".into()).await;
    };
    send_log(&ctx.serenity_context().http, &ctx.data().db, g, LogCategory::ModActions, case_embed(&c, CaseEvent::ReasonEdited)).await;
    ephemeral(ctx, format!("Case #{number} reason updated.")).await
}

/// Remove a case (kept on record, no longer counted).
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn case_remove(ctx: Context<'_>, #[min = 1] number: i32) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?;
    let by = ctx.author().id.get() as i64;
    let Some(c) = cases::soft_remove(&ctx.data().db, g.get() as i64, number, by).await? else {
        return ephemeral(ctx, "Case not found or already removed.".into()).await;
    };
    send_log(&ctx.serenity_context().http, &ctx.data().db, g, LogCategory::ModActions, case_embed(&c, CaseEvent::Removed { by })).await;
    ephemeral(ctx, format!("Case #{number} removed.")).await
}

/// Show a user's case history.
#[poise::command(slash_command, guild_only)]
pub async fn history(ctx: Context<'_>, user: User) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?.get() as i64;
    let db = &ctx.data().db;
    let target = user.id.get() as i64;
    let s = settings::get_or_create(db, g).await?;
    let all = cases::history(db, g, target, true).await?;
    let active = cases::active_warn_count(db, g, target, s.warn_decay_enabled.then_some(s.warn_decay_days)).await?;
    let mut lines = vec![format!("<@{}>: {} case(s), {active} active warning(s)", user.id, all.len())];
    for c in all.iter().take(MAX_LINES) {
        let removed = c.removed_by.map(|b| format!(" [Removed by <@{b}>]")).unwrap_or_default();
        let who = match c.actor {
            Actor::Moderator(id) => format!("<@{id}>"),
            Actor::AutoPunishment => "Auto punishment".into(),
            Actor::System => "System".into(),
        };
        // Reason is cut first so the removed marker survives the per-line truncation.
        let reason = truncate(reason_of(c), 60);
        lines.push(format!("`#{}` {} by {who} <t:{}:d>: {reason}{removed}", c.case_number, c.kind.label(), c.created_at.timestamp()));
    }
    embed_reply(ctx, lines_embed("Case history", lines)).await
}

/// Private moderator notes.
#[poise::command(slash_command, guild_only, subcommands("note_add", "note_list", "note_remove"))]
pub async fn note(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a note about a user.
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn note_add(ctx: Context<'_>, user: User, #[max_length = 1000] text: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?.get() as i64;
    let n = notes::add(&ctx.data().db, g, user.id.get() as i64, ctx.author().id.get() as i64, &text).await?;
    ephemeral(ctx, format!("Note #{} added for <@{}>.", n.id, user.id)).await
}

/// List notes about a user.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn note_list(ctx: Context<'_>, user: User) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?.get() as i64;
    let all = notes::list(&ctx.data().db, g, user.id.get() as i64).await?;
    if all.is_empty() {
        return ephemeral(ctx, "No notes.".into()).await;
    }
    let mut lines = vec![format!("<@{}>: {} note(s)", user.id, all.len())];
    lines.extend(all.iter().take(MAX_LINES).map(|n| format!("`#{}` <@{}> <t:{}:d>: {}", n.id, n.author_id, n.created_at.timestamp(), truncate(&n.content, 100))));
    embed_reply(ctx, lines_embed("Notes", lines)).await
}

/// Delete a note by its id.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn note_remove(ctx: Context<'_>, #[min = 1] id: i64) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    authorize(ctx, Permissions::MODERATE_MEMBERS, None).await?;
    let g = guild(ctx)?.get() as i64;
    let msg = if notes::remove(&ctx.data().db, g, id).await? { format!("Note #{id} deleted.") } else { "Note not found.".into() };
    ephemeral(ctx, msg).await
}

/// Show your own warnings and other cases.
#[poise::command(slash_command, guild_only)]
pub async fn mywarnings(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let g = guild(ctx)?.get() as i64;
    let me = ctx.author().id.get() as i64;
    let db = &ctx.data().db;
    let s = settings::get_or_create(db, g).await?;
    let mine = cases::history(db, g, me, false).await?;
    let active = cases::active_warn_count(db, g, me, s.warn_decay_enabled.then_some(s.warn_decay_days)).await?;
    let mut lines = vec![format!("Active warnings: {active}")];
    for c in mine.iter().take(MAX_LINES) {
        let by = match (s.dm_show_moderator, c.actor) {
            (true, Actor::Moderator(id)) => format!(" by <@{id}>"),
            _ => String::new(),
        };
        lines.push(format!("`#{}` {}{by} <t:{}:d>: {}", c.case_number, c.kind.label(), c.created_at.timestamp(), reason_of(c)));
    }
    embed_reply(ctx, lines_embed("Your record", lines)).await
}

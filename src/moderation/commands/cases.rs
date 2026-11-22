use crate::invoker::Invoker;
use crate::logging::{case_embed, send_log, CaseEvent, LogCategory};
use crate::moderation::cases::{self, Actor, Case};
use crate::moderation::commands::{log_footer, username};
use crate::moderation::notes;
use crate::moderation::perms::authorize;
use crate::reply::{self, done, list, Tone};
use crate::settings;
use crate::text::truncate;
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, Permissions, User, UserId};

const MAX_LINES: usize = 25;
const FIELD_MAX: usize = 1024;

fn reason_of(c: &Case) -> &str {
    c.reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given")
}

/// Whole lines only, so a field cut at 1024 never ends mid-line.
fn fit_lines(lines: Vec<String>) -> Vec<String> {
    let mut used = 0;
    lines
        .into_iter()
        .take_while(|l| {
            used += l.chars().count() + 1;
            used <= FIELD_MAX + 1
        })
        .collect()
}

fn not_found(what: &str) -> Error {
    UserError(format!("{what} not found.")).into()
}

/// Defer, build the invoker, run the core, send the embed.
macro_rules! respond {
    ($ctx:expr, $inv:ident => $core:expr) => {{
        $ctx.defer_ephemeral().await?;
        let $inv = Invoker::from_poise($ctx)?;
        reply::send($ctx, $core.await?).await
    }};
}

/// Inspect and manage cases.
#[poise::command(slash_command, guild_only, subcommands("case_view", "case_reason", "case_remove"))]
pub async fn case(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Show a case.
#[poise::command(slash_command, guild_only, rename = "view")]
pub async fn case_view(ctx: Context<'_>, #[min = 1] number: i32) -> Result<(), Error> {
    respond!(ctx, inv => case_view_core(&inv, number))
}

pub async fn case_view_core(inv: &Invoker<'_>, number: i32) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let c = cases::get(&inv.data.db, inv.guild_id.get() as i64, number).await?.ok_or_else(|| not_found("Case"))?;
    let mut e = case_embed(&c, CaseEvent::Created);
    if let Some(by) = c.removed_by {
        e = e.footer(CreateEmbedFooter::new(format!("Removed by <@{by}>")));
    }
    Ok(e)
}

/// Change a case's reason.
#[poise::command(slash_command, guild_only, rename = "reason")]
pub async fn case_reason(ctx: Context<'_>, #[min = 1] number: i32, #[max_length = 1000] text: String) -> Result<(), Error> {
    respond!(ctx, inv => case_reason_core(&inv, number, text))
}

pub async fn case_reason_core(inv: &Invoker<'_>, number: i32, text: String) -> Result<CreateEmbed, Error> {
    let g = inv.guild_id;
    let db = &inv.data.db;
    let existing = cases::get(db, g.get() as i64, number).await?.ok_or_else(|| not_found("Case"))?;
    authorize(inv, Permissions::MODERATE_MEMBERS, Some(UserId::new(existing.target_id as u64))).await?;
    let c = cases::set_reason(db, g.get() as i64, number, Some(text.clone())).await?.ok_or_else(|| not_found("Case"))?;
    send_log(&inv.ctx.http, db, g, LogCategory::ModActions, case_embed(&c, CaseEvent::ReasonEdited)).await;
    Ok(done(Tone::Records, &format!("✏️ Case #{number} reason updated"), &[("Reason", text)], log_footer(inv).await?.as_deref()))
}

/// Remove a case (kept on record, no longer counted).
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn case_remove(ctx: Context<'_>, #[min = 1] number: i32) -> Result<(), Error> {
    respond!(ctx, inv => case_remove_core(&inv, number))
}

pub async fn case_remove_core(inv: &Invoker<'_>, number: i32) -> Result<CreateEmbed, Error> {
    let g = inv.guild_id;
    let db = &inv.data.db;
    let gone = || UserError("Case not found or already removed.".into());
    let existing = cases::get(db, g.get() as i64, number).await?.ok_or_else(gone)?;
    authorize(inv, Permissions::MODERATE_MEMBERS, Some(UserId::new(existing.target_id as u64))).await?;
    let by = inv.user.id.get() as i64;
    let c = cases::soft_remove(db, g.get() as i64, number, by).await?.ok_or_else(gone)?;
    send_log(&inv.ctx.http, db, g, LogCategory::ModActions, case_embed(&c, CaseEvent::Removed { by })).await;
    Ok(done(Tone::Records, &format!("🗑️ Case #{number} removed"), &[("Note", "Kept on record, no longer counted.".into())], log_footer(inv).await?.as_deref()))
}

/// Show a user's case history.
#[poise::command(slash_command, guild_only)]
pub async fn history(ctx: Context<'_>, user: User) -> Result<(), Error> {
    respond!(ctx, inv => history_core(&inv, user.id))
}

pub async fn history_core(inv: &Invoker<'_>, user: UserId) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let g = inv.guild_id.get() as i64;
    let db = &inv.data.db;
    let target = user.get() as i64;
    let s = settings::get_or_create(db, g).await?;
    let all = cases::history(db, g, target, true).await?;
    let active = cases::active_warn_count(db, g, target, s.warn_decay_enabled.then_some(s.warn_decay_days)).await?;
    let decay = if s.warn_decay_enabled { format!("Decay on ({} days)", s.warn_decay_days) } else { "Decay off".into() };
    let lines = all
        .iter()
        .take(MAX_LINES)
        .map(|c| {
            let line = format!("**#{} {}** {} · {}", c.case_number, c.kind.label(), truncate(reason_of(c), 40), c.created_at.format("%-d %b"));
            match c.removed_by {
                Some(b) => format!("~~{line}~~ · removed by <@{b}>"),
                None => line,
            }
        })
        .collect();
    let n = all.len();
    let title = format!("📜 {} — {n} case{}", username(inv, user).await, if n == 1 { "" } else { "s" });
    Ok(list(Tone::Records, &title, Some(&format!("Active warnings: {active} · {decay}")), &[("Recent", fit_lines(lines))]))
}

/// Private moderator notes.
#[poise::command(slash_command, guild_only, subcommands("note_add", "note_list", "note_view", "note_remove"))]
pub async fn note(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Add a note about a user.
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn note_add(ctx: Context<'_>, user: User, #[max_length = 1000] text: String) -> Result<(), Error> {
    respond!(ctx, inv => note_add_core(&inv, user.id, text))
}

pub async fn note_add_core(inv: &Invoker<'_>, user: UserId, text: String) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let n = notes::add(&inv.data.db, inv.guild_id.get() as i64, user.get() as i64, inv.user.id.get() as i64, &text).await?;
    let title = format!("📝 Note added for {}", username(inv, user).await);
    Ok(done(Tone::Records, &title, &[("Note", format!("#{}", n.id)), ("Text", text)], None))
}

/// List notes about a user.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn note_list(ctx: Context<'_>, user: User) -> Result<(), Error> {
    respond!(ctx, inv => note_list_core(&inv, user.id))
}

pub async fn note_list_core(inv: &Invoker<'_>, user: UserId) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let all = notes::list(&inv.data.db, inv.guild_id.get() as i64, user.get() as i64).await?;
    let lines = all
        .iter()
        .take(MAX_LINES)
        .map(|n| format!("**#{}** {} · <@{}> · {}", n.id, truncate(&n.content, 60), n.author_id, n.created_at.format("%-d %b")))
        .collect();
    let count = all.len();
    let title = format!("📝 {} — {count} note{}", username(inv, user).await, if count == 1 { "" } else { "s" });
    Ok(list(Tone::Records, &title, None, &[("Notes", fit_lines(lines))]))
}

/// Show a full note by its id.
#[poise::command(slash_command, guild_only, rename = "view")]
pub async fn note_view(ctx: Context<'_>, #[min = 1] id: i64) -> Result<(), Error> {
    respond!(ctx, inv => note_view_core(&inv, id))
}

pub async fn note_view_core(inv: &Invoker<'_>, id: i64) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    let n = notes::get(&inv.data.db, inv.guild_id.get() as i64, id).await?.ok_or_else(|| not_found("Note"))?;
    let about = format!("<@{}>", n.target_id);
    let by = format!("<@{}>", n.author_id);
    let date = format!("<t:{}:f>", n.created_at.timestamp());
    Ok(list(
        Tone::Records,
        &format!("📝 Note #{}", n.id),
        Some(&n.content),
        &[("About", vec![about]), ("Author", vec![by]), ("Date", vec![date])],
    ))
}

/// Delete a note by its id.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn note_remove(ctx: Context<'_>, #[min = 1] id: i64) -> Result<(), Error> {
    respond!(ctx, inv => note_remove_core(&inv, id))
}

pub async fn note_remove_core(inv: &Invoker<'_>, id: i64) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MODERATE_MEMBERS, None).await?;
    if !notes::remove(&inv.data.db, inv.guild_id.get() as i64, id).await? {
        return Err(not_found("Note"));
    }
    Ok(done(Tone::Records, &format!("🗑️ Note #{id} deleted"), &[], None))
}

/// Show your own warnings and other cases.
#[poise::command(slash_command, guild_only)]
pub async fn mywarnings(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    let g = inv.guild_id.get() as i64;
    let me = inv.user.id.get() as i64;
    let db = &inv.data.db;
    let s = settings::get_or_create(db, g).await?;
    let mine = cases::history(db, g, me, false).await?;
    let active = cases::active_warn_count(db, g, me, s.warn_decay_enabled.then_some(s.warn_decay_days)).await?;
    let lines = mine
        .iter()
        .take(MAX_LINES)
        .map(|c| {
            let by = match (s.dm_show_moderator, c.actor) {
                (true, Actor::Moderator(id)) => format!(" by <@{id}>"),
                _ => String::new(),
            };
            format!("**#{} {}**{by} {} · {}", c.case_number, c.kind.label(), truncate(reason_of(c), 40), c.created_at.format("%-d %b"))
        })
        .collect();
    reply::send(ctx, list(Tone::Records, "📜 Your record", Some(&format!("Active warnings: {active}")), &[("Recent", fit_lines(lines))])).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_lines_keeps_whole_lines_under_the_field_limit() {
        let lines: Vec<String> = (0..10).map(|_| "x".repeat(300)).collect();
        let kept = fit_lines(lines);
        assert_eq!(kept.len(), 3);
        assert!(kept.join("\n").chars().count() <= FIELD_MAX);
        assert!(fit_lines(vec![]).is_empty());
    }
}

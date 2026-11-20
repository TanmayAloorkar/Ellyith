use crate::logging::{self, LogCategory};
use crate::moderation::cases::CaseKind;
use crate::duration::{format_duration, parse_duration};
use crate::moderation::ladder::{self, LadderAction, LadderRule};
use crate::moderation::nickname::validate_filter_config;
use crate::moderation::templates;
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
    subcommands("modroles", "protected", "logs", "dm", "ladder", "decay", "nickwords", "nickfilter"),
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

pub(crate) async fn ephemeral(ctx: Context<'_>, msg: String) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().content(msg).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

/// Kinds that get a DM (mirrors `templates::dm_kinds()`).
#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum DmKind {
    Warn,
    Timeout,
    Kick,
    Ban,
    Tempban,
}

impl From<DmKind> for CaseKind {
    fn from(k: DmKind) -> Self {
        match k {
            DmKind::Warn => CaseKind::Warn,
            DmKind::Timeout => CaseKind::Timeout,
            DmKind::Kick => CaseKind::Kick,
            DmKind::Ban => CaseKind::Ban,
            DmKind::Tempban => CaseKind::Tempban,
        }
    }
}

/// Member DM settings.
#[poise::command(slash_command, guild_only, subcommands("template_set", "template_reset", "template_view", "dm_showmod"))]
pub async fn dm(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set a template. Placeholders: {action} {reason} {duration} {server} {case} {moderator}.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn template_set(ctx: Context<'_>, kind: DmKind, text: String) -> Result<(), Error> {
    if text.chars().count() > 1500 {
        return Err(UserError("Template must be 1500 characters or fewer.".into()).into());
    }
    templates::set(&ctx.data().db, guild_id(ctx)?, kind.into(), &text).await?;
    ephemeral(ctx, format!("{} template saved.", CaseKind::from(kind).label())).await
}

/// Restore the default template.
#[poise::command(slash_command, guild_only, rename = "reset")]
pub async fn template_reset(ctx: Context<'_>, kind: DmKind) -> Result<(), Error> {
    templates::reset(&ctx.data().db, guild_id(ctx)?, kind.into()).await?;
    ephemeral(ctx, format!("{} template reset to default.", CaseKind::from(kind).label())).await
}

/// Show the template in use.
#[poise::command(slash_command, guild_only, rename = "view")]
pub async fn template_view(ctx: Context<'_>, kind: DmKind) -> Result<(), Error> {
    let ck = CaseKind::from(kind);
    let (text, tag) = match templates::get(&ctx.data().db, guild_id(ctx)?, ck).await? {
        Some(t) => (t, "custom"),
        None => (templates::default_template(ck).to_string(), "default"),
    };
    // Defang backticks so the template cannot break out of the code block.
    ephemeral(ctx, format!("{} template ({tag}):\n```\n{}\n```", ck.label(), text.replace("```", "'''"))).await
}

/// Show the moderator's name in DMs.
#[poise::command(slash_command, guild_only, rename = "showmod")]
pub async fn dm_showmod(ctx: Context<'_>, show: bool) -> Result<(), Error> {
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    s.dm_show_moderator = show;
    settings::save(db, &s).await?;
    ephemeral(ctx, format!("Moderator name in DMs: {}.", if show { "on" } else { "off" })).await
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum LadderChoice {
    Timeout,
    Kick,
    Tempban,
    Ban,
}

impl From<LadderChoice> for LadderAction {
    fn from(c: LadderChoice) -> Self {
        match c {
            LadderChoice::Timeout => LadderAction::Timeout,
            LadderChoice::Kick => LadderAction::Kick,
            LadderChoice::Tempban => LadderAction::Tempban,
            LadderChoice::Ban => LadderAction::Ban,
        }
    }
}

/// Automatic punishments for reaching a warning count.
#[poise::command(slash_command, guild_only, subcommands("ladder_set", "ladder_remove", "ladder_list"))]
pub async fn ladder(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set the action taken at an exact warning count.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn ladder_set(ctx: Context<'_>, warns: i32, action: LadderChoice, duration: Option<String>) -> Result<(), Error> {
    let duration_secs = duration.as_deref().map(parse_duration).transpose()?.map(|d| d.num_seconds());
    let rule = LadderRule { warn_count: warns, action: action.into(), duration_secs };
    ladder::validate_rule(&rule)?;
    ladder::set_rule(&ctx.data().db, guild_id(ctx)?, &rule).await?;
    ephemeral(ctx, format!("At {warns} warnings: {}.", describe_rule(&rule))).await
}

/// Remove the rule for a warning count.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn ladder_remove(ctx: Context<'_>, warns: i32) -> Result<(), Error> {
    let msg = if ladder::remove_rule(&ctx.data().db, guild_id(ctx)?, warns).await? {
        format!("Removed the rule for {warns} warnings.")
    } else {
        format!("No rule for {warns} warnings.")
    };
    ephemeral(ctx, msg).await
}

/// Show the ladder.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn ladder_list(ctx: Context<'_>) -> Result<(), Error> {
    let rules = ladder::list_rules(&ctx.data().db, guild_id(ctx)?).await?;
    let body = if rules.is_empty() {
        "none".to_string()
    } else {
        rules.iter().map(|r| format!("{} warnings: {}", r.warn_count, describe_rule(r))).collect::<Vec<_>>().join("
")
    };
    ephemeral(ctx, format!("Ladder:
{body}")).await
}

fn describe_rule(r: &LadderRule) -> String {
    match r.duration_secs {
        Some(s) => format!("{} for {}", r.action.as_str(), format_duration(chrono::Duration::seconds(s))),
        None => r.action.as_str().to_string(),
    }
}

/// Warnings expire after a number of days.
#[poise::command(slash_command, guild_only, subcommands("decay_on", "decay_off"))]
pub async fn decay(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn warning decay on.
#[poise::command(slash_command, guild_only, rename = "on")]
pub async fn decay_on(ctx: Context<'_>, days: i32) -> Result<(), Error> {
    if !(1..=365).contains(&days) {
        return Err(UserError("Days must be between 1 and 365.".into()).into());
    }
    set_decay(ctx, true, Some(days)).await
}

/// Turn warning decay off.
#[poise::command(slash_command, guild_only, rename = "off")]
pub async fn decay_off(ctx: Context<'_>) -> Result<(), Error> {
    set_decay(ctx, false, None).await
}

async fn set_decay(ctx: Context<'_>, on: bool, days: Option<i32>) -> Result<(), Error> {
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    s.warn_decay_enabled = on;
    if let Some(d) = days {
        s.warn_decay_days = d;
    }
    settings::save(db, &s).await?;
    ephemeral(ctx, if on { format!("Warnings now expire after {} days.", s.warn_decay_days) } else { "Warning decay off.".into() }).await
}

/// Words that are not allowed in nicknames.
#[poise::command(slash_command, guild_only, subcommands("nickwords_add", "nickwords_remove", "nickwords_list"))]
pub async fn nickwords(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Forbid a word in nicknames (case-insensitive substring).
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn nickwords_add(ctx: Context<'_>, word: String) -> Result<(), Error> {
    let word = word.trim().to_lowercase();
    if word.is_empty() {
        return Err(UserError("Word cannot be empty.".into()).into());
    }
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    if s.filtered_words.contains(&word) {
        return ephemeral(ctx, format!("\"{word}\" is already filtered.")).await;
    }
    s.filtered_words.push(word.clone());
    validate_filter_config(&s.nick_fallback, &s.filtered_words, &s.hoist_chars)?;
    settings::save(db, &s).await?;
    ephemeral(ctx, format!("Filtered \"{word}\".")).await
}

/// Stop filtering a word.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn nickwords_remove(ctx: Context<'_>, word: String) -> Result<(), Error> {
    let word = word.trim().to_lowercase();
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    let msg = if let Some(i) = s.filtered_words.iter().position(|w| *w == word) {
        s.filtered_words.remove(i);
        settings::save(db, &s).await?;
        format!("Removed \"{word}\".")
    } else {
        format!("\"{word}\" was not filtered.")
    };
    ephemeral(ctx, msg).await
}

/// List filtered words.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn nickwords_list(ctx: Context<'_>) -> Result<(), Error> {
    let s = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?;
    let body = if s.filtered_words.is_empty() { "none".to_string() } else { s.filtered_words.join(", ") };
    ephemeral(ctx, format!("Filtered words: {body}")).await
}

/// Nickname filter settings.
#[poise::command(slash_command, guild_only, subcommands("nickfilter_on", "nickfilter_off", "nickfilter_hoist", "nickfilter_fallback"))]
pub async fn nickfilter(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn the nickname filter on.
#[poise::command(slash_command, guild_only, rename = "on")]
pub async fn nickfilter_on(ctx: Context<'_>) -> Result<(), Error> {
    set_nickfilter(ctx, true).await
}

/// Turn the nickname filter off.
#[poise::command(slash_command, guild_only, rename = "off")]
pub async fn nickfilter_off(ctx: Context<'_>) -> Result<(), Error> {
    set_nickfilter(ctx, false).await
}

async fn set_nickfilter(ctx: Context<'_>, on: bool) -> Result<(), Error> {
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    s.nick_filter_enabled = on;
    settings::save(db, &s).await?;
    ephemeral(ctx, format!("Nickname filter: {}.", if on { "on" } else { "off" })).await
}

/// Characters a nickname may not start with.
#[poise::command(slash_command, guild_only, rename = "hoist")]
pub async fn nickfilter_hoist(ctx: Context<'_>, chars: String) -> Result<(), Error> {
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    validate_filter_config(&s.nick_fallback, &s.filtered_words, &chars)?;
    s.hoist_chars = chars;
    settings::save(db, &s).await?;
    ephemeral(ctx, format!("Hoisting characters: {}", s.hoist_chars)).await
}

/// Nickname given to members whose name is reset.
#[poise::command(slash_command, guild_only, rename = "fallback")]
pub async fn nickfilter_fallback(ctx: Context<'_>, name: String) -> Result<(), Error> {
    if name.chars().count() > 32 {
        return Err(UserError("Nicknames can be at most 32 characters.".into()).into());
    }
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    validate_filter_config(&name, &s.filtered_words, &s.hoist_chars)?;
    s.nick_fallback = name;
    settings::save(db, &s).await?;
    ephemeral(ctx, format!("Fallback nickname: {}", s.nick_fallback)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::ChoiceParameter;

    #[test]
    fn dm_kind_matches_dm_kinds() {
        let mapped: Vec<CaseKind> = (0..DmKind::list().len()).map(|i| DmKind::from_index(i).unwrap().into()).collect();
        assert_eq!(mapped, templates::dm_kinds());
    }
}

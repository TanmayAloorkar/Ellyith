use crate::duration::parse_duration;
use crate::invoker::Invoker;
use crate::logging::{self, LogCategory};
use crate::moderation::cases::CaseKind;
use crate::moderation::commands::config_core::{self as core, describe_rule, role_mentions, RoleList};
use crate::moderation::ladder::{self, LadderAction, LadderRule};
use crate::moderation::perms::require_manage_server;
use crate::moderation::templates;
use crate::panel::view;
use crate::reply::{self, Tone};
use crate::settings::{self, GuildSettings};
use crate::safety::commands::quarantine_config as quarantine;
use crate::safety::verify::{self, RoleOp};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{self as serenity, CreateAllowedMentions, CreateMessage, GuildChannel, MessageId, Role, RoleId};

/// Manage Server only (owner and Administrators pass too).
async fn manage_guild(ctx: Context<'_>) -> Result<bool, Error> {
    require_manage_server(&Invoker::from_poise(ctx)?).await?;
    Ok(true)
}

/// Configure the bot.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("modroles", "protected", "logs", "dm", "ladder", "decay", "nickwords", "nickfilter", "panel", "quarantine", "verification"),
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
    edit(ctx, RoleList::Mod, role.id, true).await
}

/// Remove a moderator role.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn modroles_remove(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, RoleList::Mod, role.id, false).await
}

/// List moderator roles.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn modroles_list(ctx: Context<'_>) -> Result<(), Error> {
    list(ctx, RoleList::Mod).await
}

/// Add a protected role.
#[poise::command(slash_command, guild_only, rename = "add")]
pub async fn protected_add(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, RoleList::Protected, role.id, true).await
}

/// Remove a protected role.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn protected_remove(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    edit(ctx, RoleList::Protected, role.id, false).await
}

/// List protected roles.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn protected_list(ctx: Context<'_>) -> Result<(), Error> {
    list(ctx, RoleList::Protected).await
}

fn guild_id(ctx: Context<'_>) -> Result<i64, Error> {
    let id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
    Ok(id.get() as i64)
}

fn roles_of(s: &GuildSettings, which: RoleList) -> &Vec<i64> {
    match which {
        RoleList::Mod => &s.mod_roles,
        RoleList::Protected => &s.protected_roles,
    }
}

async fn edit(ctx: Context<'_>, which: RoleList, role: RoleId, add: bool) -> Result<(), Error> {
    let inv = Invoker::from_poise(ctx)?;
    let s = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?;
    let mut ids: Vec<RoleId> = roles_of(&s, which).iter().map(|r| RoleId::new(*r as u64)).collect();
    ids.retain(|r| *r != role);
    if add {
        ids.push(role);
    }
    reply::send(ctx, core::set_role_list(&inv, which, ids).await?).await
}

async fn list(ctx: Context<'_>, which: RoleList) -> Result<(), Error> {
    let s = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?;
    let title = match which {
        RoleList::Mod => "🛡️ Moderator roles",
        RoleList::Protected => "🔒 Protected roles",
    };
    reply::send(ctx, reply::list(Tone::Config, title, Some(&role_mentions(roles_of(&s, which))), &[])).await
}

/// Per-category log channels.
#[poise::command(slash_command, guild_only, subcommands("logs_set", "logs_disable", "logs_list"))]
pub async fn logs(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// A log category, or `all` of them.
#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum LogChoice {
    #[name = "all"]
    All,
    #[name = "mod_actions"]
    ModActions,
    #[name = "messages"]
    Messages,
    #[name = "joins_leaves"]
    JoinsLeaves,
    #[name = "member_updates"]
    MemberUpdates,
    #[name = "channels"]
    Channels,
    #[name = "roles"]
    Roles,
    #[name = "voice"]
    Voice,
    #[name = "external_mod"]
    ExternalMod,
}

impl LogChoice {
    /// `None` for `all`; names match `LogCategory::as_str`.
    fn category(self) -> Option<LogCategory> {
        LogCategory::ALL.into_iter().find(|c| c.as_str() == poise::ChoiceParameter::name(&self))
    }
}

/// Send a log category (or all) to a channel.
#[poise::command(slash_command, guild_only, rename = "set")]
pub async fn logs_set(ctx: Context<'_>, category: LogChoice, #[channel_types("Text", "News")] channel: GuildChannel) -> Result<(), Error> {
    reply::send(ctx, core::set_log_channel(&Invoker::from_poise(ctx)?, category.category(), Some(channel.id)).await?).await
}

/// Stop logging a category (or all).
#[poise::command(slash_command, guild_only, rename = "disable")]
pub async fn logs_disable(ctx: Context<'_>, category: LogChoice) -> Result<(), Error> {
    reply::send(ctx, core::set_log_channel(&Invoker::from_poise(ctx)?, category.category(), None).await?).await
}

/// Show log channels.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn logs_list(ctx: Context<'_>) -> Result<(), Error> {
    let rows = logging::list(&ctx.data().db, guild_id(ctx)?).await?;
    let lines = rows.iter().map(|(c, ch)| format!("**{}** <#{ch}>", c.as_str())).collect();
    let intro = if rows.is_empty() { "none" } else { "Where each kind of log goes." };
    reply::send(ctx, reply::list(Tone::Config, "📋 Log channels", Some(intro), &[("Channels", lines)])).await
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
    reply::send(ctx, core::set_dm_template(&Invoker::from_poise(ctx)?, kind.into(), Some(text)).await?).await
}

/// Restore the default template.
#[poise::command(slash_command, guild_only, rename = "reset")]
pub async fn template_reset(ctx: Context<'_>, kind: DmKind) -> Result<(), Error> {
    reply::send(ctx, core::set_dm_template(&Invoker::from_poise(ctx)?, kind.into(), None).await?).await
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
    let body = format!("```\n{}\n```", text.replace("```", "'''"));
    reply::send(ctx, reply::list(Tone::Config, &format!("✉️ {} message ({tag})", ck.label()), Some(&body), &[])).await
}

/// Show the moderator's name in DMs.
#[poise::command(slash_command, guild_only, rename = "showmod")]
pub async fn dm_showmod(ctx: Context<'_>, show: bool) -> Result<(), Error> {
    reply::send(ctx, core::set_dm_showmod(&Invoker::from_poise(ctx)?, show).await?).await
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
    reply::send(ctx, core::ladder_add(&Invoker::from_poise(ctx)?, rule).await?).await
}

/// Remove the rule for a warning count.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn ladder_remove(ctx: Context<'_>, warns: i32) -> Result<(), Error> {
    reply::send(ctx, core::ladder_remove(&Invoker::from_poise(ctx)?, warns).await?).await
}

/// Show the ladder.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn ladder_list(ctx: Context<'_>) -> Result<(), Error> {
    let rules = ladder::list_rules(&ctx.data().db, guild_id(ctx)?).await?;
    let lines = rules.iter().map(|r| format!("**{} warnings** {}", r.warn_count, describe_rule(r))).collect();
    let intro = if rules.is_empty() { "none" } else { "Auto-punish at an exact warning count." };
    reply::send(ctx, reply::list(Tone::Config, "📈 Ladder", Some(intro), &[("Rules", lines)])).await
}

/// Warnings expire after a number of days.
#[poise::command(slash_command, guild_only, subcommands("decay_on", "decay_off"))]
pub async fn decay(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn warning decay on.
#[poise::command(slash_command, guild_only, rename = "on")]
pub async fn decay_on(ctx: Context<'_>, days: i32) -> Result<(), Error> {
    reply::send(ctx, core::set_decay(&Invoker::from_poise(ctx)?, Some(days)).await?).await
}

/// Turn warning decay off.
#[poise::command(slash_command, guild_only, rename = "off")]
pub async fn decay_off(ctx: Context<'_>) -> Result<(), Error> {
    reply::send(ctx, core::set_decay(&Invoker::from_poise(ctx)?, None).await?).await
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
    let mut words = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?.filtered_words;
    if !words.contains(&word) {
        words.push(word);
    }
    reply::send(ctx, core::set_nick_words(&Invoker::from_poise(ctx)?, words).await?).await
}

/// Stop filtering a word.
#[poise::command(slash_command, guild_only, rename = "remove")]
pub async fn nickwords_remove(ctx: Context<'_>, word: String) -> Result<(), Error> {
    let word = word.trim().to_lowercase();
    let mut words = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?.filtered_words;
    words.retain(|w| *w != word);
    reply::send(ctx, core::set_nick_words(&Invoker::from_poise(ctx)?, words).await?).await
}

/// List filtered words.
#[poise::command(slash_command, guild_only, rename = "list")]
pub async fn nickwords_list(ctx: Context<'_>) -> Result<(), Error> {
    let s = settings::get_or_create(&ctx.data().db, guild_id(ctx)?).await?;
    let body = if s.filtered_words.is_empty() { "none".to_string() } else { s.filtered_words.join(", ") };
    reply::send(ctx, reply::list(Tone::Config, "🚫 Filtered words", Some(&body), &[])).await
}

/// Nickname filter settings.
#[poise::command(slash_command, guild_only, subcommands("nickfilter_on", "nickfilter_off", "nickfilter_hoist", "nickfilter_fallback"))]
pub async fn nickfilter(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn the nickname filter on.
#[poise::command(slash_command, guild_only, rename = "on")]
pub async fn nickfilter_on(ctx: Context<'_>) -> Result<(), Error> {
    reply::send(ctx, core::set_nick_filter(&Invoker::from_poise(ctx)?, true).await?).await
}

/// Turn the nickname filter off.
#[poise::command(slash_command, guild_only, rename = "off")]
pub async fn nickfilter_off(ctx: Context<'_>) -> Result<(), Error> {
    reply::send(ctx, core::set_nick_filter(&Invoker::from_poise(ctx)?, false).await?).await
}

/// Characters a nickname may not start with.
#[poise::command(slash_command, guild_only, rename = "hoist")]
pub async fn nickfilter_hoist(ctx: Context<'_>, chars: String) -> Result<(), Error> {
    reply::send(ctx, core::set_hoist(&Invoker::from_poise(ctx)?, chars).await?).await
}

/// Nickname given to members whose name is reset.
#[poise::command(slash_command, guild_only, rename = "fallback")]
pub async fn nickfilter_fallback(ctx: Context<'_>, name: String) -> Result<(), Error> {
    reply::send(ctx, core::set_fallback(&Invoker::from_poise(ctx)?, name).await?).await
}

/// Discord says the message or channel is gone: Unknown Channel (10003) or Unknown Message (10008).
fn is_gone(e: &serenity::Error) -> bool {
    matches!(e, serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) if matches!(r.error.code, 10003 | 10008))
}

/// Post the moderation panel in a channel, replacing the previous one.
#[poise::command(slash_command, guild_only)]
pub async fn panel(ctx: Context<'_>, #[channel_types("Text", "News")] channel: GuildChannel) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let db = &ctx.data().db;
    let mut s = settings::get_or_create(db, guild_id(ctx)?).await?;
    let old = s.panel_channel_id.zip(s.panel_message_id);
    let (embed, rows) = view::home();
    let msg = match channel.id.send_message(ctx, CreateMessage::new().embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new())).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("posting the panel in {} failed: {e}", channel.id);
            return Err(UserError("I can't post in that channel — check my permissions there.".into()).into());
        }
    };
    s.panel_channel_id = Some(channel.id.get() as i64);
    s.panel_message_id = Some(msg.id.get() as i64);
    settings::save(db, &s).await?;
    // Best effort: a stale or undeletable old panel must never block posting a new one.
    if let Some((c, m)) = old {
        if let Err(e) = serenity::ChannelId::new(c as u64).delete_message(ctx, MessageId::new(m as u64)).await {
            if !is_gone(&e) {
                tracing::warn!("deleting the old panel message failed: {e}");
            }
        }
    }
    reply::send(ctx, reply::done(Tone::Config, "⚙️ Panel posted", &[("Channel", format!("<#{}>", channel.id))], None)).await
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum VerifyMethod {
    #[name = "off"]
    Off,
    #[name = "button"]
    Button,
    #[name = "quiz"]
    Quiz,
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum VerifyReview {
    #[name = "staff"]
    Staff,
    #[name = "auto"]
    Auto,
}

/// Member verification settings.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("ver_method", "ver_answers", "ver_channel", "ver_roles", "ver_questions", "ver_description", "ver_post")
)]
pub async fn verification(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Turn verification off, or require a button press or a quiz.
#[poise::command(slash_command, guild_only, rename = "method")]
async fn ver_method(ctx: Context<'_>, method: VerifyMethod) -> Result<(), Error> {
    reply::send(ctx, verify::set_method_core(&Invoker::from_poise(ctx)?, poise::ChoiceParameter::name(&method)).await?).await
}

/// Who checks quiz answers.
#[poise::command(slash_command, guild_only, rename = "answers")]
async fn ver_answers(ctx: Context<'_>, review: VerifyReview) -> Result<(), Error> {
    reply::send(ctx, verify::set_review_core(&Invoker::from_poise(ctx)?, poise::ChoiceParameter::name(&review)).await?).await
}

/// Where the verify message goes.
#[poise::command(slash_command, guild_only, rename = "channel")]
async fn ver_channel(ctx: Context<'_>, #[channel_types("Text")] channel: GuildChannel) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, verify::set_channel_core(&Invoker::from_poise(ctx)?, channel.id).await?).await
}

/// Roles given when a member verifies.
#[poise::command(slash_command, guild_only, rename = "roles", subcommands("ver_roles_add", "ver_roles_remove", "ver_roles_clear"))]
async fn ver_roles(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Give this role on verification.
#[poise::command(slash_command, guild_only, rename = "add")]
async fn ver_roles_add(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    reply::send(ctx, verify::set_roles_core(&Invoker::from_poise(ctx)?, RoleOp::Add(role.id)).await?).await
}

/// Stop giving this role.
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn ver_roles_remove(ctx: Context<'_>, role: Role) -> Result<(), Error> {
    reply::send(ctx, verify::set_roles_core(&Invoker::from_poise(ctx)?, RoleOp::Remove(role.id)).await?).await
}

/// Give no roles on verification.
#[poise::command(slash_command, guild_only, rename = "clear")]
async fn ver_roles_clear(ctx: Context<'_>) -> Result<(), Error> {
    reply::send(ctx, verify::set_roles_core(&Invoker::from_poise(ctx)?, RoleOp::Clear).await?).await
}

/// Quiz questions, up to 5 separated by ; (add | expected answer to auto-grade).
#[poise::command(slash_command, guild_only, rename = "questions")]
async fn ver_questions(ctx: Context<'_>, questions: String) -> Result<(), Error> {
    reply::send(ctx, verify::set_questions_core(&Invoker::from_poise(ctx)?, &questions).await?).await
}

/// Text shown on the verify message.
#[poise::command(slash_command, guild_only, rename = "description")]
async fn ver_description(ctx: Context<'_>, #[max_length = 1000] text: String) -> Result<(), Error> {
    reply::send(ctx, verify::set_description_core(&Invoker::from_poise(ctx)?, text).await?).await
}

/// Post the verify message, replacing the previous one.
#[poise::command(slash_command, guild_only, rename = "post")]
async fn ver_post(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, verify::post_core(&Invoker::from_poise(ctx)?).await?).await
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

    #[test]
    fn log_choice_covers_every_category() {
        let mapped: Vec<Option<LogCategory>> = (0..LogChoice::list().len()).map(|i| LogChoice::from_index(i).unwrap().category()).collect();
        let mut want = vec![None];
        want.extend(LogCategory::ALL.map(Some));
        assert_eq!(mapped, want);
    }
}

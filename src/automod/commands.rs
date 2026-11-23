//! `/automod …`: status, per-rule settings, banned words, allowed sites.
use crate::automod::matchers::compile_regex;
use crate::automod::rules::{parse_word_line, RuleAction, RuleConfig, RuleKind, WordKind};
use crate::automod::store;
use crate::duration::{format_duration, MAX_TIMEOUT_SECS};
use crate::invoker::Invoker;
use crate::moderation::perms::require_manage_server;
use crate::reply::{self, Tone};
use crate::{Context, Error, UserError};
use chrono::Duration;
use poise::serenity_prelude::{CreateEmbed, GuildChannel, Role};

const MAX_ENTRIES_SHOWN: usize = 10;

fn user_err(s: String) -> Error { UserError(s).into() }

fn range(field: &str, v: i32, lo: i32, hi: i32) -> Result<i32, UserError> {
    if (lo..=hi).contains(&v) { Ok(v) } else { Err(UserError(format!("{field} must be between {lo} and {hi}."))) }
}

/// Applies the given fields to a copy of `c`, checking each.
pub fn validate_settings(c: &RuleConfig, threshold: Option<i32>, window: Option<i32>, percent: Option<i32>, extra: Option<i32>) -> Result<RuleConfig, UserError> {
    let mut n = c.clone();
    if let Some(v) = threshold {
        let (lo, hi, name) = match c.kind {
            RuleKind::FastMessages | RuleKind::RepeatedMessages | RuleKind::FileSpam => (2, 100, "Threshold"),
            RuleKind::Zalgo => (1, 20, "Marks (threshold)"),
            _ => (1, 100, "Threshold"),
        };
        n.threshold = range(name, v, lo, hi)?;
    }
    if let Some(v) = window { n.window_secs = range("Window (seconds)", v, 1, 300)?; }
    if let Some(v) = percent { n.percent = range("Percent", v, 1, 100)?; }
    if let Some(v) = extra { n.extra = range("Extra (characters)", v, 1, 4000)?; }
    Ok(n)
}

pub fn validate_timeout(d: Duration) -> Result<i64, UserError> {
    let s = d.num_seconds();
    if (60..=MAX_TIMEOUT_SECS).contains(&s) { Ok(s) } else { Err(UserError("Timeout must be between 1 minute and 28 days.".into())) }
}

/// Lowercase host only: no scheme, `www.`, path or port. Needs a dot.
pub fn normalize_site(s: &str) -> Option<String> {
    let s = s.trim().to_lowercase();
    let s = s.split_once("://").map_or(s.as_str(), |(_, r)| r);
    let host = s.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host).trim_matches('.');
    let ok = host.contains('.') && host.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-');
    ok.then(|| host.to_string())
}

fn action_text(c: &RuleConfig) -> String {
    match c.action {
        RuleAction::Timeout => format!("timeout {}", format_duration(Duration::seconds(c.timeout_secs))),
        a => a.label().to_lowercase(),
    }
}

fn trigger_text(c: &RuleConfig, words: usize, sites: usize) -> Option<String> {
    Some(match c.kind {
        RuleKind::BannedWords => format!("{words} words"),
        RuleKind::FastMessages => format!("{} in {}s", c.threshold, c.window_secs),
        RuleKind::RepeatedMessages => format!("{} similar in {}s", c.threshold, c.window_secs),
        RuleKind::MassMentions => format!("{}+ mentions", c.threshold),
        RuleKind::EmojiSpam => format!("{}+ emojis", c.threshold),
        RuleKind::AllCaps => format!("{}% caps, {}+ letters", c.percent, c.threshold),
        RuleKind::FileSpam => format!("{} files in {}s", c.threshold, c.window_secs),
        RuleKind::Links => format!("{sites} allowed sites"),
        RuleKind::WallsOfText => format!("{} lines or {} characters", c.threshold, c.extra),
        RuleKind::SpoilerSpam => format!("{}+ spoilers", c.threshold),
        RuleKind::Zalgo => format!("{}+ stacked marks, {}% of text", c.threshold, c.percent),
        RuleKind::Invites | RuleKind::ScamLinks => return None,
    })
}

/// "On · <summary> · <action>" or "Off".
pub fn rule_line(c: &RuleConfig, words: usize, sites: usize) -> String {
    if !c.enabled { return "Off".into(); }
    match trigger_text(c, words, sites) {
        Some(t) => format!("On · {t} · {}", action_text(c)),
        None => format!("On · {}", action_text(c)),
    }
}

fn describe(k: RuleKind) -> &'static str {
    match k {
        RuleKind::BannedWords => "Removes messages containing a banned word. Also matches common disguises: l33t letters, extra spaces, repeated letters.",
        RuleKind::FastMessages => "Catches a member sending too many messages too quickly.",
        RuleKind::RepeatedMessages => "Catches the same message posted again and again, even with small changes: capitals, spacing, punctuation, and swaps like n1tro -> nitro.",
        RuleKind::MassMentions => "Catches messages that mention too many people or roles.",
        RuleKind::EmojiSpam => "Catches messages with too many emojis.",
        RuleKind::AllCaps => "Catches messages that are mostly capital letters.",
        RuleKind::FileSpam => "Catches many attachments sent in a short time.",
        RuleKind::Links => "Every link is removed unless its site is on the allowed list.",
        RuleKind::Invites => "Removes Discord server invites.",
        RuleKind::ScamLinks => "Removes links to known scam and phishing domains (list refreshed daily).",
        RuleKind::WallsOfText => "Catches very long messages or messages with too many lines.",
        RuleKind::SpoilerSpam => "Catches messages with too many spoiler tags.",
        RuleKind::Zalgo => "Catches messages with stacked combining marks (glitch text).",
    }
}

async fn guild_rules(inv: &Invoker<'_>) -> Result<Vec<RuleConfig>, Error> {
    Ok(store::get_rules(&inv.data.db, inv.guild_id.get() as i64).await?)
}

async fn save(inv: &Invoker<'_>, c: &RuleConfig) -> Result<(), Error> {
    store::save_rule(&inv.data.db, inv.guild_id.get() as i64, c).await?;
    inv.data.automod.invalidate(inv.guild_id.get());
    Ok(())
}

/// Saved config, or `defaults()` when the rule was never saved.
async fn one_rule(inv: &Invoker<'_>, kind: RuleKind) -> Result<RuleConfig, Error> {
    guild_rules(inv).await?.into_iter().find(|c| c.kind == kind).ok_or_else(|| "rule missing".into())
}

async fn counts(inv: &Invoker<'_>) -> Result<(usize, usize), Error> {
    let g = inv.guild_id.get() as i64;
    Ok((store::list_words(&inv.data.db, g).await?.len(), store::list_sites(&inv.data.db, g).await?.len()))
}

pub async fn status_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let rules = guild_rules(inv).await?;
    let (words, sites) = counts(inv).await?;
    let on = rules.iter().filter(|c| c.enabled).count();
    let mut groups: Vec<(&str, Vec<String>)> = Vec::new();
    for name in ["Words", "Spam", "Links", "Message shape"] {
        let lines = rules.iter().filter(|c| c.kind.group() == name).map(|c| format!("{} **{}** {}", c.kind.emoji(), c.kind.label(), rule_line(c, words, sites))).collect();
        groups.push((name, lines));
    }
    Ok(reply::list(Tone::Config, &format!("🤖 AutoMod · {on} of 13 rules on"), Some("Pick a rule with `/automod rule` to turn it on, tune it, or change what it does."), &groups))
}

pub async fn rule_view_core(inv: &Invoker<'_>, kind: RuleKind) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let c = one_rule(inv, kind).await?;
    let (words, sites) = counts(inv).await?;
    let mut lines = vec![];
    if let Some(t) = trigger_text(&c, words, sites) { lines.push(("Trigger", t)); }
    if kind == RuleKind::RepeatedMessages { lines.push(("How similar", format!("{}%", c.percent))); }
    lines.push(("Action", action_text(&c)));
    let shown = |ids: &[i64], p: &str| if ids.is_empty() { "none".to_string() } else { ids.iter().map(|i| format!("<{p}{i}>")).collect::<Vec<_>>().join(", ") };
    lines.push(("Ignored roles", shown(&c.ignored_roles, "@&")));
    lines.push(("Ignored channels", shown(&c.ignored_channels, "#")));
    let title = format!("{} {} · {}", kind.emoji(), kind.label(), if c.enabled { "On" } else { "Off" });
    Ok(reply::done(Tone::Config, &title, &lines, Some(describe(kind))))
}

pub async fn set_enabled_core(inv: &Invoker<'_>, kind: RuleKind, on: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut c = one_rule(inv, kind).await?;
    c.enabled = on;
    save(inv, &c).await?;
    let (words, sites) = counts(inv).await?;
    let t = format!("{} {} turned {}", kind.emoji(), kind.label(), if on { "on" } else { "off" });
    Ok(reply::done(Tone::Config, &t, &[("Rule", rule_line(&c, words, sites))], None))
}

pub async fn set_action_core(inv: &Invoker<'_>, kind: RuleKind, action: RuleAction, timeout: Option<Duration>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut c = one_rule(inv, kind).await?;
    if action == RuleAction::Timeout {
        let d = timeout.ok_or_else(|| user_err("Give a timeout length, like 10m, for the Timeout action.".into()))?;
        c.timeout_secs = validate_timeout(d)?;
    }
    c.action = action;
    save(inv, &c).await?;
    Ok(reply::done(Tone::Config, &format!("{} {} action saved", kind.emoji(), kind.label()), &[("Action", action_text(&c))], None))
}

pub async fn set_settings_core(inv: &Invoker<'_>, kind: RuleKind, threshold: Option<i32>, window: Option<i32>, percent: Option<i32>, extra: Option<i32>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let c = validate_settings(&one_rule(inv, kind).await?, threshold, window, percent, extra)?;
    save(inv, &c).await?;
    let (words, sites) = counts(inv).await?;
    let now = rule_line(&RuleConfig { enabled: true, ..c }, words, sites);
    Ok(reply::done(Tone::Config, &format!("{} {} settings saved", kind.emoji(), kind.label()), &[("Now", now)], None))
}

pub enum Target { Role(i64), Channel(i64) }

pub async fn ignore_core(inv: &Invoker<'_>, kind: RuleKind, target: Target, add: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut c = one_rule(inv, kind).await?;
    let (list, id, what, shown) = match target {
        Target::Role(i) => (&mut c.ignored_roles, i, "role", format!("<@&{i}>")),
        Target::Channel(i) => (&mut c.ignored_channels, i, "channel", format!("<#{i}>")),
    };
    list.retain(|x| *x != id);
    if add { list.push(id); }
    save(inv, &c).await?;
    let t = format!("{} {} ignored {what}s updated", kind.emoji(), kind.label());
    Ok(reply::done(Tone::Config, &t, &[(if add { "Ignoring" } else { "No longer ignoring" }, shown)], None))
}

fn entries(s: &str) -> Vec<&str> { s.split(';').map(str::trim).filter(|x| !x.is_empty()).collect() }

pub async fn words_add_core(inv: &Invoker<'_>, lines: &str) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let (mut valid, mut invalid) = (vec![], vec![]);
    for l in entries(lines) {
        match parse_word_line(l) {
            Some(w) if w.kind == WordKind::Regex && compile_regex(&w.pattern).is_err() => invalid.push(l.to_string()),
            Some(w) => valid.push(w),
            None => {}
        }
    }
    if valid.is_empty() && invalid.is_empty() { return Err(user_err("Give at least one word. Separate several with ;".into())); }
    let added = store::add_words(&inv.data.db, inv.guild_id.get() as i64, &valid).await?;
    inv.data.automod.invalidate(inv.guild_id.get());
    let mut out = vec![("Added", added.to_string()), ("Already there", (valid.len() - added).to_string())];
    if !invalid.is_empty() { out.push(("Invalid", format!("{} ({})", invalid.len(), invalid.join(", ")))); }
    Ok(reply::done(Tone::Config, "🚫 Banned words updated", &out, None))
}

pub async fn words_remove_core(inv: &Invoker<'_>, lines: &str) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let ws: Vec<_> = entries(lines).into_iter().filter_map(parse_word_line).collect();
    if ws.is_empty() { return Err(user_err("Give at least one word. Separate several with ;".into())); }
    let removed = store::remove_words(&inv.data.db, inv.guild_id.get() as i64, &ws).await?;
    inv.data.automod.invalidate(inv.guild_id.get());
    Ok(reply::done(Tone::Config, "🚫 Banned words updated", &[("Removed", removed.to_string()), ("Not found", (ws.len() - removed).to_string())], None))
}

fn shorten(items: &[String]) -> Vec<String> {
    if items.is_empty() { return vec![]; }
    let mut s = items.iter().take(MAX_ENTRIES_SHOWN).map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", ");
    if items.len() > MAX_ENTRIES_SHOWN { s.push_str(&format!(" … +{}", items.len() - MAX_ENTRIES_SHOWN)); }
    vec![s]
}

pub async fn words_list_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let words = store::list_words(&inv.data.db, inv.guild_id.get() as i64).await?;
    let on = one_rule(inv, RuleKind::BannedWords).await?.enabled;
    let of = |k: WordKind| words.iter().filter(|w| w.kind == k).map(|w| w.pattern.clone()).collect::<Vec<_>>();
    let groups = [("Whole word", shorten(&of(WordKind::Whole))), ("Wildcard", shorten(&of(WordKind::Wildcard))), ("Regex (advanced)", shorten(&of(WordKind::Regex)))];
    let title = format!("🚫 Banned words · {} · {} words", if on { "On" } else { "Off" }, words.len());
    Ok(reply::list(Tone::Config, &title, Some("Also matches common disguises: l33t letters, extra spaces, repeated letters."), &groups))
}

pub async fn sites_core(inv: &Invoker<'_>, site: &str, add: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let s = normalize_site(site).ok_or_else(|| user_err(format!("\"{site}\" is not a site. Use a name like youtube.com.")))?;
    let g = inv.guild_id.get() as i64;
    let changed = if add { store::add_site(&inv.data.db, g, &s).await? } else { store::remove_site(&inv.data.db, g, &s).await? };
    inv.data.automod.invalidate(inv.guild_id.get());
    let note = match (add, changed) { (true, true) => "Allowed", (true, false) => "Already allowed", (false, true) => "No longer allowed", (false, false) => "Was not on the list" };
    Ok(reply::done(Tone::Config, "🔗 Allowed sites updated", &[(note, s)], None))
}

pub async fn sites_list_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let sites = store::list_sites(&inv.data.db, inv.guild_id.get() as i64).await?;
    let on = one_rule(inv, RuleKind::Links).await?.enabled;
    let title = format!("🔗 Links · {}", if on { "On" } else { "Off" });
    Ok(reply::list(Tone::Config, &title, Some("Every link is removed unless its site is on the allowed list."), &[("Allowed sites", shorten(&sites))]))
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum RuleChoice {
    #[name = "Banned words"] BannedWords,
    #[name = "Fast messages"] FastMessages,
    #[name = "Repeated messages"] RepeatedMessages,
    #[name = "Mass mentions"] MassMentions,
    #[name = "Emoji spam"] EmojiSpam,
    #[name = "All caps"] AllCaps,
    #[name = "File spam"] FileSpam,
    #[name = "Links"] Links,
    #[name = "Server invites"] Invites,
    #[name = "Scam links"] ScamLinks,
    #[name = "Walls of text"] WallsOfText,
    #[name = "Spoiler spam"] SpoilerSpam,
    #[name = "Zalgo text"] Zalgo,
}

impl From<RuleChoice> for RuleKind {
    fn from(c: RuleChoice) -> Self {
        use RuleChoice as C;
        match c {
            C::BannedWords => Self::BannedWords, C::FastMessages => Self::FastMessages, C::RepeatedMessages => Self::RepeatedMessages,
            C::MassMentions => Self::MassMentions, C::EmojiSpam => Self::EmojiSpam, C::AllCaps => Self::AllCaps, C::FileSpam => Self::FileSpam,
            C::Links => Self::Links, C::Invites => Self::Invites, C::ScamLinks => Self::ScamLinks, C::WallsOfText => Self::WallsOfText,
            C::SpoilerSpam => Self::SpoilerSpam, C::Zalgo => Self::Zalgo,
        }
    }
}

#[derive(Debug, Clone, Copy, poise::ChoiceParameter)]
pub enum ActionChoice {
    #[name = "Delete"] Delete,
    #[name = "Delete + warn"] DeleteWarn,
    #[name = "Timeout"] Timeout,
    #[name = "Kick"] Kick,
    #[name = "Ban"] Ban,
}

impl From<ActionChoice> for RuleAction {
    fn from(c: ActionChoice) -> Self {
        match c { ActionChoice::Delete => Self::Delete, ActionChoice::DeleteWarn => Self::DeleteWarn, ActionChoice::Timeout => Self::Timeout, ActionChoice::Kick => Self::Kick, ActionChoice::Ban => Self::Ban }
    }
}

/// Banned words, spam and link rules.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD", subcommands("status", "rule", "words", "links"))]
pub async fn automod(_: Context<'_>) -> Result<(), Error> { Ok(()) }

/// Show every rule and whether it is on.
#[poise::command(slash_command, guild_only)]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, status_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Turn on, tune or inspect one rule.
#[poise::command(slash_command, guild_only, subcommands("on", "off", "view", "action", "settings", "ignore_role", "unignore_role", "ignore_channel", "unignore_channel"))]
async fn rule(_: Context<'_>) -> Result<(), Error> { Ok(()) }

/// Turn a rule on.
#[poise::command(slash_command, guild_only)]
async fn on(ctx: Context<'_>, rule: RuleChoice) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, set_enabled_core(&Invoker::from_poise(ctx)?, rule.into(), true).await?).await
}

/// Turn a rule off.
#[poise::command(slash_command, guild_only)]
async fn off(ctx: Context<'_>, rule: RuleChoice) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, set_enabled_core(&Invoker::from_poise(ctx)?, rule.into(), false).await?).await
}

/// Show one rule's settings.
#[poise::command(slash_command, guild_only)]
async fn view(ctx: Context<'_>, rule: RuleChoice) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, rule_view_core(&Invoker::from_poise(ctx)?, rule.into()).await?).await
}

/// Change what a rule does.
#[poise::command(slash_command, guild_only)]
async fn action(ctx: Context<'_>, rule: RuleChoice, action: ActionChoice, #[description = "Timeout length, like 10m (Timeout only)"] timeout: Option<String>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let d = timeout.as_deref().map(crate::duration::parse_duration).transpose()?;
    reply::send(ctx, set_action_core(&Invoker::from_poise(ctx)?, rule.into(), action.into(), d).await?).await
}

/// Change a rule's numbers.
#[poise::command(slash_command, guild_only)]
async fn settings(
    ctx: Context<'_>,
    rule: RuleChoice,
    #[description = "Count that triggers the rule"] threshold: Option<i32>,
    #[description = "Window in seconds (1-300)"] window: Option<i32>,
    #[description = "Percent (1-100)"] percent: Option<i32>,
    #[description = "Extra limit, e.g. wall characters (1-4000)"] extra: Option<i32>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, set_settings_core(&Invoker::from_poise(ctx)?, rule.into(), threshold, window, percent, extra).await?).await
}

/// Never apply a rule to a role.
#[poise::command(slash_command, guild_only, rename = "ignore-role")]
async fn ignore_role(ctx: Context<'_>, rule: RuleChoice, role: Role) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, ignore_core(&Invoker::from_poise(ctx)?, rule.into(), Target::Role(role.id.get() as i64), true).await?).await
}

/// Apply a rule to a role again.
#[poise::command(slash_command, guild_only, rename = "unignore-role")]
async fn unignore_role(ctx: Context<'_>, rule: RuleChoice, role: Role) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, ignore_core(&Invoker::from_poise(ctx)?, rule.into(), Target::Role(role.id.get() as i64), false).await?).await
}

/// Never apply a rule in a channel.
#[poise::command(slash_command, guild_only, rename = "ignore-channel")]
async fn ignore_channel(ctx: Context<'_>, rule: RuleChoice, channel: GuildChannel) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, ignore_core(&Invoker::from_poise(ctx)?, rule.into(), Target::Channel(channel.id.get() as i64), true).await?).await
}

/// Apply a rule in a channel again.
#[poise::command(slash_command, guild_only, rename = "unignore-channel")]
async fn unignore_channel(ctx: Context<'_>, rule: RuleChoice, channel: GuildChannel) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, ignore_core(&Invoker::from_poise(ctx)?, rule.into(), Target::Channel(channel.id.get() as i64), false).await?).await
}

/// Manage banned words.
#[poise::command(slash_command, guild_only, subcommands("words_add", "words_remove", "words_list"))]
async fn words(_: Context<'_>) -> Result<(), Error> { Ok(()) }

/// Add banned words.
#[poise::command(slash_command, guild_only, rename = "add")]
async fn words_add(ctx: Context<'_>, #[description = "Separate with ;  *wild*card  /regex/"] words: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, words_add_core(&Invoker::from_poise(ctx)?, &words).await?).await
}

/// Remove banned words.
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn words_remove(ctx: Context<'_>, #[description = "Separate with ;"] words: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, words_remove_core(&Invoker::from_poise(ctx)?, &words).await?).await
}

/// List banned words.
#[poise::command(slash_command, guild_only, rename = "list")]
async fn words_list(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, words_list_core(&Invoker::from_poise(ctx)?).await?).await
}

/// Manage allowed link sites.
#[poise::command(slash_command, guild_only, subcommands("links_allow", "links_remove", "links_list"))]
async fn links(_: Context<'_>) -> Result<(), Error> { Ok(()) }

/// Allow a site.
#[poise::command(slash_command, guild_only, rename = "allow")]
async fn links_allow(ctx: Context<'_>, #[description = "e.g. youtube.com"] site: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, sites_core(&Invoker::from_poise(ctx)?, &site, true).await?).await
}

/// Stop allowing a site.
#[poise::command(slash_command, guild_only, rename = "remove")]
async fn links_remove(ctx: Context<'_>, site: String) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, sites_core(&Invoker::from_poise(ctx)?, &site, false).await?).await
}

/// List allowed sites.
#[poise::command(slash_command, guild_only, rename = "list")]
async fn links_list(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, sites_list_core(&Invoker::from_poise(ctx)?).await?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(k: RuleKind) -> RuleConfig { k.defaults() }

    #[test]
    fn settings_validation() {
        let f = d(RuleKind::FastMessages);
        assert!(validate_settings(&f, Some(1), None, None, None).unwrap_err().0.contains("between 2 and 100"));
        assert!(validate_settings(&f, Some(101), None, None, None).is_err());
        assert!(validate_settings(&f, None, Some(0), None, None).unwrap_err().0.contains("Window"));
        assert!(validate_settings(&f, None, Some(301), None, None).is_err());
        assert!(validate_settings(&d(RuleKind::AllCaps), None, None, Some(101), None).unwrap_err().0.contains("Percent"));
        assert!(validate_settings(&d(RuleKind::WallsOfText), None, None, None, Some(4001)).is_err());
        assert!(validate_settings(&d(RuleKind::Zalgo), Some(21), None, None, None).is_err());
        let ok = validate_settings(&f, Some(8), Some(10), None, None).unwrap();
        assert_eq!((ok.threshold, ok.window_secs), (8, 10));
        assert_eq!(validate_settings(&d(RuleKind::Zalgo), Some(1), None, None, None).unwrap().threshold, 1);
    }

    #[test]
    fn timeout_limits() {
        assert!(validate_timeout(Duration::seconds(59)).is_err());
        assert_eq!(validate_timeout(Duration::minutes(10)).unwrap(), 600);
        assert!(validate_timeout(Duration::days(28)).is_ok());
        assert!(validate_timeout(Duration::days(29)).is_err());
    }

    #[test]
    fn site_normalising() {
        assert_eq!(normalize_site("https://www.YouTube.com/watch?v=1").as_deref(), Some("youtube.com"));
        assert_eq!(normalize_site("tenor.com:443/x").as_deref(), Some("tenor.com"));
        assert_eq!(normalize_site("localhost"), None);
        assert_eq!(normalize_site("not a site.com"), None);
    }

    #[test]
    fn rule_lines() {
        let mut c = d(RuleKind::RepeatedMessages);
        assert_eq!(rule_line(&c, 0, 0), "Off");
        c.enabled = true;
        assert_eq!(rule_line(&c, 0, 0), "On · 3 similar in 30s · delete + warn");
        let mut f = d(RuleKind::FastMessages);
        f.enabled = true;
        assert_eq!(rule_line(&f, 0, 0), "On · 6 in 5s · timeout 10m");
        let mut l = d(RuleKind::Links);
        l.enabled = true;
        assert_eq!(rule_line(&l, 0, 5), "On · 5 allowed sites · delete");
    }

    #[test]
    fn splits_entries() {
        assert_eq!(entries(" a ; b;;/x;y/ "), vec!["a", "b", "/x", "y/"]);
    }
}

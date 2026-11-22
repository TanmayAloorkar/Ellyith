//! Config operations shared by `/config …` and the Config panel. Each setter authorises first.
use crate::duration::format_duration;
use crate::invoker::Invoker;
use crate::logging::{self, LogCategory};
use crate::moderation::cases::CaseKind;
use crate::moderation::ladder::{self, LadderRule};
use crate::moderation::nickname::validate_filter_config;
use crate::moderation::perms::require_manage_server;
use crate::moderation::templates;
use crate::reply::{done, Tone};
use crate::settings::{self, GuildSettings};
use crate::{Error, UserError};
use poise::serenity_prelude::{ChannelId, CreateEmbed, CreateMessage, RoleId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleList {
    Mod,
    Protected,
}

fn gid(inv: &Invoker<'_>) -> i64 {
    inv.guild_id.get() as i64
}

async fn load(inv: &Invoker<'_>) -> Result<GuildSettings, Error> {
    Ok(settings::get_or_create(&inv.data.db, gid(inv)).await?)
}

pub fn role_mentions(ids: &[i64]) -> String {
    if ids.is_empty() { "none".into() } else { ids.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ") }
}

pub fn describe_rule(r: &LadderRule) -> String {
    match r.duration_secs {
        Some(s) => format!("{} for {}", r.action.as_str(), format_duration(chrono::Duration::seconds(s))),
        None => r.action.as_str().to_string(),
    }
}

pub fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Replaces the whole list.
pub async fn set_role_list(inv: &Invoker<'_>, which: RoleList, roles: Vec<RoleId>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    let mut ids: Vec<i64> = vec![];
    for r in roles {
        let id = r.get() as i64;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let title = match which {
        RoleList::Mod => {
            s.mod_roles = ids.clone();
            "🛡️ Moderator roles saved"
        }
        RoleList::Protected => {
            s.protected_roles = ids.clone();
            "🔒 Protected roles saved"
        }
    };
    settings::save(&inv.data.db, &s).await?;
    Ok(done(Tone::Config, title, &[("Roles", role_mentions(&ids))], None))
}

/// `None` disables the category.
pub async fn set_log_channel(inv: &Invoker<'_>, cat: LogCategory, channel: Option<ChannelId>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let db = &inv.data.db;
    match channel {
        Some(ch) => {
            let hello = CreateMessage::new().content(format!("Logging {} here.", cat.as_str()));
            if ch.send_message(inv.ctx, hello).await.is_err() {
                return Err(UserError("I can't post in that channel — check my permissions there.".into()).into());
            }
            logging::set_channel(db, gid(inv), cat, ch.get() as i64).await?;
            Ok(done(Tone::Config, "📋 Log channel saved", &[("Category", cat.as_str().into()), ("Channel", format!("<#{ch}>"))], None))
        }
        None => {
            let was = logging::clear(db, gid(inv), cat).await?;
            let state = if was { "disabled" } else { "was not enabled" };
            Ok(done(Tone::Config, "📋 Log channel", &[("Category", cat.as_str().into()), ("Status", state.into())], None))
        }
    }
}

/// `None` restores the default template.
pub async fn set_dm_template(inv: &Invoker<'_>, kind: CaseKind, text: Option<String>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    if !templates::dm_kinds().contains(&kind) {
        return Err(UserError("That action has no DM message.".into()).into());
    }
    let db = &inv.data.db;
    let state = match text {
        Some(t) => {
            if t.chars().count() > 1500 {
                return Err(UserError("Template must be 1500 characters or fewer.".into()).into());
            }
            templates::set(db, gid(inv), kind, &t).await?;
            "saved"
        }
        None => {
            templates::reset(db, gid(inv), kind).await?;
            "reset to default"
        }
    };
    Ok(done(Tone::Config, "✉️ DM message", &[("Kind", kind.label().into()), ("Status", state.into())], None))
}

pub async fn set_dm_showmod(inv: &Invoker<'_>, show: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    s.dm_show_moderator = show;
    settings::save(&inv.data.db, &s).await?;
    Ok(done(Tone::Config, "✉️ Moderator name in DMs", &[("Now", on_off(show).into())], None))
}

pub async fn ladder_add(inv: &Invoker<'_>, rule: LadderRule) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    ladder::validate_rule(&rule)?;
    ladder::set_rule(&inv.data.db, gid(inv), &rule).await?;
    Ok(done(Tone::Config, "📈 Ladder rule saved", &[("At", format!("{} warnings", rule.warn_count)), ("Action", describe_rule(&rule))], None))
}

pub async fn ladder_remove(inv: &Invoker<'_>, warns: i32) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let state = if ladder::remove_rule(&inv.data.db, gid(inv), warns).await? { "removed" } else { "no rule there" };
    Ok(done(Tone::Config, "📈 Ladder rule", &[("At", format!("{warns} warnings")), ("Status", state.into())], None))
}

/// `Some(days)` turns decay on with that many days; `None` turns it off.
pub async fn set_decay(inv: &Invoker<'_>, days: Option<i32>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    if days.is_some_and(|d| !(1..=365).contains(&d)) {
        return Err(UserError("Days must be between 1 and 365.".into()).into());
    }
    let mut s = load(inv).await?;
    s.warn_decay_enabled = days.is_some();
    if let Some(d) = days {
        s.warn_decay_days = d;
    }
    settings::save(&inv.data.db, &s).await?;
    let now = if s.warn_decay_enabled { format!("warnings expire after {} days", s.warn_decay_days) } else { "off".into() };
    Ok(done(Tone::Config, "⏱️ Warn decay", &[("Now", now)], None))
}

pub async fn set_nick_filter(inv: &Invoker<'_>, on: bool) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    s.nick_filter_enabled = on;
    settings::save(&inv.data.db, &s).await?;
    Ok(done(Tone::Config, "🚫 Name filter", &[("Now", on_off(on).into())], None))
}

/// Replaces the whole list. Words must already be trimmed and lowercased.
pub async fn set_nick_words(inv: &Invoker<'_>, words: Vec<String>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    validate_filter_config(&s.nick_fallback, &words, &s.hoist_chars)?;
    s.filtered_words = words;
    settings::save(&inv.data.db, &s).await?;
    let now = if s.filtered_words.is_empty() { "none".into() } else { s.filtered_words.join(", ") };
    Ok(done(Tone::Config, "🚫 Filtered words saved", &[("Words", now)], None))
}

pub async fn set_hoist(inv: &Invoker<'_>, chars: String) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let mut s = load(inv).await?;
    validate_filter_config(&s.nick_fallback, &s.filtered_words, &chars)?;
    s.hoist_chars = chars;
    settings::save(&inv.data.db, &s).await?;
    Ok(done(Tone::Config, "🚫 Hoisting characters saved", &[("Characters", s.hoist_chars)], None))
}

pub async fn set_fallback(inv: &Invoker<'_>, name: String) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    if name.chars().count() > 32 {
        return Err(UserError("Nicknames can be at most 32 characters.".into()).into());
    }
    let mut s = load(inv).await?;
    validate_filter_config(&name, &s.filtered_words, &s.hoist_chars)?;
    s.nick_fallback = name;
    settings::save(&inv.data.db, &s).await?;
    Ok(done(Tone::Config, "🚫 Fallback nickname saved", &[("Name", s.nick_fallback)], None))
}

/// Current values, one line each, for the Config category reply.
pub async fn settings_summary(inv: &Invoker<'_>) -> Result<Vec<String>, Error> {
    require_manage_server(inv).await?;
    let s = load(inv).await?;
    let logs = logging::list(&inv.data.db, gid(inv)).await?.len();
    let rules = ladder::list_rules(&inv.data.db, gid(inv)).await?.len();
    let decay = if s.warn_decay_enabled { format!("{} days", s.warn_decay_days) } else { "off".into() };
    Ok(vec![
        format!("**Mod roles** {}", role_mentions(&s.mod_roles)),
        format!("**Protected** {}", role_mentions(&s.protected_roles)),
        format!("**Log channels** {logs} of {} set", LogCategory::ALL.len()),
        format!("**Moderator name in DMs** {}", on_off(s.dm_show_moderator)),
        format!("**Ladder** {rules} rules"),
        format!("**Warn decay** {decay}"),
        format!("**Name filter** {} · {} words", on_off(s.nick_filter_enabled), s.filtered_words.len()),
    ])
}

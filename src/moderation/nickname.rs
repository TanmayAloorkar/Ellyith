use crate::moderation::actions::record_case;
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::perms::{is_exempt_from_auto, member_facts};
use crate::settings;
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{self as serenity, EditMember, Member};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NickViolation {
    FilteredWord,
    Hoisting,
    Unmentionable,
}

impl NickViolation {
    pub fn describe(self) -> &'static str {
        match self {
            Self::FilteredWord => "contains a filtered word",
            Self::Hoisting => "starts with a hoisting character",
            Self::Unmentionable => "has no letters or digits (cannot be mentioned)",
        }
    }
}

pub fn check_name(name: &str, filtered_words: &[String], hoist_chars: &str) -> Option<NickViolation> {
    let lower = name.to_lowercase();
    if filtered_words.iter().any(|w| !w.is_empty() && lower.contains(&w.to_lowercase())) {
        return Some(NickViolation::FilteredWord);
    }
    if name.chars().next().is_some_and(|c| hoist_chars.contains(c)) {
        return Some(NickViolation::Hoisting);
    }
    if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Some(NickViolation::Unmentionable);
    }
    None
}

/// The fallback must itself be clean, or enforcement would reset forever.
pub fn validate_filter_config(fallback: &str, filtered_words: &[String], hoist_chars: &str) -> Result<(), UserError> {
    match check_name(fallback, filtered_words, hoist_chars) {
        None => Ok(()),
        Some(v) => Err(UserError(format!("The fallback nickname {}, so it would be reset in a loop.", v.describe()))),
    }
}

pub async fn enforce(ctx: &serenity::Context, data: &Data, member: &Member) -> Result<(), Error> {
    if member.user.bot {
        return Ok(());
    }
    let s = settings::get_or_create(&data.db, member.guild_id.get() as i64).await?;
    if !s.nick_filter_enabled {
        return Ok(());
    }
    let Some(violation) = check_name(member.display_name(), &s.filtered_words, &s.hoist_chars) else { return Ok(()) };
    let facts = member_facts(ctx, member.guild_id, member).await?;
    if is_exempt_from_auto(&facts, &s) || facts.is_owner {
        return Ok(());
    }
    // Stale config (e.g. a word added later) could make the fallback itself bad: stop, don't loop.
    if check_name(&s.nick_fallback, &s.filtered_words, &s.hoist_chars).is_some() {
        tracing::warn!("nick fallback in guild {} violates its own filter; not enforcing", member.guild_id);
        return Ok(());
    }
    let bot_id = ctx.cache.current_user().id;
    let bot_member = member.guild_id.member(&ctx.http, bot_id).await?;
    let bot = member_facts(ctx, member.guild_id, &bot_member).await?;
    if facts.top_role_position >= bot.top_role_position {
        tracing::debug!("cannot reset nick of {}: role not below the bot", member.user.id);
        return Ok(());
    }
    member
        .guild_id
        .edit_member(
            &ctx.http,
            member.user.id,
            EditMember::new().nickname(&s.nick_fallback).audit_log_reason("Nickname filter"),
        )
        .await?;
    record_case(
        ctx,
        data,
        None,
        NewCase {
            guild_id: member.guild_id.get() as i64,
            kind: CaseKind::NickReset,
            target_id: member.user.id.get() as i64,
            actor: Actor::System,
            reason: Some(format!("Nickname {}", violation.describe())),
            duration_secs: None,
            expires_at: None,
            dm_failed: false,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let words = vec!["badword".to_string()];
        assert_eq!(check_name("Tanmay", &words, "!"), None);
        assert_eq!(check_name("!Tanmay", &words, "!"), Some(NickViolation::Hoisting));
        assert_eq!(check_name("xBADWORDx", &words, "!"), Some(NickViolation::FilteredWord));
        assert_eq!(check_name("ｔａｎ", &words, "!"), Some(NickViolation::Unmentionable));
        assert_eq!(check_name("★★★", &words, "!"), Some(NickViolation::Unmentionable));
        assert!(validate_filter_config("Moderated Nickname", &words, "!").is_ok());
        assert!(validate_filter_config("!Renamed", &words, "!").is_err());
        assert!(validate_filter_config("Moderated Nickname", &["nick".to_string()], "!").is_err());
    }
}

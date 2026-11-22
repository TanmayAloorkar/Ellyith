pub mod config;
pub mod punish;
pub mod cases;
pub mod nick;
pub mod voice;
pub mod purge;

use crate::duration::format_duration;
use crate::invoker::Invoker;
use crate::logging::{channel_for, LogCategory};
use crate::moderation::cases::{self as case_store, Case, CaseKind};
use crate::moderation::templates::dm_kinds;
use crate::reply::{done, Tone};
use crate::Error;
use poise::serenity_prelude::{ChannelId, CreateEmbed, UserId};

/// 1 -> "1st", 12 -> "12th", 21 -> "21st".
pub fn ordinal_suffix(n: i64) -> String {
    let s = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{s}")
}

/// Display name for a user id; falls back to the id when Discord cannot resolve it.
pub(crate) async fn username(inv: &Invoker<'_>, id: UserId) -> String {
    id.to_user(inv.ctx).await.map(|u| u.name).unwrap_or_else(|_| id.to_string())
}

/// "Logged to #channel" when the mod_actions log is set and its channel name is known.
/// A lookup failure means no footer: the action has already happened.
pub(crate) async fn log_footer(inv: &Invoker<'_>) -> Option<String> {
    let ch = channel_for(&inv.data.db, inv.guild_id.get() as i64, LogCategory::ModActions).await.ok().flatten()?;
    let id = ChannelId::new(ch as u64);
    inv.ctx.cache.guild(inv.guild_id).and_then(|g| g.channels.get(&id).map(|c| format!("Logged to #{}", c.name)))
}

/// Past-tense title prefix for the done embed.
fn done_label(kind: CaseKind) -> &'static str {
    match kind {
        CaseKind::Warn => "⚠️ Warned",
        CaseKind::Timeout => "⏳ Timed out",
        CaseKind::Untimeout => "🔊 Removed timeout from",
        CaseKind::Kick => "👢 Kicked",
        CaseKind::Ban => "🔨 Banned",
        CaseKind::Tempban => "⏱️ Temporarily banned",
        CaseKind::Unban => "🔓 Unbanned",
        CaseKind::NickSet => "✏️ Nickname set for",
        CaseKind::NickReset => "✏️ Nickname reset for",
        CaseKind::VoiceDisconnect => "🔌 Disconnected",
        CaseKind::VoiceMute => "🎙️ Voice-muted",
        CaseKind::VoiceUnmute => "🎙️ Voice-unmuted",
        CaseKind::VoiceMove => "➡️ Moved",
    }
}

/// Reply for a moderation action. `lines` holds Reason, Duration and any extras;
/// Case and DM lines are added here when the action made a case.
pub(crate) async fn action_reply(
    inv: &Invoker<'_>,
    tone: Tone,
    kind: CaseKind,
    target: UserId,
    mut lines: Vec<(&'static str, String)>,
    case: Option<&Case>,
) -> Result<CreateEmbed, Error> {
    let name = username(inv, target).await;
    if let Some(c) = case {
        let n = case_store::ordinal(&inv.data.db, inv.guild_id.get() as i64, target.get() as i64, c.case_number).await?;
        lines.insert(0, ("Case", format!("#{} · {name}'s {} case", c.case_number, ordinal_suffix(n))));
        if dm_kinds().contains(&kind) {
            lines.push(("DM", if c.dm_failed { "Could not be delivered" } else { "Delivered" }.into()));
        }
    }
    let footer = log_footer(inv).await;
    Ok(done(tone, &format!("{} {name}", done_label(kind)), &lines, footer.as_deref()))
}

/// Reason and duration lines shared by the action replies.
pub(crate) fn detail_lines(reason: Option<&str>, duration: Option<chrono::Duration>) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    if let Some(r) = reason.filter(|r| !r.is_empty()) {
        v.push(("Reason", r.to_string()));
    }
    if let Some(d) = duration {
        v.push(("Duration", format_duration(d)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals() {
        for (n, s) in [(1, "1st"), (2, "2nd"), (3, "3rd"), (4, "4th"), (11, "11th"), (12, "12th"), (13, "13th"), (21, "21st"), (22, "22nd"), (101, "101st"), (111, "111th")] {
            assert_eq!(ordinal_suffix(n), s);
        }
    }
}

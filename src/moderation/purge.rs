use crate::invoker::Invoker;
use crate::logging::{send_log, LogCategory};
use crate::moderation::actions::discord_error;
use crate::moderation::commands::log_footer;
use crate::moderation::perms::{authorize, member_facts, require_member};
use crate::reply::{done, Tone};
use crate::{Error, UserError};
use chrono::{DateTime, Duration, Utc};
use poise::serenity_prelude::{ChannelId, CreateEmbed, GetMessages, Member, MessageId, Permissions, UserId};

const MAX_SCAN: usize = 1000;
const BAD_AMOUNT: &str = "Purge needs a number from 1 to 1000, or a time like 10m or 2h (up to 14 days).";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeAmount {
    Count(u32),
    Since(Duration),
}

pub fn parse_amount(s: &str) -> Result<PurgeAmount, UserError> {
    let err = || UserError(BAD_AMOUNT.into());
    let s = s.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return match s.parse::<u32>() {
            Ok(n @ 1..=1000) => Ok(PurgeAmount::Count(n)),
            _ => Err(err()),
        };
    }
    let d = crate::duration::parse_duration(s).map_err(|_| err())?;
    if d > Duration::days(14) {
        return Err(err());
    }
    Ok(PurgeAmount::Since(d))
}

pub struct MsgInfo {
    pub id: u64,
    pub created: DateTime<Utc>,
    pub pinned: bool,
    pub author: u64,
}

#[derive(Debug, Default)]
pub struct PurgePlan {
    pub delete: Vec<u64>,
    pub too_old: usize,
    pub pinned: usize,
}

pub fn plan(newest_first: &[MsgInfo], amount: &PurgeAmount, member: Option<u64>, now: DateTime<Utc>) -> PurgePlan {
    let matching = newest_first.iter().filter(|m| member.is_none_or(|u| m.author == u));
    let candidates: Vec<&MsgInfo> = match amount {
        PurgeAmount::Count(n) => matching.take(*n as usize).collect(),
        PurgeAmount::Since(d) => matching.filter(|m| m.created > now - *d).take(MAX_SCAN).collect(),
    };
    let mut p = PurgePlan::default();
    for m in candidates {
        if m.pinned {
            p.pinned += 1;
        } else if now - m.created >= Duration::days(14) - Duration::minutes(1) {
            p.too_old += 1;
        } else {
            p.delete.push(m.id);
        }
    }
    p
}

fn describe(amount: &PurgeAmount) -> String {
    match amount {
        PurgeAmount::Count(n) => format!("{n} messages"),
        PurgeAmount::Since(d) => format!("last {}", crate::duration::format_duration(*d)),
    }
}

/// `need` in the channel, from the cache.
fn channel_perms_ok(inv: &Invoker<'_>, channel: ChannelId, member: &Member, need: Permissions) -> bool {
    inv.ctx.cache.guild(inv.guild_id).is_some_and(|g| g.channels.get(&channel).is_some_and(|c| g.user_permissions_in(c, member).contains(need)))
}

pub async fn run(inv: &Invoker<'_>, channel: ChannelId, amount: PurgeAmount, member: Option<UserId>, reason: Option<String>) -> Result<CreateEmbed, Error> {
    authorize(inv, Permissions::MANAGE_MESSAGES, None).await?;
    let mod_m = require_member(inv, inv.user.id).await?;
    let mod_f = member_facts(inv.ctx, inv.guild_id, &mod_m).await?;
    let bypass = mod_f.is_owner || mod_f.permissions.contains(Permissions::ADMINISTRATOR);
    // A mod role alone is not enough: the moderator needs Manage Messages in that channel.
    if !bypass && !channel_perms_ok(inv, channel, &mod_m, Permissions::VIEW_CHANNEL | Permissions::MANAGE_MESSAGES) {
        return Err(UserError("You need Manage Messages in that channel.".into()).into());
    }
    let bot_id = inv.ctx.cache.current_user().id;
    let bot_m = require_member(inv, bot_id).await?;
    if !channel_perms_ok(inv, channel, &bot_m, Permissions::MANAGE_MESSAGES | Permissions::READ_MESSAGE_HISTORY) {
        return Err(UserError("I don't have permission to do that.".into()).into());
    }
    let now = Utc::now();
    let member_id = member.map(|u| u.get());
    let mut scanned: Vec<MsgInfo> = Vec::new();
    let mut before: Option<MessageId> = None;
    loop {
        let mut q = GetMessages::new().limit(100);
        if let Some(b) = before {
            q = q.before(b);
        }
        let page = channel.messages(inv.ctx, q).await.map_err(discord_error)?;
        let full = page.len() == 100;
        before = page.last().map(|m| m.id);
        scanned.extend(page.iter().map(|m| MsgInfo { id: m.id.get(), created: *m.timestamp, pinned: m.pinned, author: m.author.id.get() }));
        let matched = scanned.iter().filter(|m| member_id.is_none_or(|u| m.author == u)).count();
        let satisfied = match &amount {
            PurgeAmount::Count(n) => matched >= *n as usize,
            PurgeAmount::Since(d) => scanned.last().is_some_and(|m| m.created <= now - *d),
        };
        if !full || satisfied || scanned.len() >= MAX_SCAN {
            break;
        }
    }
    let p = plan(&scanned, &amount, member_id, now);
    let ids: Vec<MessageId> = p.delete.iter().map(|i| MessageId::new(*i)).collect();
    let mut deleted = 0;
    let mut failure: Option<Error> = None;
    for chunk in ids.chunks(100) {
        let res = match chunk {
            [one] => channel.delete_message(inv.ctx, *one).await,
            _ => channel.delete_messages(inv.ctx, chunk).await,
        };
        match res {
            Ok(()) => deleted += chunk.len(),
            // Unknown Message (10008): already gone, nothing to do.
            Err(e) if matches!(&e, poise::serenity_prelude::Error::Http(poise::serenity_prelude::HttpError::UnsuccessfulRequest(r)) if r.error.code == 10008) => {}
            Err(e) => {
                failure = Some(discord_error(e));
                break;
            }
        }
    }
    if deleted == 0 {
        if let Some(e) = failure {
            return Err(e);
        }
    }
    let mut skipped = Vec::new();
    if p.too_old > 0 {
        skipped.push(format!("{} older than 14 days", p.too_old));
    }
    if p.pinned > 0 {
        skipped.push(format!("{} pinned", p.pinned));
    }
    let skipped = skipped.join(", ");
    let mut log = CreateEmbed::new()
        .title("🧹 Purge · no case")
        .field("Moderator", format!("<@{}>", inv.user.id), true)
        .field("Channel", format!("<#{channel}>"), true)
        .field("Asked", describe(&amount), true)
        .field("Deleted", deleted.to_string(), true)
        .field("Skipped", if skipped.is_empty() { "none".into() } else { skipped.clone() }, true);
    if failure.is_some() {
        log = log.field("Note", "Stopped early after a Discord error.", false);
    }
    if let Some(u) = member {
        log = log.field("Member", format!("<@{u}>"), true);
    }
    let reason_text = reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given");
    log = log.field("Reason", reason_text.chars().take(1024).collect::<String>(), false);
    let name = inv.ctx.cache.guild(inv.guild_id).and_then(|g| g.channels.get(&channel).map(|c| c.name.clone())).unwrap_or_else(|| channel.to_string());
    if deleted == 0 {
        let lines: Vec<(&str, String)> = if skipped.is_empty() { vec![] } else { vec![("Skipped", skipped)] };
        return Ok(done(Tone::Channels, &format!("🧹 Nothing to purge in #{name}"), &lines, None));
    }
    send_log(&inv.ctx.http, &inv.data.db, inv.guild_id, LogCategory::ModActions, log).await;
    let mut lines = vec![("Deleted", deleted.to_string())];
    if let Some(e) = failure {
        let msg = match e.downcast_ref::<UserError>() {
            Some(u) => u.0.clone(),
            None => "Discord rejected that action.".into(),
        };
        lines.push(("Stopped early", msg));
        if !skipped.is_empty() {
            lines.push(("Skipped", skipped));
        }
        return Ok(done(Tone::Channels, &format!("🧹 Purge stopped early in #{name}"), &lines, None));
    }
    if !skipped.is_empty() {
        lines.push(("Skipped", skipped));
    }
    let footer = log_footer(inv).await;
    Ok(done(Tone::Channels, &format!("🧹 Purged #{name}"), &lines, footer.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert!(matches!(parse_amount("50"), Ok(PurgeAmount::Count(50))));
        assert!(matches!(parse_amount("1000"), Ok(PurgeAmount::Count(1000))));
        assert!(matches!(parse_amount("10m"), Ok(PurgeAmount::Since(d)) if d.num_seconds() == 600));
        assert!(matches!(parse_amount("14d"), Ok(PurgeAmount::Since(_))));
        for bad in ["0", "1001", "15d", "abc", "-5", "5x", ""] {
            assert!(parse_amount(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn plans() {
        let now = Utc::now();
        let m = |id, mins_ago: i64, pinned, author| MsgInfo { id, created: now - Duration::minutes(mins_ago), pinned, author };
        let msgs = vec![m(5, 1, false, 1), m(4, 2, true, 1), m(3, 3, false, 2), m(2, 60 * 24 * 15, false, 1), m(1, 5, false, 1)];
        let p = plan(&msgs, &PurgeAmount::Count(4), None, now);
        assert_eq!(p.delete, vec![5, 3]);
        assert_eq!(p.pinned, 1);
        assert_eq!(p.too_old, 1);
        let p = plan(&msgs, &PurgeAmount::Count(10), Some(2), now);
        assert_eq!(p.delete, vec![3]);
        let p = plan(&msgs, &PurgeAmount::Since(Duration::minutes(4)), None, now);
        assert_eq!(p.delete, vec![5, 3]);
        assert_eq!(p.pinned, 1);
        assert_eq!(p.too_old, 0);
        let p = plan(&msgs, &PurgeAmount::Count(3), Some(99), now);
        assert!(p.delete.is_empty() && p.pinned == 0 && p.too_old == 0);
    }
}

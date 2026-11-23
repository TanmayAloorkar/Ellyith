//! Alt scoring on join, and the staff card for possible alts and new accounts.
use super::logic::{alt_score, name_similar, AltInput, AltScore};
use super::store::{self, BanRow};
use super::EMOJI_ALTS;
use crate::logging::{send_log_message, LogCategory};
use crate::panel::id::{AltAct, ProtId};
use crate::reply::{self, Tone};
use crate::safety::store as safety_store;
use crate::Data;
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{self as serenity, ButtonStyle, ChannelId, CreateActionRow, CreateMessage, GuildId, Member, UserId};

pub const REASON: &str = "Possible alt";

/// True when a ban with the same invite code landed at most 24 h before the join.
pub fn invite_after_ban(code: Option<&str>, bans: &[BanRow], joined: DateTime<Utc>) -> bool {
    let Some(code) = code else { return false };
    bans.iter().any(|b| {
        let gap = joined - b.banned_at;
        b.invite_code.as_deref() == Some(code) && gap >= chrono::Duration::zero() && gap <= chrono::Duration::hours(24)
    })
}

/// The first ban from the last 30 days whose username or display name looks like the member's.
fn name_match<'a>(names: &[&str], bans: &'a [BanRow]) -> Option<&'a BanRow> {
    bans.iter().find(|b| names.iter().any(|n| name_similar(n, &b.username) || name_similar(n, &b.display_name)))
}

pub async fn score_member(_ctx: &serenity::Context, data: &Data, member: &Member) -> AltScore {
    let (gid, uid) = (member.guild_id.get() as i64, member.user.id.get() as i64);
    let now = Utc::now();
    let s = match store::get_settings(&data.db, gid).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("alt settings for {gid} failed: {e}");
            return AltScore { total: 0, lines: vec![] };
        }
    };
    let mut bans = store::recent_bans(&data.db, gid, now - chrono::Duration::days(30)).await.unwrap_or_else(|e| {
        tracing::warn!("alt: recent bans for {gid} failed: {e}");
        vec![]
    });
    // A rejoin after an unban is not an alt of themselves.
    bans.retain(|b| b.user_id != uid);
    let code = match store::member_invite(&data.db, gid, uid).await {
        Ok(v) => v.and_then(|(c, _)| c),
        Err(e) => {
            tracing::warn!("alt: invite lookup for {uid} failed: {e}");
            None
        }
    };
    let names: Vec<&str> = [Some(member.user.name.as_str()), member.user.global_name.as_deref(), member.nick.as_deref()].into_iter().flatten().collect();
    let created = member.user.created_at().unix_timestamp();
    let joined = member.joined_at.map_or(now, |t| DateTime::from_timestamp(t.unix_timestamp(), 0).unwrap_or(now));
    let input = AltInput {
        account_age_days: (now.timestamp() - created).max(0) / 86_400,
        default_avatar: member.user.avatar.is_none(),
        name_matches: name_match(&names, &bans).map(|b| format!("@{}", b.username)),
        invite_after_ban: invite_after_ban(code.as_deref(), &bans, joined),
    };
    alt_score(&s, &input)
}

fn buttons(user: UserId) -> Vec<CreateActionRow> {
    let b = |a, label, emoji, style| super::raid::button(ProtId::Alt(a, user), label, emoji, style);
    vec![CreateActionRow::Buttons(vec![
        b(AltAct::Approve, "Approve", "✅", ButtonStyle::Success),
        b(AltAct::Quarantine, "Quarantine", "🚧", ButtonStyle::Secondary),
        b(AltAct::Kick, "Kick", "👢", ButtonStyle::Secondary),
        b(AltAct::Ban, "Ban", "🔨", ButtonStyle::Danger),
    ])]
}

/// Posts a card with Approve / Quarantine / Kick / Ban in the quarantine review channel, else the mod-actions log.
pub async fn post_card(ctx: &serenity::Context, data: &Data, guild: GuildId, user: UserId, title: &str, lines: &[(&str, String)]) {
    let msg = CreateMessage::new().embed(reply::done(Tone::Safety, title, lines, None)).components(buttons(user));
    match safety_store::get_settings(&data.db, guild.get() as i64).await {
        Ok(s) => {
            if let Some(ch) = s.review_channel_id {
                match ChannelId::new(ch as u64).send_message(ctx, msg.clone()).await {
                    Ok(_) => return,
                    Err(e) => tracing::warn!("card for {user} in review channel failed: {e}"),
                }
            }
        }
        Err(e) => tracing::warn!("review channel lookup for {guild} failed: {e}"),
    }
    if send_log_message(&ctx.http, &data.db, guild, LogCategory::ModActions, msg).await.is_none() {
        tracing::warn!("card for {user}: no mod-actions log channel");
    }
}

pub async fn post_flag(ctx: &serenity::Context, data: &Data, member: &Member, score: &AltScore, invite: Option<(String, Option<u64>)>) {
    let user = member.user.id;
    let mut lines = vec![("User", format!("<@{user}> (@{})", member.user.name))];
    lines.extend(score.lines.iter().map(|(l, d, p)| (*l, format!("{d} · +{p}"))));
    // The score's own Invite line only shows when it scored; this one always names the invite.
    lines.retain(|(l, _)| *l != "Invite");
    let invite = match invite {
        Some((code, Some(by))) => format!("{code}, made by <@{by}>"),
        Some((code, None)) => code,
        None => "unknown".into(),
    };
    let scored = score.lines.iter().find(|l| l.0 == "Invite").map_or(String::new(), |l| format!(" · +{}", l.2));
    lines.push(("Invite", format!("{invite}{scored}")));
    post_card(ctx, data, member.guild_id, user, &format!("{EMOJI_ALTS} Possible alt · score {}", score.total), &lines).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ban(code: Option<&str>, at: DateTime<Utc>) -> BanRow {
        BanRow { guild_id: 1, user_id: 2, username: "toxicguy".into(), display_name: "Toxic".into(), invite_code: code.map(Into::into), banned_at: at }
    }

    #[test]
    fn invite_signal_window() {
        let join = Utc::now();
        assert!(invite_after_ban(Some("abc"), &[ban(Some("abc"), join - chrono::Duration::hours(2))], join));
        assert!(!invite_after_ban(Some("abc"), &[ban(Some("abc"), join - chrono::Duration::hours(30))], join));
        assert!(!invite_after_ban(Some("abc"), &[ban(Some("xyz"), join - chrono::Duration::hours(2))], join));
        assert!(!invite_after_ban(None, &[ban(None, join - chrono::Duration::hours(2))], join));
    }

    #[test]
    fn name_match_any_name() {
        let bans = [ban(None, Utc::now())];
        assert!(name_match(&["alice", "t0xicguy"], &bans).is_some());
        assert!(name_match(&["alice"], &bans).is_none());
    }
}

use super::{send_log, LogCategory};
use crate::text::truncate;
use crate::Data;
use poise::serenity_prelude::{
    json, audit_log::{Action, AuditLogEntry, ChannelAction, ChannelOverwriteAction, Change, MemberAction, RoleAction},
    Context, CreateEmbed, GuildId, Member, Timestamp, User, VoiceState,
};

/// Which log category an audit entry belongs to, if any.
pub fn categorize(action: &Action, changes: &[Change], executor_is_bot: bool) -> Option<LogCategory> {
    // The bot's own kicks/bans/timeouts are already in ModActions.
    let external = (!executor_is_bot).then_some(LogCategory::ExternalMod);
    match action {
        Action::Member(MemberAction::Kick | MemberAction::BanAdd | MemberAction::BanRemove) => external,
        Action::Member(MemberAction::RoleUpdate) => Some(LogCategory::MemberUpdates),
        Action::Member(MemberAction::Update) => {
            // Timeout outranks nick when both are in one entry.
            if changes.iter().any(|c| matches!(c, Change::CommunicationDisabledUntil { .. })) {
                external
            } else if changes.iter().any(|c| matches!(c, Change::Nick { .. })) {
                Some(LogCategory::MemberUpdates)
            } else {
                None
            }
        }
        Action::Channel(ChannelAction::Create | ChannelAction::Update | ChannelAction::Delete)
        | Action::ChannelOverwrite(
            ChannelOverwriteAction::Create | ChannelOverwriteAction::Update | ChannelOverwriteAction::Delete,
        ) => Some(LogCategory::Channels),
        Action::Role(RoleAction::Create | RoleAction::Update | RoleAction::Delete) => Some(LogCategory::Roles),
        _ => None,
    }
}

fn value_text(v: &json::Value) -> String {
    let s = match v {
        json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    truncate(&s, 100)
}

/// `key: old → new` per change; Change serializes as {key, old_value, new_value}.
fn change_lines(changes: &[Change]) -> String {
    let lines: Vec<String> = changes
        .iter()
        .filter_map(|c| json::to_value(c).ok())
        .map(|v| {
            let side = |k: &str| v.get(k).map_or("none".to_string(), value_text);
            format!("{}: {} → {}", v["key"].as_str().unwrap_or("?"), side("old_value"), side("new_value"))
        })
        .collect();
    truncate(&lines.join("\n"), 1024)
}

fn entry_embed(entry: &AuditLogEntry, changes: &[Change]) -> CreateEmbed {
    let mut e = CreateEmbed::new()
        .title(format!("{:?}", entry.action))
        .field("Executor", format!("<@{}>", entry.user_id), true);
    if let Some(t) = entry.target_id {
        e = e.field("Target", format!("<@{0}> ({0})", t.get()), true);
    }
    if let Some(r) = entry.reason.as_deref().filter(|r| !r.is_empty()) {
        e = e.field("Reason", truncate(r, 1024), false);
    }
    if !changes.is_empty() {
        e = e.field("Changes", change_lines(changes), false);
    }
    e
}

pub async fn on_entry(ctx: &Context, data: &Data, guild: GuildId, entry: &AuditLogEntry) {
    let changes = entry.changes.as_deref().unwrap_or_default();
    let by_bot = entry.user_id == ctx.cache.current_user().id;
    if let Some(cat) = categorize(&entry.action, changes, by_bot) {
        send_log(&ctx.http, &data.db, guild, cat, entry_embed(entry, changes)).await;
    }
}

pub async fn on_join(ctx: &Context, data: &Data, m: &Member) {
    let created = m.user.created_at().unix_timestamp();
    let days = (Timestamp::now().unix_timestamp() - created) / 86_400;
    let e = CreateEmbed::new()
        .title("Member joined")
        .field("Member", format!("<@{0}> ({0})", m.user.id), false)
        .field("Account created", format!("<t:{created}:D> ({days} days old)"), false);
    send_log(&ctx.http, &data.db, m.guild_id, LogCategory::JoinsLeaves, e).await;
}

pub async fn on_leave(ctx: &Context, data: &Data, guild: GuildId, user: &User, member: Option<&Member>) {
    let mut e = CreateEmbed::new().title("Member left").field("Member", format!("<@{0}> ({0})", user.id), false);
    if let Some(m) = member {
        let roles: Vec<String> = m.roles.iter().map(|r| format!("<@&{r}>")).collect();
        if !roles.is_empty() {
            e = e.field("Roles", truncate(&roles.join(" "), 1024), false);
        }
    }
    send_log(&ctx.http, &data.db, guild, LogCategory::JoinsLeaves, e).await;
}

pub async fn on_voice(ctx: &Context, data: &Data, old: Option<&VoiceState>, new: &VoiceState) {
    let Some(guild) = new.guild_id else { return };
    let desc = match (old.and_then(|o| o.channel_id), new.channel_id) {
        (a, b) if a == b => return, // mute/deafen/stream toggles
        (None, Some(b)) => format!("joined <#{b}>"),
        (Some(a), None) => format!("left <#{a}>"),
        (Some(a), Some(b)) => format!("moved <#{a}> → <#{b}>"),
        (None, None) => return,
    };
    let e = CreateEmbed::new().title("Voice").description(format!("<@{}> {desc}", new.user_id));
    send_log(&ctx.http, &data.db, guild, LogCategory::Voice, e).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(a: Action, ch: &[Change], bot: bool) -> Option<LogCategory> {
        categorize(&a, ch, bot)
    }
    fn nick() -> Change {
        Change::Nick { old: Some("a".into()), new: Some("b".into()) }
    }
    fn timeout() -> Change {
        Change::CommunicationDisabledUntil { old: None, new: Some(Timestamp::now()) }
    }

    #[test]
    fn categories() {
        use LogCategory::*;
        assert_eq!(cat(Action::Member(MemberAction::Kick), &[], false), Some(ExternalMod));
        assert_eq!(cat(Action::Member(MemberAction::Kick), &[], true), None);
        assert_eq!(cat(Action::Member(MemberAction::BanAdd), &[], false), Some(ExternalMod));
        assert_eq!(cat(Action::Member(MemberAction::BanRemove), &[], true), None);
        assert_eq!(cat(Action::Member(MemberAction::RoleUpdate), &[], true), Some(MemberUpdates));
        assert_eq!(cat(Action::Role(RoleAction::Create), &[], false), Some(Roles));
        assert_eq!(cat(Action::Channel(ChannelAction::Delete), &[], false), Some(Channels));
        assert_eq!(cat(Action::ChannelOverwrite(ChannelOverwriteAction::Update), &[], true), Some(Channels));
        assert_eq!(cat(Action::GuildUpdate, &[], false), None);
    }

    #[test]
    fn member_update() {
        use LogCategory::*;
        let m = || Action::Member(MemberAction::Update);
        assert_eq!(cat(m(), &[timeout()], false), Some(ExternalMod));
        assert_eq!(cat(m(), &[timeout()], true), None);
        assert_eq!(cat(m(), &[nick()], false), Some(MemberUpdates));
        assert_eq!(cat(m(), &[nick(), timeout()], false), Some(ExternalMod));
        assert_eq!(cat(m(), &[], false), None);
    }

    #[test]
    fn change_rendering() {
        let long = Change::Nick { old: Some("x".repeat(300)), new: Some("b".into()) };
        let s = change_lines(&[long, nick()]);
        let first = s.lines().next().unwrap();
        assert!(first.starts_with("nick: x") && first.ends_with("→ b"));
        assert_eq!(first.chars().count(), "nick: ".len() + 100 + " → b".chars().count());
        assert_eq!(s.lines().nth(1).unwrap(), "nick: a → b");
    }
}

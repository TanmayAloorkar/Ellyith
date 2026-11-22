use super::{cache::{CachedMessage, MAX_CONTENT_CHARS}, send_log, LogCategory};
use crate::text::truncate;
use crate::Data;
use poise::serenity_prelude::{CreateEmbed, GuildId, Http, Message, MessageUpdateEvent};

fn link(guild: GuildId, channel: u64, id: u64) -> String {
    format!("https://discord.com/channels/{guild}/{channel}/{id}")
}

fn cached(m: &Message) -> CachedMessage {
    CachedMessage {
        author_id: m.author.id.get(),
        channel_id: m.channel_id.get(),
        content: m.content.clone(),
        attachment_urls: m.attachments.iter().take(10).map(|a| a.url.clone()).collect(),
    }
}

pub fn on_message(data: &Data, m: &Message) {
    if m.guild_id.is_some() && !m.author.bot {
        data.msg_cache.insert(m.id.get(), cached(m));
    }
}

pub async fn on_update(http: &Http, data: &Data, new: Option<&Message>, ev: &MessageUpdateEvent) {
    let Some(guild) = ev.guild_id else { return };
    if ev.author.as_ref().is_some_and(|u| u.bot) || new.is_some_and(|m| m.author.bot) {
        return;
    }
    // None: only embeds changed, not an edit.
    let Some(content) = ev.content.clone() else { return };
    let id = ev.id.get();
    let old = data.msg_cache.get(id);
    if old.as_ref().is_some_and(|o| o.content == truncate(&content, MAX_CONTENT_CHARS)) {
        return;
    }
    let author = old.as_ref().map(|o| o.author_id).or(ev.author.as_ref().map(|u| u.id.get()));
    let before = old.as_ref().map_or("Not cached".to_string(), |o| truncate(&o.content, 1024));
    let mut e = CreateEmbed::new()
        .title("Message edited")
        .field("Before", or_empty(&before), false)
        .field("After", or_empty(&truncate(&content, 1024)), false)
        .field("Channel", format!("<#{}>", ev.channel_id), true)
        .field("Link", link(guild, ev.channel_id.get(), id), false);
    if let Some(a) = author {
        e = e.field("Author", format!("<@{a}>"), true);
    }
    if data.msg_cache.update_content(id, content).is_none() {
        if let Some(m) = new {
            data.msg_cache.insert(id, cached(m));
        }
    }
    send_log(http, &data.db, guild, LogCategory::Messages, e).await;
}

pub async fn on_delete(http: &Http, data: &Data, channel: u64, id: u64, guild_id: Option<GuildId>) {
    let Some(guild) = guild_id else { return };
    let old = data.msg_cache.remove(id);
    let body = match &old {
        Some(o) if !o.content.is_empty() => truncate(&o.content, 1024),
        Some(_) => "(no text content)".to_string(),
        None => "Content not cached".to_string(),
    };
    let mut e = CreateEmbed::new()
        .title("Message deleted")
        .field("Content", body, false)
        .field("Channel", format!("<#{channel}>"), true);
    if let Some(o) = old {
        e = e.field("Author", format!("<@{}>", o.author_id), true);
        if !o.attachment_urls.is_empty() {
            e = e.field("Attachments", truncate(&o.attachment_urls.join("\n"), 1024), false);
        }
    }
    send_log(http, &data.db, guild, LogCategory::Messages, e).await;
}

pub async fn on_bulk_delete(http: &Http, data: &Data, channel: u64, ids: &[u64], guild_id: Option<GuildId>) {
    let Some(guild) = guild_id else { return };
    let found: Vec<CachedMessage> = ids.iter().filter_map(|id| data.msg_cache.remove(*id)).collect();
    send_log(http, &data.db, guild, LogCategory::Messages, bulk_embed(channel, ids.len(), &found)).await;
}

fn bulk_embed(channel: u64, count: usize, found: &[CachedMessage]) -> CreateEmbed {
    let mut e = CreateEmbed::new()
        .title("Bulk delete")
        .field("Count", count.to_string(), true)
        .field("Channel", format!("<#{channel}>"), true);
    if !found.is_empty() {
        let lines: Vec<String> = found
            .iter()
            .take(10)
            .map(|m| format!("<@{}>: {}", m.author_id, truncate(&m.content.replace('\n', " "), 100)))
            .collect();
        e = e.field(format!("Cached messages ({} of {})", lines.len(), found.len()), truncate(&lines.join("\n"), 1024), false);
    }
    e
}

// Discord rejects empty embed field values.
fn or_empty(s: &str) -> String {
    if s.is_empty() { "(empty)".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_embed_caps_lines() {
        let m = |c: &str| CachedMessage { author_id: 5, channel_id: 1, content: c.into(), attachment_urls: vec![] };
        let found: Vec<_> = (0..15).map(|_| m(&"y".repeat(500))).collect();
        let v = serde_json::to_value(bulk_embed(1, 20, &found)).unwrap();
        let val = v["fields"][2]["value"].as_str().unwrap();
        assert_eq!(val.lines().count(), 10);
        assert!(val.chars().count() <= 1024);
        assert!(v["fields"][2]["name"].as_str().unwrap().contains("10 of 15"));
    }
}

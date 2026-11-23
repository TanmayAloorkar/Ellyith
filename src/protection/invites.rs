//! Invite use tracking (which invite a join used) and ban records for alt detection.
use super::logic::invite_diff;
use super::store;
use crate::Data;
use poise::serenity_prelude::{self as serenity, GuildId};
use std::collections::HashMap;
use std::sync::Mutex;

type Entry = (u64, Option<u64>); // (uses, inviter)

/// One entry per invite, per guild. Only the home guild is ever loaded.
#[derive(Default)]
pub struct InviteCache(Mutex<HashMap<u64, HashMap<String, Entry>>>);

impl InviteCache {
    fn counts(map: &HashMap<String, Entry>) -> HashMap<String, u64> {
        map.iter().map(|(c, e)| (c.clone(), e.0)).collect()
    }

    async fn fetch(ctx: &serenity::Context, guild: GuildId) -> Option<HashMap<String, Entry>> {
        match guild.invites(&ctx.http).await {
            Ok(v) => Some(v.into_iter().map(|i| (i.code, (i.uses, i.inviter.map(|u| u.id.get())))).collect()),
            Err(e) => {
                tracing::warn!("invite fetch failed: {e}");
                None
            }
        }
    }

    pub async fn load(&self, ctx: &serenity::Context, guild: GuildId) {
        if let Some(m) = Self::fetch(ctx, guild).await {
            self.0.lock().unwrap().insert(guild.get(), m);
        }
    }

    /// The invite a fresh join used, with its inviter. None when unclear or the fetch fails.
    pub async fn resolve_join(&self, ctx: &serenity::Context, guild: GuildId) -> Option<(String, Option<u64>)> {
        let after = Self::fetch(ctx, guild).await?;
        let mut all = self.0.lock().unwrap();
        let before = all.get(&guild.get()).map(Self::counts).unwrap_or_default();
        let code = invite_diff(&before, &Self::counts(&after));
        let inviter = code.as_ref().and_then(|c| after.get(c).and_then(|e| e.1));
        all.insert(guild.get(), after);
        code.map(|c| (c, inviter))
    }

    fn add(&self, guild: GuildId, code: String, inviter: Option<u64>) {
        self.0.lock().unwrap().entry(guild.get()).or_default().insert(code, (0, inviter));
    }

    fn remove(&self, guild: GuildId, code: &str) {
        if let Some(m) = self.0.lock().unwrap().get_mut(&guild.get()) {
            m.remove(code);
        }
    }
}

pub fn on_invite_create(data: &Data, ev: &serenity::InviteCreateEvent) {
    if let Some(g) = ev.guild_id {
        data.protection.invites.add(g, ev.code.clone(), ev.inviter.as_ref().map(|u| u.id.get()));
    }
}

pub fn on_invite_delete(data: &Data, ev: &serenity::InviteDeleteEvent) {
    if let Some(g) = ev.guild_id {
        data.protection.invites.remove(g, &ev.code);
    }
}

/// Remembers who was banned and which invite they joined with, so later joins can be compared.
pub async fn on_ban(_ctx: &serenity::Context, data: &Data, guild: GuildId, user: &serenity::User) {
    let g = guild.get() as i64;
    let code = match store::member_invite(&data.db, g, user.id.get() as i64).await {
        Ok(v) => v.and_then(|(c, _)| c),
        Err(e) => {
            tracing::warn!("ban record: invite lookup failed: {e}");
            None
        }
    };
    let display = user.global_name.as_deref().unwrap_or(&user.name);
    if let Err(e) = store::record_ban(&data.db, g, user.id.get() as i64, &user.name, display, code.as_deref()).await {
        tracing::warn!("ban record failed for {}: {e}", user.id);
    }
}

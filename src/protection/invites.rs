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
pub struct InviteCache {
    map: Mutex<HashMap<u64, HashMap<String, Entry>>>,
    /// Held across fetch + diff + store so concurrent joins never apply a stale snapshot.
    resolving: tokio::sync::Mutex<()>,
}

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
            self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(guild.get(), m);
        }
    }

    /// The invite a fresh join used, with its inviter. None when unclear or the fetch fails.
    pub async fn resolve_join(&self, ctx: &serenity::Context, guild: GuildId) -> Option<(String, Option<u64>)> {
        let _one_at_a_time = self.resolving.lock().await;
        let after = Self::fetch(ctx, guild).await?;
        self.apply(guild, after)
    }

    /// Diff against the cached counts and store `after`. An unloaded guild yields None but is stored.
    fn apply(&self, guild: GuildId, after: HashMap<String, Entry>) -> Option<(String, Option<u64>)> {
        let mut all = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let code = all.get(&guild.get()).and_then(|b| invite_diff(&Self::counts(b), &Self::counts(&after)));
        let inviter = code.as_ref().and_then(|c| after.get(c).and_then(|e| e.1));
        all.insert(guild.get(), after);
        code.map(|c| (c, inviter))
    }

    fn add(&self, guild: GuildId, code: String, inviter: Option<u64>) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).entry(guild.get()).or_default().insert(code, (0, inviter));
    }

    fn remove(&self, guild: GuildId, code: &str) {
        if let Some(m) = self.map.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&guild.get()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn m(v: &[(&str, u64)]) -> HashMap<String, Entry> {
        v.iter().map(|(c, n)| (c.to_string(), (*n, Some(7)))).collect()
    }

    #[test]
    fn apply_diffs_and_stores() {
        let c = InviteCache::default();
        let g = GuildId::new(1);
        assert_eq!(c.apply(g, m(&[("a", 1)])), None); // unloaded: no guess, but stored
        assert_eq!(c.apply(g, m(&[("a", 2)])), Some(("a".into(), Some(7))));
        assert_eq!(c.apply(g, m(&[("a", 2)])), None);
    }
}

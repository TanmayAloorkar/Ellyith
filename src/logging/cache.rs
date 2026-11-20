use crate::text::truncate;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_CONTENT_CHARS: usize = 4000;
const MAX_ATTACHMENTS: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct CachedMessage {
    pub author_id: u64,
    pub channel_id: u64,
    pub content: String,
    pub attachment_urls: Vec<String>,
}

type Inner = (HashMap<u64, (Instant, CachedMessage)>, VecDeque<u64>);

/// Bounded in-memory message store (oldest-inserted evicted first, entries expire after `ttl`).
/// Message content lives only here, never in the database.
pub struct MessageCache {
    cap: usize,
    ttl: Duration,
    inner: Mutex<Inner>,
}

impl MessageCache {
    pub fn new(cap: usize, ttl: Duration) -> Self {
        Self { cap, ttl, inner: Mutex::new((HashMap::new(), VecDeque::new())) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn insert(&self, id: u64, m: CachedMessage) {
        self.insert_at(id, m, Instant::now())
    }

    pub fn insert_at(&self, id: u64, mut m: CachedMessage, now: Instant) {
        m.content = truncate(&m.content, MAX_CONTENT_CHARS);
        m.attachment_urls.truncate(MAX_ATTACHMENTS);
        let (map, order) = &mut *self.lock();
        if map.insert(id, (now, m)).is_none() {
            order.push_back(id);
        }
        while map.len() > self.cap {
            match order.pop_front() {
                Some(old) => {
                    map.remove(&old);
                }
                None => break,
            }
        }
    }

    pub fn get(&self, id: u64) -> Option<CachedMessage> {
        self.get_at(id, Instant::now())
    }

    pub fn get_at(&self, id: u64, now: Instant) -> Option<CachedMessage> {
        let g = self.lock();
        g.0.get(&id).filter(|(t, _)| self.fresh(*t, now)).map(|(_, m)| m.clone())
    }

    /// Replace the content of a live entry; returns the old content.
    pub fn update_content(&self, id: u64, content: String) -> Option<String> {
        let now = Instant::now();
        let mut g = self.lock();
        let (t, m) = g.0.get_mut(&id)?;
        if !self.fresh(*t, now) {
            return None;
        }
        Some(std::mem::replace(&mut m.content, truncate(&content, MAX_CONTENT_CHARS)))
    }

    pub fn remove(&self, id: u64) -> Option<CachedMessage> {
        self.remove_at(id, Instant::now())
    }

    pub fn remove_at(&self, id: u64, now: Instant) -> Option<CachedMessage> {
        let (map, order) = &mut *self.lock();
        let (t, m) = map.remove(&id)?;
        // Keep the queue in step with the map so it cannot outgrow `cap`.
        if let Some(i) = order.iter().position(|x| *x == id) {
            order.remove(i);
        }
        self.fresh(t, now).then_some(m)
    }

    fn fresh(&self, inserted: Instant, now: Instant) -> bool {
        now.saturating_duration_since(inserted) <= self.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(c: &str) -> CachedMessage {
        CachedMessage { author_id: 1, channel_id: 2, content: c.into(), attachment_urls: vec![] }
    }

    #[test]
    fn evicts_expires_updates_removes() {
        let c = MessageCache::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        c.insert_at(1, msg("a"), t0);
        c.insert_at(2, msg("b"), t0);
        c.insert_at(3, msg("c"), t0);
        assert!(c.get_at(1, t0).is_none());
        assert_eq!(c.get_at(3, t0).unwrap().content, "c");
        assert!(c.get_at(3, t0 + Duration::from_secs(61)).is_none());
        assert_eq!(c.update_content(2, "B".into()), Some("b".into()));
        assert_eq!(c.remove(2).unwrap().content, "B");
        assert!(c.remove(2).is_none());
    }

    #[test]
    fn bounds_entry_size_and_queue() {
        let c = MessageCache::new(2, Duration::from_secs(60));
        let mut m = msg(&"x".repeat(5000));
        m.attachment_urls = vec!["u".into(); 30];
        c.insert(1, m);
        let got = c.get(1).unwrap();
        assert_eq!(got.content.chars().count(), 4000);
        assert_eq!(got.attachment_urls.len(), 10);
        c.remove(1);
        // insert+remove churn must not leak stale ids in the queue
        for i in 0..50 {
            c.insert(100 + i, msg("a"));
            c.remove(100 + i);
        }
        c.insert(7, msg("a"));
        c.insert(8, msg("b"));
        assert_eq!(c.lock().1.len(), 2);
    }
}

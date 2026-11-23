//! Per-member sliding windows for multi-message rules. All time is injected.
//! One entry list per (guild, user); each entry carries a "consumed" flag per rule,
//! so a trigger clears that rule's entries only.
use std::{collections::HashMap, sync::Mutex, time::{Duration, Instant}};

use super::normalize::is_repeat;

pub struct Seen { pub channel: u64, pub message: u64, pub at: Instant, pub folded: String, pub attachments: usize }

#[derive(Clone, Copy)]
enum Rule { Fast = 0, Repeat = 1, Files = 2 }

struct Entry { seen: Seen, used: [bool; 3] }

pub struct Windows { cap_members: usize, max_window: Duration, members: Mutex<HashMap<(u64, u64), Vec<Entry>>> }

const MAX_ENTRIES: usize = 50;

type Ids = Option<Vec<(u64, u64)>>;

impl Windows {
    pub fn new(cap_members: usize, max_window: Duration) -> Self { Self { cap_members, max_window, members: Mutex::new(HashMap::new()) } }

    pub fn push(&self, guild: u64, user: u64, seen: Seen) {
        let mut m = self.members.lock().unwrap_or_else(|e| e.into_inner());
        if !m.contains_key(&(guild, user)) && m.len() >= self.cap_members {
            let oldest = m.iter().min_by_key(|(_, v)| v.last().map(|e| e.seen.at)).map(|(k, _)| *k);
            if let Some(k) = oldest { m.remove(&k); }
        }
        let list = m.entry((guild, user)).or_default();
        let (now, max) = (seen.at, self.max_window);
        list.retain(|e| now.saturating_duration_since(e.seen.at) <= max);
        list.push(Entry { seen, used: [false; 3] });
        if list.len() > MAX_ENTRIES { list.remove(0); }
    }

    pub fn check_fast(&self, guild: u64, user: u64, threshold: usize, window: Duration, now: Instant) -> Ids {
        self.check(guild, user, Rule::Fast, threshold.max(2), window, now, |_, _| true, |_| 1)
    }

    pub fn check_repeat(&self, guild: u64, user: u64, threshold: usize, window: Duration, percent: u8, now: Instant) -> Ids {
        if self.newest_is_short(guild, user) { return None; }
        self.check(guild, user, Rule::Repeat, threshold.max(2), window, now,
            |newest, e| is_repeat(&newest.folded, &e.folded, percent), |_| 1)
    }

    pub fn check_files(&self, guild: u64, user: u64, threshold: usize, window: Duration, now: Instant) -> Ids {
        self.check(guild, user, Rule::Files, threshold, window, now, |_, e| e.attachments > 0, |e| e.attachments)
    }

    fn newest_is_short(&self, guild: u64, user: u64) -> bool {
        let m = self.members.lock().unwrap_or_else(|e| e.into_inner());
        m.get(&(guild, user)).and_then(|v| v.last()).is_none_or(|e| e.seen.folded.chars().count() < 5)
    }

    /// Counted entries = unconsumed, in window, matching `keep` (against the newest unconsumed one).
    #[allow(clippy::too_many_arguments)]
    fn check(&self, guild: u64, user: u64, rule: Rule, threshold: usize, window: Duration, now: Instant,
             keep: impl Fn(&Seen, &Seen) -> bool, weight: impl Fn(&Seen) -> usize) -> Ids {
        let mut m = self.members.lock().unwrap_or_else(|e| e.into_inner());
        let list = m.get_mut(&(guild, user))?;
        let r = rule as usize;
        let live = |e: &Entry| !e.used[r] && now.saturating_duration_since(e.seen.at) <= window;
        let newest_idx = list.iter().rposition(&live)?;
        let picked: Vec<usize> = (0..list.len())
            .filter(|&i| live(&list[i]) && ((i == newest_idx && !matches!(rule, Rule::Files)) || keep(&list[newest_idx].seen, &list[i].seen)))
            .collect();
        if picked.iter().map(|&i| weight(&list[i].seen)).sum::<usize>() < threshold { return None; }
        for &i in &picked { list[i].used[r] = true; }
        Some(picked.iter().map(|&i| (list[i].seen.channel, list[i].seen.message)).collect())
    }

    /// Drop entries older than `max_window` and members left empty.
    pub fn prune(&self, now: Instant, max_window: Duration) {
        let mut m = self.members.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, v| {
            v.retain(|e| now.saturating_duration_since(e.seen.at) <= max_window && !e.used.iter().all(|u| *u));
            !v.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Duration = Duration::from_secs(30);
    fn seen(t0: Instant, secs: u64, msg: u64, text: &str, files: usize) -> Seen {
        Seen { channel: 1, message: msg, at: t0 + Duration::from_secs(secs), folded: super::super::normalize::fold(text), attachments: files }
    }
    fn at(t0: Instant, secs: u64) -> Instant { t0 + Duration::from_secs(secs) }

    #[test]
    fn fast_triggers_at_threshold() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        for i in 0..4 { w.push(1, 2, seen(t, i, i, "x", 0)); }
        assert!(w.check_fast(1, 2, 5, W, at(t, 4)).is_none());
        w.push(1, 2, seen(t, 4, 4, "x", 0));
        assert_eq!(w.check_fast(1, 2, 5, W, at(t, 4)).unwrap().len(), 5);
    }

    #[test]
    fn repeat_counts_only_similar() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        w.push(1, 2, seen(t, 0, 0, "FREE NITRO here!!", 0));
        w.push(1, 2, seen(t, 1, 1, "something else entirely", 0));
        w.push(1, 2, seen(t, 2, 2, "free  n1tro HERE", 0));
        w.push(1, 2, seen(t, 3, 3, "Free Nitro here!!!", 0));
        let ids = w.check_repeat(1, 2, 3, W, 80, at(t, 3)).unwrap();
        assert_eq!(ids.iter().map(|x| x.1).collect::<Vec<_>>(), vec![0, 2, 3]);
        w.push(1, 2, seen(t, 4, 4, "ok", 0));
        assert!(w.check_repeat(1, 2, 2, W, 80, at(t, 4)).is_none()); // short never repeats
        assert!(w.check_repeat(1, 2, 1, W, 80, at(t, 4)).is_none());
    }

    #[test]
    fn files_sums_attachments() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        w.push(1, 2, seen(t, 0, 0, "a", 2));
        w.push(1, 2, seen(t, 1, 1, "b", 0));
        w.push(1, 2, seen(t, 2, 2, "c", 3));
        assert!(w.check_files(1, 2, 6, W, at(t, 2)).is_none());
        assert_eq!(w.check_files(1, 2, 5, W, at(t, 2)).unwrap(), vec![(1, 0), (1, 2)]);
    }

    #[test]
    fn window_expires() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        w.push(1, 2, seen(t, 0, 0, "x", 0));
        w.push(1, 2, seen(t, 40, 1, "x", 0));
        assert!(w.check_fast(1, 2, 2, W, at(t, 40)).is_none());
        w.prune(at(t, 40), W);
        assert_eq!(w.members.lock().unwrap_or_else(|e| e.into_inner())[&(1, 2)].len(), 1);
        w.prune(at(t, 100), W);
        assert!(w.members.lock().unwrap_or_else(|e| e.into_inner()).is_empty());
    }

    #[test]
    fn trigger_clears_member() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        for i in 0..3 { w.push(1, 2, seen(t, i, i, "x", 1)); }
        assert!(w.check_fast(1, 2, 3, W, at(t, 3)).is_some());
        assert!(w.check_fast(1, 2, 3, W, at(t, 3)).is_none());
        assert!(w.check_files(1, 2, 3, W, at(t, 3)).is_some()); // other rule unaffected
    }

    #[test]
    fn cap_drops_oldest() {
        let (w, t) = (Windows::new(2, W), Instant::now());
        w.push(1, 1, seen(t, 0, 0, "x", 0));
        w.push(1, 2, seen(t, 5, 1, "x", 0));
        w.push(1, 3, seen(t, 6, 2, "x", 0));
        let m = w.members.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!m.contains_key(&(1, 1)) && m.contains_key(&(1, 2)) && m.contains_key(&(1, 3)));
    }

    #[test]
    fn per_member_list_is_bounded() {
        let (w, t) = (Windows::new(10, Duration::from_secs(1000)), Instant::now());
        for i in 0..200 { w.push(1, 2, seen(t, i, i, "x", 0)); }
        assert!(w.members.lock().unwrap()[&(1, 2)].len() <= 50);
        w.push(1, 2, seen(t, 5000, 999, "x", 0));
        assert_eq!(w.members.lock().unwrap()[&(1, 2)].len(), 1);
    }

    #[test]
    fn threshold_below_two_is_two() {
        let (w, t) = (Windows::new(10, W), Instant::now());
        w.push(1, 2, seen(t, 0, 0, "x", 0));
        assert!(w.check_fast(1, 2, 1, W, at(t, 0)).is_none());
    }
}

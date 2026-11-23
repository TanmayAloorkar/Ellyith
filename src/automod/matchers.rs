//! Pure message matchers: banned words, counts, links. No Discord types.
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use super::normalize::{fold, lookalike};
use super::rules::{WordEntry, WordKind};
use crate::UserError;

pub fn compile_regex(p: &str) -> Result<Regex, UserError> {
    let bad = || UserError("That pattern is invalid or too large.".into());
    if p.chars().count() > 200 { return Err(bad()); }
    RegexBuilder::new(p).size_limit(262_144).case_insensitive(true).build().map_err(|_| bad())
}

/// A whole-word entry: one item per space-separated word, look-alikes mapped, letters not collapsed.
enum Compiled { Whole(String, Vec<Vec<char>>), Wildcard(String, Regex), Regex(String, Regex) }

pub struct CompiledWords(Vec<Compiled>);

fn is_zero_width(c: char) -> bool { matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}') }

fn mapped(w: &str) -> Vec<char> {
    w.chars().flat_map(char::to_lowercase).map(lookalike).filter(|c| c.is_alphanumeric()).collect()
}

/// `tok` is `pat` with each letter repeated at least as often ("asss" matches "ass", "as" does not).
fn runs_match(tok: &[char], pat: &[char]) -> bool {
    fn runs(s: &[char]) -> Vec<(char, usize)> {
        let mut v: Vec<(char, usize)> = Vec::new();
        for &c in s {
            match v.last_mut() { Some((l, n)) if *l == c => *n += 1, _ => v.push((c, 1)) }
        }
        v
    }
    let (t, p) = (runs(tok), runs(pat));
    !p.is_empty() && t.len() == p.len() && t.iter().zip(&p).all(|(a, b)| a.0 == b.0 && a.1 >= b.1)
}

/// Mapped word tokens (zero-width chars, combining marks and apostrophes dropped first).
fn tokens(content: &str) -> Vec<Vec<char>> {
    let clean: String = content
        .chars()
        .filter(|&c| !is_zero_width(c) && !is_mark(c) && !matches!(c, '\'' | '\u{2019}'))
        .collect();
    clean
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '@' | '$' | '|')))
        .map(mapped)
        .filter(|w| !w.is_empty())
        .collect()
}

/// Longest joined run of single-character tokens we try ("b a d" -> "bad").
const MAX_JOIN: usize = 30;

/// Extra candidates: every contiguous window (2+) inside a run of single-character tokens.
fn joined_windows(toks: &[Vec<char>]) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].len() != 1 { i += 1; continue; }
        let mut j = i;
        while j < toks.len() && toks[j].len() == 1 { j += 1; }
        for s in i..j {
            for e in (s + 2)..=j.min(s + MAX_JOIN) { out.push(toks[s..e].iter().flatten().copied().collect()); }
        }
        i = j;
    }
    out
}

impl CompiledWords {
    /// Entries whose regex no longer compiles are skipped (they are validated on save).
    pub fn new(entries: &[WordEntry]) -> Self {
        let v = entries.iter().filter_map(|e| match e.kind {
            WordKind::Whole => {
                let items: Vec<Vec<char>> = e.pattern.split_whitespace().map(mapped).filter(|w| !w.is_empty()).collect();
                (!items.is_empty()).then(|| Compiled::Whole(e.pattern.clone(), items))
            }
            WordKind::Wildcard => {
                let body = e.pattern.split('*').map(|p| regex::escape(&fold(p))).collect::<Vec<_>>().join(".*");
                Regex::new(&format!("^{body}$")).ok().map(|r| Compiled::Wildcard(e.pattern.clone(), r))
            }
            WordKind::Regex => compile_regex(&e.pattern).ok().map(|r| Compiled::Regex(e.pattern.clone(), r)),
        }).collect();
        Self(v)
    }

    /// The first entry (as written) that `content` hits.
    pub fn hit(&self, content: &str) -> Option<String> {
        let folded = fold(content);
        let toks = tokens(content);
        let joined = joined_windows(&toks);
        self.0.iter().find_map(|c| match c {
            Compiled::Whole(p, items) => {
                let found = if items.len() == 1 {
                    toks.iter().chain(&joined).any(|t| runs_match(t, &items[0]))
                } else {
                    toks.windows(items.len()).any(|w| w.iter().zip(items).all(|(t, i)| runs_match(t, i)))
                };
                found.then(|| p.clone())
            }
            Compiled::Wildcard(p, r) => r.is_match(&folded).then(|| p.clone()),
            Compiled::Regex(p, r) => r.is_match(content).then(|| p.clone()),
        })
    }
}

pub fn mention_count(user_mentions: usize, role_mentions: usize, content: &str) -> usize {
    let text = ["@everyone", "@here"].iter().flat_map(|m| content.match_indices(m)).filter(|(i, _)| {
        !content[..*i].chars().last().is_some_and(char::is_alphanumeric)
    }).count();
    user_mentions + role_mentions + text
}

static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:\w+:\d+>").unwrap());
static UNICODE_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Extended_Pictographic}").unwrap());

pub fn emoji_count(content: &str) -> usize {
    CUSTOM_EMOJI.find_iter(content).count() + UNICODE_EMOJI.find_iter(&CUSTOM_EMOJI.replace_all(content, "")).count()
}

/// Share of letters that are uppercase; None under 10 letters.
pub fn caps_percent(content: &str) -> Option<u8> {
    let letters = content.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 10 { return None; }
    Some((content.chars().filter(|c| c.is_uppercase()).count() * 100 / letters) as u8)
}

pub fn line_count(content: &str) -> usize { content.lines().count() }

pub fn spoiler_count(content: &str) -> usize { content.matches("||").count() / 2 }

fn is_mark(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{1ab0}'..='\u{1aff}' | '\u{1dc0}'..='\u{1dff}' | '\u{20d0}'..='\u{20ff}' | '\u{fe20}'..='\u{fe2f}')
}

/// Zalgo if 4+ combining marks stack on one character, or marks are over 30% of a 10+ char message.
/// Real accents stack 1-2 marks, so these thresholds spare normal text.
pub fn zalgo(content: &str) -> bool {
    let (mut run, mut marks, mut total) = (0, 0, 0);
    for c in content.chars() {
        total += 1;
        if is_mark(c) {
            marks += 1;
            run += 1;
            if run >= 4 { return true; }
        } else {
            run = 0;
        }
    }
    total >= 10 && marks * 100 / total > 30
}

static HOST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(https?://)?((?:[a-z0-9-]+\.)+([a-z]{2,}))\b").unwrap());
static INVITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:discord\.gg|discord(?:app)?\.com/invite)/([a-z0-9-]+)").unwrap());

const TLDS: &[&str] = &[
    "com", "net", "org", "io", "gg", "co", "me", "xyz", "ru", "tk", "ly", "app", "dev", "link", "site", "online",
    "shop", "club", "top", "info", "biz", "us", "uk", "tv", "gift", "ml", "ga", "cf", "gq", "cc", "to", "live",
    "fun", "store", "tech", "pw", "su", "in", "de", "fr", "br", "eu",
];

/// Hosts of scheme URLs (any TLD) and bare `domain.tld` (common TLDs only), lowercase, no `www.`.
pub fn extract_hosts(content: &str) -> Vec<String> {
    HOST.captures_iter(content)
        .filter(|c| c.get(1).is_some() || TLDS.contains(&c[3].to_lowercase().as_str()))
        .map(|c| {
            let h = c[2].to_lowercase();
            h.strip_prefix("www.").map_or(h.clone(), str::to_string)
        })
        .collect()
}

/// Equal to an allowed domain or a subdomain of it.
pub fn site_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|a| {
        let a = a.to_lowercase();
        host == a || host.ends_with(&format!(".{a}"))
    })
}

pub fn invite_codes(content: &str) -> Vec<String> {
    INVITE.captures_iter(content).map(|c| c[1].to_string()).collect()
}

/// The host or any parent domain is in the set.
pub fn host_in_set(host: &str, set: &HashSet<String>) -> bool {
    let mut h = host;
    loop {
        if set.contains(h) { return true; }
        match h.split_once('.') { Some((_, rest)) if rest.contains('.') => h = rest, _ => return false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(v: &[&str]) -> CompiledWords {
        CompiledWords::new(&v.iter().map(|s| super::super::rules::parse_word_line(s).unwrap()).collect::<Vec<_>>())
    }

    #[test]
    fn words_whole_wildcard_regex() {
        let w = words(&["bad"]);
        for t in ["so b4d!", "b a d", "baaad", "BAD"] { assert!(w.hit(t).is_some(), "{t}"); }
        assert!(w.hit("badge").is_none());
        assert!(w.hit("bad badge").is_some());
        assert_eq!(words(&["*scam*"]).hit("megascammer").as_deref(), Some("*scam*"));
        assert!(words(&["/d[i1]sc[o0]rd\\.gift/"]).hit("Discord.gift/x").is_some());
    }

    #[test]
    fn word_edge_cases() {
        let no = |w: &str, t: &str| assert!(words(&[w]).hit(t).is_none(), "{w} vs {t}");
        let yes = |w: &str, t: &str| assert!(words(&[w]).hit(t).is_some(), "{w} vs {t}");
        no("ass", "as"); yes("ass", "asss"); yes("ass", "a$$");
        no("butt", "but"); no("hell", "hel"); yes("hell", "heeell"); no("boob", "bob");
        yes("bad", "b\u{200b}ad"); yes("bad", "a b a d"); yes("bad", "u b a d");
        yes("im", "I'm"); no("ma", "I'm a");
        yes("free nitro", "FREE n1tro now"); no("free nitro", "freenitro");
    }

    #[test]
    fn regex_ci_and_mentions() {
        assert!(words(&["/discord\\.gift/"]).hit("DISCORD.GIFT/x").is_some());
        assert_eq!(mention_count(0, 0, "email@here.com @here"), 1);
    }

    #[test]
    fn host_filtering() {
        for t in ["node.js", "file.txt", "e.g", "ok.Thanks"] { assert!(extract_hosts(t).is_empty(), "{t}"); }
        assert_eq!(extract_hosts("discord.gg/abc"), ["discord.gg"]);
        assert_eq!(extract_hosts("example.com/x"), ["example.com"]);
        assert_eq!(extract_hosts("free-nitro.gift"), ["free-nitro.gift"]);
        assert_eq!(extract_hosts("http://a.thing/x"), ["a.thing"]);
    }

    #[test]
    fn regex_limits() {
        assert!(compile_regex("(a+)+$").is_ok());
        assert!(compile_regex(&"a".repeat(201)).is_err());
        assert!(compile_regex("(a{1000}){1000}").is_err());
        assert!(compile_regex(r"\w+").is_ok() && compile_regex(r"\d{3,}").is_ok());
        assert_eq!(compile_regex("(").unwrap_err().0, "That pattern is invalid or too large.");
    }

    #[test]
    fn mention_and_emoji_counts() {
        assert_eq!(mention_count(2, 1, "@everyone and @here"), 5);
        assert_eq!(emoji_count("hi <:pog:123> <a:dance:456> \u{1F600}\u{1F389}"), 4);
        assert_eq!(emoji_count("plain 123"), 0);
    }

    #[test]
    fn caps_rules() {
        assert_eq!(caps_percent("HELLO THERE FRIEND"), Some(100));
        assert_eq!(caps_percent("HI"), None);
        assert_eq!(caps_percent("hello There friend"), Some(6));
    }

    #[test]
    fn wall_spoiler_zalgo() {
        assert_eq!(line_count("a\nb\nc"), 3);
        assert_eq!(spoiler_count("||a|| ||b|| ||"), 2);
        assert!(zalgo("h\u{300}\u{301}\u{302}\u{303}i"));
        assert!(!zalgo("caf\u{e9} cre\u{300}me"));
    }

    #[test]
    fn allowed_site_matching() {
        let allowed = vec!["youtube.com".to_string()];
        assert!(site_allowed("sub.youtube.com", &allowed));
        assert!(site_allowed("youtube.com", &allowed));
        assert!(!site_allowed("youtube.com.evil.io", &allowed));
        assert!(!site_allowed("notyoutube.com", &allowed));
        assert_eq!(extract_hosts("see example.com/path and https://WWW.Foo.org/x"), ["example.com", "foo.org"]);
    }

    #[test]
    fn invite_codes_found() {
        assert_eq!(invite_codes("join discord.gg/abc123 or https://discord.com/invite/XyZ"), ["abc123", "XyZ"]);
        assert!(invite_codes("nothing here").is_empty());
    }

    #[test]
    fn scam_host_parent_match() {
        let set: HashSet<String> = ["evil.com".to_string()].into();
        assert!(host_in_set("evil.com", &set));
        assert!(host_in_set("a.b.evil.com", &set));
        assert!(!host_in_set("com", &set));
        assert!(!host_in_set("good.org", &set));
    }
}

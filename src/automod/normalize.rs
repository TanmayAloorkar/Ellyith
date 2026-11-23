//! Message folding and similarity for repeat detection and banned-word disguises.
use std::sync::LazyLock;

use regex::Regex;

/// Lowercase, map look-alikes (`1`/`|`→i, 0→o, 3→e, 4→a, 5→s, 7→t, @→a, $→s; `l` stays),
/// drop everything that is not a letter/digit (spaces, punctuation, zero-width), collapse runs.
/// Look-alike map used by `fold` and the banned-word matcher.
pub fn lookalike(c: char) -> char {
    match c {
        '0' => 'o', '1' | '|' => 'i', '3' => 'e', '4' | '@' => 'a', '5' | '$' => 's', '7' => 't',
        c => c,
    }
}

pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = lookalike(c);
        if !c.is_alphanumeric() { continue; }
        if !out.ends_with(c) { out.push(c); }
    }
    out
}

/// Folded text longer than this is cut before similarity, so Levenshtein stays cheap.
const MAX_FOLDED: usize = 500;

static EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").unwrap());
static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>|<#\d+>").unwrap());

/// `fold` for the repeat window: custom emoji become their name, mentions are dropped
/// (their IDs are not text), capped at 500 chars.
pub fn fold_for_repeat(s: &str) -> String {
    let s = MENTION.replace_all(s, " ");
    fold(&EMOJI.replace_all(&s, " $1 ")).chars().take(MAX_FOLDED).collect()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 0-100 (100 = identical) normalised Levenshtein of the folded strings.
pub fn similarity(a: &str, b: &str) -> u8 {
    let (a, b): (Vec<char>, Vec<char>) = (fold(a).chars().take(MAX_FOLDED).collect(), fold(b).chars().take(MAX_FOLDED).collect());
    let max = a.len().max(b.len());
    if max == 0 { return 100; }
    (100 - levenshtein(&a, &b) * 100 / max) as u8
}

/// Repeat if similarity >= `percent`; folded strings under 5 chars never repeat.
pub fn is_repeat(a: &str, b: &str, percent: u8) -> bool {
    fold(a).chars().count() >= 5 && fold(b).chars().count() >= 5 && similarity(a, b) >= percent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_examples() {
        assert_eq!(fold("FREE  n1tro HERE!!"), fold("free nitro here"));
        assert_eq!(fold("b\u{200b}4d"), "bad");
    }

    #[test]
    fn variations_are_similar() {
        let a = "FREE NITRO here!!";
        assert!(similarity(a, "free  n1tro HERE") >= 80);
        assert!(is_repeat(a, "Free Nitro here!!!", 80));
        assert!(!is_repeat(a, "see you at the game tonight", 80));
    }

    #[test]
    fn short_messages_never_repeat() {
        assert!(!is_repeat("ok", "ok", 80));
        assert!(!is_repeat("lol", "LOL!", 80));
    }

    #[test]
    fn repeat_fold_drops_emoji_ids_and_mentions() {
        let f = fold_for_repeat("<:pepe:123456789012345678>");
        assert_eq!(f, "pepe");
        assert!(!is_repeat(&f, &f, 80)); // three of these never add up to a repeat
        assert_eq!(fold_for_repeat("<@123456789012345678> <@&5> <#6> hello there"), "helothere");
        assert_eq!(fold_for_repeat("<a:blobdance:99> hi"), "blobdancehi");
        assert_eq!(fold_for_repeat(&"ab".repeat(1000)).chars().count(), 500);
    }

    #[test]
    fn similarity_is_capped() {
        let long = "abcdefghij".repeat(1000);
        assert_eq!(similarity(&long, &long), 100);
    }
}

/// Cut `s` to at most `max_chars` chars, ending in `…` when shortened.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    s.chars().take(max_chars - 1).chain(std::iter::once('…')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn short_is_unchanged() { assert_eq!(truncate("abc", 5), "abc"); }
    #[test] fn cuts_with_ellipsis() { assert_eq!(truncate("abcdef", 4), "abc…"); }
    #[test] fn multibyte_never_panics() { assert_eq!(truncate("ééééé", 3), "éé…"); }
}

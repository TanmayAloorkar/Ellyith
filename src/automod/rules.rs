use std::str::FromStr;

macro_rules! rule_kinds {
    ($($v:ident => $s:literal, $label:literal, $emoji:literal, $group:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum RuleKind { $($v),* }
        impl RuleKind {
            pub const ALL: [RuleKind; 13] = [$(Self::$v),*];
            pub fn as_str(self) -> &'static str { match self { $(Self::$v => $s),* } }
            pub fn label(self) -> &'static str { match self { $(Self::$v => $label),* } }
            pub fn emoji(self) -> &'static str { match self { $(Self::$v => $emoji),* } }
            pub fn group(self) -> &'static str { match self { $(Self::$v => $group),* } }
        }
        impl FromStr for RuleKind {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s { $($s => Ok(Self::$v),)* _ => Err(format!("unknown rule {s}")) }
            }
        }
    };
}

rule_kinds! {
    BannedWords => "words", "Banned words", "🚫", "Words";
    FastMessages => "fast", "Fast messages", "⚡", "Spam";
    RepeatedMessages => "repeat", "Repeated messages", "🔁", "Spam";
    MassMentions => "mentions", "Mass mentions", "📣", "Spam";
    EmojiSpam => "emoji", "Emoji spam", "😀", "Spam";
    AllCaps => "caps", "All caps", "🔠", "Message shape";
    FileSpam => "files", "File spam", "📎", "Spam";
    Links => "links", "Links", "🔗", "Links";
    Invites => "invites", "Server invites", "📨", "Links";
    ScamLinks => "scam", "Scam links", "🎣", "Links";
    WallsOfText => "wall", "Walls of text", "🧱", "Message shape";
    SpoilerSpam => "spoilers", "Spoiler spam", "🙈", "Message shape";
    Zalgo => "zalgo", "Zalgo text", "👾", "Message shape";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction { Delete, DeleteWarn, Timeout, Kick, Ban }

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self { Self::Delete => "delete", Self::DeleteWarn => "warn", Self::Timeout => "timeout", Self::Kick => "kick", Self::Ban => "ban" }
    }
    pub fn label(self) -> &'static str {
        match self { Self::Delete => "Delete", Self::DeleteWarn => "Delete + warn", Self::Timeout => "Timeout", Self::Kick => "Kick", Self::Ban => "Ban" }
    }
    pub fn strength(self) -> u8 {
        match self { Self::Delete => 0, Self::DeleteWarn => 1, Self::Timeout => 2, Self::Kick => 3, Self::Ban => 4 }
    }
}

impl FromStr for RuleAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "delete" => Ok(Self::Delete), "warn" => Ok(Self::DeleteWarn), "timeout" => Ok(Self::Timeout),
            "kick" => Ok(Self::Kick), "ban" => Ok(Self::Ban),
            _ => Err(format!("unknown action {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleConfig {
    pub kind: RuleKind,
    pub enabled: bool,
    pub action: RuleAction,
    pub timeout_secs: i64,
    pub threshold: i32,
    pub window_secs: i32,
    pub percent: i32,
    pub extra: i32,
    pub ignored_roles: Vec<i64>,
    pub ignored_channels: Vec<i64>,
}

impl RuleKind {
    pub fn is_multi_message(self) -> bool {
        matches!(self, Self::FastMessages | Self::RepeatedMessages | Self::FileSpam)
    }

    /// Spec §4 defaults, rule starts disabled. Field meaning per rule:
    /// fast: threshold messages in window_secs; repeat: threshold similar messages in window_secs at percent similarity;
    /// mentions: threshold mentions; emoji: threshold emojis; caps: percent capitals, threshold = minimum letters;
    /// files: threshold attachments in window_secs; wall: threshold lines, extra characters;
    /// spoilers: threshold spoiler tags; zalgo: threshold stacked marks on one character, percent of characters carrying marks;
    /// words/links/invites/scam: no numbers.
    pub fn defaults(self) -> RuleConfig {
        use RuleAction::*;
        let (action, timeout_secs, threshold, window_secs, percent, extra) = match self {
            Self::BannedWords => (DeleteWarn, 600, 0, 0, 0, 0),
            Self::FastMessages => (Timeout, 600, 6, 5, 0, 0),
            Self::RepeatedMessages => (DeleteWarn, 600, 3, 30, 80, 0),
            Self::MassMentions => (DeleteWarn, 600, 5, 0, 0, 0),
            Self::EmojiSpam => (Delete, 600, 10, 0, 0, 0),
            Self::AllCaps => (Delete, 600, 10, 0, 70, 0),
            Self::FileSpam => (DeleteWarn, 600, 5, 10, 0, 0),
            Self::Links | Self::Invites => (Delete, 600, 0, 0, 0, 0),
            Self::ScamLinks => (Timeout, 86_400, 0, 0, 0, 0),
            Self::WallsOfText => (Delete, 600, 15, 0, 0, 1500),
            Self::SpoilerSpam => (Delete, 600, 5, 0, 0, 0),
            Self::Zalgo => (Delete, 600, 4, 0, 40, 0),
        };
        RuleConfig { kind: self, enabled: false, action, timeout_secs, threshold, window_secs, percent, extra, ignored_roles: vec![], ignored_channels: vec![] }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordKind { Whole, Wildcard, Regex }

impl WordKind {
    pub fn as_str(self) -> &'static str {
        match self { Self::Whole => "whole", Self::Wildcard => "wildcard", Self::Regex => "regex" }
    }
}

impl FromStr for WordKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s { "whole" => Ok(Self::Whole), "wildcard" => Ok(Self::Wildcard), "regex" => Ok(Self::Regex), _ => Err(format!("unknown word kind {s}")) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordEntry {
    pub kind: WordKind,
    pub pattern: String,
}

/// One line of the add/remove pop-up: `/x/` regex, contains `*` wildcard, else whole word.
pub fn parse_word_line(s: &str) -> Option<WordEntry> {
    let s = s.trim();
    if s.is_empty() { return None; }
    if s.len() > 2 && s.starts_with('/') && s.ends_with('/') {
        return Some(WordEntry { kind: WordKind::Regex, pattern: s[1..s.len() - 1].to_string() });
    }
    let kind = if s.contains('*') { WordKind::Wildcard } else { WordKind::Whole };
    Some(WordEntry { kind, pattern: s.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_round_trip() {
        for k in RuleKind::ALL { assert_eq!(k.as_str().parse::<RuleKind>(), Ok(k)); }
        for a in [RuleAction::Delete, RuleAction::DeleteWarn, RuleAction::Timeout, RuleAction::Kick, RuleAction::Ban] {
            assert_eq!(a.as_str().parse::<RuleAction>(), Ok(a));
        }
        assert!("nope".parse::<RuleKind>().is_err());
    }

    #[test]
    fn parse_word_line_kinds() {
        assert_eq!(parse_word_line("badword"), Some(WordEntry { kind: WordKind::Whole, pattern: "badword".into() }));
        assert_eq!(parse_word_line("*scam*").unwrap().kind, WordKind::Wildcard);
        let r = parse_word_line("/d[i1]sc/").unwrap();
        assert_eq!((r.kind, r.pattern.as_str()), (WordKind::Regex, "d[i1]sc"));
        assert_eq!(parse_word_line("  "), None);
    }

    #[test]
    fn action_strength_order() {
        use RuleAction::*;
        let s: Vec<u8> = [Delete, DeleteWarn, Timeout, Kick, Ban].iter().map(|a| a.strength()).collect();
        assert!(s.windows(2).all(|w| w[0] < w[1]));
    }
}

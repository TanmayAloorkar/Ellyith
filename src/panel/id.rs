use poise::serenity_prelude::{ChannelId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Members,
    NamesVoice,
    Channels,
    Records,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Warn,
    Timeout,
    Untimeout,
    Kick,
    Tempban,
    Ban,
    Unban,
    NickSet,
    NickReset,
    Disconnect,
    VoiceMute,
    VoiceUnmute,
    Move,
    Purge,
    History,
    CaseView,
    CaseReason,
    CaseRemove,
    NoteAdd,
    NoteList,
    NoteView,
    NoteDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelId {
    /// Back to the category list.
    Home,
    Category(Category),
    Action(Action),
    /// User select.
    PickUser(Action),
    /// Channel select (Move carries the user, Purge none).
    PickChannel(Action, Option<UserId>),
    /// Pop-up submit.
    Modal(Action, Option<UserId>, Option<ChannelId>),
}

const CATEGORIES: [(Category, &str); 5] = [
    (Category::Members, "members"),
    (Category::NamesVoice, "names"),
    (Category::Channels, "channels"),
    (Category::Records, "records"),
    (Category::Config, "config"),
];

const ACTIONS: [(Action, &str); 22] = [
    (Action::Warn, "warn"),
    (Action::Timeout, "timeout"),
    (Action::Untimeout, "untimeout"),
    (Action::Kick, "kick"),
    (Action::Tempban, "tempban"),
    (Action::Ban, "ban"),
    (Action::Unban, "unban"),
    (Action::NickSet, "nickset"),
    (Action::NickReset, "nickreset"),
    (Action::Disconnect, "disconnect"),
    (Action::VoiceMute, "vmute"),
    (Action::VoiceUnmute, "vunmute"),
    (Action::Move, "move"),
    (Action::Purge, "purge"),
    (Action::History, "history"),
    (Action::CaseView, "caseview"),
    (Action::CaseReason, "casereason"),
    (Action::CaseRemove, "caseremove"),
    (Action::NoteAdd, "noteadd"),
    (Action::NoteList, "notelist"),
    (Action::NoteView, "noteview"),
    (Action::NoteDelete, "notedelete"),
];

impl Category {
    fn token(self) -> &'static str {
        CATEGORIES.iter().find(|(c, _)| *c == self).unwrap().1
    }
    fn parse(s: &str) -> Option<Self> {
        CATEGORIES.iter().find(|(_, t)| *t == s).map(|(c, _)| *c)
    }
}

impl Action {
    fn token(self) -> &'static str {
        ACTIONS.iter().find(|(a, _)| *a == self).unwrap().1
    }
    fn parse(s: &str) -> Option<Self> {
        ACTIONS.iter().find(|(_, t)| *t == s).map(|(a, _)| *a)
    }
}

fn opt(id: Option<u64>) -> String {
    id.map_or("-".into(), |v| v.to_string())
}

fn parse_opt(s: &str) -> Option<Option<u64>> {
    if s == "-" {
        return Some(None);
    }
    // Reject "0", signs and leading zeros so every id has one encoding.
    if s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().map(Some)
}

impl PanelId {
    pub fn encode(&self) -> String {
        match self {
            PanelId::Home => "mp:h".into(),
            PanelId::Category(c) => format!("mp:c:{}", c.token()),
            PanelId::Action(a) => format!("mp:a:{}", a.token()),
            PanelId::PickUser(a) => format!("mp:u:{}", a.token()),
            PanelId::PickChannel(a, u) => format!("mp:p:{}:{}", a.token(), opt(u.map(|u| u.get()))),
            PanelId::Modal(a, u, c) => format!("mp:m:{}:{}:{}", a.token(), opt(u.map(|u| u.get())), opt(c.map(|c| c.get()))),
        }
    }

    pub fn decode(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.strip_prefix("mp:")?.split(':').collect();
        let user = |s| parse_opt(s).map(|o| o.map(UserId::new));
        let chan = |s| parse_opt(s).map(|o| o.map(ChannelId::new));
        match parts.as_slice() {
            ["h"] => Some(PanelId::Home),
            ["c", c] => Category::parse(c).map(PanelId::Category),
            ["a", a] => Action::parse(a).map(PanelId::Action),
            ["u", a] => Action::parse(a).map(PanelId::PickUser),
            ["p", a, u] => Some(PanelId::PickChannel(Action::parse(a)?, user(u)?)),
            ["m", a, u, c] => Some(PanelId::Modal(Action::parse(a)?, user(u)?, chan(c)?)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_junk() {
        let big_u = UserId::new(u64::MAX);
        let big_c = ChannelId::new(u64::MAX);
        let mut all = vec![
            PanelId::Home,
            PanelId::Category(Category::Config),
            PanelId::Action(Action::NoteDelete),
            PanelId::PickUser(Action::Kick),
            PanelId::PickChannel(Action::Move, Some(big_u)),
            PanelId::PickChannel(Action::Purge, None),
            PanelId::Modal(Action::Move, Some(big_u), Some(big_c)),
            PanelId::Modal(Action::Unban, None, None),
        ];
        all.extend(CATEGORIES.iter().map(|(c, _)| PanelId::Category(*c)));
        all.extend(ACTIONS.iter().map(|(a, _)| PanelId::Modal(*a, Some(big_u), Some(big_c))));
        for id in all {
            let s = id.encode();
            assert!(s.starts_with("mp:") && s.len() <= 100);
            assert_eq!(PanelId::decode(&s), Some(id));
        }
        for junk in ["", "mp:", "mp:zz", "mp:m:kick:abc:-", "other:home", "mp:c:members:extra", "mp:m:kick:0:-", "mp:p:move:+5"] {
            assert_eq!(PanelId::decode(junk), None, "{junk}");
        }
    }
}

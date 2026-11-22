use super::id::{Action, Category, PanelId};
use crate::reply::Tone;
use poise::serenity_prelude::{ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateEmbedFooter};

type Views = (CreateEmbed, Vec<CreateActionRow>);

const HOME_ROWS: [(&str, &str); 5] = [
    ("👤 Members", "Warn, timeout, kick and ban"),
    ("🏷️ Names & voice", "Nicknames, voice mute, move, disconnect"),
    ("🧹 Channels", "Purge messages"),
    ("📁 Records", "History, cases and private notes"),
    ("⚙️ Config", "Bot settings · Manage Server only"),
];

const HOME_ORDER: [(Category, ButtonStyle); 5] = [
    (Category::Members, ButtonStyle::Primary),
    (Category::NamesVoice, ButtonStyle::Primary),
    (Category::Channels, ButtonStyle::Primary),
    (Category::Records, ButtonStyle::Primary),
    (Category::Config, ButtonStyle::Secondary),
];

/// One action button plus its one-line explanation.
type Item = (Action, &'static str, &'static str, ButtonStyle);
/// A group of action buttons: one embed field, one button row.
type Group = (&'static str, &'static [Item]);

use ButtonStyle::{Danger as D, Primary as P, Secondary as S, Success as G};

const MEMBERS: &[Group] = &[
    ("Light", &[
        (Action::Warn, "⚠️ Warn", "Logged warning + DM. Counts toward auto-punishment.", S),
        (Action::Timeout, "🔇 Timeout", "Can't chat for a set time, up to 28 days.", S),
    ]),
    ("Remove", &[
        (Action::Kick, "👢 Kick", "Removed, can rejoin with an invite.", D),
        (Action::Tempban, "⏳ Tempban", "Banned for a set time, lifted automatically.", D),
        (Action::Ban, "🔨 Ban", "Banned until someone unbans them.", D),
    ]),
    ("Undo", &[
        (Action::Untimeout, "🔊 Untimeout", "End a timeout early.", G),
        (Action::Unban, "♻️ Unban", "Lift a ban. Needs the user ID.", G),
    ]),
];

const NAMES_VOICE: &[Group] = &[
    ("Nicknames", &[
        (Action::NickSet, "🏷️ Set nickname", "Change a member's nickname.", S),
        (Action::NickReset, "🧽 Reset nickname", "Remove a member's nickname.", S),
    ]),
    ("Voice mute", &[
        (Action::VoiceMute, "🎙️ Voice mute", "Mute a member in voice. Logged as a case.", S),
        (Action::VoiceUnmute, "🔈 Voice unmute", "Lift a voice mute.", G),
    ]),
    ("Voice channels", &[
        (Action::Move, "➡️ Move", "Move a member to another voice channel.", S),
        (Action::Disconnect, "🔌 Disconnect", "Kick a member out of voice.", S),
    ]),
];

const CHANNELS: &[Group] = &[(
    "",
    &[(Action::Purge, "🧹 Purge", "Delete the last **N** messages, or everything from the last **5m**, **2h**… Optional: only one member's messages.", D)],
)];

const RECORDS: &[Group] = &[
    ("Cases", &[
        (Action::History, "📜 History", "All cases for a member and their active warnings.", P),
        (Action::CaseView, "🔍 View case", "One case by number.", S),
        (Action::CaseReason, "✏️ Edit reason", "Fix the reason on a case.", S),
        (Action::CaseRemove, "🗑️ Remove case", "Hidden and stops counting. Admins still see it.", D),
    ]),
    ("Notes · staff only", &[
        (Action::NoteAdd, "📝 Add note", "Private note on a member.", S),
        (Action::NoteList, "🗒️ Notes", "List a member's notes.", S),
        (Action::NoteView, "👁️ View note", "Read one note in full.", S),
        (Action::NoteDelete, "❌ Delete note", "Remove a note.", D),
    ]),
];

/// Config text only: its action buttons arrive with the config ids.
const CONFIG: &[(&str, &[(&str, &str)])] = &[
    ("People", &[("🛡️ Mod roles", "Who can use moderation."), ("🔒 Protected", "Roles mods can't act on.")]),
    ("Messages & logs", &[("📋 Log channels", "Where each kind of log goes."), ("✉️ DM messages", "What members are told when punished.")]),
    ("Rules", &[("📈 Ladder", "Auto-punish after N warnings."), ("⏱️ Warn decay", "Old warnings stop counting."), ("🚫 Name filter", "Reset bad nicknames.")]),
];

fn button(id: PanelId, label: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(id.encode()).label(label).style(style)
}

fn back() -> CreateButton {
    button(PanelId::Home, "← Back", S)
}

pub fn back_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![back()])
}

/// Emoji and label of an action's panel button.
pub fn action_label(a: Action) -> &'static str {
    [MEMBERS, NAMES_VOICE, CHANNELS, RECORDS]
        .into_iter()
        .flatten()
        .flat_map(|(_, items)| items.iter())
        .find(|(x, ..)| *x == a)
        .map_or("Action", |(_, l, ..)| l)
}

fn line(label: &str, desc: &str) -> String {
    format!("**{label}** {desc}")
}

pub fn home() -> Views {
    let rows: Vec<String> = HOME_ROWS.iter().map(|(l, d)| line(l, d)).collect();
    let embed = CreateEmbed::new()
        .colour(Tone::Panel.colour())
        .title("Moderation panel")
        .description(format!(
            "Pick what you need. Every reply is visible only to you and follows the same rules as the slash commands.\n\n{}",
            rows.join("\n")
        ));
    let buttons = HOME_ORDER.iter().zip(HOME_ROWS).map(|((c, st), (label, _))| button(PanelId::Category(*c), label, *st)).collect();
    (embed, vec![CreateActionRow::Buttons(buttons)])
}

fn grouped(tone: Tone, title: &str, intro: &str, groups: &[Group], footer: Option<&str>) -> Views {
    let mut embed = CreateEmbed::new().colour(tone.colour()).title(title);
    let mut rows: Vec<Vec<CreateButton>> = vec![];
    let mut ungrouped = vec![];
    for (name, items) in groups {
        let lines: Vec<String> = items.iter().map(|(_, l, d, _)| line(l, d)).collect();
        if name.is_empty() {
            ungrouped = lines;
        } else {
            embed = embed.field(*name, lines.join("\n"), false);
        }
        rows.push(items.iter().map(|(a, l, _, st)| button(PanelId::Action(*a), l, *st)).collect());
    }
    let desc = if ungrouped.is_empty() { intro.to_string() } else { format!("{intro}\n\n{}", ungrouped.join("\n")) };
    embed = embed.description(desc);
    if let Some(f) = footer {
        embed = embed.footer(CreateEmbedFooter::new(f));
    }
    // Back shares the last row when it is short, like the mockup.
    match rows.last_mut().filter(|r| r.len() <= 3) {
        Some(r) => r.push(back()),
        None => rows.push(vec![back()]),
    }
    (embed, rows.into_iter().map(CreateActionRow::Buttons).collect())
}

pub fn category(c: Category) -> Views {
    match c {
        Category::Members => grouped(Tone::Members, "Members", "Choose an action, then pick the member.", MEMBERS, None),
        Category::NamesVoice => grouped(Tone::NamesVoice, "Names & voice", "Choose an action, then pick the member.", NAMES_VOICE, None),
        Category::Channels => grouped(Tone::Channels, "Channels", "Clean up a channel.", CHANNELS, Some("More channel tools (lockdown) arrive with the next update.")),
        Category::Records => grouped(Tone::Records, "Records", "Look things up and keep notes.", RECORDS, None),
        Category::Config => {
            let mut e = CreateEmbed::new().colour(Tone::Config.colour()).title("Config").description("Bot settings. Changes apply right away.");
            for (name, rows) in CONFIG {
                e = e.field(*name, rows.iter().map(|(l, d)| line(l, d)).collect::<Vec<_>>().join("\n"), false);
            }
            (e, vec![CreateActionRow::Buttons(vec![back()])])
        }
    }
}

/// Every button custom_id in the rows, in order (test helper: serde_json is a dev-dependency).
#[cfg(test)]
pub fn button_ids(rows: &[CreateActionRow]) -> Vec<String> {
    let v = serde_json::to_value(rows).expect("rows serialize");
    v.as_array()
        .into_iter()
        .flatten()
        .flat_map(|r| r["components"].as_array().into_iter().flatten())
        .filter_map(|c| c["custom_id"].as_str().map(String::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_every_id_decodes() {
        let cats = [Category::Members, Category::NamesVoice, Category::Channels, Category::Records, Category::Config];
        for (embed, rows) in std::iter::once(home()).chain(cats.map(category)) {
            let v = serde_json::to_value(&rows).unwrap();
            assert!(v.as_array().unwrap().len() <= 5);
            for row in v.as_array().unwrap() {
                assert!(row["components"].as_array().unwrap().len() <= 5);
            }
            let ids = button_ids(&rows);
            assert!(!ids.is_empty());
            for id in ids {
                assert!(PanelId::decode(&id).is_some(), "{id}");
            }
            let e = serde_json::to_value(&embed).unwrap();
            assert!(e["description"].as_str().map_or(0, |d| d.chars().count()) <= 4096);
        }
    }

    #[test]
    fn home_has_category_buttons() {
        let ids = button_ids(&home().1);
        assert_eq!(ids.len(), 5);
        assert_eq!(PanelId::decode(&ids[4]), Some(PanelId::Category(Category::Config)));
    }
}

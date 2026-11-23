use crate::{Context, Error};
use poise::serenity_prelude::{CreateAllowedMentions, CreateEmbed, CreateEmbedFooter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Members,
    NamesVoice,
    Channels,
    Records,
    Config,
    Panel,
    Safety,
}

impl Tone {
    /// One colour for every bot embed (owner, 2026-11-23); tones kept so it can be split again.
    pub fn colour(self) -> u32 {
        BAR
    }
}

/// Owner's chosen bar colour, #BAA3FF.
pub const BAR: u32 = 0xbaa3ff;
const REFUSED: u32 = BAR;

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn done(tone: Tone, title: &str, lines: &[(&str, String)], footer: Option<&str>) -> CreateEmbed {
    let body: Vec<String> = lines.iter().map(|(l, v)| format!("**{l}** {}", cut(v, 1024))).collect();
    let mut e = CreateEmbed::new().colour(tone.colour()).title(title).description(cut(&body.join("\n"), 4096));
    if let Some(f) = footer {
        e = e.footer(CreateEmbedFooter::new(f));
    }
    e
}

pub fn refused(reason: &str, dm_skipped: bool) -> CreateEmbed {
    let footer = if dm_skipped { "Nothing was changed. No DM was sent." } else { "Nothing was changed." };
    CreateEmbed::new().colour(REFUSED).title("Can't do that").description(reason).footer(CreateEmbedFooter::new(footer))
}

pub fn list(tone: Tone, title: &str, intro: Option<&str>, groups: &[(&str, Vec<String>)]) -> CreateEmbed {
    let mut e = CreateEmbed::new().colour(tone.colour()).title(title);
    if let Some(i) = intro {
        e = e.description(i);
    }
    // Discord rejects empty field values.
    for (name, lines) in groups.iter().filter(|(_, l)| !l.is_empty()) {
        e = e.field(*name, cut(&lines.join("\n"), 1024), false);
    }
    e
}

pub async fn send(ctx: Context<'_>, embed: CreateEmbed) -> Result<(), Error> {
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders() {
        assert_eq!(Tone::Members.colour(), 0xbaa3ff);
        assert_eq!(Tone::Panel.colour(), 0xbaa3ff);
        assert_eq!(Tone::Safety.colour(), 0xbaa3ff);
        let e = serde_json::to_value(refused("That member's highest role is at or above yours.", true)).unwrap();
        assert_eq!(e["title"], "Can't do that");
        assert_eq!(e["footer"]["text"], "Nothing was changed. No DM was sent.");
        assert_eq!(e["color"], 0xbaa3ff);
        let e = serde_json::to_value(refused("x", false)).unwrap();
        assert_eq!(e["footer"]["text"], "Nothing was changed.");
        let d = serde_json::to_value(done(Tone::Members, "👢 Kicked username", &[("Case", "#12".into()), ("Reason", "x".repeat(2000))], Some("Logged to #mod-log"))).unwrap();
        let desc = d["description"].as_str().unwrap();
        assert!(desc.contains("**Case** #12"));
        assert_eq!(desc.lines().nth(1).unwrap().chars().count(), "**Reason** ".len() + 1024);
        assert_eq!(d["footer"]["text"], "Logged to #mod-log");
        let l = serde_json::to_value(list(Tone::Records, "📜 username — 4 cases", Some("Active warnings: 2"), &[("Recent", vec!["a".into(), "b".into()]), ("Big", vec!["y".repeat(2000)])])).unwrap();
        assert_eq!(l["fields"][0]["name"], "Recent");
        assert_eq!(l["fields"][0]["value"], "a\nb");
        assert_eq!(l["fields"][1]["value"].as_str().unwrap().chars().count(), 1024);
    }

    #[test]
    fn done_caps_description_and_list_skips_empty_groups() {
        let lines: Vec<(&str, String)> = (0..10).map(|_| ("L", "x".repeat(1024))).collect();
        let d = serde_json::to_value(done(Tone::Members, "t", &lines, None)).unwrap();
        assert_eq!(d["description"].as_str().unwrap().chars().count(), 4096);
        let l = serde_json::to_value(list(Tone::Records, "t", None, &[("Empty", vec![]), ("Full", vec!["a".into()])])).unwrap();
        assert_eq!(l["fields"].as_array().unwrap().len(), 1);
        assert_eq!(l["fields"][0]["name"], "Full");
    }
}

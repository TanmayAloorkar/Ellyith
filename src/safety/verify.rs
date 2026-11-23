//! Verification: the public Verify button, the quiz, and `/config verification …`.
use crate::invoker::Invoker;
use crate::moderation::cases::Actor;
use crate::moderation::perms::require_manage_server;
use crate::panel::flows::{modal_values, msg, outcome_of};
use crate::panel::id::PanelId;
use crate::reply::{self, Tone};
use crate::safety::logic::{parse_question_line, quiz_passes};
use crate::safety::quarantine::{bot_top, post_review_card, release, require_setup, setup_sync_counts, sync_lines, QuarantineReason};
use crate::safety::store::{self, QuarantineRow, SafetySettings};
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateModal, EditInteractionResponse, GuildId, InputTextStyle, MessageId,
    ModalInteraction, ReactionType, RoleId, UserId,
};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const VERIFY_EMOJI: &str = "✅";
const COOLDOWN: Duration = Duration::from_secs(60);
const MAX_QUESTIONS: usize = 5;
// The panel role select shows at most 25.
const MAX_ROLES: usize = 25;
const NEED_CHANNEL: &str = "Pick a verify channel first.";
const WAIT: &str = "Some answers weren't right. Try again in 1 minute.";

type Cooldowns = HashMap<(u64, u64), Instant>;
static FAILED: Mutex<Option<Cooldowns>> = Mutex::new(None);

/// True while the member must wait; drops entries older than a minute first.
fn cooling(m: &mut Cooldowns, key: (u64, u64), now: Instant) -> bool {
    m.retain(|_, t| now.duration_since(*t) < COOLDOWN);
    m.contains_key(&key)
}

fn mark(m: &mut Cooldowns, key: (u64, u64), now: Instant) {
    m.insert(key, now);
}

fn on_cooldown(key: (u64, u64)) -> bool {
    cooling(FAILED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default(), key, Instant::now())
}

pub fn verify_message(s: &SafetySettings) -> CreateMessage {
    let embed = CreateEmbed::new().colour(Tone::Safety.colour()).title("👋 Welcome! Verify to get in").description(&s.verify_description);
    let button = CreateButton::new(PanelId::Verify.encode()).label("Verify").emoji(ReactionType::Unicode(VERIFY_EMOJI.into())).style(ButtonStyle::Success);
    CreateMessage::new().embed(embed).components(vec![CreateActionRow::Buttons(vec![button])]).allowed_mentions(CreateAllowedMentions::new())
}

pub fn quiz_modal(s: &SafetySettings) -> Option<CreateModal> {
    if s.quiz_questions.is_empty() {
        return None;
    }
    let rows = s
        .quiz_questions
        .iter()
        .enumerate()
        .map(|(i, q)| CreateActionRow::InputText(CreateInputText::new(InputTextStyle::Paragraph, q, i.to_string()).max_length(200).required(true)))
        .collect();
    Some(CreateModal::new(PanelId::VerifyQuiz.encode(), "Quick questions").components(rows))
}

fn note(title: &str, body: &str) -> CreateEmbed {
    reply::list(Tone::Safety, title, Some(body), &[])
}

fn already() -> CreateEmbed {
    note("✅ Already verified", "You're already verified.")
}

fn welcome() -> CreateEmbed {
    note("✅ You're in", "Welcome to the server.")
}

fn thanks() -> CreateEmbed {
    note("📝 Answers received", "Thanks — staff will check your answers soon.")
}

/// The member's row, only while they wait for verification.
async fn pending(data: &Data, gid: GuildId, user: UserId) -> Result<Option<QuarantineRow>, Error> {
    let row = store::quarantine_get(&data.db, gid.get() as i64, user.get() as i64).await?;
    Ok(row.filter(|r| r.reason == QuarantineReason::Verification.as_str()))
}

async fn say(ctx: &serenity::Context, ci: &ComponentInteraction, e: CreateEmbed) -> Result<(), Error> {
    ci.create_response(&ctx.http, CreateInteractionResponse::Message(msg(e, vec![]).ephemeral(true))).await?;
    Ok(())
}

/// Verify button. Button mode defers then releases; quiz mode opens the pop-up at once.
pub async fn on_press(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction) -> Result<(), Error> {
    let Some(gid) = ci.guild_id else { return say(ctx, ci, reply::refused("Use this button in the server.", false)).await };
    let s = store::get_settings(&data.db, gid.get() as i64).await?;
    let Some(row) = pending(data, gid, ci.user.id).await? else { return say(ctx, ci, already()).await };
    if s.verify_mode != "quiz" {
        ci.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
        let res = release(ctx, data, gid, ci.user.id, Actor::System).await.map(|ok| if ok { welcome() } else { already() });
        let e = outcome_of(PanelId::Verify, false, res);
        ci.edit_response(&ctx.http, EditInteractionResponse::new().embed(e)).await?;
        return Ok(());
    }
    if row.answers.is_some() {
        return say(ctx, ci, thanks()).await;
    }
    if on_cooldown((gid.get(), ci.user.id.get())) {
        return say(ctx, ci, note("⏳ Wait a moment", WAIT)).await;
    }
    match quiz_modal(&s) {
        Some(m) => ci.create_response(&ctx.http, CreateInteractionResponse::Modal(m)).await?,
        None => say(ctx, ci, reply::refused("Verification isn't set up yet. Ask a moderator.", false)).await?,
    }
    Ok(())
}

/// Quiz submit: staff mode stores the answers for review, auto mode grades them.
pub async fn on_submit(ctx: &serenity::Context, data: &Data, mi: &ModalInteraction) -> Result<(), Error> {
    let Some(gid) = mi.guild_id else { return Ok(()) };
    mi.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let res = submit(ctx, data, gid, mi.user.id, modal_values(mi)).await;
    let e = outcome_of(PanelId::VerifyQuiz, false, res);
    mi.edit_response(&ctx.http, EditInteractionResponse::new().embed(e)).await?;
    Ok(())
}

async fn submit(ctx: &serenity::Context, data: &Data, gid: GuildId, user: UserId, values: Vec<String>) -> Result<CreateEmbed, Error> {
    let Some(mut row) = pending(data, gid, user).await? else { return Ok(already()) };
    let s = store::get_settings(&data.db, gid.get() as i64).await?;
    if s.verify_review == "staff" {
        if row.answers.is_some() {
            return Ok(thanks());
        }
        if !store::quarantine_set_answers(&data.db, row.guild_id, row.user_id, &values).await? {
            return Ok(already());
        }
        row.answers = Some(values);
        // Answers are saved; a missing card is not worth failing the member's reply.
        if let Err(e) = post_review_card(ctx, data, gid, &row).await {
            tracing::warn!("posting review card for {user} failed: {e}");
        }
        return Ok(thanks());
    }
    let key = (gid.get(), user.get());
    if on_cooldown(key) {
        return Ok(note("⏳ Wait a moment", WAIT));
    }
    if !quiz_passes(&s.quiz_answers, &values) {
        mark(FAILED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default(), key, Instant::now());
        return Ok(note("❌ Not quite", WAIT));
    }
    Ok(if release(ctx, data, gid, user, Actor::System).await? { welcome() } else { already() })
}

// ---- /config verification ----

fn cfg(title: &str, lines: &[(&str, String)]) -> CreateEmbed {
    reply::done(Tone::Config, title, lines, None)
}

async fn load(inv: &Invoker<'_>) -> Result<SafetySettings, Error> {
    require_manage_server(inv).await?;
    Ok(store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?)
}

pub async fn set_method_core(inv: &Invoker<'_>, mode: &str) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    if mode != "off" {
        if s.verify_channel_id.is_none() {
            return Err(UserError(NEED_CHANNEL.into()).into());
        }
        {
            let g = inv.ctx.cache.guild(inv.guild_id).ok_or("guild not in cache")?;
            require_setup(&s, &g)?;
        }
        if mode == "quiz" && s.quiz_questions.is_empty() {
            return Err(UserError("Add at least one question first.".into()).into());
        }
    }
    s.verify_mode = mode.into();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(cfg("👋 Verification method saved", &[("Method", mode.into())]))
}

pub async fn set_review_core(inv: &Invoker<'_>, mode: &str) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    s.verify_review = mode.into();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(cfg("👋 Quiz review saved", &[("Answers", mode.into())]))
}

pub async fn set_channel_core(inv: &Invoker<'_>, channel: ChannelId) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    s.verify_channel_id = Some(channel.get() as i64);
    store::save_settings(&inv.data.db, &s).await?;
    let mut lines = vec![("Channel", format!("<#{}>", channel.get()))];
    // Without a quarantine role there is nothing to sync yet.
    if s.quarantine_role_id.is_some() {
        let (updated, failed) = setup_sync_counts(inv).await?;
        lines.extend(sync_lines(updated, failed));
    }
    Ok(cfg("👋 Verify channel saved", &lines))
}

pub enum RoleOp {
    Add(RoleId),
    Remove(RoleId),
    /// Replace the whole list (panel multi-select); every role is checked like `Add`.
    Set(Vec<RoleId>),
    Clear,
}

async fn check_roles(inv: &Invoker<'_>, roles: &[RoleId]) -> Result<(), Error> {
    let top = bot_top(inv.ctx, inv.guild_id).await?;
    let g = inv.ctx.cache.guild(inv.guild_id).ok_or("guild not in cache")?;
    for r in roles {
        let role = g.roles.get(r).ok_or_else(|| UserError("That role no longer exists.".into()))?;
        if role.managed {
            return Err(UserError("That role is managed by an integration.".into()).into());
        }
        if role.position >= top {
            return Err(UserError("That role is not below mine.".into()).into());
        }
    }
    Ok(())
}

pub async fn set_roles_core(inv: &Invoker<'_>, op: RoleOp) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    match op {
        RoleOp::Add(r) => {
            check_roles(inv, &[r]).await?;
            add_role(&mut s.verify_roles, r.get() as i64)?;
        }
        RoleOp::Set(roles) => {
            check_roles(inv, &roles).await?;
            s.verify_roles = roles.iter().map(|r| r.get() as i64).collect();
        }
        RoleOp::Remove(r) => s.verify_roles.retain(|x| *x != r.get() as i64),
        RoleOp::Clear => s.verify_roles.clear(),
    }
    store::save_settings(&inv.data.db, &s).await?;
    let roles = if s.verify_roles.is_empty() { "none".to_string() } else { s.verify_roles.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" ") };
    Ok(cfg("👋 Verified roles saved", &[("Roles", roles)]))
}

fn add_role(roles: &mut Vec<i64>, id: i64) -> Result<(), UserError> {
    if !roles.contains(&id) {
        if roles.len() >= MAX_ROLES {
            return Err(UserError("At most 25 verified roles.".into()));
        }
        roles.push(id);
    }
    Ok(())
}

/// Quiz mode needs a question to show.
fn check_questions(mode: &str, questions: &[String]) -> Result<(), UserError> {
    if mode == "quiz" && questions.is_empty() {
        return Err(UserError("Switch the method off quiz first, or keep at least one question.".into()));
    }
    Ok(())
}

/// Lines are `question | expected answer` separated by `;`. Errors before anything is saved.
pub fn parse_questions(text: &str) -> Result<(Vec<String>, Vec<String>), UserError> {
    parse_question_lines(text.split(';'))
}

/// One `question | answer` per item (panel pop-up inputs); empty items are dropped.
pub fn parse_question_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<(Vec<String>, Vec<String>), UserError> {
    let mut parsed = Vec::new();
    for line in lines {
        parsed.extend(parse_question_line(line)?);
    }
    if parsed.len() > MAX_QUESTIONS {
        return Err(UserError("At most 5 questions.".into()));
    }
    Ok(parsed.into_iter().unzip())
}

pub async fn set_questions_core(inv: &Invoker<'_>, text: &str) -> Result<CreateEmbed, Error> {
    save_questions(inv, parse_questions(text)?).await
}

pub async fn set_question_lines_core(inv: &Invoker<'_>, lines: &[String]) -> Result<CreateEmbed, Error> {
    save_questions(inv, parse_question_lines(lines.iter().map(String::as_str))?).await
}

async fn save_questions(inv: &Invoker<'_>, parsed: (Vec<String>, Vec<String>)) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    check_questions(&s.verify_mode, &parsed.0)?;
    (s.quiz_questions, s.quiz_answers) = parsed;
    store::save_settings(&inv.data.db, &s).await?;
    let list = if s.quiz_questions.is_empty() {
        "none".to_string()
    } else {
        s.quiz_questions.iter().enumerate().map(|(i, q)| format!("{}. {q}", i + 1)).collect::<Vec<_>>().join("\n")
    };
    Ok(cfg("👋 Questions saved", &[("Questions", format!("\n{list}"))]))
}

pub async fn set_description_core(inv: &Invoker<'_>, text: String) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    let text = text.trim();
    if text.is_empty() || text.chars().count() > 1000 {
        return Err(UserError("Description must be 1 to 1000 characters.".into()).into());
    }
    s.verify_description = text.into();
    store::save_settings(&inv.data.db, &s).await?;
    Ok(cfg("👋 Description saved", &[("Text", s.verify_description.clone())]))
}

/// Post the verify message and delete the previous one (best effort), like `/config panel`.
pub async fn post_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    let mut s = load(inv).await?;
    let channel = ChannelId::new(s.verify_channel_id.ok_or_else(|| UserError(NEED_CHANNEL.into()))? as u64);
    let sent = match channel.send_message(inv.ctx, verify_message(&s)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("posting the verify message in {channel} failed: {e}");
            return Err(UserError("I can't post in that channel — check my permissions there.".into()).into());
        }
    };
    let old = s.verify_message_id.replace(sent.id.get() as i64);
    store::save_settings(&inv.data.db, &s).await?;
    if let Some(m) = old {
        if let Err(e) = channel.delete_message(inv.ctx, MessageId::new(m as u64)).await {
            tracing::debug!("deleting the old verify message failed: {e}");
        }
    }
    Ok(cfg("👋 Verify message posted", &[("Channel", format!("<#{}>", channel.get()))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::store::SafetySettings;

    fn settings(questions: &[&str]) -> SafetySettings {
        SafetySettings {
            guild_id: 1,
            quarantine_role_id: None,
            review_channel_id: None,
            deny_action: "kick".into(),
            verify_mode: "quiz".into(),
            verify_review: "auto".into(),
            verify_channel_id: None,
            verify_roles: vec![],
            verify_description: "Press the button.".into(),
            verify_message_id: None,
            quiz_questions: questions.iter().map(|q| q.to_string()).collect(),
            quiz_answers: questions.iter().map(|_| String::new()).collect(),
            lockdown_channels: vec![],
            lockdown_message: String::new(),
        }
    }

    #[test]
    fn verify_message_shape() {
        let v = serde_json::to_value(verify_message(&settings(&[]))).unwrap();
        let e = &v["embeds"][0];
        assert_eq!(e["title"], "👋 Welcome! Verify to get in");
        assert_eq!(e["description"], "Press the button.");
        assert_eq!(e["color"], 0xbaa3ff);
        let b = &v["components"][0]["components"][0];
        assert_eq!((b["custom_id"].as_str(), b["label"].as_str(), b["style"].as_u64()), (Some("mp:v"), Some("Verify"), Some(3)));
        assert_eq!(b["emoji"]["name"], "✅");
    }

    #[test]
    fn quiz_modal_shape() {
        assert!(quiz_modal(&settings(&[])).is_none());
        let v = serde_json::to_value(quiz_modal(&settings(&["Favourite colour?", "Rules read?"])).unwrap()).unwrap();
        assert_eq!(v["custom_id"], "mp:vq");
        assert!(v["title"].as_str().unwrap().chars().count() <= 45);
        let rows = v["components"].as_array().unwrap();
        let labels: Vec<&str> = rows.iter().map(|r| r["components"][0]["label"].as_str().unwrap()).collect();
        assert_eq!(labels, ["Favourite colour?", "Rules read?"]);
        assert_eq!(rows[0]["components"][0]["max_length"], 200);
        assert_eq!(rows[0]["components"][0]["style"], 2);
    }

    #[test]
    fn cooldown_blocks_then_expires() {
        let mut m = HashMap::new();
        let t0 = Instant::now();
        assert!(!cooling(&mut m, (1, 2), t0));
        mark(&mut m, (1, 2), t0);
        assert!(cooling(&mut m, (1, 2), t0 + Duration::from_secs(30)));
        assert!(!cooling(&mut m, (1, 3), t0 + Duration::from_secs(30)));
        assert!(!cooling(&mut m, (1, 2), t0 + Duration::from_secs(61)));
        assert!(m.is_empty());
    }

    #[test]
    fn add_role_caps_at_25() {
        let mut v: Vec<i64> = (0..24).collect();
        add_role(&mut v, 100).unwrap();
        add_role(&mut v, 100).unwrap(); // already there: no-op even when full
        assert_eq!(v.len(), 25);
        assert_eq!(add_role(&mut v, 101).unwrap_err().0, "At most 25 verified roles.");
        assert_eq!(v.len(), 25);
    }

    #[test]
    fn questions_kept_in_quiz_mode() {
        let msg = "Switch the method off quiz first, or keep at least one question.";
        assert_eq!(check_questions("quiz", &[]).unwrap_err().0, msg);
        assert!(check_questions("quiz", &["Q?".into()]).is_ok());
        assert!(check_questions("button", &[]).is_ok());
        assert!(check_questions("off", &[]).is_ok());
    }

    #[test]
    fn parse_questions_rules() {
        let (q, a) = parse_questions("Colour? | blue ; Rules read?;;").unwrap();
        assert_eq!((q, a), (vec!["Colour?".to_string(), "Rules read?".into()], vec!["blue".to_string(), "".into()]));
        assert_eq!(parse_questions("1;2;3;4;5;6").unwrap_err().0, "At most 5 questions.");
        assert!(parse_questions(&"x".repeat(46)).is_err());
        assert!(parse_questions("").unwrap().0.is_empty());
    }
}

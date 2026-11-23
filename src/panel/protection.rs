//! Protection buttons. Raid alert, raid-ended review and alt card presses are mod-only, checked per press like review cards.
use super::flows::{outcome_of, STALE};
use super::id::{AltAct, PanelId, ProtId};
use crate::invoker::Invoker;
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::perms::{authorize, is_absent};
use crate::protection::{alts, raid};
use crate::reply::{self, Tone};
use crate::safety::quarantine::{self, deny, release_core, QuarantineReason};
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage, EditInteractionResponse, EditMessage, Permissions, UserId,
};
use std::time::Duration;

fn confirm_text(ban: bool) -> (&'static str, &'static str) {
    if ban {
        ("🔨 Ban all raiders?", "Every raider still in the server is banned, one per second. This can't be undone in one step.")
    } else {
        ("👢 Kick all raiders?", "Every raider still in the server is kicked, one per second.")
    }
}

fn perm(ban: bool) -> Permissions {
    if ban { Permissions::BAN_MEMBERS } else { Permissions::KICK_MEMBERS }
}

async fn followup(ctx: &serenity::Context, ci: &ComponentInteraction, embed: CreateEmbed, rows: Vec<CreateActionRow>) -> Result<(), Error> {
    let f = CreateInteractionResponseFollowup::new().embed(embed).components(rows).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    ci.create_followup(&ctx.http, f).await?;
    Ok(())
}

/// Approve or deny each user; one summary embed with the ones that failed.
async fn review_each(inv: &Invoker<'_>, users: &[UserId], approve: bool) -> CreateEmbed {
    let (mut ok, mut failed) = (0, vec![]);
    for (i, &u) in users.iter().enumerate() {
        // Deny kicks or bans: spaced like the other bulk raid actions.
        if !approve && i > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let res = if approve { release_core(inv, u).await } else { deny(inv, u).await };
        match res {
            Ok(_) => ok += 1,
            Err(e) => {
                let why = e.downcast_ref::<UserError>().map_or_else(|| "error, see the log".to_string(), |u| u.0.clone());
                if !e.is::<UserError>() {
                    tracing::warn!("raid review of {u} failed: {e:?}");
                }
                failed.push(format!("<@{u}>: {why}"));
            }
        }
    }
    let title = if approve { format!("✅ Approved {ok} raiders") } else { format!("⛔ Denied {ok} raiders") };
    let lines: Vec<(&str, String)> = if failed.is_empty() { vec![] } else { vec![("Couldn't", failed.join("\n"))] };
    reply::done(Tone::Safety, &title, &lines, None)
}

/// Rebuild the review message's select and buttons from who is still waiting.
async fn refresh(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, raid_id: i64, page: usize) -> Result<(), Error> {
    let guild = ci.guild_id.expect("checked in handle");
    let ids: Vec<u64> = raid::waiting(data, guild, raid_id).await?.iter().map(|r| r.user_id as u64).collect();
    ci.channel_id.edit_message(ctx, ci.message.id, EditMessage::new().components(raid::review_rows(ctx, guild, raid_id, &ids, page))).await?;
    Ok(())
}

/// A button on an alt or new-account card. The card loses its buttons and gets a footer saying who did what.
async fn alt_press(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, inv: &Invoker<'_>, act: AltAct, user: UserId) -> Result<CreateEmbed, Error> {
    let guild = inv.guild_id;
    let perm = match act {
        AltAct::Kick => Permissions::KICK_MEMBERS,
        AltAct::Ban => Permissions::BAN_MEMBERS,
        AltAct::Approve | AltAct::Quarantine => Permissions::MODERATE_MEMBERS,
    };
    authorize(inv, perm, Some(user)).await?;
    let by = Actor::Moderator(ci.user.id.get() as i64);
    let done = match act {
        AltAct::Approve => "✅ Approved",
        AltAct::Quarantine => {
            let member = guild.member(ctx, user).await.map_err(|e| if is_absent(&e) { UserError("They are no longer in the server.".into()).into() } else { Error::from(e) })?;
            quarantine::enter(ctx, data, guild, &member, QuarantineReason::Alt, by, Some(alts::REASON.into()), None).await?;
            "🚧 Quarantined"
        }
        AltAct::Kick | AltAct::Ban => {
            let kind = if act == AltAct::Ban { CaseKind::Ban } else { CaseKind::Kick };
            let req = ActionRequest { guild_id: guild, target: user, kind, actor: by, reason: Some(alts::REASON.into()), duration: None, ban_delete_days: 0, log_case: true };
            execute(ctx, data, req).await?;
            if act == AltAct::Ban { "🔨 Banned" } else { "👢 Kicked" }
        }
    };
    let footer = format!("{done} by @{} · {}", ci.user.name, chrono::Utc::now().format("%H:%M"));
    let card = ci.message.embeds.first().cloned().map(|e| CreateEmbed::from(e).footer(CreateEmbedFooter::new(footer)));
    let mut edit = EditMessage::new().components(vec![]);
    if let Some(card) = card {
        edit = edit.embed(card);
    }
    if let Err(e) = ci.channel_id.edit_message(ctx, ci.message.id, edit).await {
        tracing::warn!("marking alt card failed: {e}");
    }
    Ok(reply::done(Tone::Safety, done, &[("User", format!("<@{user}>"))], None))
}

pub async fn on_component(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, id: ProtId) -> Result<(), Error> {
    let guild = ci.guild_id.expect("checked in handle");
    let inv = Invoker::from_interaction(ctx, data, guild, &ci.user);
    let by = Actor::Moderator(ci.user.id.get() as i64);
    let what = PanelId::Protection(id);
    match id {
        // The confirm lives on an ephemeral message: answer by editing it in place.
        ProtId::RaidBulkConfirm(ban) => {
            ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
            // Drop the button first so a second press can't run it twice.
            ci.edit_response(&ctx.http, EditInteractionResponse::new().embed(reply::done(Tone::Safety, "Working…", &[], Some("One raider per second."))).components(vec![])).await?;
            let res = async {
                authorize(&inv, perm(ban), None).await?;
                raid::bulk(ctx, data, guild, !ban, by).await
            }
            .await;
            ci.edit_response(&ctx.http, EditInteractionResponse::new().embed(outcome_of(what, false, res)).components(vec![]).allowed_mentions(CreateAllowedMentions::new())).await?;
            return Ok(());
        }
        ProtId::ReviewPage(raid_id, page) => {
            ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
            let res = async {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                refresh(ctx, data, ci, raid_id, page).await
            }
            .await;
            if let Err(e) = res {
                followup(ctx, ci, outcome_of(what, false, Err(e)), vec![]).await?;
            }
            return Ok(());
        }
        _ => {}
    }
    // Everything else answers with an ephemeral followup.
    ci.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let mut rows = vec![];
    let res: Result<CreateEmbed, Error> = async {
        match id {
            ProtId::RaidEnd => {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                raid::end(ctx, data, guild, by).await
            }
            ProtId::RaidBulk(ban) => {
                authorize(&inv, perm(ban), None).await?;
                let (title, body) = confirm_text(ban);
                let label = if ban { "Ban all" } else { "Kick all" };
                rows.push(CreateActionRow::Buttons(vec![CreateButton::new(PanelId::Protection(ProtId::RaidBulkConfirm(ban)).encode()).label(label).style(ButtonStyle::Danger)]));
                Ok(reply::done(Tone::Safety, title, &[], None).description(body))
            }
            ProtId::ReviewApprove(raid_id, page) => {
                let ComponentInteractionDataKind::StringSelect { values } = &ci.data.kind else { return Err(UserError(STALE.into()).into()) };
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                let users: Vec<UserId> = values.iter().filter_map(|v| v.parse::<u64>().ok().filter(|n| *n > 0)).map(UserId::new).collect();
                let embed = review_each(&inv, &users, true).await;
                refresh(ctx, data, ci, raid_id, page).await?;
                Ok(embed)
            }
            ProtId::ReviewApproveAll(raid_id) | ProtId::ReviewDenyRest(raid_id) => {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                let approve = matches!(id, ProtId::ReviewApproveAll(_));
                let users: Vec<UserId> = raid::waiting(data, guild, raid_id).await?.iter().map(|r| UserId::new(r.user_id as u64)).collect();
                let embed = review_each(&inv, &users, approve).await;
                refresh(ctx, data, ci, raid_id, 0).await?;
                Ok(embed)
            }
            ProtId::Alt(act, user) => alt_press(ctx, data, ci, &inv, act, user).await,
            ProtId::RaidBulkConfirm(_) | ProtId::ReviewPage(..) => unreachable!("answered above"),
        }
    }
    .await;
    followup(ctx, ci, outcome_of(what, false, res), rows).await
}

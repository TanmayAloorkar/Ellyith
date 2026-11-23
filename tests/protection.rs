use aio_bot::protection::store::*;
use chrono::{Duration, Utc};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn settings_defaults_and_save(pool: PgPool) {
    let d = get_settings(&pool, 1).await.unwrap();
    assert_eq!(d.guild_id, 1);
    assert!(!d.raid_enabled && !d.alt_enabled && !d.gate_enabled && !d.nuke_enabled);
    assert_eq!((d.raid_joins, d.raid_secs, d.raid_filter_days, d.raid_end_mins), (10, 10, 7, 10));
    assert_eq!(d.raid_responses, vec!["alert".to_string(), "quarantine".to_string()]);
    assert!(d.raid_filter.is_empty());
    assert_eq!((d.alt_days, d.alt_pts_age, d.alt_pts_avatar, d.alt_pts_name, d.alt_pts_invite, d.alt_threshold), (7, 40, 20, 40, 30, 60));
    assert_eq!(d.alt_response, "flag");
    assert_eq!((d.gate_days, d.gate_action.as_str()), (7, "quarantine"));
    assert_eq!((d.nuke_window_secs, d.nuke_bans, d.nuke_kicks, d.nuke_chan_del, d.nuke_role_del, d.nuke_webhooks, d.nuke_perms), (10, 3, 3, 2, 2, 3, 1));

    let mut c = d.clone();
    c.raid_enabled = true;
    c.raid_joins = 20;
    c.raid_responses = vec!["ban".into()];
    c.raid_filter = vec!["age".into(), "avatar".into()];
    c.alt_response = "quarantine".into();
    c.gate_action = "kick".into();
    c.nuke_enabled = true;
    c.nuke_perms = 4;
    save_settings(&pool, &c).await.unwrap();
    assert_eq!(get_settings(&pool, 1).await.unwrap(), c);
    let other = get_settings(&pool, 2).await.unwrap();
    assert_eq!(other.raid_joins, 10);
    assert!(!other.raid_enabled);
}

#[sqlx::test(migrations = "./migrations")]
async fn raid_state_survives_reload(pool: PgPool) {
    let now = Utc::now();
    let r = start_raid(&pool, 1, now + Duration::minutes(10)).await.unwrap();
    let a = active_raid(&pool, 1).await.unwrap().unwrap();
    assert_eq!(a, r);
    assert!(active_raid(&pool, 2).await.unwrap().is_none());
    assert!(due_raids(&pool, now).await.unwrap().is_empty());
    assert_eq!(due_raids(&pool, now + Duration::minutes(11)).await.unwrap().len(), 1);

    set_raid_alert(&pool, r.id, 5, 6).await.unwrap();
    set_raid_lockdown(&pool, r.id, true).await.unwrap();
    extend_raid(&pool, r.id, now + Duration::minutes(30)).await.unwrap();
    let a = active_raid(&pool, 1).await.unwrap().unwrap();
    assert_eq!((a.alert_channel, a.alert_message, a.started_lockdown), (Some(5), Some(6), true));
    assert!(due_raids(&pool, now + Duration::minutes(11)).await.unwrap().is_empty());

    assert!(end_raid(&pool, r.id).await.unwrap());
    assert!(!end_raid(&pool, r.id).await.unwrap());
    assert!(active_raid(&pool, 1).await.unwrap().is_none());
    assert!(start_raid(&pool, 1, now + Duration::minutes(10)).await.is_ok());
}

#[sqlx::test(migrations = "./migrations")]
async fn raiders_idempotent(pool: PgPool) {
    let r = start_raid(&pool, 1, Utc::now() + Duration::minutes(10)).await.unwrap();
    let t = Utc::now() - Duration::days(1);
    add_raider(&pool, r.id, 7, t, true).await.unwrap();
    add_raider(&pool, r.id, 7, t, true).await.unwrap();
    add_raider(&pool, r.id, 8, t, false).await.unwrap();
    let rows = raiders(&pool, r.id).await.unwrap();
    assert_eq!(rows.iter().map(|x| (x.user_id, x.default_avatar)).collect::<Vec<_>>(), vec![(7, true), (8, false)]);
}

#[sqlx::test(migrations = "./migrations")]
async fn trust_crud(pool: PgPool) {
    assert!(trusted(&pool, 1).await.unwrap().is_empty());
    assert!(trust_add(&pool, 1, TrustKind::User, 5).await.unwrap());
    assert!(!trust_add(&pool, 1, TrustKind::User, 5).await.unwrap());
    assert!(trust_add(&pool, 1, TrustKind::Role, 5).await.unwrap());
    assert_eq!(trusted(&pool, 1).await.unwrap(), vec![(TrustKind::Role, 5), (TrustKind::User, 5)]);
    assert!(trusted(&pool, 2).await.unwrap().is_empty());
    assert!(trust_remove(&pool, 1, TrustKind::User, 5).await.unwrap());
    assert!(!trust_remove(&pool, 1, TrustKind::User, 5).await.unwrap());
    assert_eq!(trusted(&pool, 1).await.unwrap(), vec![(TrustKind::Role, 5)]);
}

#[sqlx::test(migrations = "./migrations")]
async fn strip_restore_once(pool: PgPool) {
    let id = record_strip(&pool, 1, 2, &[10, 11], "Anti-nuke: bans").await.unwrap();
    let s = get_strip(&pool, id).await.unwrap().unwrap();
    assert_eq!((s.guild_id, s.user_id, s.roles.clone(), s.reason.as_str(), s.restored), (1, 2, vec![10, 11], "Anti-nuke: bans", false));
    assert!(mark_strip_restored(&pool, id).await.unwrap());
    assert!(!mark_strip_restored(&pool, id).await.unwrap());
    assert!(get_strip(&pool, id).await.unwrap().unwrap().restored);
    assert!(get_strip(&pool, id + 100).await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn bans_prune(pool: PgPool) {
    record_ban(&pool, 1, 2, "bad", "Bad One", Some("abc")).await.unwrap();
    record_ban(&pool, 1, 3, "worse", "Worse", None).await.unwrap();
    let now = Utc::now();
    let recent = recent_bans(&pool, 1, now - Duration::hours(1)).await.unwrap();
    assert_eq!(recent.len(), 2);
    let b = recent.iter().find(|b| b.user_id == 2).unwrap();
    assert_eq!((b.username.as_str(), b.display_name.as_str(), b.invite_code.as_deref()), ("bad", "Bad One", Some("abc")));
    assert!(recent_bans(&pool, 1, now + Duration::hours(1)).await.unwrap().is_empty());
    assert!(recent_bans(&pool, 2, now - Duration::hours(1)).await.unwrap().is_empty());
    assert_eq!(prune_bans(&pool, now - Duration::hours(1)).await.unwrap(), 0);
    assert_eq!(prune_bans(&pool, now + Duration::hours(1)).await.unwrap(), 2);
    assert!(recent_bans(&pool, 1, now - Duration::hours(1)).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn member_invite_round_trip(pool: PgPool) {
    assert_eq!(member_invite(&pool, 1, 2).await.unwrap(), None);
    record_member_invite(&pool, 1, 2, Some("abc"), Some(9)).await.unwrap();
    assert_eq!(member_invite(&pool, 1, 2).await.unwrap(), Some((Some("abc".to_string()), Some(9))));
    record_member_invite(&pool, 1, 2, None, None).await.unwrap();
    assert_eq!(member_invite(&pool, 1, 2).await.unwrap(), Some((None, None)));
}

#[sqlx::test(migrations = "./migrations")]
async fn ban_record_carries_invite(pool: PgPool) {
    record_member_invite(&pool, 1, 2, Some("abc"), Some(9)).await.unwrap();
    let code = member_invite(&pool, 1, 2).await.unwrap().and_then(|(c, _)| c);
    record_ban(&pool, 1, 2, "bad", "Bad One", code.as_deref()).await.unwrap();
    let b = recent_bans(&pool, 1, Utc::now() - Duration::hours(1)).await.unwrap();
    assert_eq!(b[0].invite_code.as_deref(), Some("abc"));
}

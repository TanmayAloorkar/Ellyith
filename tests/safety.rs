use aio_bot::safety::store::*;
use chrono::Utc;
use sqlx::PgPool;

fn qrow(guild_id: i64, user_id: i64) -> QuarantineRow {
    QuarantineRow { guild_id, user_id, reason: "manual".into(), saved_roles: vec![10, 11], answers: None, created_at: Utc::now(), review_channel_id: None, review_message_id: None }
}

#[sqlx::test(migrations = "./migrations")]
async fn settings_default_and_save(pool: PgPool) {
    let mut s = get_settings(&pool, 1).await.unwrap();
    assert_eq!((s.verify_mode.as_str(), s.deny_action.as_str()), ("off", "kick"));
    assert_eq!(s.lockdown_message, "Channel locked while staff deal with a problem.");
    s.quiz_questions = vec!["q1".into()];
    s.quiz_answers = vec!["".into()];
    s.lockdown_channels = vec![5];
    save_settings(&pool, &s).await.unwrap();
    assert_eq!(get_settings(&pool, 1).await.unwrap(), s);
    assert_eq!(get_settings(&pool, 2).await.unwrap().lockdown_channels, Vec::<i64>::new());
}

#[sqlx::test(migrations = "./migrations")]
async fn quarantine_crud(pool: PgPool) {
    assert!(quarantine_insert(&pool, &qrow(1, 5)).await.unwrap());
    assert!(!quarantine_insert(&pool, &qrow(1, 5)).await.unwrap());
    assert_eq!(quarantine_get(&pool, 1, 5).await.unwrap().unwrap().saved_roles, vec![10, 11]);
    quarantine_set_review(&pool, 1, 5, 7, 8).await.unwrap();
    let r = quarantine_get(&pool, 1, 5).await.unwrap().unwrap();
    assert_eq!((r.review_channel_id, r.review_message_id), (Some(7), Some(8)));
    assert_eq!(quarantine_list(&pool, 1).await.unwrap().len(), 1);
    assert_eq!(quarantine_list(&pool, 2).await.unwrap().len(), 0);
    assert!(quarantine_delete(&pool, 1, 5).await.unwrap());
    assert!(!quarantine_delete(&pool, 1, 5).await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn quarantine_list_oldest_first(pool: PgPool) {
    quarantine_insert(&pool, &qrow(1, 1)).await.unwrap();
    quarantine_insert(&pool, &qrow(1, 2)).await.unwrap();
    let ids: Vec<i64> = quarantine_list(&pool, 1).await.unwrap().iter().map(|r| r.user_id).collect();
    assert_eq!(ids, [1, 2]);
}

#[sqlx::test(migrations = "./migrations")]
async fn lockdown_save_load_clear(pool: PgPool) {
    assert!(!lockdown_active(&pool, 1).await.unwrap());
    let ows = vec![SavedOverwrite { target_id: 1, is_member: false, allow: 2048, deny: 0 }, SavedOverwrite { target_id: 9, is_member: true, allow: 0, deny: 2048 }];
    lockdown_save_channel(&pool, 1, 100, &ows, true).await.unwrap();
    lockdown_save_channel(&pool, 1, 101, &[], false).await.unwrap();
    assert!(lockdown_active(&pool, 1).await.unwrap());
    assert_eq!(lockdown_load(&pool, 1).await.unwrap(), vec![(100, true, ows), (101, false, vec![])]);
    lockdown_clear(&pool, 1).await.unwrap();
    assert!(!lockdown_active(&pool, 1).await.unwrap());
    assert!(lockdown_load(&pool, 1).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn quarantine_set_answers_keeps_row(pool: PgPool) {
    assert!(!quarantine_set_answers(&pool, 1, 5, &["a".into()]).await.unwrap());
    quarantine_insert(&pool, &qrow(1, 5)).await.unwrap();
    assert!(quarantine_set_answers(&pool, 1, 5, &["blue".into(), "x".into()]).await.unwrap());
    let r = quarantine_get(&pool, 1, 5).await.unwrap().unwrap();
    assert_eq!((r.answers, r.saved_roles), (Some(vec!["blue".into(), "x".into()]), vec![10, 11]));
}

#[sqlx::test(migrations = "./migrations")]
async fn lockdown_clear_one_channel(pool: PgPool) {
    let ows = vec![SavedOverwrite { target_id: 1, is_member: false, allow: 0, deny: 2048 }];
    lockdown_save_channel(&pool, 1, 100, &ows, false).await.unwrap();
    lockdown_save_channel(&pool, 1, 101, &ows, true).await.unwrap();
    lockdown_clear_channel(&pool, 1, 100).await.unwrap();
    assert_eq!(lockdown_load(&pool, 1).await.unwrap(), vec![(101, true, ows)]);
}

#[sqlx::test(migrations = "./migrations")]
async fn quarantine_gate_reason_and_switch(pool: PgPool) {
    let mut row = qrow(1, 5);
    row.reason = "gate".into();
    assert!(quarantine_insert(&pool, &row).await.unwrap());
    assert_eq!(quarantine_get(&pool, 1, 5).await.unwrap().unwrap().reason, "gate");
    row.user_id = 6;
    row.reason = "verification".into();
    quarantine_insert(&pool, &row).await.unwrap();
    assert!(!quarantine_set_reason(&pool, 1, 5, "verification", "raid").await.unwrap());
    assert!(quarantine_set_reason(&pool, 1, 6, "verification", "raid").await.unwrap());
    assert_eq!(quarantine_get(&pool, 1, 6).await.unwrap().unwrap().reason, "raid");
    row.user_id = 7;
    row.reason = "bogus".into();
    assert!(quarantine_insert(&pool, &row).await.is_err());
}

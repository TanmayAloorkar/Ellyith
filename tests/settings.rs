use aio_bot::moderation::ladder::{list_rules, remove_rule};
use aio_bot::settings::{get_or_create, save};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn first_get_creates_defaults_and_ladder(pool: PgPool) {
    let s = get_or_create(&pool, 1).await.unwrap();
    assert!(!s.warn_decay_enabled);
    assert_eq!(s.warn_decay_days, 30);
    assert_eq!(s.hoist_chars, "!");
    assert_eq!(list_rules(&pool, 1).await.unwrap().len(), 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn ladder_not_reseeded_after_admin_clears_it(pool: PgPool) {
    get_or_create(&pool, 1).await.unwrap();
    for n in [3, 5, 7] {
        assert!(remove_rule(&pool, 1, n).await.unwrap());
    }
    get_or_create(&pool, 1).await.unwrap();
    assert!(list_rules(&pool, 1).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn save_round_trips(pool: PgPool) {
    let mut s = get_or_create(&pool, 1).await.unwrap();
    s.mod_roles = vec![10, 11];
    s.warn_decay_enabled = true;
    s.warn_decay_days = 14;
    s.filtered_words = vec!["bad".into()];
    save(&pool, &s).await.unwrap();
    let r = get_or_create(&pool, 1).await.unwrap();
    assert_eq!(r.mod_roles, vec![10, 11]);
    assert!(r.warn_decay_enabled);
    assert_eq!(r.warn_decay_days, 14);
    assert_eq!(r.filtered_words, vec!["bad".to_string()]);
}

#[sqlx::test(migrations = "./migrations")]
async fn defaults_match_sql_defaults(pool: PgPool) {
    let s = get_or_create(&pool, 5).await.unwrap();
    let d = aio_bot::settings::GuildSettings::defaults(5);
    assert_eq!(format!("{s:?}"), format!("{d:?}"));
}

#[sqlx::test(migrations = "./migrations")]
async fn panel_ids_default_none_and_round_trip(pool: PgPool) {
    let mut s = get_or_create(&pool, 1).await.unwrap();
    assert_eq!((s.panel_channel_id, s.panel_message_id), (None, None));
    s.panel_channel_id = Some(1);
    s.panel_message_id = Some(2);
    save(&pool, &s).await.unwrap();
    let r = get_or_create(&pool, 1).await.unwrap();
    assert_eq!((r.panel_channel_id, r.panel_message_id), (Some(1), Some(2)));
}

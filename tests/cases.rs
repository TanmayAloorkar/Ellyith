use aio_bot::moderation::cases::{active_warn_count, create, history, ordinal, soft_remove, Actor, CaseKind, NewCase};
use sqlx::PgPool;

fn new(guild: i64, target: i64, kind: CaseKind) -> NewCase {
    NewCase {
        guild_id: guild,
        kind,
        target_id: target,
        actor: Actor::Moderator(1),
        reason: None,
        duration_secs: None,
        expires_at: None,
        dm_failed: false,
    }
}
fn warn(guild: i64, target: i64) -> NewCase {
    new(guild, target, CaseKind::Warn)
}

#[sqlx::test(migrations = "./migrations")]
async fn numbers_are_sequential_per_guild(pool: PgPool) {
    assert_eq!(create(&pool, &warn(1, 100)).await.unwrap().case_number, 1);
    assert_eq!(create(&pool, &warn(1, 100)).await.unwrap().case_number, 2);
    assert_eq!(create(&pool, &warn(2, 100)).await.unwrap().case_number, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn soft_remove_hides_from_count_but_not_history(pool: PgPool) {
    create(&pool, &warn(1, 100)).await.unwrap();
    create(&pool, &warn(1, 100)).await.unwrap();
    assert!(soft_remove(&pool, 1, 1, 555).await.unwrap().is_some());
    assert!(soft_remove(&pool, 1, 1, 555).await.unwrap().is_none()); // already removed
    assert_eq!(active_warn_count(&pool, 1, 100, None).await.unwrap(), 1);
    assert_eq!(history(&pool, 1, 100, true).await.unwrap().len(), 2);
    assert_eq!(history(&pool, 1, 100, false).await.unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn decay_excludes_old_warns(pool: PgPool) {
    create(&pool, &warn(1, 100)).await.unwrap();
    sqlx::query("UPDATE cases SET created_at = now() - interval '40 days' WHERE case_number = 1")
        .execute(&pool)
        .await
        .unwrap();
    create(&pool, &warn(1, 100)).await.unwrap();
    assert_eq!(active_warn_count(&pool, 1, 100, Some(30)).await.unwrap(), 1);
    assert_eq!(active_warn_count(&pool, 1, 100, None).await.unwrap(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_warns_count(pool: PgPool) {
    create(&pool, &warn(1, 100)).await.unwrap();
    create(&pool, &new(1, 100, CaseKind::Kick)).await.unwrap();
    assert_eq!(active_warn_count(&pool, 1, 100, None).await.unwrap(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn actor_and_fields_round_trip(pool: PgPool) {
    let mut c = new(1, 100, CaseKind::Ban);
    c.actor = Actor::AutoPunishment;
    c.reason = Some("spam".into());
    let made = create(&pool, &c).await.unwrap();
    let got = aio_bot::moderation::cases::get(&pool, 1, made.case_number).await.unwrap().unwrap();
    assert_eq!(got.actor, Actor::AutoPunishment);
    assert_eq!(got.kind, CaseKind::Ban);
    let edited = aio_bot::moderation::cases::set_reason(&pool, 1, 1, Some("x".into())).await.unwrap().unwrap();
    assert_eq!(edited.reason.as_deref(), Some("x"));
}

#[sqlx::test(migrations = "./migrations")]
async fn ordinal_counts_target_cases_including_removed(pool: PgPool) {
    let a = create(&pool, &warn(1, 100)).await.unwrap();
    create(&pool, &warn(1, 200)).await.unwrap();
    let c = create(&pool, &warn(1, 100)).await.unwrap();
    soft_remove(&pool, 1, a.case_number, 9).await.unwrap();
    assert_eq!(ordinal(&pool, 1, 100, c.case_number).await.unwrap(), 2);
    assert_eq!(ordinal(&pool, 1, 100, a.case_number).await.unwrap(), 1);
}

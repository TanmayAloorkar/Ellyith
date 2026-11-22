use aio_bot::moderation::jobs::{cancel, due, mark_done, schedule, JobKind};
use chrono::{Duration, Utc};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn due_returns_only_past_undone(pool: PgPool) {
    let now = Utc::now();
    schedule(&pool, 1, JobKind::Unban, 100, now - Duration::minutes(1)).await.unwrap();
    schedule(&pool, 1, JobKind::Unban, 101, now + Duration::hours(1)).await.unwrap();
    let d = due(&pool, now).await.unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].target_id, 100);
    mark_done(&pool, d[0].id).await.unwrap();
    assert!(due(&pool, now).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn cancel_removes_pending_for_target_only(pool: PgPool) {
    let past = Utc::now() - Duration::minutes(1);
    schedule(&pool, 1, JobKind::Unban, 100, past).await.unwrap();
    schedule(&pool, 1, JobKind::Unban, 101, past).await.unwrap();
    assert_eq!(cancel(&pool, 1, JobKind::Unban, 100).await.unwrap(), 1);
    let d = due(&pool, Utc::now()).await.unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].target_id, 101);
}

use aio_bot::moderation::cases::CaseKind;
use aio_bot::moderation::templates::{get, reset, set};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn set_get_reset(pool: PgPool) {
    assert_eq!(get(&pool, 1, CaseKind::Warn).await.unwrap(), None);
    set(&pool, 1, CaseKind::Warn, "a").await.unwrap();
    set(&pool, 1, CaseKind::Warn, "b").await.unwrap();
    assert_eq!(get(&pool, 1, CaseKind::Warn).await.unwrap().as_deref(), Some("b"));
    assert_eq!(get(&pool, 2, CaseKind::Warn).await.unwrap(), None);
    reset(&pool, 1, CaseKind::Warn).await.unwrap();
    assert_eq!(get(&pool, 1, CaseKind::Warn).await.unwrap(), None);
}

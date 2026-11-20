use aio_bot::logging::{channel_for, clear, list, set_channel, LogCategory};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn set_replace_clear_and_independent(pool: PgPool) {
    use LogCategory::*;
    assert_eq!(channel_for(&pool, 1, ModActions).await.unwrap(), None);
    set_channel(&pool, 1, ModActions, 10).await.unwrap();
    set_channel(&pool, 1, Messages, 20).await.unwrap();
    assert_eq!(channel_for(&pool, 1, ModActions).await.unwrap(), Some(10));
    set_channel(&pool, 1, ModActions, 11).await.unwrap();
    assert_eq!(channel_for(&pool, 1, ModActions).await.unwrap(), Some(11));
    assert_eq!(channel_for(&pool, 2, ModActions).await.unwrap(), None);
    assert_eq!(list(&pool, 1).await.unwrap().len(), 2);
    assert!(clear(&pool, 1, ModActions).await.unwrap());
    assert!(!clear(&pool, 1, ModActions).await.unwrap());
    assert_eq!(channel_for(&pool, 1, ModActions).await.unwrap(), None);
    assert_eq!(channel_for(&pool, 1, Messages).await.unwrap(), Some(20));
}

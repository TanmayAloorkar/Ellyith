use aio_bot::moderation::notes::{add, list, remove};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn add_list_remove(pool: PgPool) {
    let a = add(&pool, 1, 100, 7, "first").await.unwrap();
    let b = add(&pool, 1, 100, 7, "second").await.unwrap();
    add(&pool, 2, 100, 7, "other guild").await.unwrap();
    let l = list(&pool, 1, 100).await.unwrap();
    assert_eq!(l.iter().map(|n| n.content.as_str()).collect::<Vec<_>>(), ["second", "first"]);
    assert_eq!(l[0].id, b.id);
    assert!(!remove(&pool, 2, a.id).await.unwrap(), "other guild cannot delete");
    assert!(remove(&pool, 1, a.id).await.unwrap());
    assert!(!remove(&pool, 1, a.id).await.unwrap());
    assert_eq!(list(&pool, 1, 100).await.unwrap().len(), 1);
}

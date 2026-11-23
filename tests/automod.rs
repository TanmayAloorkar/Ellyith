use aio_bot::automod::rules::*;
use aio_bot::automod::store::*;
use sqlx::PgPool;

fn w(kind: WordKind, p: &str) -> WordEntry {
    WordEntry { kind, pattern: p.into() }
}

#[sqlx::test(migrations = "./migrations")]
async fn rules_default_off_and_save(pool: PgPool) {
    let rules = get_rules(&pool, 1).await.unwrap();
    assert_eq!(rules.len(), 13);
    assert!(rules.iter().all(|r| !r.enabled));
    let get = |k: RuleKind| rules.iter().find(|r| r.kind == k).unwrap().clone();
    let f = get(RuleKind::FastMessages);
    assert_eq!((f.threshold, f.window_secs, f.action, f.timeout_secs), (6, 5, RuleAction::Timeout, 600));
    let r = get(RuleKind::RepeatedMessages);
    assert_eq!((r.threshold, r.window_secs, r.percent), (3, 30, 80));
    assert_eq!(get(RuleKind::MassMentions).threshold, 5);
    assert_eq!(get(RuleKind::EmojiSpam).threshold, 10);
    assert_eq!(get(RuleKind::AllCaps).percent, 70);
    let fs = get(RuleKind::FileSpam);
    assert_eq!((fs.threshold, fs.window_secs), (5, 10));
    let wl = get(RuleKind::WallsOfText);
    assert_eq!((wl.threshold, wl.extra), (15, 1500));
    assert_eq!(get(RuleKind::SpoilerSpam).threshold, 5);
    let s = get(RuleKind::ScamLinks);
    assert_eq!((s.action, s.timeout_secs), (RuleAction::Timeout, 86_400));
    assert_eq!(get(RuleKind::BannedWords).action, RuleAction::DeleteWarn);
    assert_eq!(get(RuleKind::Links).action, RuleAction::Delete);

    let mut c = get(RuleKind::AllCaps);
    c.enabled = true;
    c.ignored_roles = vec![9];
    c.ignored_channels = vec![4, 5];
    save_rule(&pool, 1, &c).await.unwrap();
    assert_eq!(get_rules(&pool, 1).await.unwrap().iter().find(|r| r.kind == RuleKind::AllCaps).unwrap(), &c);
    assert!(get_rules(&pool, 2).await.unwrap().iter().all(|r| !r.enabled));
}

#[sqlx::test(migrations = "./migrations")]
async fn words_crud(pool: PgPool) {
    let added = add_words(&pool, 1, &[w(WordKind::Whole, "bad"), w(WordKind::Wildcard, "*scam*"), w(WordKind::Whole, "bad")]).await.unwrap();
    assert_eq!(added, 2);
    assert_eq!(list_words(&pool, 1).await.unwrap().len(), 2);
    assert_eq!(remove_words(&pool, 1, &[w(WordKind::Whole, "bad"), w(WordKind::Whole, "nope")]).await.unwrap(), 1);
    assert_eq!(list_words(&pool, 1).await.unwrap(), vec![w(WordKind::Wildcard, "*scam*")]);
    assert!(list_words(&pool, 2).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn sites_crud(pool: PgPool) {
    assert!(add_site(&pool, 1, "youtube.com").await.unwrap());
    assert!(!add_site(&pool, 1, "youtube.com").await.unwrap());
    assert_eq!(list_sites(&pool, 1).await.unwrap(), vec!["youtube.com".to_string()]);
    assert!(remove_site(&pool, 1, "youtube.com").await.unwrap());
    assert!(!remove_site(&pool, 1, "youtube.com").await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn scam_domains_replace(pool: PgPool) {
    save_scam_domains(&pool, &["a.com".into(), "b.com".into()]).await.unwrap();
    save_scam_domains(&pool, &["c.com".into(), "c.com".into()]).await.unwrap();
    assert_eq!(load_scam_domains(&pool).await.unwrap(), vec!["c.com".to_string()]);
}

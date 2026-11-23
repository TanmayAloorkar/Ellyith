//! Daily-refreshed scam domain list, kept in the DB and in `AutoMod`.
use crate::automod::store;
use crate::Data;
use std::collections::HashSet;
use std::time::Duration;

// Discord-AntiScam "scam-links": community-maintained JSON array of phishing domains.
const SOURCE: &str = "https://raw.githubusercontent.com/Discord-AntiScam/scam-links/main/list.json";
const MIN_DOMAINS: usize = 100;

/// The source is a JSON array of domain strings; anything else (an HTML error page, a moved file) is None.
/// Entries without a dot or with spaces are dropped.
pub fn parse_list(body: &str) -> Option<HashSet<String>> {
    let items = serde_json::from_str::<Vec<String>>(body).ok()?;
    Some(items.iter().map(|s| s.trim().to_lowercase()).filter(|s| s.contains('.') && !s.contains(char::is_whitespace)).collect())
}

/// Guards against an empty or truncated download wiping the last good copy.
pub fn accept(new: &HashSet<String>) -> bool {
    new.len() >= MIN_DOMAINS
}

async fn fetch() -> Result<String, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("aio-bot")
        .build()?
        .get(SOURCE)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
}

pub async fn refresh(data: &Data) {
    let new = match fetch().await.map(|b| parse_list(&b)) {
        Ok(Some(set)) => set,
        Ok(None) => return tracing::warn!("scam list rejected: not a JSON array of domains"),
        Err(e) => return tracing::warn!("scam list download failed: {e}"),
    };
    if !accept(&new) {
        return tracing::warn!("scam list rejected: only {} domains", new.len());
    }
    let list: Vec<String> = new.iter().cloned().collect();
    if let Err(e) = store::save_scam_domains(&data.db, &list).await {
        tracing::warn!("scam list save failed: {e}");
    }
    data.automod.set_scam(new);
}

/// Startup: load the DB copy, then refresh now and every 24 h.
pub async fn run(data: Data) {
    match store::load_scam_domains(&data.db).await {
        Ok(v) => data.automod.set_scam(v.into_iter().collect()),
        Err(e) => tracing::warn!("scam list load failed: {e}"),
    }
    loop {
        refresh(&data).await;
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_formats() {
        let j = parse_list(r#"["A.com", " b.org ", "nodot", "a b.com"]"#);
        assert_eq!(j, Some(HashSet::from(["a.com".to_string(), "b.org".to_string()])));
        assert_eq!(parse_list("# c\n\n  Evil.COM \nb.xyz # x\n"), None);
        assert_eq!(parse_list("<!DOCTYPE html>\n<html><body>404: Not Found</body></html>"), None);
        assert_eq!(parse_list(r#"{"domains": ["a.com"]}"#), None);
    }

    #[test]
    fn small_list_rejected() {
        let small: HashSet<String> = (0..99).map(|i| format!("d{i}.com")).collect();
        assert!(!accept(&small));
        let ok: HashSet<String> = (0..100).map(|i| format!("d{i}.com")).collect();
        assert!(accept(&ok));
    }
}

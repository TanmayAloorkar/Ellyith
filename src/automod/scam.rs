//! Daily-refreshed scam domain list, kept in the DB and in `AutoMod`.
use crate::automod::store;
use crate::Data;
use std::collections::HashSet;
use std::time::Duration;

// Discord-AntiScam "scam-links": community-maintained JSON array of phishing domains.
const SOURCE: &str = "https://raw.githubusercontent.com/Discord-AntiScam/scam-links/main/list.json";
const MIN_DOMAINS: usize = 100;

/// One domain per line (`#` comments, blanks skipped) or a JSON array of strings.
pub fn parse_list(body: &str) -> HashSet<String> {
    let items: Vec<String> = match serde_json::from_str::<Vec<String>>(body) {
        Ok(v) => v,
        Err(_) => body
            .lines()
            .map(|l| l.split('#').next().unwrap_or(""))
            .map(str::to_string)
            .collect(),
    };
    items.iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
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
    let new = match fetch().await {
        Ok(body) => parse_list(&body),
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
        let s = parse_list("# c\n\n  Evil.COM \nb.xyz # x\n");
        assert_eq!(s, HashSet::from(["evil.com".to_string(), "b.xyz".to_string()]));
        let j = parse_list(r#"["A.com", " b.org "]"#);
        assert_eq!(j, HashSet::from(["a.com".to_string(), "b.org".to_string()]));
    }

    #[test]
    fn small_list_rejected() {
        let small: HashSet<String> = (0..99).map(|i| format!("d{i}.com")).collect();
        assert!(!accept(&small));
        let ok: HashSet<String> = (0..100).map(|i| format!("d{i}.com")).collect();
        assert!(accept(&ok));
    }
}

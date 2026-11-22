use crate::UserError;
use chrono::Duration;

/// Discord's maximum timeout.
pub const MAX_TIMEOUT_SECS: i64 = 28 * 24 * 3600;

/// Parse forms like `30m`, `1h30m`, `2D`, `1w`. Zero or overflow is an error.
pub fn parse_duration(s: &str) -> Result<Duration, UserError> {
    let err = || UserError(format!("Invalid duration \"{s}\". Use forms like 30m, 2h, 1d12h, 1w."));
    let mut rest = s.trim();
    let mut total: i64 = 0;
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let n: i64 = rest[..digits].parse().map_err(|_| err())?;
        let mut tail = rest[digits..].chars();
        let mult = match tail.next().map(|c| c.to_ascii_lowercase()) {
            Some('s') => 1,
            Some('m') => 60,
            Some('h') => 3600,
            Some('d') => 86_400,
            Some('w') => 604_800,
            _ => return Err(err()),
        };
        total = n.checked_mul(mult).and_then(|v| total.checked_add(v)).ok_or_else(err)?;
        rest = tail.as_str();
    }
    if total == 0 {
        return Err(err());
    }
    Duration::try_seconds(total).ok_or_else(err)
}

/// Format as e.g. `1h 30m`, `7d`.
pub fn format_duration(d: Duration) -> String {
    let t = d.num_seconds();
    let parts: Vec<String> = [(t / 86_400, 'd'), (t % 86_400 / 3600, 'h'), (t % 3600 / 60, 'm'), (t % 60, 's')]
        .into_iter()
        .filter(|(n, _)| *n != 0)
        .map(|(n, u)| format!("{n}{u}"))
        .collect();
    if parts.is_empty() { "0s".into() } else { parts.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parses_units() {
        assert_eq!(parse_duration("90s").unwrap().num_seconds(), 90);
        assert_eq!(parse_duration("1h30m").unwrap().num_seconds(), 5400);
        assert_eq!(parse_duration("2D").unwrap().num_seconds(), 172_800);
        assert_eq!(parse_duration("1w").unwrap().num_seconds(), 604_800);
    }
    #[test] fn rejects_bad() {
        for s in ["", "0m", "10", "5x", "h", "-1h", "99999999999999w"] { assert!(parse_duration(s).is_err(), "{s}"); }
    }
    #[test] fn formats() {
        assert_eq!(format_duration(chrono::Duration::seconds(5400)), "1h 30m");
        assert_eq!(format_duration(chrono::Duration::days(7)), "7d");
    }
}

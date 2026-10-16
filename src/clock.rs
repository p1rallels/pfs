use std::time::Duration;

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// RFC 3339, UTC, always 3 fractional digits, so timestamps sort as strings.
pub fn now() -> String {
    let now = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.millisecond()
    )
}

pub fn since(ts: &str) -> Option<Duration> {
    let then = OffsetDateTime::parse(ts, &Rfc3339).ok()?;
    let elapsed = OffsetDateTime::now_utc() - then;
    Duration::try_from(elapsed).ok()
}

pub fn human(duration: Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

pub fn ago(ts: &str) -> String {
    since(ts).map(human).unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_secs(5)), "5s");
        assert_eq!(human(Duration::from_secs(7 * 60 + 3)), "7m");
        assert_eq!(human(Duration::from_secs(2 * 3600)), "2h");
        assert_eq!(human(Duration::from_secs(3 * 86400)), "3d");
    }

    #[test]
    fn now_round_trips() {
        assert!(since(&now()).is_some());
    }

    #[test]
    fn now_has_fixed_width() {
        let ts = now();
        assert_eq!(ts.len(), 24);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[19..20], ".");
    }
}

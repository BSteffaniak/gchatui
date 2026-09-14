//! Presentation-only timestamp formatting. Call before rendering, since local
//! timezone discovery can consult operating-system configuration.
use chrono::{DateTime, FixedOffset, Local};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub enum ClockFormat {
    #[serde(rename = "12h")]
    TwelveHour,
    #[default]
    #[serde(rename = "24h")]
    TwentyFourHour,
}

#[derive(Debug, Clone)]
pub struct TimestampFormat(String);

impl TimestampFormat {
    pub fn parse(value: String) -> Result<Self, &'static str> {
        use chrono::format::{Item, StrftimeItems};
        if value.trim().is_empty()
            || value.chars().any(char::is_control)
            || StrftimeItems::new(&value).any(|item| matches!(item, Item::Error))
        {
            return Err(
                "timestamp_format must be a nonempty, valid Chrono strftime format without control characters",
            );
        }
        let sample =
            DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("static timestamp");
        if sample
            .format(&value)
            .to_string()
            .chars()
            .any(char::is_control)
        {
            return Err(
                "timestamp_format must not produce control characters (including %n or %t)",
            );
        }
        Ok(Self(value))
    }

    pub fn format_local(&self, timestamp: &str) -> String {
        DateTime::parse_from_rfc3339(timestamp).map_or_else(
            |_| timestamp.to_string(),
            |date| date.with_timezone(&Local).format(&self.0).to_string(),
        )
    }
}

impl ClockFormat {
    pub fn format_local(self, timestamp: &str) -> String {
        DateTime::parse_from_rfc3339(timestamp).map_or_else(
            |_| timestamp.to_string(),
            |date| self.format(date.with_timezone(&Local).fixed_offset()),
        )
    }

    fn format(self, date: DateTime<FixedOffset>) -> String {
        date.format(match self {
            Self::TwelveHour => "%b %-d, %Y · %-I:%M %p",
            Self::TwentyFourHour => "%b %-d, %Y · %H:%M",
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_formats_validate_and_preserve_invalid_timestamp_fallback() {
        for invalid in ["", " ", "%", "%Q", "%n", "%t", "hello\nworld"] {
            assert!(
                TimestampFormat::parse(invalid.into()).is_err(),
                "{invalid:?}"
            );
        }
        let format = TimestampFormat::parse("%Y/%m/%d %-I:%M %p %%".into()).unwrap();
        let date = DateTime::parse_from_rfc3339("2026-09-14T18:30:00Z").unwrap();
        assert_eq!(date.format(&format.0).to_string(), "2026/09/14 6:30 PM %");
        assert_eq!(format.format_local("unknown"), "unknown");
    }

    #[test]
    fn midnight_noon_and_fractional_seconds() {
        for (input, twelve, twenty_four) in [
            (
                "2026-09-14T00:05:00Z",
                "Sep 14, 2026 · 12:05 AM",
                "Sep 14, 2026 · 00:05",
            ),
            (
                "2026-09-14T12:30:59.123Z",
                "Sep 14, 2026 · 12:30 PM",
                "Sep 14, 2026 · 12:30",
            ),
            (
                "2026-09-14T23:59:00-04:00",
                "Sep 14, 2026 · 11:59 PM",
                "Sep 14, 2026 · 23:59",
            ),
        ] {
            let date = DateTime::parse_from_rfc3339(input).unwrap();
            assert_eq!(ClockFormat::TwelveHour.format(date), twelve);
            assert_eq!(ClockFormat::TwentyFourHour.format(date), twenty_four);
        }
        assert_eq!(
            ClockFormat::TwentyFourHour.format_local("unknown"),
            "unknown"
        );
    }
}

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

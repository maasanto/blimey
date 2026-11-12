//! Durations as written in show files: `500ms`, `2s`.

use std::time::Duration;

use serde::Deserialize;

/// A cue duration. Musical units (beats, bars) are a variant each, resolved against the tempo.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub enum Length {
    Time(Duration),
}

impl Length {
    pub fn duration(self) -> Duration {
        match self {
            Length::Time(duration) => duration,
        }
    }
}

impl TryFrom<String> for Length {
    type Error = String;

    fn try_from(text: String) -> Result<Length, String> {
        let invalid =
            || format!("duration {text:?} must be a number then ms or s, e.g. 500ms or 2s");
        let unit_start = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(unit_start);
        let number: f64 = number.parse().map_err(|_| invalid())?;
        let seconds = match unit.trim() {
            "ms" => number / 1000.0,
            "s" => number,
            _ => return Err(invalid()),
        };
        Ok(Length::Time(Duration::from_secs_f64(seconds)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Duration, String> {
        Length::try_from(text.to_string()).map(Length::duration)
    }

    #[test]
    fn reads_milliseconds_and_seconds() {
        assert_eq!(parse("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse("1.5 s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse("0s"), Ok(Duration::ZERO));
    }

    #[test]
    fn rejects_missing_or_unknown_units() {
        let message = "duration \"2\" must be a number then ms or s, e.g. 500ms or 2s";
        assert_eq!(parse("2"), Err(message.into()));
        assert!(parse("2 minutes").is_err());
        assert!(parse("-1s").is_err());
        assert!(parse("s").is_err());
    }
}

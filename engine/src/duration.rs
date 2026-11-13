//! Durations and time signatures as written in show files: `500ms`, `2s`, `0.5 beats`,
//! `4 bars`; `4/4`, `6/8`.
//!
//! A beat is a quarter note, the unit of MIDI clock (24 ticks per beat) and of Reaper's BPM,
//! so a bar of 6/8 is 3 beats. `ClockFollower::progress` measures a `Length` as it runs.

use std::time::Duration;

use serde::Deserialize;

/// A cue duration: a time, or beats and bars that follow the tempo while they run.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub enum Length {
    Time(Duration),
    Beats(f64),
    Bars(f64),
}

impl TryFrom<String> for Length {
    type Error = String;

    fn try_from(text: String) -> Result<Length, String> {
        let invalid = || {
            format!(
                "duration {text:?} must be a number then ms, s, beats or bars, e.g. 500ms, 2s, 0.5 beats or 4 bars"
            )
        };
        let unit_start = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(unit_start);
        let number: f64 = number.parse().map_err(|_| invalid())?;
        if !number.is_finite() {
            return Err(invalid());
        }
        let seconds = |seconds| Duration::try_from_secs_f64(seconds).map_err(|_| invalid());
        match unit.trim() {
            "ms" => Ok(Length::Time(seconds(number / 1000.0)?)),
            "s" => Ok(Length::Time(seconds(number)?)),
            "beat" | "beats" => Ok(Length::Beats(number)),
            "bar" | "bars" => Ok(Length::Bars(number)),
            _ => Err(invalid()),
        }
    }
}

/// A song's time signature. Defaults to 4/4.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    /// Quarter-note beats per bar: 4 in 4/4, 3 in 6/8.
    pub fn beats_per_bar(self) -> f64 {
        f64::from(self.numerator) * 4.0 / f64::from(self.denominator)
    }
}

impl TryFrom<String> for TimeSignature {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let invalid = || {
            format!(
                "time signature {text:?} must be beats/note value with a power of two up to 32 below, e.g. 4/4 or 6/8"
            )
        };
        let (numerator, denominator) = text.split_once('/').ok_or_else(invalid)?;
        let numerator: u8 = numerator.trim().parse().map_err(|_| invalid())?;
        let denominator: u8 = denominator.trim().parse().map_err(|_| invalid())?;
        if numerator == 0 || !denominator.is_power_of_two() || denominator > 32 {
            return Err(invalid());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Length, String> {
        Length::try_from(text.to_string())
    }

    #[test]
    fn reads_every_unit_with_or_without_a_space() {
        assert_eq!(parse("500ms"), Ok(Length::Time(Duration::from_millis(500))));
        assert_eq!(
            parse("1.5 s"),
            Ok(Length::Time(Duration::from_millis(1500)))
        );
        assert_eq!(parse("0s"), Ok(Length::Time(Duration::ZERO)));
        assert_eq!(parse("0.5 beats"), Ok(Length::Beats(0.5)));
        assert_eq!(parse("1 beat"), Ok(Length::Beats(1.0)));
        assert_eq!(parse("4 bars"), Ok(Length::Bars(4.0)));
        assert_eq!(parse("1bar"), Ok(Length::Bars(1.0)));
    }

    #[test]
    fn rejects_missing_or_unknown_units() {
        let message = "duration \"2\" must be a number then ms, s, beats or bars, e.g. 500ms, 2s, 0.5 beats or 4 bars";
        assert_eq!(parse("2"), Err(message.into()));
        for text in ["2 minutes", "-1s", "s", "bars", "", "4 bars x", "1e3 s"] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn rejects_numbers_too_large_for_a_duration_instead_of_panicking() {
        let too_long = format!("{}s", "9".repeat(400));
        for text in [
            "99999999999999999999s",
            &too_long,
            "99999999999999999999999ms",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert!(parse(&format!("{} beats", "9".repeat(400))).is_err());
    }

    #[test]
    fn a_bar_counts_quarter_note_beats_from_the_time_signature() {
        let time_signature = |text: &str| TimeSignature::try_from(text.to_string()).unwrap();

        assert_eq!(TimeSignature::default().beats_per_bar(), 4.0);
        assert_eq!(time_signature("6/8").beats_per_bar(), 3.0);
        assert_eq!(time_signature("7/8").beats_per_bar(), 3.5);
        for text in ["4", "0/4", "4/3", "4/64", "x/4"] {
            assert!(TimeSignature::try_from(text.to_string()).is_err(), "{text}");
        }
    }
}

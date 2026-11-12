//! Envelope shapes (plan §8): level as a pure function of normalised time.
//!
//! Swells (C4), pattern hits (C5) and pixel scenes (C9) all evaluate the same shapes.

use std::f64::consts::TAU;

use serde::Deserialize;

/// In show files: `ramp-up`, `ramp-down`, `ease-in`, `sine`, `hold`, or
/// `{ points: [[0, 0], [0.5, 1], [1, 0]] }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "EnvelopeEntry")]
pub enum Envelope {
    RampUp,
    RampDown,
    /// Ramp up with a slow start: the plan's `curve: ease-in`.
    EaseIn,
    /// One smooth pulse: up from 0, full at the midpoint, back to 0.
    Sine,
    Hold,
    /// `(time, level)` pairs with rising times, both 0–1, joined by straight lines. Before the
    /// first point and after the last, the nearest point's level holds.
    Points(Vec<(f64, f64)>),
}

/// serde_yaml_ng reads an enum variant with data only from a YAML tag (`!points`), so the
/// written form goes through this.
#[derive(Deserialize)]
#[serde(
    untagged,
    expecting = "a curve: ramp-up, ramp-down, ease-in, sine, hold, or { points: [[time, level], …] }"
)]
enum EnvelopeEntry {
    Named(NamedEnvelope),
    Points { points: Vec<(f64, f64)> },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum NamedEnvelope {
    RampUp,
    RampDown,
    EaseIn,
    Sine,
    Hold,
}

impl TryFrom<EnvelopeEntry> for Envelope {
    type Error = String;

    fn try_from(entry: EnvelopeEntry) -> Result<Self, String> {
        let points = match entry {
            EnvelopeEntry::Named(NamedEnvelope::RampUp) => return Ok(Self::RampUp),
            EnvelopeEntry::Named(NamedEnvelope::RampDown) => return Ok(Self::RampDown),
            EnvelopeEntry::Named(NamedEnvelope::EaseIn) => return Ok(Self::EaseIn),
            EnvelopeEntry::Named(NamedEnvelope::Sine) => return Ok(Self::Sine),
            EnvelopeEntry::Named(NamedEnvelope::Hold) => return Ok(Self::Hold),
            EnvelopeEntry::Points { points } => points,
        };
        let within_0_to_1 = |value: f64| (0.0..=1.0).contains(&value);
        if points.is_empty() {
            return Err("points: give at least one [time, level] pair".into());
        }
        if !points
            .iter()
            .all(|&(time, level)| within_0_to_1(time) && within_0_to_1(level))
        {
            return Err("points: times and levels must be within 0-1".into());
        }
        if !points.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return Err("points: times must rise from one point to the next".into());
        }
        Ok(Self::Points(points))
    }
}

impl Envelope {
    /// The level (0–1) at `progress` through the envelope; `progress` is clamped to 0–1.
    pub fn level(&self, progress: f64) -> f64 {
        let progress = progress.clamp(0.0, 1.0);
        match self {
            Self::RampUp => progress,
            Self::RampDown => 1.0 - progress,
            Self::EaseIn => progress * progress,
            Self::Sine => (1.0 - (TAU * progress).cos()) / 2.0,
            Self::Hold => 1.0,
            Self::Points(points) => interpolate(points, progress),
        }
    }
}

fn interpolate(points: &[(f64, f64)], progress: f64) -> f64 {
    let after = points.partition_point(|&(time, _)| time <= progress);
    match (
        after.checked_sub(1).map(|index| points[index]),
        points.get(after),
    ) {
        (None, None) => 0.0,
        (None, Some(&(_, level))) | (Some((_, level)), None) => level,
        (Some((time_0, level_0)), Some(&(time_1, level_1))) => {
            level_0 + (level_1 - level_0) * (progress - time_0) / (time_1 - time_0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(envelope: &Envelope) -> [f64; 5] {
        [0.0, 0.25, 0.5, 0.75, 1.0].map(|progress| envelope.level(progress))
    }

    fn assert_close(actual: [f64; 5], expected: [f64; 5]) {
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-12,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn ramps_go_linearly_between_off_and_full() {
        assert_close(levels(&Envelope::RampUp), [0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_close(levels(&Envelope::RampDown), [1.0, 0.75, 0.5, 0.25, 0.0]);
    }

    #[test]
    fn ease_in_starts_slow_and_ends_full() {
        assert_close(levels(&Envelope::EaseIn), [0.0, 0.0625, 0.25, 0.5625, 1.0]);
    }

    #[test]
    fn sine_is_one_pulse_peaking_at_the_midpoint() {
        assert_close(levels(&Envelope::Sine), [0.0, 0.5, 1.0, 0.5, 0.0]);
    }

    #[test]
    fn hold_stays_full() {
        assert_close(levels(&Envelope::Hold), [1.0; 5]);
    }

    #[test]
    fn custom_points_join_with_lines_and_hold_their_ends() {
        let envelope = Envelope::Points(vec![(0.25, 0.2), (0.5, 1.0), (0.75, 0.6)]);

        assert_close(levels(&envelope), [0.2, 0.2, 1.0, 0.6, 0.6]);
        assert!((envelope.level(0.625) - 0.8).abs() < 1e-12);
    }

    fn parse(yaml: &str) -> Result<Envelope, String> {
        serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())
    }

    #[test]
    fn reads_shape_names_and_points_from_yaml() {
        assert_eq!(parse("ease-in"), Ok(Envelope::EaseIn));
        assert_eq!(parse("ramp-down"), Ok(Envelope::RampDown));
        assert_eq!(
            parse("{ points: [[0, 0], [0.5, 1], [1, 0]] }"),
            Ok(Envelope::Points(vec![(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)]))
        );
    }

    #[test]
    fn rejects_unknown_shapes_and_points_out_of_range_or_order() {
        assert_eq!(
            parse("swell-up"),
            Err("a curve: ramp-up, ramp-down, ease-in, sine, hold, or { points: [[time, level], …] }".into())
        );
        assert_eq!(
            parse("{ points: [] }"),
            Err("points: give at least one [time, level] pair".into())
        );
        assert_eq!(
            parse("{ points: [[0, 0], [1, 1.5]] }"),
            Err("points: times and levels must be within 0-1".into())
        );
        assert_eq!(
            parse("{ points: [[0.5, 0], [0.5, 1]] }"),
            Err("points: times must rise from one point to the next".into())
        );
    }

    #[test]
    fn progress_outside_0_to_1_is_clamped() {
        assert_eq!(Envelope::RampUp.level(-0.5), 0.0);
        assert_eq!(Envelope::RampUp.level(1.5), 1.0);
    }
}

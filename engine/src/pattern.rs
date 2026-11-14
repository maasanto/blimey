//! Pattern cues (plan §8): envelope hits on the beats of a 1–4 bar loop that starts on a bar
//! line and repeats until released.
//!
//! `Pattern::level` is the pattern's output as a 0–1 intensity, for anything that follows one.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::Show;
use crate::clock_follower::{ClockFollower, Moment};
use crate::cue::{INSTANT, PixelCues, resolve_intensity, resolve_look};
use crate::duration::{Length, TimeSignature};
use crate::envelope::Envelope;
use crate::look::{Level, Look};

/// A note this close after a bar line starts the loop at once, on that bar line, instead of
/// waiting for the next one. One MIDI clock tick, the finest step the clock places a note on:
/// 18–27 ms at 140–92 BPM, under one frame, so the first hit loses at most a frame of light.
const START_WINDOW_BEATS: f64 = 1.0 / 24.0;
/// Hits sit on a grid of quarter beats.
const STEPS_PER_BEAT: f64 = 4.0;
const MAX_BARS: f64 = 4.0;

/// A pattern cue as written in a song file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PatternEntry {
    name: String,
    length: Length,
    target: Mapping,
    hits: Vec<HitEntry>,
    release: Option<Length>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HitEntry {
    at: f64,
    shape: Envelope,
    length: Length,
    level: Level,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub name: String,
    /// Loop length in beats.
    pub beats: f64,
    pub target: PatternTarget,
    pub hits: Vec<Hit>,
    pub release: Length,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternTarget {
    /// `{ all-wash: dimmer }`: the level scales these fixtures' intensity, dark between hits.
    Dimmer(BTreeSet<String>),
    /// `{ all-wash: { color: red } }`: the level is how much of the look shows over the layers
    /// below, as a swell's curve is.
    Look(Look),
    /// `{ bridge-chase: intensity }`: the level scales the intensity of the pixel cues on these
    /// notes, dark between hits.
    Intensity(BTreeSet<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Beats from the start of the loop: `at` 1 is 0.
    pub offset: f64,
    pub beats: f64,
    pub shape: Envelope,
    pub level: f64,
}

impl PatternEntry {
    pub(crate) fn resolve(
        self,
        show: &Show,
        time_signature: TimeSignature,
        pixel_cues: &PixelCues,
    ) -> Result<Pattern, String> {
        let beats_per_bar = time_signature.beats_per_bar();
        let bars = match self.length {
            Length::Bars(bars) if bars.fract() == 0.0 && (1.0..=MAX_BARS).contains(&bars) => bars,
            _ => return Err("length must be 1, 2, 3 or 4 bars".into()),
        };
        let beats = bars * beats_per_bar;
        let hits = self
            .hits
            .into_iter()
            .enumerate()
            .map(|(index, hit)| {
                hit.resolve(beats, beats_per_bar)
                    .map_err(|error| format!("hit {}: {error}", index + 1))
            })
            .collect::<Result<_, _>>()?;
        Ok(Pattern {
            target: resolve_target(show, &self.target, pixel_cues)?,
            name: self.name,
            beats,
            hits,
            release: self.release.unwrap_or(INSTANT),
        })
    }
}

impl HitEntry {
    fn resolve(self, pattern_beats: f64, beats_per_bar: f64) -> Result<Hit, String> {
        let offset = self.at - 1.0;
        if !((0.0..pattern_beats).contains(&offset) && (offset * STEPS_PER_BEAT).fract() == 0.0) {
            return Err(format!(
                "at {} must be a beat of the pattern from 1 to {} in quarter beats, e.g. 1, 2.5 or 3.75",
                self.at,
                pattern_beats + 1.0 - 1.0 / STEPS_PER_BEAT
            ));
        }
        let beats = match self.length {
            Length::Beats(beats) => beats,
            Length::Bars(bars) => bars * beats_per_bar,
            Length::Time(_) => {
                return Err("length must be in beats or bars, e.g. 0.5 beats".into());
            }
        };
        if !(beats > 0.0 && beats <= pattern_beats) {
            return Err(format!(
                "length must be above 0 and at most the pattern's {pattern_beats} beats"
            ));
        }
        Ok(Hit {
            offset,
            beats,
            shape: self.shape,
            level: f64::from(self.level.0),
        })
    }
}

fn resolve_target(
    show: &Show,
    target: &Mapping,
    pixel_cues: &PixelCues,
) -> Result<PatternTarget, String> {
    if let Some(notes) = resolve_intensity(target, pixel_cues) {
        return notes.map(PatternTarget::Intensity);
    }
    if target
        .values()
        .all(|value| value.as_str() == Some("dimmer"))
    {
        let mut fixtures = BTreeSet::new();
        for name in target.keys() {
            let name = name.as_str().ok_or("keys must be fixture or group names")?;
            let resolved = show
                .rig
                .resolve(name)
                .map_err(|error| format!("{name}: {error}"))?;
            fixtures.extend(resolved.into_iter().map(|fixture| fixture.name.clone()));
        }
        return Ok(PatternTarget::Dimmer(fixtures));
    }
    if target.values().any(|value| value.is_string()) {
        return Err(
            "target: give either `dimmer` or look settings such as { color: red } to every fixture or group, or `intensity` to every pixel cue"
                .into(),
        );
    }
    resolve_look(show, target).map(PatternTarget::Look)
}

impl Pattern {
    /// Where the loop of a note at `at` starts: the next bar line, or the bar line just passed
    /// if the note is within `START_WINDOW_BEATS` of it. Only `beats` places the loop, and it
    /// may be ahead of `at`.
    pub fn start(clock: &ClockFollower, at: Duration) -> Moment {
        let into_bar = clock.bar_position(at).beat - 1.0;
        let bar_line = clock.beats(at) - into_bar;
        Moment {
            at,
            beats: if into_bar <= START_WINDOW_BEATS {
                bar_line
            } else {
                bar_line + clock.time_signature().beats_per_bar()
            },
        }
    }

    /// The level, 0–1, at `at` of the loop started at `start` (see `start`): the highest of the
    /// hits sounding then. `None` before the loop starts.
    pub fn level(&self, clock: &ClockFollower, start: Moment, at: Duration) -> Option<f64> {
        let elapsed = clock.beats(at) - start.beats;
        (elapsed >= 0.0).then(|| self.level_after(elapsed))
    }

    /// The level, 0–1, `elapsed` beats (0 or more) after the loop started.
    pub fn level_after(&self, elapsed: f64) -> f64 {
        let position = elapsed % self.beats;
        self.hits
            .iter()
            .map(|hit| {
                // Beats since this hit last started, in this pass or, while its start is still
                // ahead in this pass, the one before, so a hit running over the loop's end
                // carries on into the next pass.
                let mut since = position - hit.offset;
                if since < 0.0 {
                    since += self.beats;
                }
                if since < hit.beats && since <= elapsed {
                    hit.level * hit.shape.level(since / hit.beats)
                } else {
                    0.0
                }
            })
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cue::Cue;
    use crate::song::Song;
    use std::path::Path;

    /// No MIDI clock, so beats run at the song's 120 BPM: 2 a second, a 4/4 bar every 2 s.
    fn clock() -> ClockFollower {
        ClockFollower::new(120.0, TimeSignature::default())
    }

    fn seconds(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    /// The level `beats` into the loop of `pattern` started at 0.
    fn level_at(pattern: &Pattern, beats: f64) -> f64 {
        let clock = clock();
        let start = Moment {
            at: Duration::ZERO,
            beats: 0.0,
        };
        pattern
            .level(&clock, start, seconds(beats / 2.0))
            .expect("the loop started at 0")
    }

    fn hit(at: f64, beats: f64, shape: Envelope, level: f64) -> Hit {
        Hit {
            offset: at - 1.0,
            beats,
            shape,
            level,
        }
    }

    fn one_bar(hits: Vec<Hit>) -> Pattern {
        Pattern {
            name: "pattern".into(),
            beats: 4.0,
            target: PatternTarget::Dimmer(BTreeSet::new()),
            hits,
            release: INSTANT,
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }

    #[test]
    fn a_note_just_before_a_bar_line_waits_for_it_and_one_just_after_starts_at_once() {
        let clock = clock();
        let pattern = one_bar(vec![hit(1.0, 0.5, Envelope::RampUp, 1.0)]);
        let bar_2 = 2.0;

        let early = Pattern::start(&clock, seconds(bar_2 - 0.010));
        assert_close(early.beats, 4.0);
        assert_eq!(pattern.level(&clock, early, seconds(bar_2 - 0.001)), None);
        assert_close(pattern.level(&clock, early, seconds(bar_2)).unwrap(), 0.0);

        let late = Pattern::start(&clock, seconds(bar_2 + 0.010));
        assert_close(late.beats, 4.0);
        let at_note = pattern.level(&clock, late, seconds(bar_2 + 0.010)).unwrap();
        assert_close(at_note, 0.02 / 0.5);

        let mid_bar = Pattern::start(&clock, seconds(bar_2 + 0.5));
        assert_close(mid_bar.beats, 8.0);
    }

    #[test]
    fn the_loop_repeats_without_a_seam_and_a_hit_over_its_end_carries_on() {
        let pattern = one_bar(vec![
            hit(1.0, 0.5, Envelope::Sine, 0.6),
            hit(4.75, 0.5, Envelope::RampUp, 1.0),
        ]);

        // The wrapping hit has not played yet in the first pass.
        assert_close(level_at(&pattern, 0.1), 0.6 * Envelope::Sine.level(0.2));
        for bar_line in [4.0, 8.0, 12.0] {
            assert_close(level_at(&pattern, bar_line - 0.001), 0.498);
            assert_close(level_at(&pattern, bar_line), 0.5);
            assert_close(level_at(&pattern, bar_line + 0.001), 0.502);
        }
        for step in 0..64 {
            let position = f64::from(step) / 16.0;
            assert_close(
                level_at(&pattern, 12.0 + position),
                level_at(&pattern, 4.0 + position),
            );
        }
    }

    #[test]
    fn overlapping_hits_take_the_highest_value() {
        let pattern = one_bar(vec![
            hit(1.0, 1.0, Envelope::Hold, 0.5),
            hit(1.5, 1.0, Envelope::RampUp, 1.0),
        ]);

        assert_close(level_at(&pattern, 0.25), 0.5);
        assert_close(level_at(&pattern, 0.75), 0.5);
        assert_close(level_at(&pattern, 0.95), 0.5);
        assert_close(level_at(&pattern, 1.25), 0.75);
        assert_close(level_at(&pattern, 2.0), 0.0);
    }

    fn show() -> Show {
        Show::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/cues"
        )))
        .unwrap()
    }

    fn parse(cue: &str) -> Result<Cue, String> {
        let yaml = format!("program: 3\ntime_signature: 6/8\ncues:\n  42:\n{cue}");
        Song::parse("song", &yaml, &show()).map(|mut song| song.cues.remove(&42).unwrap())
    }

    const PULSE: &str =
        "    name: pulse\n    type: pattern\n    length: 2 bars\n    target: { both: dimmer }\n";

    #[test]
    fn reads_a_pattern_in_the_song_time_signature() {
        let cue = parse(&format!(
            "{PULSE}    release: 1 beat\n    hits:\n      - {{ at: 6.75, shape: ease-in, length: 1 bar, level: 80% }}\n"
        ))
        .unwrap();

        let Cue::Pattern(pattern) = cue else {
            panic!("cue 42 is a pattern");
        };
        assert_eq!(pattern.beats, 6.0, "two bars of 6/8");
        assert_eq!(
            pattern.target,
            PatternTarget::Dimmer(BTreeSet::from(["dimmer-par".into(), "rgb-par".into()]))
        );
        assert_eq!(
            pattern.hits,
            [Hit {
                offset: 5.75,
                beats: 3.0,
                shape: Envelope::EaseIn,
                level: f64::from(0.8_f32)
            }]
        );
        assert_eq!(pattern.release, Length::Beats(1.0));
    }

    #[test]
    fn refuses_lengths_beats_and_targets_it_cannot_play() {
        let with_hit =
            |hit: &str| parse(&format!("{PULSE}    hits:\n      - {hit}\n")).unwrap_err();
        let hit_error = |message: &str| format!("line 4: cue 42: hit 1: {message}");

        assert_eq!(
            parse(&format!(
                "{}    hits: []\n",
                PULSE.replace("2 bars", "8 beats")
            ))
            .unwrap_err(),
            "line 4: cue 42: length must be 1, 2, 3 or 4 bars"
        );
        assert_eq!(
            with_hit("{ at: 1.1, shape: sine, length: 1 beat, level: 50% }"),
            hit_error(
                "at 1.1 must be a beat of the pattern from 1 to 6.75 in quarter beats, e.g. 1, 2.5 or 3.75"
            )
        );
        assert!(
            with_hit("{ at: 7, shape: sine, length: 1 beat, level: 50% }")
                .starts_with(&hit_error("at 7 must be"))
        );
        assert_eq!(
            with_hit("{ at: 1, shape: sine, length: 100ms, level: 50% }"),
            hit_error("length must be in beats or bars, e.g. 0.5 beats")
        );
        assert_eq!(
            with_hit("{ at: 1, shape: sine, length: 3 bars, level: 50% }"),
            hit_error("length must be above 0 and at most the pattern's 6 beats")
        );
        assert_eq!(
            parse(&format!(
                "{}    hits: []\n",
                PULSE.replace(
                    "{ both: dimmer }",
                    "{ dimmer-par: dimmer, rgb-par: { color: red } }"
                )
            ))
            .unwrap_err(),
            "line 4: cue 42: target: give either `dimmer` or look settings such as { color: red } to every fixture or group, or `intensity` to every pixel cue"
        );
    }
}

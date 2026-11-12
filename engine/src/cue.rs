//! Cues: what a channel-2 note fires (plan §5, §6). Each cue type is one variant of `CueEntry`
//! (as written in a song file) and of `Cue` (checked against the rig, ready to draw); the
//! playback layer only uses `Cue`'s methods.

use std::time::Duration;

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::clock_follower::{ClockFollower, Moment};
use crate::duration::{Length, TimeSignature};
use crate::envelope::Envelope;
use crate::look::{self, Look, LookSettings};
use crate::pattern::{Pattern, PatternEntry, PatternTarget};
use crate::rig::Rig;
use crate::{Frame, Show};

/// A cue as written in a song file, before its targets and colours are checked.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum CueEntry {
    Scene {
        name: String,
        fade_in: Option<Length>,
        release: Option<Length>,
        values: Mapping,
    },
    Swell {
        name: String,
        target: Mapping,
        duration: Length,
        curve: Option<Envelope>,
        release: Option<Length>,
    },
    Pattern(PatternEntry),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    /// A fixed look: fades in from note-on, holds, fades out from note-off.
    Scene(Scene),
    /// A look reached along a curve over a duration from note-on, then held until note-off.
    Swell(Swell),
    /// Envelope hits looping on the beat from the bar line after note-on until note-off.
    Pattern(Pattern),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub name: String,
    pub fade_in: Length,
    pub release: Length,
    pub look: Look,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Swell {
    pub name: String,
    pub duration: Length,
    /// How far the target look shows over `duration`: 0 is the layers below, 1 the target.
    pub curve: Envelope,
    pub release: Length,
    pub target: Look,
}

pub(crate) const INSTANT: Length = Length::Time(Duration::ZERO);

impl CueEntry {
    pub(crate) fn resolve(self, show: &Show, time_signature: TimeSignature) -> Result<Cue, String> {
        match self {
            CueEntry::Scene {
                name,
                fade_in,
                release,
                values,
            } => Ok(Cue::Scene(Scene {
                look: resolve_look(show, &values)?,
                name,
                fade_in: fade_in.unwrap_or(INSTANT),
                release: release.unwrap_or(INSTANT),
            })),
            CueEntry::Swell {
                name,
                target,
                duration,
                curve,
                release,
            } => Ok(Cue::Swell(Swell {
                target: resolve_look(show, &target)?,
                name,
                duration,
                curve: curve.unwrap_or(Envelope::RampUp),
                release: release.unwrap_or(INSTANT),
            })),
            CueEntry::Pattern(entry) => entry.resolve(show, time_signature).map(Cue::Pattern),
        }
    }
}

/// Builds a look from `{ <fixture or group>: { color: …, dimmer: … } }`, in file order, so a
/// later target wins over an earlier one on the fixtures they share.
pub(crate) fn resolve_look(show: &Show, values: &Mapping) -> Result<Look, String> {
    let mut look = Look::default();
    for (target, settings) in values {
        let target = target
            .as_str()
            .ok_or("keys must be fixture or group names")?;
        let settings = LookSettings::deserialize(settings.clone())
            .map_err(|error| format!("{target}: {error}"))?;
        look.set(show, target, &settings)
            .map_err(|error| format!("{target}: {error}"))?;
    }
    Ok(look)
}

impl Cue {
    pub fn name(&self) -> &str {
        match self {
            Cue::Scene(scene) => &scene.name,
            Cue::Swell(swell) => &swell.name,
            Cue::Pattern(pattern) => &pattern.name,
        }
    }

    /// The moment the cue counts from for its note-on at `at`: the note, or for a pattern the
    /// bar line its loop starts on.
    pub fn start(&self, clock: &ClockFollower, at: Duration) -> Moment {
        match self {
            Cue::Pattern(_) => Pattern::start(clock, at),
            _ => clock.moment(at),
        }
    }

    /// How much of the cue shows over the layers below, 0–1, at `at` after it started at
    /// `started` (see `start`).
    pub fn opacity(&self, clock: &ClockFollower, started: Moment, at: Duration) -> f32 {
        let opacity = match self {
            Cue::Scene(scene) => clock.progress(scene.fade_in, started, at),
            Cue::Swell(swell) => swell
                .curve
                .level(clock.progress(swell.duration, started, at)),
            // A dimmer target draws its fixtures dark, so it shows the more, the lower the level.
            Cue::Pattern(pattern) => match (&pattern.target, pattern.level(clock, started, at)) {
                (_, None) => 0.0,
                (PatternTarget::Dimmer(_), Some(level)) => 1.0 - level,
                (PatternTarget::Look(_), Some(level)) => level,
            },
        };
        opacity as f32
    }

    /// How long the cue takes to fade out after its note-off.
    pub fn release(&self) -> Length {
        match self {
            Cue::Scene(scene) => scene.release,
            Cue::Swell(swell) => swell.release,
            Cue::Pattern(pattern) => pattern.release,
        }
    }

    /// Draws the cue at full opacity over `frame`, its dimmers scaled by `intensity` (0–1).
    /// A pattern's dimmer target is its fixtures as below, darkened by `intensity`.
    pub fn draw(&self, rig: &Rig, intensity: f32, frame: &mut Frame) {
        let mut look = match self {
            Cue::Scene(scene) => scene.look.clone(),
            Cue::Swell(swell) => swell.target.clone(),
            Cue::Pattern(pattern) => match &pattern.target {
                PatternTarget::Look(look) => look.clone(),
                PatternTarget::Dimmer(fixtures) => {
                    look::scale_intensity(rig, fixtures, 1.0 - intensity, frame);
                    return;
                }
            },
        };
        for fixture in look.fixtures.values_mut() {
            fixture.dimmer *= intensity;
        }
        look::render(rig, &look, frame);
    }
}

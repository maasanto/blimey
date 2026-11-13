//! Cues: what a channel-2 note fires (plan §5, §6). Each cue type is one variant of `CueEntry`
//! (as written in a song file) and of `Cue` (checked against the rig, ready to draw); the
//! playback layer only uses `Cue`'s methods.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::clock_follower::{ClockFollower, Moment};
use crate::duration::{Length, TimeSignature};
use crate::envelope::Envelope;
use crate::look::{self, Look, LookSettings};
use crate::pattern::{Pattern, PatternEntry, PatternTarget};
use crate::pixel::{PixelEntry, PixelScene};
use crate::rig::Rig;
use crate::{Show, Universes};

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
    Pixel(PixelEntry),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    /// A fixed look: fades in from note-on, holds, fades out from note-off.
    Scene(Scene),
    /// A look reached along a curve over a duration from note-on, then held until note-off.
    Swell(Swell),
    /// Envelope hits looping on the beat from the bar line after note-on until note-off.
    Pattern(Pattern),
    /// A shape animated over every pixel of the rig: fades in from note-on, out from note-off.
    Pixel(PixelScene),
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
    /// How far the target shows over `duration`: 0 is the layers below, 1 the target.
    pub curve: Envelope,
    pub release: Length,
    pub target: SwellTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwellTarget {
    /// `{ all-wash: { dimmer: 100% } }`: the curve is how much of the look shows.
    Look(Look),
    /// `{ bridge-chase: intensity }`: the curve scales the intensity of the pixel cues on these
    /// notes.
    Intensity(BTreeSet<u8>),
}

pub(crate) const INSTANT: Length = Length::Time(Duration::ZERO);

/// A song's pixel cues by name, for targets that name one: name → notes.
pub(crate) type PixelCues = BTreeMap<String, BTreeSet<u8>>;

impl CueEntry {
    /// The name of a pixel cue; `None` for other cue types.
    pub(crate) fn pixel_name(&self) -> Option<&str> {
        match self {
            CueEntry::Pixel(entry) => Some(entry.name()),
            _ => None,
        }
    }

    pub(crate) fn resolve(
        self,
        show: &Show,
        time_signature: TimeSignature,
        pixel_cues: &PixelCues,
    ) -> Result<Cue, String> {
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
                target: match resolve_intensity(&target, pixel_cues) {
                    Some(notes) => SwellTarget::Intensity(notes?),
                    None => SwellTarget::Look(resolve_look(show, &target)?),
                },
                name,
                duration,
                curve: curve.unwrap_or(Envelope::RampUp),
                release: release.unwrap_or(INSTANT),
            })),
            CueEntry::Pattern(entry) => entry
                .resolve(show, time_signature, pixel_cues)
                .map(Cue::Pattern),
            CueEntry::Pixel(entry) => entry.resolve(show).map(Cue::Pixel),
        }
    }
}

/// Builds a look from `{ <fixture or group>: { color: …, dimmer: … } }`, in file order, so a
/// later target wins over an earlier one on the fixtures they share.
pub fn resolve_look(show: &Show, values: &Mapping) -> Result<Look, String> {
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

/// `{ <pixel cue name>: intensity, … }`: the notes of the song's pixel cues of those names.
/// `None` when the target is something else, such as a look.
pub(crate) fn resolve_intensity(
    target: &Mapping,
    pixel_cues: &PixelCues,
) -> Option<Result<BTreeSet<u8>, String>> {
    let is_intensity = !target.is_empty()
        && target
            .values()
            .all(|value| value.as_str() == Some("intensity"));
    if !is_intensity {
        return None;
    }
    let mut notes = BTreeSet::new();
    for name in target.keys() {
        let Some(name) = name.as_str() else {
            return Some(Err("keys must be pixel cue names".into()));
        };
        match pixel_cues.get(name) {
            Some(named) => notes.extend(named),
            None => {
                return Some(Err(format!(
                    "{name}: no pixel cue of that name in this song"
                )));
            }
        }
    }
    Some(Ok(notes))
}

impl Cue {
    pub fn name(&self) -> &str {
        match self {
            Cue::Scene(scene) => &scene.name,
            Cue::Swell(swell) => &swell.name,
            Cue::Pattern(pattern) => &pattern.name,
            Cue::Pixel(scene) => &scene.name,
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
            Cue::Pixel(scene) => clock.progress(scene.fade_in, started, at),
            Cue::Swell(swell) => {
                let level = swell
                    .curve
                    .level(clock.progress(swell.duration, started, at));
                match swell.target {
                    SwellTarget::Look(_) => level,
                    SwellTarget::Intensity(_) => 1.0 - level,
                }
            }
            // A dimmer or intensity target darkens, so it shows the more, the lower the level.
            Cue::Pattern(pattern) => match (&pattern.target, pattern.level(clock, started, at)) {
                (_, None) => 0.0,
                (PatternTarget::Dimmer(_) | PatternTarget::Intensity(_), Some(level)) => {
                    1.0 - level
                }
                (PatternTarget::Look(_), Some(level)) => level,
            },
        };
        opacity as f32
    }

    /// The notes of the pixel cues whose intensity this swell or pattern drives. Such a cue
    /// draws nothing itself: its opacity darkens those pixel cues, as a dimmer pattern darkens
    /// its fixtures.
    pub fn intensity_targets(&self) -> Option<&BTreeSet<u8>> {
        match self {
            Cue::Swell(Swell {
                target: SwellTarget::Intensity(notes),
                ..
            })
            | Cue::Pattern(Pattern {
                target: PatternTarget::Intensity(notes),
                ..
            }) => Some(notes),
            _ => None,
        }
    }

    /// How long the cue takes to fade out after its note-off.
    pub fn release(&self) -> Length {
        match self {
            Cue::Scene(scene) => scene.release,
            Cue::Swell(swell) => swell.release,
            Cue::Pattern(pattern) => pattern.release,
            Cue::Pixel(scene) => scene.release,
        }
    }

    /// Draws the cue at full opacity over `out`, as it is at `at` after it started at
    /// `started`, its dimmers scaled by `intensity` (0–1). A pattern's dimmer target is its
    /// fixtures as below, darkened by `intensity`.
    pub fn draw(
        &self,
        rig: &Rig,
        clock: &ClockFollower,
        started: Moment,
        at: Duration,
        intensity: f32,
        out: &mut Universes,
    ) {
        let frame = &mut out.enttec;
        let mut look = match self {
            Cue::Pixel(scene) => return scene.draw(rig, clock, started, at, intensity, out),
            Cue::Scene(scene) => scene.look.clone(),
            Cue::Swell(swell) => match &swell.target {
                SwellTarget::Look(look) => look.clone(),
                SwellTarget::Intensity(_) => return,
            },
            Cue::Pattern(pattern) => match &pattern.target {
                PatternTarget::Look(look) => look.clone(),
                PatternTarget::Dimmer(fixtures) => {
                    look::scale_intensity(rig, fixtures, 1.0 - intensity, frame);
                    return;
                }
                PatternTarget::Intensity(_) => return,
            },
        };
        for fixture in look.fixtures.values_mut() {
            fixture.dimmer *= intensity;
        }
        look::render(rig, &look, frame);
    }
}

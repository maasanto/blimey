//! Cues: what a channel-2 note fires (plan §5, §6). Each cue type is one variant of `CueEntry`
//! (as written in a song file) and of `Cue` (checked against the rig, ready to draw); the
//! playback layer only uses `Cue`'s methods.

use std::time::Duration;

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::duration::Length;
use crate::look::{self, Look, LookSettings};
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    /// A fixed look: fades in from note-on, holds, fades out from note-off.
    Scene(Scene),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub name: String,
    pub fade_in: Length,
    pub release: Length,
    pub look: Look,
}

const INSTANT: Length = Length::Time(Duration::ZERO);

impl CueEntry {
    pub(crate) fn resolve(self, show: &Show) -> Result<Cue, String> {
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
        }
    }

    /// How far the cue has faded in, 0–1, `elapsed` after its note-on.
    pub fn opacity(&self, elapsed: Duration) -> f32 {
        match self {
            Cue::Scene(scene) => ramp(elapsed, scene.fade_in.duration()),
        }
    }

    /// How long the cue takes to fade out after its note-off.
    pub fn release(&self) -> Duration {
        match self {
            Cue::Scene(scene) => scene.release.duration(),
        }
    }

    /// Draws the cue at full opacity over `frame`, its dimmers scaled by `intensity` (0–1).
    pub fn draw(&self, rig: &Rig, intensity: f32, frame: &mut Frame) {
        match self {
            Cue::Scene(scene) => {
                let mut look = scene.look.clone();
                for fixture in look.fixtures.values_mut() {
                    fixture.dimmer *= intensity;
                }
                look::render(rig, &look, frame);
            }
        }
    }
}

/// Progress through `span`, 0–1, `elapsed` into it. A zero span is complete at once.
pub(crate) fn ramp(elapsed: Duration, span: Duration) -> f32 {
    if span.is_zero() {
        return 1.0;
    }
    (elapsed.as_secs_f32() / span.as_secs_f32()).min(1.0)
}

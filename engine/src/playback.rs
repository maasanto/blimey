//! What the MIDI received so far has set up (plan §5): legacy levels, the selected song, its
//! active cues, the master dimmer and the live parameters. Renders it in layer order: legacy,
//! then cues, then the master dimmer.

use std::time::Duration;

use serde::Serialize;

use crate::clock_follower::{ClockFollower, Moment};
use crate::cue::Cue;
use crate::fixture::Attribute;
use crate::look::{self, Look};
use crate::rig::Rig;
use crate::song::Song;
use crate::{DMX_CHANNELS, Frame, MidiEvent, Show};

/// Status bytes on channel 2 (cues, song select, master, live parameters).
const CUE_NOTE_OFF: u8 = 0x81;
const CUE_NOTE_ON: u8 = 0x91;
const CUE_CONTROL_CHANGE: u8 = 0xB1;
const CUE_PROGRAM_CHANGE: u8 = 0xC1;
/// Note-on on channel 16.
const PANIC_NOTE_ON: u8 = 0x9F;
const PANIC_NOTE: u8 = 127;
const MASTER_CONTROLLER: u8 = 7;
const FIRST_LIVE_PARAMETER: u8 = 20;
pub const LIVE_PARAMETERS: usize = 8;
const MIDI_DATA_MAX: f32 = 127.0;
/// Reaper's default tempo, for a song without `bpm` when no MIDI clock arrives.
const DEFAULT_BPM: f32 = 120.0;

pub(crate) struct Playback {
    legacy: Frame,
    /// Name of the selected song, if any.
    song: Option<String>,
    /// Musical time for the selected song, started when it was selected.
    clock: Option<ClockFollower>,
    /// Whether panic put the safe look up; it stays until the next song select.
    safe_look: bool,
    /// Lowest first: a cue fired later draws over the earlier ones.
    cues: Vec<ActiveCue>,
    master: f32,
    live_parameters: [f32; LIVE_PARAMETERS],
    /// Cues started or released since the last `take_cue_events`.
    cue_events: Vec<CueEvent>,
}

/// A cue starting or being released, for the event log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CueEvent {
    /// Time of the MIDI event that caused it.
    pub at: Duration,
    pub action: CueAction,
    pub song: String,
    pub note: u8,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CueAction {
    Start,
    Release,
}

struct ActiveCue {
    note: u8,
    started: Moment,
    /// Note-on velocity as 0–1.
    intensity: f32,
    released: Option<Release>,
}

struct Release {
    at: Moment,
    /// Opacity when released, which then fades to 0.
    opacity: f32,
}

/// The engine state behind a frame, as streamed to the UI (docs/api.md).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Status {
    pub song: Option<String>,
    /// Lowest first.
    pub cues: Vec<CueStatus>,
    pub master: f32,
    /// CC 20–27 on channel 2, as 0–1.
    pub live_parameters: [f32; LIVE_PARAMETERS],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CueStatus {
    pub note: u8,
    pub name: String,
    pub intensity: f32,
    pub released: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Playback {
            legacy: [0; DMX_CHANNELS],
            song: None,
            clock: None,
            safe_look: false,
            cues: Vec::new(),
            master: 1.0,
            live_parameters: [0.0; LIVE_PARAMETERS],
            cue_events: Vec::new(),
        }
    }
}

impl Playback {
    pub fn apply(&mut self, show: &Show, event: &MidiEvent) {
        if let Some(clock) = &mut self.clock {
            clock.apply(event.at, &event.message);
        }
        show.legacy.apply(&event.message, &mut self.legacy);
        match *event.message.as_slice() {
            [CUE_PROGRAM_CHANGE, program] => self.select_song(show, program),
            [CUE_NOTE_ON, note, velocity] if velocity > 0 => {
                self.start(show, note, velocity, event.at)
            }
            [CUE_NOTE_ON | CUE_NOTE_OFF, note, _] => self.release(show, note, event.at),
            [CUE_CONTROL_CHANGE, MASTER_CONTROLLER, value] => {
                self.master = f32::from(value) / MIDI_DATA_MAX
            }
            [CUE_CONTROL_CHANGE, controller, value] => {
                let index = usize::from(controller.wrapping_sub(FIRST_LIVE_PARAMETER));
                if let Some(parameter) = self.live_parameters.get_mut(index) {
                    *parameter = f32::from(value) / MIDI_DATA_MAX;
                }
            }
            [PANIC_NOTE_ON, PANIC_NOTE, velocity] if velocity > 0 => self.panic(),
            _ => {}
        }
    }

    fn song<'show>(&self, show: &'show Show) -> Option<&'show Song> {
        let name = self.song.as_deref()?;
        show.songs.iter().find(|song| song.name == name)
    }

    /// Output only depends on MIDI since the song started: its cues start from nothing.
    fn select_song(&mut self, show: &Show, program: u8) {
        let song = show.songs.iter().find(|song| song.program == program);
        self.song = song.map(|song| song.name.clone());
        self.clock = song.map(|song| {
            let bpm = song.bpm.unwrap_or(DEFAULT_BPM);
            ClockFollower::new(f64::from(bpm), song.time_signature)
        });
        self.cues.clear();
        self.safe_look = false;
    }

    fn start(&mut self, show: &Show, note: u8, velocity: u8, at: Duration) {
        let Some(clock) = &self.clock else {
            return;
        };
        let Some(cue) = self.song(show).and_then(|song| song.cues.get(&note)) else {
            return;
        };
        let started = cue.start(clock, at);
        // A retriggered cue fades out under its new instance.
        self.release(show, note, at);
        self.cues.push(ActiveCue {
            note,
            started,
            intensity: f32::from(velocity) / MIDI_DATA_MAX,
            released: None,
        });
        self.record_cue_event(show, CueAction::Start, note, at);
    }

    fn release(&mut self, show: &Show, note: u8, at: Duration) {
        let (Some(cue), Some(clock)) = (
            self.song(show).and_then(|song| song.cues.get(&note)),
            &self.clock,
        ) else {
            return;
        };
        let mut is_released = false;
        for active in &mut self.cues {
            if active.note == note && active.released.is_none() {
                active.released = Some(Release {
                    at: clock.moment(at),
                    opacity: cue.opacity(clock, active.started, at),
                });
                is_released = true;
            }
        }
        if is_released {
            self.record_cue_event(show, CueAction::Release, note, at);
        }
    }

    fn record_cue_event(&mut self, show: &Show, action: CueAction, note: u8, at: Duration) {
        let Some(song) = self.song(show) else {
            return;
        };
        self.cue_events.push(CueEvent {
            at,
            action,
            song: song.name.clone(),
            note,
            name: song.cues[&note].name().into(),
        });
    }

    /// Cues started or released since the last call, in order.
    pub fn take_cue_events(&mut self) -> Vec<CueEvent> {
        std::mem::take(&mut self.cue_events)
    }

    /// A line for the log when the selected song's tempo source changes (see `ClockFollower`).
    pub fn log_line(&mut self, at: Duration) -> Option<String> {
        let line = self.clock.as_mut()?.log_line(at)?;
        Some(format!(
            "{}: {line}",
            self.song.as_deref().unwrap_or_default()
        ))
    }

    /// Panic: clears legacy levels and cues and puts up the song's safe look.
    fn panic(&mut self) {
        self.legacy = [0; DMX_CHANNELS];
        self.cues.clear();
        self.safe_look = true;
    }

    /// The frame due at `at`, with `look` (from `blimey fire`) between legacy and cues.
    /// Drops cues that have finished fading out.
    pub fn render(&mut self, show: &Show, look: &Look, at: Duration) -> Frame {
        let mut frame = self.legacy;
        look::render(&show.rig, look, &mut frame);
        let (Some(song), Some(clock)) = (self.song(show), self.clock.as_mut()) else {
            apply_master(&show.rig, self.master, &mut frame);
            return frame;
        };
        // The builder's metronome: an edited `bpm` takes over at once.
        clock.set_song_bpm(at, f64::from(song.bpm.unwrap_or(DEFAULT_BPM)));
        let clock = &*clock;
        if self.safe_look {
            look::render(&show.rig, &song.safe_look, &mut frame);
        }
        let faded_out =
            |cue: &Cue, release: &Release| clock.progress(cue.release(), release.at, at) as f32;
        self.cues.retain(|active| {
            song.cues.get(&active.note).is_some_and(|cue| {
                active
                    .released
                    .as_ref()
                    .is_none_or(|release| faded_out(cue, release) < 1.0)
            })
        });
        for active in &self.cues {
            let cue = &song.cues[&active.note];
            let opacity = match &active.released {
                None => cue.opacity(clock, active.started, at),
                Some(release) => release.opacity * (1.0 - faded_out(cue, release)),
            };
            let mut lit = frame;
            cue.draw(&show.rig, active.intensity, &mut lit);
            // ponytail: channels crossfade one by one, so a fixture with a dimmer channel fading
            // up from black multiplies two ramps (an ease-in). Fade in look space if that shows.
            for (below, above) in frame.iter_mut().zip(lit) {
                let blended = f32::from(*below) + (f32::from(above) - f32::from(*below)) * opacity;
                *below = blended.round() as u8;
            }
        }
        apply_master(&show.rig, self.master, &mut frame);
        frame
    }

    pub fn status(&self, show: &Show) -> Status {
        let song = self.song(show);
        Status {
            song: song.map(|song| song.name.clone()),
            cues: self
                .cues
                .iter()
                .filter_map(|active| {
                    let cue = song?.cues.get(&active.note)?;
                    Some(CueStatus {
                        note: active.note,
                        name: cue.name().into(),
                        intensity: active.intensity,
                        released: active.released.is_some(),
                    })
                })
                .collect(),
            master: self.master,
            live_parameters: self.live_parameters,
        }
    }
}

/// Scales intensity channels by `master`: dimmer channels, the colour channels of fixtures
/// without a dimmer, and channels no fixture claims (legacy-only). Other channels, such as a
/// shutter, keep their value.
fn apply_master(rig: &Rig, master: f32, frame: &mut Frame) {
    if master >= 1.0 {
        return;
    }
    let mut is_intensity = [true; DMX_CHANNELS];
    for fixture in &rig.fixtures {
        for (offset, channel) in fixture.channels.iter().enumerate() {
            is_intensity[fixture.address - 1 + offset] = match channel.attribute {
                Some(Attribute::Dimmer) => true,
                Some(_) => fixture.dimmer.is_none(),
                None => false,
            };
        }
    }
    for (value, scaled) in frame.iter_mut().zip(is_intensity) {
        if scaled {
            *value = (f32::from(*value) * master).round() as u8;
        }
    }
}

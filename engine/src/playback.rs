//! What the MIDI received so far has set up (plan §5): legacy levels, the selected song, its
//! active cues, the master dimmer and the live parameters. Renders it in layer order: legacy,
//! then cues, then the master dimmer.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

use crate::clock_follower::{ClockFollower, Moment};
use crate::cue::{Cue, CueTiming};
use crate::fixture::Attribute;
use crate::look::{self, Look, VirtualDimmers};
use crate::rig::Rig;
use crate::song::Song;
use crate::{ArtNetFrames, DMX_CHANNELS, Frame, MidiEvent, Show, Universe, Universes};

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

pub(crate) struct Playback {
    legacy: Frame,
    /// Name of the selected song, if any.
    song: Option<String>,
    /// Musical time for the selected song, started when it was selected.
    clock: Option<ClockFollower>,
    /// Whether panic put the safe look up; it stays until the next song select.
    safe_look: bool,
    /// In firing order; `layered` gives the drawing order.
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
    /// See `Cue::bar_line`.
    bar_line: Option<Moment>,
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
    /// Beats since the cue started, or for a pattern since its loop did: negative while the
    /// loop waits for its bar line. Places the builder's playhead.
    pub beats: f64,
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

/// The channel-2 MIDI that takes a fresh engine to the song with `program` and to `master`
/// (0–1): its Program Change, then CC 7 unless the master is at full, where it starts.
/// Cues are not part of it: after a restart the engine waits for the next one.
pub fn resume_messages(program: Option<u8>, master: f32) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    if let Some(program) = program {
        messages.push(vec![CUE_PROGRAM_CHANGE, program]);
    }
    if master < 1.0 {
        let value = (master.clamp(0.0, 1.0) * MIDI_DATA_MAX).round() as u8;
        messages.push(vec![CUE_CONTROL_CHANGE, MASTER_CONTROLLER, value]);
    }
    messages
}

impl Playback {
    pub fn apply(&mut self, show: &Show, event: &MidiEvent) {
        if let Some(clock) = &mut self.clock {
            clock.apply(event.at, &event.message);
        }
        show.legacy.apply(&event.message, &mut self.legacy);
        match *event.message.as_slice() {
            [CUE_PROGRAM_CHANGE, program] => self.select_song(show, program, event.at),
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
    fn select_song(&mut self, show: &Show, program: u8, at: Duration) {
        let song = show.songs.iter().find(|song| song.program == program);
        self.song = song.map(|song| song.name.clone());
        self.clock =
            song.map(|song| ClockFollower::started_at(at, song.tempo(), song.time_signature));
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
        let bar_line = cue.bar_line(clock, at);
        // A retriggered cue fades out under its new instance.
        self.release(show, note, at);
        self.cues.push(ActiveCue {
            note,
            started,
            bar_line,
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
    pub fn render(&mut self, show: &Show, look: &Look, at: Duration) -> Universes {
        let mut output = Universes {
            enttec: self.legacy,
            artnet: show.rig.artnet_frames(),
        };
        // Legacy notes are raw DMX: colour at full, at intensity 1.
        let mut dimmers = VirtualDimmers::of(&output.enttec);
        look::render(&show.rig, look, &mut output.enttec, &mut dimmers);
        // The builder's metronome: an edited `bpm` takes over at once.
        if let (Some(song), Some(clock)) = (self.song(show), self.clock.as_mut()) {
            clock.set_song_bpm(at, song.tempo());
        }
        let (Some(song), Some(clock)) = (self.song(show), &self.clock) else {
            apply_master(&show.rig, self.master, &mut output);
            return output;
        };
        if self.safe_look {
            look::render(&show.rig, &song.safe_look, &mut output.enttec, &mut dimmers);
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
        let opacity = |active: &ActiveCue, cue: &Cue| match &active.released {
            None => cue.opacity(clock, active.started, at),
            Some(release) => release.opacity * (1.0 - faded_out(cue, release)),
        };
        // Swells and patterns on a pixel cue's intensity darken it, as a dimmer pattern darkens
        // its fixtures, and draw nothing themselves. Several on one pixel cue multiply.
        let mut intensity_scales: BTreeMap<u8, f32> = BTreeMap::new();
        for active in &self.cues {
            let cue = &song.cues[&active.note];
            for note in cue.intensity_targets().into_iter().flatten() {
                let darkened = opacity(active, cue) * active.intensity;
                *intensity_scales.entry(*note).or_insert(1.0) *= 1.0 - darkened;
            }
        }
        for active in self.layered(song) {
            let cue = &song.cues[&active.note];
            if cue.intensity_targets().is_some() {
                continue;
            }
            let opacity = opacity(active, cue);
            let intensity = active.intensity * intensity_scales.get(&active.note).unwrap_or(&1.0);
            // Only pixel cues draw on Art-Net: the others get no copy of it to blend back.
            let mut lit = Universes {
                enttec: output.enttec,
                artnet: match cue {
                    Cue::Pixel(_) => output.artnet.clone(),
                    _ => ArtNetFrames::new(),
                },
            };
            let mut lit_dimmers = dimmers.clone();
            let timing = CueTiming {
                started: active.started,
                bar_line: active.bar_line,
                released: active.released.as_ref().map(|release| release.at),
            };
            cue.draw(
                &show.rig,
                clock,
                timing,
                at,
                intensity,
                &mut lit,
                &mut lit_dimmers,
            );
            dimmers.crossfade(&lit_dimmers, opacity);
            // ponytail: channels crossfade one by one, so a fixture with a dimmer channel fading
            // up from black multiplies two ramps (an ease-in). Fade in look space if that shows.
            crossfade(&mut output.enttec, &lit.enttec, opacity);
            for (universe, lit) in &lit.artnet {
                crossfade(output.frame_mut(Universe::ArtNet(*universe)), lit, opacity);
            }
        }
        apply_master(&show.rig, self.master, &mut output);
        output
    }

    /// The state behind the frame due at `at`.
    pub fn status(&self, show: &Show, at: Duration) -> Status {
        let song = self.song(show);
        Status {
            song: song.map(|song| song.name.clone()),
            cues: song
                .map(|song| self.layered(song))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|active| {
                    let cue = song?.cues.get(&active.note)?;
                    Some(CueStatus {
                        note: active.note,
                        name: cue.name().into(),
                        intensity: active.intensity,
                        released: active.released.is_some(),
                        beats: self.clock.as_ref()?.beats(at) - active.started.beats,
                    })
                })
                .collect(),
            master: self.master,
            live_parameters: self.live_parameters,
        }
    }

    /// Active cues, lowest layer first (plan §5): scene, swell and pattern cues, then pixel
    /// cues, each in the order they were fired.
    fn layered(&self, song: &Song) -> Vec<&ActiveCue> {
        let mut cues: Vec<&ActiveCue> = self.cues.iter().collect();
        cues.sort_by_key(|active| matches!(song.cues.get(&active.note), Some(Cue::Pixel(_))));
        cues
    }
}

/// Moves each channel of `below` towards `above` by `opacity` (0–1).
fn crossfade(below: &mut Frame, above: &Frame, opacity: f32) {
    for (below, above) in below.iter_mut().zip(above) {
        let blended = f32::from(*below) + (f32::from(*above) - f32::from(*below)) * opacity;
        *below = blended.round() as u8;
    }
}

/// Scales intensity channels by `master`: dimmer channels, the colour channels of fixtures
/// without a dimmer, and channels no fixture claims (legacy-only and pixel grids, every
/// Art-Net channel included). Other channels, such as a shutter, keep their value.
fn apply_master(rig: &Rig, master: f32, output: &mut Universes) {
    if master >= 1.0 {
        return;
    }
    let scale = |value: &mut u8| *value = (f32::from(*value) * master).round() as u8;
    output.artnet.values_mut().flatten().for_each(scale);
    let frame = &mut output.enttec;
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
            scale(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FRAME_PERIOD, MidiFile, MidiSource};
    use std::path::Path;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

    fn cues_show() -> Show {
        Show::load(Path::new(&format!("{FIXTURES}/shows/cues"))).unwrap()
    }

    /// Plays `events` on an engine that has been running for `uptime`, frame by frame.
    fn render_after(show: &Show, events: &[MidiEvent], uptime: Duration) -> Vec<Universes> {
        let mut playback = Playback::default();
        let mut pending = events.iter().peekable();
        let last = events.last().map_or(Duration::ZERO, |event| event.at);
        let frame_count = last.as_nanos() / FRAME_PERIOD.as_nanos() + 40;
        (0..frame_count as u32)
            .map(|index| {
                let deadline = FRAME_PERIOD * index;
                while let Some(event) = pending.next_if(|event| event.at <= deadline) {
                    let shifted = MidiEvent {
                        at: event.at + uptime,
                        message: event.message.clone(),
                    };
                    playback.apply(show, &shifted);
                }
                playback.render(show, &Look::default(), deadline + uptime)
            })
            .collect()
    }

    fn midi_events(name: &str) -> Vec<MidiEvent> {
        let mut midi = MidiFile::open(Path::new(&format!("{FIXTURES}/midi/{name}.mid"))).unwrap();
        std::iter::from_fn(|| midi.poll(Duration::MAX)).collect()
    }

    #[test]
    fn output_does_not_depend_on_how_long_the_engine_has_run() {
        let show = cues_show();
        let event = |seconds: f64, message: &[u8]| MidiEvent {
            at: Duration::from_secs_f64(seconds),
            message: message.to_vec(),
        };
        // No clock, 120 BPM: verse-pulse over a green scene, fired exactly on the bar 2 line,
        // 2 s after the select.
        let on_a_bar_line = vec![
            event(0.0, &[CUE_PROGRAM_CHANGE, 1]),
            event(0.0, &[CUE_NOTE_ON, 38, 127]),
            event(2.0, &[CUE_NOTE_ON, 42, 127]),
            event(6.0, &[CUE_NOTE_OFF, 42, 0]),
        ];
        // The same on a steady MIDI clock at 125 BPM, a tick every 20 ms: the bar 2 line is at
        // 1.92 s, on tick 96.
        let mut on_the_clock = vec![
            event(0.0, &[CUE_PROGRAM_CHANGE, 1]),
            event(0.0, &[0xFA]),
            event(0.0, &[CUE_NOTE_ON, 38, 127]),
        ];
        for tick in 0..400 {
            on_the_clock.push(event(f64::from(tick) * 0.020, &[0xF8]));
            if tick == 96 {
                on_the_clock.push(event(1.92, &[CUE_NOTE_ON, 42, 127]));
            }
        }
        let hundred_hours = Duration::from_secs(100 * 3600) + Duration::from_millis(1234);

        for (name, events) in [
            ("on-a-bar-line", on_a_bar_line),
            ("on-the-clock", on_the_clock),
            ("pattern-92", midi_events("pattern-92")),
            ("pattern-140", midi_events("pattern-140")),
            ("swell-clock", midi_events("swell-clock")),
        ] {
            let fresh = render_after(&show, &events, Duration::ZERO);
            let later = render_after(&show, &events, hundred_hours);
            let first_difference = fresh.iter().zip(&later).position(|(a, b)| a != b);
            assert_eq!(
                first_difference, None,
                "{name}: frame index of the first difference"
            );
        }
    }

    #[test]
    fn status_counts_beats_from_a_cue_start_and_a_pattern_loop_start() {
        let show = cues_show();
        let mut playback = Playback::default();
        let seconds = Duration::from_secs_f64;
        // No clock, 120 BPM: 2 beats a second. The pattern's note lands mid-bar, so its loop
        // starts on the bar 2 line at 2 s.
        for (at, message) in [
            (0.0, [CUE_PROGRAM_CHANGE, 1].as_slice()),
            (0.5, &[CUE_NOTE_ON, 39, 127]),
            (1.0, &[CUE_NOTE_ON, 42, 127]),
        ] {
            playback.apply(
                &show,
                &MidiEvent {
                    at: seconds(at),
                    message: message.to_vec(),
                },
            );
        }
        let beats = |at: f64| -> Vec<(u8, f64)> {
            let status = playback.status(&show, seconds(at));
            status
                .cues
                .iter()
                .map(|cue| (cue.note, cue.beats))
                .collect()
        };

        assert_eq!(beats(1.5), [(39, 2.0), (42, -1.0)]);
        assert_eq!(beats(3.25), [(39, 5.5), (42, 2.5)]);
    }

    /// The builder's playhead follows a pattern that drives a pixel cue's intensity, though
    /// the pattern draws nothing itself.
    #[test]
    fn a_pattern_linked_to_a_pixel_cue_reports_its_beats_while_it_dims_the_cue() {
        let mut show = cues_show();
        let song = "program: 1\ncues:\n  40: { name: red, type: pixel, params: { color: \"#ff0000\" } }\n  42:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { red: intensity }\n    hits:\n      - { at: 1, shape: ramp-up, length: 1 beat, level: 100% }\n";
        show.songs = vec![Song::parse("linked", song, &show).unwrap()];
        let mut playback = Playback::default();
        for message in [
            [CUE_PROGRAM_CHANGE, 1].as_slice(),
            &[CUE_NOTE_ON, 40, 127],
            &[CUE_NOTE_ON, 42, 127],
        ] {
            let event = MidiEvent {
                at: Duration::ZERO,
                message: message.to_vec(),
            };
            playback.apply(&show, &event);
        }
        // 120 BPM: half a beat in, half-way up the ramp.
        let at = Duration::from_millis(250);

        let output = playback.render(&show, &Look::default(), at);
        let status = playback.status(&show, at);

        assert_eq!(output.enttec[4], 128, "rgb-par red at half intensity");
        let beats: Vec<(u8, f64)> = status
            .cues
            .iter()
            .map(|cue| (cue.note, cue.beats))
            .collect();
        assert_eq!(beats, [(42, 0.5), (40, 0.5)]);
    }
}

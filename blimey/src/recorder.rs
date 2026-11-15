//! `run`'s diagnostics: the state it resumes after a restart and the event log. Both are
//! written on one background thread, so the render loop only hands records over. Neither
//! ever stops the engine: a state folder it can't use is reported, and the engine runs
//! without them.

use std::fs;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use blimey_engine::{CueEvent, MidiEvent, Status};

use crate::event_log::EventLog;
use crate::state::{SavedState, StateFile};
use crate::stream::is_ticking;

enum Record {
    Midi(MidiEvent),
    Cue(CueEvent),
    Skipped { at: Duration, frames: u32 },
    State(SavedState),
}

pub struct Recorder {
    // ponytail: unbounded, so a stalled disk grows memory rather than stalling frames: about
    // 50 bytes a MIDI event. Bound it with dropped-event counting if stalls ever last minutes.
    records: Option<Sender<Record>>,
    writer: Option<JoinHandle<()>>,
    /// The state the engine started with, then the last one handed to the writer.
    state: SavedState,
}

impl Recorder {
    /// Reads the saved state in `state_dir`, logs the start, and starts the writer. Without a
    /// usable `state_dir` the engine starts fresh and records nothing.
    pub fn start(state_dir: Option<&Path>, show_dir: &Path, mode: &str) -> Recorder {
        let state_dir = state_dir.filter(|dir| {
            fs::create_dir_all(dir)
                .map_err(|error| {
                    eprintln!(
                        "state: cannot use {}: {error}; running without saved state or event log",
                        dir.display()
                    )
                })
                .is_ok()
        });
        let state_file = state_dir.map(StateFile::open);
        let state = state_file
            .as_ref()
            .map(|file| file.saved().clone())
            .unwrap_or_default();
        let mut log = state_dir.and_then(|dir| {
            EventLog::open(dir)
                .map_err(|error| {
                    eprintln!(
                        "log: cannot open the event log in {}: {error}; running without it",
                        dir.display()
                    )
                })
                .ok()
        });
        if let Some(log) = &mut log {
            log.start(show_dir, mode, state.song.as_deref(), state.master);
        }
        let (records, received) = mpsc::channel();
        let writer = thread::spawn(move || {
            let (mut state_file, mut log) = (state_file, log);
            for record in received {
                match (record, &mut state_file, &mut log) {
                    (Record::Midi(event), _, Some(log)) => log.midi(&event),
                    (Record::Cue(event), _, Some(log)) => log.cue(&event),
                    (Record::Skipped { at, frames }, _, Some(log)) => log.skipped(at, frames),
                    (Record::State(state), Some(file), _) => file.save(&state),
                    _ => {}
                }
            }
        });
        Recorder {
            records: Some(records),
            writer: Some(writer),
            state,
        }
    }

    /// The song and master the engine resumes with.
    pub fn saved(&self) -> &SavedState {
        &self.state
    }

    /// Saves the song and master when they change. A failed save is retried on the next change.
    pub fn status(&mut self, status: &Status) {
        let state = SavedState::of(status);
        if state != self.state {
            self.state = state.clone();
            self.send(Record::State(state));
        }
    }

    pub fn midi(&self, event: &MidiEvent) {
        if !is_ticking(&event.message) {
            self.send(Record::Midi(event.clone()));
        }
    }

    pub fn cue(&self, event: &CueEvent) {
        self.send(Record::Cue(event.clone()));
    }

    pub fn skipped(&self, at: Duration, frames: u32) {
        self.send(Record::Skipped { at, frames });
    }

    fn send(&self, record: Record) {
        if let Some(records) = &self.records {
            // Fails only if the writer panicked, which it reports itself.
            let _ = records.send(record);
        }
    }
}

impl Drop for Recorder {
    /// Waits for every record to be written.
    fn drop(&mut self) {
        self.records = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::path::PathBuf;
    use std::time::Instant;

    use blimey_engine::look::Look;
    use blimey_engine::{DmxSink, FRAME_PERIOD, FakeClock, Frame, MidiSource, Show, run};
    use serde_json::Value;

    use crate::state::Resumed;

    const SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues");

    fn scratch_dir(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("blimey-recorder-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Records like `blimey run` does, and keeps each frame with the time it was sent.
    struct Recorded {
        recorder: Recorder,
        frames: Vec<(Instant, Frame)>,
    }

    impl DmxSink for Recorded {
        fn send(&mut self, _at: Duration, frame: &Frame) -> io::Result<()> {
            self.frames.push((Instant::now(), *frame));
            Ok(())
        }

        fn status(&mut self, status: &Status) {
            self.recorder.status(status);
        }

        fn midi(&mut self, event: &MidiEvent) {
            self.recorder.midi(event);
        }

        fn cue(&mut self, event: &CueEvent) {
            self.recorder.cue(event);
        }
    }

    /// Runs `frame_count` frames of the cues show from a fresh start in `state_dir`, after the
    /// resumed state, with `midi`. Returns once everything is written.
    fn run_engine(
        state_dir: &Path,
        midi: impl MidiSource,
        frame_count: u32,
    ) -> Vec<(Instant, Frame)> {
        let show = Show::load(Path::new(SHOW)).unwrap();
        let recorder = Recorder::start(Some(state_dir), Path::new(SHOW), "show");
        let mut midi = Resumed::new(recorder.saved().resume_events(&show), midi);
        let mut sink = Recorded {
            recorder,
            frames: Vec::new(),
        };
        run(
            &mut FakeClock::default(),
            &mut midi,
            &show,
            &Look::default(),
            &mut sink,
            frame_count,
        )
        .unwrap();
        sink.frames
    }

    /// Delivers `messages` at once.
    struct Messages(Vec<Vec<u8>>);

    impl MidiSource for Messages {
        fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
            (!self.0.is_empty()).then(|| MidiEvent {
                at: until,
                message: self.0.remove(0),
            })
        }
    }

    fn first_frame(state_dir: &Path, messages: &[&[u8]]) -> Frame {
        let messages = Messages(messages.iter().map(|message| message.to_vec()).collect());
        run_engine(state_dir, messages, 1)[0].1
    }

    #[test]
    fn a_restarted_engine_resumes_the_song_and_master_and_waits_for_the_next_cue() {
        let dir = scratch_dir("resume");
        let blue_snap: &[u8] = &[0x91, 37, 127];

        let before_restart = first_frame(&dir, &[&[0xC1, 1], &[0xB1, 7, 64], blue_snap]);
        let after_restart = first_frame(&dir, &[]);
        let next_cue = first_frame(&dir, &[blue_snap]);

        assert_eq!(before_restart[0..4], [129, 0, 0, 255], "dimmer at 64/127");
        assert_eq!(after_restart[0..4], [0, 0, 0, 0], "no cue resumed");
        assert_eq!(next_cue, before_restart, "song and master resumed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn skipped_frames_and_failed_cues_are_logged() {
        let dir = scratch_dir("skipped");
        let recorder = Recorder::start(Some(&dir), Path::new(SHOW), "show");

        recorder.skipped(Duration::from_millis(1250), 40);
        recorder.cue(&CueEvent {
            at: Duration::from_millis(1275),
            action: blimey_engine::CueAction::Fail,
            song: "first".into(),
            note: 37,
            name: "blue-snap".into(),
        });
        drop(recorder);

        let lines: Vec<Value> = fs::read_to_string(dir.join("events.log"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            (
                lines[1]["type"].as_str(),
                lines[1]["frames"].as_u64(),
                lines[1]["at_ms"].as_u64()
            ),
            (Some("skipped"), Some(40), Some(1250))
        );
        assert_eq!(
            (lines[2]["action"].as_str(), lines[2]["note"].as_u64()),
            (Some("fail"), Some(37))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_unusable_state_folder_runs_the_show_without_state_or_log() {
        let dir = scratch_dir("unusable");
        let not_a_folder = dir.join("a-file");
        fs::write(&not_a_folder, "").unwrap();

        let frame = first_frame(&not_a_folder, &[&[0xC1, 1], &[0x91, 37, 127]]);

        assert_eq!(frame[0..4], [255, 0, 0, 255]);
        fs::remove_dir_all(dir).unwrap();
    }

    /// 250 messages due by each frame (10 000 a second): cue notes on and off, master moves
    /// and live parameters, as a busy Reaper session might send.
    struct Flood {
        per_frame: usize,
        due: Option<Duration>,
        left: usize,
        sent: u64,
    }

    impl MidiSource for Flood {
        fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
            if self.due != Some(until) {
                self.due = Some(until);
                self.left = self.per_frame;
            }
            self.left = self.left.checked_sub(1)?;
            self.sent += 1;
            let value = (self.sent % 128) as u8;
            let message = match self.sent % 4 {
                0 => vec![0x91, 36 + value % 4, 100],
                1 => vec![0x81, 36 + value % 4, 0],
                2 => vec![0xB1, 7, value],
                _ => vec![0xB1, 20, value],
            };
            Some(MidiEvent { at: until, message })
        }
    }

    #[test]
    fn frames_keep_their_slot_while_a_midi_flood_is_logged() {
        let dir = scratch_dir("flood");
        const FRAMES: u32 = 80;
        let flood = Flood {
            per_frame: 250,
            due: None,
            left: 0,
            sent: 0,
        };

        let frames = run_engine(&dir, flood, FRAMES);

        let slowest = frames
            .windows(2)
            .map(|pair| pair[1].0 - pair[0].0)
            .max()
            .unwrap();
        assert!(
            slowest < FRAME_PERIOD / 2,
            "a frame took {slowest:?} with logging on"
        );
        let midi_lines = fs::read_to_string(dir.join("events.log"))
            .unwrap()
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["type"] == "midi")
            .count();
        assert_eq!(midi_lines, 250 * FRAMES as usize, "every event is logged");
        fs::remove_dir_all(dir).unwrap();
    }
}

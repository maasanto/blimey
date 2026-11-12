//! What `blimey run` resumes with after a restart (plan §9): the last selected song and the
//! master level, saved in `state.json` in the state folder whenever they change.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use blimey_engine::{MidiEvent, MidiSource, Show, Status};
use serde_json::{Value, json};

const FILE_NAME: &str = "state.json";
const PROGRAM_CHANGE: u8 = 0xC1;
const CONTROL_CHANGE: u8 = 0xB1;
const MASTER_CONTROLLER: u8 = 7;
const MIDI_DATA_MAX: f32 = 127.0;

/// Where `blimey run` keeps its state and event log unless told otherwise: outside the show
/// folder, which stays read-only in show mode.
pub fn default_dir() -> io::Result<PathBuf> {
    let home = std::env::home_dir().ok_or_else(|| {
        io::Error::other("HOME is not set; pass --state-dir to say where to keep the state")
    })?;
    Ok(home.join("Library/Application Support/Blimey"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedState {
    pub song: Option<String>,
    pub master: f32,
}

impl Default for SavedState {
    /// A fresh engine's state: no song, master at full.
    fn default() -> Self {
        SavedState {
            song: None,
            master: 1.0,
        }
    }
}

impl SavedState {
    /// The channel-2 MIDI that takes a fresh engine to this state, due at once: the song's
    /// Program Change, then CC 7 for the master. Cues are not resumed: the engine waits for
    /// the next one.
    pub fn resume_events(&self, show: &Show) -> Vec<MidiEvent> {
        let event = |message: Vec<u8>| MidiEvent {
            at: Duration::ZERO,
            message,
        };
        let mut events = Vec::new();
        if let Some(name) = &self.song {
            match show.songs.iter().find(|song| &song.name == name) {
                Some(song) => events.push(event(vec![PROGRAM_CHANGE, song.program])),
                None => eprintln!("state: the saved song {name:?} is not in the show; no song"),
            }
        }
        if self.master < 1.0 {
            let value = (self.master.clamp(0.0, 1.0) * MIDI_DATA_MAX).round() as u8;
            events.push(event(vec![CONTROL_CHANGE, MASTER_CONTROLLER, value]));
        }
        events
    }
}

pub struct StateFile {
    path: PathBuf,
    saved: SavedState,
    is_failure_reported: bool,
}

impl StateFile {
    /// Reads the saved state. Without a state file the engine starts fresh; an unreadable one
    /// is reported and also gives a fresh start, so a bad file never keeps the show dark.
    pub fn open(state_dir: &Path) -> StateFile {
        let path = state_dir.join(FILE_NAME);
        let saved = match fs::read_to_string(&path) {
            Ok(text) => parse(&text).unwrap_or_else(|| {
                eprintln!("state: cannot read {}; starting fresh", path.display());
                SavedState::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => SavedState::default(),
            Err(error) => {
                eprintln!(
                    "state: cannot read {}: {error}; starting fresh",
                    path.display()
                );
                SavedState::default()
            }
        };
        StateFile {
            path,
            saved,
            is_failure_reported: false,
        }
    }

    pub fn saved(&self) -> &SavedState {
        &self.saved
    }

    /// Saves the song and master of `status` if they changed. Never fails: a write error is
    /// reported once on stderr and retried on the next change.
    pub fn record(&mut self, status: &Status) {
        let state = SavedState {
            song: status.song.clone(),
            master: status.master,
        };
        if state == self.saved {
            return;
        }
        let text = json!({ "song": state.song, "master": state.master }).to_string();
        match write_atomically(&self.path, &text) {
            Ok(()) => {
                self.saved = state;
                self.is_failure_reported = false;
            }
            Err(error) if !self.is_failure_reported => {
                eprintln!("state: cannot write {}: {error}", self.path.display());
                self.is_failure_reported = true;
            }
            Err(_) => {}
        }
    }
}

fn parse(text: &str) -> Option<SavedState> {
    let state: Value = serde_json::from_str(text).ok()?;
    Some(SavedState {
        song: state["song"].as_str().map(String::from),
        master: state["master"].as_f64()? as f32,
    })
}

/// A crash mid-write leaves the old state or the new one.
// ponytail: no fsync, which guards against power loss rather than a crash and would cost a disk
// flush per frame during a master fade. Add one if power cuts ever lose the state.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

/// `midi` with `first` delivered before anything it sends.
pub struct Resumed<M> {
    first: std::vec::IntoIter<MidiEvent>,
    midi: M,
}

impl<M> Resumed<M> {
    pub fn new(first: Vec<MidiEvent>, midi: M) -> Self {
        Resumed {
            first: first.into_iter(),
            midi,
        }
    }
}

impl<M: MidiSource> MidiSource for Resumed<M> {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        self.first.next().or_else(|| self.midi.poll(until))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blimey_engine::look::Look;
    use blimey_engine::{FakeClock, FakeDmx, run};

    const SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues");

    /// Records the state like `blimey run` does, and keeps the frames.
    struct Persisted {
        state: StateFile,
        frames: FakeDmx,
    }

    impl blimey_engine::DmxSink for Persisted {
        fn send(&mut self, at: Duration, frame: &blimey_engine::Frame) -> io::Result<()> {
            self.frames.send(at, frame)
        }

        fn status(&mut self, status: &Status) {
            self.state.record(status);
        }
    }

    fn events(messages: &[&[u8]]) -> Vec<MidiEvent> {
        messages
            .iter()
            .map(|message| MidiEvent {
                at: Duration::ZERO,
                message: message.to_vec(),
            })
            .collect()
    }

    /// Runs one frame of `show` after `messages`, recording the state in `dir`.
    fn run_once(dir: &Path, show: &Show, messages: &[&[u8]]) -> blimey_engine::Frame {
        let state = StateFile::open(dir);
        let resume = state.saved().resume_events(show);
        let mut sink = Persisted {
            state,
            frames: FakeDmx::default(),
        };
        let mut midi = Resumed::new(resume, Resumed::new(events(messages), NoMidi));
        run(
            &mut FakeClock::default(),
            &mut midi,
            show,
            &Look::default(),
            &mut sink,
            1,
        )
        .unwrap();
        sink.frames.frames[0].values
    }

    struct NoMidi;

    impl MidiSource for NoMidi {
        fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
            None
        }
    }

    #[test]
    fn a_restarted_engine_resumes_the_song_and_master_and_waits_for_the_next_cue() {
        let dir = std::env::temp_dir().join(format!("blimey-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let show = Show::load(Path::new(SHOW)).unwrap();
        let blue_snap: &[u8] = &[0x91, 37, 127];

        let before_restart = run_once(&dir, &show, &[&[0xC1, 1], &[0xB1, 7, 64], blue_snap]);
        let after_restart = run_once(&dir, &show, &[]);
        let next_cue = run_once(&dir, &show, &[blue_snap]);

        assert_eq!(before_restart[0..4], [129, 0, 0, 255], "dimmer at 64/127");
        assert_eq!(after_restart[0..4], [0, 0, 0, 0], "no cue resumed");
        assert_eq!(next_cue, before_restart, "song and master resumed");
        assert_eq!(
            StateFile::open(&dir).saved(),
            &SavedState {
                song: Some("first".into()),
                master: 64.0 / 127.0
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_state_file_gives_a_fresh_start() {
        let dir = std::env::temp_dir().join(format!("blimey-state-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "{\"song\": \"first\", \"mas").unwrap();

        assert_eq!(StateFile::open(&dir).saved(), &SavedState::default());
        fs::remove_dir_all(dir).unwrap();
    }
}

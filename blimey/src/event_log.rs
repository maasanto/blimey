//! The event log for post-gig diagnosis (plan §9): one JSON object per line for every MIDI
//! event and every cue start and release, in `events.log` in the state folder. Each line is
//! written with one `write` call and no buffering, so a crash loses nothing already logged.
//! The file rotates to `events.log.1` … `events.log.4` past `ROTATE_AT_BYTES`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blimey_engine::{CueEvent, MidiEvent};
use serde_json::{Value, json};

use crate::stream::is_ticking;

const FILE_NAME: &str = "events.log";
/// About a day of a busy show without MIDI clock ticks, which are not logged.
const ROTATE_AT_BYTES: u64 = 10_000_000;
const KEPT_FILES: u32 = 4;

pub struct EventLog {
    path: PathBuf,
    file: File,
    length: u64,
    is_failure_reported: bool,
}

impl EventLog {
    pub fn open(state_dir: &Path) -> io::Result<EventLog> {
        let path = state_dir.join(FILE_NAME);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(EventLog {
            length: file.metadata()?.len(),
            path,
            file,
            is_failure_reported: false,
        })
    }

    /// The first line of a run: what it plays, and the song and master it resumed with.
    pub fn start(&mut self, show_dir: &Path, mode: &str, song: Option<&str>, master: f32) {
        self.write(json!({
            "type": "start",
            "show": show_dir,
            "mode": mode,
            "song": song,
            "master": master,
        }));
    }

    /// MIDI clock ticks and active sensing are left out, as in the stream: they arrive
    /// dozens of times a second.
    pub fn midi(&mut self, event: &MidiEvent) {
        if !is_ticking(&event.message) {
            self.write(json!({
                "type": "midi",
                "at_ms": event.at.as_millis() as u64,
                "message": event.message,
            }));
        }
    }

    pub fn cue(&mut self, event: &CueEvent) {
        self.write(json!({
            "type": "cue",
            "at_ms": event.at.as_millis() as u64,
            "action": event.action,
            "song": event.song,
            "note": event.note,
            "name": event.name,
        }));
    }

    /// Never fails: an error is reported once until the next successful write, and the engine
    /// keeps going. A failed rotation keeps logging to the current file.
    fn write(&mut self, mut line: Value) {
        let unix_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        line["time_ms"] = json!(unix_time.as_millis() as u64);
        let mut text = line.to_string();
        text.push('\n');
        if let Err(error) = self.rotate_if_full() {
            // Retried after another ROTATE_AT_BYTES, not on every line.
            self.length = 0;
            self.report(format_args!("cannot rotate: {error}; still logging to it"));
        }
        match self.file.write_all(text.as_bytes()) {
            Ok(()) => {
                self.length += text.len() as u64;
                self.is_failure_reported = false;
            }
            Err(error) => self.report(format_args!("cannot write: {error}; still trying")),
        }
    }

    fn report(&mut self, problem: std::fmt::Arguments) {
        if !self.is_failure_reported {
            eprintln!("log: {}: {problem}", self.path.display());
            self.is_failure_reported = true;
        }
    }

    fn rotate_if_full(&mut self) -> io::Result<()> {
        if self.length < ROTATE_AT_BYTES {
            return Ok(());
        }
        let numbered = |number: u32| self.path.with_extension(format!("log.{number}"));
        for number in (1..KEPT_FILES).rev() {
            if numbered(number).exists() {
                fs::rename(numbered(number), numbered(number + 1))?;
            }
        }
        fs::rename(&self.path, numbered(1))?;
        self.file = File::create(&self.path)?;
        self.length = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use blimey_engine::{MidiFile, Show};

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

    fn scratch_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blimey-log-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lines(path: &Path) -> Vec<Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Logs every event the render loop hands its sink, as `blimey run` does.
    struct Logged(EventLog);

    impl blimey_engine::DmxSink for Logged {
        fn send(&mut self, _at: Duration, _frame: &blimey_engine::Frame) -> io::Result<()> {
            Ok(())
        }

        fn midi(&mut self, event: &MidiEvent) {
            self.0.midi(event);
        }

        fn cue(&mut self, event: &CueEvent) {
            self.0.cue(event);
        }
    }

    #[test]
    fn a_replay_logs_every_midi_event_and_each_cue_start_and_release() {
        let dir = scratch_dir("replay");
        let show = Show::load(&Path::new(FIXTURES).join("shows/cues")).unwrap();
        let midi_file = || MidiFile::open(&Path::new(FIXTURES).join("midi/cue-fade.mid")).unwrap();
        let mut sent = Vec::new();
        let mut file = midi_file();
        while let Some(event) = blimey_engine::MidiSource::poll(&mut file, Duration::MAX) {
            sent.push(json!({ "type": "midi", "at_ms": event.at.as_millis() as u64, "message": event.message }));
        }
        let mut sink = Logged(EventLog::open(&dir).unwrap());
        let frame_count = file.duration().as_millis() as u32 / 25 + 1;

        blimey_engine::run(
            &mut blimey_engine::FakeClock::default(),
            &mut midi_file(),
            &show,
            &Default::default(),
            &mut sink,
            frame_count,
        )
        .unwrap();

        let mut logged = lines(&dir.join(FILE_NAME));
        for line in &mut logged {
            assert!(line["time_ms"].as_u64().unwrap() > 0, "{line}");
            line.as_object_mut().unwrap().remove("time_ms");
        }
        let midi: Vec<&Value> = logged
            .iter()
            .filter(|line| line["type"] == "midi")
            .collect();
        assert_eq!(midi, sent.iter().collect::<Vec<_>>());
        let cues: Vec<(&Value, &Value)> = logged
            .iter()
            .filter(|line| line["type"] == "cue")
            .map(|line| (&line["action"], &line["name"]))
            .collect();
        assert_eq!(
            cues,
            [
                (&json!("start"), &json!("red-fade")),
                (&json!("release"), &json!("red-fade"))
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_full_log_rotates_and_keeps_four_old_files() {
        let dir = scratch_dir("rotate");
        let mut log = EventLog::open(&dir).unwrap();
        let event = MidiEvent {
            at: Duration::ZERO,
            message: vec![0x91, 36, 127],
        };

        for _ in 0..6 {
            log.length = ROTATE_AT_BYTES;
            log.midi(&event);
        }

        let mut files: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "events.log",
                "events.log.1",
                "events.log.2",
                "events.log.3",
                "events.log.4"
            ]
        );
        assert_eq!(lines(&dir.join(FILE_NAME)).len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_rotation_that_fails_keeps_logging_to_the_current_file() {
        let dir = scratch_dir("rotate-fails");
        let mut log = EventLog::open(&dir).unwrap();
        // A file can't be renamed over a folder that has something in it.
        fs::write(dir.join("events.log.3"), "").unwrap();
        fs::create_dir_all(dir.join("events.log.4/in-the-way")).unwrap();
        let event = |note| MidiEvent {
            at: Duration::ZERO,
            message: vec![0x91, note, 127],
        };

        log.length = ROTATE_AT_BYTES;
        log.midi(&event(36));
        log.midi(&event(37));

        let notes: Vec<Value> = lines(&dir.join(FILE_NAME))
            .iter()
            .map(|line| line["message"][1].clone())
            .collect();
        assert_eq!(notes, [json!(36), json!(37)]);
        fs::remove_dir_all(dir).unwrap();
    }
}

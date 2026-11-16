//! Stress tests from plan §15: corrupted show files and a MIDI flood must never crash the
//! engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use blimey_engine::look::Look;
use blimey_engine::{FRAME_PERIOD, FakeClock, FakeDmx, MidiEvent, MidiSource, Show, run};

const SHOWS: [&str; 2] = [
    "../tests/fixtures/shows/demo",
    "../tests/fixtures/shows/cues",
];

/// xorshift64: a fixed seed, so a failure replays exactly.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn yaml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(yaml_files(&path));
        } else if path
            .extension()
            .is_some_and(|extension| extension == "yaml")
        {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// One of: cut the file short, overwrite a byte, insert a YAML-significant character, or
/// duplicate a line.
fn corrupt(text: &[u8], random: &mut Random) -> Vec<u8> {
    let mut bytes = text.to_vec();
    let at = random.below(bytes.len() + 1);
    match random.below(4) {
        0 => bytes.truncate(at),
        1 if at < bytes.len() => bytes[at] = random.byte(),
        2 => {
            const SIGNIFICANT: &[u8] = b":-{}[]#&*!|>'\"%@` \n\t0123456789";
            bytes.insert(at, SIGNIFICANT[random.below(SIGNIFICANT.len())]);
        }
        _ => {
            let line_start = bytes[..at]
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(0, |newline| newline + 1);
            let line_end = bytes[at..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |newline| at + newline + 1);
            let line = bytes[line_start..line_end].to_vec();
            bytes.splice(line_start..line_start, line);
        }
    }
    bytes
}

#[test]
fn corrupted_show_files_load_or_fail_with_an_error_naming_a_show_file() {
    let mut random = Random(0x5EED);
    for show in SHOWS {
        let original = Path::new(env!("CARGO_MANIFEST_DIR")).join(show);
        let scratch = std::env::temp_dir().join(format!(
            "blimey-fuzz-{}-{}",
            original.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&scratch);
        copy_dir(&original, &scratch);
        for file in yaml_files(&scratch) {
            let text = fs::read(&file).unwrap();
            for _ in 0..200 {
                let corrupted = corrupt(&text, &mut random);
                fs::write(&file, &corrupted).unwrap();
                if let Err(error) = Show::load(&scratch) {
                    // Not always the corrupted file: a group cut from the rig fails the song
                    // that uses it.
                    assert!(
                        error.to_string().contains(".yaml"),
                        "{}: error names no show file: {error}\n--- content ---\n{}",
                        file.display(),
                        String::from_utf8_lossy(&corrupted)
                    );
                }
            }
            fs::write(&file, &text).unwrap();
        }
        fs::remove_dir_all(scratch).unwrap();
    }
}

/// `per_frame` random messages due by each frame: every status byte on every channel, data
/// bytes out of range, and messages cut short or run long, as a broken cable or driver might
/// deliver.
struct Flood {
    random: Random,
    per_frame: usize,
    sent_for: Option<Duration>,
    pending: usize,
}

impl MidiSource for Flood {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        if self.sent_for != Some(until) {
            self.sent_for = Some(until);
            self.pending = self.per_frame;
        }
        if self.pending == 0 {
            return None;
        }
        self.pending -= 1;
        let length = self.random.below(5);
        let mut message: Vec<u8> = (0..length).map(|_| self.random.byte()).collect();
        if let Some(status) = message.first_mut()
            && self.random.below(2) == 0
        {
            *status |= 0x80;
        }
        Some(MidiEvent { at: until, message })
    }
}

#[test]
fn a_flood_of_10_000_malformed_midi_events_a_second_keeps_every_frame() {
    const SECONDS: u32 = 10;
    const EVENTS_PER_SECOND: u32 = 10_000;
    let frame_count = SECONDS * 40;
    for show in SHOWS {
        let show = Show::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(show)).unwrap();
        let mut midi = Flood {
            random: Random(0xF100D),
            per_frame: (EVENTS_PER_SECOND / 40) as usize,
            sent_for: None,
            pending: 0,
        };
        let mut sink = FakeDmx::default();
        let started = Instant::now();

        run(
            &mut FakeClock::default(),
            &mut midi,
            &show,
            &Look::default(),
            &mut sink,
            frame_count,
        )
        .unwrap();

        let elapsed = started.elapsed();
        assert_eq!(sink.frames.len(), frame_count as usize);
        // Each frame's work must fit its 25 ms slot, or the live engine would drop frames.
        assert!(
            elapsed < FRAME_PERIOD * frame_count,
            "{SECONDS} s of flood took {elapsed:?} to render"
        );
    }
}

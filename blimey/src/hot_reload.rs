//! Edit mode's hot reload: the show is reloaded when a file in its folder changes. A show that
//! fails to load is reported and the previous one keeps running.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use blimey_engine::{Show, ShowSource};

pub struct HotReload {
    dir: PathBuf,
    show: Show,
    stamps: Vec<FileStamp>,
}

type FileStamp = (PathBuf, Option<SystemTime>, u64);

impl HotReload {
    pub fn load(dir: &Path) -> io::Result<HotReload> {
        Ok(HotReload {
            dir: dir.to_path_buf(),
            stamps: stamps(dir),
            show: Show::load(dir)?,
        })
    }
}

impl ShowSource for HotReload {
    /// Checks the folder on every frame: a few `stat`s, far below the frame budget.
    fn current(&mut self) -> &Show {
        let stamps = stamps(&self.dir);
        if stamps != self.stamps {
            self.stamps = stamps;
            match Show::load(&self.dir) {
                Ok(show) => {
                    self.show = show;
                    eprintln!("blimey: reloaded {}", self.dir.display());
                }
                Err(error) => eprintln!("blimey: {error}; still running the previous show"),
            }
        }
        &self.show
    }
}

/// Path, modification time and size of every file under `dir`, sorted.
fn stamps(dir: &Path) -> Vec<FileStamp> {
    let mut stamps = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return stamps;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            stamps.extend(self::stamps(&entry.path()));
        } else {
            stamps.push((entry.path(), metadata.modified().ok(), metadata.len()));
        }
    }
    stamps.sort();
    stamps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use blimey_engine::look::Look;
    use blimey_engine::{FakeClock, FakeDmx, MidiEvent, MidiSource};

    const SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues");

    fn song(colour: &str) -> String {
        format!(
            "program: 1\ncues:\n  36:\n    name: look\n    type: scene\n    values:\n      dimmer-par: {{ color: {colour} }}\n"
        )
    }

    /// Program 1 and note 36 at 0 ms, then saves `edits` to the song file at their times, as an
    /// editor would while the show runs.
    struct EditWhilePlaying {
        events: Vec<MidiEvent>,
        edits: Vec<(Duration, PathBuf, String)>,
    }

    impl MidiSource for EditWhilePlaying {
        fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
            if let Some((at, path, text)) = self.edits.first()
                && *at <= until
            {
                fs::write(path, text).unwrap();
                self.edits.remove(0);
            }
            (!self.events.is_empty()).then(|| self.events.remove(0))
        }
    }

    fn outputs(edit: &[&str]) -> crate::Outputs {
        use clap::Parser;
        let args = ["blimey", "run", SHOW, "--artnet"].iter().chain(edit);
        match crate::Command::parse_from(args) {
            crate::Command::Run { outputs, .. } => outputs,
            _ => unreachable!("parsed as `run`"),
        }
    }

    /// A scratch copy of the cues show's rig, palettes and profiles, with no songs.
    fn show_copy(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blimey-{test}-{}", std::process::id()));
        fs::create_dir_all(dir.join("fixtures")).unwrap();
        fs::create_dir_all(dir.join("songs")).unwrap();
        for file in [
            "rig.yaml",
            "palettes.yaml",
            "fixtures/dimmer-par.yaml",
            "fixtures/rgb-par.yaml",
        ] {
            fs::copy(Path::new(SHOW).join(file), dir.join(file)).unwrap();
        }
        dir
    }

    /// Program 1 and note 36, at 0 ms.
    fn fire_note_36() -> Vec<MidiEvent> {
        let event = |message: &[u8]| MidiEvent {
            at: Duration::ZERO,
            message: message.to_vec(),
        };
        vec![event(&[0xC1, 1]), event(&[0x91, 36, 127])]
    }

    #[test]
    fn an_edited_bpm_paces_a_running_beat_fade_from_the_next_frame() {
        let dir = show_copy("hot-reload-bpm");
        let song_file = dir.join("songs/song.yaml");
        let song = |bpm| {
            format!(
                "program: 1\nbpm: {bpm}\ncues:\n  36:\n    name: fade\n    type: scene\n    fade_in: 1 beat\n    values:\n      rgb-par: {{ color: \"#ffffff\" }}\n"
            )
        };
        fs::write(&song_file, song(60)).unwrap();
        let mut midi = EditWhilePlaying {
            events: fire_note_36(),
            edits: vec![(Duration::from_millis(475), song_file, song(120))],
        };
        let mut sink = FakeDmx::default();

        blimey_engine::run(
            &mut FakeClock::default(),
            &mut midi,
            HotReload::load(&dir).unwrap(),
            &Look::default(),
            &mut sink,
            31,
        )
        .unwrap();

        let red_at = |ms: usize| sink.frames[ms / 25].values[4];
        assert_eq!(
            [red_at(250), red_at(500), red_at(625), red_at(750)],
            [64, 128, 191, 255],
            "half a beat at 60 BPM, then the other half at 120 BPM from the reload at 500 ms"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// `dimmer-par`'s red, green and blue over six frames of a song showing red, saved blue at
    /// 50 ms, then saved broken at 100 ms.
    fn colours_while_editing(outputs: &crate::Outputs, test: &str) -> Vec<[u8; 3]> {
        let dir = show_copy(test);
        let song_file = dir.join("songs/song.yaml");
        fs::write(&song_file, song("red")).unwrap();
        let mut midi = EditWhilePlaying {
            events: fire_note_36(),
            edits: vec![
                (Duration::from_millis(50), song_file.clone(), song("blue")),
                (
                    Duration::from_millis(100),
                    song_file.clone(),
                    song("not-a-colour"),
                ),
            ],
        };
        let mut sink = FakeDmx::default();

        blimey_engine::run(
            &mut FakeClock::default(),
            &mut midi,
            outputs.shows(&dir).unwrap(),
            &Look::default(),
            &mut sink,
            6,
        )
        .unwrap();

        fs::remove_dir_all(dir).unwrap();
        sink.frames
            .iter()
            .map(|frame| frame.values[1..4].try_into().unwrap())
            .collect()
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    #[test]
    fn in_edit_mode_saving_a_song_updates_the_look_and_a_broken_save_keeps_the_previous_show() {
        assert_eq!(
            colours_while_editing(&outputs(&["--edit"]), "edit-mode"),
            [RED, RED, RED, BLUE, BLUE, BLUE],
            "the save at 50 ms shows from the next frame; the broken one at 100 ms changes nothing"
        );
    }

    #[test]
    fn in_show_mode_saving_a_song_changes_nothing() {
        assert_eq!(colours_while_editing(&outputs(&[]), "show-mode"), [RED; 6]);
    }
}

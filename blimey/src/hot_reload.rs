//! Edit mode's hot reload: the show is reloaded when a file in its folder changes. A show that
//! fails to load is reported and the previous one keeps running.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use blimey_engine::arrangement::{self, EXPORT_DIR};
use blimey_engine::{Show, ShowSource};

use crate::state::LastGoodShow;

pub struct HotReload {
    dir: PathBuf,
    show: Show,
    stamps: Vec<FileStamp>,
    /// Where each show that reloads is kept, if anywhere.
    last_good: Option<LastGoodShow>,
    reloads: u64,
}

pub type FileStamp = (PathBuf, Option<SystemTime>, u64);

impl HotReload {
    pub fn load(dir: &Path) -> io::Result<HotReload> {
        let (stamps, show) = load_keeping(dir, None);
        let show = show?;
        export_arrangements(dir, &show);
        Ok(HotReload {
            dir: dir.to_path_buf(),
            stamps,
            show,
            last_good: None,
            reloads: 0,
        })
    }

    /// Plays `show`, loaded from `dir` or standing in for it, until `dir` changes from
    /// `stamps`, then reloads from `dir`, keeping each show that loads in `last_good`.
    pub fn new(
        dir: &Path,
        stamps: Vec<FileStamp>,
        show: Show,
        last_good: Option<LastGoodShow>,
    ) -> HotReload {
        HotReload {
            dir: dir.to_path_buf(),
            stamps,
            show,
            last_good,
            reloads: 0,
        }
    }
}

impl ShowSource for HotReload {
    fn current(&mut self) -> &Show {
        self.current_with_loads().0
    }

    /// Checks the folder on every frame: a few `stat`s, far below the frame budget.
    fn current_with_loads(&mut self) -> (&Show, u64) {
        if stamps(&self.dir) != self.stamps {
            let (stamps, loaded) = load_keeping(&self.dir, self.last_good.as_ref());
            self.stamps = stamps;
            match loaded {
                Ok(show) => {
                    export_arrangements(&self.dir, &show);
                    self.show = show;
                    self.reloads += 1;
                    eprintln!("blimey: reloaded {}", self.dir.display());
                }
                Err(error) => eprintln!("blimey: {error}; still running the previous show"),
            }
        }
        (&self.show, self.reloads)
    }
}

/// Loads the show in `dir`, and the stamps of its files from before, so a change during the
/// load is seen as one. A show that loads is kept in `last_good`, as read just before, unless
/// the folder changed meanwhile: the copy is always a show that loaded.
pub fn load_keeping(
    dir: &Path,
    last_good: Option<&LastGoodShow>,
) -> (Vec<FileStamp>, io::Result<Show>) {
    let before = stamps(dir);
    let files = last_good.and_then(|last_good| last_good.read(dir));
    let show = Show::load(dir);
    if let (Ok(_), Some(last_good), Some(files)) = (&show, last_good, files)
        && stamps(dir) == before
    {
        last_good.save(&files);
    }
    (before, show)
}

/// Edit mode writes the MIDI file of each song with an arrangement on every load, so Reaper's
/// copy follows the saves. A file that can't be written is reported; the show plays on.
pub fn export_arrangements(dir: &Path, show: &Show) {
    for song in &show.songs {
        if let Err(error) = arrangement::export(dir, song) {
            eprintln!(
                "blimey: {error}; the MIDI file of {} for Reaper is out of date",
                song.name
            );
        }
    }
}

/// Path, modification time and size of every file under `dir`, sorted. Leaves out the export
/// folders, which loading writes to.
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
            if entry.file_name() != EXPORT_DIR {
                stamps.extend(self::stamps(&entry.path()));
            }
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

    #[test]
    fn edit_mode_exports_an_arranged_song_on_load_and_on_each_reload_without_reloading_for_it() {
        let dir = show_copy("hot-reload-export");
        let song_file = dir.join("songs/arranged.yaml");
        let arranged =
            fs::read_to_string(Path::new(SHOW).join("../arranged/songs/arranged.yaml")).unwrap();
        fs::write(&song_file, &arranged).unwrap();
        let exported = dir.join(EXPORT_DIR).join("arranged.mid");

        let mut shows = HotReload::load(&dir).unwrap();
        let at_120_bpm = fs::read(&exported).unwrap();
        assert_eq!(shows.current_with_loads().1, 0, "the export is no edit");

        fs::write(&song_file, arranged.replace("bpm: 120", "bpm: 60")).unwrap();
        assert_eq!(shows.current_with_loads().1, 1);
        let at_60_bpm = fs::read(&exported).unwrap();
        assert_ne!(at_60_bpm, at_120_bpm, "the save is exported");
        assert_eq!(
            at_60_bpm,
            arrangement::midi_file(&shows.current().songs[0]).unwrap()
        );
        assert_eq!(shows.current_with_loads().1, 1);
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

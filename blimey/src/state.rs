//! What `blimey run` resumes with after a restart (plan §9): the last selected song and the
//! master level, saved in `state.json` in the state folder whenever they change; and the last
//! show that loaded, for a show that no longer does.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use blimey_engine::{MidiEvent, MidiSource, Show, Status, resume_messages};
use serde_json::{Value, json};

const FILE_NAME: &str = "state.json";

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
    pub fn of(status: &Status) -> SavedState {
        SavedState {
            song: status.song.clone(),
            master: status.master,
        }
    }

    /// The channel-2 MIDI that takes a fresh engine to this state, due at once: the song's
    /// Program Change, then CC 7 for the master. Cues are not resumed: the engine waits for
    /// the next one.
    pub fn resume_events(&self, show: &Show) -> Vec<MidiEvent> {
        let program = self.song.as_ref().and_then(|name| {
            let song = show.songs.iter().find(|song| &song.name == name);
            if song.is_none() {
                eprintln!("state: the saved song {name:?} is not in the show; no song");
            }
            Some(song?.program)
        });
        resume_messages(program, self.master)
            .into_iter()
            .map(|message| MidiEvent {
                at: Duration::ZERO,
                message,
            })
            .collect()
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

    /// Never fails: a write error is reported once until a write succeeds again.
    pub fn save(&mut self, state: &SavedState) {
        let text = json!({ "song": state.song, "master": state.master }).to_string();
        match write_atomically(&self.path, &text) {
            Ok(()) => self.is_failure_reported = false,
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

/// The files of the last show that loaded, kept in the state folder, so a show broken before a
/// gig starts as it last worked rather than not at all.
pub struct LastGoodShow {
    state_dir: PathBuf,
}

/// The kept show: its YAML files by path in the show folder.
const LAST_GOOD_SHOW: &str = "last-good-show.json";
/// Where `load` unpacks the kept files to load them.
const LAST_GOOD_SHOW_DIR: &str = "last-good-show";

impl LastGoodShow {
    pub fn new(state_dir: &Path) -> Self {
        LastGoodShow {
            state_dir: state_dir.to_path_buf(),
        }
    }

    /// Keeps the YAML files of `show_dir`, which just loaded, replacing the previous copy at
    /// once. Never fails: an error is reported, and the previous copy stays.
    pub fn save(&self, show_dir: &Path) {
        let mut files = BTreeMap::new();
        let saved = yaml_files(show_dir, Path::new(""), &mut files)
            .and_then(|()| fs::create_dir_all(&self.state_dir))
            .and_then(|()| write_atomically(&self.path(), serde_json::to_string(&files)?));
        if let Err(error) = saved {
            eprintln!(
                "state: cannot keep a copy of the show in {}: {error}",
                self.path().display()
            );
        }
    }

    /// The last show that loaded, or `None` if there is none or it doesn't load, reported.
    pub fn load(&self) -> Option<Show> {
        let text = match fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                eprintln!("state: cannot read {}: {error}", self.path().display());
                return None;
            }
        };
        let unpacked = self.state_dir.join(LAST_GOOD_SHOW_DIR);
        unpack(&text, &unpacked)
            .and_then(|()| Show::load(&unpacked))
            .map_err(|error| {
                eprintln!(
                    "state: cannot load the copy of the show kept in {}: {error}",
                    self.path().display()
                )
            })
            .ok()
    }

    fn path(&self) -> PathBuf {
        self.state_dir.join(LAST_GOOD_SHOW)
    }
}

/// Adds the `.yaml` and `.yml` files under `dir`, at `folder` in the show, to `files`.
fn yaml_files(dir: &Path, folder: &Path, files: &mut BTreeMap<PathBuf, String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let (path, name) = (entry.path(), folder.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            yaml_files(&path, &name, files)?;
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("yaml" | "yml")
        ) {
            files.insert(name, fs::read_to_string(&path)?);
        }
    }
    Ok(())
}

/// Writes the files of a kept show to `dir`, emptied first.
fn unpack(text: &str, dir: &Path) -> io::Result<()> {
    let files: BTreeMap<PathBuf, String> = serde_json::from_str(text)?;
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    for (name, contents) in files {
        if !name
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a path inside the show", name.display()),
            ));
        }
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap_or(dir))?;
        fs::write(path, contents)?;
    }
    Ok(())
}

/// A crash mid-write leaves the old state or the new one.
// ponytail: no fsync, which guards against power loss rather than a crash and would cost a disk
// flush per change during a master fade. Add one if power cuts ever lose the state.
pub fn write_atomically(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let temporary = path.with_added_extension("tmp");
    fs::write(&temporary, contents)?;
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

    #[test]
    fn a_corrupt_state_file_gives_a_fresh_start() {
        let dir = std::env::temp_dir().join(format!("blimey-state-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "{\"song\": \"first\", \"mas").unwrap();

        assert_eq!(StateFile::open(&dir).saved(), &SavedState::default());
        fs::remove_dir_all(dir).unwrap();
    }
}

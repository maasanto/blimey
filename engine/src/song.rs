//! Songs (plan §6): `songs/<name>.yaml`, each holding the Program Change that selects it and
//! the cue table its channel-2 notes fire.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_yaml_ng::Mapping;

use crate::Show;
use crate::cue::{Cue, CueEntry, resolve_look};
use crate::look::Look;

const MIDI_DATA_MAX: u8 = 127;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SongFile {
    program: u8,
    bpm: Option<f32>,
    #[serde(default)]
    safe_look: Mapping,
    #[serde(default)]
    cues: BTreeMap<u8, CueEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    /// The file name without `.yaml`.
    pub name: String,
    /// The channel-2 Program Change value, 0–127, that selects the song.
    pub program: u8,
    /// Tempo for beat durations when Reaper sends no MIDI clock.
    pub bpm: Option<f32>,
    /// What panic shows. An empty look leaves everything dark.
    pub safe_look: Look,
    /// Channel-2 note number → the cue it fires.
    pub cues: BTreeMap<u8, Cue>,
}

impl Song {
    /// Parses a song file. `show` holds the rig and palettes its looks name.
    pub fn parse(name: &str, yaml: &str, show: &Show) -> Result<Song, String> {
        let file: SongFile = serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())?;
        // Checks against the rig run after parsing, so serde can't point at the line: find it.
        let located = |key: &str, error: String| match line_of_key(yaml, key) {
            Some(line) => format!("line {line}: {error}"),
            None => error,
        };
        if file.program > MIDI_DATA_MAX {
            return Err(located("program", "program must be 0-127".into()));
        }
        if file.bpm.is_some_and(|bpm| bpm <= 0.0) {
            return Err(located("bpm", "bpm must be above 0".into()));
        }
        let safe_look = resolve_look(show, &file.safe_look)
            .map_err(|error| located("safe_look", format!("safe_look: {error}")))?;
        let mut cues = BTreeMap::new();
        for (slot, entry) in file.cues {
            let in_cue = |error: String| located(&slot.to_string(), format!("cue {slot}: {error}"));
            if slot > MIDI_DATA_MAX {
                return Err(in_cue("cue numbers are note numbers, 0-127".into()));
            }
            cues.insert(slot, entry.resolve(show).map_err(in_cue)?);
        }
        Ok(Song {
            name: name.into(),
            program: file.program,
            bpm: file.bpm,
            safe_look,
            cues,
        })
    }
}

/// The 1-based line that starts with `key:`. Finds keys of block-style YAML only.
fn line_of_key(yaml: &str, key: &str) -> Option<usize> {
    yaml.lines()
        .position(|line| {
            line.trim_start()
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
        })
        .map(|index| index + 1)
}

/// Loads every `*.yaml` in `dir`, in file-name order. A missing folder means no songs.
pub(crate) fn load_songs(dir: &Path, show: &Show) -> io::Result<Vec<Song>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("{}: {error}", dir.display()),
            ));
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "yaml")
        })
        .collect();
    paths.sort();
    let mut songs: Vec<Song> = Vec::new();
    for path in paths {
        let in_file = |error: String| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        };
        let yaml = fs::read_to_string(&path).map_err(|error| in_file(error.to_string()))?;
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let song = Song::parse(&name, &yaml, show).map_err(in_file)?;
        if let Some(other) = songs.iter().find(|other| other.program == song.program) {
            return Err(in_file(format!(
                "program {} already selects song {:?}",
                song.program, other.name
            )));
        }
        songs.push(song);
    }
    Ok(songs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show() -> Show {
        Show::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/fixtures/shows/cues"
        )))
        .unwrap()
    }

    fn error(yaml: &str) -> String {
        Song::parse("song", yaml, &show()).unwrap_err()
    }

    const SCENE: &str = "program: 3\ncues:\n  36:\n    name: verse\n    type: scene\n";

    #[test]
    fn a_scene_reads_its_fades_and_resolves_its_look() {
        let song = Song::parse(
            "song",
            &format!(
                "{SCENE}    fade_in: 2s\n    values: {{ both: {{ color: red, dimmer: 60% }} }}\n"
            ),
            &show(),
        )
        .unwrap();

        let Cue::Scene(scene) = &song.cues[&36];
        assert_eq!(scene.name, "verse");
        assert_eq!(scene.fade_in.duration().as_millis(), 2000);
        assert!(scene.release.duration().is_zero());
        assert_eq!(scene.look.fixtures.len(), 2);
        assert_eq!(scene.look.fixtures["rgb-par"].dimmer, 0.6);
    }

    #[test]
    fn checks_against_the_rig_name_the_line_of_the_cue() {
        assert_eq!(
            error(&format!(
                "{SCENE}    values:\n      both: {{ color: teal }}\n"
            )),
            "line 3: cue 36: both: no colour \"teal\" in palettes.yaml, and not a #rrggbb value"
        );
        assert_eq!(
            error(&format!(
                "{SCENE}    values:\n      back-wash: {{ color: red }}\n"
            )),
            "line 3: cue 36: back-wash: \"back-wash\" is neither a fixture nor a group in the rig"
        );
        assert_eq!(
            error("program: 3\nsafe_look:\n  both: { dimmer: 120% }\n"),
            "line 2: safe_look: both: level \"120%\" must be a percentage from 0% to 100%"
        );
    }

    #[test]
    fn file_format_errors_carry_serde_line_numbers() {
        assert_eq!(
            error(&format!(
                "{SCENE}    fade_in: 2 seconds\n    values: {{}}\n"
            )),
            "cues: duration \"2 seconds\" must be a number then ms or s, e.g. 500ms or 2s at line 3 column 3"
        );
        assert_eq!(
            error("program: 3\ncues:\n  36:\n    name: verse\n    type: scen\n"),
            "cues.36.type: unknown variant `scen`, expected `scene` at line 5 column 11"
        );
        assert_eq!(
            error("program: 3\ntempo: 92\n"),
            "unknown field `tempo`, expected one of `program`, `bpm`, `safe_look`, `cues` at line 2 column 1"
        );
    }

    #[test]
    fn rejects_numbers_outside_midi() {
        assert_eq!(error("program: 128\n"), "line 1: program must be 0-127");
        assert_eq!(
            error("program: 3\ncues:\n  128: { name: x, type: scene, values: {} }\n"),
            "line 3: cue 128: cue numbers are note numbers, 0-127"
        );
    }

    #[test]
    fn two_songs_on_one_program_are_refused() {
        let dir =
            std::env::temp_dir().join(format!("blimey-shared-program-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.yaml"), "program: 3\n").unwrap();
        fs::write(dir.join("b.yaml"), "program: 3\n").unwrap();

        let message = load_songs(&dir, &show()).unwrap_err().to_string();

        assert!(
            message.ends_with("b.yaml: program 3 already selects song \"a\""),
            "{message}"
        );
    }
}

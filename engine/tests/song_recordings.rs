//! Every recorded song of the real show replays to its reference dump (plan §9), found by
//! convention: `shows/fossile/recordings/<name>.mid` against
//! `tests/fixtures/dumps/fossile-<name>.dump`. `<name>` is the song's name, or
//! `<song>.synthetic` for a stand-in written by hand rather than recorded from Reaper.
//! See docs/migration.md for recording a song and creating its reference.

use std::fs;
use std::path::{Path, PathBuf};

use blimey_engine::{MidiFile, Show, first_difference, parse_dump, replay};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");

fn recordings(show_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(show_dir.join("recordings"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mid"))
        .collect();
    files.sort();
    files
}

#[test]
fn every_recorded_song_matches_its_reference_dump() {
    let show_dir = Path::new(ROOT).join("shows/fossile");
    let show = Show::load(&show_dir).unwrap();
    let recordings = recordings(&show_dir);
    assert!(
        !recordings.is_empty(),
        "no recordings in shows/fossile/recordings"
    );

    let mut failures = Vec::new();
    for recording in &recordings {
        let name = recording.file_stem().unwrap().to_string_lossy();
        let song = name.split('.').next().unwrap_or_default();
        if !show.songs.iter().any(|candidate| candidate.name == song) {
            failures.push(format!(
                "{name}.mid: shows/fossile/songs has no song {song:?}"
            ));
            continue;
        }
        let reference = Path::new(ROOT).join(format!("tests/fixtures/dumps/fossile-{name}.dump"));
        let Ok(text) = fs::read_to_string(&reference) else {
            failures.push(format!(
                "{name}.mid: no reference; create it with `blimey replay shows/fossile --midi shows/fossile/recordings/{name}.mid --out tests/fixtures/dumps/fossile-{name}.dump`"
            ));
            continue;
        };
        let frames = replay(&show, MidiFile::open(recording).unwrap());
        if let Some(difference) = first_difference(&frames, &parse_dump(&text).unwrap()) {
            failures.push(format!("{name}.mid: {difference}"));
        }
    }
    for song in &show.songs {
        let prefix = format!("{}.", song.name);
        let is_recorded = recordings.iter().any(|recording| {
            let file = recording.file_name().unwrap().to_string_lossy();
            file.starts_with(&prefix)
        });
        if !is_recorded {
            eprintln!("note: no recording yet for {}", song.name);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

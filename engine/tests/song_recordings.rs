//! Every recorded song of the demo show replays to its reference dump (plan §9), found by
//! convention: `tests/fixtures/shows/demo/recordings/<name>.mid` against
//! `tests/fixtures/dumps/demo-<name>.dump`. `<name>` is the song's name, or
//! `<song>.synthetic` for a stand-in written by hand rather than recorded from the DAW.
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
    let show_dir = Path::new(ROOT).join("tests/fixtures/shows/demo");
    let show = Show::load(&show_dir).unwrap();
    let recordings = recordings(&show_dir);
    assert!(
        !recordings.is_empty(),
        "no recordings in tests/fixtures/shows/demo/recordings"
    );

    let mut failures = Vec::new();
    for recording in &recordings {
        let name = recording.file_stem().unwrap().to_string_lossy();
        let song = song_of(&name);
        if !show.songs.iter().any(|candidate| candidate.name == song) {
            failures.push(format!(
                "{name}.mid: tests/fixtures/shows/demo/songs has no song {song:?}"
            ));
            continue;
        }
        let reference = Path::new(ROOT).join(format!("tests/fixtures/dumps/demo-{name}.dump"));
        let Ok(text) = fs::read_to_string(&reference) else {
            failures.push(format!(
                "{name}.mid: no reference; create it with `blimey replay tests/fixtures/shows/demo --midi tests/fixtures/shows/demo/recordings/{name}.mid --out tests/fixtures/dumps/demo-{name}.dump`"
            ));
            continue;
        };
        let frames = replay(&show, MidiFile::open(recording).unwrap());
        if let Some(difference) = first_difference(&frames, &parse_dump(&text).unwrap()) {
            failures.push(format!("{name}.mid: {difference}"));
        }
    }
    for song in &show.songs {
        let is_recorded = recordings.iter().any(|recording| {
            song_of(&recording.file_stem().unwrap().to_string_lossy()) == song.name
        });
        if !is_recorded {
            eprintln!("note: no recording yet for {}", song.name);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The song a recording named `name` (file name without `.mid`) is of.
fn song_of(name: &str) -> &str {
    name.strip_suffix(".synthetic").unwrap_or(name)
}

#[test]
fn only_a_trailing_synthetic_marks_a_stand_in() {
    assert_eq!(song_of("demo-song.synthetic"), "demo-song");
    assert_eq!(song_of("mr.blue"), "mr.blue");
    assert_eq!(song_of("mr.blue.synthetic"), "mr.blue");
}

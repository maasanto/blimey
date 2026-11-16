use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use blimey_engine::Show;
use blimey_engine::arrangement;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn export(show: &Path, song: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("export")
        .arg(show)
        .args(song)
        .output()
        .unwrap()
}

#[test]
fn export_writes_each_arranged_song_to_the_export_folder() {
    let fixture = Path::new(FIXTURES).join("shows/arranged");
    let show = Path::new(env!("CARGO_TARGET_TMPDIR")).join("arranged-export");
    let _ = fs::remove_dir_all(&show);
    for file in [
        "rig.yaml",
        "palettes.yaml",
        "fixtures/dimmer-par.yaml",
        "fixtures/rgb-par.yaml",
        "songs/arranged.yaml",
    ] {
        fs::create_dir_all(show.join(file).parent().unwrap()).unwrap();
        fs::copy(fixture.join(file), show.join(file)).unwrap();
    }

    let output = export(&show, &[]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let exported = show.join("export/arranged.mid");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", exported.display())
    );
    let song = &Show::load(&show).unwrap().songs[0];
    assert_eq!(
        fs::read(&exported).unwrap(),
        arrangement::midi_file(song).unwrap()
    );
}

#[test]
fn exporting_a_song_without_an_arrangement_says_so() {
    let output = export(&Path::new(FIXTURES).join("shows/cues"), &["first"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: song \"first\" has no arrangement to export; give it sections and an arrangement\n"
    );
}

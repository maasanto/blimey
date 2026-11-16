use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn validate(show: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("validate")
        .arg(show)
        .output()
        .unwrap()
}

#[test]
fn a_valid_show_passes() {
    let show = Path::new(FIXTURES).join("shows/cues");

    let output = validate(&show);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}: ok, 2 fixtures, 2 songs\n", show.display())
    );
}

#[test]
fn a_broken_song_names_its_file_and_line() {
    let show = Path::new(env!("CARGO_TARGET_TMPDIR")).join("show-with-a-typo");
    fs::create_dir_all(show.join("songs")).unwrap();
    let song = show.join("songs/demo-song.yaml");
    fs::write(
        &song,
        "program: 3\ncues:\n  36:\n    name: verse-blue\n    type: scene\n    fade_in: 2 secs\n    values: {}\n",
    )
    .unwrap();

    let output = validate(&show);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "error: {}: cues: duration \"2 secs\" must be a number then ms, s, beats or bars, e.g. 500ms, 2s, 0.5 beats or 4 bars at line 3 column 3\n",
            song.display()
        )
    );
}

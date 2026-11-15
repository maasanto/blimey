use std::fs;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn replay_empty_show(extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("replay")
        .arg(format!("{FIXTURES}/shows/empty"))
        .arg("--midi")
        .arg(format!("{FIXTURES}/midi/tempo-change.mid"))
        .args(extra_args)
        .output()
        .unwrap()
}

#[test]
fn empty_show_matches_the_all_zero_reference() {
    let reference = format!("{FIXTURES}/dumps/empty-show-tempo-change.dump");

    let output = replay_empty_show(&["--compare", &reference]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn replay_says_where_the_tempo_comes_from_on_stderr() {
    let output = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("replay")
        .arg(format!("{FIXTURES}/shows/cues"))
        .arg("--midi")
        .arg(format!("{FIXTURES}/midi/pattern-92.mid"))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("blimey: first: "), "{stderr}");
}

#[test]
fn out_writes_the_reference_byte_for_byte() {
    let out = format!("{}/empty-show.dump", env!("CARGO_TARGET_TMPDIR"));

    let output = replay_empty_show(&["--out", &out]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(out).unwrap(),
        fs::read_to_string(format!("{FIXTURES}/dumps/empty-show-tempo-change.dump")).unwrap()
    );
}

#[test]
fn legacy_song_matches_its_reference() {
    let output = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("replay")
        .arg(format!("{FIXTURES}/shows/legacy"))
        .arg("--midi")
        .arg(format!("{FIXTURES}/midi/legacy-song.mid"))
        .arg("--compare")
        .arg(format!("{FIXTURES}/dumps/legacy-song.dump"))
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compare_fails_on_the_first_differing_channel() {
    let reference = format!("{}/lit-channel.dump", env!("CARGO_TARGET_TMPDIR"));
    fs::write(&reference, "# blimey frame dump v1\n0\n25 7=128\n").unwrap();

    let output = replay_empty_show(&["--compare", &reference]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("frame 1 (25 ms), channel 7: expected 128, got 0"),
        "{stderr}"
    );
}

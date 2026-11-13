//! A short accelerated soak (plan §15) of every replay fixture: the CI stand-in for the
//! 100-hour run and the real-time 4 h soak on the M1 (docs/migration.md).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SETLIST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/setlists/cues.yaml"
);

fn soak(hours: &str, report: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
        .args([
            "soak",
            SETLIST,
            "--accelerated",
            "--hours",
            hours,
            "--report",
        ])
        .arg(report)
        .output()
        .unwrap()
}

fn report_path(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("soak/{name}.txt"));
    let _ = fs::remove_file(&path);
    path
}

#[test]
fn an_hour_of_the_replay_fixtures_drops_no_frame_drifts_nowhere_and_keeps_memory_flat() {
    let path = report_path("an-hour");

    let output = soak("1", &path);

    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && report.starts_with("Blimey soak: PASS"),
        "{report}"
    );
    assert!(report.contains("144000 rendered, 0 dropped"), "{report}");
    assert_eq!(fs::read_to_string(&path).unwrap(), report, "--report");
}

#[test]
fn a_soak_too_short_to_compare_passes_is_refused() {
    let output = soak("0.01", &report_path("too-short"));

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("fewer than the 3 passes"), "{stderr}");
}

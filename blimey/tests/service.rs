//! `blimey service install --dry-run`: refusals and the plist it would install. A dry run
//! touches neither launchd nor ~/Library.

use std::path::Path;
use std::process::{Command, Output};

const SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues");

fn dry_run(run_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
        .args(["service", "install", "--dry-run"])
        .args(run_args)
        .current_dir(env!("CARGO_TARGET_TMPDIR"))
        .output()
        .unwrap()
}

/// An agent that exits at once would be restarted by launchd every second, forever.
#[test]
fn an_engine_that_would_exit_at_start_is_refused() {
    for (run_args, error) in [
        (&[SHOW][..], "run needs somewhere to send frames"),
        (&["no-such-show", "--artnet"], "no-such-show"),
    ] {
        let output = dry_run(run_args);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{run_args:?} was accepted");
        assert!(stderr.contains(error), "{run_args:?}: {stderr}");
        assert!(output.stdout.is_empty(), "{run_args:?}: printed a plist");
    }
}

#[test]
fn a_relative_state_folder_logs_to_an_absolute_path() {
    let output = dry_run(&[SHOW, "--artnet", "--state-dir", "state"]);

    let plist = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = Path::new(env!("CARGO_TARGET_TMPDIR")).join("state/engine.log");
    assert!(
        plist.contains(&format!(
            "<key>StandardErrorPath</key>\n  <string>{}</string>",
            log.display()
        )),
        "{plist}"
    );
}

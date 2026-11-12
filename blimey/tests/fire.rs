use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};

use futures_util::StreamExt;
use serde_json::Value;
use tokio_tungstenite::connect_async;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn blimey() -> Command {
    Command::new(env!("CARGO_BIN_EXE_blimey"))
}

fn fire(show: &str, args: &[&str]) -> Output {
    blimey()
        .arg("fire")
        .arg(format!("{FIXTURES}/shows/{show}"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn setting_a_look_by_name_matches_the_reference_dump() {
    let output = fire(
        "four-washes",
        &["--set", "all-wash", "color=deep-blue", "dimmer=60%"],
    );

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reference = fs::read_to_string(format!(
        "{FIXTURES}/dumps/four-washes-all-wash-deep-blue-60.dump"
    ))
    .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), reference);
}

#[test]
fn overlapping_addresses_are_rejected_at_load() {
    let output = fire(
        "overlapping-addresses",
        &["--set", "wash-a", "color=#ff0000"],
    );

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(r#"fixtures "wash-a" (DMX 1-16) and "wash-b" (DMX 16-31) overlap"#),
        "{stderr}"
    );
}

#[test]
fn an_unknown_target_names_itself() {
    let output = fire("four-washes", &["--set", "back-wash", "color=deep-blue"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: --set back-wash: \"back-wash\" is neither a fixture nor a group in the rig\n"
    );
}

/// Kills the streaming engine when the test ends, pass or fail.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

#[tokio::test]
async fn streams_the_rig_then_frames_with_output_and_preview_colours() {
    let child = blimey()
        .arg("fire")
        .arg(format!("{FIXTURES}/shows/four-washes"))
        .args([
            "--set",
            "outer-wash",
            "color=#ff0000",
            "dimmer=100%",
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut engine = Running(child);
    let mut banner = String::new();
    BufReader::new(engine.0.stdout.take().unwrap())
        .read_line(&mut banner)
        .unwrap();
    let url = banner.trim().strip_prefix("streaming on ").unwrap();

    let (mut socket, _) = connect_async(url).await.unwrap();
    let mut next = async || -> Value {
        let message = socket.next().await.unwrap().unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    };

    let rig = next().await;
    assert_eq!(rig["type"], "rig");
    assert_eq!(rig["protocol"], 1);
    assert_eq!(rig["fixtures"][3]["name"], "wash-4");
    assert_eq!(rig["fixtures"][3]["address"], 101);
    assert_eq!(rig["fixtures"][3]["channels"][0], "Red 1");
    assert_eq!(rig["fixtures"][0]["pixels"].as_array().unwrap().len(), 4);
    assert_eq!(rig["groups"]["outer-wash"][1], "wash-4");

    let first = next().await;
    let second = next().await;
    assert_eq!(first["type"], "frame");
    assert!(second["at_ms"].as_u64() > first["at_ms"].as_u64());
    let dmx = second["dmx"].as_array().unwrap();
    assert_eq!(dmx.len(), 512);
    assert_eq!(
        (dmx[0].as_u64(), dmx[16].as_u64(), dmx[100].as_u64()),
        (Some(255), Some(0), Some(255))
    );
    assert_eq!(second["pixels"]["wash-1"][3], "#ff0000");
    assert_eq!(second["pixels"]["wash-2"][0], "#000000");
    assert_eq!(second["song"], Value::Null, "fire selects no song");
    assert_eq!(second["cues"], serde_json::json!([]));
    assert_eq!(second["master"], 1.0);
}

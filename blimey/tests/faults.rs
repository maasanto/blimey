//! Fault injection (plan §9, §15) on the real binary, against an emulated Enttec box on a pty
//! and a virtual CoreMIDI port standing in for the DAW: kill the engine, drop the MIDI port,
//! drop the serial port. macOS only, and ignored by a plain `cargo test`: run
//! `scripts/fault-injection.sh`.
#![cfg(target_os = "macos")]

#[allow(dead_code)]
#[path = "../src/enttec/emulator.rs"]
mod emulator;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use blimey_engine::Frame;
use emulator::EmulatedEnttec;
use midir::os::unix::VirtualOutput;
use midir::{MidiOutput, MidiOutputConnection};

const SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/cues");
/// Plan §9: a restarted engine is back within a second.
const RESTART_TARGET: Duration = Duration::from_secs(1);
const PATIENCE: Duration = Duration::from_secs(5);
/// The cues show's song 1, then master at 64/127.
const SELECT_SONG: [u8; 2] = [0xC1, 1];
const HALF_MASTER: [u8; 3] = [0xB1, 7, 64];
/// Cue 37 of song 1 snaps `dimmer-par` (DMX 1-4: dimmer, red, green, blue) to blue.
const BLUE_SNAP: [u8; 3] = [0x91, 37, 127];
const BLUE_AT_FULL: [u8; 4] = [255, 0, 0, 255];
const BLUE_AT_HALF: [u8; 4] = [129, 0, 0, 255];

/// A `blimey run` process in show mode, killed when dropped.
struct Engine(Child);

impl Engine {
    fn start(dmx: &EmulatedEnttec, midi_port: &str, state_dir: &Path) -> Engine {
        let child = Command::new(env!("CARGO_BIN_EXE_blimey"))
            .args(["run", SHOW, "--midi-port", midi_port, "--dmx-port"])
            .arg(dmx.path())
            .arg("--state-dir")
            .arg(state_dir)
            .stdout(Stdio::null())
            .spawn()
            .expect("start blimey");
        Engine(child)
    }
}

impl Drop for Engine {
    /// SIGKILL: the engine gets no chance to clean up, as in a crash.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The DAW's end of the MIDI link: a virtual source the engine connects to by name.
fn daw(port_name: &str) -> MidiOutputConnection {
    MidiOutput::new("blimey-faults")
        .unwrap()
        .create_virtual(port_name)
        .expect("create a virtual CoreMIDI port")
}

fn send(daw: &mut MidiOutputConnection, message: &[u8]) {
    daw.send(message).unwrap();
}

/// A name and a state folder no other test run shares.
fn scratch(test: &str) -> (String, PathBuf) {
    let name = format!("blimey-faults-{test}-{}", std::process::id());
    let dir = std::env::temp_dir().join(&name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    (name, dir)
}

/// Waits for a frame received from now on whose DMX 1-4 is `wanted` (or any frame, with
/// `None`), and returns how long that took.
fn wait_for(device: &EmulatedEnttec, wanted: Option<[u8; 4]>) -> Duration {
    let started = Instant::now();
    let seen = device.frames().len();
    let is_wanted = |frame: &Frame| wanted.is_none_or(|values| frame[0..4] == values);
    while started.elapsed() < PATIENCE {
        if device.frames()[seen..].iter().any(is_wanted) {
            return started.elapsed();
        }
        thread::sleep(Duration::from_millis(2));
    }
    let last = device.frames().last().map(|frame| frame[0..4].to_vec());
    panic!("no frame with DMX 1-4 = {wanted:?} within {PATIENCE:?}; last frame: {last:?}");
}

/// Waits until `state.json` in `state_dir` holds song 1 at half master, and returns how long
/// that took. The engine saves it on a background thread, a few milliseconds after the frame.
fn wait_for_saved_state(state_dir: &Path) -> Duration {
    let started = Instant::now();
    let is_saved = || {
        let text = fs::read_to_string(state_dir.join("state.json")).ok()?;
        let state: serde_json::Value = serde_json::from_str(&text).ok()?;
        let master = state["master"].as_f64()?;
        Some(state["song"] == "first" && (master - 64.0 / 127.0).abs() < 1e-6)
    };
    while started.elapsed() < PATIENCE {
        if is_saved() == Some(true) {
            return started.elapsed();
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("state.json never held song 1 at half master within {PATIENCE:?}");
}

#[test]
#[ignore = "needs macOS CoreMIDI and a pty: run scripts/fault-injection.sh"]
fn killed_engine_restarts_within_a_second_with_its_song_and_master() {
    let device = EmulatedEnttec::plug_in().unwrap();
    let (port_name, state_dir) = scratch("kill");
    let mut daw = daw(&port_name);
    let engine = Engine::start(&device, &port_name, &state_dir);
    wait_for(&device, None);
    for message in [&SELECT_SONG[..], &HALF_MASTER, &BLUE_SNAP] {
        send(&mut daw, message);
    }
    wait_for(&device, Some(BLUE_AT_HALF));
    // A crash before the save loses the change (docs/api.md): kill only once it is saved.
    let saved = wait_for_saved_state(&state_dir);

    drop(engine);
    let restarted = Instant::now();
    let _engine = Engine::start(&device, &port_name, &state_dir);
    wait_for(&device, None);
    let first_frame = restarted.elapsed();
    let dark = device.frames().last().unwrap()[0..4].to_vec();
    send(&mut daw, &BLUE_SNAP);
    let next_cue = wait_for(&device, Some(BLUE_AT_HALF));

    eprintln!(
        "state saved {saved:?} after the frame; restart to first frame: {first_frame:?}; next cue on output after {next_cue:?}"
    );
    assert!(
        first_frame < RESTART_TARGET,
        "first frame after {first_frame:?}"
    );
    assert_eq!(
        dark, [0; 4],
        "no cue resumed: the engine waits for the next one"
    );
    assert!(next_cue < RESTART_TARGET, "next cue after {next_cue:?}");
    fs::remove_dir_all(state_dir).unwrap();
}

#[test]
#[ignore = "needs macOS CoreMIDI and a pty: run scripts/fault-injection.sh"]
fn engine_runs_without_its_state_folder() {
    let device = EmulatedEnttec::plug_in().unwrap();
    let (port_name, scratch_dir) = scratch("state");
    let not_a_folder = scratch_dir.join("a-file");
    fs::write(&not_a_folder, "").unwrap();
    let mut daw = daw(&port_name);
    let _engine = Engine::start(&device, &port_name, &not_a_folder);
    wait_for(&device, None);
    send(&mut daw, &SELECT_SONG);
    send(&mut daw, &BLUE_SNAP);

    wait_for(&device, Some(BLUE_AT_FULL));
    fs::remove_dir_all(scratch_dir).unwrap();
}

#[test]
#[ignore = "needs macOS CoreMIDI and a pty: run scripts/fault-injection.sh"]
fn engine_follows_the_midi_port_when_it_drops_and_comes_back() {
    let device = EmulatedEnttec::plug_in().unwrap();
    let (port_name, state_dir) = scratch("midi");
    let mut first_port = daw(&port_name);
    let _engine = Engine::start(&device, &port_name, &state_dir);
    wait_for(&device, None);
    send(&mut first_port, &SELECT_SONG);
    send(&mut first_port, &BLUE_SNAP);
    wait_for(&device, Some(BLUE_AT_FULL));

    drop(first_port);
    thread::sleep(Duration::from_secs(1));
    let mut second_port = daw(&port_name);
    let restored = Instant::now();
    while device.frames().last().unwrap()[0..4] != BLUE_AT_HALF {
        assert!(
            restored.elapsed() < PATIENCE,
            "no MIDI after the port came back"
        );
        send(&mut second_port, &HALF_MASTER);
        thread::sleep(Duration::from_millis(50));
    }

    eprintln!("MIDI back after {:?}", restored.elapsed());
    fs::remove_dir_all(state_dir).unwrap();
}

#[test]
#[ignore = "needs macOS CoreMIDI and a pty: run scripts/fault-injection.sh"]
fn engine_resumes_output_when_the_serial_port_drops_and_comes_back() {
    let mut device = EmulatedEnttec::plug_in().unwrap();
    let (port_name, state_dir) = scratch("serial");
    let mut daw = daw(&port_name);
    let _engine = Engine::start(&device, &port_name, &state_dir);
    wait_for(&device, None);
    send(&mut daw, &SELECT_SONG);
    send(&mut daw, &BLUE_SNAP);
    wait_for(&device, Some(BLUE_AT_FULL));

    device.unplug();
    thread::sleep(Duration::from_millis(500));
    device.plug_back_in().unwrap();
    let back = wait_for(&device, Some(BLUE_AT_FULL));

    eprintln!("output back {back:?} after replug");
    assert!(back < RESTART_TARGET, "output back after {back:?}");
    fs::remove_dir_all(state_dir).unwrap();
}

use std::fs;
use std::net::UdpSocket;
use std::process::{Child, Command};
use std::time::Duration;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

/// Kills the engine when the test ends, pass or fail.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

struct ArtDmx {
    universe: u16,
    sequence: u8,
    channels: Vec<u8>,
}

fn decode(packet: &[u8]) -> ArtDmx {
    assert_eq!(&packet[..10], b"Art-Net\0\x00\x50", "ArtDmx header");
    let length = usize::from(u16::from_be_bytes([packet[16], packet[17]]));
    assert_eq!(packet.len(), 18 + length);
    ArtDmx {
        universe: u16::from_le_bytes([packet[14], packet[15]]),
        sequence: packet[12],
        channels: packet[18..].to_vec(),
    }
}

/// The single frame of the dump `fire` prints for the same look, as 512 channel values.
fn reference_channels() -> Vec<u8> {
    let dump = fs::read_to_string(format!(
        "{FIXTURES}/dumps/four-washes-all-wash-deep-blue-60.dump"
    ))
    .unwrap();
    let frame_line = dump.lines().find(|line| line.starts_with("0 ")).unwrap();
    let mut channels = vec![0; 512];
    for pair in frame_line.split_whitespace().skip(1) {
        let (channel, value) = pair.split_once('=').unwrap();
        channels[channel.parse::<usize>().unwrap() - 1] = value.parse().unwrap();
    }
    channels
}

#[test]
fn fire_mirrors_the_frame_over_art_net_on_the_chosen_universe() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("fire")
        .arg(format!("{FIXTURES}/shows/four-washes"))
        .args(["--set", "all-wash", "color=deep-blue", "dimmer=60%"])
        .arg("--artnet")
        .arg(receiver.local_addr().unwrap().to_string())
        .args(["--artnet-universe", "258"])
        .spawn()
        .unwrap();
    let _engine = Running(child);

    let mut buffer = [0; 1024];
    let mut next = || {
        let size = receiver.recv(&mut buffer).unwrap();
        decode(&buffer[..size])
    };
    let first = next();
    let second = next();

    assert_eq!(first.universe, 258, "Net 1, SubUni 2");
    assert_eq!(first.channels, reference_channels());
    assert_eq!(second.channels, first.channels, "the look is held");
    assert_eq!(second.sequence, first.sequence + 1);
}

/// Under launchd stderr is a log file, which a full disk stops taking; a closed pipe fails the
/// same way. Losing the log must not stop the light.
#[test]
fn output_carries_on_when_stderr_cannot_be_written() {
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (closed, stderr) = std::io::pipe().unwrap();
    drop(closed);
    let child = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("fire")
        .arg(format!("{FIXTURES}/shows/four-washes"))
        .args(["--set", "all-wash", "color=deep-blue", "dimmer=60%"])
        .arg("--artnet")
        .arg(receiver.local_addr().unwrap().to_string())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let mut engine = Running(child);

    let mut buffer = [0; 1024];
    let size = receiver.recv(&mut buffer).expect("an Art-Net frame");

    assert_eq!(decode(&buffer[..size]).channels, reference_channels());
    assert!(engine.0.try_wait().unwrap().is_none(), "the engine exited");
}

#[test]
fn a_universe_without_artnet_is_refused() {
    let output = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .arg("fire")
        .arg(format!("{FIXTURES}/shows/four-washes"))
        .args(["--set", "all-wash", "color=deep-blue"])
        .args(["--artnet-universe", "1"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--artnet"));
}

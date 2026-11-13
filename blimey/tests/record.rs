//! `blimey record` round trip on the real binary: a virtual CoreMIDI port plays a song in real
//! time, as Reaper would, and the recording replays to the same frames as the song. A UDP
//! socket stands in for EMU's Art-Net output, captured on the same timeline. macOS only,
//! and ignored by a plain `cargo test`: run
//! `cargo test -p blimey --test record -- --ignored --nocapture`.
#![cfg(target_os = "macos")]

use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use blimey_engine::{MidiEvent, MidiFile, MidiSource, Show, format_dump, parse_dump, replay};
use midir::MidiOutput;
use midir::os::unix::VirtualOutput;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");
const PATIENCE: Duration = Duration::from_secs(5);
/// From sending a message to the recorder stamping it, through CoreMIDI on the same machine.
const MAX_LATENCY: Duration = Duration::from_millis(2);
/// When EMU's stand-in sends its one Art-Net frame, into the song.
const EMU_FRAME_AT: Duration = Duration::from_millis(300);

/// Legacy notes on the legacy show (notes 37-52 drive DMX 6-21), with clock and transport,
/// each event mid-frame.
fn song() -> Vec<MidiEvent> {
    let event = |micros: u64, message: &[u8]| MidiEvent {
        at: Duration::from_micros(micros),
        message: message.to_vec(),
    };
    let mut events = vec![event(0, &[0xFA])];
    events.extend((0..40).map(|frame| event(12_500 + 25_000 * frame, &[0xF8])));
    events.extend([
        event(112_500, &[0x90, 37, 100]),
        event(512_500, &[0x90, 40, 64]),
        event(612_500, &[0x80, 37, 0]),
        event(912_500, &[0x80, 40, 0]),
        event(1_012_500, &[0xFC]),
    ]);
    events.sort_by_key(|event| event.at);
    events
}

#[test]
#[ignore = "needs macOS CoreMIDI: cargo test -p blimey --test record -- --ignored"]
fn a_recorded_song_replays_to_the_same_frames_as_the_song() {
    let port_name = format!("blimey-record-{}", std::process::id());
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{port_name}.mid"));
    let mut reaper = MidiOutput::new("blimey-record-test")
        .unwrap()
        .create_virtual(&port_name)
        .expect("create a virtual CoreMIDI port");
    let emu_dump = out.with_extension("dump");
    let mut recorder = start_recording(
        &port_name,
        &out,
        &[
            "--until-stop".as_ref(),
            "--artnet".as_ref(),
            emu_dump.as_os_str(),
        ],
    );

    // What was actually played: the sender's own timing jitters by a few milliseconds.
    let mut played = Vec::new();
    let mut emu_sent_at = None;
    let started = Instant::now();
    for event in song() {
        if let Some(wait) = event.at.checked_sub(started.elapsed()) {
            thread::sleep(wait);
        }
        if emu_sent_at.is_none() && event.at >= EMU_FRAME_AT {
            emu_sent_at = Some(started.elapsed());
            send_emu_frame();
        }
        played.push(MidiEvent {
            at: started.elapsed(),
            message: event.message.clone(),
        });
        reaper.send(&event.message).unwrap();
    }
    let deadline = Instant::now() + PATIENCE;
    while recorder.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "recording did not stop at MIDI Stop"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let recorded = events(MidiFile::open(&out).unwrap());
    let start = played[0].at;
    for (got, sent) in recorded.iter().zip(&played) {
        assert_eq!(got.message, sent.message);
        let sent_at = sent.at - start;
        assert!(
            got.at.abs_diff(sent_at) < MAX_LATENCY,
            "{:02X?} sent at {sent_at:?}, recorded at {:?}",
            got.message,
            got.at
        );
    }
    assert_eq!(recorded.len(), played.len());
    let emu_frames = parse_dump(&fs::read_to_string(&emu_dump).unwrap()).unwrap();
    let [first, emu_frame] = emu_frames.as_slice() else {
        panic!(
            "expected the two Art-Net packets sent, got {}",
            emu_frames.len()
        );
    };
    assert_eq!(
        first.values[..3],
        [0, 0, 77],
        "universe 0 is the Enttec one"
    );
    assert!(first.artnet.is_empty());
    assert_eq!(
        emu_frame.values[..3],
        [0, 0, 77],
        "the latest of every universe"
    );
    assert_eq!(emu_frame.artnet[&2][..2], [9, 0], "universe 2, DMX 1");
    let emu_sent_at = emu_sent_at.unwrap() - start;
    assert!(
        emu_frame.at.abs_diff(emu_sent_at) <= MAX_LATENCY,
        "Art-Net sent at {emu_sent_at:?}, recorded at {:?}",
        emu_frame.at
    );
    let show = Show::load(&Path::new(FIXTURES).join("shows/legacy")).unwrap();
    let played = played
        .into_iter()
        .map(|event| MidiEvent {
            at: event.at - start,
            ..event
        })
        .collect::<Vec<_>>();
    assert_eq!(
        format_dump(&replay(&show, MidiFile::open(&out).unwrap())),
        format_dump(&replay(
            &show,
            MidiFile::parse(&MidiFile::encode(&played).unwrap()).unwrap()
        ))
    );
}

#[test]
#[ignore = "needs macOS CoreMIDI: cargo test -p blimey --test record -- --ignored"]
fn a_take_is_on_disk_every_few_seconds_and_complete_after_sigterm() {
    let port_name = format!("blimey-record-term-{}", std::process::id());
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{port_name}.mid"));
    let _ = fs::remove_file(&out);
    let mut reaper = MidiOutput::new("blimey-record-test")
        .unwrap()
        .create_virtual(&port_name)
        .expect("create a virtual CoreMIDI port");
    let mut recorder = start_recording(&port_name, &out, &[]);
    let note = |velocity| [0x90, 37, velocity];

    reaper.send(&note(10)).unwrap();
    thread::sleep(Duration::from_secs(6));
    let saved = events(MidiFile::open(&out).expect("saved within 5 s, as a kill -9 would find it"));
    reaper.send(&note(20)).unwrap();
    thread::sleep(Duration::from_millis(100));
    // SAFETY: kill only sends a signal to the recorder's process id.
    assert_eq!(
        unsafe { libc::kill(recorder.id() as i32, libc::SIGTERM) },
        0
    );
    let status = recorder.wait().unwrap();

    assert_eq!(saved.len(), 1);
    assert!(status.success(), "{status}");
    let messages: Vec<Vec<u8>> = events(MidiFile::open(&out).unwrap())
        .into_iter()
        .map(|event| event.message)
        .collect();
    assert_eq!(messages, [note(10), note(20)]);
}

/// Starts `blimey record <out> --midi-port <port_name> <args>` and waits until it records.
fn start_recording(port_name: &str, out: &Path, args: &[&OsStr]) -> Child {
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_blimey"))
        .args(["record", "--midi-port", port_name])
        .arg(out)
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
        .expect("start blimey record");
    let (listening, lines) = mpsc::channel();
    let stderr = BufReader::new(recorder.stderr.take().unwrap());
    thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            eprintln!("{line}");
            let _ = listening.send(line);
        }
    });
    while !lines
        .recv_timeout(PATIENCE)
        .expect("blimey record never connected")
        .starts_with("record: recording")
    {}
    recorder
}

/// EMU's side, written from the specification rather than with the engine's encoder: universe
/// 0 with DMX 3 at 77, then a pixel grid's universe 2 with DMX 1 at 9.
fn send_emu_frame() {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    for (universe, channels) in [(0, [0, 0, 77, 0]), (2, [9, 0, 0, 0])] {
        let mut packet = b"Art-Net\0".to_vec();
        packet.extend([0x00, 0x50, 0, 14, 1, 0, universe, 0, 0, 4]);
        packet.extend(channels);
        socket
            .send_to(&packet, (Ipv4Addr::LOCALHOST, 6454))
            .unwrap();
    }
}

fn events(mut midi: MidiFile) -> Vec<MidiEvent> {
    std::iter::from_fn(|| midi.poll(Duration::MAX)).collect()
}

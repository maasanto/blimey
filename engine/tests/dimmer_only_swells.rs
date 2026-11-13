//! `chorus-swell` (`{ all-wash: { dimmer: 100% } }` over 4 bars) on the real show, whose Wild
//! Washes run 16ch with no dimmer channel: replayed through the engine, it brings the scene
//! below to full through their virtual dimmers, as a dimmer channel would.
//!
//! The song is `tell-me` at its 92 BPM, no MIDI clock: 4 bars are 16 beats, 10.43 s.

use std::path::Path;
use std::time::Duration;

use blimey_engine::look::{Look, LookSettings, VirtualDimmers, render};
use blimey_engine::{FRAME_PERIOD, Frame, MidiEvent, MidiFile, RecordedFrame, Show, replay};

const FOSSILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../shows/fossile");
const PROGRAM_CHANGE: u8 = 0xC1;
const NOTE_ON: u8 = 0x91;
const VERSE_BLUE: u8 = 36;
const CHORUS_AMBER: u8 = 37;
const BLACKOUT: u8 = 38;
const CHORUS_SWELL: u8 = 39;
const SWELL_SECONDS: f64 = 16.0 * 60.0 / 92.0;

fn show() -> Show {
    Show::load(Path::new(FOSSILE)).unwrap()
}

/// Plays tell-me's cues at full velocity, `(seconds, note)`, and renders `until` seconds.
fn play(show: &Show, notes: &[(f64, u8)], until: f64) -> Vec<RecordedFrame> {
    let event = |seconds: f64, message: Vec<u8>| MidiEvent {
        at: Duration::from_secs_f64(seconds),
        message,
    };
    let mut events = vec![event(0.0, vec![PROGRAM_CHANGE, 3])];
    events.extend(
        notes
            .iter()
            .map(|&(at, note)| event(at, vec![NOTE_ON, note, 127])),
    );
    // A live parameter, which changes no light, so the replay runs to `until`.
    events.push(event(until, vec![0xB1, 20, 0]));
    replay(
        show,
        MidiFile::parse(&MidiFile::encode(&events).unwrap()).unwrap(),
    )
}

fn at(frames: &[RecordedFrame], seconds: f64) -> Frame {
    let index = (seconds / FRAME_PERIOD.as_secs_f64()).round() as usize;
    frames[index].values
}

/// The frame `blimey fire --set all-wash <settings>` draws.
fn all_wash(show: &Show, settings: &[&str]) -> Frame {
    let pairs: Vec<String> = settings.iter().map(|pair| pair.to_string()).collect();
    let mut look = Look::default();
    look.set(show, "all-wash", &LookSettings::from_pairs(&pairs).unwrap())
        .unwrap();
    let mut frame = [0; 512];
    render(
        &show.rig,
        &look,
        &mut frame,
        &mut VirtualDimmers::of(&[0; 512]),
    );
    frame
}

#[test]
fn over_verse_blue_at_60_percent_it_reaches_deep_blue_at_full() {
    let show = show();
    let frames = play(&show, &[(0.0, VERSE_BLUE), (3.0, CHORUS_SWELL)], 15.0);

    let at_60 = all_wash(&show, &["color=deep-blue", "dimmer=60%"]);
    let at_full = all_wash(&show, &["color=deep-blue"]);
    assert_eq!(at(&frames, 3.0), at_60);
    let brightness = |frame: Frame| frame.iter().map(|&value| u32::from(value)).sum::<u32>();
    let halfway = brightness(at(&frames, 3.0 + SWELL_SECONDS / 2.0));
    assert!(
        brightness(at_60) < halfway && halfway < brightness(at_full),
        "on its way up"
    );
    assert_eq!(at(&frames, 14.0), at_full);
}

#[test]
fn over_chorus_amber_at_full_it_holds() {
    let show = show();
    let frames = play(&show, &[(0.0, CHORUS_AMBER), (1.0, CHORUS_SWELL)], 12.0);

    let amber = at(&frames, 1.0);
    for seconds in [3.0, 7.0, 11.5] {
        assert_eq!(at(&frames, seconds), amber, "at {seconds} s");
    }
}

#[test]
fn after_a_blackout_it_brings_the_scene_back_up() {
    let show = show();
    let notes = [(0.0, VERSE_BLUE), (3.0, BLACKOUT), (5.0, CHORUS_SWELL)];
    let frames = play(&show, &notes, 16.0);

    assert_eq!(
        at(&frames, 5.0),
        all_wash(&show, &["color=deep-blue", "dimmer=0%"])
    );
    assert_eq!(at(&frames, 16.0), all_wash(&show, &["color=deep-blue"]));
}

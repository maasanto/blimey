//! `chorus-swell` (`{ all-wash: { dimmer: 100% } }` over 4 bars) on the real show, whose Wild
//! Washes run 16ch with no dimmer channel: replayed through the engine, it brings the scene
//! below to full through their virtual dimmers, as a dimmer channel would.
//!
//! The song is `demo-song` at its 92 BPM, no MIDI clock: 4 bars are 16 beats, 10.43 s.

use std::path::Path;
use std::time::Duration;

use blimey_engine::look::{Look, LookSettings, VirtualDimmers, render};
use blimey_engine::{
    DMX_CHANNELS, FRAME_PERIOD, Frame, MidiEvent, MidiFile, RecordedFrame, Show, replay,
};

const DEMO_SHOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures/shows/demo");
const PROGRAM_CHANGE: u8 = 0xC1;
const NOTE_ON: u8 = 0x91;
const NOTE_OFF: u8 = 0x81;
const CONTROL_CHANGE: u8 = 0xB1;
/// CC 20, the first live parameter: it changes no light.
const LIVE_PARAMETER: u8 = 20;
const VERSE_BLUE: u8 = 36;
const CHORUS_AMBER: u8 = 37;
const BLACKOUT: u8 = 38;
const CHORUS_SWELL: u8 = 39;
const SWELL_SECONDS: f64 = 16.0 * 60.0 / 92.0;

fn show() -> Show {
    Show::load(Path::new(DEMO_SHOW)).unwrap()
}

fn on(seconds: f64, note: u8) -> (f64, [u8; 3]) {
    (seconds, [NOTE_ON, note, 127])
}

fn off(seconds: f64, note: u8) -> (f64, [u8; 3]) {
    (seconds, [NOTE_OFF, note, 0])
}

/// Selects demo-song, plays `messages` at their seconds, and renders `until` seconds.
fn play(show: &Show, messages: &[(f64, [u8; 3])], until: f64) -> Vec<RecordedFrame> {
    let event = |seconds: f64, message: Vec<u8>| MidiEvent {
        at: Duration::from_secs_f64(seconds),
        message,
    };
    let mut events = vec![event(0.0, vec![PROGRAM_CHANGE, 3])];
    events.extend(
        messages
            .iter()
            .map(|(seconds, message)| event(*seconds, message.to_vec())),
    );
    // So the replay runs to `until`.
    events.push(event(until, vec![CONTROL_CHANGE, LIVE_PARAMETER, 0]));
    replay(
        show,
        MidiFile::parse(&MidiFile::encode(&events).unwrap()).unwrap(),
    )
}

fn at(frames: &[RecordedFrame], seconds: f64) -> Frame {
    let index = (seconds / FRAME_PERIOD.as_secs_f64()).round() as usize;
    frames[index].values
}

/// The frame `blimey fire` draws from darkness for these `(target, settings)`, in order.
fn look(show: &Show, entries: &[(&str, &[&str])]) -> Frame {
    let mut look = Look::default();
    for (target, settings) in entries {
        let pairs: Vec<String> = settings.iter().map(|pair| pair.to_string()).collect();
        look.set(show, target, &LookSettings::from_pairs(&pairs).unwrap())
            .unwrap();
    }
    let mut frame = [0; DMX_CHANNELS];
    render(
        &show.rig,
        &look,
        &mut frame,
        &mut VirtualDimmers::of(&[0; DMX_CHANNELS]),
    );
    frame
}

fn all_wash(show: &Show, settings: &[&str]) -> Frame {
    look(show, &[("all-wash", settings)])
}

#[test]
fn over_verse_blue_at_60_percent_it_reaches_deep_blue_at_full() {
    let show = show();
    let frames = play(&show, &[on(0.0, VERSE_BLUE), on(3.0, CHORUS_SWELL)], 15.0);

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
    let frames = play(&show, &[on(0.0, CHORUS_AMBER), on(1.0, CHORUS_SWELL)], 12.0);

    let amber = at(&frames, 1.0);
    for seconds in [3.0, 7.0, 11.5] {
        assert_eq!(at(&frames, seconds), amber, "at {seconds} s");
    }
}

#[test]
fn after_a_blackout_it_brings_the_scene_back_up() {
    let show = show();
    let messages = [
        on(0.0, VERSE_BLUE),
        on(3.0, BLACKOUT),
        on(5.0, CHORUS_SWELL),
    ];
    let frames = play(&show, &messages, 16.0);

    assert_eq!(
        at(&frames, 5.0),
        all_wash(&show, &["color=deep-blue", "dimmer=0%"])
    );
    assert_eq!(at(&frames, 16.0), all_wash(&show, &["color=deep-blue"]));
}

/// Verse-blue blacked out, then chorus-amber, then the swell at full above both. Releasing
/// chorus-amber fades it out over 500 ms under the swell, back to the blacked-out blue.
#[test]
fn mid_fade_it_shows_the_colour_the_fade_shows_not_one_hidden_under_a_blackout() {
    let show = show();
    let messages = [
        on(0.0, VERSE_BLUE),
        on(0.5, BLACKOUT),
        on(2.0, CHORUS_AMBER),
        on(2.1, CHORUS_SWELL),
        off(13.0, CHORUS_AMBER),
    ];
    let frames = play(&show, &messages, 14.0);
    let chorus_amber = look(
        &show,
        &[
            ("all-wash", &["color=amber"]),
            ("outer-wash", &["color=magenta"]),
        ],
    );

    assert_eq!(at(&frames, 12.9), chorus_amber);
    let halfway = at(&frames, 13.25);
    for (channel, (&shown, &amber)) in halfway.iter().zip(&chorus_amber).enumerate() {
        assert!(
            shown.abs_diff(amber) <= 1,
            "halfway through the release, DMX {}: {shown}, the amber is {amber}",
            channel + 1
        );
    }
    assert_eq!(at(&frames, 13.75), all_wash(&show, &["color=deep-blue"]));
}

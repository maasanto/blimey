//! Replays of channel-2 cue MIDI against `tests/fixtures/shows/cues`, compared to reference
//! dumps, plus the timings and levels each one is about.
//!
//! The show: `dimmer-par` on DMX 1-4 (dimmer, red, green, blue), `rgb-par` on DMX 5-7 (red,
//! green, blue, no dimmer); legacy notes 60-66 drive DMX 1-7 and note 70 drives DMX 20. Song
//! `first` is program 1, `second` program 2. The MIDI files are listed with each test, times in
//! milliseconds; regenerate a reference with
//! `blimey replay tests/fixtures/shows/cues --midi <file>.mid --out <file>.dump`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use blimey_engine::{MidiFile, RecordedFrame, Show, first_difference, parse_dump, replay};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn replay_against_reference(name: &str) -> Vec<RecordedFrame> {
    let show = Show::load(Path::new(&format!("{FIXTURES}/shows/cues"))).unwrap();
    let midi = MidiFile::open(Path::new(&format!("{FIXTURES}/midi/{name}.mid"))).unwrap();
    let frames = replay(&show, midi);
    let reference = fs::read_to_string(format!("{FIXTURES}/dumps/{name}.dump")).unwrap();
    if let Some(difference) = first_difference(&frames, &parse_dump(&reference).unwrap()) {
        panic!("{name}: {difference}");
    }
    frames
}

/// DMX channels `first..=last` (1-based) of the frame at `millis`.
fn channels(frames: &[RecordedFrame], millis: u64, first: usize, last: usize) -> Vec<u8> {
    let frame = frames
        .iter()
        .find(|frame| frame.at == Duration::from_millis(millis))
        .unwrap();
    frame.values[first - 1..last].to_vec()
}

/// 0: program 1. 110: note 36 (red-fade: 1 s fade-in, 500 ms release). 1610: note-off 36.
#[test]
fn a_fade_reaches_its_level_within_one_frame_of_its_programmed_end() {
    let frames = replay_against_reference("cue-fade");
    let red = |frame: &RecordedFrame| frame.values[1];
    let first_frame_where =
        |test: &dyn Fn(&RecordedFrame) -> bool| frames.iter().find(|frame| test(frame)).unwrap().at;

    let full = first_frame_where(&|frame| red(frame) == 255);
    let lit = first_frame_where(&|frame| red(frame) > 0);
    let dark_again = first_frame_where(&|frame| frame.at > full && red(frame) == 0);

    let within_one_frame = |at: Duration, programmed: u64| {
        let programmed = Duration::from_millis(programmed);
        at >= programmed && at - programmed < Duration::from_millis(25)
    };
    assert!(within_one_frame(lit, 110), "lit at {lit:?}");
    assert!(within_one_frame(full, 1110), "full at {full:?}");
    assert!(within_one_frame(dark_again, 2110), "dark at {dark_again:?}");
    assert_eq!(
        channels(&frames, 600, 1, 4),
        [125, 125, 0, 0],
        "halfway through the fade: 490 ms of 1 s"
    );
}

/// 0: program 1; legacy notes 60 (vel 100), 61 (50), 64 (100), 65 (127).
/// 500: note 37 (blue-snap on dimmer-par) at velocity 64. 1000: legacy note 60 at velocity 20.
/// 1500: note-off 37.
#[test]
fn a_cue_overrides_legacy_on_its_fixtures_only_until_released() {
    let frames = replay_against_reference("cue-over-legacy");

    assert_eq!(channels(&frames, 0, 1, 7), [200, 100, 0, 0, 200, 255, 0]);
    assert_eq!(
        channels(&frames, 500, 1, 7),
        [129, 0, 0, 255, 200, 255, 0],
        "dimmer at velocity 64 of 127; rgb-par keeps legacy"
    );
    assert_eq!(
        channels(&frames, 1250, 1, 4),
        [129, 0, 0, 255],
        "legacy changes under the cue don't show"
    );
    assert_eq!(channels(&frames, 1500, 1, 7), [40, 100, 0, 0, 200, 255, 0]);
}

/// 0: program 1. 100: note 36 (red-fade). 600: program 2. 800: note 36 (second's green-fade,
/// 200 ms fade-in). 1200: note-off 36. 1300: program 1. 1400: note 37 (blue-snap).
#[test]
fn a_song_switch_mid_stream_starts_the_new_song_from_nothing() {
    let frames = replay_against_reference("song-switch");

    assert_eq!(channels(&frames, 575, 1, 2), [121, 121], "red fading in");
    assert_eq!(channels(&frames, 600, 1, 7), [0; 7], "program 2 drops it");
    assert_eq!(
        channels(&frames, 1000, 5, 7),
        [0, 255, 0],
        "same note, new song's cue"
    );
    assert_eq!(channels(&frames, 1200, 1, 7), [0; 7]);
    assert_eq!(channels(&frames, 1400, 1, 4), [255, 0, 0, 255]);
}

/// 0: program 1; legacy notes 60 and 70 (vel 100). 200: note 38 (both-green-half).
/// 500: panic (channel 16 note 127). 800: legacy note 70 at velocity 50. 1000: note 37.
#[test]
fn panic_clears_legacy_and_cues_and_shows_the_safe_look() {
    let frames = replay_against_reference("panic");
    let safe_look = [51, 255, 255, 255, 51, 51, 51];

    assert_eq!(channels(&frames, 475, 1, 7), [128, 0, 255, 0, 0, 128, 0]);
    assert_eq!(channels(&frames, 475, 20, 20), [200]);
    assert_eq!(channels(&frames, 500, 1, 7), safe_look);
    assert_eq!(channels(&frames, 500, 20, 20), [0], "legacy cleared");
    assert_eq!(channels(&frames, 800, 20, 20), [100], "legacy plays again");
    assert_eq!(
        channels(&frames, 1000, 1, 7),
        [255, 0, 0, 255, 51, 51, 51],
        "new cues draw over the safe look"
    );
}

/// The time of the first frame after `after` whose dimmer-par dimmer (DMX 1) passes `test`.
fn first_dimmer_frame(frames: &[RecordedFrame], after: Duration, test: impl Fn(u8) -> bool) -> f64 {
    frames
        .iter()
        .find(|frame| frame.at > after && test(frame.values[0]))
        .expect("the dump reaches the level")
        .at
        .as_secs_f64()
}

fn assert_within_one_frame(actual: f64, programmed: f64, what: &str) {
    assert!(
        (actual - programmed).abs() <= 0.025,
        "{what} at {actual:.3} s, programmed for {programmed:.3} s"
    );
}

/// Seconds per beat in both swell files: their notes sit on a 92 BPM grid.
const BEAT_AT_92: f64 = 60.0 / 92.0;

/// Both at 92 BPM, from 0: program 1. Beat 4 (2.609 s): note 39 (four-bar-swell: dimmer-par
/// to white over 4 bars, 1-beat release). Beat 21 (13.696 s): note-off 39. Ends at beat 23.
/// `swell-clock` also carries Start and MIDI clock as SMF escapes; the song's bpm is 120.
#[test]
fn a_swell_on_the_midi_clock_ends_on_the_bar_line() {
    let frames = replay_against_reference("swell-clock");

    let full = first_dimmer_frame(&frames, Duration::ZERO, |dimmer| dimmer == 255);
    let dark = first_dimmer_frame(&frames, Duration::from_secs_f64(full), |dimmer| dimmer == 0);

    assert_within_one_frame(full, 20.0 * BEAT_AT_92, "full");
    assert_within_one_frame(dark, 22.0 * BEAT_AT_92, "dark after a 1-beat release");
    let halfway = channels(&frames, 7825, 1, 1)[0];
    assert!(
        (126..=129).contains(&halfway),
        "beat 12 is halfway: {halfway}"
    );
}

#[test]
fn without_a_clock_a_swell_runs_at_the_song_bpm() {
    let frames = replay_against_reference("swell-no-clock");

    let full = first_dimmer_frame(&frames, Duration::ZERO, |dimmer| dimmer == 255);
    let dark = first_dimmer_frame(&frames, Duration::from_secs_f64(full), |dimmer| dimmer == 0);

    let half_second_beats = 0.5;
    assert_within_one_frame(full, 4.0 * BEAT_AT_92 + 16.0 * half_second_beats, "full");
    assert_within_one_frame(dark, 21.0 * BEAT_AT_92 + half_second_beats, "dark");
}

/// At the song's 120 BPM, 0: program 1; note 38 (both-green-half). 500: note 40 (both-brighten:
/// `{ both: { dimmer: 100% } }` over 1 bar, 500 ms release). 3000: note-off 40. 4000: note-off 38.
#[test]
fn a_dimmer_only_swell_brightens_the_scene_below_and_keeps_its_colour() {
    let frames = replay_against_reference("dimmer-swell");
    let green_half = [128, 0, 255, 0, 0, 128, 0];

    assert_eq!(channels(&frames, 500, 1, 7), green_half);
    assert_eq!(
        channels(&frames, 1500, 1, 7),
        [192, 0, 255, 0, 0, 192, 0],
        "halfway, alike: dimmer-par's dimmer channel and rgb-par's virtual dimmer"
    );
    assert_eq!(
        channels(&frames, 2500, 1, 7),
        [255, 0, 255, 0, 0, 255, 0],
        "full green on both"
    );
    assert_eq!(
        channels(&frames, 3500, 1, 7),
        green_half,
        "released back to the scene"
    );
}

/// 0: program 1; note 38 (both-green-half); legacy note 70 (vel 100).
/// 300: CC 7 = 64. 600: CC 7 = 0. 900: CC 7 = 127. 1000: CC 20 = 100 (a live parameter).
#[test]
fn the_master_dimmer_scales_intensity_channels_last() {
    let frames = replay_against_reference("master");

    assert_eq!(channels(&frames, 0, 1, 7), [128, 0, 255, 0, 0, 128, 0]);
    assert_eq!(channels(&frames, 0, 20, 20), [200]);
    assert_eq!(
        channels(&frames, 300, 1, 7),
        [65, 0, 255, 0, 0, 65, 0],
        "dimmer channel scaled, colour behind it kept; rgb-par's colour scaled"
    );
    assert_eq!(
        channels(&frames, 300, 20, 20),
        [101],
        "unpatched legacy scaled"
    );
    assert_eq!(channels(&frames, 600, 1, 7), [0, 0, 255, 0, 0, 0, 0]);
    assert_eq!(channels(&frames, 900, 1, 7), [128, 0, 255, 0, 0, 128, 0]);
    assert_eq!(channels(&frames, 1000, 1, 7), [128, 0, 255, 0, 0, 128, 0]);
}

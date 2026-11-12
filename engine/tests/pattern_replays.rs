//! The plan's beat 1 / 2 / 3.5 pattern replayed on a MIDI clock at two tempos, against
//! `tests/fixtures/shows/cues` (see `cue_replays.rs` for the rig).
//!
//! Both files, generated with MIDI clock and Start as SMF escapes, from 0: program 1 (whose bpm
//! is 120, so a pattern ignoring the clock fails), Start, note 38 (both fixtures green at 50%,
//! DMX 1 and 6 at 128). Note 42 (verse-pulse: hits on beats 1, 2 and 3.5 scaling both
//! fixtures' intensity, 1-beat release) 10 ms before the bar 2 line at 92 BPM, 10 ms after it
//! at 140 BPM. Note-off 42 at beat 16.25, a quarter beat into the first hit. Ends at beat 18.5.
//! Regenerate a reference with
//! `blimey replay tests/fixtures/shows/cues --midi <file>.mid --out <file>.dump`.

use std::fs;
use std::path::Path;

use blimey_engine::{
    FRAME_PERIOD, MidiFile, RecordedFrame, Show, first_difference, parse_dump, replay,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");
const BELOW: f64 = 128.0;
const BAR_2_LINE: f64 = 4.0;
const NOTE_OFF: f64 = 16.25;
const RELEASE_BEATS: f64 = 1.0;
/// Rounding, plus the clock follower settling within a hair of the clock.
const TOLERANCE: f64 = 2.0;

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

/// verse-pulse's level, written out independently of the engine, `position` beats into the bar.
fn verse_pulse(position: f64) -> f64 {
    let hit = |offset: f64, level: f64, shape: fn(f64) -> f64| {
        let progress = (position - offset) / 0.5;
        if (0.0..1.0).contains(&progress) {
            level * shape(progress)
        } else {
            0.0
        }
    };
    let ramp_up = |progress| progress;
    let sine = |progress: f64| (1.0 - (std::f64::consts::TAU * progress).cos()) / 2.0;
    hit(0.0, 1.0, ramp_up)
        .max(hit(1.0, 0.8, sine))
        .max(hit(2.5, 1.0, ramp_up))
}

fn assert_pattern_plays_on_the_clock(name: &str, bpm: f64) {
    let frames = replay_against_reference(name);
    let beat = |frame: &RecordedFrame| frame.at.as_secs_f64() * bpm / 60.0;
    let frame_beats = FRAME_PERIOD.as_secs_f64() * bpm / 60.0;
    // Frames this close to a jump may fall either side of it: the loop's first start and the
    // ends of the two ramps. Later bar lines are no jump, so the loop's seams are checked.
    let near_a_jump = |beat: f64| {
        let position = (beat - BAR_2_LINE).rem_euclid(4.0);
        (beat - BAR_2_LINE).abs() < frame_beats
            || [0.5, 3.0]
                .iter()
                .any(|jump| (position - jump).abs() < frame_beats)
    };

    for frame in &frames {
        let (beat, dimmer) = (beat(frame), f64::from(frame.values[0]));
        assert_eq!(
            frame.values[0], frame.values[5],
            "{name}: the dimmer and the colour-only fixture pulse alike at beat {beat:.3}"
        );
        if beat < BAR_2_LINE - frame_beats {
            assert_eq!(
                dimmer, BELOW,
                "{name}: untouched before the bar line at beat {beat:.3}"
            );
        } else if beat < NOTE_OFF && !near_a_jump(beat) {
            let expected = BELOW * verse_pulse((beat - BAR_2_LINE).rem_euclid(4.0));
            assert!(
                (dimmer - expected).abs() <= TOLERANCE,
                "{name}: {dimmer} at beat {beat:.3}, expected {expected:.1}"
            );
        }
    }

    let released: Vec<(f64, u8)> = frames
        .iter()
        .map(|frame| (beat(frame), frame.values[0]))
        .filter(|(beat, _)| *beat >= NOTE_OFF)
        .collect();
    let (_, at_note_off) = released[0];
    assert!(
        (f64::from(at_note_off) - BELOW / 2.0).abs() <= 4.0,
        "{name}: {at_note_off} at note-off, halfway up the first ramp"
    );
    assert!(
        released.windows(2).all(|pair| pair[0].1 <= pair[1].1),
        "{name}: the release only rises back to the look below: {released:?}"
    );
    let (back_at, _) = released
        .iter()
        .find(|(_, dimmer)| f64::from(*dimmer) == BELOW)
        .unwrap();
    assert!(
        (back_at - (NOTE_OFF + RELEASE_BEATS)).abs() <= frame_beats,
        "{name}: back to the look below at beat {back_at:.3}, after a 1-beat release"
    );
}

#[test]
fn the_beat_1_2_3_5_pattern_plays_on_the_clock_at_92_bpm() {
    assert_pattern_plays_on_the_clock("pattern-92", 92.0);
}

#[test]
fn the_beat_1_2_3_5_pattern_plays_on_the_clock_at_140_bpm() {
    assert_pattern_plays_on_the_clock("pattern-140", 140.0);
}

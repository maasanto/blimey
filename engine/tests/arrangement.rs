//! The arranged song of `tests/fixtures/shows/arranged`, exported for the DAW and replayed.
//!
//! The rig is the cues show's (see `cue_replays.rs`). Song `arranged`, program 1, 120 BPM, plays
//! [verse, waltz, verse, waltz]: verse is 2 bars of 4/4 (8 beats), waltz 2 bars of 3/4 (6 beats),
//! so the sections start on beats 0, 8, 14 and 22. Verse holds scene 36 (both red) throughout and
//! scene 37 (rgb-par blue) at 50% from bar 2 beat 3; waltz holds 36 for its first bar and the
//! pattern 42 (a 1-beat ramp down on both dimmers, from each bar line of its 1-bar loop)
//! throughout. Regenerate the reference with `blimey export <copy of the show>` then
//! `blimey replay tests/fixtures/shows/arranged --midi <copy>/export/arranged.mid --out
//! tests/fixtures/dumps/arranged.dump`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use blimey_engine::arrangement::{self, ArrangementEvent, EventKind};
use blimey_engine::{MidiFile, Show, first_difference, parse_dump, replay};
use midly::{MetaMessage, Smf, TrackEventKind};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");

fn show() -> Show {
    Show::load(Path::new(&format!("{FIXTURES}/shows/arranged"))).unwrap()
}

#[test]
fn the_event_list_places_each_block_across_the_3_4_section_and_holds_a_scene_as_one_note() {
    let show = show();
    let events = arrangement::events(&show.songs[0]);

    let event = |beats: f64, kind| ArrangementEvent {
        beats,
        seconds: beats / 2.0,
        kind,
    };
    let signature = |numerator, denominator| EventKind::TimeSignature {
        numerator,
        denominator,
    };
    let marker = |section: &str| EventKind::Marker {
        section: section.into(),
    };
    let on = |scene, velocity| EventKind::NoteOn { scene, velocity };
    let off = |scene| EventKind::NoteOff { scene };
    assert_eq!(
        events,
        [
            event(0.0, signature(4, 4)),
            event(0.0, marker("verse")),
            event(0.0, EventKind::ProgramChange { program: 1 }),
            event(0.0, on(36, 127)),
            event(6.0, on(37, 64)),
            event(8.0, signature(3, 4)),
            event(8.0, marker("waltz")),
            event(8.0, off(37)),
            event(8.0, on(42, 127)),
            event(11.0, off(36)),
            event(14.0, signature(4, 4)),
            event(14.0, marker("verse")),
            event(14.0, off(42)),
            event(14.0, on(36, 127)),
            event(20.0, on(37, 64)),
            event(22.0, signature(3, 4)),
            event(22.0, marker("waltz")),
            event(22.0, off(37)),
            event(22.0, on(42, 127)),
            event(25.0, off(36)),
            event(28.0, off(42)),
        ],
        "scene 36 runs from the verse into the waltz as one note each time"
    );
}

#[test]
fn the_exported_file_replays_to_the_reference_and_a_pattern_starts_on_the_3_4_bar_line() {
    let show = show();
    let midi = MidiFile::parse(&arrangement::midi_file(&show.songs[0]).unwrap()).unwrap();

    let frames = replay(&show, midi);

    let reference = fs::read_to_string(format!("{FIXTURES}/dumps/arranged.dump")).unwrap();
    if let Some(difference) = first_difference(&frames, &parse_dump(&reference).unwrap()) {
        panic!("arranged: {difference}");
    }
    let dimmer_at = |millis| {
        frames
            .iter()
            .find(|frame| frame.at == Duration::from_millis(millis))
            .unwrap()
            .values[0]
    };
    // The second waltz starts on beat 22, 11 s: mid-bar in 4/4, where the loop would wait for
    // beat 24, but its first bar line in 3/4.
    assert_eq!(dimmer_at(10_975), 255, "the red scene before the pattern");
    assert_eq!(
        dimmer_at(11_250),
        128,
        "halfway down the ramp, half a beat in"
    );
}

#[test]
fn the_exported_file_parses_back_with_its_tempo_signatures_and_markers() {
    let show = show();
    let bytes = arrangement::midi_file(&show.songs[0]).unwrap();
    let smf = Smf::parse(&bytes).unwrap();

    let mut tick = 0;
    let mut metas = Vec::new();
    for event in &smf.tracks[0] {
        tick += event.delta.as_int();
        match event.kind {
            TrackEventKind::Meta(MetaMessage::Tempo(tempo)) => {
                metas.push((tick, format!("tempo {}", tempo.as_int())));
            }
            TrackEventKind::Meta(MetaMessage::TimeSignature(numerator, power, ..)) => {
                metas.push((tick, format!("{numerator}/{}", 1 << power)));
            }
            TrackEventKind::Meta(MetaMessage::Marker(text)) => {
                metas.push((tick, String::from_utf8_lossy(text).into_owned()));
            }
            _ => {}
        }
    }

    let beat = 960;
    let expected: Vec<(u32, String)> = [
        (0, "tempo 500000"),
        (0, "4/4"),
        (0, "verse"),
        (8 * beat, "3/4"),
        (8 * beat, "waltz"),
        (14 * beat, "4/4"),
        (14 * beat, "verse"),
        (22 * beat, "3/4"),
        (22 * beat, "waltz"),
    ]
    .into_iter()
    .map(|(tick, text)| (tick, text.to_owned()))
    .collect();
    assert_eq!(metas, expected);
}

//! Pixel cues (plan §7, §15): every shape × effect on four rigs, compared to reference dumps in
//! `tests/fixtures/dumps/pixel/<rig>/<shape>-<effect>.dump`, plus the mapping and determinism
//! checks behind them.
//!
//! Each case selects a 150 BPM song and fires one pixel cue at 0 ms: red, width 0.25, speed
//! 0.25 beats (100 ms, 4 frames), seed 7. A reference holds those 4 frames: one cycle.
//! Regenerate the references, after checking the change is wanted, with
//! `BLIMEY_BLESS=1 cargo test -p blimey-engine --test pixel_dumps`.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::Duration;

use blimey_engine::look::Look;
use blimey_engine::song::Song;
use blimey_engine::{
    FakeClock, FakeDmx, MidiEvent, MidiSource, RecordedFrame, Show, first_difference, format_dump,
    parse_dump, run,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fixtures");
const SHAPES: [&str; 6] = ["bar", "sweep", "circle", "split", "gradient", "one-by-one"];
const EFFECTS: [&str; 5] = ["static", "move", "pulse", "chase", "sparkle"];
/// The 4 washes are 4 RGBW pixels each; the grids are described in their `rig.yaml`.
const RIGS: [&str; 4] = [
    "four-washes",
    "pixel-grid-8x8",
    "pixel-column-1x32",
    "pixel-grid-16x16",
];
const CUE_NOTE: u8 = 60;

/// Events in time order, each delivered once due.
struct Events(VecDeque<MidiEvent>);

impl MidiSource for Events {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        self.0
            .front()
            .is_some_and(|event| event.at <= until)
            .then(|| self.0.pop_front().unwrap())
    }
}

/// Renders `frames` frames of `rig` with `cue` (the YAML body of a pixel cue, indented 4)
/// fired at `fired_at`.
fn render(rig: &str, cue: &str, fired_at: Duration, frames: u32) -> Vec<RecordedFrame> {
    let cues = format!("  {CUE_NOTE}:\n    name: pixels\n    type: pixel\n{cue}");
    render_song(rig, &cues, &[(fired_at, CUE_NOTE)], frames)
}

/// Renders a 150 BPM song holding `cues` (the YAML under `cues:`), firing each note at its time.
fn render_song(rig: &str, cues: &str, notes: &[(Duration, u8)], frames: u32) -> Vec<RecordedFrame> {
    let mut show = Show::load(Path::new(&format!("{FIXTURES}/shows/{rig}"))).unwrap();
    let song = format!("program: 1\nbpm: 150\ncues:\n{cues}");
    show.songs = vec![Song::parse("pixels", &song, &show).unwrap()];
    let event = |at, message: &[u8]| MidiEvent {
        at,
        message: message.to_vec(),
    };
    let mut midi = Events(
        std::iter::once(event(Duration::ZERO, &[0xC1, 1]))
            .chain(
                notes
                    .iter()
                    .map(|&(at, note)| event(at, &[0x91, note, 127])),
            )
            .collect(),
    );
    let mut sink = FakeDmx::default();
    run(
        &mut FakeClock::default(),
        &mut midi,
        &show,
        &Look::default(),
        &mut sink,
        frames,
    )
    .unwrap();
    sink.frames
}

fn matrix_cue(shape: &str, effect: &str) -> String {
    format!(
        "    shape: {shape}\n    effect: {effect}\n    seed: 7\n    params: {{ color: \"#ff0000\", width: 0.25, speed: 0.25 beats }}\n"
    )
}

#[test]
fn every_shape_and_effect_matches_its_reference_on_every_rig() {
    let bless = std::env::var_os("BLIMEY_BLESS").is_some();
    let mut failures = Vec::new();
    for rig in RIGS {
        for shape in SHAPES {
            for effect in EFFECTS {
                let frames = render(rig, &matrix_cue(shape, effect), Duration::ZERO, 4);
                let path = format!("{FIXTURES}/dumps/pixel/{rig}/{shape}-{effect}.dump");
                if bless {
                    fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
                    fs::write(&path, format_dump(&frames)).unwrap();
                    continue;
                }
                let reference = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{path}: {error}; bless to create it"));
                if let Some(difference) =
                    first_difference(&frames, &parse_dump(&reference).unwrap())
                {
                    failures.push(format!("{rig} {shape} {effect}: {difference}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The references could hold a dark or frozen case without anyone noticing: check on the 8×8
/// grid, where every shape has room, that each case lights and only static holds still.
#[test]
fn on_a_grid_every_case_lights_and_only_static_holds_still() {
    for shape in SHAPES {
        for effect in EFFECTS {
            let frames = render(
                "pixel-grid-8x8",
                &matrix_cue(shape, effect),
                Duration::ZERO,
                4,
            );
            let case = format!("{shape} {effect}");
            assert!(
                frames
                    .iter()
                    .any(|frame| frame.values.iter().any(|v| *v > 0)),
                "{case} stays dark"
            );
            let holds_still = frames
                .windows(2)
                .all(|pair| pair[0].values == pair[1].values);
            assert_eq!(holds_still, effect == "static", "{case}");
        }
    }
}

/// Each grid pixel's colour, on its own channels, in the grid's colour order.
#[test]
fn a_gradient_lands_on_each_pixels_channels_in_wiring_and_colour_order() {
    let cue = "    shape: gradient\n    params: { color: \"#ff8040\" }\n";

    // 8×8, serpentine from the bottom left, GRB. Static rests mid-travel, so the gradient runs
    // from dark at the left edge (x 0) to full at the right (x 1).
    let frame = &render("pixel-grid-8x8", cue, Duration::ZERO, 1)[0];
    let level = |column: f32| column / 8.0 + 1.0 / 16.0;
    let grb = |column: f32| {
        let at = |full: f32| (full * level(column)).round() as u8;
        [at(128.0), at(255.0), at(64.0)]
    };
    assert_eq!(frame.values[0..3], grb(0.0), "pixel 1: bottom left");
    assert_eq!(frame.values[21..24], grb(7.0), "pixel 8: bottom right");
    assert_eq!(
        frame.values[24..27],
        grb(7.0),
        "pixel 9: the next row up, back from the right"
    );
    assert_eq!(frame.values[189..192], grb(0.0), "pixel 64: top left");

    // 16×16 over Art-Net, serpentine from the top right, BGR: pixel 171 starts universe 2.
    let frame = &render("pixel-grid-16x16", cue, Duration::ZERO, 1)[0];
    let level = |column: f32| column / 16.0 + 1.0 / 32.0;
    let bgr = |column: f32| {
        let at = |full: f32| (full * level(column)).round() as u8;
        [at(64.0), at(128.0), at(255.0)]
    };
    assert_eq!(frame.artnet[&1][0..3], bgr(15.0), "pixel 1: top right");
    assert_eq!(frame.artnet[&1][45..48], bgr(0.0), "pixel 16: top left");
    // Pixel 171 is the 11th of row 11 from the top, which runs from the right as row 1 does.
    assert_eq!(frame.artnet[&2][0..3], bgr(5.0), "pixel 171");
    assert_eq!(frame.artnet.len(), 2);
}

/// The bottom row of the 8×8 grid, wired from the bottom left: whether each column is lit red.
fn lit_columns(frame: &RecordedFrame) -> Vec<usize> {
    (0..8)
        .filter(|column| frame.values[column * 3 + 1] > 0)
        .collect()
}

#[test]
fn split_mirrors_one_bar_per_half_about_the_centre() {
    let frames = render(
        "pixel-grid-8x8",
        &matrix_cue("split", "static"),
        Duration::ZERO,
        1,
    );
    assert_eq!(
        lit_columns(&frames[0]),
        [1, 2, 5, 6],
        "a quarter of the canvas, mid-way in each half"
    );

    let cue = "    shape: split\n    effect: move\n    params: { color: \"#ff0000\", width: 0.25, speed: 800ms }\n";
    let frames = render("pixel-grid-8x8", cue, Duration::ZERO, 32);
    assert_eq!(lit_columns(&frames[0]), [3, 4], "together at the centre");
    assert_eq!(
        lit_columns(&frames[24]),
        [0, 1, 6, 7],
        "apart towards the edges"
    );
}

#[test]
fn one_by_one_steps_through_a_grid_column_by_column() {
    let frames = render(
        "pixel-grid-8x8",
        &matrix_cue("one-by-one", "chase"),
        Duration::ZERO,
        4,
    );

    let columns: Vec<Vec<usize>> = frames.iter().map(lit_columns).collect();
    assert_eq!(columns, [[1], [3], [5], [7]]);
}

/// Plan §5: legacy < cues < pixel < master, whatever order the notes come in.
#[test]
fn a_pixel_cue_draws_above_a_scene_fired_after_it() {
    let cues = "  60: { name: red-pixels, type: pixel, params: { color: \"#ff0000\" } }\n  61: { name: blue-scene, type: scene, values: { all-wash: { color: \"#0000ff\" } } }\n";
    let frames = render_song(
        "four-washes",
        cues,
        &[(Duration::ZERO, 60), (Duration::from_millis(100), 61)],
        8,
    );
    let wash_1 = |frame: &RecordedFrame| frame.values[0..4].to_vec();

    assert_eq!(wash_1(&frames[2]), [255, 0, 0, 0], "the pixel cue alone");
    assert_eq!(
        wash_1(&frames[6]),
        [255, 0, 0, 0],
        "the scene fired later stays under it"
    );
}

#[test]
fn bottom_to_top_runs_from_the_floor_up() {
    let cue = "    shape: sweep\n    effect: move\n    params: { color: \"#ffffff\", speed: 3200ms, direction: bottom-to-top }\n";
    let frames = render("pixel-column-1x32", cue, Duration::ZERO, 40);
    // WRGB from address 101, wired from the top: pixel 32 is the bottom one.
    let white = |frame: &RecordedFrame, pixel: usize| frame.values[100 + (pixel - 1) * 4 + 1];

    let half_way = &frames[16 * 2];
    assert_eq!(white(half_way, 32), 255, "the floor fills first");
    assert_eq!(white(half_way, 1), 0, "the top fills last");
}

#[test]
fn the_same_seed_gives_the_same_frames_and_a_frame_depends_only_on_time_since_the_note() {
    let cue = |seed: u64| {
        format!(
            "    effect: sparkle\n    seed: {seed}\n    params: {{ color: \"#ffa01e\", speed: 1 beat }}\n    layer_2: {{ shape: circle, effect: move, blend: add, params: {{ width: 0.2 }} }}\n"
        )
    };
    let frames = |seed, fired_at| render("pixel-grid-8x8", &cue(seed), fired_at, 80);

    let first = frames(7, Duration::ZERO);
    assert_eq!(first, frames(7, Duration::ZERO));
    assert_ne!(
        first,
        frames(8, Duration::ZERO),
        "another seed sparkles elsewhere"
    );

    let later = frames(7, Duration::from_secs(1));
    for (early, late) in first[..40].iter().zip(&later[40..]) {
        assert_eq!(early.values, late.values, "{:?}", late.at);
    }
}

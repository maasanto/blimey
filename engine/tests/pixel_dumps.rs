//! Pixel cues (plan §7, §15, §16): every shape × effect on four rigs, compared to reference
//! dumps in `tests/fixtures/dumps/pixel/<rig>/<shape>-<effect>.dump`, the mapping and
//! determinism checks behind them, and layers with areas and mutes.
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
use blimey_engine::rig::Rig;
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

/// Renders a 150 BPM song holding `cues` (the YAML under `cues:`) on the show folder `rig`,
/// firing each channel-2 note at its time.
fn render_song(rig: &str, cues: &str, notes: &[(Duration, u8)], frames: u32) -> Vec<RecordedFrame> {
    let show = Show::load(Path::new(&format!("{FIXTURES}/shows/{rig}"))).unwrap();
    let messages: Vec<(Duration, Vec<u8>)> = notes
        .iter()
        .map(|&(at, note)| (at, vec![0x91, note, 127]))
        .collect();
    render_show(show, cues, &messages, frames)
}

/// Renders a 150 BPM song holding `cues` on `show`, selected at 0 ms, then `messages`.
fn render_show(
    mut show: Show,
    cues: &str,
    messages: &[(Duration, Vec<u8>)],
    frames: u32,
) -> Vec<RecordedFrame> {
    let song = format!("program: 1\nbpm: 150\ncues:\n{cues}");
    show.songs = vec![Song::parse("pixels", &song, &show).unwrap()];
    play(&show, messages, frames)
}

/// Renders `show`, its song on program 1 selected at 0 ms, then `messages`.
fn play(show: &Show, messages: &[(Duration, Vec<u8>)], frames: u32) -> Vec<RecordedFrame> {
    let select = (Duration::ZERO, vec![0xC1, 1]);
    let mut midi = Events(
        std::iter::once(&select)
            .chain(messages)
            .map(|(at, message)| MidiEvent {
                at: *at,
                message: message.clone(),
            })
            .collect(),
    );
    let mut sink = FakeDmx::default();
    run(
        &mut FakeClock::default(),
        &mut midi,
        show,
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
fn one_by_one_chases_a_grid_column_by_column_whatever_the_width() {
    // Default params but a cycle of 8 frames: width 1.
    let cue = "    shape: one-by-one\n    effect: chase\n    params: { color: \"#ff0000\", speed: 200ms }\n";
    let frames = render("pixel-grid-8x8", cue, Duration::ZERO, 8);

    let columns: Vec<Vec<usize>> = frames.iter().map(lit_columns).collect();
    assert_eq!(columns, [[0], [1], [2], [3], [4], [5], [6], [7]]);
}

#[test]
fn one_by_one_chases_the_washes_one_by_one_with_default_params() {
    let cue = "    shape: one-by-one\n    effect: chase\n    params: { color: \"#ff0000\", speed: 100ms }\n";
    let frames = render("four-washes", cue, Duration::ZERO, 4);
    // Wash addresses in the test rig; red is each wash's first channel.
    let lit_washes = |frame: &RecordedFrame| -> Vec<usize> {
        [1, 17, 33, 101]
            .into_iter()
            .enumerate()
            .filter(|(_, address)| frame.values[address - 1] > 0)
            .map(|(wash, _)| wash + 1)
            .collect()
    };

    let washes: Vec<Vec<usize>> = frames.iter().map(lit_washes).collect();
    assert_eq!(washes, [[1], [2], [3], [4]]);
}

#[test]
fn one_by_one_lights_grids_at_the_same_stage_position_together() {
    // Two 2-column grids, one above the other, at the same position across the stage.
    let grid = |name: &str, y: f32, address: u32| {
        format!(
            "  - {{ name: {name}, type: pixel-grid, columns: 2, rows: 1, colour_order: RGB, address: {address}, canvas: {{ x: 0, y: {y}, width: 1, height: 0.5 }} }}\n"
        )
    };
    let rig = format!("fixtures:\n{}{}", grid("low", 0.0, 1), grid("high", 0.5, 7));
    let show = Show {
        rig: Rig::parse(&rig, Path::new(".")).unwrap(),
        ..Show::default()
    };
    let cues = "  60: { name: chase, type: pixel, shape: one-by-one, effect: chase, params: { color: \"#ff0000\", speed: 50ms } }\n";

    let frames = render_show(show, cues, &[(Duration::ZERO, vec![0x91, 60, 127])], 2);

    // Red of low's left and right pixels, then high's.
    let reds = |frame: &RecordedFrame| {
        [
            frame.values[0],
            frame.values[3],
            frame.values[6],
            frame.values[9],
        ]
    };
    assert_eq!(reds(&frames[0]), [255, 0, 255, 0], "both left columns");
    assert_eq!(reds(&frames[1]), [0, 255, 0, 255], "both right columns");
}

/// Plan §5: a pixel cue overrides only the channels it touches.
#[test]
fn a_pixel_cue_leaves_fixtures_and_channels_it_does_not_touch_to_the_layers_below() {
    let show = Show::load(Path::new(&format!("{FIXTURES}/shows/pixel-layers"))).unwrap();
    let cues = "  61: { name: haze, type: scene, values: { smoke-dimmer: { dimmer: 60% } } }\n  62: { name: red, type: pixel, params: { color: \"#ff0000\" } }\n";
    let messages = [
        (Duration::ZERO, vec![0x90, 60, 100]),
        (Duration::ZERO, vec![0x91, 61, 127]),
        (Duration::ZERO, vec![0x91, 62, 127]),
    ];

    let frames = render_show(show, cues, &messages, 1);

    let values = &frames[0].values;
    assert_eq!(
        values[0], 153,
        "the scene's dimmer on a fixture with no emitters"
    );
    assert_eq!(
        values[1..4],
        [255, 0, 0],
        "the pixel cue on the par's emitters"
    );
    assert_eq!(values[4], 200, "legacy keeps the par's strobe channel");
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

/// Plan §8: a pattern drives a pixel cue's intensity. At 150 BPM a beat is 16 frames: a 1-beat
/// ramp-up on beat 1, 50% held for half a beat on beat 3, released on beat 3.5 over a beat.
#[test]
fn a_pattern_on_a_pixel_cues_intensity_pulses_it_then_lets_it_back_up_on_release() {
    let cues = "  60: { name: red-wash, type: pixel, params: { color: \"#ff0000\" } }\n  61:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { red-wash: intensity }\n    release: 1 beat\n    hits:\n      - { at: 1, shape: ramp-up, length: 1 beat, level: 100% }\n      - { at: 3, shape: hold, length: 0.5 beats, level: 50% }\n";
    let show = Show::load(Path::new(&format!("{FIXTURES}/shows/four-washes"))).unwrap();
    let messages = [
        (Duration::ZERO, vec![0x91, 60, 127]),
        (Duration::ZERO, vec![0x91, 61, 127]),
        (Duration::from_millis(1000), vec![0x81, 61, 0]),
    ];

    let frames = render_show(show, cues, &messages, 64);

    let path = format!("{FIXTURES}/dumps/pixel/intensity-link.dump");
    if std::env::var_os("BLIMEY_BLESS").is_some() {
        fs::write(&path, format_dump(&frames)).unwrap();
    }
    let reference = parse_dump(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(first_difference(&frames, &reference), None);
    // Wash 1's first red, which the pixel cue lights full at full intensity.
    let red: Vec<u8> = [0, 8, 16, 32, 40, 48, 56]
        .iter()
        .map(|frame| frames[*frame].values[0])
        .collect();
    assert_eq!(
        red,
        [0, 128, 0, 128, 0, 128, 255],
        "dark, half-way up the ramp, dark, the 50% hit, dark at release, half-way back, full a beat after"
    );
}

/// A pixel cue whose intensity a pattern drives, over a red scene at 50% that a dimmer-only
/// swell brings to full: the pattern still pulses the pixel cue, and once the pixel cue goes the
/// washes show the swell's full red, the virtual dimmers below untouched by either. At 150 BPM
/// a beat is 16 frames; both are released on beat 4.
#[test]
fn a_pixel_cue_linked_to_a_pattern_leaves_a_dimmer_only_swell_below_it_alone() {
    let cues = "  50: { name: red-half, type: scene, values: { all-wash: { color: \"#ff0000\", dimmer: 50% } } }\n  51: { name: full, type: swell, target: { all-wash: { dimmer: 100% } }, duration: 1 beat }\n  60: { name: green-wash, type: pixel, params: { color: \"#00ff00\" } }\n  61:\n    name: pulse\n    type: pattern\n    length: 1 bar\n    target: { green-wash: intensity }\n    hits:\n      - { at: 1, shape: ramp-up, length: 1 beat, level: 100% }\n";
    let show = Show::load(Path::new(&format!("{FIXTURES}/shows/four-washes"))).unwrap();
    let note_on = |note: u8| (Duration::ZERO, vec![0x91, note, 127]);
    let beat_4 = Duration::from_millis(1200);
    let messages = [
        note_on(50),
        note_on(51),
        note_on(60),
        note_on(61),
        (beat_4, vec![0x81, 60, 0]),
        (beat_4, vec![0x81, 61, 0]),
    ];

    let frames = render_show(show, cues, &messages, 56);

    // Wash 1's first red, green, blue and white.
    let wash_1 = |frame: usize| frames[frame].values[0..4].to_vec();
    assert_eq!(wash_1(8), [0, 128, 0, 0], "half-way up the pattern's ramp");
    assert_eq!(wash_1(24), [0, 0, 0, 0], "the pixel cue dark between hits");
    assert_eq!(
        wash_1(52),
        [255, 0, 0, 0],
        "the scene's red, brought to full"
    );
}

#[test]
fn a_swell_on_a_pixel_cues_intensity_brings_it_up_along_its_curve() {
    let cues = "  60: { name: red-wash, type: pixel, params: { color: \"#ff0000\" } }\n  62: { name: rise, type: swell, target: { red-wash: intensity }, duration: 1 beat }\n";
    let frames = render_song(
        "four-washes",
        cues,
        &[(Duration::ZERO, 60), (Duration::ZERO, 62)],
        20,
    );

    let red = |frame: usize| frames[frame].values[0];
    assert_eq!([red(0), red(8), red(16), red(19)], [0, 128, 255, 255]);
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

/// Compares `frames` with the reference dump at `name` under `dumps/pixel/`, or writes it when
/// blessing.
fn assert_reference(frames: &[RecordedFrame], name: &str) {
    let path = format!("{FIXTURES}/dumps/pixel/{name}");
    if std::env::var_os("BLIMEY_BLESS").is_some() {
        fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        fs::write(&path, format_dump(frames)).unwrap();
    }
    let reference = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path}: {error}; bless to create it"));
    assert_eq!(
        first_difference(frames, &parse_dump(&reference).unwrap()),
        None,
        "{name}"
    );
}

/// At 150 BPM: the example's chase cycle.
const BAR_FRAMES: u32 = 64;

/// Plan §16's example cue without its swell, held for a bar: on the three-row rig, and on the
/// 8×8 grid without its `mute`, since that rig has no inner washes.
#[test]
fn the_layered_example_cue_matches_its_reference_on_three_rows_of_washes_and_on_a_grid() {
    let three_rows = Show::load(Path::new(&format!("{FIXTURES}/shows/three-rows"))).unwrap();
    let note_on = [(Duration::ZERO, vec![0x91, 43, 127])];

    let frames = play(&three_rows, &note_on, BAR_FRAMES);

    assert_reference(&frames, "layers/three-rows.dump");
    // Each wash's first pixel, RGBW, by its address.
    let pixel =
        |frame: &RecordedFrame, address: usize| frame.values[address - 1..address + 3].to_vec();
    let warm_white = [255, 210, 160, 0];
    let (low_1, low_4, high_1, high_2) = (1, 49, 129, 145);
    assert!(
        frames
            .iter()
            .all(|frame| pixel(frame, low_1) == warm_white && pixel(frame, high_2) == warm_white),
        "the left half holds warm white, sparkle muted on the inner washes"
    );
    assert!(
        frames
            .iter()
            .any(|frame| pixel(frame, high_1) != warm_white),
        "the top row sparkles on the outer washes"
    );
    assert_eq!(
        [pixel(&frames[0], low_4), pixel(&frames[40], low_4)],
        [[0, 0, 0, 0], [255, 160, 30, 0]],
        "the amber chase reaches the right wash on its third step"
    );

    let song = fs::read_to_string(format!(
        "{FIXTURES}/shows/three-rows/songs/chorus-build.yaml"
    ))
    .unwrap();
    let mute = "        mute: [inner-wash]\n";
    assert!(song.contains(mute));
    let mut grid = Show {
        rig: Show::load(Path::new(&format!("{FIXTURES}/shows/pixel-grid-8x8")))
            .unwrap()
            .rig,
        palettes: three_rows.palettes,
        ..Show::default()
    };
    grid.songs = vec![Song::parse("chorus-build", &song.replace(mute, ""), &grid).unwrap()];

    assert_reference(
        &play(&grid, &note_on, BAR_FRAMES),
        "layers/pixel-grid-8x8.dump",
    );
}

/// Plan §16: a shape runs in its area's own 0–1 space. For every shape, effect and direction, a
/// layer in an area gives the same frames as that shape on the whole canvas of a rig squeezed
/// into the area. Positions are short binary fractions, so squeezing them is exact in floats.
#[test]
fn a_shape_in_an_area_matches_it_on_the_whole_canvas_of_a_rig_squeezed_into_the_area() {
    const DIRECTIONS: [&str; 4] = [
        "left-to-right",
        "right-to-left",
        "bottom-to-top",
        "top-to-bottom",
    ];
    // x, y, width, height.
    let areas = [
        [0.25, 0.5, 0.5, 0.25],
        [0.0, 0.0, 0.5, 1.0],
        [0.5, 0.25, 0.5, 0.75],
    ];
    // Two grids of different sizes, one above the other, filling the area squeezed into.
    let rig = |[x, y, width, height]: [f32; 4]| {
        let grid = |name: &str, columns: u32, rows: u32, address: u32, canvas: [f32; 4]| {
            format!(
                "  - {{ name: {name}, type: pixel-grid, columns: {columns}, rows: {rows}, colour_order: RGB, address: {address}, canvas: {{ x: {}, y: {}, width: {}, height: {} }} }}\n",
                x + width * canvas[0],
                y + height * canvas[1],
                width * canvas[2],
                height * canvas[3],
            )
        };
        let yaml = format!(
            "fixtures:\n{}{}",
            grid("low", 8, 4, 1, [0.0, 0.0, 1.0, 0.5]),
            grid("high", 4, 4, 97, [0.25, 0.5, 0.5, 0.5]),
        );
        Show {
            rig: Rig::parse(&yaml, Path::new(".")).unwrap(),
            ..Show::default()
        }
    };
    let note_on = [(Duration::ZERO, vec![0x91, CUE_NOTE, 127])];
    let mut shapes = SHAPES.to_vec();
    shapes.push("fill");
    for area in areas {
        let [x, y, width, height] = area;
        for shape in &shapes {
            for effect in EFFECTS {
                for direction in DIRECTIONS {
                    let layer = format!(
                        "shape: {shape}, effect: {effect}, params: {{ color: \"#ff0000\", background: \"#000040\", width: 0.25, speed: 0.25 beats, direction: {direction} }}"
                    );
                    let cue = |area: &str| {
                        format!(
                            "  {CUE_NOTE}:\n    name: pixels\n    type: pixel\n    seed: 7\n    layers:\n      - {{ {area}{layer} }}\n"
                        )
                    };
                    let in_area = render_show(
                        rig(area),
                        &cue(&format!(
                            "area: {{ x: {x}, y: {y}, width: {width}, height: {height} }}, "
                        )),
                        &note_on,
                        4,
                    );
                    let whole = render_show(rig([0.0, 0.0, 1.0, 1.0]), &cue(""), &note_on, 4);

                    assert_eq!(in_area, whole, "{shape} {effect} {direction} in {area:?}");
                }
            }
        }
    }
}

/// A muted fixture, or the fixtures of a muted group, show the layers below.
#[test]
fn a_layer_leaves_the_fixtures_and_groups_it_mutes_to_the_layers_below() {
    let cases = [
        ("[]", ["blue", "blue", "blue", "blue"]),
        ("[wash-2]", ["blue", "red", "blue", "blue"]),
        ("[outer-wash]", ["red", "blue", "blue", "red"]),
        ("[outer-wash, wash-2]", ["red", "red", "blue", "red"]),
        ("[all-wash]", ["red", "red", "red", "red"]),
    ];
    for (mute, expected) in cases {
        let cues = format!(
            "  60:\n    name: muted\n    type: pixel\n    layers:\n      - {{ shape: fill, params: {{ color: \"#ff0000\" }} }}\n      - {{ shape: fill, blend: over, params: {{ color: \"#0000ff\" }}, mute: {mute} }}\n"
        );
        let frame = &render_song("four-washes", &cues, &[(Duration::ZERO, 60)], 1)[0];

        // Wash addresses in the test rig; each pixel is RGBW.
        let colours =
            [1, 17, 33, 101].map(|address| match frame.values[address - 1..address + 2] {
                [255, 0, 0] => "red",
                [0, 0, 255] => "blue",
                _ => "neither",
            });
        assert_eq!(colours, expected, "mute: {mute}");
    }

    let cues = "  60:\n    name: muted\n    type: pixel\n    layers:\n      - { shape: fill, params: { color: \"#ff0000\" } }\n      - { shape: fill, blend: over, params: { color: \"#0000ff\" }, mute: [grid] }\n";
    let frame = &render_song("pixel-grid-8x8", cues, &[(Duration::ZERO, 60)], 1)[0];
    assert_eq!(frame.values[0..3], [0, 255, 0], "a muted grid, GRB");
}

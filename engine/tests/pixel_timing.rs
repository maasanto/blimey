//! Render-time budgets for pixel cues (plan §7 scale target, §15 load test), run by `cargo test`
//! in CI.
//!
//! Measured through the render loop, two or eight layers on every pixel. To stay steady on shared
//! runners, each check renders several batches of 2 s of show (80 frames) and keeps the fastest
//! batch's mean frame time: other jobs and preemption only ever add time, so the minimum is the
//! engine's own cost. The budgets hold in the unoptimised test build, which is several times
//! slower than the release build that plays shows.

use std::path::Path;
use std::time::{Duration, Instant};

use blimey_engine::look::Look;
use blimey_engine::rig::Rig;
use blimey_engine::song::Song;
use blimey_engine::{ArtNetFrames, DmxSink, FakeClock, Frame, MidiEvent, MidiSource, Show, run};

const BATCHES: usize = 5;
const FRAMES_PER_BATCH: u32 = 80;
const TWO_LAYERS: &str = "program: 1\ncues:\n  60:\n    name: busy\n    type: pixel\n    effect: chase\n    params: { color: \"#ff8040\", width: 0.1, speed: 1 beat }\n    layer_2: { effect: sparkle, blend: add, params: { color: \"#2040ff\", speed: 2 beats } }\n";
/// One-by-one ranks every pixel against the rig's lines, on both layers.
const TWO_LAYERS_ONE_BY_ONE: &str = "program: 1\ncues:\n  60:\n    name: busy\n    type: pixel\n    shape: one-by-one\n    effect: chase\n    params: { color: \"#ff8040\", speed: 1 beat }\n    layer_2: { shape: one-by-one, effect: move, blend: max, params: { color: \"#2040ff\", speed: 2 beats, direction: bottom-to-top } }\n";
/// Plan §16's limit: 8 layers, in areas, sparkle and one-by-one among them.
const EIGHT_LAYERS: &str = "program: 1\ncues:\n  60:\n    name: busy\n    type: pixel\n    layers:\n      - { shape: fill, params: { color: \"#402010\" } }\n      - { area: { x: 0, y: 0, width: 0.5, height: 1 }, effect: chase, blend: over, params: { color: \"#ff8040\", width: 0.25, speed: 1 beat } }\n      - { area: { x: 0.5, y: 0, width: 0.5, height: 1 }, shape: one-by-one, effect: chase, blend: over, params: { color: \"#2040ff\", speed: 1 beat } }\n      - { area: { x: 0, y: 0.6, width: 1, height: 0.4 }, shape: fill, effect: sparkle, blend: add, params: { color: \"#ffffff\", speed: 2 beats } }\n      - { area: { x: 0.25, y: 0.25, width: 0.5, height: 0.5 }, shape: circle, effect: move, blend: max, params: { color: \"#00ff00\", width: 0.2 } }\n      - { shape: gradient, effect: move, blend: add, level: 30%, params: { color: \"#ff0000\", direction: bottom-to-top } }\n      - { area: { x: 0, y: 0, width: 1, height: 0.3 }, shape: split, effect: pulse, blend: over, params: { color: \"#ffff00\", width: 0.1, background: \"#000020\" } }\n      - { shape: sweep, effect: move, blend: mask, params: { speed: 4 beats } }\n";
/// 40 × 34 RGB pixels: 1360, exactly 8 universes of 170.
const WALL: &str = "fixtures:\n  - { name: wall, type: pixel-grid, columns: 40, rows: 34, colour_order: RGB, wiring: serpentine, output: artnet, universe: 1, address: 1, canvas: { x: 0, y: 0, width: 1, height: 1 } }\n";

/// Selects the song and fires the cue on the first poll.
struct FireAtOnce(Vec<MidiEvent>);

impl MidiSource for FireAtOnce {
    fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
        (!self.0.is_empty()).then(|| self.0.remove(0))
    }
}

/// Counts what it is sent, so rendering can't be optimised away.
#[derive(Default)]
struct Checksum(u64);

impl DmxSink for Checksum {
    fn send(&mut self, _at: Duration, frame: &Frame) -> std::io::Result<()> {
        self.0 += frame.iter().map(|value| u64::from(*value)).sum::<u64>();
        Ok(())
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) -> std::io::Result<()> {
        self.0 += universes
            .values()
            .flatten()
            .map(|v| u64::from(*v))
            .sum::<u64>();
        Ok(())
    }
}

/// The fastest batch's mean time per frame for the cue in `song` on `rig_yaml`.
fn frame_time(rig_yaml: &str, song: &str) -> Duration {
    let mut show = Show {
        rig: Rig::parse(rig_yaml, Path::new(".")).unwrap(),
        ..Show::default()
    };
    show.songs = vec![Song::parse("busy", song, &show).unwrap()];
    let fire = || {
        FireAtOnce(vec![
            MidiEvent {
                at: Duration::ZERO,
                message: vec![0xC1, 1],
            },
            MidiEvent {
                at: Duration::ZERO,
                message: vec![0x91, 60, 127],
            },
        ])
    };
    (0..BATCHES)
        .map(|_| {
            let mut sink = Checksum::default();
            let started = Instant::now();
            run(
                &mut FakeClock::default(),
                &mut fire(),
                &show,
                &Look::default(),
                &mut sink,
                FRAMES_PER_BATCH,
            )
            .unwrap();
            let elapsed = started.elapsed();
            assert!(sink.0 > 0, "the cue lit nothing");
            elapsed / FRAMES_PER_BATCH
        })
        .min()
        .unwrap()
}

#[test]
fn a_full_universe_of_pixels_renders_in_under_1_ms() {
    // 128 RGBW pixels: all 512 channels of the Enttec universe.
    let rig = "fixtures:\n  - { name: grid, type: pixel-grid, columns: 16, rows: 8, colour_order: RGBW, address: 1, canvas: { x: 0, y: 0, width: 1, height: 1 } }\n";

    let time = frame_time(rig, TWO_LAYERS);

    eprintln!("512 channels: {time:?} per frame");
    assert!(time < Duration::from_millis(1), "{time:?} per frame");
}

#[test]
fn eight_universes_of_grids_render_in_under_5_ms() {
    assert_eq!(
        Rig::parse(WALL, Path::new("."))
            .unwrap()
            .artnet_universes
            .len(),
        8
    );

    for (name, song) in [
        ("chase and sparkle", TWO_LAYERS),
        ("one-by-one", TWO_LAYERS_ONE_BY_ONE),
        ("8 layers", EIGHT_LAYERS),
    ] {
        let time = frame_time(WALL, song);

        eprintln!("8 universes, {name}: {time:?} per frame");
        assert!(
            time < Duration::from_millis(5),
            "{name}: {time:?} per frame"
        );
    }
}

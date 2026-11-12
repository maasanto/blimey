//! Renders a contact sheet of every pixel shape × effect on a 16×16 grid, to pick defaults by eye:
//! `cargo run -p blimey-engine --example pixel_sheet`, written to
//! `claude-artifacts/previews/pixel-sheet.png`.
//!
//! Rows are shapes, columns effects, in the order printed. Each cell is one cycle of the effect,
//! 8 frames from left to right; each frame is the grid as seen from the audience.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use blimey_engine::look::Look;
use blimey_engine::rig::Rig;
use blimey_engine::song::Song;
use blimey_engine::{
    FakeClock, FakeDmx, MidiEvent, MidiSource, RecordedFrame, Show, Universe, run,
};

const SHAPES: [&str; 6] = ["bar", "sweep", "circle", "split", "gradient", "one-by-one"];
const EFFECTS: [&str; 5] = ["static", "move", "pulse", "chase", "sparkle"];
const SIDE: usize = 16;
const GRID: &str = "fixtures:\n  - { name: grid, type: pixel-grid, columns: 16, rows: 16, colour_order: RGB, output: artnet, universe: 0, address: 1, canvas: { x: 0, y: 0, width: 1, height: 1 } }\n";
/// At 150 BPM, half a beat is 200 ms: 8 frames per cycle.
const FRAMES: usize = 8;
const LED: usize = 4;
const FRAME_GAP: usize = 4;
const CELL_GAP: usize = 16;
const GAP_GREY: u8 = 0x30;

struct Events(VecDeque<MidiEvent>);

impl MidiSource for Events {
    fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
        self.0.pop_front()
    }
}

fn render(rig: &Rig, shape: &str, effect: &str) -> Vec<RecordedFrame> {
    let mut show = Show {
        rig: rig.clone(),
        ..Show::default()
    };
    let song = format!(
        "program: 1\nbpm: 150\ncues:\n  60:\n    name: preview\n    type: pixel\n    shape: {shape}\n    effect: {effect}\n    seed: 7\n    params: {{ color: \"#ffa01e\", width: 0.25, speed: 0.5 beats }}\n"
    );
    show.songs = vec![Song::parse("preview", &song, &show).unwrap()];
    let message = |bytes: &[u8]| MidiEvent {
        at: Duration::ZERO,
        message: bytes.to_vec(),
    };
    let mut midi = Events(VecDeque::from([
        message(&[0xC1, 1]),
        message(&[0x91, 60, 127]),
    ]));
    let mut sink = FakeDmx::default();
    run(
        &mut FakeClock::default(),
        &mut midi,
        &show,
        &Look::default(),
        &mut sink,
        FRAMES as u32,
    )
    .unwrap();
    sink.frames
}

fn main() {
    let rig = Rig::parse(GRID, Path::new(".")).unwrap();
    let frame_width = SIDE * LED;
    let cell_width = FRAMES * frame_width + (FRAMES - 1) * FRAME_GAP;
    let width = EFFECTS.len() * cell_width + (EFFECTS.len() + 1) * CELL_GAP;
    let height = SHAPES.len() * frame_width + (SHAPES.len() + 1) * CELL_GAP;
    let mut image = vec![GAP_GREY; width * height * 3];

    for (row, shape) in SHAPES.iter().enumerate() {
        for (column, effect) in EFFECTS.iter().enumerate() {
            for (index, frame) in render(&rig, shape, effect).iter().enumerate() {
                let left =
                    CELL_GAP + column * (cell_width + CELL_GAP) + index * (frame_width + FRAME_GAP);
                let top = CELL_GAP + row * (frame_width + CELL_GAP);
                for pixel in &rig.grids[0].pixels {
                    let Universe::ArtNet(port_address) = pixel.universe else {
                        unreachable!("the sample grid is on Art-Net");
                    };
                    let universe = &frame.artnet[&port_address];
                    let level = |channel: Option<usize>| universe[channel.unwrap()];
                    let rgb = [
                        level(pixel.channels.red),
                        level(pixel.channels.green),
                        level(pixel.channels.blue),
                    ];
                    let cell = |position: f32| (position * SIDE as f32 - 0.5).round() as usize;
                    let x = left + cell(pixel.x) * LED;
                    // Canvas y counts up from the floor; image rows count down from the top.
                    let y = top + (SIDE - 1 - cell(pixel.y)) * LED;
                    for dy in 0..LED - 1 {
                        for dx in 0..LED - 1 {
                            let at = ((y + dy) * width + x + dx) * 3;
                            image[at..at + 3].copy_from_slice(&rgb);
                        }
                    }
                }
            }
        }
    }

    let out: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../claude-artifacts/previews/pixel-sheet.png");
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut encoder = png::Encoder::new(
        BufWriter::new(File::create(&out).unwrap()),
        width as u32,
        height as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&image)
        .unwrap();
    println!("rows, top to bottom: {}", SHAPES.join(", "));
    println!("columns, left to right: {}", EFFECTS.join(", "));
    println!("each cell: one cycle, 8 frames left to right; amber, width 0.25, seed 7");
    println!("wrote {}", out.canonicalize().unwrap().display());
}

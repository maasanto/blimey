//! Renders a contact sheet of every pixel shape × effect on a 16×16 grid, to pick defaults by eye:
//! `cargo run -p blimey-engine --example pixel_sheet`, written to
//! `claude-artifacts/previews/pixel-sheet.png`.
//!
//! Rows are shapes, columns effects, both labelled. Each cell is one cycle of the effect,
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

const SHAPES: [&str; 11] = [
    "bar",
    "sweep",
    "circle",
    "split",
    "gradient",
    "one-by-one",
    "diamond",
    "diagonal",
    "stripes",
    "checker",
    "wedge",
];
const EFFECTS: [&str; 8] = [
    "static", "move", "pulse", "chase", "sparkle", "bounce", "strobe", "noise",
];
const SIDE: usize = 16;
const GRID: &str = "fixtures:\n  - { name: grid, type: pixel-grid, columns: 16, rows: 16, colour_order: RGB, output: artnet, universe: 0, address: 1, canvas: { x: 0, y: 0, width: 1, height: 1 } }\n";
/// At 150 BPM, half a beat is 200 ms: 8 frames per cycle.
const FRAMES: usize = 8;
const LED: usize = 4;
const FRAME_WIDTH: usize = SIDE * LED;
const FRAME_GAP: usize = 4;
const CELL_WIDTH: usize = FRAMES * FRAME_WIDTH + (FRAMES - 1) * FRAME_GAP;
const CELL_GAP: usize = 16;
const GAP_GREY: [u8; 3] = [0x30; 3];
const LABEL_GREY: [u8; 3] = [0xd0; 3];
/// Each font pixel is drawn this many image pixels square.
const LABEL_SCALE: usize = 3;
/// Room for the longest row label, "one-by-one", and for the column labels above the cells.
const LEFT: usize = 10 * 4 * LABEL_SCALE + CELL_GAP;
const TOP: usize = 5 * LABEL_SCALE + CELL_GAP;

/// A 3×5 font for the labels' letters, one string per row, `#` lit.
const FONT: [(char, [&str; 5]); 23] = [
    ('a', [".#.", "#.#", "###", "#.#", "#.#"]),
    ('b', ["##.", "#.#", "##.", "#.#", "##."]),
    ('c', [".##", "#..", "#..", "#..", ".##"]),
    ('d', ["##.", "#.#", "#.#", "#.#", "##."]),
    ('e', ["###", "#..", "##.", "#..", "###"]),
    ('g', [".##", "#..", "#.#", "#.#", ".##"]),
    ('h', ["#.#", "#.#", "###", "#.#", "#.#"]),
    ('i', ["###", ".#.", ".#.", ".#.", "###"]),
    ('k', ["#.#", "#.#", "##.", "#.#", "#.#"]),
    ('l', ["#..", "#..", "#..", "#..", "###"]),
    ('m', ["#.#", "###", "###", "#.#", "#.#"]),
    ('n', ["##.", "#.#", "#.#", "#.#", "#.#"]),
    ('o', [".#.", "#.#", "#.#", "#.#", ".#."]),
    ('p', ["##.", "#.#", "##.", "#..", "#.."]),
    ('r', ["##.", "#.#", "##.", "#.#", "#.#"]),
    ('s', [".##", "#..", ".#.", "..#", "##."]),
    ('t', ["###", ".#.", ".#.", ".#.", ".#."]),
    ('u', ["#.#", "#.#", "#.#", "#.#", "###"]),
    ('v', ["#.#", "#.#", "#.#", "#.#", ".#."]),
    ('w', ["#.#", "#.#", "###", "###", "#.#"]),
    ('y', ["#.#", "#.#", ".#.", ".#.", ".#."]),
    ('-', ["...", "...", "###", "...", "..."]),
    (' ', ["...", "...", "...", "...", "..."]),
];

struct Events(VecDeque<MidiEvent>);

impl MidiSource for Events {
    fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
        self.0.pop_front()
    }
}

/// An RGB image being drawn.
struct Sheet {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Sheet {
    fn new(width: usize, height: usize) -> Sheet {
        Sheet {
            width,
            height,
            pixels: GAP_GREY.repeat(width * height),
        }
    }

    fn fill(&mut self, left: usize, top: usize, side: usize, rgb: [u8; 3]) {
        for y in top..top + side {
            for x in left..left + side {
                let at = (y * self.width + x) * 3;
                self.pixels[at..at + 3].copy_from_slice(&rgb);
            }
        }
    }

    fn text(&mut self, left: usize, top: usize, text: &str) {
        for (index, letter) in text.chars().enumerate() {
            let (_, rows) = FONT
                .iter()
                .find(|(glyph, _)| *glyph == letter)
                .unwrap_or_else(|| panic!("no glyph for {letter:?}"));
            for (row, bits) in rows.iter().enumerate() {
                for (column, bit) in bits.chars().enumerate() {
                    if bit == '#' {
                        let x = left + (index * 4 + column) * LABEL_SCALE;
                        self.fill(x, top + row * LABEL_SCALE, LABEL_SCALE, LABEL_GREY);
                    }
                }
            }
        }
    }

    /// One frame of the grid, as seen from the audience, its top left at (`left`, `top`).
    fn frame(&mut self, rig: &Rig, frame: &RecordedFrame, left: usize, top: usize) {
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
            // Canvas y counts up from the floor; image rows count down from the top.
            let x = left + cell(pixel.x) * LED;
            let y = top + (SIDE - 1 - cell(pixel.y)) * LED;
            self.fill(x, y, LED - 1, rgb);
        }
    }

    fn save(&self, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = BufWriter::new(File::create(path).unwrap());
        let mut encoder = png::Encoder::new(file, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&self.pixels).unwrap();
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
    let mut sheet = Sheet::new(
        LEFT + EFFECTS.len() * (CELL_WIDTH + CELL_GAP),
        TOP + SHAPES.len() * (FRAME_WIDTH + CELL_GAP),
    );
    for (column, effect) in EFFECTS.iter().enumerate() {
        sheet.text(
            LEFT + column * (CELL_WIDTH + CELL_GAP),
            CELL_GAP / 2,
            effect,
        );
    }
    for (row, shape) in SHAPES.iter().enumerate() {
        let top = TOP + row * (FRAME_WIDTH + CELL_GAP);
        sheet.text(
            CELL_GAP / 2,
            top + (FRAME_WIDTH - 5 * LABEL_SCALE) / 2,
            shape,
        );
        for (column, effect) in EFFECTS.iter().enumerate() {
            for (index, frame) in render(&rig, shape, effect).iter().enumerate() {
                let left =
                    LEFT + column * (CELL_WIDTH + CELL_GAP) + index * (FRAME_WIDTH + FRAME_GAP);
                sheet.frame(&rig, frame, left, top);
            }
        }
    }
    let out: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../claude-artifacts/previews/pixel-sheet.png");
    sheet.save(&out);
    println!("each cell: one cycle, 8 frames left to right; amber, width 0.25, seed 7");
    println!("wrote {}", out.canonicalize().unwrap().display());
}

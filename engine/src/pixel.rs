//! Pixel scenes (plan §7, §8): a preset shape on the 2D canvas, animated by an effect, evaluated
//! at every pixel of every fixture and grid each frame. Up to two layers, blended.
//!
//! A frame is a pure function of the cue's start, the time and the beats since then: effects
//! count cycles of their `speed` from the note, and sparkle draws from a hash of the cue's seed,
//! the pixel's position and the cycle, never from running state.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::clock_follower::{ClockFollower, Moment};
use crate::colour::{Rgb, RgbwMode, to_dmx};
use crate::cue::CueEntry;
use crate::duration::{Length, TimeSignature};
use crate::envelope::Envelope;
use crate::look::{colour, draw_pixel};
use crate::rig::{Fixture, Rig, line_key};
use crate::{ArtNetFrames, DMX_CHANNELS, Show, Universes};

/// Where the shape sits, 0–1 along its travel, for effects that don't move it.
const REST_POSITION: f64 = 0.5;
/// A sparkle flash lasts this fraction of a cycle, fading linearly.
// ponytail: fixed flash length; make it a param if the preview sheet asks for denser sparkle.
const FLASH: f64 = 0.25;
/// Frames often land exactly on a chase step, on the beat; time and beat arithmetic can put
/// them a hair before it. This keeps them on the step they land on.
const STEP_ROUNDING: f64 = 1e-9;
/// Mixed into the cue's seed for layer 2, so its sparkle doesn't mirror layer 1's.
const LAYER_2_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const WHITE: Rgb = Rgb {
    red: 1.0,
    green: 1.0,
    blue: 1.0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// A band `width` wide across the direction of travel.
    #[default]
    Bar,
    /// Everything behind the travel position: a wipe.
    Sweep,
    /// A ring `width` thick around the canvas centre: outward for left-to-right and
    /// bottom-to-top, inward for right-to-left and top-to-bottom.
    Circle,
    /// The canvas split into halves mirrored about the centre line, a bar `width` wide in each:
    /// moving apart, from the centre to the edges, for left-to-right and bottom-to-top, and
    /// together for right-to-left and top-to-bottom.
    Split,
    /// A ramp from the background colour to `color` along the direction.
    Gradient,
    /// One cell at a time, in order along the direction: each fixture, and each line of a grid
    /// across the direction.
    OneByOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    /// The shape held mid-travel; `phase` moves it.
    #[default]
    Static,
    /// The shape glides along its travel once per cycle.
    Move,
    /// The shape held mid-travel, its level following `curve` once per cycle.
    Pulse,
    /// The shape jumps along its travel in steps of `width`, once per cycle.
    Chase,
    /// Each pixel of the shape, held mid-travel, flashes once per cycle at a random moment.
    Sparkle,
}

/// On the canvas as seen from the audience: bottom is the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    #[default]
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

/// How layer 2 combines with layer 1, per pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    /// The brighter of the two, per emitter.
    #[default]
    Max,
    /// The sum, clipped at full.
    Add,
    /// Layer 1 shows only where layer 2's shape is lit, as much as it is.
    Mask,
}

/// A pixel cue as written in a song file (docs/api.md).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PixelEntry {
    name: String,
    fade_in: Option<Length>,
    release: Option<Length>,
    #[serde(default)]
    seed: u64,
    #[serde(default)]
    rgbw: RgbwMode,
    #[serde(default)]
    shape: Shape,
    #[serde(default)]
    effect: Effect,
    #[serde(default)]
    params: ParamsEntry,
    layer_2: Option<SecondLayerEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecondLayerEntry {
    #[serde(default)]
    shape: Shape,
    #[serde(default)]
    effect: Effect,
    #[serde(default)]
    params: ParamsEntry,
    #[serde(default)]
    blend: Blend,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParamsEntry {
    color: Option<String>,
    background: Option<String>,
    width: Option<f64>,
    speed: Option<Length>,
    direction: Option<Direction>,
    phase: Option<f64>,
    curve: Option<Envelope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PixelScene {
    pub name: String,
    pub fade_in: Length,
    pub release: Length,
    pub seed: u64,
    pub rgbw: RgbwMode,
    pub layer: Layer,
    /// The second layer and how it blends over the first.
    pub layer_2: Option<(Blend, Layer)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub shape: Shape,
    pub effect: Effect,
    pub color: Rgb,
    pub background: Rgb,
    /// 0–1 of the canvas: a bar's or ring's thickness, and a chase's step.
    pub width: f64,
    /// One cycle of the effect.
    pub speed: Length,
    pub direction: Direction,
    /// Offset in cycles: shifts a moving effect in time, a static shape along its travel.
    pub phase: f64,
    /// A pulse's level over one cycle.
    pub curve: Envelope,
}

impl PixelEntry {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn resolve(self, show: &Show) -> Result<PixelScene, String> {
        let layer_2 = match self.layer_2 {
            Some(entry) => Some((
                entry.blend,
                entry
                    .params
                    .resolve(show, entry.shape, entry.effect)
                    .map_err(|error| format!("layer_2: {error}"))?,
            )),
            None => None,
        };
        Ok(PixelScene {
            layer: self.params.resolve(show, self.shape, self.effect)?,
            layer_2,
            name: self.name,
            fade_in: self.fade_in.unwrap_or(Length::Time(Duration::ZERO)),
            release: self.release.unwrap_or(Length::Time(Duration::ZERO)),
            seed: self.seed,
            rgbw: self.rgbw,
        })
    }
}

impl ParamsEntry {
    fn resolve(self, show: &Show, shape: Shape, effect: Effect) -> Result<Layer, String> {
        let colour_or = |name: Option<String>, default: Rgb| match name {
            Some(name) => colour(show, &name),
            None => Ok(default),
        };
        let width = self.width.unwrap_or(1.0);
        if !(width > 0.0 && width <= 1.0) {
            return Err(format!("width {width} must be above 0 and at most 1"));
        }
        let speed = self.speed.unwrap_or(Length::Bars(1.0));
        let is_zero = match speed {
            Length::Time(time) => time.is_zero(),
            Length::Beats(count) | Length::Bars(count) => count == 0.0,
        };
        if is_zero {
            return Err("speed must be above 0".into());
        }
        Ok(Layer {
            shape,
            effect,
            color: colour_or(self.color, WHITE)?,
            background: colour_or(self.background, Rgb::default())?,
            width,
            speed,
            direction: self.direction.unwrap_or_default(),
            phase: self.phase.unwrap_or(0.0),
            curve: self.curve.unwrap_or(Envelope::Sine),
        })
    }
}

impl PixelScene {
    /// A pixel cue as in a song file, `type: pixel` included, its colours from `show`.
    pub fn parse(show: &Show, cue: serde_json::Value) -> Result<PixelScene, String> {
        match CueEntry::deserialize(cue).map_err(|error| error.to_string())? {
            CueEntry::Pixel(entry) => entry.resolve(show),
            _ => Err("expected a pixel cue, with type pixel".into()),
        }
    }

    /// Draws the scene at full opacity over `out`, on every fixture and grid of the rig.
    ///
    /// `intensity` (0–1) is the cue's input level, today the note's velocity: it goes on the
    /// dimmer channel of fixtures that have one, and scales the colours of the others.
    pub fn draw(
        &self,
        rig: &Rig,
        clock: &ClockFollower,
        started: Moment,
        at: Duration,
        intensity: f32,
        out: &mut Universes,
    ) {
        let layer = self.layer.at(rig, clock, started, at, self.seed);
        let layer_2 = self.layer_2.as_ref().map(|(blend, layer)| {
            let seed = self.seed ^ LAYER_2_SEED;
            (*blend, layer.at(rig, clock, started, at, seed))
        });
        let colour = |x: f32, y: f32, cell: Cell| {
            let below = layer.colour(x, y, cell);
            match &layer_2 {
                None => below,
                Some((blend, above)) => blend.apply(below, above, x, y, cell),
            }
        };
        // Plan §5: a pixel cue overrides only the channels it touches, its fixtures' emitters
        // and dimmer. Fixtures without emitters, and other channels, keep the layers below.
        for fixture in rig.fixtures.iter().filter(|fixture| has_emitters(fixture)) {
            let start = fixture.address - 1;
            let colour_scale = match fixture.dimmer {
                Some(offset) => {
                    out.enttec[start + offset] = to_dmx(intensity);
                    1.0
                }
                None => intensity,
            };
            let centre = fixture.centre();
            for pixel in &fixture.pixels {
                let lit = colour(pixel.x, pixel.y, centre);
                draw_pixel(
                    pixel.channels,
                    start,
                    lit,
                    self.rgbw,
                    colour_scale,
                    &mut out.enttec,
                );
            }
        }
        for pixel in rig.grids.iter().flat_map(|grid| &grid.pixels) {
            let lit = colour(pixel.x, pixel.y, (pixel.x, pixel.y));
            let frame = out.frame_mut(pixel.universe);
            draw_pixel(pixel.channels, 0, lit, self.rgbw, intensity, frame);
        }
    }
}

/// Columns and rows of the square sample grid a thumbnail shows, filling the canvas: the most
/// RGB pixels that fit the Enttec universe in a square.
pub const THUMBNAIL_SIDE: usize = 12;
/// Frames in a thumbnail, evenly over one cycle, as in the preview sheet.
pub const THUMBNAIL_FRAMES: usize = 8;
/// The tempo beats and bars run at in a thumbnail: it shows one cycle whatever the speed.
const THUMBNAIL_BPM: f64 = 120.0;

/// A pixel cue, as in a song file, rendered on a sample grid for the builder's pickers, as
/// the preview sheet (`examples/pixel_sheet.rs`) renders it: `THUMBNAIL_FRAMES` frames evenly
/// over one cycle of layer 1, each the grid's colours row by row from the top left as the
/// audience sees it, at full intensity. `show` gives the palettes.
pub fn thumbnail(show: &Show, cue: serde_json::Value) -> Result<Vec<Vec<Rgb>>, String> {
    let scene = PixelScene::parse(show, cue)?;
    let rig = Rig::parse(
        &format!(
            "fixtures:\n  - {{ name: sample, type: pixel-grid, columns: {THUMBNAIL_SIDE}, rows: {THUMBNAIL_SIDE}, colour_order: RGB, address: 1, canvas: {{ x: 0, y: 0, width: 1, height: 1 }} }}\n"
        ),
        Path::new("."),
    )
    .expect("the sample grid fits the Enttec universe");
    let clock = ClockFollower::new(THUMBNAIL_BPM, TimeSignature::default());
    let started = clock.moment(Duration::ZERO);
    let second = Duration::from_secs(1);
    let cycles_per_second = clock.cycles(scene.layer.speed, started, second);
    let cycle = Duration::try_from_secs_f64(1.0 / cycles_per_second)
        .map_err(|_| "speed is too long to render a cycle of".to_string())?;
    let frames = (0..THUMBNAIL_FRAMES).map(|index| {
        let mut out = Universes {
            enttec: [0; DMX_CHANNELS],
            artnet: ArtNetFrames::new(),
        };
        let at = cycle.mul_f64(index as f64 / THUMBNAIL_FRAMES as f64);
        scene.draw(&rig, &clock, started, at, 1.0, &mut out);
        let (_, colours) = rig.pixel_colours(&out.enttec, &out.artnet).remove(0);
        colours
    });
    Ok(frames.collect())
}

fn has_emitters(fixture: &Fixture) -> bool {
    fixture.pixels.iter().any(|pixel| {
        let channels = pixel.channels;
        [channels.red, channels.green, channels.blue, channels.white]
            .iter()
            .any(Option::is_some)
    })
}

/// Where one-by-one places a pixel: its fixture's centre, so a fixture lights as one, or for
/// a grid the pixel itself, so a grid steps one line at a time across the direction (a
/// column for left-to-right, a row for bottom-to-top). Lines, not single pixels: a 16×16 grid
/// steps through 16 lines the eye follows across the stage, where 256 single pixels would be
/// too small and too many to read; a column still steps pixel by pixel. Cells at the same
/// position along the travel, in different grids or fixtures, light together.
type Cell = (f32, f32);

/// A layer at one moment: what every pixel needs that doesn't depend on the pixel.
struct LayerFrame<'frame> {
    layer: &'frame Layer,
    /// 0–1 along the travel.
    position: f64,
    /// Cycles since the note, phase included.
    cycles: f64,
    /// The pulse curve's level now; 1 for other effects.
    level: f64,
    seed: u64,
    /// For one-by-one: the rig's lines across the direction, ascending on the canvas.
    lines: &'frame [i32],
}

impl Layer {
    fn at<'frame>(
        &'frame self,
        rig: &'frame Rig,
        clock: &ClockFollower,
        started: Moment,
        at: Duration,
        seed: u64,
    ) -> LayerFrame<'frame> {
        let lines = match self.direction {
            Direction::LeftToRight | Direction::RightToLeft => &rig.lines_x,
            Direction::BottomToTop | Direction::TopToBottom => &rig.lines_y,
        };
        let cycles = clock.cycles(self.speed, started, at) + self.phase;
        let within = cycles.rem_euclid(1.0);
        let position = match self.effect {
            Effect::Static => (REST_POSITION + self.phase).rem_euclid(1.0),
            Effect::Move => within,
            Effect::Chase => {
                // One-by-one steps a cell at a time, whatever the width.
                let steps = match self.shape {
                    Shape::OneByOne => lines.len().max(1) as f64,
                    _ => (1.0 / self.travel_width()).round().max(1.0),
                };
                ((within * steps + STEP_ROUNDING).floor() + 0.5) / steps
            }
            Effect::Pulse | Effect::Sparkle => REST_POSITION,
        };
        let level = match self.effect {
            Effect::Pulse => self.curve.level(within),
            _ => 1.0,
        };
        LayerFrame {
            layer: self,
            position,
            cycles,
            level,
            seed,
            lines,
        }
    }

    /// `width` as a fraction of the shape's travel. A split's bars travel half the canvas, from
    /// the centre to an edge, so each is `width` of the canvas wide.
    fn travel_width(&self) -> f64 {
        match self.shape {
            Shape::Split => (2.0 * self.width).min(1.0),
            _ => self.width,
        }
    }
}

impl LayerFrame<'_> {
    /// One-by-one's turn for `cell`, from 0, in order along the travel.
    fn turn(&self, (x, y): Cell) -> usize {
        let key = match self.layer.direction {
            Direction::LeftToRight | Direction::RightToLeft => line_key(x),
            Direction::BottomToTop | Direction::TopToBottom => line_key(y),
        };
        let from_start = self
            .lines
            .binary_search(&key)
            .expect("the rig lists every fixture's and grid pixel's line");
        match self.layer.direction {
            Direction::LeftToRight | Direction::BottomToTop => from_start,
            Direction::RightToLeft | Direction::TopToBottom => self.lines.len() - 1 - from_start,
        }
    }

    /// How lit the pixel at (`x`, `y`) in one-by-one `cell` is, 0–1.
    fn level(&self, x: f32, y: f32, cell: Cell) -> f64 {
        let Layer {
            shape,
            effect,
            direction,
            ..
        } = *self.layer;
        let position = self.position;
        let reversed = matches!(direction, Direction::RightToLeft | Direction::TopToBottom);
        let reverse = |u: f64| if reversed { 1.0 - u } else { u };
        let half_width = self.layer.travel_width() / 2.0;
        let band = |u: f64| f64::from(u8::from((u - position).abs() <= half_width));
        let shape_level = match shape {
            Shape::Bar => band(travel(direction, x, y)),
            Shape::Sweep => f64::from(u8::from(travel(direction, x, y) <= position)),
            Shape::Circle => {
                let from_centre = f64::from((x - 0.5).hypot(y - 0.5));
                band(reverse(from_centre / 0.5f64.sqrt()))
            }
            Shape::Split => {
                let across = match direction {
                    Direction::LeftToRight | Direction::RightToLeft => x,
                    Direction::TopToBottom | Direction::BottomToTop => y,
                };
                band(reverse((2.0 * f64::from(across) - 1.0).abs()))
            }
            Shape::Gradient => (travel(direction, x, y) - position + 0.5).rem_euclid(1.0),
            Shape::OneByOne => {
                let lit_turn = (position * self.lines.len() as f64).floor();
                f64::from(u8::from(self.turn(cell) as f64 == lit_turn))
            }
        };
        let flash = match effect {
            Effect::Sparkle => self.flash(x, y),
            _ => 1.0,
        };
        shape_level * self.level * flash
    }

    /// A sparkle pixel's level: one flash per cycle, starting at a random moment.
    fn flash(&self, x: f32, y: f32) -> f64 {
        let cycle = self.cycles.floor();
        let start = random(self.seed, x, y, cycle as i64) * (1.0 - FLASH);
        let since = self.cycles - cycle - start;
        if (0.0..FLASH).contains(&since) {
            1.0 - since / FLASH
        } else {
            0.0
        }
    }

    fn colour(&self, x: f32, y: f32, cell: Cell) -> Rgb {
        mix(
            self.layer.background,
            self.layer.color,
            self.level(x, y, cell) as f32,
        )
    }
}

impl Blend {
    fn apply(self, below: Rgb, above: &LayerFrame, x: f32, y: f32, cell: Cell) -> Rgb {
        let combine = |combine: fn(f32, f32) -> f32, above: Rgb| Rgb {
            red: combine(below.red, above.red),
            green: combine(below.green, above.green),
            blue: combine(below.blue, above.blue),
        };
        match self {
            Blend::Max => combine(f32::max, above.colour(x, y, cell)),
            Blend::Add => combine(|a, b| (a + b).min(1.0), above.colour(x, y, cell)),
            Blend::Mask => mix(Rgb::default(), below, above.level(x, y, cell) as f32),
        }
    }
}

/// 0–1 along the direction of travel.
fn travel(direction: Direction, x: f32, y: f32) -> f64 {
    f64::from(match direction {
        Direction::LeftToRight => x,
        Direction::RightToLeft => 1.0 - x,
        Direction::BottomToTop => y,
        Direction::TopToBottom => 1.0 - y,
    })
}

fn mix(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let lerp = |from: f32, to: f32| from + (to - from) * amount;
    Rgb {
        red: lerp(from.red, to.red),
        green: lerp(from.green, to.green),
        blue: lerp(from.blue, to.blue),
    }
}

/// Uniform in 0–1, fixed by its inputs: SplitMix64's finaliser over each in turn.
fn random(seed: u64, x: f32, y: f32, cycle: i64) -> f64 {
    let mix = |state: u64, input: u64| {
        let mut z = (state ^ input).wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let hash = [u64::from(x.to_bits()), u64::from(y.to_bits()), cycle as u64]
        .into_iter()
        .fold(seed, mix);
    (hash >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_is_fixed_by_its_inputs_and_spread_over_0_to_1() {
        let draws: Vec<f64> = (0..1000).map(|cycle| random(7, 0.25, 0.5, cycle)).collect();

        assert_eq!(random(7, 0.25, 0.5, 3), draws[3]);
        assert_ne!(random(8, 0.25, 0.5, 3), draws[3]);
        assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        assert!((mean - 0.5).abs() < 0.05, "mean {mean}");
    }
}

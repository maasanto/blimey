//! Pixel scenes (plan §7, §8): a preset shape on the 2D canvas, animated by an effect, evaluated
//! at every pixel of every fixture and grid each frame. Up to two layers, blended.
//!
//! A frame is a pure function of the cue's start, the time and the beats since then: effects
//! count cycles of their `speed` from the note, and sparkle draws from a hash of the cue's seed,
//! the pixel's position and the cycle, never from running state.

use std::time::Duration;

use serde::Deserialize;

use crate::clock_follower::{ClockFollower, Moment};
use crate::colour::{Rgb, RgbwMode};
use crate::duration::Length;
use crate::envelope::Envelope;
use crate::look::{colour, draw_fixture, draw_pixel};
use crate::rig::Rig;
use crate::{Show, Universes};

/// Where the shape sits, 0–1 along its travel, for effects that don't move it.
const REST_POSITION: f64 = 0.5;
/// A sparkle flash lasts this fraction of a cycle, fading linearly.
// ponytail: fixed flash length; make it a param if the preview sheet asks for denser sparkle.
const FLASH: f64 = 0.25;
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
    /// Two bands mirrored about the centre line: apart for left-to-right and bottom-to-top,
    /// together for right-to-left and top-to-bottom.
    Split,
    /// A ramp from the background colour to `color` along the direction.
    Gradient,
    /// One fixture or grid at a time, in order along the direction.
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
        let colour = |x: f32, y: f32, unit: usize| {
            let below = layer.colour(x, y, unit);
            match &layer_2 {
                None => below,
                Some((blend, above)) => blend.apply(below, above, x, y, unit),
            }
        };
        for (unit, fixture) in rig.fixtures.iter().enumerate() {
            draw_fixture(
                fixture,
                intensity,
                self.rgbw,
                |pixel| colour(pixel.x, pixel.y, unit),
                &mut out.enttec,
            );
        }
        for (index, grid) in rig.grids.iter().enumerate() {
            let unit = rig.fixtures.len() + index;
            for pixel in &grid.pixels {
                let frame = out.frame_mut(pixel.universe);
                let lit = colour(pixel.x, pixel.y, unit);
                draw_pixel(pixel.channels, 0, lit, self.rgbw, intensity, frame);
            }
        }
    }
}

/// A layer at one moment: what every pixel needs that doesn't depend on the pixel.
struct LayerFrame<'scene> {
    layer: &'scene Layer,
    /// 0–1 along the travel.
    position: f64,
    /// Cycles since the note, phase included.
    cycles: f64,
    /// The pulse curve's level now; 1 for other effects.
    level: f64,
    seed: u64,
    /// For one-by-one: each fixture's then each grid's turn, from 0, in order along the travel.
    turns: Vec<usize>,
}

impl Layer {
    fn at(
        &self,
        rig: &Rig,
        clock: &ClockFollower,
        started: Moment,
        at: Duration,
        seed: u64,
    ) -> LayerFrame<'_> {
        let cycles = clock.cycles(self.speed, started, at) + self.phase;
        let within = cycles.rem_euclid(1.0);
        let position = match self.effect {
            Effect::Static => (REST_POSITION + self.phase).rem_euclid(1.0),
            Effect::Move => within,
            Effect::Chase => {
                let steps = (1.0 / self.width).round().max(1.0);
                ((within * steps).floor() + 0.5) / steps
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
            turns: self.turns(rig),
        }
    }

    /// Orders fixtures then grids by where their pixels sit, on average, along the travel.
    fn turns(&self, rig: &Rig) -> Vec<usize> {
        if self.shape != Shape::OneByOne {
            return Vec::new();
        }
        let mean = |positions: Vec<f64>| positions.iter().sum::<f64>() / positions.len() as f64;
        let travel = |x: f32, y: f32| travel(self.direction, x, y);
        let centres: Vec<f64> = rig
            .fixtures
            .iter()
            .map(|fixture| mean(fixture.pixels.iter().map(|p| travel(p.x, p.y)).collect()))
            .chain(
                rig.grids
                    .iter()
                    .map(|grid| mean(grid.pixels.iter().map(|p| travel(p.x, p.y)).collect())),
            )
            .collect();
        let mut order: Vec<usize> = (0..centres.len()).collect();
        order.sort_by(|a, b| centres[*a].total_cmp(&centres[*b]));
        let mut turns = vec![0; centres.len()];
        for (turn, unit) in order.into_iter().enumerate() {
            turns[unit] = turn;
        }
        turns
    }
}

impl LayerFrame<'_> {
    /// How lit the pixel at (`x`, `y`) of fixture or grid `unit` is, 0–1.
    fn level(&self, x: f32, y: f32, unit: usize) -> f64 {
        let Layer {
            shape,
            effect,
            width,
            direction,
            ..
        } = *self.layer;
        let position = self.position;
        let reversed = matches!(direction, Direction::RightToLeft | Direction::TopToBottom);
        let reverse = |u: f64| if reversed { 1.0 - u } else { u };
        let band = |u: f64| f64::from(u8::from((u - position).abs() <= width / 2.0));
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
                let count = self.turns.len() as f64;
                let lit_turn = (position * count).floor();
                f64::from(u8::from(self.turns[unit] as f64 == lit_turn))
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

    fn colour(&self, x: f32, y: f32, unit: usize) -> Rgb {
        mix(
            self.layer.background,
            self.layer.color,
            self.level(x, y, unit) as f32,
        )
    }
}

impl Blend {
    fn apply(self, below: Rgb, above: &LayerFrame, x: f32, y: f32, unit: usize) -> Rgb {
        let combine = |combine: fn(f32, f32) -> f32, above: Rgb| Rgb {
            red: combine(below.red, above.red),
            green: combine(below.green, above.green),
            blue: combine(below.blue, above.blue),
        };
        match self {
            Blend::Max => combine(f32::max, above.colour(x, y, unit)),
            Blend::Add => combine(|a, b| (a + b).min(1.0), above.colour(x, y, unit)),
            Blend::Mask => mix(Rgb::default(), below, above.level(x, y, unit) as f32),
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

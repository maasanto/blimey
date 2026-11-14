//! Pixel scenes (plan §7, §16): a stack of layers, each a preset shape in an area of the 2D
//! canvas, animated by an effect, evaluated at every pixel of every fixture and grid each frame
//! and blended over the layers below it.
//!
//! A frame is a pure function of the cue's start, the time and the beats since then: effects
//! count cycles of their `speed` from the note, and sparkle draws from a hash of the layer's
//! seed, the pixel's position and the cycle, never from running state.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::clock_follower::{ClockFollower, Moment};
use crate::colour::{Rgb, RgbwMode, to_dmx};
use crate::cue::{CueEntry, CueTiming, SwellTiming};
use crate::duration::{Length, TimeSignature};
use crate::envelope::Envelope;
use crate::look::{Level, colour, draw_pixel};
use crate::pattern::{Loop, LoopEntry};
use crate::rig::{Canvas, Fixture, Rig, line_key};
use crate::{ArtNetFrames, DMX_CHANNELS, Show, Universes};

/// Layers a pixel cue may stack (plan §16), so 8 universes of grids still render in 5 ms.
const MAX_LAYERS: usize = 8;
const WHOLE_CANVAS: Canvas = Canvas {
    x: 0.0,
    y: 0.0,
    width: 1.0,
    height: 1.0,
};

/// Where the shape sits, 0–1 along its travel, for effects that don't move it.
const REST_POSITION: f64 = 0.5;
/// A sparkle flash lasts this fraction of a cycle, fading linearly.
// ponytail: fixed flash length; make it a param if the preview sheet asks for denser sparkle.
const FLASH: f64 = 0.25;
/// Frames often land exactly on a chase step, on the beat; time and beat arithmetic can put
/// them a hair before it. This keeps them on the step they land on.
const STEP_ROUNDING: f64 = 1e-9;
/// Mixed into the cue's seed once per layer up the stack, so no layer's sparkle mirrors
/// another's. Layer 2 gets the seed two-layer cues have always given it, so they render the same.
const LAYER_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
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
    /// The whole area.
    Fill,
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

/// How a layer meets the layers below it, per pixel of its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blend {
    /// The brighter of the two, per emitter.
    #[default]
    Max,
    /// The sum, clipped at full.
    Add,
    /// The layers below show only where this layer's shape is lit, as much as it is.
    Mask,
    /// This layer covers the ones below as much as it is lit; with a background, fully.
    Over,
}

/// A pixel cue as written in a song file (docs/api.md): `layers`, or the older form of one
/// layer at the top of the cue and an optional `layer_2`.
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
    shape: Option<Shape>,
    effect: Option<Effect>,
    params: Option<ParamsEntry>,
    layer_2: Option<SecondLayerEntry>,
    layers: Option<Vec<LayerEntry>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerEntry {
    area: Option<Canvas>,
    #[serde(default)]
    shape: Shape,
    #[serde(default)]
    effect: Effect,
    #[serde(default)]
    params: ParamsEntry,
    level: Option<Level>,
    #[serde(default)]
    blend: Blend,
    #[serde(default)]
    mute: Vec<String>,
    swell: Option<SwellTiming>,
    pattern: Option<LoopEntry>,
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
    /// Bottom first, 1 to `MAX_LAYERS`.
    pub layers: Vec<Layer>,
    /// On the rig the scene was resolved against.
    drawn: Drawn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// The part of the canvas the layer draws on. The shape runs in the area's own 0–1 space.
    pub area: Canvas,
    pub shape: Shape,
    pub effect: Effect,
    pub color: Rgb,
    /// Where the shape isn't lit; without one, black, and an `over` layer is see-through there.
    pub background: Option<Rgb>,
    /// 0–1 of the area: a bar's or ring's thickness, and a chase's step.
    pub width: f64,
    /// One cycle of the effect.
    pub speed: Length,
    pub direction: Direction,
    /// Offset in cycles: shifts a moving effect in time, a static shape along its travel.
    pub phase: f64,
    /// A pulse's level over one cycle.
    pub curve: Envelope,
    pub blend: Blend,
    /// 0–1, how much the layer shows: it dims a `max` or `add` layer, as a scene's dimmer does,
    /// and fades an `over` or `mask` layer into the ones below. At 0 the layer changes nothing.
    pub level: f32,
    /// The fixtures and grids the layer leaves to the layers below, groups expanded.
    pub mute: BTreeSet<String>,
    /// Scales the layer as `level` does.
    pub rhythm: Option<Rhythm>,
}

/// A layer's own swell or pattern, timed as the cue types of the same name (plan §16).
#[derive(Debug, Clone, PartialEq)]
pub enum Rhythm {
    Swell(SwellTiming),
    /// Hits on a loop from the bar line after the note, dark between them.
    Pattern(Loop),
}

impl PixelEntry {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn resolve(
        self,
        show: &Show,
        time_signature: TimeSignature,
    ) -> Result<PixelScene, String> {
        let has_top_layer = self.shape.is_some()
            || self.effect.is_some()
            || self.params.is_some()
            || self.layer_2.is_some();
        let layers = match self.layers {
            Some(_) if has_top_layer => {
                return Err(
                    "a pixel cue has either layers or shape, effect, params and layer_2, not both"
                        .into(),
                );
            }
            Some(entries) => LayerEntry::resolve_all(entries, show, time_signature)?,
            None => {
                let layer_1 = LayerEntry {
                    shape: self.shape.unwrap_or_default(),
                    effect: self.effect.unwrap_or_default(),
                    params: self.params.unwrap_or_default(),
                    ..LayerEntry::default()
                };
                let mut layers = vec![layer_1.resolve(show, time_signature)?];
                if let Some(entry) = self.layer_2 {
                    layers.push(
                        entry
                            .resolve(show, time_signature)
                            .map_err(|error| format!("layer_2: {error}"))?,
                    );
                }
                layers
            }
        };
        Ok(PixelScene {
            drawn: Drawn::new(&layers, &show.rig),
            layers,
            name: self.name,
            fade_in: self.fade_in.unwrap_or(Length::Time(Duration::ZERO)),
            release: self.release.unwrap_or(Length::Time(Duration::ZERO)),
            seed: self.seed,
            rgbw: self.rgbw,
        })
    }
}

impl SecondLayerEntry {
    fn resolve(self, show: &Show, time_signature: TimeSignature) -> Result<Layer, String> {
        LayerEntry {
            shape: self.shape,
            effect: self.effect,
            params: self.params,
            blend: self.blend,
            ..LayerEntry::default()
        }
        .resolve(show, time_signature)
    }
}

impl LayerEntry {
    fn resolve_all(
        entries: Vec<LayerEntry>,
        show: &Show,
        time_signature: TimeSignature,
    ) -> Result<Vec<Layer>, String> {
        if !(1..=MAX_LAYERS).contains(&entries.len()) {
            return Err(format!(
                "{} layers; a pixel cue has 1 to {MAX_LAYERS}",
                entries.len()
            ));
        }
        entries
            .into_iter()
            .zip(1..)
            .map(|(entry, number)| {
                entry
                    .resolve(show, time_signature)
                    .map_err(|error| format!("layer {number}: {error}"))
            })
            .collect()
    }

    fn resolve(self, show: &Show, time_signature: TimeSignature) -> Result<Layer, String> {
        let area = self.area.unwrap_or(WHOLE_CANVAS);
        check_area(area)?;
        let rhythm = match (self.swell, self.pattern) {
            (Some(_), Some(_)) => return Err("a layer has a swell or a pattern, not both".into()),
            (Some(swell), None) => Some(Rhythm::Swell(swell)),
            (None, Some(pattern)) => Some(Rhythm::Pattern(
                pattern
                    .resolve(time_signature)
                    .map_err(|error| format!("pattern: {error}"))?,
            )),
            (None, None) => None,
        };
        let mut mute = BTreeSet::new();
        for name in &self.mute {
            mute.extend(muted(&show.rig, name)?);
        }
        let params = self.params;
        let width = params.width.unwrap_or(1.0);
        if !(width > 0.0 && width <= 1.0) {
            return Err(format!("width {width} must be above 0 and at most 1"));
        }
        let speed = params.speed.unwrap_or(Length::Bars(1.0));
        let is_zero = match speed {
            Length::Time(time) => time.is_zero(),
            Length::Beats(count) | Length::Bars(count) => count == 0.0,
        };
        if is_zero {
            return Err("speed must be above 0".into());
        }
        Ok(Layer {
            area,
            shape: self.shape,
            effect: self.effect,
            color: params.color.map_or(Ok(WHITE), |name| colour(show, &name))?,
            background: params
                .background
                .map(|name| colour(show, &name))
                .transpose()?,
            width,
            speed,
            direction: params.direction.unwrap_or_default(),
            phase: params.phase.unwrap_or(0.0),
            curve: params.curve.unwrap_or(Envelope::Sine),
            blend: self.blend,
            level: self.level.map_or(1.0, |level| level.0),
            mute,
            rhythm,
        })
    }
}

impl Rhythm {
    /// The level, 0–1, `now` of a cue played with `timing`.
    fn level(&self, clock: &ClockFollower, timing: CueTiming, now: Moment) -> f64 {
        match self {
            Rhythm::Swell(swell) => swell.level(clock, timing.started, now),
            // Until its loop starts, the layer shows as it is, as a pattern linked to the cue's
            // intensity leaves the cue. A cue fired before a save gave it a pattern has no bar
            // line: its loop starts at the next note-on.
            Rhythm::Pattern(hits) => timing
                .bar_line
                .and_then(|bar_line| hits.level(bar_line, now))
                .unwrap_or(1.0),
        }
    }
}

fn check_area(area: Canvas) -> Result<(), String> {
    let Canvas { width, height, .. } = area;
    if !(width > 0.0 && height > 0.0) {
        return Err(format!(
            "area: width {width} and height {height} must both be above 0"
        ));
    }
    area.check().map_err(|error| format!("area: {error}"))
}

/// The fixtures and grids `name` stands for in a `mute` list: itself, or a group's fixtures.
fn muted(rig: &Rig, name: &str) -> Result<Vec<String>, String> {
    if rig.grids.iter().any(|grid| grid.name == name) {
        return Ok(vec![name.to_owned()]);
    }
    let fixtures = rig
        .resolve(name)
        .map_err(|error| format!("mute: {error}"))?;
    Ok(fixtures
        .into_iter()
        .map(|fixture| fixture.name.clone())
        .collect())
}

impl PixelScene {
    pub(crate) fn has_pattern_layer(&self) -> bool {
        (self.layers.iter()).any(|layer| matches!(layer.rhythm, Some(Rhythm::Pattern(_))))
    }

    /// A pixel cue as in a song file, `type: pixel` included, its colours from `show` and its
    /// patterns' bars from `time_signature`.
    pub fn parse(
        show: &Show,
        time_signature: TimeSignature,
        cue: serde_json::Value,
    ) -> Result<PixelScene, String> {
        match CueEntry::deserialize(cue).map_err(|error| error.to_string())? {
            CueEntry::Pixel(entry) => (*entry).resolve(show, time_signature),
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
        timing: CueTiming,
        at: Duration,
        intensity: f32,
        out: &mut Universes,
    ) {
        let (cue_rhythm, rhythms) = self.rhythms(clock, timing, at);
        let intensity = intensity * cue_rhythm;
        let drawn = self.drawn_on(rig);
        let frames: [Option<LayerFrame>; MAX_LAYERS] = std::array::from_fn(|index| {
            let seed = self.seed ^ LAYER_SEED.wrapping_mul(index as u64);
            let layer = self.layers.get(index)?;
            Some(layer.at(
                &drawn.lines[index],
                clock,
                timing.started,
                at,
                seed,
                rhythms[index],
            ))
        });
        self.draw_fixtures(rig, &drawn, &frames, intensity, out);
        self.draw_grids(rig, &drawn, &frames, intensity, out);
    }

    /// The layers' rhythms at `at`, and the part of them that goes on the cue's intensity.
    fn rhythms(
        &self,
        clock: &ClockFollower,
        timing: CueTiming,
        at: Duration,
    ) -> (f32, [f32; MAX_LAYERS]) {
        let now = timing.released.unwrap_or_else(|| clock.moment(at));
        let mut rhythms: [f32; MAX_LAYERS] = std::array::from_fn(|index| {
            let rhythm = self
                .layers
                .get(index)
                .and_then(|layer| layer.rhythm.as_ref());
            rhythm.map_or(1.0, |rhythm| rhythm.level(clock, timing, now) as f32)
        });
        // A lone layer's rhythm goes on the cue's intensity, so a fixture with a dimmer channel
        // follows it there, as it would a swell or pattern linked to the cue's intensity.
        let cue_rhythm = match self.layers.len() {
            1 => std::mem::replace(&mut rhythms[0], 1.0),
            _ => 1.0,
        };
        (cue_rhythm, rhythms)
    }

    /// What the layers draw on `rig`: worked out at load for the show's rig, and here for the
    /// others the builder previews and thumbnails a scene on.
    fn drawn_on(&self, rig: &Rig) -> Cow<'_, Drawn> {
        if self.drawn.rig == *rig {
            Cow::Borrowed(&self.drawn)
        } else {
            Cow::Owned(Drawn::new(&self.layers, rig))
        }
    }

    /// Plan §5: a pixel cue overrides only the channels it touches, its fixtures' emitters and
    /// dimmer. Fixtures without emitters, pixels no layer draws, and other channels keep the
    /// layers below.
    fn draw_fixtures(
        &self,
        rig: &Rig,
        drawn: &Drawn,
        frames: &[Option<LayerFrame>],
        intensity: f32,
        out: &mut Universes,
    ) {
        let fixtures = rig.fixtures.iter().zip(&drawn.fixtures);
        for (fixture, layers) in fixtures.filter(|(fixture, layers)| {
            has_emitters(fixture) && layers.iter().any(|&layers| layers != 0)
        }) {
            let start = fixture.address - 1;
            let colour_scale = match fixture.dimmer {
                Some(offset) => {
                    out.enttec[start + offset] = to_dmx(intensity);
                    1.0
                }
                None => intensity,
            };
            let centre = fixture.centre();
            for (pixel, &layers) in fixture.pixels.iter().zip(layers) {
                let Some(lit) = stack(frames, layers, pixel.x, pixel.y, centre) else {
                    continue;
                };
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
    }

    fn draw_grids(
        &self,
        rig: &Rig,
        drawn: &Drawn,
        frames: &[Option<LayerFrame>],
        intensity: f32,
        out: &mut Universes,
    ) {
        let grid_pixels = rig
            .grids
            .iter()
            .zip(&drawn.grids)
            .flat_map(|(grid, layers)| grid.pixels.iter().zip(layers));
        for (pixel, &layers) in grid_pixels {
            let Some(lit) = stack(frames, layers, pixel.x, pixel.y, (pixel.x, pixel.y)) else {
                continue;
            };
            let frame = out.frame_mut(pixel.universe);
            draw_pixel(pixel.channels, 0, lit, self.rgbw, intensity, frame);
        }
    }
}

/// The colour at (`x`, `y`) in one-by-one `cell`: each of `layers` (a bit per layer, from the
/// bottom) over the ones below it, from black. `None` when no layer draws there.
fn stack(frames: &[Option<LayerFrame>], layers: u8, x: f32, y: f32, cell: Cell) -> Option<Rgb> {
    if layers == 0 {
        return None;
    }
    let drawing = frames
        .iter()
        .enumerate()
        .filter(|(index, _)| layers & 1 << index != 0)
        .map(|(_, frame)| frame.as_ref().expect("a layer for each bit"));
    Some(drawing.fold(Rgb::default(), |below, layer| layer.over(below, x, y, cell)))
}

/// What each layer draws on one rig, worked out once rather than every frame.
#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    rig: Rig,
    /// Per fixture, then per grid, in rig order, and per pixel: the layers that draw it, a bit
    /// each from the bottom one.
    fixtures: Vec<Vec<u8>>,
    grids: Vec<Vec<u8>>,
    /// Per layer: its one-by-one lines, empty for other shapes.
    lines: Vec<Vec<i32>>,
}

impl Drawn {
    fn new(layers: &[Layer], rig: &Rig) -> Drawn {
        let drawing = |name: &str, x: f32, y: f32| {
            layers
                .iter()
                .zip(0..)
                .filter(|(layer, _)| !layer.mute.contains(name) && layer.catches(x, y))
                .fold(0u8, |bits, (_, index)| bits | 1 << index)
        };
        Drawn {
            fixtures: (rig.fixtures.iter())
                .map(|fixture| {
                    let pixels = fixture.pixels.iter();
                    pixels
                        .map(|pixel| drawing(&fixture.name, pixel.x, pixel.y))
                        .collect()
                })
                .collect(),
            grids: (rig.grids.iter())
                .map(|grid| {
                    let pixels = grid.pixels.iter();
                    pixels
                        .map(|pixel| drawing(&grid.name, pixel.x, pixel.y))
                        .collect()
                })
                .collect(),
            lines: (layers.iter())
                .map(|layer| match layer.shape {
                    Shape::OneByOne => layer.lines(rig),
                    _ => Vec::new(),
                })
                .collect(),
            rig: rig.clone(),
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
/// over one `cycle`, each the grid's colours
/// row by row from the top left as the audience sees it, at full intensity. `show` gives the
/// palettes, `time_signature` the song's bars.
pub fn thumbnail(
    show: &Show,
    time_signature: TimeSignature,
    cue: serde_json::Value,
) -> Result<Vec<Vec<Rgb>>, String> {
    let scene = PixelScene::parse(show, time_signature, cue)?;
    let rig = Rig::parse(
        &format!(
            "fixtures:\n  - {{ name: sample, type: pixel-grid, columns: {THUMBNAIL_SIDE}, rows: {THUMBNAIL_SIDE}, colour_order: RGB, address: 1, canvas: {{ x: 0, y: 0, width: 1, height: 1 }} }}\n"
        ),
        Path::new("."),
    )
    .expect("the sample grid fits the Enttec universe");
    let frames = cycle(&scene, &rig, time_signature)?.into_iter().map(|out| {
        let (_, colours) = rig.pixel_colours(&out.enttec, &out.artnet).remove(0);
        colours
    });
    Ok(frames.collect())
}

/// `scene` drawn on `rig` at full intensity, `THUMBNAIL_FRAMES` frames evenly over the longest
/// of its moving layers' cycles and its layers' swells and pattern loops, 1 bar without any,
/// beats running at `THUMBNAIL_BPM` in `time_signature`.
pub fn cycle(
    scene: &PixelScene,
    rig: &Rig,
    time_signature: TimeSignature,
) -> Result<Vec<Universes>, String> {
    let clock = ClockFollower::new(THUMBNAIL_BPM, time_signature);
    let started = clock.moment(Duration::ZERO);
    let second = Duration::from_secs(1);
    let cycles_in_a_second = |speed| clock.cycles(speed, started, second);
    let lengths = scene.layers.iter().flat_map(|layer| {
        let speed = (layer.effect != Effect::Static).then_some(layer.speed);
        let rhythm = layer.rhythm.as_ref().map(|rhythm| match rhythm {
            Rhythm::Swell(swell) => swell.duration,
            Rhythm::Pattern(hits) => Length::Beats(hits.beats),
        });
        speed.into_iter().chain(rhythm)
    });
    let cycles_per_second = lengths
        .map(cycles_in_a_second)
        .min_by(f64::total_cmp)
        .unwrap_or_else(|| cycles_in_a_second(Length::Bars(1.0)));
    let cycle = Duration::try_from_secs_f64(1.0 / cycles_per_second)
        .map_err(|_| "speed is too long to render a cycle of".to_string())?;
    let timing = CueTiming {
        started,
        bar_line: Some(started),
        released: None,
    };
    let frames = (0..THUMBNAIL_FRAMES).map(|index| {
        let mut out = Universes {
            enttec: [0; DMX_CHANNELS],
            artnet: ArtNetFrames::new(),
        };
        let at = cycle.mul_f64(index as f64 / THUMBNAIL_FRAMES as f64);
        scene.draw(rig, &clock, timing, at, 1.0, &mut out);
        out
    });
    Ok(frames.collect())
}

/// Whether `fixture` has colour emitters, the only channels a pixel cue lights besides a dimmer.
pub fn has_emitters(fixture: &Fixture) -> bool {
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
    /// The layer's rhythm now, 1 without one.
    rhythm: f32,
    seed: u64,
    /// For one-by-one: the lines across the direction of the cells the layer catches,
    /// ascending in its area.
    lines: &'frame [i32],
}

impl Layer {
    fn at<'frame>(
        &'frame self,
        lines: &'frame [i32],
        clock: &ClockFollower,
        started: Moment,
        at: Duration,
        seed: u64,
        rhythm: f32,
    ) -> LayerFrame<'frame> {
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
            rhythm,
            seed,
            lines,
        }
    }

    /// `width` as a fraction of the shape's travel. A split's bars travel half the area, from
    /// the centre to an edge, so each is `width` of the area wide.
    fn travel_width(&self) -> f64 {
        match self.shape {
            Shape::Split => (2.0 * self.width).min(1.0),
            _ => self.width,
        }
    }

    /// Whether the area holds the canvas position (`x`, `y`). Its far edges are open, so a
    /// position on an edge two areas share is in one of them only, unless the edge is the
    /// canvas's own.
    fn catches(&self, x: f32, y: f32) -> bool {
        let within = |position: f32, start: f32, length: f32| {
            let end = start + length;
            position >= start && (position < end || (end >= 1.0 && position <= end))
        };
        within(x, self.area.x, self.area.width) && within(y, self.area.y, self.area.height)
    }

    /// A canvas position in the area's own 0–1 space.
    fn in_area(&self, (x, y): Cell) -> Cell {
        (
            (x - self.area.x) / self.area.width,
            (y - self.area.y) / self.area.height,
        )
    }

    /// One-by-one's key for `cell`, in the area's space, across the direction.
    fn line_key(&self, cell: Cell) -> i32 {
        let (x, y) = self.in_area(cell);
        match self.direction {
            Direction::LeftToRight | Direction::RightToLeft => line_key(x),
            Direction::BottomToTop | Direction::TopToBottom => line_key(y),
        }
    }

    /// One-by-one's lines across the direction, as `line_key`s, distinct and ascending: those
    /// of the cells with a pixel the layer catches and draws, muted fixtures and fixtures
    /// without colour emitters left out.
    fn lines(&self, rig: &Rig) -> Vec<i32> {
        let fixtures = rig
            .fixtures
            .iter()
            .filter(|fixture| has_emitters(fixture) && !self.mute.contains(&fixture.name))
            .flat_map(|fixture| {
                let centre = fixture.centre();
                fixture
                    .pixels
                    .iter()
                    .map(move |pixel| (pixel.x, pixel.y, centre))
            });
        let grids = rig
            .grids
            .iter()
            .filter(|grid| !self.mute.contains(&grid.name))
            .flat_map(|grid| &grid.pixels)
            .map(|pixel| (pixel.x, pixel.y, (pixel.x, pixel.y)));
        let keys: BTreeSet<i32> = fixtures
            .chain(grids)
            .filter(|&(x, y, _)| self.catches(x, y))
            .map(|(_, _, cell)| self.line_key(cell))
            .collect();
        keys.into_iter().collect()
    }
}

impl LayerFrame<'_> {
    /// One-by-one's turn for `cell`, from 0, in order along the travel.
    fn turn(&self, cell: Cell) -> usize {
        let key = self.layer.line_key(cell);
        let from_start = self
            .lines
            .binary_search(&key)
            .expect("the lines hold every cell the layer catches");
        match self.layer.direction {
            Direction::LeftToRight | Direction::BottomToTop => from_start,
            Direction::RightToLeft | Direction::TopToBottom => self.lines.len() - 1 - from_start,
        }
    }

    /// How lit the pixel at (`x`, `y`), in the area's space, in one-by-one `cell` is, 0–1.
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
            Shape::Fill => 1.0,
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

    /// The layer at the canvas position (`x`, `y`), which its area catches, in one-by-one
    /// `cell`, blended over `below`.
    fn over(&self, below: Rgb, x: f32, y: f32, cell: Cell) -> Rgb {
        let (x, y) = self.layer.in_area((x, y));
        self.layer.blend.apply(
            below,
            self.layer,
            self.layer.level * self.rhythm,
            self.level(x, y, cell) as f32,
        )
    }
}

impl Blend {
    /// `layer` at `level` (0–1) over `below` where its shape is `lit` (0–1). At a `level` of 0
    /// it leaves `below` as it is, whatever the blend.
    fn apply(self, below: Rgb, layer: &Layer, level: f32, lit: f32) -> Rgb {
        let combine = |combine: fn(f32, f32) -> f32, above: Rgb| Rgb {
            red: combine(below.red, above.red),
            green: combine(below.green, above.green),
            blue: combine(below.blue, above.blue),
        };
        let colour = mix(layer.background.unwrap_or_default(), layer.color, lit);
        match self {
            Blend::Max => combine(f32::max, mix(Rgb::default(), colour, level)),
            Blend::Add => combine(|a, b| (a + b).min(1.0), mix(Rgb::default(), colour, level)),
            Blend::Mask => {
                let let_through = lit * level + (1.0 - level);
                mix(Rgb::default(), below, let_through)
            }
            Blend::Over => match layer.background {
                Some(_) => mix(below, colour, level),
                None => mix(below, layer.color, lit * level),
            },
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

    #[test]
    fn a_thumbnail_spans_one_cycle_of_the_slowest_moving_layer() {
        let cue = serde_json::json!({ "name": "t", "type": "pixel", "layers": [
            { "shape": "fill", "params": { "color": "#000000", "speed": "1 beat" } },
            { "effect": "chase", "params": { "color": "#00ff00", "width": 0.25, "speed": "1 bar" } },
            { "effect": "chase", "params": { "color": "#ff0000", "width": 0.25, "speed": "2 bars" } },
        ] });

        let frames = thumbnail(&Show::default(), TimeSignature::default(), cue).unwrap();

        // The red 2-bar chase's first lit column on the top row: four steps over the 8 frames.
        let first_lit: Vec<Option<usize>> = frames
            .iter()
            .map(|frame| {
                frame[..THUMBNAIL_SIDE]
                    .iter()
                    .position(|colour| colour.red == 1.0)
            })
            .collect();
        assert_eq!(
            first_lit,
            [0, 0, 3, 3, 6, 6, 9, 9].map(Some),
            "the 1-bar chase leads twice as fast, the static fill doesn't count"
        );
    }

    #[test]
    fn a_thumbnail_spans_the_layers_swells_and_pattern_loops_too() {
        // A red layer with `rhythm`, under a 1-bar loop that never lights.
        let red_over_frames = |rhythm: (&str, serde_json::Value)| {
            let mut red = serde_json::json!({ "shape": "fill", "params": { "color": "#ff0000" } });
            red[rhythm.0] = rhythm.1;
            let silent = serde_json::json!({ "shape": "fill", "blend": "add", "pattern": { "length": "1 bar", "hits": [] } });
            let cue = serde_json::json!({ "name": "t", "type": "pixel", "layers": [red, silent] });
            let frames = thumbnail(&Show::default(), TimeSignature::default(), cue).unwrap();
            frames
                .iter()
                .map(|frame| frame[0].red)
                .collect::<Vec<f32>>()
        };

        let swell = red_over_frames((
            "swell",
            serde_json::json!({ "duration": "4 bars", "curve": "ramp-up" }),
        ));
        assert_eq!(swell[0], 0.0);
        assert!(
            swell.is_sorted() && swell[7] > 0.85,
            "up its 4 bars, not 1: {swell:?}"
        );

        let pattern = red_over_frames((
            "pattern",
            serde_json::json!({ "length": "2 bars", "hits": [{ "at": 5, "shape": "hold", "length": "4 beats", "level": "100%" }] }),
        ));
        assert_eq!(
            pattern,
            [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            "the hit in bar 2 shows"
        );
    }

    /// The builder's Hits ingredient, half-beat punches on every beat of a bar, lone on its cue:
    /// the frames on the beat catch each punch at its peak.
    #[test]
    fn a_thumbnail_of_short_hits_shows_them_lit() {
        let punch = |at: u8| serde_json::json!({ "at": at, "shape": "ramp-down", "length": "0.5 beats", "level": "100%" });
        let hits = [1, 2, 3, 4].map(punch);
        let cue = serde_json::json!({ "name": "t", "type": "pixel", "layers": [{
            "shape": "fill", "params": { "color": "#ff0000" }, "blend": "over",
            "pattern": { "length": "1 bar", "hits": hits },
        }] });

        let frames = thumbnail(&Show::default(), TimeSignature::default(), cue).unwrap();

        let red: Vec<f32> = frames.iter().map(|frame| frame[0].red).collect();
        assert_eq!(
            red,
            [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            "on each beat, dark half a beat after"
        );
    }

    #[test]
    fn each_blend_meets_the_layer_below_as_documented() {
        let rgb = |red, green, blue| Rgb { red, green, blue };
        let below = rgb(0.25, 0.5, 1.0);
        // A red layer, at `lit` of its shape.
        let cases = [
            ("max", None, "100%", 0.5, rgb(0.5, 0.5, 1.0)),
            ("max", None, "50%", 1.0, rgb(0.5, 0.5, 1.0)),
            ("add", None, "100%", 0.5, rgb(0.75, 0.5, 1.0)),
            ("add", None, "100%", 1.0, rgb(1.0, 0.5, 1.0)),
            ("mask", None, "100%", 0.5, rgb(0.125, 0.25, 0.5)),
            ("mask", None, "50%", 0.0, rgb(0.125, 0.25, 0.5)),
            ("mask", None, "50%", 1.0, below),
            ("over", None, "100%", 1.0, rgb(1.0, 0.0, 0.0)),
            ("over", None, "100%", 0.5, rgb(0.625, 0.25, 0.5)),
            ("over", None, "100%", 0.0, below),
            ("over", None, "50%", 1.0, rgb(0.625, 0.25, 0.5)),
            ("over", Some("#0000ff"), "100%", 0.5, rgb(0.5, 0.0, 0.5)),
            ("over", Some("#000000"), "100%", 0.0, rgb(0.0, 0.0, 0.0)),
            ("over", Some("#000000"), "50%", 0.0, rgb(0.125, 0.25, 0.5)),
            // At 0% a layer vanishes, so a swell from 0 fades it in.
            ("max", None, "0%", 1.0, below),
            ("add", None, "0%", 1.0, below),
            ("mask", None, "0%", 0.0, below),
            ("over", None, "0%", 1.0, below),
            ("over", Some("#000000"), "0%", 0.0, below),
        ];
        for (blend, background, level, lit, expected) in cases {
            let cue = serde_json::json!({ "name": "t", "type": "pixel", "layers": [{
                "blend": blend,
                "level": level,
                "params": { "color": "#ff0000", "background": background },
            }] });
            let scene = PixelScene::parse(&Show::default(), TimeSignature::default(), cue).unwrap();
            let layer = &scene.layers[0];

            assert_eq!(
                layer.blend.apply(below, layer, layer.level, lit),
                expected,
                "{blend}, background {background:?}, level {level}, lit {lit}"
            );
        }
    }
}

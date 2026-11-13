//! The rig: which fixtures hang where, at which DMX address, in which groups.
//! Parsed from a show's `rig.yaml`, with profiles from its `fixtures/` folder.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

use crate::colour::Rgb;
use crate::fixture::{Channel, FixtureProfile, PixelChannels};
use crate::{ArtNetFrames, DMX_CHANNELS, Frame, Universe};

/// Art-Net port-addresses are 15 bits.
pub const MAX_ARTNET_UNIVERSE: u16 = 0x7FFF;

/// `rig.yaml` as written. Field order here is the canonical order in the file. It is read with
/// each fixture as plain YAML, to tell profiled fixtures from pixel grids by `type` (see
/// `Entry::read`), and written with typed entries.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RigFile<Fixture = Value> {
    fixtures: Vec<Fixture>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    groups: BTreeMap<String, Vec<String>>,
}

/// One `fixtures` entry.
#[derive(Serialize)]
#[serde(untagged)]
enum Entry {
    Profiled(FixtureEntry),
    Grid(GridEntry),
}

impl Entry {
    /// Errors name the fixture, or its 1-based position without a name.
    fn read(index: usize, entry: Value) -> Result<Entry, String> {
        let name = match entry.get("name").and_then(Value::as_str) {
            Some(name) => format!("{name:?}"),
            None => format!("{}", index + 1),
        };
        let is_grid = entry.get("type").is_some();
        let entry = if is_grid {
            serde_yaml_ng::from_value(entry).map(Entry::Grid)
        } else {
            serde_yaml_ng::from_value(entry).map(Entry::Profiled)
        };
        entry.map_err(|error| format!("fixture {name}: {error}"))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FixtureEntry {
    name: String,
    /// File name, without `.yaml`, of the profile in the show's `fixtures/` folder.
    profile: String,
    mode: String,
    /// 1-based DMX start address.
    address: usize,
    canvas: Canvas,
}

/// A generic RGB or RGBW pixel grid (plan §7): no profile, its layout is in the entry.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GridEntry {
    name: String,
    #[serde(rename = "type")]
    _type: GridType,
    columns: usize,
    rows: usize,
    colour_order: ColourOrder,
    #[serde(default)]
    wiring: Wiring,
    #[serde(default)]
    start_corner: StartCorner,
    #[serde(default)]
    output: Output,
    /// Art-Net port-address of the first pixel; only for `output: artnet`.
    #[serde(skip_serializing_if = "Option::is_none")]
    universe: Option<u16>,
    /// 1-based DMX start address in that universe.
    address: usize,
    canvas: Canvas,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum GridType {
    PixelGrid,
}

/// Where each emitter sits within a pixel's channels: `RGB`, `GRB`, `RGBW`, `WRGB`…
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
struct ColourOrder {
    offsets: PixelChannels,
    channels_per_pixel: usize,
}

impl TryFrom<String> for ColourOrder {
    type Error = String;

    fn try_from(text: String) -> Result<ColourOrder, String> {
        let invalid = || {
            format!(
                "colour order {text:?} must hold R, G and B, and optionally W, once each, e.g. RGB, GRB or RGBW"
            )
        };
        let mut offsets = PixelChannels::default();
        for (offset, letter) in text.chars().enumerate() {
            let slot = match letter {
                'R' => &mut offsets.red,
                'G' => &mut offsets.green,
                'B' => &mut offsets.blue,
                'W' => &mut offsets.white,
                _ => return Err(invalid()),
            };
            if slot.replace(offset).is_some() {
                return Err(invalid());
            }
        }
        if offsets.red.is_none() || offsets.green.is_none() || offsets.blue.is_none() {
            return Err(invalid());
        }
        Ok(ColourOrder {
            offsets,
            channels_per_pixel: text.len(),
        })
    }
}

impl From<ColourOrder> for String {
    fn from(order: ColourOrder) -> String {
        let PixelChannels {
            red,
            green,
            blue,
            white,
        } = order.offsets;
        let mut letters: Vec<(usize, char)> = [(red, 'R'), (green, 'G'), (blue, 'B'), (white, 'W')]
            .into_iter()
            .filter_map(|(offset, letter)| Some((offset?, letter)))
            .collect();
        letters.sort();
        letters.into_iter().map(|(_, letter)| letter).collect()
    }
}

impl ColourOrder {
    /// The emitters of a pixel whose first channel is `first` (0-based).
    fn at(&self, first: usize) -> PixelChannels {
        let shift = |offset: Option<usize>| offset.map(|offset| first + offset);
        PixelChannels {
            red: shift(self.offsets.red),
            green: shift(self.offsets.green),
            blue: shift(self.offsets.blue),
            white: shift(self.offsets.white),
        }
    }
}

/// How the pixel chain runs along the rows from the start corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Wiring {
    /// Every row starts on the start corner's side.
    #[default]
    RowByRow,
    /// Each row runs back the way the previous one came.
    Serpentine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum StartCorner {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Output {
    #[default]
    Enttec,
    Artnet,
}

/// Where a fixture sits on the normalised 2D stage canvas, a front view from the audience:
/// `x` from the left edge (0, stage right) to the right (1, stage left), `y` from the floor (0) up (1). Its pixels are
/// spread evenly across `width`, in pixel order, starting at `x`. A pixel grid's pixels fill
/// the rectangle `width` wide and `height` tall whose bottom-left corner is (`x`, `y`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub x: f32,
    pub y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: f32,
}

fn is_zero(width: &f32) -> bool {
    *width == 0.0
}

/// `rig.yaml` content, given as its JSON equivalent, in the engine's canonical YAML form.
pub fn canonical_yaml(content: serde_json::Value) -> Result<String, String> {
    let file: RigFile = serde_json::from_value(content).map_err(|error| error.to_string())?;
    let fixtures = file
        .fixtures
        .into_iter()
        .enumerate()
        .map(|(index, entry)| Entry::read(index, entry))
        .collect::<Result<Vec<Entry>, String>>()?;
    let file = RigFile {
        fixtures,
        groups: file.groups,
    };
    serde_yaml_ng::to_string(&file).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rig {
    pub fixtures: Vec<Fixture>,
    pub grids: Vec<PixelGrid>,
    pub groups: BTreeMap<String, Vec<String>>,
    /// The Art-Net universes the grids use, ascending.
    pub artnet_universes: Vec<u16>,
    /// Where fixtures (at their centre) and grid pixels sit across the canvas, as `line_key`s,
    /// distinct and ascending: the lines a pixel cue's one-by-one steps through.
    pub lines_x: Vec<i32>,
    /// The same up the canvas.
    pub lines_y: Vec<i32>,
}

/// Positions closer than this share a line, so grids hung at the same height or position
/// step together despite rounding.
const LINE_KEYS_PER_CANVAS: f32 = 10_000.0;

/// A canvas position (0–1) as a line, in 1/10000ths of the canvas.
pub fn line_key(position: f32) -> i32 {
    (position * LINE_KEYS_PER_CANVAS).round() as i32
}

/// A pixel grid or column, driven only by pixel cues.
#[derive(Debug, Clone, PartialEq)]
pub struct PixelGrid {
    pub name: String,
    /// In wiring order.
    pub pixels: Vec<GridPixel>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPixel {
    pub x: f32,
    pub y: f32,
    pub universe: Universe,
    /// 0-based channels within `universe`.
    pub channels: PixelChannels,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    pub name: String,
    pub profile: String,
    pub mode: String,
    /// 1-based DMX start address.
    pub address: usize,
    pub channels: Vec<Channel>,
    /// Offset of the master dimmer channel from `address`, if the mode has one.
    pub dimmer: Option<usize>,
    pub pixels: Vec<Pixel>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pixel {
    pub x: f32,
    pub y: f32,
    pub channels: PixelChannels,
}

impl Rig {
    /// Parses `rig.yaml`, loading each profile it names from `profiles_dir`.
    pub fn parse(yaml: &str, profiles_dir: &Path) -> Result<Rig, String> {
        let file: RigFile = serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())?;
        let mut profiles: BTreeMap<String, FixtureProfile> = BTreeMap::new();
        let mut rig = Rig {
            groups: file.groups,
            ..Rig::default()
        };
        for (index, entry) in file.fixtures.into_iter().enumerate() {
            match Entry::read(index, entry)? {
                Entry::Grid(grid) => {
                    let in_grid = |error: String| format!("fixture {:?}: {error}", grid.name);
                    rig.grids.push(PixelGrid::new(&grid).map_err(in_grid)?);
                }
                Entry::Profiled(entry) => {
                    let in_fixture = |error: String| format!("fixture {:?}: {error}", entry.name);
                    if !profiles.contains_key(&entry.profile) {
                        let path = profiles_dir.join(format!("{}.yaml", entry.profile));
                        profiles.insert(entry.profile.clone(), FixtureProfile::load(&path)?);
                    }
                    let profile = &profiles[&entry.profile];
                    rig.fixtures
                        .push(Fixture::new(&entry, profile).map_err(in_fixture)?);
                }
            }
        }
        let universes: BTreeSet<u16> = rig
            .grids
            .iter()
            .flat_map(|grid| &grid.pixels)
            .filter_map(|pixel| match pixel.universe {
                Universe::ArtNet(port_address) => Some(port_address),
                Universe::Enttec => None,
            })
            .collect();
        rig.artnet_universes = universes.into_iter().collect();
        let positions: Vec<(f32, f32)> = rig
            .fixtures
            .iter()
            .map(Fixture::centre)
            .chain(
                rig.grids
                    .iter()
                    .flat_map(|grid| &grid.pixels)
                    .map(|p| (p.x, p.y)),
            )
            .collect();
        let lines = |axis: fn(&(f32, f32)) -> f32| {
            let keys: BTreeSet<i32> = positions.iter().map(|p| line_key(axis(p))).collect();
            keys.into_iter().collect()
        };
        rig.lines_x = lines(|position| position.0);
        rig.lines_y = lines(|position| position.1);
        rig.validate()?;
        Ok(rig)
    }

    fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        let all_names = self
            .fixtures
            .iter()
            .map(|fixture| &fixture.name)
            .chain(self.grids.iter().map(|grid| &grid.name));
        if let Some(twice) = all_names.into_iter().find(|name| !names.insert(*name)) {
            return Err(format!("fixture {twice:?} is defined twice"));
        }
        let mut spans: Vec<Span> = self
            .fixtures
            .iter()
            .map(|fixture| Span {
                name: &fixture.name,
                universe: Universe::Enttec,
                first: fixture.address,
                last: fixture.last_address(),
            })
            .chain(self.grids.iter().flat_map(PixelGrid::spans))
            .collect();
        spans.sort_by_key(|span| (span.universe, span.first));
        for pair in spans.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            if second.universe == first.universe && second.first <= first.last {
                return Err(format!(
                    "fixtures {:?} ({first}) and {:?} ({second}) overlap",
                    first.name, second.name
                ));
            }
        }
        for (group, members) in &self.groups {
            if names.contains(group) {
                return Err(format!("group {group:?} has the same name as a fixture"));
            }
            if let Some(unknown) = members.iter().find(|name| self.fixture(name).is_none()) {
                return Err(format!("group {group:?} lists unknown fixture {unknown:?}"));
            }
        }
        Ok(())
    }

    pub fn fixture(&self, name: &str) -> Option<&Fixture> {
        self.fixtures.iter().find(|fixture| fixture.name == name)
    }

    /// The fixtures a look target names: one fixture, or every fixture in a group.
    pub fn resolve(&self, target: &str) -> Result<Vec<&Fixture>, String> {
        if let Some(fixture) = self.fixture(target) {
            return Ok(vec![fixture]);
        }
        let members = self
            .groups
            .get(target)
            .ok_or_else(|| format!("{target:?} is neither a fixture nor a group in the rig"))?;
        Ok(members
            .iter()
            .filter_map(|name| self.fixture(name))
            .collect())
    }

    /// A dark frame for every Art-Net universe the grids use, so each one goes out every
    /// frame, blackout included.
    pub fn artnet_frames(&self) -> ArtNetFrames {
        self.artnet_universes
            .iter()
            .map(|universe| (*universe, [0; DMX_CHANNELS]))
            .collect()
    }

    /// The colour each pixel of each fixture, then each grid, shows for a frame on the Enttec
    /// universe (`frame`) and the Art-Net ones (`artnet`, dark where missing), white and dimmer
    /// folded in, for previews.
    pub fn pixel_colours<'rig>(
        &'rig self,
        frame: &Frame,
        artnet: &ArtNetFrames,
    ) -> Vec<(&'rig str, Vec<Rgb>)> {
        let dark = [0; DMX_CHANNELS];
        let grid_colours = |grid: &'rig PixelGrid| {
            let colours = grid.pixels.iter().map(|pixel| {
                let universe = match pixel.universe {
                    Universe::Enttec => frame,
                    Universe::ArtNet(port_address) => artnet.get(&port_address).unwrap_or(&dark),
                };
                emitted(pixel.channels, 1.0, |channel| universe[channel])
            });
            (grid.name.as_str(), colours.collect())
        };
        self.fixtures
            .iter()
            .map(|fixture| (fixture.name.as_str(), fixture.pixel_colours(frame)))
            .chain(self.grids.iter().map(grid_colours))
            .collect()
    }
}

/// What a pixel shows: its white added to red, green and blue, scaled by `dimmer`, clipped at
/// full. `value` reads a channel.
fn emitted(channels: PixelChannels, dimmer: f32, value: impl Fn(usize) -> u8) -> Rgb {
    let level =
        |channel: Option<usize>| channel.map_or(0.0, |channel| f32::from(value(channel)) / 255.0);
    let white = level(channels.white);
    let lit = |colour: Option<usize>| ((level(colour) + white) * dimmer).min(1.0);
    Rgb {
        red: lit(channels.red),
        green: lit(channels.green),
        blue: lit(channels.blue),
    }
}

impl Fixture {
    fn new(entry: &FixtureEntry, profile: &FixtureProfile) -> Result<Fixture, String> {
        let mode = profile.mode(&entry.mode)?;
        let (dimmer, pixel_channels) = mode.layout()?;
        let fixture = Fixture {
            name: entry.name.clone(),
            profile: entry.profile.clone(),
            mode: entry.mode.clone(),
            address: entry.address,
            channels: mode.channels.clone(),
            dimmer,
            pixels: entry.canvas.place(&pixel_channels),
        };
        // The address check comes first: `last_address` overflows for a huge one.
        if !(1..=DMX_CHANNELS).contains(&fixture.address) || fixture.last_address() > DMX_CHANNELS {
            return Err(format!(
                "{} channels at address {} do not fit in DMX 1-{DMX_CHANNELS}",
                fixture.channels.len(),
                fixture.address
            ));
        }
        if entry.canvas.height != 0.0 {
            return Err(
                "canvas height is for pixel grids: a profiled fixture's pixels lie on one row"
                    .into(),
            );
        }
        entry.canvas.check()?;
        Ok(fixture)
    }

    pub fn last_address(&self) -> usize {
        self.address + self.channels.len() - 1
    }

    /// The mean position of its pixels.
    pub fn centre(&self) -> (f32, f32) {
        let count = self.pixels.len() as f32;
        let sum = |axis: fn(&Pixel) -> f32| self.pixels.iter().map(axis).sum::<f32>() / count;
        (sum(|pixel| pixel.x), sum(|pixel| pixel.y))
    }

    fn pixel_colours(&self, frame: &Frame) -> Vec<Rgb> {
        let value = |offset: usize| frame[self.address - 1 + offset];
        let dimmer = self
            .dimmer
            .map_or(1.0, |offset| f32::from(value(offset)) / 255.0);
        self.pixels
            .iter()
            .map(|pixel| emitted(pixel.channels, dimmer, value))
            .collect()
    }
}

/// A fixture's channels within one universe, 1-based, for overlap checks.
struct Span<'rig> {
    name: &'rig str,
    universe: Universe,
    first: usize,
    last: usize,
}

impl fmt::Display for Span<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self.universe {
            Universe::Enttec => write!(formatter, "DMX {}-{}", self.first, self.last),
            Universe::ArtNet(port_address) => write!(
                formatter,
                "Art-Net universe {port_address}, DMX {}-{}",
                self.first, self.last
            ),
        }
    }
}

impl PixelGrid {
    /// Lays the pixels out along the wiring, starting a new universe where the next pixel
    /// doesn't fit: 170 RGB or 128 RGBW pixels per universe.
    fn new(entry: &GridEntry) -> Result<PixelGrid, String> {
        let GridEntry {
            columns,
            rows,
            colour_order,
            address,
            canvas,
            ..
        } = *entry;
        if columns == 0 || rows == 0 {
            return Err("a grid needs at least 1 column and 1 row".into());
        }
        canvas.check()?;
        let mut universe = match (entry.output, entry.universe) {
            (Output::Enttec, None) => Universe::Enttec,
            (Output::Enttec, Some(_)) => {
                return Err(
                    "universe is for output: artnet; the Enttec box has a single universe".into(),
                );
            }
            (Output::Artnet, Some(port_address)) if port_address <= MAX_ARTNET_UNIVERSE => {
                Universe::ArtNet(port_address)
            }
            (Output::Artnet, Some(port_address)) => {
                return Err(format!(
                    "universe {port_address} is beyond Art-Net's last, {MAX_ARTNET_UNIVERSE}"
                ));
            }
            (Output::Artnet, None) => {
                return Err(
                    "output: artnet needs a universe: the Art-Net port-address, from 0".into(),
                );
            }
        };
        if !(1..=DMX_CHANNELS).contains(&address) {
            return Err(format!(
                "address {address} is not a DMX channel, 1-{DMX_CHANNELS}"
            ));
        }
        let size = colour_order.channels_per_pixel;
        let mut first = address - 1;
        // No capacity up front: a typo such as `rows: 100000` must reach the universe check below
        // rather than overflow or exhaust memory here.
        let count = columns.saturating_mul(rows);
        let mut pixels = Vec::new();
        for index in 0..count {
            if first + size > DMX_CHANNELS {
                universe = match universe {
                    Universe::ArtNet(port_address) if port_address < MAX_ARTNET_UNIVERSE => {
                        Universe::ArtNet(port_address + 1)
                    }
                    Universe::ArtNet(_) => {
                        return Err(format!(
                            "pixel {} runs past Art-Net's last universe",
                            index + 1
                        ));
                    }
                    Universe::Enttec => {
                        return Err(format!(
                            "{} pixels from address {address} do not fit in the Enttec universe; pixel {} would start past channel {DMX_CHANNELS}. Use output: artnet for a grid this size",
                            count,
                            index + 1
                        ));
                    }
                };
                first = 0;
            }
            let (column, row) = entry.position(index);
            pixels.push(GridPixel {
                x: canvas.x + canvas.width * (column as f32 + 0.5) / columns as f32,
                y: canvas.y + canvas.height * (row as f32 + 0.5) / rows as f32,
                universe,
                channels: colour_order.at(first),
            });
            first += size;
        }
        Ok(PixelGrid {
            name: entry.name.clone(),
            pixels,
        })
    }

    /// The channels the grid takes in each universe it uses.
    fn spans(&self) -> Vec<Span<'_>> {
        let mut spans: Vec<Span> = Vec::new();
        for pixel in &self.pixels {
            let channels = [
                pixel.channels.red,
                pixel.channels.green,
                pixel.channels.blue,
                pixel.channels.white,
            ];
            let first = channels.iter().flatten().min().expect("R, G and B") + 1;
            let last = channels.iter().flatten().max().expect("R, G and B") + 1;
            match spans.last_mut() {
                Some(span) if span.universe == pixel.universe => span.last = span.last.max(last),
                _ => spans.push(Span {
                    name: &self.name,
                    universe: pixel.universe,
                    first,
                    last,
                }),
            }
        }
        spans
    }
}

impl GridEntry {
    /// Column from the left and row from the bottom, as seen from the audience, of the pixel
    /// `index` along the wiring.
    fn position(&self, index: usize) -> (usize, usize) {
        let (line, along) = (index / self.columns, index % self.columns);
        let starts_left = matches!(
            self.start_corner,
            StartCorner::TopLeft | StartCorner::BottomLeft
        );
        let runs_back = self.wiring == Wiring::Serpentine && line % 2 == 1;
        let column = if starts_left != runs_back {
            along
        } else {
            self.columns - 1 - along
        };
        let row = match self.start_corner {
            StartCorner::BottomLeft | StartCorner::BottomRight => line,
            StartCorner::TopLeft | StartCorner::TopRight => self.rows - 1 - line,
        };
        (column, row)
    }
}

impl Canvas {
    fn check(&self) -> Result<(), String> {
        let Canvas {
            x,
            y,
            width,
            height,
        } = *self;
        if [x, y, width, height, x + width, y + height]
            .iter()
            .all(|value| (0.0..=1.0).contains(value))
        {
            return Ok(());
        }
        Err(format!(
            "canvas {{ x: {x}, y: {y}, width: {width}, height: {height} }} leaves the 0-1 stage canvas"
        ))
    }

    fn place(&self, pixels: &[PixelChannels]) -> Vec<Pixel> {
        let count = pixels.len() as f32;
        pixels
            .iter()
            .enumerate()
            .map(|(index, &channels)| Pixel {
                // ponytail: pixels lie on one row; give profiles a pixel grid when a fixture needs 2D.
                x: self.x + self.width * (index as f32 + 0.5) / count,
                y: self.y,
                channels,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_address_is_refused_instead_of_overflowing() {
        let profiles = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/shows/four-washes/fixtures");
        let rig = |address: &str| {
            format!(
                "fixtures:\n  - {{ name: wash, profile: stairville-wild-wash-pro-648-rgbw, mode: 16ch, address: {address}, canvas: {{ x: 0, y: 0 }} }}\n"
            )
        };

        let error = Rig::parse(&rig("18446744073709551610"), &profiles).unwrap_err();

        assert!(error.contains("do not fit in DMX 1-512"), "{error}");
        assert!(Rig::parse(&rig("497"), &profiles).is_ok());
    }

    fn grid(fields: &str) -> Result<Rig, String> {
        Rig::parse(
            &format!("fixtures:\n  - {{ name: grid, type: pixel-grid, {fields} }}\n"),
            Path::new("."),
        )
    }

    /// (column from the left, row from the bottom) of each pixel in wiring order, on a canvas
    /// that fills the whole front view.
    fn cells(rig: &Rig, columns: usize, rows: usize) -> Vec<(usize, usize)> {
        rig.grids[0]
            .pixels
            .iter()
            .map(|pixel| {
                let column = (pixel.x * columns as f32 - 0.5).round() as usize;
                let row = (pixel.y * rows as f32 - 0.5).round() as usize;
                (column, row)
            })
            .collect()
    }

    #[test]
    fn wiring_starts_at_the_corner_seen_from_the_audience_and_serpentine_runs_back() {
        let layout = |wiring: &str, corner: &str| {
            let rig = grid(&format!(
                "columns: 3, rows: 2, colour_order: RGB, wiring: {wiring}, start_corner: {corner}, address: 1, canvas: {{ x: 0, y: 0, width: 1, height: 1 }}"
            ))
            .unwrap();
            cells(&rig, 3, 2)
        };
        let bottom_row_first = [(0, 0), (1, 0), (2, 0)];

        assert_eq!(
            layout("row-by-row", "bottom-left"),
            [bottom_row_first.as_slice(), &[(0, 1), (1, 1), (2, 1)]].concat(),
            "row 0 is the bottom row: y 0 is the floor"
        );
        assert_eq!(
            layout("serpentine", "bottom-left"),
            [bottom_row_first.as_slice(), &[(2, 1), (1, 1), (0, 1)]].concat()
        );
        assert_eq!(
            layout("row-by-row", "top-left"),
            [(0, 1), (1, 1), (2, 1), (0, 0), (1, 0), (2, 0)]
        );
        assert_eq!(
            layout("serpentine", "top-right"),
            [(2, 1), (1, 1), (0, 1), (0, 0), (1, 0), (2, 0)]
        );
        assert_eq!(
            layout("serpentine", "bottom-right"),
            [(2, 0), (1, 0), (0, 0), (0, 1), (1, 1), (2, 1)]
        );
        assert_eq!(
            layout("row-by-row", "top-right"),
            [(2, 1), (1, 1), (0, 1), (2, 0), (1, 0), (0, 0)]
        );
    }

    #[test]
    fn every_colour_order_puts_each_emitter_at_its_letter() {
        fn orders(letters: &str) -> Vec<String> {
            if letters.len() == 1 {
                return vec![letters.into()];
            }
            letters
                .char_indices()
                .flat_map(|(index, letter)| {
                    let rest = format!("{}{}", &letters[..index], &letters[index + 1..]);
                    orders(&rest)
                        .into_iter()
                        .map(move |order| format!("{letter}{order}"))
                })
                .collect()
        }
        let all = [orders("RGB"), orders("RGBW")].concat();
        assert_eq!(all.len(), 6 + 24);

        for order in all {
            let rig = grid(&format!(
                "columns: 2, rows: 1, colour_order: {order}, address: 11, canvas: {{ x: 0, y: 0 }}"
            ))
            .unwrap();
            let second = rig.grids[0].pixels[1].channels;
            let at = |letter| order.find(letter).map(|offset| 10 + order.len() + offset);
            assert_eq!(
                [second.red, second.green, second.blue, second.white],
                [at('R'), at('G'), at('B'), at('W')],
                "{order}"
            );
        }
        assert!(
            grid("columns: 1, rows: 1, colour_order: RGBR, address: 1, canvas: { x: 0, y: 0 }")
                .unwrap_err()
                .contains(
                    "colour order \"RGBR\" must hold R, G and B, and optionally W, once each"
                )
        );
    }

    #[test]
    fn a_grid_continues_on_the_next_universe_at_170_rgb_or_128_rgbw_pixels() {
        let rgb = grid(
            "columns: 16, rows: 16, colour_order: RGB, output: artnet, universe: 2, address: 1, canvas: { x: 0, y: 0, width: 1, height: 1 }",
        )
        .unwrap();
        let pixels = &rgb.grids[0].pixels;
        assert_eq!(pixels[169].universe, Universe::ArtNet(2));
        assert_eq!(pixels[169].channels.blue, Some(509));
        assert_eq!(pixels[170].universe, Universe::ArtNet(3));
        assert_eq!(pixels[170].channels.red, Some(0));
        assert_eq!(pixels[255].channels.red, Some((255 - 170) * 3));
        assert_eq!(
            rgb.artnet_frames().keys().copied().collect::<Vec<_>>(),
            [2, 3]
        );

        let rgbw = grid(
            "columns: 1, rows: 129, colour_order: RGBW, output: artnet, universe: 0, address: 1, canvas: { x: 0, y: 0, height: 1 }",
        )
        .unwrap();
        assert_eq!(rgbw.grids[0].pixels[127].universe, Universe::ArtNet(0));
        assert_eq!(rgbw.grids[0].pixels[128].universe, Universe::ArtNet(1));
    }

    #[test]
    fn grids_that_cannot_be_output_are_refused() {
        let error = |fields: &str| grid(fields).unwrap_err();

        assert_eq!(
            error("columns: 1, rows: 129, colour_order: RGBW, address: 1, canvas: { x: 0, y: 0 }"),
            "fixture \"grid\": 129 pixels from address 1 do not fit in the Enttec universe; pixel 129 would start past channel 512. Use output: artnet for a grid this size"
        );
        assert_eq!(
            error(
                "columns: 1, rows: 1, colour_order: RGB, output: artnet, address: 1, canvas: { x: 0, y: 0 }"
            ),
            "fixture \"grid\": output: artnet needs a universe: the Art-Net port-address, from 0"
        );
        assert!(
            error("columns: 1, rows: 1, colour_order: RGB, adress: 1, canvas: { x: 0, y: 0 }")
                .starts_with("fixture \"grid\": unknown field `adress`")
        );
        assert!(
            error("columns: 2, rows: 1, colour_order: RGB, address: 1, canvas: { x: 0.5, y: 0, width: 0.6 }")
                .ends_with("leaves the 0-1 stage canvas")
        );
        for size in [
            "columns: 100000, rows: 100000",
            "columns: 4294967296, rows: 4294967296",
        ] {
            assert!(
                error(&format!(
                    "{size}, colour_order: RGB, output: artnet, universe: 0, address: 1, canvas: {{ x: 0, y: 0 }}"
                ))
                .ends_with("runs past Art-Net's last universe"),
                "{size}"
            );
        }
    }

    #[test]
    fn the_canonical_form_keeps_a_grid_and_reads_back_the_same() {
        let content = serde_json::json!({ "fixtures": [{
            "canvas": { "height": 1, "width": 1, "x": 0, "y": 0 },
            "address": 1, "universe": 2, "output": "artnet", "wiring": "serpentine",
            "colour_order": "WGRB", "rows": 2, "columns": 3, "type": "pixel-grid", "name": "wall"
        }] });

        let yaml = canonical_yaml(content).unwrap();

        assert!(
            yaml.starts_with("fixtures:\n- name: wall\n  type: pixel-grid\n  columns: 3\n"),
            "{yaml}"
        );
        assert!(yaml.contains("colour_order: WGRB\n"), "{yaml}");
        let rig = Rig::parse(&yaml, Path::new(".")).unwrap();
        assert_eq!(rig.grids[0].pixels.len(), 6);
        assert_eq!(rig.grids[0].pixels[0].universe, Universe::ArtNet(2));
    }

    #[test]
    fn grids_overlapping_in_the_same_universe_are_refused() {
        let grids = |second_universe: u16| {
            Rig::parse(
                &format!(
                    "fixtures:
  - {{ name: a, type: pixel-grid, columns: 2, rows: 1, colour_order: RGB, output: artnet, universe: 4, address: 1, canvas: {{ x: 0, y: 0 }} }}
  - {{ name: b, type: pixel-grid, columns: 2, rows: 1, colour_order: RGB, output: artnet, universe: {second_universe}, address: 4, canvas: {{ x: 0, y: 0 }} }}
"
                ),
                Path::new("."),
            )
        };

        assert_eq!(
            grids(4).unwrap_err(),
            "fixtures \"a\" (Art-Net universe 4, DMX 1-6) and \"b\" (Art-Net universe 4, DMX 4-9) overlap"
        );
        assert!(grids(5).is_ok());
    }
}

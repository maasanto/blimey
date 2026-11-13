//! The rig: which fixtures hang where, at which DMX address, in which groups.
//! Parsed from a show's `rig.yaml`, with profiles from its `fixtures/` folder.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::colour::Rgb;
use crate::fixture::{Channel, FixtureProfile, PixelChannels};
use crate::{DMX_CHANNELS, Frame};

/// `rig.yaml` as written. Field order here is the canonical order in the file.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RigFile {
    fixtures: Vec<FixtureEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    groups: BTreeMap<String, Vec<String>>,
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

/// Where a fixture sits on the normalised 2D stage canvas, a front view from the audience:
/// `x` from the left edge (0, stage right) to the right (1, stage left), `y` from the floor (0) up (1). Its pixels are
/// spread evenly across `width`, in pixel order, starting at `x`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub x: f32,
    pub y: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: f32,
}

fn is_zero(width: &f32) -> bool {
    *width == 0.0
}

/// `rig.yaml` content, given as its JSON equivalent, in the engine's canonical YAML form.
pub fn canonical_yaml(content: serde_json::Value) -> Result<String, String> {
    let file: RigFile = serde_json::from_value(content).map_err(|error| error.to_string())?;
    serde_yaml_ng::to_string(&file).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rig {
    pub fixtures: Vec<Fixture>,
    pub groups: BTreeMap<String, Vec<String>>,
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
        let mut profiles: BTreeMap<&str, FixtureProfile> = BTreeMap::new();
        let mut fixtures = Vec::new();
        for entry in &file.fixtures {
            if !profiles.contains_key(entry.profile.as_str()) {
                let path = profiles_dir.join(format!("{}.yaml", entry.profile));
                profiles.insert(&entry.profile, FixtureProfile::load(&path)?);
            }
            let fixture = Fixture::new(entry, &profiles[entry.profile.as_str()])
                .map_err(|error| format!("fixture {:?}: {error}", entry.name))?;
            fixtures.push(fixture);
        }
        let rig = Rig {
            fixtures,
            groups: file.groups,
        };
        rig.validate()?;
        Ok(rig)
    }

    fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();
        if let Some(twice) = self
            .fixtures
            .iter()
            .find(|fixture| !names.insert(&fixture.name))
        {
            return Err(format!("fixture {:?} is defined twice", twice.name));
        }
        let mut by_address: Vec<&Fixture> = self.fixtures.iter().collect();
        by_address.sort_by_key(|fixture| fixture.address);
        for pair in by_address.windows(2) {
            let (first, second) = (pair[0], pair[1]);
            if second.address <= first.last_address() {
                return Err(format!(
                    "fixtures {:?} (DMX {}-{}) and {:?} (DMX {}-{}) overlap",
                    first.name,
                    first.address,
                    first.last_address(),
                    second.name,
                    second.address,
                    second.last_address()
                ));
            }
        }
        for (group, members) in &self.groups {
            if self.fixture(group).is_some() {
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

    /// The colour each pixel shows for a DMX frame, white and dimmer folded in, for previews.
    pub fn pixel_colours<'rig>(&'rig self, frame: &Frame) -> Vec<(&'rig str, Vec<Rgb>)> {
        self.fixtures
            .iter()
            .map(|fixture| (fixture.name.as_str(), fixture.pixel_colours(frame)))
            .collect()
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
        let Canvas { x, y, width } = entry.canvas;
        if ![x, y, width, x + width]
            .iter()
            .all(|v| (0.0..=1.0).contains(v))
        {
            return Err(format!(
                "canvas {{ x: {x}, y: {y}, width: {width} }} leaves the 0-1 stage canvas"
            ));
        }
        Ok(fixture)
    }

    pub fn last_address(&self) -> usize {
        self.address + self.channels.len() - 1
    }

    fn pixel_colours(&self, frame: &Frame) -> Vec<Rgb> {
        let level = |offset: Option<usize>| {
            offset.map_or(0.0, |offset| {
                f32::from(frame[self.address - 1 + offset]) / 255.0
            })
        };
        let dimmer = self.dimmer.map_or(1.0, |offset| level(Some(offset)));
        self.pixels
            .iter()
            .map(|pixel| {
                let white = level(pixel.channels.white);
                let lit = |colour: Option<usize>| ((level(colour) + white) * dimmer).min(1.0);
                Rgb {
                    red: lit(pixel.channels.red),
                    green: lit(pixel.channels.green),
                    blue: lit(pixel.channels.blue),
                }
            })
            .collect()
    }
}

impl Canvas {
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
}

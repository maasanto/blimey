//! Fixture profiles: for each DMX mode, what every channel drives. Stored as YAML in a show's
//! `fixtures/` folder, usually produced by `strata fixture import` from Open Fixture Library JSON.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureProfile {
    pub name: String,
    /// Where the channel layout comes from, typically the manufacturer's manual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub modes: Vec<Mode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub name: String,
    /// In DMX order: the first entry is the fixture's start address.
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub name: String,
    /// What the engine drives on this channel. Channels without one hold `default`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<Attribute>,
    /// For colour attributes, the 1-based pixel (segment) the channel belongs to; defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixel: Option<usize>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub default: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attribute {
    /// Master intensity for the whole fixture.
    Dimmer,
    Red,
    Green,
    Blue,
    White,
}

/// The emitters of one pixel, as channel offsets from the fixture's start address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PixelChannels {
    pub red: Option<usize>,
    pub green: Option<usize>,
    pub blue: Option<usize>,
    pub white: Option<usize>,
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

impl FixtureProfile {
    pub fn load(path: &Path) -> Result<FixtureProfile, String> {
        let in_file = |error: String| format!("{}: {error}", path.display());
        let text = fs::read_to_string(path).map_err(|error| in_file(error.to_string()))?;
        let profile: FixtureProfile =
            serde_yaml_ng::from_str(&text).map_err(|error| in_file(error.to_string()))?;
        for mode in &profile.modes {
            mode.layout().map_err(in_file)?;
        }
        Ok(profile)
    }

    pub fn mode(&self, name: &str) -> Result<&Mode, String> {
        self.modes
            .iter()
            .find(|mode| mode.name == name)
            .ok_or_else(|| {
                let names: Vec<_> = self.modes.iter().map(|mode| mode.name.as_str()).collect();
                format!(
                    "{} has no mode {name:?}; its modes are {}",
                    self.name,
                    names.join(", ")
                )
            })
    }
}

impl Mode {
    /// The dimmer channel offset, if any, and each pixel's emitters in pixel order.
    ///
    /// Fails if two channels drive the same attribute of the same pixel, the kind of
    /// profile error that mis-tags a fixture.
    pub fn layout(&self) -> Result<(Option<usize>, Vec<PixelChannels>), String> {
        let mut dimmer = None;
        let mut pixels: Vec<PixelChannels> = Vec::new();
        for (offset, channel) in self.channels.iter().enumerate() {
            let Some(attribute) = channel.attribute else {
                continue;
            };
            let pixel = channel.pixel.unwrap_or(1);
            if pixel == 0 {
                return Err(format!(
                    "mode {}: channel {} has pixel 0; pixels count from 1",
                    self.name,
                    offset + 1
                ));
            }
            if pixels.len() < pixel {
                pixels.resize(pixel, PixelChannels::default());
            }
            let emitters = &mut pixels[pixel - 1];
            let slot = match attribute {
                Attribute::Dimmer => &mut dimmer,
                Attribute::Red => &mut emitters.red,
                Attribute::Green => &mut emitters.green,
                Attribute::Blue => &mut emitters.blue,
                Attribute::White => &mut emitters.white,
            };
            if let Some(first) = slot.replace(offset) {
                return Err(format!(
                    "mode {}: channels {} and {} both drive {attribute:?} of pixel {pixel}",
                    self.name,
                    first + 1,
                    offset + 1
                ));
            }
        }
        if pixels.is_empty() {
            pixels.push(PixelChannels::default());
        }
        Ok((dimmer, pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(attribute: Attribute, pixel: Option<usize>) -> Channel {
        Channel {
            name: format!("{attribute:?}"),
            attribute: Some(attribute),
            pixel,
            default: 0,
        }
    }

    #[test]
    fn layout_groups_emitters_by_pixel() {
        let mode = Mode {
            name: "6ch".into(),
            channels: vec![
                channel(Attribute::Dimmer, None),
                channel(Attribute::Red, Some(1)),
                channel(Attribute::Green, Some(1)),
                channel(Attribute::Red, Some(2)),
                channel(Attribute::Green, Some(2)),
            ],
        };

        let (dimmer, pixels) = mode.layout().unwrap();

        assert_eq!(dimmer, Some(0));
        assert_eq!(
            pixels,
            [
                PixelChannels {
                    red: Some(1),
                    green: Some(2),
                    ..PixelChannels::default()
                },
                PixelChannels {
                    red: Some(3),
                    green: Some(4),
                    ..PixelChannels::default()
                },
            ]
        );
    }

    #[test]
    fn layout_rejects_an_attribute_driven_twice() {
        let mode = Mode {
            name: "3ch".into(),
            channels: vec![
                channel(Attribute::Red, None),
                channel(Attribute::Green, None),
                channel(Attribute::Red, Some(1)),
            ],
        };

        assert_eq!(
            mode.layout().unwrap_err(),
            "mode 3ch: channels 1 and 3 both drive Red of pixel 1"
        );
    }
}

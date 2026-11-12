//! Looks: colour and dimmer per fixture, set by fixture or group name and palette colour,
//! rendered through the rig's profiles on top of the layers below.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::colour::{Rgb, RgbwMode, to_dmx};
use crate::rig::Rig;
use crate::{Frame, Show};

/// The settings applied to a fixture or group, as written in show files
/// (`{ color: deep-blue, dimmer: 60% }`) or on the command line. Unset fields keep their value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookSettings {
    pub color: Option<String>,
    pub dimmer: Option<Level>,
    pub rgbw: Option<RgbwMode>,
}

impl LookSettings {
    /// Parses `key=value` pairs, such as `color=deep-blue dimmer=60%`, with the same rules
    /// and messages as show files.
    pub fn from_pairs(pairs: &[String]) -> Result<LookSettings, String> {
        let mut mapping = serde_yaml_ng::Mapping::new();
        for pair in pairs {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("{pair:?} is not key=value"))?;
            mapping.insert(key.into(), value.into());
        }
        serde_yaml_ng::from_value(mapping.into()).map_err(|error| error.to_string())
    }
}

/// A 0–1 level, written as a percentage: `60%`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Level(pub f32);

impl TryFrom<String> for Level {
    type Error = String;

    fn try_from(text: String) -> Result<Level, String> {
        text.strip_suffix('%')
            .and_then(|percent| percent.trim().parse::<f32>().ok())
            .filter(|percent| (0.0..=100.0).contains(percent))
            .map(|percent| Level(percent / 100.0))
            .ok_or_else(|| format!("level {text:?} must be a percentage from 0% to 100%"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixtureLook {
    pub color: Rgb,
    pub dimmer: f32,
    pub rgbw: RgbwMode,
}

impl Default for FixtureLook {
    /// Dimmer full and colour black, so setting just a colour lights the fixture.
    fn default() -> Self {
        FixtureLook {
            color: Rgb::default(),
            dimmer: 1.0,
            rgbw: RgbwMode::default(),
        }
    }
}

/// Per-fixture colour and dimmer. Fixtures the look doesn't mention are left to the layers
/// below it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    pub fixtures: BTreeMap<String, FixtureLook>,
}

impl Look {
    /// Applies `settings` to a fixture or every fixture of a group.
    pub fn set(
        &mut self,
        show: &Show,
        target: &str,
        settings: &LookSettings,
    ) -> Result<(), String> {
        let color = settings
            .color
            .as_deref()
            .map(|name| colour(show, name))
            .transpose()?;
        for fixture in show.rig.resolve(target)? {
            let look = self.fixtures.entry(fixture.name.clone()).or_default();
            if let Some(color) = color {
                look.color = color;
            }
            if let Some(Level(dimmer)) = settings.dimmer {
                look.dimmer = dimmer;
            }
            if let Some(rgbw) = settings.rgbw {
                look.rgbw = rgbw;
            }
        }
        Ok(())
    }
}

/// A palette name, or a literal `#rrggbb`.
fn colour(show: &Show, name: &str) -> Result<Rgb, String> {
    if name.starts_with('#') {
        return Rgb::from_hex(name);
    }
    show.palettes
        .get(name)
        .copied()
        .ok_or_else(|| format!("no colour {name:?} in palettes.yaml, and not a #rrggbb value"))
}

/// Draws `look` over `frame`. Every channel of a fixture in the look is overwritten: colour
/// and dimmer from the look, the rest at their profile default. Other fixtures are untouched.
///
/// A fixture with a dimmer channel gets the look's dimmer there; one without has its
/// colour channels scaled by it.
pub fn render(rig: &Rig, look: &Look, frame: &mut Frame) {
    for fixture in &rig.fixtures {
        let Some(fixture_look) = look.fixtures.get(&fixture.name) else {
            continue;
        };
        let start = fixture.address - 1;
        for (offset, channel) in fixture.channels.iter().enumerate() {
            frame[start + offset] = channel.default;
        }
        let colour_scale = match fixture.dimmer {
            Some(offset) => {
                frame[start + offset] = to_dmx(fixture_look.dimmer);
                1.0
            }
            None => fixture_look.dimmer,
        };
        for pixel in &fixture.pixels {
            let mode = match pixel.channels.white {
                Some(_) => fixture_look.rgbw,
                None => RgbwMode::Saturated,
            };
            let emitted = mode.convert(fixture_look.color);
            let emitters = [
                (pixel.channels.red, emitted.red),
                (pixel.channels.green, emitted.green),
                (pixel.channels.blue, emitted.blue),
                (pixel.channels.white, emitted.white),
            ];
            for (offset, level) in emitters {
                if let Some(offset) = offset {
                    frame[start + offset] = to_dmx(level * colour_scale);
                }
            }
        }
    }
}

/// Scales the intensity `frame` gives `fixtures` by `factor` (0–1), as a look's dimmer does:
/// the dimmer channel of a fixture with one, the colour channels of one without.
pub fn scale_intensity(rig: &Rig, fixtures: &BTreeSet<String>, factor: f32, frame: &mut Frame) {
    for fixture in rig
        .fixtures
        .iter()
        .filter(|fixture| fixtures.contains(&fixture.name))
    {
        let offsets: Vec<usize> = match fixture.dimmer {
            Some(offset) => vec![offset],
            None => fixture
                .pixels
                .iter()
                .flat_map(|pixel| {
                    let channels = pixel.channels;
                    [channels.red, channels.green, channels.blue, channels.white]
                })
                .flatten()
                .collect(),
        };
        for offset in offsets {
            let value = &mut frame[fixture.address - 1 + offset];
            *value = (f32::from(*value) * factor).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DMX_CHANNELS;
    use crate::fixture::{Attribute, Channel};
    use crate::rig::{Fixture, Pixel};

    /// One RGBW par at DMX 10: dimmer, red, green, blue, white, then a shutter that must
    /// sit at 255 to open.
    fn par_show() -> Show {
        let channel = |name: &str, attribute, default| Channel {
            name: name.into(),
            attribute,
            pixel: None,
            default,
        };
        let channels = vec![
            channel("Dimmer", Some(Attribute::Dimmer), 0),
            channel("Red", Some(Attribute::Red), 0),
            channel("Green", Some(Attribute::Green), 0),
            channel("Blue", Some(Attribute::Blue), 0),
            channel("White", Some(Attribute::White), 0),
            channel("Shutter", None, 255),
        ];
        let mode = crate::fixture::Mode {
            name: "6ch".into(),
            channels: channels.clone(),
        };
        let (dimmer, pixel_channels) = mode.layout().unwrap();
        let par = Fixture {
            name: "par".into(),
            profile: "par".into(),
            mode: "6ch".into(),
            address: 10,
            channels,
            dimmer,
            pixels: vec![Pixel {
                x: 0.5,
                y: 0.5,
                channels: pixel_channels[0],
            }],
        };
        Show {
            rig: Rig {
                fixtures: vec![par],
                groups: BTreeMap::from([("everything".into(), vec!["par".into()])]),
            },
            palettes: BTreeMap::from([("pink".into(), Rgb::from_hex("#ff8080").unwrap())]),
            ..Show::default()
        }
    }

    fn set(show: &Show, look: &mut Look, target: &str, pairs: &[&str]) -> Result<(), String> {
        let pairs: Vec<String> = pairs.iter().map(|pair| pair.to_string()).collect();
        look.set(show, target, &LookSettings::from_pairs(&pairs)?)
    }

    fn rendered(show: &Show, look: &Look, below: Frame) -> Frame {
        let mut frame = below;
        render(&show.rig, look, &mut frame);
        frame
    }

    #[test]
    fn dimmer_goes_to_the_dimmer_channel_and_white_rich_to_white() {
        let show = par_show();
        let mut look = Look::default();

        set(
            &show,
            &mut look,
            "everything",
            &["color=pink", "dimmer=50%", "rgbw=white-rich"],
        )
        .unwrap();
        let frame = rendered(&show, &look, [0; DMX_CHANNELS]);

        assert_eq!(frame[9..15], [128, 127, 0, 0, 128, 255]);
    }

    #[test]
    fn a_fixture_in_the_look_overrides_every_channel_below_and_others_show_through() {
        let show = par_show();
        let mut look = Look::default();
        let below = [7; DMX_CHANNELS];

        assert_eq!(rendered(&show, &look, below), below);

        set(&show, &mut look, "par", &["color=pink"]).unwrap();
        set(&show, &mut look, "par", &["dimmer=100%"]).unwrap();
        let frame = rendered(&show, &look, below);

        assert_eq!(frame[9..15], [255, 255, 128, 128, 0, 255]);
        assert_eq!((frame[8], frame[15]), (7, 7));
    }

    #[test]
    fn bad_settings_are_named() {
        let show = par_show();
        let mut look = Look::default();

        assert_eq!(
            set(&show, &mut look, "par", &["dimmer=60"]).unwrap_err(),
            "level \"60\" must be a percentage from 0% to 100%"
        );
        assert_eq!(
            set(&show, &mut look, "par", &["color=teal"]).unwrap_err(),
            "no colour \"teal\" in palettes.yaml, and not a #rrggbb value"
        );
        assert_eq!(
            set(&show, &mut look, "front", &["color=pink"]).unwrap_err(),
            "\"front\" is neither a fixture nor a group in the rig"
        );
        assert!(
            set(&show, &mut look, "par", &["colour=pink"])
                .unwrap_err()
                .starts_with("unknown field `colour`")
        );
    }
}

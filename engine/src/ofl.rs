//! Import of Open Fixture Library JSON (https://open-fixture-library.org, fixture schema)
//! into a `FixtureProfile`.
//!
//! Only what Blimey drives is interpreted: intensity and red/green/blue/white colour
//! intensity, per pixel for matrix fixtures. Every other channel is kept by name and held at
//! its OFL default value.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::fixture::{Attribute, Channel, FixtureProfile, Mode};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OflFixture {
    name: String,
    #[serde(default)]
    links: OflLinks,
    #[serde(default)]
    available_channels: BTreeMap<String, OflChannel>,
    #[serde(default)]
    template_channels: BTreeMap<String, OflChannel>,
    matrix: Option<OflMatrix>,
    modes: Vec<OflMode>,
}

#[derive(Deserialize, Default)]
struct OflLinks {
    #[serde(default)]
    manual: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OflChannel {
    capability: Option<Value>,
    default_value: Option<Value>,
    #[serde(default)]
    fine_channel_aliases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OflMatrix {
    pixel_keys: Option<Vec<Vec<Vec<Option<String>>>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OflMode {
    name: String,
    short_name: Option<String>,
    channels: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatrixInsert {
    insert: String,
    repeat_for: Value,
    channel_order: String,
    template_channels: Vec<String>,
}

pub fn import_ofl(json: &str) -> Result<FixtureProfile, String> {
    let fixture: OflFixture =
        serde_json::from_str(json).map_err(|error| format!("not an OFL fixture: {error}"))?;
    let pixel_keys = fixture.pixel_keys()?;
    let modes = fixture
        .modes
        .iter()
        .map(|mode| {
            let name = mode.short_name.clone().unwrap_or_else(|| mode.name.clone());
            let channels = fixture
                .mode_channels(mode, &pixel_keys)
                .map_err(|error| format!("mode {name}: {error}"))?;
            Ok(Mode { name, channels })
        })
        .collect::<Result<_, String>>()?;
    Ok(FixtureProfile {
        name: fixture.name,
        source: fixture.links.manual.into_iter().next(),
        modes,
    })
}

impl OflFixture {
    /// Matrix pixel keys in pixel order (x fastest, then y, then z).
    fn pixel_keys(&self) -> Result<Vec<String>, String> {
        let Some(matrix) = &self.matrix else {
            return Ok(Vec::new());
        };
        // ponytail: matrices given as pixelCount are refused; support them when a fixture needs it.
        let layers = matrix.pixel_keys.as_ref().ok_or(
            "matrix.pixelCount is not supported; describe the matrix with matrix.pixelKeys",
        )?;
        Ok(layers
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .cloned()
            .collect())
    }

    fn mode_channels(&self, mode: &OflMode, pixel_keys: &[String]) -> Result<Vec<Channel>, String> {
        let mut channels = Vec::new();
        for entry in &mode.channels {
            match entry {
                Value::Null => channels.push(unused_channel()),
                Value::String(name) => channels.push(self.channel(name, pixel_keys)?),
                Value::Object(_) => {
                    let insert: MatrixInsert = serde_json::from_value(entry.clone())
                        .map_err(|error| format!("unsupported channel insert: {error}"))?;
                    for name in insert.channel_names(pixel_keys)? {
                        channels.push(self.channel(&name, pixel_keys)?);
                    }
                }
                other => return Err(format!("unexpected channel entry {other}")),
            }
        }
        Ok(channels)
    }

    /// Resolves a mode's channel name: an available channel, a fine-channel alias, or a
    /// template channel expanded for one pixel.
    fn channel(&self, name: &str, pixel_keys: &[String]) -> Result<Channel, String> {
        if let Some(channel) = self.available_channels.get(name) {
            return channel.to_profile(name, None);
        }
        let is_fine_alias = self
            .available_channels
            .values()
            .chain(self.template_channels.values())
            .any(|channel| {
                channel
                    .fine_channel_aliases
                    .iter()
                    .any(|alias| alias == name)
            });
        if is_fine_alias {
            // ponytail: 16-bit channels drive only their coarse byte; the fine byte stays at 0.
            return Ok(Channel {
                name: name.to_string(),
                ..unused_channel()
            });
        }
        for (template_name, template) in &self.template_channels {
            for (index, key) in pixel_keys.iter().enumerate() {
                if template_name.replace("$pixelKey", key) == name {
                    return template.to_profile(name, Some(index + 1));
                }
            }
        }
        Err(format!("channel {name:?} is not defined in the fixture"))
    }
}

impl MatrixInsert {
    fn channel_names(&self, pixel_keys: &[String]) -> Result<Vec<String>, String> {
        if self.insert != "matrixChannels" {
            return Err(format!("unsupported channel insert {:?}", self.insert));
        }
        let pixels: Vec<String> = match &self.repeat_for {
            Value::String(order) if order == "eachPixelABC" => {
                let mut sorted = pixel_keys.to_vec();
                sorted.sort();
                sorted
            }
            Value::Array(keys) => keys
                .iter()
                .map(|key| key.as_str().map(String::from))
                .collect::<Option<_>>()
                .ok_or("repeatFor must list pixel keys as strings")?,
            other => {
                return Err(format!(
                    "unsupported repeatFor {other}; list the pixel keys"
                ));
            }
        };
        let name = |template: &String, pixel: &String| template.replace("$pixelKey", pixel);
        Ok(match self.channel_order.as_str() {
            "perPixel" => pixels
                .iter()
                .flat_map(|pixel| self.template_channels.iter().map(move |t| name(t, pixel)))
                .collect(),
            "perChannel" => self
                .template_channels
                .iter()
                .flat_map(|t| pixels.iter().map(move |pixel| name(t, pixel)))
                .collect(),
            other => return Err(format!("unsupported channelOrder {other:?}")),
        })
    }
}

impl OflChannel {
    fn to_profile(&self, name: &str, pixel: Option<usize>) -> Result<Channel, String> {
        let attribute = self.attribute();
        Ok(Channel {
            name: name.to_string(),
            attribute,
            pixel: pixel.filter(|_| attribute.is_some_and(|a| a != Attribute::Dimmer)),
            default: self
                .default_dmx_value()
                .map_err(|error| format!("channel {name:?}: {error}"))?,
        })
    }

    fn attribute(&self) -> Option<Attribute> {
        let capability = self.capability.as_ref()?;
        match (capability["type"].as_str()?, capability["color"].as_str()) {
            ("Intensity", _) => Some(Attribute::Dimmer),
            ("ColorIntensity", Some("Red")) => Some(Attribute::Red),
            ("ColorIntensity", Some("Green")) => Some(Attribute::Green),
            ("ColorIntensity", Some("Blue")) => Some(Attribute::Blue),
            ("ColorIntensity", Some("White")) => Some(Attribute::White),
            _ => None,
        }
    }

    /// OFL gives defaults as a DMX value or as a percentage string such as `"50%"`.
    fn default_dmx_value(&self) -> Result<u8, String> {
        let invalid = || {
            format!(
                "defaultValue {:?} is not 0-255 or a percentage",
                self.default_value
            )
        };
        match &self.default_value {
            None => Ok(0),
            Some(Value::Number(number)) => number
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(invalid),
            Some(Value::String(text)) => text
                .strip_suffix('%')
                .and_then(|percent| percent.parse::<f32>().ok())
                .filter(|percent| (0.0..=100.0).contains(percent))
                .map(|percent| crate::colour::to_dmx(percent / 100.0))
                .ok_or_else(invalid),
            Some(_) => Err(invalid()),
        }
    }
}

fn unused_channel() -> Channel {
    Channel {
        name: "(unused)".into(),
        attribute: None,
        pixel: None,
        default: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_plain_channels_with_their_defaults() {
        let json = r##"{
            "name": "Par",
            "links": { "manual": ["https://example.com/par.pdf"] },
            "availableChannels": {
                "Dimmer": { "capability": { "type": "Intensity" } },
                "Red": { "capability": { "type": "ColorIntensity", "color": "Red" } },
                "Amber": { "capability": { "type": "ColorIntensity", "color": "Amber" } },
                "Shutter": { "defaultValue": "100%", "capabilities": [] }
            },
            "modes": [{ "name": "5-channel", "shortName": "5ch",
                        "channels": ["Dimmer", "Red", "Amber", "Shutter", null] }]
        }"##;

        let profile = import_ofl(json).unwrap();

        assert_eq!(
            profile.source.as_deref(),
            Some("https://example.com/par.pdf")
        );
        let mode = profile.mode("5ch").unwrap();
        let summary: Vec<_> = mode
            .channels
            .iter()
            .map(|c| (c.name.as_str(), c.attribute, c.default))
            .collect();
        assert_eq!(
            summary,
            [
                ("Dimmer", Some(Attribute::Dimmer), 0),
                ("Red", Some(Attribute::Red), 0),
                ("Amber", None, 0),
                ("Shutter", None, 255),
                ("(unused)", None, 0),
            ]
        );
    }

    #[test]
    fn expands_matrix_channels_per_channel() {
        let json = r##"{
            "name": "Bar",
            "templateChannels": {
                "Red $pixelKey": { "capability": { "type": "ColorIntensity", "color": "Red" } },
                "Green $pixelKey": { "capability": { "type": "ColorIntensity", "color": "Green" } }
            },
            "matrix": { "pixelKeys": [[["A", "B"]]] },
            "modes": [{ "name": "4ch", "channels": [{
                "insert": "matrixChannels", "repeatFor": "eachPixelABC",
                "channelOrder": "perChannel", "templateChannels": ["Red $pixelKey", "Green $pixelKey"]
            }] }]
        }"##;

        let profile = import_ofl(json).unwrap();

        let channels: Vec<_> = profile.modes[0]
            .channels
            .iter()
            .map(|c| (c.name.as_str(), c.attribute.unwrap(), c.pixel.unwrap()))
            .collect();
        assert_eq!(
            channels,
            [
                ("Red A", Attribute::Red, 1),
                ("Red B", Attribute::Red, 2),
                ("Green A", Attribute::Green, 1),
                ("Green B", Attribute::Green, 2),
            ]
        );
    }

    #[test]
    fn names_an_undefined_channel() {
        let json = r#"{ "name": "X", "modes": [{ "name": "1ch", "channels": ["Dimmer"] }] }"#;

        assert_eq!(
            import_ofl(json).unwrap_err(),
            "mode 1ch: channel \"Dimmer\" is not defined in the fixture"
        );
    }
}

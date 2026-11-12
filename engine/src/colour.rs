//! Colours as the engine works with them (RGB, 0–1 per component), named palettes, and the
//! conversion to RGBW emitters.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgb {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Rgb {
    /// Parses `#rrggbb`.
    pub fn from_hex(hex: &str) -> Result<Rgb, String> {
        let invalid = || format!("{hex:?} is not a colour, expected #rrggbb");
        let digits = hex
            .strip_prefix('#')
            .filter(|digits| digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()));
        let value = digits
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .ok_or_else(invalid)?;
        let component = |shift: u32| ((value >> shift) & 0xff) as f32 / 255.0;
        Ok(Rgb {
            red: component(16),
            green: component(8),
            blue: component(0),
        })
    }

    pub fn to_hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            to_dmx(self.red),
            to_dmx(self.green),
            to_dmx(self.blue)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgbw {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub white: f32,
}

/// How an RGB colour is spread over an RGBW fixture's emitters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RgbwMode {
    /// White emitter off: the deepest colours, as an RGB-only fixture would show them.
    #[default]
    Saturated,
    /// The part common to R, G and B moves to the white emitter: same hue, brighter and
    /// smoother pastels and whites.
    WhiteRich,
}

impl RgbwMode {
    pub fn convert(self, colour: Rgb) -> Rgbw {
        let white = match self {
            RgbwMode::Saturated => 0.0,
            RgbwMode::WhiteRich => colour.red.min(colour.green).min(colour.blue),
        };
        Rgbw {
            red: colour.red - white,
            green: colour.green - white,
            blue: colour.blue - white,
            white,
        }
    }
}

/// A 0–1 level as an 8-bit DMX value, clamped and rounded.
pub fn to_dmx(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteEntry {
    rgb: String,
    /// The gel the colour approximates, for humans; the engine only uses `rgb`.
    #[serde(default, rename = "lee")]
    _lee: Option<String>,
}

/// Parses `palettes.yaml`: colour name → `{ rgb: "#rrggbb", lee: "<gel>" }`.
pub fn parse_palettes(yaml: &str) -> Result<BTreeMap<String, Rgb>, String> {
    let entries: BTreeMap<String, PaletteEntry> =
        serde_yaml_ng::from_str(yaml).map_err(|error| error.to_string())?;
    entries
        .into_iter()
        .map(|(name, entry)| {
            let colour = Rgb::from_hex(&entry.rgb).map_err(|error| format!("{name}: {error}"))?;
            Ok((name, colour))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORANGE: Rgb = Rgb {
        red: 1.0,
        green: 0.5,
        blue: 0.2,
    };

    #[test]
    fn saturated_leaves_white_off() {
        assert_eq!(
            RgbwMode::Saturated.convert(ORANGE),
            Rgbw {
                red: 1.0,
                green: 0.5,
                blue: 0.2,
                white: 0.0
            }
        );
    }

    #[test]
    fn white_rich_moves_the_common_part_to_white() {
        let rgbw = RgbwMode::WhiteRich.convert(ORANGE);

        assert_eq!(rgbw.white, 0.2);
        assert_eq!(
            [rgbw.red, rgbw.green, rgbw.blue],
            [1.0 - 0.2, 0.5 - 0.2, 0.0]
        );
    }

    #[test]
    fn white_rich_turns_white_into_the_white_emitter_alone() {
        let white = Rgb {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        };

        assert_eq!(
            RgbwMode::WhiteRich.convert(white),
            Rgbw {
                white: 1.0,
                ..Rgbw::default()
            }
        );
        assert_eq!(RgbwMode::Saturated.convert(white).white, 0.0);
    }

    #[test]
    fn primaries_are_the_same_in_both_modes() {
        let red = Rgb {
            red: 1.0,
            ..Rgb::default()
        };

        assert_eq!(
            RgbwMode::WhiteRich.convert(red),
            RgbwMode::Saturated.convert(red)
        );
    }

    #[test]
    fn hex_round_trips_and_rejects_garbage() {
        assert_eq!(Rgb::from_hex("#0a1bff").unwrap().to_hex(), "#0a1bff");
        assert_eq!(
            Rgb::from_hex("blue").unwrap_err(),
            "\"blue\" is not a colour, expected #rrggbb"
        );
        assert!(Rgb::from_hex("#12345").is_err());
    }
}

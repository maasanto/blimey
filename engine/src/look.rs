//! Looks: colour and dimmer per fixture, set by fixture or group name and palette colour,
//! rendered through the rig's profiles on top of the layers below.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::colour::{Rgb, RgbwMode, to_dmx};
use crate::fixture::PixelChannels;
use crate::rig::{Fixture, Pixel, Rig};
use crate::{DMX_CHANNELS, Frame, Show};

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
    /// `None` keeps the colour the layers below give the fixture, so a look of only a dimmer sets
    /// the intensity of what is underneath: its dimmer channel, or without one its colour scaled
    /// down (see `render`).
    pub color: Option<Rgb>,
    pub dimmer: f32,
    pub rgbw: RgbwMode,
}

impl Default for FixtureLook {
    /// Dimmer full and the colour from below, so setting just a colour lights the fixture and
    /// setting just a dimmer keeps its colour.
    fn default() -> Self {
        FixtureLook {
            color: None,
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
            if color.is_some() {
                look.color = color;
            }
            if let Some(Level(dimmer)) = settings.dimmer {
                look.dimmer = dimmer;
            }
            if let Some(rgbw) = settings.rgbw {
                if look.color.is_none() {
                    return Err(format!(
                        "rgbw needs a color for {:?}: without one the fixture keeps the colour \
                         from below, already converted; add a color or remove rgbw",
                        fixture.name
                    ));
                }
                look.rgbw = rgbw;
            }
        }
        Ok(())
    }
}

/// A palette name, or a literal `#rrggbb`.
pub(crate) fn colour(show: &Show, name: &str) -> Result<Rgb, String> {
    if name.starts_with('#') {
        return Rgb::from_hex(name);
    }
    show.palettes
        .get(name)
        .copied()
        .ok_or_else(|| format!("no colour {name:?} in palettes.yaml, and not a #rrggbb value"))
}

/// The virtual dimmer of every fixture without a dimmer channel, such as the Wild Wash in 16ch:
/// each colour emitter's level at full and the intensity it shows at, kept beside the Enttec
/// frame, whose DMX is only their product. A look that sets just a dimmer reads the colour at
/// full here, so on these fixtures too it can bring a dim scene to full, or one out of a
/// blackout, as a dimmer channel does. Indexed like the frame; only those emitters' entries
/// are used.
#[derive(Debug, Clone)]
pub struct VirtualDimmers {
    at_full: [f32; DMX_CHANNELS],
    intensity: [f32; DMX_CHANNELS],
}

impl VirtualDimmers {
    /// `frame` as drawn straight to DMX, like legacy notes: each value is a colour at full,
    /// shown at intensity 1.
    pub fn of(frame: &Frame) -> Self {
        VirtualDimmers {
            at_full: frame.map(|value| f32::from(value) / 255.0),
            intensity: [1.0; DMX_CHANNELS],
        }
    }

    /// Moves each entry towards `above`'s by `opacity` (0–1), as the frames crossfade.
    pub fn crossfade(&mut self, above: &VirtualDimmers, opacity: f32) {
        let mix = |below: &mut f32, above: f32| *below += (above - *below) * opacity;
        for (below, above) in self.at_full.iter_mut().zip(above.at_full) {
            mix(below, above);
        }
        for (below, above) in self.intensity.iter_mut().zip(above.intensity) {
            mix(below, above);
        }
    }

    /// Records the emitter on `channel` (0-based) as `at_full`, shown at `intensity`.
    pub(crate) fn set(&mut self, channel: usize, at_full: f32, intensity: f32) {
        self.at_full[channel] = at_full;
        self.intensity[channel] = intensity;
    }
}

/// Draws `look` over `frame`, which holds the layers below, with their virtual `dimmers`.
/// Other fixtures are untouched.
///
/// A fixture given a colour has every channel overwritten: colour and dimmer from the look, the
/// rest at their profile default. A fixture with a dimmer channel gets the look's dimmer there;
/// one without has its colour channels scaled by it.
///
/// A fixture given only a dimmer keeps everything below but its intensity: its dimmer channel
/// is set, or without one its virtual dimmer, which shows the colour from below at full at that
/// level. With nothing below it stays dark.
pub fn render(rig: &Rig, look: &Look, frame: &mut Frame, dimmers: &mut VirtualDimmers) {
    for fixture in &rig.fixtures {
        let Some(fixture_look) = look.fixtures.get(&fixture.name) else {
            continue;
        };
        match fixture_look.color {
            Some(color) => draw_fixture(
                fixture,
                fixture_look.dimmer,
                fixture_look.rgbw,
                |_: &Pixel| color,
                frame,
                dimmers,
            ),
            None => dim_fixture(fixture, fixture_look.dimmer, frame, dimmers),
        }
    }
}

/// Sets the intensity of a fixture over the colour below: its dimmer channel, or without one
/// its virtual dimmer.
fn dim_fixture(fixture: &Fixture, dimmer: f32, frame: &mut Frame, dimmers: &mut VirtualDimmers) {
    let start = fixture.address - 1;
    match fixture.dimmer {
        Some(offset) => frame[start + offset] = to_dmx(dimmer),
        None => {
            for channel in colour_channels(fixture).map(|offset| start + offset) {
                let at_full = dimmers.at_full[channel];
                dimmers.set(channel, at_full, dimmer);
                frame[channel] = to_dmx(at_full * dimmer);
            }
        }
    }
}

/// Offsets from the fixture's address of every pixel's colour emitters.
fn colour_channels(fixture: &Fixture) -> impl Iterator<Item = usize> + '_ {
    fixture.pixels.iter().flat_map(|pixel| {
        let channels = pixel.channels;
        [channels.red, channels.green, channels.blue, channels.white]
            .into_iter()
            .flatten()
    })
}

fn scale(value: &mut u8, factor: f32) {
    *value = (f32::from(*value) * factor).round() as u8;
}

/// Draws one fixture over `frame`: every channel at its profile default, then `dimmer` and
/// each pixel's colour. A fixture with a dimmer channel gets `dimmer` there; one without has
/// its colour channels scaled by it, which its virtual dimmer records.
fn draw_fixture(
    fixture: &Fixture,
    dimmer: f32,
    rgbw: RgbwMode,
    colour: impl Fn(&Pixel) -> Rgb,
    frame: &mut Frame,
    dimmers: &mut VirtualDimmers,
) {
    let start = fixture.address - 1;
    for (offset, channel) in fixture.channels.iter().enumerate() {
        frame[start + offset] = channel.default;
    }
    let colour_scale = match fixture.dimmer {
        Some(offset) => {
            frame[start + offset] = to_dmx(dimmer);
            1.0
        }
        None => dimmer,
    };
    for pixel in &fixture.pixels {
        let emitted = draw_pixel(
            pixel.channels,
            start,
            colour(pixel),
            rgbw,
            colour_scale,
            frame,
        );
        if fixture.dimmer.is_none() {
            record_virtual_dimmer(start, emitted, colour_scale, dimmers);
        }
    }
}

/// Records a pixel of a fixture without a dimmer channel, drawn as `emitted` at `intensity`.
pub(crate) fn record_virtual_dimmer(
    start: usize,
    emitted: Emitted,
    intensity: f32,
    dimmers: &mut VirtualDimmers,
) {
    for (offset, at_full) in emitted {
        if let Some(offset) = offset {
            dimmers.set(start + offset, at_full, intensity);
        }
    }
}

/// A pixel's emitters, as offsets from its fixture's start, and their levels at full.
pub(crate) type Emitted = [(Option<usize>, f32); 4];

/// Writes `colour`, scaled by `scale`, to a pixel's emitters at `channels` from `start`, and
/// returns their levels at full. `rgbw` only applies to a pixel with a white emitter.
pub(crate) fn draw_pixel(
    channels: PixelChannels,
    start: usize,
    colour: Rgb,
    rgbw: RgbwMode,
    scale: f32,
    frame: &mut Frame,
) -> Emitted {
    let mode = match channels.white {
        Some(_) => rgbw,
        None => RgbwMode::Saturated,
    };
    let emitted = mode.convert(colour);
    let emitters = [
        (channels.red, emitted.red),
        (channels.green, emitted.green),
        (channels.blue, emitted.blue),
        (channels.white, emitted.white),
    ];
    for (offset, level) in emitters {
        if let Some(offset) = offset {
            frame[start + offset] = to_dmx(level * scale);
        }
    }
    emitters
}

/// Scales the intensity `frame` gives `fixtures` by `factor` (0–1), as a look's dimmer does:
/// the dimmer channel of a fixture with one, the colour channels and virtual dimmer of one
/// without.
pub fn scale_intensity(
    rig: &Rig,
    fixtures: &BTreeSet<String>,
    factor: f32,
    frame: &mut Frame,
    dimmers: &mut VirtualDimmers,
) {
    for fixture in rig
        .fixtures
        .iter()
        .filter(|fixture| fixtures.contains(&fixture.name))
    {
        let start = fixture.address - 1;
        match fixture.dimmer {
            Some(offset) => scale(&mut frame[start + offset], factor),
            None => {
                for channel in colour_channels(fixture).map(|offset| start + offset) {
                    scale(&mut frame[channel], factor);
                    dimmers.intensity[channel] *= factor;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Attribute, Channel};
    use crate::rig::{Fixture, Pixel};

    fn channel(name: &str, attribute: Option<Attribute>, default: u8) -> Channel {
        Channel {
            name: name.into(),
            attribute,
            pixel: None,
            default,
        }
    }

    fn fixture(name: &str, address: usize, channels: Vec<Channel>) -> Fixture {
        let mode = crate::fixture::Mode {
            name: "mode".into(),
            channels: channels.clone(),
        };
        let (dimmer, pixel_channels) = mode.layout().unwrap();
        Fixture {
            name: name.into(),
            profile: name.into(),
            mode: "mode".into(),
            address,
            channels,
            dimmer,
            pixels: vec![Pixel {
                x: 0.5,
                y: 0.5,
                channels: pixel_channels[0],
            }],
        }
    }

    /// One RGBW par at DMX 10: dimmer, red, green, blue, white, then a shutter that must
    /// sit at 255 to open. And at DMX 20, an RGB wash with no dimmer channel, as the Wild Wash
    /// in 16ch.
    fn par_show() -> Show {
        let par = fixture(
            "par",
            10,
            vec![
                channel("Dimmer", Some(Attribute::Dimmer), 0),
                channel("Red", Some(Attribute::Red), 0),
                channel("Green", Some(Attribute::Green), 0),
                channel("Blue", Some(Attribute::Blue), 0),
                channel("White", Some(Attribute::White), 0),
                channel("Shutter", None, 255),
            ],
        );
        let wash = fixture(
            "wash",
            20,
            vec![
                channel("Red", Some(Attribute::Red), 0),
                channel("Green", Some(Attribute::Green), 0),
                channel("Blue", Some(Attribute::Blue), 0),
            ],
        );
        Show {
            rig: Rig {
                fixtures: vec![par, wash],
                groups: BTreeMap::from([("everything".into(), vec!["par".into()])]),
                ..Rig::default()
            },
            palettes: BTreeMap::from([("pink".into(), Rgb::from_hex("#ff8080").unwrap())]),
            ..Show::default()
        }
    }

    fn set(show: &Show, look: &mut Look, target: &str, pairs: &[&str]) -> Result<(), String> {
        let pairs: Vec<String> = pairs.iter().map(|pair| pair.to_string()).collect();
        look.set(show, target, &LookSettings::from_pairs(&pairs)?)
    }

    /// `look` over `below`, drawn straight to DMX.
    fn rendered(show: &Show, look: &Look, below: Frame) -> Frame {
        let mut frame = below;
        render(&show.rig, look, &mut frame, &mut VirtualDimmers::of(&below));
        frame
    }

    /// Each look over the ones before it, from darkness, as cue layers stack.
    fn stacked(show: &Show, looks: &[&[(&str, &[&str])]]) -> Frame {
        let mut frame = [0; DMX_CHANNELS];
        let mut dimmers = VirtualDimmers::of(&frame);
        for entries in looks {
            let mut look = Look::default();
            for (target, pairs) in *entries {
                set(show, &mut look, target, pairs).unwrap();
            }
            render(&show.rig, &look, &mut frame, &mut dimmers);
        }
        frame
    }

    /// The par's dimmer, red, green, blue, and the wash's red, green, blue.
    fn par_and_wash(frame: Frame) -> [u8; 7] {
        [
            frame[9], frame[10], frame[11], frame[12], frame[19], frame[20], frame[21],
        ]
    }

    const PINK_AT_60: &[(&str, &[&str])] = &[
        ("par", &["color=pink", "dimmer=60%"]),
        ("wash", &["color=pink", "dimmer=60%"]),
    ];

    #[test]
    fn a_dimmer_alone_shows_the_colour_below_at_its_level_on_both_kinds_of_fixture() {
        let show = par_show();
        let full: &[(&str, &[&str])] = &[("par", &["dimmer=100%"]), ("wash", &["dimmer=100%"])];
        let half: &[(&str, &[&str])] = &[("par", &["dimmer=50%"]), ("wash", &["dimmer=50%"])];

        assert_eq!(
            par_and_wash(stacked(&show, &[PINK_AT_60])),
            [153, 255, 128, 128, 153, 77, 77],
            "pink at 60%: on the dimmer channel, or the colour scaled"
        );
        assert_eq!(
            par_and_wash(stacked(&show, &[PINK_AT_60, full])),
            [255, 255, 128, 128, 255, 128, 128],
            "brought to full alike"
        );
        assert_eq!(
            par_and_wash(stacked(&show, &[PINK_AT_60, half])),
            [128, 255, 128, 128, 128, 64, 64],
            "to 50% alike, not 50% of the 60%"
        );
    }

    #[test]
    fn a_dimmer_alone_brings_a_scene_back_out_of_a_blackout() {
        let show = par_show();
        let blackout: &[(&str, &[&str])] = &[("par", &["dimmer=0%"]), ("wash", &["dimmer=0%"])];
        let full: &[(&str, &[&str])] = &[("par", &["dimmer=100%"]), ("wash", &["dimmer=100%"])];

        assert_eq!(
            par_and_wash(stacked(&show, &[PINK_AT_60, blackout])),
            [0, 255, 128, 128, 0, 0, 0]
        );
        assert_eq!(
            par_and_wash(stacked(&show, &[PINK_AT_60, blackout, full])),
            [255, 255, 128, 128, 255, 128, 128]
        );
    }

    #[test]
    fn a_dim_colour_at_full_stays_as_written() {
        let show = par_show();
        let grey: &[(&str, &[&str])] = &[("wash", &["color=#404040"])];
        let full: &[(&str, &[&str])] = &[("wash", &["dimmer=100%"])];

        assert_eq!(par_and_wash(stacked(&show, &[grey]))[4..], [64, 64, 64]);
        assert_eq!(
            par_and_wash(stacked(&show, &[grey, full]))[4..],
            [64, 64, 64]
        );
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
    fn a_dimmer_alone_sets_the_dimmer_channel_over_the_colour_below() {
        let show = par_show();
        let mut look = Look::default();
        let mut below = [0; DMX_CHANNELS];
        // The par at 30%, pink, its shutter open.
        below[9..15].copy_from_slice(&[77, 255, 128, 128, 0, 255]);

        set(&show, &mut look, "par", &["dimmer=100%"]).unwrap();

        assert_eq!(
            rendered(&show, &look, below)[9..15],
            [255, 255, 128, 128, 0, 255]
        );
        assert_eq!(
            rendered(&show, &look, [0; DMX_CHANNELS])[9..15],
            [255, 0, 0, 0, 0, 0],
            "nothing below: no colour to light"
        );
    }

    #[test]
    fn a_dimmer_alone_scales_the_colour_below_on_a_fixture_without_a_dimmer_channel() {
        let show = par_show();
        let mut look = Look::default();
        let mut below = [0; DMX_CHANNELS];
        below[19..22].copy_from_slice(&[200, 100, 0]);

        set(&show, &mut look, "wash", &["dimmer=50%"]).unwrap();
        assert_eq!(rendered(&show, &look, below)[19..22], [100, 50, 0]);

        set(&show, &mut look, "wash", &["dimmer=100%"]).unwrap();
        assert_eq!(
            rendered(&show, &look, below),
            below,
            "the colour as it is below"
        );
        assert_eq!(rendered(&show, &look, [0; DMX_CHANNELS]), [0; DMX_CHANNELS]);
    }

    #[test]
    fn a_dimmer_after_a_colour_in_the_same_look_keeps_that_colour() {
        let show = par_show();
        let mut look = Look::default();

        set(&show, &mut look, "wash", &["color=pink"]).unwrap();
        set(&show, &mut look, "wash", &["dimmer=50%"]).unwrap();

        assert_eq!(
            rendered(&show, &look, [0; DMX_CHANNELS])[19..22],
            [128, 64, 64]
        );
    }

    #[test]
    fn rgbw_without_a_colour_to_convert_is_refused() {
        let show = par_show();
        let mut look = Look::default();

        assert_eq!(
            set(&show, &mut look, "par", &["dimmer=50%", "rgbw=white-rich"]).unwrap_err(),
            "rgbw needs a color for \"par\": without one the fixture keeps the colour from below, \
             already converted; add a color or remove rgbw"
        );
        set(&show, &mut look, "par", &["color=pink"]).unwrap();
        set(&show, &mut look, "par", &["rgbw=white-rich"]).unwrap();
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

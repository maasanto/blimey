use std::fs;
use std::path::Path;

use blimey_engine::fixture::{Attribute, FixtureProfile, Mode};
use blimey_engine::ofl::import_ofl;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const WILD_WASH_OFL: &str = "shows/fossile/fixtures/stairville-wild-wash-pro-648-rgbw.ofl.json";

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(ROOT).join(path)).unwrap()
}

/// The manufacturer's 16Ch table as (channel, attribute, segment), parsed from the verbatim
/// function text, e.g. "Intensity red, segment 1 (0 % to 100 %)".
fn manual_16ch_table() -> Vec<(usize, Attribute, usize)> {
    read("tests/fixtures/manuals/stairville-wild-wash-pro-648-rgbw-16ch.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (channel, function) = line.split_once('\t').unwrap();
            let rest = function.strip_prefix("Intensity ").unwrap();
            let (colour, rest) = rest.split_once(", segment ").unwrap();
            let segment = rest.split_whitespace().next().unwrap();
            let attribute = match colour {
                "red" => Attribute::Red,
                "green" => Attribute::Green,
                "blue" => Attribute::Blue,
                "white" => Attribute::White,
                other => panic!("unexpected colour {other:?} in the manual table"),
            };
            (
                channel.parse().unwrap(),
                attribute,
                segment.parse().unwrap(),
            )
        })
        .collect()
}

fn assert_matches_manual(mode: &Mode, which: &str) {
    let table = manual_16ch_table();
    assert_eq!(table.len(), 16, "the manual lists 16 channels");
    assert_eq!(mode.channels.len(), table.len(), "{which}: channel count");
    for (channel, attribute, segment) in table {
        let actual = &mode.channels[channel - 1];
        assert_eq!(
            (actual.attribute, actual.pixel),
            (Some(attribute), Some(segment)),
            "{which}: channel {channel} ({})",
            actual.name
        );
    }
    let (dimmer, _) = mode.layout().unwrap();
    assert_eq!(dimmer, None, "{which}: 16Ch has no dimmer channel");
}

#[test]
fn wild_wash_16ch_profile_matches_the_manufacturer_table() {
    let imported = import_ofl(&read(WILD_WASH_OFL)).unwrap();
    assert_matches_manual(imported.mode("16ch").unwrap(), "OFL import");

    for profile_path in [
        "shows/fossile/fixtures/stairville-wild-wash-pro-648-rgbw.yaml",
        "tests/fixtures/shows/four-washes/fixtures/stairville-wild-wash-pro-648-rgbw.yaml",
    ] {
        let profile = FixtureProfile::load(&Path::new(ROOT).join(profile_path)).unwrap();
        assert_eq!(
            profile, imported,
            "{profile_path} is stale; re-run blimey fixture import"
        );
        assert_matches_manual(profile.mode("16ch").unwrap(), profile_path);
    }
}

#[test]
fn imports_an_upstream_ofl_file() {
    let profile = import_ofl(&read(
        "tests/fixtures/ofl/stairville-wild-wash-648-led-rgb-dmx.json",
    ))
    .unwrap();

    let mode = profile.mode("6ch").unwrap();
    let channels: Vec<_> = mode
        .channels
        .iter()
        .map(|channel| (channel.name.as_str(), channel.attribute))
        .collect();
    assert_eq!(
        channels,
        [
            ("Dimmer", Some(Attribute::Dimmer)),
            ("Shutter / Strobe Advanced", None),
            ("Red", Some(Attribute::Red)),
            ("Green", Some(Attribute::Green)),
            ("Blue", Some(Attribute::Blue)),
            ("Sound Sensitivity", None),
        ]
    );
    assert_eq!(profile.modes.len(), 8);
}

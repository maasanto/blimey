//! Plain-text frame dumps, specified in docs/frame-dump.md.

use std::fmt::Write;
use std::time::Duration;

use crate::{ArtNetFrames, DMX_CHANNELS, Frame, RecordedFrame};

const HEADER: &str = "# blimey frame dump v1";
const DARK: Frame = [0; DMX_CHANNELS];

pub fn format_dump(frames: &[RecordedFrame]) -> String {
    let mut dump = format!("{HEADER}\n");
    for frame in frames {
        write!(dump, "{}", frame.at.as_millis()).unwrap();
        write_channels(&mut dump, "", &frame.values);
        for (universe, values) in &frame.artnet {
            write_channels(&mut dump, &format!("{universe}:"), values);
        }
        dump.push('\n');
    }
    dump
}

/// ` <prefix><channel>=<value>` for each channel that is not zero.
fn write_channels(dump: &mut String, prefix: &str, values: &Frame) {
    for (index, value) in values.iter().enumerate() {
        if *value != 0 {
            write!(dump, " {prefix}{}={value}", index + 1).unwrap();
        }
    }
}

pub fn parse_dump(text: &str) -> Result<Vec<RecordedFrame>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.starts_with('#'))
        .map(|(index, line)| {
            parse_frame(line).map_err(|error| format!("line {}: {error}: {line:?}", index + 1))
        })
        .collect()
}

fn parse_frame(line: &str) -> Result<RecordedFrame, String> {
    let mut fields = line.split_whitespace();
    let millis = fields
        .next()
        .and_then(|time| time.parse().ok())
        .ok_or("time must be a whole number of milliseconds")?;
    let mut values = DARK;
    let mut artnet = ArtNetFrames::new();
    for field in fields {
        let (channel, value) = field.split_once('=').ok_or("expected channel=value")?;
        let (frame, channel) = match channel.split_once(':') {
            None => (&mut values, channel),
            Some((universe, channel)) => {
                let universe: u16 = universe
                    .parse()
                    .map_err(|_| "Art-Net universe must be a number")?;
                (artnet.entry(universe).or_insert(DARK), channel)
            }
        };
        let channel: usize = channel
            .parse()
            .ok()
            .filter(|channel| (1..=DMX_CHANNELS).contains(channel))
            .ok_or("channel must be 1-512")?;
        frame[channel - 1] = value.parse().map_err(|_| "value must be 0-255")?;
    }
    Ok(RecordedFrame {
        at: Duration::from_millis(millis),
        values,
        artnet,
    })
}

/// Describes the first frame and channel where `actual` departs from `expected`, or `None` if they match.
/// An Art-Net universe missing from one side counts as dark.
pub fn first_difference(actual: &[RecordedFrame], expected: &[RecordedFrame]) -> Option<String> {
    for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
        let millis = want.at.as_millis();
        if got.at != want.at {
            return Some(format!(
                "frame {index}: expected time {millis} ms, got {} ms",
                got.at.as_millis()
            ));
        }
        let mut pairs = vec![(String::new(), &got.values, &want.values)];
        for universe in got.artnet.keys().chain(want.artnet.keys()) {
            pairs.push((
                format!("{universe}:"),
                got.artnet.get(universe).unwrap_or(&DARK),
                want.artnet.get(universe).unwrap_or(&DARK),
            ));
        }
        for (prefix, got, want) in pairs {
            if let Some(channel) = (0..DMX_CHANNELS).find(|&channel| got[channel] != want[channel])
            {
                return Some(format!(
                    "frame {index} ({millis} ms), channel {prefix}{}: expected {}, got {}",
                    channel + 1,
                    want[channel],
                    got[channel]
                ));
            }
        }
    }
    (actual.len() != expected.len())
        .then(|| format!("expected {} frames, got {}", expected.len(), actual.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(millis: u64, set: &[(usize, u8)]) -> RecordedFrame {
        let mut values = DARK;
        for &(channel, value) in set {
            values[channel - 1] = value;
        }
        RecordedFrame {
            at: Duration::from_millis(millis),
            values,
            artnet: ArtNetFrames::new(),
        }
    }

    #[test]
    fn lists_only_non_zero_channels_one_based() {
        let frames = [frame(0, &[]), frame(25, &[(1, 255), (512, 7)])];

        assert_eq!(
            format_dump(&frames),
            "# blimey frame dump v1\n0\n25 1=255 512=7\n"
        );
    }

    #[test]
    fn parses_what_it_formats() {
        let mut with_artnet = frame(50, &[(6, 128), (21, 1)]);
        let mut universe = DARK;
        universe[0] = 9;
        with_artnet.artnet.insert(3, universe);
        let frames = vec![frame(0, &[(6, 254)]), frame(25, &[]), with_artnet];

        let dump = format_dump(&frames);

        assert!(dump.ends_with("\n50 6=128 21=1 3:1=9\n"), "{dump}");
        assert_eq!(parse_dump(&dump), Ok(frames));
    }

    #[test]
    fn parse_errors_name_the_line() {
        let error = parse_dump("# header\n0\n25 513=1\n").unwrap_err();

        assert_eq!(error, "line 3: channel must be 1-512: \"25 513=1\"");
    }

    #[test]
    fn reports_first_differing_frame_and_channel() {
        let expected = [frame(0, &[]), frame(25, &[(6, 200)]), frame(50, &[(1, 1)])];
        let actual = [frame(0, &[]), frame(25, &[(6, 199)]), frame(50, &[(2, 1)])];

        assert_eq!(
            first_difference(&actual, &expected).as_deref(),
            Some("frame 1 (25 ms), channel 6: expected 200, got 199")
        );
    }

    #[test]
    fn a_dark_art_net_universe_matches_a_missing_one_and_differences_name_the_universe() {
        let mut dark = frame(0, &[]);
        dark.artnet.insert(2, DARK);
        let mut lit = dark.clone();
        lit.artnet.get_mut(&2).unwrap()[9] = 40;

        assert_eq!(first_difference(&[dark.clone()], &[frame(0, &[])]), None);
        assert_eq!(
            first_difference(&[dark], &[lit]).as_deref(),
            Some("frame 0 (0 ms), channel 2:10: expected 40, got 0")
        );
    }

    #[test]
    fn reports_timing_and_length_differences() {
        let expected = [frame(0, &[]), frame(25, &[])];

        assert_eq!(
            first_difference(&[frame(0, &[]), frame(30, &[])], &expected).as_deref(),
            Some("frame 1: expected time 25 ms, got 30 ms")
        );
        assert_eq!(
            first_difference(&expected[..1], &expected).as_deref(),
            Some("expected 2 frames, got 1")
        );
        assert_eq!(first_difference(&expected, &expected), None);
    }
}

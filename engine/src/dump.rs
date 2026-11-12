//! Plain-text frame dumps, specified in docs/frame-dump.md.

use std::fmt::Write;
use std::time::Duration;

use crate::{DMX_CHANNELS, RecordedFrame};

const HEADER: &str = "# strata frame dump v1";

pub fn format_dump(frames: &[RecordedFrame]) -> String {
    let mut dump = format!("{HEADER}\n");
    for frame in frames {
        write!(dump, "{}", frame.at.as_millis()).unwrap();
        for (index, value) in frame.values.iter().enumerate() {
            if *value != 0 {
                write!(dump, " {}={value}", index + 1).unwrap();
            }
        }
        dump.push('\n');
    }
    dump
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
    let mut values = [0; DMX_CHANNELS];
    for field in fields {
        let (channel, value) = field.split_once('=').ok_or("expected channel=value")?;
        let channel: usize = channel
            .parse()
            .ok()
            .filter(|channel| (1..=DMX_CHANNELS).contains(channel))
            .ok_or("channel must be 1-512")?;
        values[channel - 1] = value.parse().map_err(|_| "value must be 0-255")?;
    }
    Ok(RecordedFrame {
        at: Duration::from_millis(millis),
        values,
    })
}

/// Describes the first frame and channel where `actual` departs from `expected`, or `None` if they match.
pub fn first_difference(actual: &[RecordedFrame], expected: &[RecordedFrame]) -> Option<String> {
    for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
        let millis = want.at.as_millis();
        if got.at != want.at {
            return Some(format!(
                "frame {index}: expected time {millis} ms, got {} ms",
                got.at.as_millis()
            ));
        }
        if let Some(channel) =
            (0..DMX_CHANNELS).find(|&channel| got.values[channel] != want.values[channel])
        {
            return Some(format!(
                "frame {index} ({millis} ms), channel {}: expected {}, got {}",
                channel + 1,
                want.values[channel],
                got.values[channel]
            ));
        }
    }
    (actual.len() != expected.len())
        .then(|| format!("expected {} frames, got {}", expected.len(), actual.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(millis: u64, set: &[(usize, u8)]) -> RecordedFrame {
        let mut values = [0; DMX_CHANNELS];
        for &(channel, value) in set {
            values[channel - 1] = value;
        }
        RecordedFrame {
            at: Duration::from_millis(millis),
            values,
        }
    }

    #[test]
    fn lists_only_non_zero_channels_one_based() {
        let frames = [frame(0, &[]), frame(25, &[(1, 255), (512, 7)])];

        assert_eq!(
            format_dump(&frames),
            "# strata frame dump v1\n0\n25 1=255 512=7\n"
        );
    }

    #[test]
    fn parses_what_it_formats() {
        let frames = vec![
            frame(0, &[(6, 254)]),
            frame(25, &[]),
            frame(50, &[(6, 128), (21, 1)]),
        ];

        assert_eq!(parse_dump(&format_dump(&frames)), Ok(frames));
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

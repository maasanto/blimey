//! Legacy raw notes (plan §5, channel 1): a note sets one DMX channel, as in the EMU sessions.

use std::io;

use serde::Deserialize;

use crate::{DMX_CHANNELS, Frame};

const NOTE_ON: u8 = 0x90;
const NOTE_OFF: u8 = 0x80;
const LEGACY_CHANNEL: u8 = 0;
const NOTES: usize = 128;

/// `legacy-map.yaml`: which channel-1 notes drive which DMX channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyMap {
    /// Zero-based DMX channel for each note number.
    channels: [Option<u16>; NOTES],
}

impl Default for LegacyMap {
    fn default() -> Self {
        Self {
            channels: [None; NOTES],
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyMapFile {
    ranges: Vec<NoteRange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteRange {
    first_note: u8,
    last_note: u8,
    /// DMX channel (1-512) driven by `first_note`; the following notes drive the following channels.
    address: u16,
}

impl LegacyMap {
    pub fn parse(yaml: &str) -> io::Result<Self> {
        let file: LegacyMapFile =
            serde_yaml_ng::from_str(yaml).map_err(|error| invalid(error.to_string()))?;
        let mut map = Self::default();
        for range in file.ranges {
            let NoteRange {
                first_note,
                last_note,
                address,
            } = range;
            let name = format!("range {first_note}-{last_note}");
            if first_note > last_note || usize::from(last_note) >= NOTES {
                return Err(invalid(format!(
                    "{name}: notes must be 0-127 with first_note <= last_note"
                )));
            }
            let last_address = usize::from(address) + usize::from(last_note - first_note);
            if address == 0 || last_address > DMX_CHANNELS {
                return Err(invalid(format!(
                    "{name}: address {address} puts the range on channels {address}-{last_address}, outside 1-{DMX_CHANNELS}"
                )));
            }
            for (offset, note) in (first_note..=last_note).enumerate() {
                let slot = &mut map.channels[usize::from(note)];
                if slot.is_some() {
                    return Err(invalid(format!(
                        "{name}: note {note} is already mapped by another range"
                    )));
                }
                *slot = Some(address - 1 + offset as u16);
            }
        }
        Ok(map)
    }

    /// Applies one MIDI message to the legacy layer's levels; anything but a mapped channel-1 note is ignored.
    pub fn apply(&self, message: &[u8], levels: &mut Frame) {
        let &[status, note, velocity] = message else {
            return;
        };
        if status & 0x0F != LEGACY_CHANNEL {
            return;
        }
        let level = match status & 0xF0 {
            NOTE_ON => level_for_velocity(velocity),
            NOTE_OFF => 0,
            _ => return,
        };
        if let Some(Some(channel)) = self.channels.get(usize::from(note)) {
            levels[usize::from(*channel)] = level;
        }
    }
}

/// Velocity × 2, except the top velocity 127 gives full 255 rather than 254, so a full-velocity
/// note means full on. A note-on with velocity 0 is a note-off, per the MIDI spec.
fn level_for_velocity(velocity: u8) -> u8 {
    if velocity >= 127 { 255 } else { velocity * 2 }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOSSILE_WASHES: &str = "ranges:\n  - { first_note: 37, last_note: 52, address: 6 }\n";

    fn levels_after(map: &LegacyMap, messages: &[&[u8]]) -> Frame {
        let mut levels = [0; DMX_CHANNELS];
        for message in messages {
            map.apply(message, &mut levels);
        }
        levels
    }

    #[test]
    fn notes_37_to_52_drive_channels_6_to_21_at_twice_the_velocity() {
        let map = LegacyMap::parse(FOSSILE_WASHES).unwrap();

        let levels = levels_after(&map, &[&[0x90, 37, 100], &[0x90, 52, 1]]);

        assert_eq!(levels[5], 200, "note 37 → channel 6");
        assert_eq!(levels[20], 2, "note 52 → channel 21");
        assert_eq!(levels.iter().filter(|level| **level != 0).count(), 2);
    }

    #[test]
    fn velocity_127_is_full_on() {
        let map = LegacyMap::parse(FOSSILE_WASHES).unwrap();

        assert_eq!(levels_after(&map, &[&[0x90, 40, 127]])[8], 255);
        assert_eq!(levels_after(&map, &[&[0x90, 40, 126]])[8], 252);
    }

    #[test]
    fn note_off_and_zero_velocity_note_on_go_dark() {
        let map = LegacyMap::parse(FOSSILE_WASHES).unwrap();

        assert_eq!(
            levels_after(&map, &[&[0x90, 40, 90], &[0x80, 40, 64]])[8],
            0
        );
        assert_eq!(levels_after(&map, &[&[0x90, 40, 90], &[0x90, 40, 0]])[8], 0);
    }

    #[test]
    fn retriggered_note_takes_the_new_velocity_and_one_note_off_ends_it() {
        let map = LegacyMap::parse(FOSSILE_WASHES).unwrap();

        let retriggered = levels_after(&map, &[&[0x90, 40, 100], &[0x90, 40, 30]]);
        let released = levels_after(&map, &[&[0x90, 40, 100], &[0x90, 40, 30], &[0x80, 40, 0]]);

        assert_eq!(retriggered[8], 60);
        assert_eq!(released[8], 0);
    }

    #[test]
    fn ignores_other_channels_unmapped_notes_and_other_messages() {
        let map = LegacyMap::parse(FOSSILE_WASHES).unwrap();

        let levels = levels_after(
            &map,
            &[
                &[0x91, 40, 100], // note on, channel 2
                &[0x90, 36, 100], // unmapped note
                &[0xB0, 40, 100], // control change
                &[0xC0, 3],       // program change
                &[0xF8],          // clock
            ],
        );

        assert_eq!(levels, [0; DMX_CHANNELS]);
    }

    #[test]
    fn rejects_ranges_that_overlap_or_leave_the_universe() {
        let overlap = "ranges:\n  - { first_note: 37, last_note: 52, address: 6 }\n  - { first_note: 52, last_note: 53, address: 100 }\n";
        let past_512 = "ranges:\n  - { first_note: 0, last_note: 1, address: 512 }\n";
        let typo = "ranges:\n  - { first_note: 37, last_note: 52, adress: 6 }\n";

        let message = |yaml| LegacyMap::parse(yaml).unwrap_err().to_string();

        assert_eq!(
            message(overlap),
            "range 52-53: note 52 is already mapped by another range"
        );
        assert_eq!(
            message(past_512),
            "range 0-1: address 512 puts the range on channels 512-513, outside 1-512"
        );
        assert!(
            message(typo).contains("unknown field `adress`"),
            "{}",
            message(typo)
        );
    }
}

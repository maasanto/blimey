use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use midly::{Format, MetaMessage, Smf, Timing, TrackEventKind};

use crate::{MidiEvent, MidiSource};

/// SMF default when a file sets no tempo: 120 BPM.
const DEFAULT_MICROS_PER_BEAT: u64 = 500_000;

/// A Standard MIDI File played back as a `MidiSource`, with tick times resolved through its tempo map.
pub struct MidiFile {
    events: VecDeque<MidiEvent>,
    duration: Duration,
}

impl MidiFile {
    pub fn open(path: &Path) -> io::Result<Self> {
        let with_path =
            |error: io::Error| io::Error::new(error.kind(), format!("{}: {error}", path.display()));
        Self::parse(&fs::read(path).map_err(with_path)?).map_err(with_path)
    }

    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let smf = Smf::parse(bytes)
            .map_err(|error| invalid(format!("not a valid MIDI file: {error}")))?;
        if smf.header.format == Format::Sequential {
            return Err(invalid(
                "format 2 (sequential) MIDI files are not supported; export as format 0 or 1"
                    .into(),
            ));
        }
        let ticks_per_beat = match smf.header.timing {
            Timing::Metrical(ticks) if ticks.as_int() > 0 => u64::from(ticks.as_int()),
            Timing::Metrical(_) => return Err(invalid("header declares 0 ticks per beat".into())),
            Timing::Timecode(..) => {
                return Err(invalid(
                    "SMPTE timecode MIDI files are not supported; export with musical (PPQ) timing"
                        .into(),
                ));
            }
        };

        // Tracks of a format 1 file play together; a stable sort keeps same-tick events in track order.
        let mut timed: Vec<(u64, TrackEventKind)> = smf
            .tracks
            .iter()
            .flat_map(|track| {
                track.iter().scan(0u64, |tick, event| {
                    *tick += u64::from(event.delta.as_int());
                    Some((*tick, event.kind))
                })
            })
            .collect();
        timed.sort_by_key(|(tick, _)| *tick);

        let mut events = VecDeque::new();
        let mut micros_per_beat = DEFAULT_MICROS_PER_BEAT;
        let (mut tempo_tick, mut tempo_micros) = (0u64, 0u64);
        let mut duration = Duration::ZERO;
        for (tick, kind) in timed {
            let micros = (tick - tempo_tick)
                .checked_mul(micros_per_beat)
                .map(|scaled| tempo_micros + scaled / ticks_per_beat)
                .ok_or_else(|| invalid("MIDI file too long".into()))?;
            let at = Duration::from_micros(micros);
            duration = at;
            if let TrackEventKind::Meta(MetaMessage::Tempo(tempo)) = kind {
                (tempo_tick, tempo_micros) = (tick, micros);
                micros_per_beat = u64::from(tempo.as_int());
            } else if let TrackEventKind::Escape(message) = kind {
                // SMF can only carry MIDI clock, Start, Stop and Song Position as escapes.
                events.push_back(MidiEvent {
                    at,
                    message: message.to_vec(),
                });
            } else if let Some(live) = kind.as_live_event() {
                let mut message = Vec::new();
                live.write_std(&mut message)?;
                events.push_back(MidiEvent { at, message });
            }
        }
        Ok(Self { events, duration })
    }

    /// Time of the file's last event, end-of-track markers included.
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

impl MidiSource for MidiFile {
    fn poll(&mut self, until: Duration) -> Option<MidiEvent> {
        if self.events.front()?.at <= until {
            self.events.pop_front()
        } else {
            None
        }
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Format 1, 480 PPQ. Tempo track: 120 BPM, then 80 BPM from tick 960 (1 s).
    /// Note track, 480 ticks apart: ch1 note 37 on, ch1 off, ch2 note 36 on, ch2 off.
    const TEMPO_CHANGE: &[u8] = include_bytes!("../../tests/fixtures/midi/tempo-change.mid");

    fn event(millis: u64, message: &[u8]) -> MidiEvent {
        MidiEvent {
            at: Duration::from_millis(millis),
            message: message.to_vec(),
        }
    }

    #[test]
    fn resolves_tick_times_through_tempo_changes() {
        let mut midi = MidiFile::parse(TEMPO_CHANGE).unwrap();

        let events: Vec<_> = std::iter::from_fn(|| midi.poll(Duration::MAX)).collect();

        assert_eq!(
            events,
            [
                event(0, &[0x90, 37, 127]),
                event(500, &[0x80, 37, 0]),
                event(1000, &[0x91, 36, 100]),
                event(1750, &[0x81, 36, 0]),
            ]
        );
        assert_eq!(midi.duration(), Duration::from_millis(1750));
    }

    #[test]
    fn poll_only_returns_events_that_are_due() {
        let mut midi = MidiFile::parse(TEMPO_CHANGE).unwrap();

        assert_eq!(midi.poll(Duration::ZERO), Some(event(0, &[0x90, 37, 127])));
        assert_eq!(midi.poll(Duration::from_millis(499)), None);
        assert_eq!(
            midi.poll(Duration::from_millis(500)),
            Some(event(500, &[0x80, 37, 0]))
        );
    }

    #[test]
    fn rejects_garbage_with_a_clear_error() {
        let error = MidiFile::parse(b"not midi").err().unwrap();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().starts_with("not a valid MIDI file"),
            "{error}"
        );
    }
}

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use midly::live::LiveEvent;
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

use crate::arrangement::{ArrangementEvent, EventKind, TICKS_PER_BEAT};
use crate::{MidiEvent, MidiSource};

/// SMF default when a file sets no tempo: 120 BPM.
const DEFAULT_MICROS_PER_BEAT: u64 = 500_000;
/// Channel 2, numbered from 0, as midly does.
const CUE_CHANNEL: u4 = u4::new(1);
/// The time signature meta event's usual metronome click, once a quarter note, and its
/// quarter note of eight 32nds.
const CLOCKS_PER_CLICK: u8 = 24;
const THIRTY_SECONDS_PER_BEAT: u8 = 8;
/// Files `MidiFile::encode` writes run at the default 120 BPM with this resolution: one tick is
/// 20 µs, and a 25 ms frame period is a whole number of ticks.
const ENCODED_TICKS_PER_BEAT: u16 = 25_000;
const ENCODED_TICK: Duration = Duration::from_micros(20);

/// A Standard MIDI File played back as a `MidiSource`, with tick times resolved through its tempo map.
#[derive(Clone)]
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

    /// A format 0 Standard MIDI File holding `events`, which `parse` reads back as the same
    /// messages. Times round up to the next 20 µs tick, so each event stays in the frame it was
    /// due in. Channel messages are stored as MIDI events, anything else (clock, Start, Stop) as
    /// SMF escapes.
    pub fn encode(events: &[MidiEvent]) -> io::Result<Vec<u8>> {
        let mut track = Vec::with_capacity(events.len() + 1);
        let mut previous_tick = 0;
        for event in events {
            let tick = event.at.as_nanos().div_ceil(ENCODED_TICK.as_nanos());
            let delta = tick
                .checked_sub(previous_tick)
                .ok_or_else(|| invalid(format!("event at {:?} is out of order", event.at)))?;
            let delta = u32::try_from(delta)
                .ok()
                .and_then(u28::try_from)
                .ok_or_else(|| {
                    invalid(format!(
                        "no MIDI for over 89 minutes before the event at {:?}",
                        event.at
                    ))
                })?;
            previous_tick = tick;
            let kind = match LiveEvent::parse(&event.message) {
                Ok(LiveEvent::Midi { channel, message }) => {
                    TrackEventKind::Midi { channel, message }
                }
                _ => TrackEventKind::Escape(&event.message),
            };
            track.push(TrackEvent { delta, kind });
        }
        write_track(ENCODED_TICKS_PER_BEAT, track)
    }

    /// A format 0 Standard MIDI File playing an arrangement's `events` at `bpm`, the channel-2
    /// protocol of plan §5 with the time signatures and markers as meta events. Times are the
    /// events' beats, on a grid of `arrangement::TICKS_PER_BEAT`.
    pub fn encode_arrangement(bpm: f64, events: &[ArrangementEvent]) -> io::Result<Vec<u8>> {
        let micros_per_beat = u24::try_from((60_000_000.0 / bpm).round() as u32)
            .ok_or_else(|| invalid(format!("{bpm} BPM is too slow for a MIDI file")))?;
        let mut track = vec![TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(micros_per_beat)),
        }];
        let mut previous_tick = 0;
        for event in events {
            let tick = (event.beats * f64::from(TICKS_PER_BEAT)).round() as u32;
            let delta = tick
                .checked_sub(previous_tick)
                .and_then(u28::try_from)
                .ok_or_else(|| invalid(format!("event at beat {} out of range", event.beats)))?;
            previous_tick = tick;
            let channel_2 = |message| TrackEventKind::Midi {
                channel: CUE_CHANNEL,
                message,
            };
            let kind = match &event.kind {
                EventKind::TimeSignature {
                    numerator,
                    denominator,
                } => TrackEventKind::Meta(MetaMessage::TimeSignature(
                    *numerator,
                    denominator.trailing_zeros() as u8,
                    CLOCKS_PER_CLICK,
                    THIRTY_SECONDS_PER_BEAT,
                )),
                EventKind::Marker { section } => {
                    TrackEventKind::Meta(MetaMessage::Marker(section.as_bytes()))
                }
                EventKind::ProgramChange { program } => channel_2(MidiMessage::ProgramChange {
                    program: u7::new(*program),
                }),
                EventKind::NoteOn { scene, velocity } => channel_2(MidiMessage::NoteOn {
                    key: u7::new(*scene),
                    vel: u7::new(*velocity),
                }),
                EventKind::NoteOff { scene } => channel_2(MidiMessage::NoteOff {
                    key: u7::new(*scene),
                    vel: u7::new(0),
                }),
            };
            track.push(TrackEvent { delta, kind });
        }
        write_track(TICKS_PER_BEAT, track)
    }
}

fn write_track(ticks_per_beat: u16, mut track: Vec<TrackEvent>) -> io::Result<Vec<u8>> {
    track.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    let smf = Smf {
        header: Header::new(
            Format::SingleTrack,
            Timing::Metrical(u15::new(ticks_per_beat)),
        ),
        tracks: vec![track],
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes)?;
    Ok(bytes)
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
    fn encode_round_trips_channel_and_clock_messages_on_a_20_us_grid() {
        let micros = |micros, message: &[u8]| MidiEvent {
            at: Duration::from_micros(micros),
            message: message.to_vec(),
        };
        let recorded = [
            micros(0, &[0xFA]),
            micros(10, &[0xF8]),
            micros(24_999, &[0xC1, 3]),
            micros(25_000, &[0x91, 36, 100]),
            micros(1_000_007, &[0x81, 36, 0]),
            micros(1_000_007, &[0xFC]),
        ];

        let mut midi = MidiFile::parse(&MidiFile::encode(&recorded).unwrap()).unwrap();
        let events: Vec<_> = std::iter::from_fn(|| midi.poll(Duration::MAX)).collect();

        assert_eq!(
            events,
            [
                micros(0, &[0xFA]),
                micros(20, &[0xF8]),
                micros(25_000, &[0xC1, 3]),
                micros(25_000, &[0x91, 36, 100]),
                micros(1_000_020, &[0x81, 36, 0]),
                micros(1_000_020, &[0xFC]),
            ],
            "times round up to the tick, so 24.999 ms stays in the 25 ms frame"
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

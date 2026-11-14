//! Blimey engine: turns timestamped MIDI events into DMX frames at a fixed 40 Hz.
//!
//! Time, MIDI input and DMX output are all injected, so the same loop runs
//! against real devices or, in tests, against a MIDI file, a fake clock and
//! a recording sink.

pub mod clock_follower;
pub mod colour;
pub mod cue;
mod dump;
pub mod duration;
pub mod envelope;
pub mod fixture;
mod legacy;
pub mod look;
mod midi_file;
pub mod ofl;
pub mod pattern;
pub mod pixel;
mod playback;
pub mod rig;
mod show;
pub mod show_file;
pub mod song;

pub use dump::{first_difference, format_dump, parse_dump};
pub use legacy::LegacyMap;
pub use midi_file::MidiFile;
pub use playback::{CueAction, CueEvent, CueStatus, Status, resume_messages};
pub use show::Show;

use std::collections::BTreeMap;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

pub const DMX_CHANNELS: usize = 512;
pub const FRAME_PERIOD: Duration = Duration::from_millis(25);

/// One universe of DMX values; index 0 is channel 1.
pub type Frame = [u8; DMX_CHANNELS];

/// Art-Net port-address → that universe's channels.
pub type ArtNetFrames = BTreeMap<u16, Frame>;

/// Where a channel goes out: the Enttec box's single universe, or an Art-Net universe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Universe {
    Enttec,
    /// The 15-bit Art-Net port-address.
    ArtNet(u16),
}

/// One rendered frame on every universe: the Enttec one, and the Art-Net ones the rig's pixel
/// grids use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Universes {
    pub enttec: Frame,
    pub artnet: ArtNetFrames,
}

impl Universes {
    pub fn frame_mut(&mut self, universe: Universe) -> &mut Frame {
        match universe {
            Universe::Enttec => &mut self.enttec,
            Universe::ArtNet(port_address) => {
                self.artnet.entry(port_address).or_insert([0; DMX_CHANNELS])
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiEvent {
    /// Time since the source started.
    pub at: Duration,
    /// One complete MIDI message, status byte first.
    pub message: Vec<u8>,
}

pub trait MidiSource {
    /// The next event timestamped at or before `until`, in order, or `None` if there is none yet.
    fn poll(&mut self, until: Duration) -> Option<MidiEvent>;
}

pub trait DmxSink {
    /// Outputs one frame. `at` is the clock time at which it was sent.
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()>;

    /// Outputs the Art-Net universes of the next frame sent, just before `send`. Sinks without
    /// Art-Net output ignore them. An error doesn't stop the render loop, as a network node
    /// may come back: the driver reports it, and a wrapping sink, such as the soak's, can
    /// count it.
    fn send_artnet(&mut self, _universes: &ArtNetFrames) -> io::Result<()> {
        Ok(())
    }

    /// The engine state behind the next frame sent. Sinks that only output DMX ignore it.
    fn status(&mut self, _status: &Status) {}

    /// Each MIDI event the engine takes in, before it applies it. For the event log.
    fn midi(&mut self, _event: &MidiEvent) {}

    /// Each cue the engine starts or releases. For the event log.
    fn cue(&mut self, _event: &CueEvent) {}

    /// How many frames the render loop skipped, after falling too far behind, before the next
    /// one sent.
    fn skipped(&mut self, _frames: u32) {}

    /// A line for the engine's log, such as where the tempo comes from. Sinks without a log
    /// ignore it.
    fn log(&mut self, _line: &str) {}
}

/// Where the render loop gets the show for each frame.
pub trait ShowSource {
    fn current(&mut self) -> &Show;
}

/// A show that never changes.
impl ShowSource for &Show {
    fn current(&mut self) -> &Show {
        self
    }
}

/// A show that never changes.
impl ShowSource for Show {
    fn current(&mut self) -> &Show {
        self
    }
}

impl<S: ShowSource + ?Sized> ShowSource for Box<S> {
    fn current(&mut self) -> &Show {
        (**self).current()
    }
}

pub trait Clock {
    /// Time since the clock started.
    fn now(&self) -> Duration;
    /// Blocks until `now() >= deadline`; returns at once if it already is.
    fn sleep_until(&mut self, deadline: Duration);
}

/// Copies share the same start, so a MIDI input thread can stamp events on the render loop's timeline.
#[derive(Clone, Copy)]
pub struct MonotonicClock {
    start: Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep_until(&mut self, deadline: Duration) {
        if let Some(remaining) = deadline.checked_sub(self.now()) {
            thread::sleep(remaining);
        }
    }
}

/// A clock that only moves when slept on, landing exactly on each deadline.
#[derive(Default)]
pub struct FakeClock {
    now: Duration,
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.now
    }

    fn sleep_until(&mut self, deadline: Duration) {
        self.now = self.now.max(deadline);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedFrame {
    pub at: Duration,
    /// The Enttec universe.
    pub values: Frame,
    pub artnet: ArtNetFrames,
}

/// A sink that keeps every frame it is sent.
#[derive(Default)]
pub struct FakeDmx {
    pub frames: Vec<RecordedFrame>,
    /// The Art-Net universes of the next frame.
    artnet: ArtNetFrames,
}

impl DmxSink for FakeDmx {
    fn send(&mut self, at: Duration, frame: &Frame) -> io::Result<()> {
        self.frames.push(RecordedFrame {
            at,
            values: *frame,
            artnet: std::mem::take(&mut self.artnet),
        });
        Ok(())
    }

    fn send_artnet(&mut self, universes: &ArtNetFrames) -> io::Result<()> {
        self.artnet = universes.clone();
        Ok(())
    }
}

/// Frames more than this late are skipped rather than sent in a burst: after a stall, output
/// picks up at the frame due now.
const MAX_LATE_FRAMES: u32 = 4;

/// Renders frames 0 to `frame_count` - 1, frame N due at N × `FRAME_PERIOD` on `clock`.
///
/// Layers, lowest first (plan §5): legacy notes, `look` on the fixtures it sets, cues, pixel
/// cues, then the master dimmer. Each frame renders at its deadline, so fades depend only on
/// event timestamps. Deadlines are absolute, so a late frame never shifts the ones after it;
/// frames more than `MAX_LATE_FRAMES` late are skipped (see `DmxSink::skipped`).
pub fn run(
    clock: &mut impl Clock,
    midi: &mut impl MidiSource,
    mut shows: impl ShowSource,
    look: &look::Look,
    sink: &mut impl DmxSink,
    frame_count: u32,
) -> io::Result<()> {
    let mut playback = playback::Playback::default();
    let mut index = 0;
    while index < frame_count {
        clock.sleep_until(FRAME_PERIOD * index);
        let due =
            u32::try_from(clock.now().as_nanos() / FRAME_PERIOD.as_nanos()).unwrap_or(u32::MAX);
        if due > index.saturating_add(MAX_LATE_FRAMES) {
            sink.skipped(due - index);
            index = due;
            if index >= frame_count {
                break;
            }
        }
        let deadline = FRAME_PERIOD * index;
        index += 1;
        let show = shows.current();
        while let Some(event) = midi.poll(deadline) {
            sink.midi(&event);
            playback.apply(show, &event);
            for cue in playback.take_cue_events() {
                sink.cue(&cue);
            }
        }
        let output = playback.render(show, look, deadline);
        for line in playback.log_lines(deadline) {
            sink.log(&line);
        }
        sink.status(&playback.status(show, deadline));
        // Reported by the driver; see `DmxSink::send_artnet`.
        let _ = sink.send_artnet(&output.artnet);
        sink.send(clock.now(), &output.enttec)?;
    }
    Ok(())
}

/// Renders a MIDI file offline: one frame per period, from 0 up to the end of the file.
pub fn replay(show: &Show, mut midi: MidiFile) -> Vec<RecordedFrame> {
    let periods = midi.duration().as_nanos() / FRAME_PERIOD.as_nanos();
    let frame_count = u32::try_from(periods + 1).expect("MIDI file longer than 3 years");
    let mut sink = FakeDmx::default();
    run(
        &mut FakeClock::default(),
        &mut midi,
        show,
        &look::Look::default(),
        &mut sink,
        frame_count,
    )
    .expect("FakeDmx never fails");
    sink.frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn replay_emits_frame_n_at_exactly_n_periods() {
        let midi = MidiFile::open(Path::new("../tests/fixtures/midi/tempo-change.mid")).unwrap();

        let frames = replay(&Show::default(), midi);

        assert_eq!(frames.len(), 71, "1.75 s file → frames 0 ms to 1750 ms");
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.at, Duration::from_millis(25 * index as u64));
            assert_eq!(frame.values, [0; DMX_CHANNELS]);
        }
    }

    /// Delivers its events all at once, on the first poll.
    struct Burst(Vec<MidiEvent>);

    impl MidiSource for Burst {
        fn poll(&mut self, _until: Duration) -> Option<MidiEvent> {
            self.0.pop()
        }
    }

    #[test]
    fn look_overrides_legacy_only_on_the_fixtures_it_sets() {
        let mut show = Show::load(Path::new("../tests/fixtures/shows/four-washes")).unwrap();
        show.legacy = LegacyMap::parse(
            "ranges:\n  - { first_note: 60, last_note: 60, address: 1 }\n  - { first_note: 61, last_note: 61, address: 17 }\n",
        )
        .unwrap();
        let note_on = |note| MidiEvent {
            at: Duration::ZERO,
            message: vec![0x90, note, 100],
        };
        let mut look = look::Look::default();
        let green = look::LookSettings::from_pairs(&["color=#00ff00".into()]).unwrap();
        look.set(&show, "wash-1", &green).unwrap();
        let mut sink = FakeDmx::default();

        run(
            &mut FakeClock::default(),
            &mut Burst(vec![note_on(60), note_on(61)]),
            &show,
            &look,
            &mut sink,
            1,
        )
        .unwrap();

        let frame = sink.frames[0].values;
        assert_eq!(frame[0..2], [0, 255], "wash-1 is the look's green");
        assert_eq!(
            frame[16], 200,
            "wash-2 is not in the look: legacy shows through"
        );
    }

    /// Lands on each deadline, except once: it wakes from `deadline` a second late, as after
    /// a stall.
    struct StallingClock {
        now: Duration,
        deadline: Duration,
    }

    impl Clock for StallingClock {
        fn now(&self) -> Duration {
            self.now
        }

        fn sleep_until(&mut self, deadline: Duration) {
            self.now = self.now.max(deadline);
            if deadline == self.deadline {
                self.now += Duration::from_secs(1);
            }
        }
    }

    #[test]
    fn after_a_stall_frames_resume_on_time_rather_than_in_a_burst() {
        let mut clock = StallingClock {
            now: Duration::ZERO,
            deadline: FRAME_PERIOD * 10,
        };
        let mut sink = FakeDmx::default();

        run(
            &mut clock,
            &mut Burst(Vec::new()),
            &Show::default(),
            &look::Look::default(),
            &mut sink,
            100,
        )
        .unwrap();

        let sent: Vec<Duration> = sink.frames.iter().map(|frame| frame.at).collect();
        assert_eq!(
            sent[..10],
            (0..10).map(|n| FRAME_PERIOD * n).collect::<Vec<_>>()
        );
        assert_eq!(
            sent[10..12],
            [FRAME_PERIOD * 50, FRAME_PERIOD * 51],
            "the frame due when it woke, then on time: the 40 frames it slept through are skipped"
        );
        assert_eq!(sent.len(), 60);
    }

    #[test]
    fn monotonic_clock_sleeps_until_the_deadline() {
        let mut clock = MonotonicClock::default();
        let deadline = clock.now() + Duration::from_millis(5);

        clock.sleep_until(deadline);

        assert!(clock.now() >= deadline);
    }
}

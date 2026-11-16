//! Musical time from the DAW's MIDI clock (plan §5), or from the song's BPM when there is none.
//!
//! Elapsed beats follow a continuous line, re-aimed at every tick so that it converges on the
//! clock's position within about a beat. A tempo change, a jittery tick, a dropout or a resumed
//! clock only bends the line: it never jumps and never goes back, so fades driven by it stay
//! smooth. The clock's position comes from counting ticks and a least-squares line through the
//! latest ones, so one early or late tick barely moves it.

use std::collections::VecDeque;
use std::time::Duration;

use crate::duration::{Length, TimeSignature};

const TIMING_CLOCK: u8 = 0xF8;
const START: u8 = 0xFA;
const CONTINUE: u8 = 0xFB;
const STOP: u8 = 0xFC;
const SONG_POSITION: u8 = 0xF2;
const TICKS_PER_BEAT: f64 = 24.0;
const SIXTEENTHS_PER_BEAT: f64 = 4.0;
/// One beat of tick intervals: enough to average out jitter, short enough to follow a tempo change.
const TEMPO_WINDOW: usize = 25;
/// Fewer ticks than this give too noisy a tempo; the previous estimate stands meanwhile.
const MIN_FIT_TICKS: usize = 6;
/// No tick for this long means the clock is gone. A tick every 250 ms is 10 BPM, far below any song.
const CLOCK_TIMEOUT: Duration = Duration::from_millis(250);
/// A stalled MIDI input delivers the ticks it held all at once. Ticks closer than this (a tick
/// every 5 ms is 500 BPM) are such a burst.
const BURST_GAP: Duration = Duration::from_millis(2);
/// A fit outside this is not a tempo but ticks bunched in some other way; the previous estimate
/// stands. Without the bound, a near-zero interval makes musical time race, or turn NaN.
const PLAUSIBLE_BPM: std::ops::RangeInclusive<f64> = 20.0..=500.0;
/// Bounds on the line's speed against the clock's tempo while it catches up. Keeps beats moving
/// forward when ahead, and fades no more than twice as fast as written when behind.
const CATCH_UP_SPEEDS: (f64, f64) = (0.5, 2.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoSource {
    MidiClock,
    /// No clock, or it stopped: musical time runs at the song's BPM.
    SongBpm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarPosition {
    /// Counted from 1, as DAWs show it.
    pub bar: u32,
    /// From 1 to just under the beats per bar plus one; fractional between beats.
    pub beat: f64,
    /// The length of this bar, in beats.
    pub beats_per_bar: f64,
}

/// Where bar lines fall, in beats from the top of the song: each stretch has bars of one length
/// from its first beat up to the next stretch, the last one for ever.
#[derive(Debug, Clone, PartialEq)]
pub struct BarGrid(Vec<BarStretch>);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarStretch {
    /// Its first beat, from the top of the song.
    pub beats: f64,
    /// The number of its first bar, from 1.
    pub bar: u32,
    pub beats_per_bar: f64,
}

impl BarGrid {
    /// Bars of one signature throughout.
    pub fn uniform(time_signature: TimeSignature) -> Self {
        Self(vec![BarStretch {
            beats: 0.0,
            bar: 1,
            beats_per_bar: time_signature.beats_per_bar(),
        }])
    }

    /// `stretches` in order, the first from beat 0.
    pub fn new(stretches: Vec<BarStretch>) -> Self {
        Self(stretches)
    }

    pub fn position(&self, song_beats: f64) -> BarPosition {
        let stretch = self
            .0
            .iter()
            .rev()
            .find(|stretch| stretch.beats <= song_beats)
            .unwrap_or(&self.0[0]);
        let into_stretch = song_beats - stretch.beats;
        let bars = (into_stretch / stretch.beats_per_bar).floor();
        BarPosition {
            bar: stretch.bar.saturating_add(bars as u32),
            beat: into_stretch - bars * stretch.beats_per_bar + 1.0,
            beats_per_bar: stretch.beats_per_bar,
        }
    }
}

/// A point in time with the beats elapsed at it, so a length can later be measured from it in
/// either unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    pub at: Duration,
    pub beats: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Playing,
    Stopped,
    /// After Start or Continue: the next tick plays the current song position rather than moving past it.
    Cued,
}

/// Times are in seconds from `from`, a recent tick, not from engine start: the same ticks then
/// give the same result however long the engine has run.
#[derive(Debug, Clone, Copy)]
struct TickFit {
    from: Duration,
    interval_seconds: f64,
    /// Where the line puts the latest tick.
    latest_tick_seconds: f64,
}

/// Elapsed beats are `beats + (t - at) × per_second`, up to where the song's BPM takes over.
#[derive(Debug, Clone, Copy)]
struct BeatLine {
    at: Duration,
    beats: f64,
    per_second: f64,
}

#[derive(Debug, Clone)]
pub struct ClockFollower {
    song_bpm: f64,
    /// For bar durations, which keep the song's signature.
    time_signature: TimeSignature,
    /// For bar positions, which follow the arrangement's sections.
    bar_grid: BarGrid,
    line: BeatLine,
    /// The clock's position at the latest tick, in elapsed beats.
    clock_beats: f64,
    /// The latest tick times, oldest first; empty until the clock ticks.
    ticks: VecDeque<Duration>,
    /// The line fitted through `ticks`.
    tick_fit: TickFit,
    transport: Transport,
    /// Song position, in beats from the top of the song, when elapsed beats were `song_from_beats`.
    song_beats: f64,
    song_from_beats: f64,
    reported_source: Option<TempoSource>,
}

impl ClockFollower {
    /// A follower started at engine time zero.
    pub fn new(song_bpm: f64, time_signature: TimeSignature) -> Self {
        Self::started_at(Duration::ZERO, song_bpm, time_signature)
    }

    /// A follower whose beats and bars count from `at`, the song select.
    pub fn started_at(at: Duration, song_bpm: f64, time_signature: TimeSignature) -> Self {
        Self {
            song_bpm,
            time_signature,
            bar_grid: BarGrid::uniform(time_signature),
            line: BeatLine {
                at,
                beats: 0.0,
                per_second: song_bpm / 60.0,
            },
            clock_beats: 0.0,
            ticks: VecDeque::with_capacity(TEMPO_WINDOW),
            tick_fit: TickFit {
                from: at,
                interval_seconds: 60.0 / song_bpm / TICKS_PER_BEAT,
                latest_tick_seconds: 0.0,
            },
            transport: Transport::Playing,
            song_beats: 0.0,
            song_from_beats: 0.0,
            reported_source: None,
        }
    }

    /// Counts bar durations in `time_signature` and places bar lines on `grid` from now on, as
    /// when the song's signature or arrangement is edited.
    pub fn set_bars(&mut self, time_signature: TimeSignature, grid: &BarGrid) {
        self.time_signature = time_signature;
        if self.bar_grid != *grid {
            self.bar_grid = grid.clone();
        }
    }

    /// Changes the tempo used without a clock, as when the song's `bpm` is edited. Musical time
    /// carries on from where it is at `at`, at the new pace.
    pub fn set_song_bpm(&mut self, at: Duration, bpm: f64) {
        if bpm == self.song_bpm {
            return;
        }
        if self.source(at) == TempoSource::SongBpm {
            self.line = BeatLine {
                at,
                beats: self.beats(at),
                per_second: bpm / 60.0,
            };
        }
        self.song_bpm = bpm;
        self.reported_source = None;
    }

    /// Applies one MIDI message received at `at`; anything but clock and transport messages is
    /// ignored. Apply messages in time order, before asking about any later time.
    pub fn apply(&mut self, at: Duration, message: &[u8]) {
        match *message {
            [TIMING_CLOCK] => self.tick(at),
            [START] => {
                self.song_beats = 0.0;
                self.transport = Transport::Cued;
            }
            [CONTINUE] => self.transport = Transport::Cued,
            [STOP] => {
                self.song_beats = self.song_position(at);
                self.transport = Transport::Stopped;
            }
            [SONG_POSITION, low, high] => {
                let sixteenths = (u16::from(high) << 7) | u16::from(low);
                self.song_beats = f64::from(sixteenths) / SIXTEENTHS_PER_BEAT;
                self.song_from_beats = self.beats(at);
            }
            _ => {}
        }
    }

    fn tick(&mut self, at: Duration) {
        let beats = self.beats(at);
        match self.ticks.back() {
            Some(_) if self.clock_is_live(at) => self.clock_beats += 1.0 / TICKS_PER_BEAT,
            Some(&latest) if self.transport == Transport::Playing => {
                // The DAW played on through a dropout: count the ticks lost at the last tempo.
                let elapsed =
                    at.saturating_sub(latest).as_secs_f64() / self.tick_fit.interval_seconds;
                self.clock_beats += elapsed.round().max(1.0) / TICKS_PER_BEAT;
                self.ticks.clear();
            }
            _ => {
                // The clock starts, or restarts after a stop, where musical time already is.
                self.clock_beats = beats;
                self.ticks.clear();
            }
        }
        if self.transport == Transport::Cued {
            self.song_from_beats = beats;
            self.transport = Transport::Playing;
        }
        // A burst still counts, but its arrival times say nothing of the tempo: fit afresh after it.
        if self
            .ticks
            .back()
            .is_some_and(|&latest| at.saturating_sub(latest) < BURST_GAP)
        {
            self.ticks.clear();
        }
        if self.ticks.len() == TEMPO_WINDOW {
            self.ticks.pop_front();
        }
        self.ticks.push_back(at);
        self.tick_fit = self.fit_ticks();

        // Aim to close the gap to the clock within about one beat.
        let clock_per_second = 1.0 / (self.tick_fit.interval_seconds * TICKS_PER_BEAT);
        let behind = self.clock_position(at) - beats;
        let (slowest, fastest) = CATCH_UP_SPEEDS;
        self.line = BeatLine {
            at,
            beats,
            per_second: clock_per_second * (1.0 + behind).clamp(slowest, fastest),
        };
    }

    /// Least-squares line through the latest ticks; the previous tempo until there are enough.
    fn fit_ticks(&self) -> TickFit {
        let first = self.ticks[0];
        let latest = self.ticks[self.ticks.len() - 1];
        let previous_tempo = TickFit {
            from: first,
            interval_seconds: self.tick_fit.interval_seconds,
            latest_tick_seconds: (latest - first).as_secs_f64(),
        };
        if self.ticks.len() < MIN_FIT_TICKS {
            return previous_tempo;
        }
        let count = self.ticks.len() as f64;
        let mean_index = (count - 1.0) / 2.0;
        let offsets = || self.ticks.iter().map(|tick| (*tick - first).as_secs_f64());
        let mean_offset = offsets().sum::<f64>() / count;
        let (covariance, variance) =
            offsets()
                .enumerate()
                .fold((0.0, 0.0), |(covariance, variance), (index, offset)| {
                    let from_mean = index as f64 - mean_index;
                    (
                        covariance + from_mean * (offset - mean_offset),
                        variance + from_mean * from_mean,
                    )
                });
        let interval_seconds = covariance / variance;
        if !PLAUSIBLE_BPM.contains(&(60.0 / (interval_seconds * TICKS_PER_BEAT))) {
            return previous_tempo;
        }
        TickFit {
            from: first,
            interval_seconds,
            latest_tick_seconds: mean_offset + interval_seconds * mean_index,
        }
    }

    /// Where the clock is at `at`, in elapsed beats, extrapolating from its latest tick.
    fn clock_position(&self, at: Duration) -> f64 {
        let TickFit {
            from,
            interval_seconds,
            latest_tick_seconds,
        } = self.tick_fit;
        self.clock_beats
            + (at.saturating_sub(from).as_secs_f64() - latest_tick_seconds)
                / (interval_seconds * TICKS_PER_BEAT)
    }

    /// Beats elapsed since the follower started: continuous, never going back. Count durations
    /// with it.
    pub fn beats(&self, at: Duration) -> f64 {
        let line = |at: Duration| {
            self.line.beats + at.saturating_sub(self.line.at).as_secs_f64() * self.line.per_second
        };
        let song_bpm_from = self.song_bpm_from();
        if at <= song_bpm_from {
            line(at)
        } else {
            line(song_bpm_from) + (at - song_bpm_from).as_secs_f64() * self.song_bpm / 60.0
        }
    }

    pub fn moment(&self, at: Duration) -> Moment {
        Moment {
            at,
            beats: self.beats(at),
        }
    }

    /// How far through `length` it is at `at`, from 0 to 1, counting from `from`. Beats and bars
    /// count elapsed beats, so they follow the tempo while they run. A zero length is done at once.
    pub fn progress(&self, length: Length, from: Moment, at: Duration) -> f64 {
        self.progress_to(length, from, self.moment(at))
    }

    /// As `progress`, at `to` rather than at a time: a moment taken earlier still counts the
    /// beats it had, where the beats of a past time follow the clock since.
    pub fn progress_to(&self, length: Length, from: Moment, to: Moment) -> f64 {
        let (elapsed, span) = self.elapsed(length, from, to);
        if span > 0.0 {
            (elapsed / span).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// How many `length`s have passed at `at` since `from`, unclamped, for effects that repeat.
    /// Counts as `progress` does. `length` must not be zero.
    pub fn cycles(&self, length: Length, from: Moment, at: Duration) -> f64 {
        let (elapsed, span) = self.elapsed(length, from, self.moment(at));
        elapsed / span
    }

    /// Time or beats from `from` to `to`, and `length` in the same unit.
    fn elapsed(&self, length: Length, from: Moment, to: Moment) -> (f64, f64) {
        let beats = to.beats - from.beats;
        match length {
            Length::Time(span) => (
                to.at.saturating_sub(from.at).as_secs_f64(),
                span.as_secs_f64(),
            ),
            Length::Beats(span) => (beats, span),
            Length::Bars(bars) => (beats, bars * self.time_signature.beats_per_bar()),
        }
    }

    /// When the song's BPM takes over: once the clock times out, or at once if it never ticked.
    /// A BPM edited after the timeout moves the line, and runs from there.
    fn song_bpm_from(&self) -> Duration {
        self.ticks.back().map_or(self.line.at, |latest| {
            (*latest + CLOCK_TIMEOUT).max(self.line.at)
        })
    }

    pub fn bar_position(&self, at: Duration) -> BarPosition {
        self.bar_grid.position(self.song_position(at))
    }

    pub fn source(&self, at: Duration) -> TempoSource {
        if self.clock_is_live(at) {
            TempoSource::MidiClock
        } else {
            TempoSource::SongBpm
        }
    }

    pub fn bpm(&self, at: Duration) -> f64 {
        match self.source(at) {
            TempoSource::MidiClock => 60.0 / (self.tick_fit.interval_seconds * TICKS_PER_BEAT),
            TempoSource::SongBpm => self.song_bpm,
        }
    }

    /// A line for the engine log when the tempo source has changed since the last call.
    /// The first call always gives one, so the log says where tempo comes from at start.
    pub fn log_line(&mut self, now: Duration) -> Option<String> {
        let source = self.source(now);
        if self.reported_source == Some(source) {
            return None;
        }
        self.reported_source = Some(source);
        Some(match source {
            TempoSource::MidiClock => "following the MIDI clock".to_string(),
            TempoSource::SongBpm => {
                format!("no MIDI clock: using the song's {} BPM", self.song_bpm)
            }
        })
    }

    fn song_position(&self, at: Duration) -> f64 {
        match self.transport {
            Transport::Playing => self.song_beats + self.beats(at) - self.song_from_beats,
            Transport::Stopped | Transport::Cued => self.song_beats,
        }
    }

    fn clock_is_live(&self, at: Duration) -> bool {
        self.ticks
            .back()
            .is_some_and(|latest| at.saturating_sub(*latest) < CLOCK_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: &[u8] = &[TIMING_CLOCK];

    fn millis(milliseconds: f64) -> Duration {
        Duration::from_secs_f64(milliseconds / 1000.0)
    }

    /// Sends ticks `first..first + count` of a steady clock at `bpm`, each up to 1 ms early or
    /// late; returns the exact time in ms of the tick after the last.
    fn send_ticks(follower: &mut ClockFollower, bpm: f64, first: u32, count: u32) -> f64 {
        for index in first..first + count {
            follower.apply(millis(tick_arrival_ms(bpm, index)), CLOCK);
        }
        f64::from(first + count) * 60_000.0 / bpm / TICKS_PER_BEAT
    }

    fn tick_arrival_ms(bpm: f64, index: u32) -> f64 {
        let jitter_ms = [0.0, 1.0, -1.0][index as usize % 3];
        f64::from(index) * 60_000.0 / bpm / TICKS_PER_BEAT + jitter_ms
    }

    #[test]
    fn a_new_song_bpm_changes_the_pace_without_moving_musical_time() {
        let mut follower = ClockFollower::new(60.0, TimeSignature::default());
        let second = Duration::from_secs(1);

        follower.set_song_bpm(second, 120.0);

        assert_eq!(follower.beats(second), 1.0);
        assert_eq!(follower.beats(2 * second), 3.0);
        assert_eq!(
            follower.log_line(second).as_deref(),
            Some("no MIDI clock: using the song's 120 BPM")
        );
    }

    #[test]
    fn a_bpm_edited_during_a_clock_dropout_still_catches_up_with_the_clock() {
        let mut follower = ClockFollower::new(120.0, TimeSignature::default());
        // Two beats of clock at 120 BPM, then a dropout from 1 s to 2.5 s while the DAW plays on.
        send_ticks(&mut follower, 120.0, 0, 49);

        follower.set_song_bpm(millis(1500.0), 60.0);
        // Musical time is a beat behind when the clock comes back; four beats to catch up.
        let next_ms = send_ticks(&mut follower, 120.0, 120, 96);

        let clock_beats = next_ms / 500.0;
        let beats = follower.beats(millis(next_ms));
        assert!(
            (beats - clock_beats).abs() < 0.1,
            "{beats} beats against the clock's {clock_beats}"
        );
    }

    #[test]
    fn estimates_the_tempo_through_jitter() {
        let mut follower = ClockFollower::new(120.0, TimeSignature::default());

        let next_ms = send_ticks(&mut follower, 92.0, 0, 48);
        let after_last = millis(next_ms - 10.0);

        assert_eq!(follower.source(after_last), TempoSource::MidiClock);
        assert!((follower.bpm(after_last) - 92.0).abs() < 0.2);
        assert!((follower.beats(after_last) - 2.0).abs() < 1.0 / TICKS_PER_BEAT);
    }

    #[test]
    fn start_puts_the_next_tick_on_bar_1_beat_1() {
        let mut follower = ClockFollower::new(120.0, TimeSignature::default());

        follower.apply(millis(0.0), &[START]);
        let next_ms = send_ticks(&mut follower, 120.0, 0, 4 * 24 + 1);

        let BarPosition { bar, beat, .. } = follower.bar_position(millis(next_ms - 20.0));
        assert_eq!(bar, 2);
        assert!((beat - 1.0).abs() < 1.0 / TICKS_PER_BEAT, "{beat}");
    }

    #[test]
    fn stop_freezes_the_bar_and_continue_resumes_from_the_song_position() {
        let three_four = TimeSignature {
            numerator: 3,
            denominator: 4,
        };
        let mut follower = ClockFollower::new(120.0, three_four);
        follower.apply(millis(0.0), &[START]);
        let next_ms = send_ticks(&mut follower, 120.0, 0, 24 + 1);
        follower.apply(millis(next_ms), &[STOP]);
        let stopped = follower.bar_position(millis(next_ms));

        assert_eq!(follower.bar_position(millis(next_ms + 5000.0)), stopped);
        assert_eq!(stopped.bar, 1);
        assert!((stopped.beat - 2.0).abs() < 0.05, "{stopped:?}");

        // 6 s later (288 ticks at 120 BPM), the DAW cues bar 9 (32 beats = 128 sixteenths)
        // and continues.
        let cue_ms = next_ms + 6000.0;
        follower.apply(millis(cue_ms), &[SONG_POSITION, 0, 1]);
        follower.apply(millis(cue_ms), &[CONTINUE]);
        send_ticks(&mut follower, 120.0, 313, 1);
        let cued = follower.bar_position(millis(cue_ms));
        let next_ms = send_ticks(&mut follower, 120.0, 314, 24);
        let a_beat_later = follower.bar_position(millis(next_ms - 20.0));

        assert_eq!(cued.bar, 11, "32 beats into 3/4 is bar 11 beat 3");
        assert!((cued.beat - 3.0).abs() < 1e-9, "{cued:?}");
        assert_eq!(a_beat_later.bar, 12);
        assert!((a_beat_later.beat - 1.0).abs() < 1.0 / TICKS_PER_BEAT);
    }

    #[test]
    fn ticks_delivered_in_a_burst_do_not_race_musical_time_ahead() {
        let mut follower = ClockFollower::new(120.0, TimeSignature::default());
        let next_ms = send_ticks(&mut follower, 120.0, 0, 48);
        // MIDI input stalls for 500 ms, then delivers the buffered ticks all at once.
        let burst_ms = next_ms + 500.0;
        for _ in 0..12 {
            follower.apply(millis(burst_ms), CLOCK);
        }
        let mut previous = follower.beats(millis(burst_ms));
        // Burst at tick 72's time; the clock carries on from tick 73.
        for tick in 73..73 + 4 * 24 {
            let at_ms = f64::from(tick) * 500.0 / TICKS_PER_BEAT;
            follower.apply(millis(at_ms), CLOCK);
            let beats = follower.beats(millis(at_ms));
            assert!(
                beats - previous < 0.1,
                "beats jumped from {previous} to {beats} at {at_ms} ms"
            );
            previous = beats;
        }
    }

    #[test]
    fn without_a_clock_beats_run_at_the_song_bpm_and_it_is_logged() {
        let mut follower = ClockFollower::new(92.0, TimeSignature::default());

        assert_eq!(
            follower.log_line(Duration::ZERO).as_deref(),
            Some("no MIDI clock: using the song's 92 BPM")
        );
        assert_eq!(follower.log_line(Duration::from_secs(1)), None);
        assert_eq!(
            follower.source(Duration::from_secs(60)),
            TempoSource::SongBpm
        );
        assert!((follower.beats(Duration::from_secs(60)) - 92.0).abs() < 1e-9);
        assert_eq!(follower.bar_position(Duration::from_secs(60)).bar, 24);
    }

    #[test]
    fn through_a_dropout_beats_carry_on_smoothly_then_catch_up_with_the_clock() {
        let mut follower = ClockFollower::new(92.0, TimeSignature::default());
        follower.log_line(Duration::ZERO);
        let mut beats_every_5_ms = vec![];
        let mut sample_before = |follower: &ClockFollower, until_ms: f64| {
            while (beats_every_5_ms.len() as f64) * 5.0 < until_ms {
                let at = millis(beats_every_5_ms.len() as f64 * 5.0);
                beats_every_5_ms.push(follower.beats(at));
            }
        };

        // The DAW plays on at 140 BPM, but ticks 48 to 159 (2 s) are lost; the song says 92.
        let last_tick = 160 + 4 * 24 - 1;
        for tick in (0..48).chain(160..=last_tick) {
            let arrival_ms = tick_arrival_ms(140.0, tick);
            sample_before(&follower, arrival_ms);
            follower.apply(millis(arrival_ms), CLOCK);
            if tick == 47 {
                let lost = millis(arrival_ms + 300.0);
                assert_eq!(
                    follower.log_line(millis(arrival_ms)).as_deref(),
                    Some("following the MIDI clock")
                );
                assert_eq!(follower.source(lost), TempoSource::SongBpm);
                assert_eq!(follower.bpm(lost), 92.0);
                assert_eq!(
                    follower.log_line(lost).as_deref(),
                    Some("no MIDI clock: using the song's 92 BPM")
                );
            }
        }

        // Catching up runs at most twice the clock's tempo, give or take 10% of tempo-estimate
        // noise; the old snap to the clock moved a third of a beat at once.
        let fastest_step = 2.0 * 1.1 * 140.0 / 60.0 * 0.005;
        for (index, step) in beats_every_5_ms.windows(2).enumerate() {
            let advance = step[1] - step[0];
            assert!(
                (0.0..=fastest_step + 1e-9).contains(&advance),
                "beats moved {advance} at {} ms",
                index * 5
            );
        }
        let last_arrival = millis(tick_arrival_ms(140.0, last_tick));
        let clock_beats = f64::from(last_tick) / TICKS_PER_BEAT;
        let after_4_beats = follower.beats(last_arrival);
        assert!(
            (after_4_beats - clock_beats).abs() < 0.05,
            "{after_4_beats} beats, the clock is at {clock_beats}"
        );
        assert_eq!(follower.source(last_arrival), TempoSource::MidiClock);
    }
}

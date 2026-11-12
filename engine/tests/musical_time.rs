//! Musical timing under synthetic MIDI clocks (plan §15): a 4-bar swell is
//! `ClockFollower::progress` over `4 bars` from its note, stepped frame by frame as the render
//! loop does. Seeded runs vary tempo, jitter, frame phase, dropouts and stops.

use std::time::Duration;

use blimey_engine::FRAME_PERIOD;
use blimey_engine::clock_follower::{ClockFollower, Moment};
use blimey_engine::duration::{Length, TimeSignature};
use blimey_engine::envelope::Envelope;

const NOTE_ON: &[u8] = &[0x91, 38, 127];
const START: &[u8] = &[0xFA];
const CONTINUE: &[u8] = &[0xFB];
const STOP: &[u8] = &[0xFC];
const CLOCK: &[u8] = &[0xF8];
const TICKS_PER_BEAT: u32 = 24;
const SWELL: Length = Length::Bars(4.0);
const SWELL_BEATS: u32 = 16;
/// How far off each clock tick and the note may arrive, as through a real MIDI port.
const MAX_JITTER: Duration = Duration::from_millis(2);
/// Deliberately not the clock's tempo, so a swell that ignores the clock fails.
const SONG_BPM: f64 = 120.0;
const MAX_BPM: f64 = 180.0;

/// xorshift64 with a fixed seed per run: every run is reproducible.
struct Random(u64);

impl Random {
    /// Uniform in 0–1.
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 as f64 / u64::MAX as f64
    }

    fn jitter(&mut self) -> f64 {
        (self.unit() * 2.0 - 1.0) * MAX_JITTER.as_secs_f64()
    }
}

fn follower(bpm: f64) -> ClockFollower {
    ClockFollower::new(bpm, TimeSignature::default())
}

/// Exact time in seconds of every tick from Reaper playing from the top at `origin`, the
/// tempo of each beat given by `bpm_of_beat`.
fn tick_times(origin: f64, beats: u32, bpm_of_beat: impl Fn(u32) -> f64) -> Vec<f64> {
    let mut at = origin;
    (0..=beats * TICKS_PER_BEAT)
        .map(|tick| {
            let this_tick = at;
            at += 60.0 / bpm_of_beat(tick / TICKS_PER_BEAT) / f64::from(TICKS_PER_BEAT);
            this_tick
        })
        .collect()
}

struct Rendered {
    /// Seconds.
    at: f64,
    beats: f64,
    /// The swell's level, `None` until its note.
    level: Option<f64>,
}

impl Rendered {
    fn is_full(&self) -> bool {
        self.level.is_some_and(|level| level >= 1.0 - 1e-9)
    }
}

/// Runs MIDI events through `follower` frame by frame, as the render loop does, with a 4-bar
/// ramp up to full starting on the note.
fn render(
    mut events: Vec<(f64, &'static [u8])>,
    mut follower: ClockFollower,
) -> impl Iterator<Item = Rendered> {
    events.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut events = events
        .into_iter()
        .map(|(at, message)| (Duration::from_secs_f64(at), message))
        .peekable();
    let mut started: Option<Moment> = None;
    (0..).map(move |frame| {
        let now = FRAME_PERIOD * frame;
        while let Some((at, message)) = events.next_if(|(at, _)| *at <= now) {
            follower.apply(at, message);
            if message == NOTE_ON {
                started = Some(follower.moment(at));
            }
        }
        Rendered {
            at: now.as_secs_f64(),
            beats: follower.beats(now),
            level: started.map(|started| follower.progress(SWELL, started, now)),
        }
    })
}

/// The time of the first frame at full level, in seconds.
fn first_full_frame(events: Vec<(f64, &'static [u8])>, follower: ClockFollower) -> f64 {
    render(events, follower)
        .find(Rendered::is_full)
        .expect("frames go on until the swell ends")
        .at
}

/// The frames from the start of the session until the swell is full.
fn frames_until_full(events: Vec<(f64, &'static [u8])>, follower: ClockFollower) -> Vec<Rendered> {
    render(events, follower)
        .take_while(|frame| !frame.is_full())
        .collect()
}

/// 100 runs of a 4-bar swell on a bar line under a jittery clock, each from a random point of
/// the frame grid and a random bar; asserts it ends within a frame of the bar line 4 bars later.
fn assert_swells_end_on_the_bar_line(bpm_of_beat: impl Fn(u32, u32) -> f64) {
    for run in 1..=100 {
        let mut random = Random(run);
        let origin = 0.5 + random.unit() * FRAME_PERIOD.as_secs_f64();
        let note_beat = 4 * (1 + (random.unit() * 8.0) as u32);
        let ticks = tick_times(origin, note_beat + SWELL_BEATS, |beat| {
            bpm_of_beat(note_beat, beat)
        });
        let mut events = vec![(origin - 0.001, START)];
        events.extend(ticks.iter().map(|at| (at + random.jitter(), CLOCK)));
        let note_tick = (note_beat * TICKS_PER_BEAT) as usize;
        events.push((ticks[note_tick] + random.jitter(), NOTE_ON));
        let bar_line = ticks[note_tick + (SWELL_BEATS * TICKS_PER_BEAT) as usize];

        let error = first_full_frame(events, follower(SONG_BPM)) - bar_line;

        assert!(
            error.abs() <= FRAME_PERIOD.as_secs_f64(),
            "run {run}: swell ended {:.1} ms from the bar line",
            error * 1000.0
        );
    }
}

#[test]
fn four_bar_swell_ends_on_the_bar_line_at_92_bpm() {
    assert_swells_end_on_the_bar_line(|_, _| 92.0);
}

#[test]
fn four_bar_swell_ends_on_the_bar_line_at_140_bpm() {
    assert_swells_end_on_the_bar_line(|_, _| 140.0);
}

#[test]
fn four_bar_swell_follows_a_tempo_change_halfway() {
    assert_swells_end_on_the_bar_line(|note_beat, beat| {
        if beat < note_beat + SWELL_BEATS / 2 {
            92.0
        } else {
            140.0
        }
    });
}

#[test]
fn without_a_clock_a_swell_lasts_its_beats_at_the_song_bpm() {
    let mut follower = follower(92.0);
    let bar_line = 1.0 + f64::from(SWELL_BEATS) * 60.0 / 92.0;

    assert_eq!(
        follower.log_line(Duration::ZERO).as_deref(),
        Some("no MIDI clock: using the song's 92 BPM")
    );
    let error = first_full_frame(vec![(1.0, NOTE_ON)], follower) - bar_line;
    assert!(
        (0.0..=FRAME_PERIOD.as_secs_f64()).contains(&error),
        "swell ended {:.1} ms from the bar line",
        error * 1000.0
    );
}

#[test]
fn a_length_in_seconds_ignores_the_tempo() {
    let follower = follower(92.0);
    let at = Duration::from_millis;
    let started = follower.moment(at(1000));
    let half_second = Length::Time(at(500));

    assert_eq!(follower.progress(half_second, started, at(900)), 0.0);
    assert_eq!(follower.progress(half_second, started, at(1250)), 0.5);
    assert_eq!(Envelope::EaseIn.level(0.5), 0.25);
    assert_eq!(follower.progress(half_second, started, at(9000)), 1.0);
    assert_eq!(
        follower.progress(Length::Beats(0.0), started, at(1000)),
        1.0
    );
}

#[test]
fn a_swell_rises_evenly_between_ticks() {
    // At 92 BPM a tick comes every 27 ms, about one per 25 ms frame: moving by whole ticks
    // would leave some frames standing still and others rising twice as far.
    let mut random = Random(7);
    let ticks = tick_times(0.5, 4 + SWELL_BEATS, |_| 92.0);
    let mut events = vec![(0.499, START), (ticks[4 * 24], NOTE_ON)];
    events.extend(ticks.iter().map(|at| (at + random.jitter(), CLOCK)));
    let even_rise = 92.0 / 60.0 * FRAME_PERIOD.as_secs_f64() / f64::from(SWELL_BEATS);

    let levels: Vec<f64> = frames_until_full(events, follower(92.0))
        .iter()
        .filter_map(|frame| frame.level)
        .collect();

    for (index, pair) in levels.windows(2).enumerate() {
        let rise = pair[1] - pair[0];
        assert!(
            (rise / even_rise - 1.0).abs() < 0.05,
            "frame {index} of the swell rose {:.2}× the even rise",
            rise / even_rise
        );
    }
}

/// Reaper playing 12 bars, each at a random tempo from 60 to `MAX_BPM` and with a jittery
/// clock. Some bars lose the clock for 0.3–2 s while Reaper plays on; others start with a
/// stop of 0.3–3 s, then continue. The swell's note lands on bar 3.
fn eventful_session(random: &mut Random) -> Vec<(f64, &'static [u8])> {
    let mut at = 0.5;
    let mut events = vec![(at - 0.001, START)];
    let mut silent_until = 0.0;
    for bar in 0..12 {
        let bpm = 60.0 + random.unit() * (MAX_BPM - 60.0);
        let fate = random.unit();
        if bar > 0 && fate < 0.15 {
            silent_until = at + 0.3 + random.unit() * 1.7;
        } else if bar > 0 && fate < 0.3 {
            events.push((at, STOP));
            at += 0.3 + random.unit() * 2.7;
            events.push((at, CONTINUE));
        }
        if bar == 2 {
            events.push((at + random.jitter(), NOTE_ON));
        }
        for _ in 0..4 * TICKS_PER_BEAT {
            if at >= silent_until {
                events.push((at + random.jitter(), CLOCK));
            }
            at += 60.0 / bpm / f64::from(TICKS_PER_BEAT);
        }
    }
    events
}

#[test]
fn swells_stay_smooth_through_tempo_changes_dropouts_and_stops() {
    // A 4-bar ramp at the fastest tempo rises 0.0047 a frame. Catching up with the clock after
    // a dropout runs at most twice the clock's tempo, and the tempo estimate may be a little
    // off just after the clock returns: allow 25% for it. That is 0.0117 a frame, 3 of 255 DMX
    // steps; snapping onto the clock moved a third of a beat, 0.02, at once.
    let max_rise =
        2.0 * 1.25 * MAX_BPM / 60.0 * FRAME_PERIOD.as_secs_f64() / f64::from(SWELL_BEATS);
    for run in 1..=100 {
        let mut random = Random(run);
        let events = eventful_session(&mut random);
        let song_bpm = 60.0 + random.unit() * (MAX_BPM - 60.0);

        let frames = frames_until_full(events, follower(song_bpm));

        for pair in frames.windows(2) {
            let [before, after] = pair else {
                unreachable!()
            };
            assert!(
                after.beats >= before.beats,
                "run {run}: beats went back at {:.3} s",
                after.at
            );
            if let (Some(level_before), Some(level_after)) = (before.level, after.level) {
                let rise = level_after - level_before;
                assert!(
                    (0.0..=max_rise).contains(&rise),
                    "run {run}: level rose {rise:.4} at {:.3} s",
                    after.at
                );
            }
        }
    }
}

// The playback scheduler on a fake audio clock: no browser, no engine.

import { test, expect } from '@playwright/test';
import { createScheduler } from '../src/scheduler.js';

/** One engine frame (FRAME_PERIOD in engine/src/lib.rs). */
const FRAME_SECONDS = 0.025;
/** How late a browser timer fires, at worst, in these tests. */
const TIMER_LATENESS = 0.004;

// As `GET /api/songs/<song>/arrangement/events` lists them, at 120 BPM.
const SONG = {
  bpm: 120,
  events: [
    { beats: 0, seconds: 0, type: 'time_signature', numerator: 4, denominator: 4 },
    { beats: 0, seconds: 0, type: 'marker', section: 'verse' },
    { beats: 0, seconds: 0, type: 'program_change', program: 1 },
    { beats: 0, seconds: 0, type: 'note_on', scene: 36, velocity: 127 },
    { beats: 6, seconds: 3, type: 'note_on', scene: 37, velocity: 64 },
    { beats: 8, seconds: 4, type: 'note_off', scene: 36 },
    { beats: 8, seconds: 4, type: 'note_off', scene: 37 },
    { beats: 8, seconds: 4, type: 'marker', section: 'chorus' },
    { beats: 8, seconds: 4, type: 'note_on', scene: 42, velocity: 127 },
    { beats: 12, seconds: 6, type: 'note_off', scene: 42 },
  ],
};

/** The song's 3 bars: the last block ends at 6 s, the arrangement at 8 s. */
const SONG_SECONDS = 8;

/** A scheduler on a fake clock, its sound heard `latency` seconds late, recording what it sends and when. */
function fakePlayback({ latency = 0 } = {}) {
  let now = 0;
  let nextTimer = 0;
  const timers = new Map();
  const sent = [];
  const scheduler = createScheduler({
    clock: () => now,
    latency: () => latency,
    send: (message) => sent.push({ at: now, message }),
    setTimer: (wake, ms) => {
      nextTimer += 1;
      timers.set(nextTimer, { wake, at: now + Math.max(0, ms) / 1000 });
      return nextTimer;
    },
    clearTimer: (timer) => timers.delete(timer),
  });
  scheduler.load(SONG, SONG_SECONDS);
  /** Moves the clock on by `seconds`, running each timer due on the way a little late. */
  function advance(seconds) {
    const until = now + seconds;
    for (;;) {
      const [id, due] = [...timers].sort(([, a], [, b]) => a.at - b.at)[0] ?? [];
      if (!due || due.at + TIMER_LATENESS > until) break;
      timers.delete(id);
      now = due.at + TIMER_LATENESS;
      due.wake();
    }
    now = until;
  }
  /** The messages sent since the last call. */
  let read = 0;
  const since = () => sent.slice(read, (read = sent.length)).map(({ message }) => message);
  return { scheduler, sent, advance, since };
}

test('playing from the top sends the song, then every event within a frame of its time, and stops at the end of the last bar', () => {
  const { scheduler, sent, advance } = fakePlayback();
  scheduler.play();
  advance(7);
  expect(scheduler.isPlaying(), 'the bars after the last block still play').toBe(true);
  advance(3);

  expect(sent.map(({ message }) => message)).toEqual([
    [0xc1, 1],
    [0xf2, 0, 0],
    [0x91, 36, 127],
    [0x91, 37, 64],
    [0x81, 36, 0],
    [0x81, 37, 0],
    [0x91, 42, 127],
    [0x81, 42, 0],
  ]);
  const due = [0, 0, 0, 3, 4, 4, 4, 6];
  sent.forEach(({ at }, index) => expect(Math.abs(at - due[index]), `message ${index}`).toBeLessThanOrEqual(FRAME_SECONDS));
  expect(scheduler.isPlaying()).toBe(false);
  expect(scheduler.position()).toBe(SONG_SECONDS);
});

test('each scene is sent as the sound is heard, the output latency after the clock', () => {
  const latency = 0.2;
  const { scheduler, sent, advance } = fakePlayback({ latency });
  scheduler.play();
  advance(5);

  const scenes = sent.slice(2);
  const due = [0, 3, 4, 4, 4].map((seconds) => seconds + latency);
  scenes.forEach(({ at }, index) => expect(Math.abs(at - due[index]), `scene message ${index}`).toBeLessThanOrEqual(FRAME_SECONDS));
});

test('an arrangement without blocks plays to its end', () => {
  const { scheduler, advance } = fakePlayback();
  scheduler.load({ bpm: 120, events: SONG.events.filter((event) => !event.scene) }, SONG_SECONDS);
  scheduler.play();
  advance(SONG_SECONDS - 1);
  expect(scheduler.isPlaying()).toBe(true);
  advance(2);
  expect(scheduler.isPlaying()).toBe(false);
});

test('an event an edit adds before the next one is sent on time', () => {
  const { scheduler, sent, advance } = fakePlayback();
  scheduler.play();
  advance(0.5);
  const withRed = [...SONG.events.slice(0, 4), { beats: 2, seconds: 1, type: 'note_on', scene: 40, velocity: 127 }, ...SONG.events.slice(4)];
  scheduler.load({ ...SONG, events: withRed }, SONG_SECONDS);
  advance(1);

  const added = sent.find(({ message }) => message[1] === 40);
  expect(Math.abs(added.at - 1)).toBeLessThanOrEqual(FRAME_SECONDS);
});

test('a seek mid-block releases what it started, sends the position and fires the blocks playing there', () => {
  const { scheduler, advance, since } = fakePlayback();
  scheduler.play();
  advance(3.5);
  since();

  scheduler.seek(5);
  expect(since(), 'beat 10 is sixteenth 40').toEqual([[0x81, 36, 0], [0x81, 37, 0], [0xf2, 40, 0], [0x91, 42, 127]]);
  advance(3.1);
  expect(since(), 'then on to the end').toEqual([[0x81, 42, 0]]);

  scheduler.play(3.5);
  since();
  scheduler.seek(1);
  expect(since(), 'back into the first block, which plays again; the other one waits for its beat').toEqual([
    [0x81, 36, 0],
    [0x81, 37, 0],
    [0xf2, 8, 0],
    [0x91, 36, 127],
  ]);
  advance(2.1);
  expect(since()).toEqual([[0x91, 37, 64]]);
});

test('pause releases everything it started and resumes where it stopped', () => {
  const { scheduler, advance, since } = fakePlayback();
  scheduler.play();
  advance(3.5);
  since();

  scheduler.pause();
  expect(since()).toEqual([[0x81, 36, 0], [0x81, 37, 0]]);
  advance(20);
  expect(since(), 'nothing while paused').toEqual([]);
  expect(scheduler.position()).toBe(3.5);

  scheduler.play();
  expect(since()).toEqual([[0xc1, 1], [0xf2, 28, 0], [0x91, 36, 127], [0x91, 37, 64]]);
});

test('an edit while playing takes effect from the next event, releasing a block taken out', () => {
  const { scheduler, advance, since } = fakePlayback();
  scheduler.play();
  advance(3.5);
  since();

  const withoutBlue = SONG.events.filter((event) => event.scene !== 37);
  const later = withoutBlue.map((event) => (event.scene === 42 ? { ...event, beats: event.beats + 2, seconds: event.seconds + 1 } : event));
  scheduler.load({ ...SONG, events: later }, SONG_SECONDS);
  expect(since()).toEqual([[0x81, 37, 0]]);
  advance(1);
  expect(since()).toEqual([[0x81, 36, 0]]);
  advance(0.6);
  expect(since()).toEqual([[0x91, 42, 127]]);
});

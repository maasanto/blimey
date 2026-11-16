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

/** A scheduler on a fake clock, recording what it sends and when. */
function fakePlayback() {
  let now = 0;
  let nextTimer = 0;
  const timers = new Map();
  const sent = [];
  const clicks = [];
  const scheduler = createScheduler({
    clock: () => now,
    send: (message) => sent.push({ at: now, message }),
    click: (beat) => clicks.push({ at: now, beat }),
    setTimer: (wake, ms) => {
      nextTimer += 1;
      timers.set(nextTimer, { wake, at: now + Math.max(0, ms) / 1000 });
      return nextTimer;
    },
    clearTimer: (timer) => timers.delete(timer),
  });
  scheduler.load(SONG);
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
  return { scheduler, sent, clicks, advance, since };
}

test('playing from the top sends the song, then every event within a frame of its time, and stops at the end', () => {
  const { scheduler, sent, clicks, advance } = fakePlayback();
  scheduler.play();
  advance(10);

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
  expect(clicks.map(({ beat }) => beat)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
  clicks.forEach(({ at, beat }) => expect(Math.abs(at - beat / 2)).toBeLessThanOrEqual(FRAME_SECONDS));
  expect(scheduler.isPlaying()).toBe(false);
  expect(scheduler.position()).toBe(6);
});

test('a seek mid-block releases what it started, sends the position and fires the blocks playing there', () => {
  const { scheduler, advance, since } = fakePlayback();
  scheduler.play();
  advance(3.5);
  since();

  scheduler.seek(5);
  expect(since(), 'beat 10 is sixteenth 40').toEqual([[0x81, 36, 0], [0x81, 37, 0], [0xf2, 40, 0], [0x91, 42, 127]]);
  advance(1.1);
  expect(since()).toEqual([[0x81, 42, 0]]);

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
  scheduler.load({ ...SONG, events: later });
  expect(since()).toEqual([[0x81, 37, 0]]);
  advance(1);
  expect(since()).toEqual([[0x81, 36, 0]]);
  advance(0.6);
  expect(since()).toEqual([[0x91, 42, 127]]);
});

// Plays a song's arrangement events (docs/api.md, Arrangement events) through `send`, timed on
// `clock`, the audio clock while the song's audio plays: the browser is the clock, as Reaper is
// on stage (plan §17, Playback in Blimey). Pure, so tests run it on a fake clock and timers.

import { NOTE_OFF, NOTE_ON, PROGRAM_CHANGE } from './audition.js';

const SONG_POSITION = 0xf2;
/** Song Position Pointer counts sixteenth notes, four to the beat, in 14 bits. */
const SIXTEENTHS_PER_BEAT = 4;
const HIGHEST_POSITION = 0x3fff;
const DATA_BITS = 7;
const DATA_MASK = 0x7f;

/**
 * `clock()` is the time in seconds; `send(message)` sends one MIDI message; `click(beat)` sounds
 * the click on each beat from the top of the song; `onstart(seconds)` and `onstop()` say when
 * playback starts from a point of the song and stops, to start and stop the audio with it.
 * `setTimer` and `clearTimer` default to the browser's.
 */
export function createScheduler({ clock, send, click = () => {}, onstart = () => {}, onstop = () => {}, setTimer = setTimeout, clearTimer = clearTimeout }) {
  let timeline = [];
  let bpm = 120;
  let program = null;
  let end = 0;
  /** While playing: the clock and the song's second at the last start. */
  let origin = null;
  let stoppedAt = 0;
  let cursor = 0;
  let timer = null;
  /** The scenes this playback started and hasn't released. */
  const held = new Set();

  const position = () => (origin ? origin.song + clock() - origin.clock : stoppedAt);
  const sixteenths = (seconds) => Math.min(HIGHEST_POSITION, Math.max(0, Math.round((seconds * bpm * SIXTEENTHS_PER_BEAT) / 60)));
  /** On the Song Position Pointer's grid, so the engine's bar position matches the playhead. */
  const onGrid = (seconds) => (sixteenths(seconds) * 60) / (bpm * SIXTEENTHS_PER_BEAT);

  function fire(event) {
    if (event.type === 'note_on') {
      send([NOTE_ON, event.scene, event.velocity]);
      held.add(event.scene);
    } else if (event.type === 'note_off' && held.delete(event.scene)) {
      send([NOTE_OFF, event.scene, 0]);
    } else if (event.type === 'click') {
      click(event.beat);
    }
  }

  function release(scene) {
    held.delete(scene);
    send([NOTE_OFF, scene, 0]);
  }

  function wake() {
    const now = position();
    while (cursor < timeline.length && timeline[cursor].seconds <= now) fire(timeline[cursor++]);
    if (cursor < timeline.length) timer = setTimer(wake, (timeline[cursor].seconds - now) * 1000);
    else stop(end);
  }

  function start(from) {
    const at = onGrid(from);
    const pointer = sixteenths(from);
    send([SONG_POSITION, pointer & DATA_MASK, pointer >> DATA_BITS]);
    const state = stateAt(timeline, at);
    for (const [scene, velocity] of state.playing) fire({ type: 'note_on', scene, velocity });
    cursor = state.next;
    origin = { clock: clock(), song: at };
    onstart(at);
    wake();
  }

  function stop(at) {
    if (!origin) return;
    clearTimer(timer);
    origin = null;
    stoppedAt = at;
    for (const scene of held) release(scene);
    onstop();
  }

  return {
    position,
    isPlaying: () => origin !== null,
    /**
     * Takes the song's events, `{ bpm, events }`. While playing, they take effect from the
     * next event, and a scene this playback started that no longer plays here is released.
     */
    load(song) {
      bpm = song.bpm;
      program = song.events.find((event) => event.type === 'program_change')?.program ?? null;
      timeline = withClicks(song.events, bpm);
      end = timeline.at(-1)?.seconds ?? 0;
      if (!origin) return;
      const state = stateAt(timeline, position());
      for (const scene of held) if (!state.playing.has(scene)) release(scene);
      cursor = state.next;
    },
    /** Plays from the paused point, or from `from` seconds; from the top once at the end. */
    play(from = stoppedAt) {
      if (origin || !timeline.length) return;
      if (program !== null) send([PROGRAM_CHANGE, program]);
      start(from < end ? from : 0);
    },
    pause: () => stop(position()),
    /** Releases what it started and plays on from `to`, firing the scenes playing there. */
    seek(to) {
      if (!origin) {
        stoppedAt = onGrid(to);
        return;
      }
      stop(to);
      start(to);
    },
  };
}

/**
 * What plays at `seconds`: the scene → velocity of each block started before it and not ended
 * by it, and the index of the first event still to come. At one time note offs come before note
 * ons and clicks, so a block ending there is over and one starting there is still to come.
 */
function stateAt(timeline, seconds) {
  const playing = new Map();
  let next = 0;
  for (; next < timeline.length; next += 1) {
    const event = timeline[next];
    if (event.seconds > seconds || (event.seconds === seconds && event.type !== 'note_off')) break;
    if (event.type === 'note_on') playing.set(event.scene, event.velocity);
    else if (event.type === 'note_off') playing.delete(event.scene);
  }
  return { playing, next };
}

/** The events with a click on each beat before the last, after the events on that beat. */
function withClicks(events, bpm) {
  const end = events.at(-1)?.beats ?? 0;
  const clicks = Array.from({ length: Math.ceil(end) }, (_, beat) => ({ type: 'click', beat, beats: beat, seconds: (beat * 60) / bpm }));
  return [...events, ...clicks].sort((a, b) => a.seconds - b.seconds || (a.type === 'click') - (b.type === 'click'));
}

// Audio for the playback tests and screenshots, written at test time rather than committed.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const SAMPLE_RATE = 8000;
const BEEP_HZ = 1000;
const BEEP_LEVEL = 20000;

/** `seconds` of 16-bit mono WAV: a 10 ms beep every half second, the beats at 120 BPM. */
export function beepsWav(seconds) {
  const samples = Math.round(seconds * SAMPLE_RATE);
  const wav = Buffer.alloc(44 + samples * 2);
  wav.write('RIFF', 0);
  wav.writeUInt32LE(36 + samples * 2, 4);
  wav.write('WAVEfmt ', 8);
  wav.writeUInt32LE(16, 16);
  wav.writeUInt16LE(1, 20);
  wav.writeUInt16LE(1, 22);
  wav.writeUInt32LE(SAMPLE_RATE, 24);
  wav.writeUInt32LE(SAMPLE_RATE * 2, 28);
  wav.writeUInt16LE(2, 32);
  wav.writeUInt16LE(16, 34);
  wav.write('data', 36);
  wav.writeUInt32LE(samples * 2, 40);
  for (let index = 0; index < samples; index += 1) {
    const isBeep = index % (SAMPLE_RATE / 2) < SAMPLE_RATE / 100;
    const level = isBeep ? Math.round(Math.sin((index / SAMPLE_RATE) * 2 * Math.PI * BEEP_HZ) * BEEP_LEVEL) : 0;
    wav.writeInt16LE(level, 44 + index * 2);
  }
  return wav;
}

/** A RIFF chunk, with the padding byte an odd-sized one takes. */
function riffChunk(id, body) {
  const header = Buffer.alloc(8);
  header.write(id, 0);
  header.writeUInt32LE(body.length, 4);
  return Buffer.concat([header, body, Buffer.alloc(body.length % 2)]);
}

/** `seconds` of beeps, as `beepsWav`, with a cue point named by a `labl` for each of `markers`, `{ name, at }` in seconds: as Reaper renders markers. */
export function markedWav(seconds, markers) {
  const CUE_POINT_BYTES = 24;
  const cue = Buffer.alloc(4 + markers.length * CUE_POINT_BYTES);
  cue.writeUInt32LE(markers.length, 0);
  markers.forEach(({ at }, index) => {
    const point = 4 + index * CUE_POINT_BYTES;
    const sample = Math.round(at * SAMPLE_RATE);
    cue.writeUInt32LE(index + 1, point);
    cue.writeUInt32LE(sample, point + 4);
    cue.write('data', point + 8);
    cue.writeUInt32LE(sample, point + 20);
  });
  const labels = markers.map(({ name }, index) => {
    const label = Buffer.alloc(4 + Buffer.byteLength(name) + 1);
    label.writeUInt32LE(index + 1, 0);
    label.write(name, 4);
    return riffChunk('labl', label);
  });
  const wav = Buffer.concat([beepsWav(seconds), riffChunk('cue ', cue), riffChunk('LIST', Buffer.concat([Buffer.from('adtl'), ...labels]))]);
  wav.writeUInt32LE(wav.length - 8, 4);
  return wav;
}

/**
 * Gives the page's audio contexts a clock that runs with `performance.now()` from their creation,
 * heard `outputLatency` seconds late, whatever the host's audio output does: a machine without a
 * working output stalls the real clock. The rest of Web Audio stays real.
 */
export function steadyAudioClock(page, { outputLatency = 0 } = {}) {
  return page.addInitScript((latency) => {
    const created = new WeakMap();
    const define = (name, get) => Object.defineProperty(AudioContext.prototype, name, { get, configurable: true });
    define('currentTime', function () {
      if (!created.has(this)) created.set(this, performance.now());
      return (performance.now() - created.get(this)) / 1000;
    });
    define('outputLatency', () => latency);
    define('baseLatency', () => 0);
    const resume = AudioContext.prototype.resume;
    // A host whose output never starts never resolves the real resume.
    AudioContext.prototype.resume = function () {
      resume.call(this).catch(() => {});
      return Promise.resolve();
    };
  }, outputLatency);
}

/**
 * Gives the arranged fixture's song in `show` `audio: { file }`, and writes 16 s of beeps there
 * unless `isMissing`. Returns the song file as it then is.
 */
export function withAudio(show, file, { isMissing = false } = {}) {
  const songFile = path.join(show, 'songs/arranged.yaml');
  const song = readFileSync(songFile, 'utf8').replace('bpm: 120\n', `bpm: 120\naudio: { file: ${file} }\n`);
  writeFileSync(songFile, song);
  if (!isMissing) {
    mkdirSync(path.dirname(path.join(show, file)), { recursive: true });
    writeFileSync(path.join(show, file), beepsWav(16));
  }
  return song;
}

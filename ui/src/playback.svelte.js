// Plays a song in the builder: its audio, decoded in the browser, and the scheduler sending its
// events on the audio clock; a click on each beat when there is no audio (plan §17).

import { createScheduler } from './scheduler.js';

/** The waveform's resolution: one peak per 10 ms of audio. */
export const PEAKS_PER_SECOND = 100;
const CLICK_HZ = 1000;
const CLICK_SECONDS = 0.03;
const CLICK_GAIN = 0.3;

export class Playback {
  /** Where the song is, in seconds from bar 1. */
  position = $state(0);
  isPlaying = $state(false);
  /** The song's audio and its peaks, `{ buffer, peaks }`; null without audio, or until it decodes. */
  audio = $state.raw(null);
  /** The song names an audio file the engine doesn't have: it plays silent, with the click. */
  isAudioMissing = $state(false);
  /** Where bar 1 falls in the audio, in seconds. */
  #offset = 0;
  #context = new AudioContext();
  #source = null;
  #frame = null;
  #scheduler;
  /** Counts audio loads, so a slow one that a newer one replaced is dropped. */
  #loads = 0;

  /** `send(message)` sends one MIDI message to the engine. */
  constructor(send) {
    this.#scheduler = createScheduler({
      clock: () => this.#context.currentTime,
      send,
      click: () => {
        if (!this.audio) this.#click();
      },
      onstart: (seconds) => this.#start(seconds),
      onstop: () => this.#stop(),
    });
  }

  /** The song's events, `GET /api/songs/<song>/arrangement/events`. */
  load(song) {
    this.#scheduler.load(song);
  }

  /** Decodes the audio at `url`, or plays with the click for none; a missing file (`404`) plays silent too. */
  async loadAudio(url) {
    const load = ++this.#loads;
    this.audio = null;
    this.isAudioMissing = false;
    if (!url) return;
    const response = await fetch(url);
    if (load !== this.#loads) return;
    if (response.status === 404) {
      this.isAudioMissing = true;
      return;
    }
    if (!response.ok) throw new Error(`The song's audio didn't load: ${response.status} ${response.statusText}.`);
    const buffer = await this.#context.decodeAudioData(await response.arrayBuffer());
    if (load !== this.#loads) return;
    this.audio = { buffer, peaks: peaksOf(buffer) };
    if (this.isPlaying) this.#restartAudio();
  }

  set offset(seconds) {
    if (seconds === this.#offset) return;
    this.#offset = seconds;
    if (this.isPlaying) this.#restartAudio();
  }

  async play() {
    await this.#context.resume();
    // The audio clock stands still for a moment after resuming (over 100 ms in Chromium), then
    // the audio plays from where it stood: start on it once it moves, so the first scenes land
    // with the music rather than ahead of it.
    const stood = this.#context.currentTime;
    while (this.#context.currentTime === stood) await new Promise(requestAnimationFrame);
    this.#scheduler.play(this.position);
  }

  pause() {
    this.#scheduler.pause();
  }

  seek(seconds) {
    this.#scheduler.seek(seconds);
    this.position = this.#scheduler.position();
  }

  close() {
    this.#scheduler.pause();
    this.#context.close();
  }

  #start(seconds) {
    this.isPlaying = true;
    this.#playAudio(seconds);
    const follow = () => {
      this.position = this.#scheduler.position();
      this.#frame = requestAnimationFrame(follow);
    };
    follow();
  }

  #stop() {
    cancelAnimationFrame(this.#frame);
    this.#source?.stop();
    this.#source = null;
    this.isPlaying = false;
    this.position = this.#scheduler.position();
  }

  #playAudio(seconds) {
    const at = this.#offset + seconds;
    if (!this.audio || at >= this.audio.buffer.duration) return;
    this.#source = new AudioBufferSourceNode(this.#context, { buffer: this.audio.buffer });
    this.#source.connect(this.#context.destination);
    this.#source.start(0, at);
  }

  #restartAudio() {
    this.#source?.stop();
    this.#source = null;
    this.#playAudio(this.#scheduler.position());
  }

  #click() {
    const now = this.#context.currentTime;
    const tone = new OscillatorNode(this.#context, { frequency: CLICK_HZ });
    const gain = new GainNode(this.#context, { gain: CLICK_GAIN });
    gain.gain.exponentialRampToValueAtTime(0.001, now + CLICK_SECONDS);
    tone.connect(gain).connect(this.#context.destination);
    tone.start(now);
    tone.stop(now + CLICK_SECONDS);
  }
}

/** The loudest sample of each stretch of `PEAKS_PER_SECOND`, 0–1, over the audio's channels. */
function peaksOf(buffer) {
  const stretch = Math.max(1, Math.round(buffer.sampleRate / PEAKS_PER_SECOND));
  const peaks = new Float32Array(Math.ceil(buffer.length / stretch));
  for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
    const samples = buffer.getChannelData(channel);
    for (let index = 0; index < samples.length; index += 1) {
      const peak = Math.floor(index / stretch);
      peaks[peak] = Math.max(peaks[peak], Math.abs(samples[index]));
    }
  }
  return peaks;
}

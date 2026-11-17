// Plays a song in the builder: its audio, decoded in the browser, and the scheduler sending its
// events on the audio clock; a click on each beat when there is no audio (plan §17).

import { request } from './engine.svelte.js';
import { createScheduler } from './scheduler.js';

/** The waveform's resolution: one peak per 10 ms of audio. */
export const PEAKS_PER_SECOND = 100;
const CLICK_HZ = 1000;
const CLICK_SECONDS = 0.03;
const CLICK_GAIN = 0.3;
/** How long the audio clock may take to start after Play, well over Chromium's ~135 ms. */
const CLOCK_START_MS = 2000;
const CLOCK_POLL_MS = 5;

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
  /** The click track of the playback under way, through one gain to silence it at once. */
  #clicks = null;
  #frame = null;
  #scheduler;
  #bpm = 120;
  #seconds = 0;
  /** How long after the audio clock the sound is heard, as read at the last start. */
  #latency = 0;
  /** Counts audio loads, so a slow one that a newer one replaced is dropped. */
  #loads = 0;
  /** The URL of the audio loaded or loading; undefined before the first load and after a failed one. */
  #audioUrl;

  /** Why playback last stopped on its own, until the timeline shows it; null otherwise. */
  failure = $state(null);
  /** Counts uploads of the song's audio, so a new file under the same name loads anew rather than from the cache. */
  uploads = $state(0);
  #sending = Promise.resolve();

  constructor() {
    this.#scheduler = createScheduler({
      clock: () => this.#context.currentTime,
      latency: () => {
        this.#latency = (this.#context.outputLatency ?? 0) + this.#context.baseLatency;
        return this.#latency;
      },
      send: (message) => this.#send(message),
      onstart: (seconds) => this.#start(seconds),
      onstop: () => this.#stop(),
    });
  }

  /** The song's events, `GET /api/songs/<song>/arrangement/events`, and its length in `seconds`. */
  load(song, seconds) {
    this.#bpm = song.bpm;
    this.#seconds = seconds;
    this.#scheduler.load(song, seconds);
  }

  /** Decodes the audio at `url`, or plays with the click for none; a missing file (`404`) plays silent too. */
  async loadAudio(url) {
    // A timeline opened again on the song playing keeps its audio, which plays on uncut; a missing file is looked for again.
    if (url === this.#audioUrl && !this.isAudioMissing) return;
    this.#audioUrl = url;
    const load = ++this.#loads;
    this.audio = null;
    this.isAudioMissing = false;
    if (!url) return;
    try {
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
    } catch (failure) {
      // Opening the timeline again tries once more.
      if (load === this.#loads) this.#audioUrl = undefined;
      throw failure;
    }
  }

  set offset(seconds) {
    if (seconds === this.#offset) return;
    this.#offset = seconds;
    if (this.isPlaying && this.audio) this.#restartAudio();
  }

  async play() {
    await this.#context.resume();
    // The audio clock stands still for a moment after resuming (over 100 ms in Chromium), then
    // the audio plays from where it stood: start on it once it moves, so the first scenes land
    // with the music rather than ahead of it.
    const stood = this.#context.currentTime;
    const deadline = performance.now() + CLOCK_START_MS;
    while (this.#context.currentTime === stood) {
      if (this.#context.state === 'closed') return;
      if (performance.now() > deadline) {
        throw new Error("The browser's audio output didn't start, so the song can't play: check that the browser may play sound and an output device is connected, then press Play again.");
      }
      await new Promise((resolve) => setTimeout(resolve, CLOCK_POLL_MS));
    }
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

  /**
   * Sends MIDI one message at a time, each once the one before is in, so the engine takes them
   * in order: parallel requests could land a note before its song. A failure stops playback.
   */
  #send(message) {
    this.#sending = this.#sending
      .then(() => request('POST', '/api/midi', { message }))
      .catch((failure) => {
        this.pause();
        this.failure = failure;
      });
  }

  #start(seconds) {
    this.isPlaying = true;
    if (this.audio) this.#playAudio(seconds);
    else this.#playClicks(seconds);
    const follow = () => {
      this.position = this.#scheduler.position();
      this.#frame = requestAnimationFrame(follow);
    };
    follow();
  }

  #stop() {
    cancelAnimationFrame(this.#frame);
    this.#silence();
    this.isPlaying = false;
    this.position = this.#scheduler.position();
  }

  #silence() {
    this.#source?.stop();
    this.#source = null;
    this.#clicks?.disconnect();
    this.#clicks = null;
  }

  /** Plays the audio from the song's second `seconds`, starting now: it is heard as the scheduler plays it. */
  #playAudio(seconds) {
    const at = this.#offset + seconds;
    if (at >= this.audio.buffer.duration) return;
    this.#source = new AudioBufferSourceNode(this.#context, { buffer: this.audio.buffer });
    this.#source.connect(this.#context.destination);
    this.#source.start(0, at);
  }

  /** Puts the audio, now loaded or moved, where the scheduler will be once what starts now is heard. */
  #restartAudio() {
    this.#silence();
    this.#playAudio(this.#scheduler.position() + this.#latency);
  }

  /** Schedules a click on each beat from the song's second `seconds` to its end, on the audio clock from now. */
  // ponytail: every click of the song is scheduled at once; schedule a few seconds ahead if long songs ever stutter.
  #playClicks(seconds) {
    const now = this.#context.currentTime;
    this.#clicks = new GainNode(this.#context, { gain: CLICK_GAIN });
    this.#clicks.connect(this.#context.destination);
    const beatSeconds = 60 / this.#bpm;
    for (let beat = Math.ceil(seconds / beatSeconds); beat * beatSeconds < this.#seconds; beat += 1) {
      const at = now + beat * beatSeconds - seconds;
      const tone = new OscillatorNode(this.#context, { frequency: CLICK_HZ });
      const envelope = new GainNode(this.#context);
      envelope.gain.setValueAtTime(1, at);
      envelope.gain.exponentialRampToValueAtTime(0.001, at + CLICK_SECONDS);
      tone.connect(envelope).connect(this.#clicks);
      tone.start(at);
      tone.stop(at + CLICK_SECONDS);
    }
  }
}

/** The song's playback last opened, so it plays on while another tab or screen is shown. */
let kept = null;

/** The playback of the song `songName`: the one kept when it is that song's, else a new one, which stops the other. */
export function playbackOf(songName) {
  if (kept?.songName === songName) return kept.playback;
  kept?.playback.close();
  kept = { songName, playback: new Playback() };
  return kept.playback;
}

/** Pauses the song playing, if any: each scene it fires would clear the preview of the scene being edited. */
export const pausePlayback = () => kept?.playback.pause();

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

// Pixel thumbnails rendered by the engine (docs/api.md, Pixel thumbnail), so pickers show what
// the lights will do without re-implementing an effect.

import { request } from './engine.svelte.js';

/** Waits for a slider to settle before rendering a picker's worth of thumbnails. */
export const THUMBNAIL_DEBOUNCE_MS = 150;

export class Thumbnails {
  /** Key → `POST /api/pixel-thumbnail` result, or null when it failed; replaced whole. */
  byKey = $state.raw({});
  /** Why thumbnails are missing, from the engine. */
  error = $state('');
  #generation = 0;

  /**
   * Renders each `[key, layer]` as a one-layer cue in `song`'s time signature; what an earlier
   * call returns late is dropped.
   */
  load(variants, seed, song) {
    this.#generation += 1;
    const current = this.#generation;
    this.error = '';
    const path = song ? `/api/pixel-thumbnail?song=${encodeURIComponent(song)}` : '/api/pixel-thumbnail';
    for (const [key, layer] of variants) {
      request('POST', path, { name: 'thumbnail', type: 'pixel', seed, layers: [layer] })
        .then((thumbnail) => {
          if (current === this.#generation) this.byKey = { ...this.byKey, [key]: thumbnail };
        })
        .catch((failure) => {
          if (current !== this.#generation) return;
          this.byKey = { ...this.byKey, [key]: null };
          this.error = failure.message;
        });
    }
  }
}

/**
 * The frame of `thumbnail` showing `now` ms into its loop, played over its `cycle_ms`, or
 * undefined without one. The same array until the next frame is due, so a `{@const}` of it
 * redraws only then.
 */
export function frameAt(thumbnail, now) {
  if (!thumbnail) return undefined;
  const { frames } = thumbnail;
  return frames[Math.floor((now * frames.length) / thumbnail.cycle_ms) % frames.length];
}

/** Draws `frame` of `thumbnail` on `canvas`, one canvas pixel per thumbnail pixel. */
export function paint(canvas, thumbnail, frame) {
  const context = canvas.getContext('2d');
  if (!frame) return context.clearRect(0, 0, canvas.width, canvas.height);
  frame.forEach((colour, index) => {
    context.fillStyle = colour;
    context.fillRect(index % thumbnail.columns, Math.floor(index / thumbnail.columns), 1, 1);
  });
}

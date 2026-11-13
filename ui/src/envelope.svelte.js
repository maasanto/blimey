// Curves as the engine plays them, sampled by `POST /api/envelope`, so the editors draw what
// the lights will do rather than a copy of the formulas.

import { request } from './engine.svelte.js';

export const SHAPES = ['ramp-up', 'ramp-down', 'ease-in', 'sine', 'hold'];
/** Levels per curve: index 25 is a quarter of the way through. */
export const SAMPLES = 101;

const sampled = $state({});
const requested = new Set();

/** The curve's levels, 0–1, from start to end; undefined until the engine has answered. */
export function curveLevels(curve) {
  const key = JSON.stringify(curve);
  if (!requested.has(key)) {
    requested.add(key);
    request('POST', '/api/envelope', { curve, samples: SAMPLES }).then(
      ({ levels }) => (sampled[key] = levels),
      (failure) => {
        requested.delete(key);
        throw failure;
      },
    );
  }
  return sampled[key];
}

/** SVG polyline points for `levels` across `width` × `height`, level 1 at the top. */
export function curvePoints(levels, { x = 0, width, height, scale = 1 }) {
  const last = levels.length - 1;
  return levels.map((level, index) => `${x + (width * index) / last},${height * (1 - level * scale)}`).join(' ');
}

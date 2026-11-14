// Durations as song files write them (docs/api.md): `500ms`, `2s`, `0.5 beats`, `4 bars`.

const DURATION = /^\s*([0-9]+(?:\.[0-9]+)?)\s*(ms|s|beats?|bars?)\s*$/;

/** `{ count, unit }` with unit `ms`, `s`, `beats` or `bars`; null for text that isn't a duration. */
export function parseLength(text) {
  const match = DURATION.exec(text ?? '');
  if (!match) return null;
  const unit = { beat: 'beats', bar: 'bars' }[match[2]] ?? match[2];
  return { count: Number(match[1]), unit };
}

/** As a song file writes it: `1 bar`, `0.5 beats`, `500ms`. */
export function formatLength(count, unit) {
  if (unit === 'ms' || unit === 's') return `${count}${unit}`;
  return `${count} ${count === 1 ? unit.slice(0, -1) : unit}`;
}

/** Quarter-note beats per bar, as the engine counts them: 4 in 4/4, 3 in 6/8. */
export function beatsPerBar(timeSignature = '4/4') {
  const [numerator, denominator] = timeSignature.split('/').map(Number);
  return (numerator * 4) / denominator;
}

/** The length in beats, or null for one in time rather than beats or bars. */
export function lengthInBeats({ count, unit }, barBeats) {
  return { beats: count, bars: count * barBeats }[unit] ?? null;
}

/** `text`, a length as a song file writes it, in beats: 0 for one in time, or for text that isn't a length. */
export const beatsIn = (text, barBeats) => lengthInBeats(parseLength(text) ?? { count: 0, unit: 'beats' }, barBeats) ?? 0;

// Auditions through `POST /api/midi`, as Reaper would fire a scene (docs/api.md, Scenes and
// songs in the builder): what scene pads and timeline blocks share.

// Until the Reaper control chunk, song N is channel-2 Program Change N and scene N channel-2
// note N (docs/api.md, Scenes and songs in the builder).
export const NOTE_ON = 0x91;
export const NOTE_OFF = 0x81;
export const PROGRAM_CHANGE = 0xc1;

/** A press held this long auditions; a shorter one is a click. */
export const HOLD_MS = 250;
export const FULL_VELOCITY = 127;

/**
 * Starts `onhold()` once a press has been held for `HOLD_MS`, unless `isPlaying()`: a scene
 * already playing, from Reaper or elsewhere, is left to play. `onheld()` is told when the hold
 * time passes either way. `release(onrelease)` ends the press, calling `onrelease()` once an
 * audition it started has been sent; `cancel()` ends it before the hold time.
 */
export function holdToAudition({ isPlaying, onhold, onheld }) {
  let started = null;
  const timer = setTimeout(() => {
    onheld?.();
    if (!isPlaying()) started = onhold();
  }, HOLD_MS);
  return {
    cancel: () => clearTimeout(timer),
    release(onrelease) {
      clearTimeout(timer);
      started?.then(onrelease);
      started = null;
    },
  };
}

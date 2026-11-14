<script>
  import { engine, request } from './engine.svelte.js';
  import { curvePoints } from './envelope.svelte.js';

  /**
   * `cues`: the song file's cues, note → cue. `selected`: the note open in the editor.
   * `isPlaying(note)`: whether the engine plays that cue. `onopen(note)` opens a pad, empty or
   * not; `onhold(note)` starts an audition and resolves once it is sent, `onrelease(note)` ends
   * it; `onmove(from, to)` moves a cue to another pad.
   */
  let { songName, cues = {}, selected, editable, isPlaying, onopen, onhold, onrelease, onmove } = $props();

  const PADS_PER_BANK = 16;
  /** Banks line up on note 36 (C1), where drum pads and the show's cues start. */
  const ALIGNED_ON = 36;
  const LAST_NOTE = 127;
  const FIRST_BANK = ALIGNED_ON - PADS_PER_BANK * Math.ceil(ALIGNED_ON / PADS_PER_BANK);
  const BANKS = Array.from({ length: Math.ceil((LAST_NOTE + 1 - FIRST_BANK) / PADS_PER_BANK) }, (_, index) => FIRST_BANK + PADS_PER_BANK * index);
  /** A press held this long auditions; a shorter one opens the pad. */
  const HOLD_MS = 250;
  /** A press that moves this far drags the cue to another pad. */
  const DRAG_PX = 6;
  /** Thumbnails are 8 frames over one cycle: about a second a loop, as in the pixel editor. */
  const THUMBNAIL_FRAME_MS = 125;
  // The 0–1 canvas at 16:9, as on the front view.
  const WIDTH = 160;
  const HEIGHT = 90;
  const FIXTURE_RADIUS = 6;

  let bank = $state(ALIGNED_ON);
  let tick = $state(0);
  /** `GET …/thumbnails` for `song`: note → thumbnail, replaced whole by the next one. */
  let thumbnails = $state.raw({ song: null, byNote: {} });
  let thumbnailError = $state('');
  /** The press on a pad: a click until it is held or moved. */
  let pressed = $state(null);
  let isClickSuppressed = false;

  const notesOf = (start) =>
    Array.from({ length: PADS_PER_BANK }, (_, index) => start + index).filter((note) => note >= 0 && note <= LAST_NOTE);
  const notes = $derived(notesOf(bank));
  const range = (start) => {
    const shown = notesOf(start);
    return `${shown[0]}–${shown.at(-1)}`;
  };
  const cueCount = (start) => notesOf(start).filter((note) => cues[note]).length;

  /**
   * Every pixel of the rig on the thumbnail. A fixture's pixels are drawn larger than life, so
   * a wash reads as one bar of colour; a grid's are sized to their spacing, so it reads as a grid.
   */
  const lights = $derived([
    ...(engine.rig?.fixtures ?? []).flatMap(({ name, pixels }) => pixels.map((pixel, index) => ({ name, index, ...pixel, radius: FIXTURE_RADIUS }))),
    ...(engine.rig?.grids ?? []).flatMap(({ name, pixels }) => {
      const spacing = Math.min(gap(pixels.map((pixel) => pixel.x)) * WIDTH, gap(pixels.map((pixel) => pixel.y)) * HEIGHT);
      const radius = Math.min(FIXTURE_RADIUS, 0.45 * spacing);
      return pixels.map((pixel, index) => ({ name, index, ...pixel, radius }));
    }),
  ]);

  function gap(positions) {
    const sorted = [...new Set(positions)].sort((a, b) => a - b);
    return Math.min(...sorted.slice(1).map((position, index) => position - sorted[index]));
  }

  $effect(() => {
    const timer = setInterval(() => (tick += 1), THUMBNAIL_FRAME_MS);
    return () => clearInterval(timer);
  });

  // Rendered by the engine from the saved song, again whenever it or the rig changes.
  $effect(() => {
    const song = songName;
    cues;
    engine.rig;
    let isCurrent = true;
    request('GET', `/api/songs/${song}/thumbnails`)
      .then((byNote) => {
        if (!isCurrent) return;
        thumbnails = { song, byNote };
        thumbnailError = '';
      })
      .catch((failure) => {
        if (!isCurrent) return;
        thumbnails = { song, byNote: {} };
        thumbnailError = failure.message;
      });
    return () => (isCurrent = false);
  });

  const thumbnailOf = (note) => (thumbnails.song === songName ? thumbnails.byNote[note] : undefined);

  function press(event, note) {
    if (event.button !== 0 || !cues[note]) return;
    pressed = { note, x: event.clientX, y: event.clientY, gesture: 'click', over: null, started: null };
    pressed.timer = setTimeout(() => {
      pressed.gesture = 'hold';
      pressed.started = onhold(note);
    }, HOLD_MS);
  }

  function drag(event) {
    if (!pressed || pressed.gesture === 'hold') return;
    if (pressed.gesture === 'click') {
      if (Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) < DRAG_PX) return;
      clearTimeout(pressed.timer);
      pressed.gesture = 'drag';
    }
    const under = document.elementFromPoint(event.clientX, event.clientY);
    // Dragging over another bank's button opens it, to drop the cue there.
    const bankButton = under?.closest('[data-bank]');
    if (bankButton) bank = Number(bankButton.dataset.bank);
    const pad = under?.closest('[data-pad]');
    pressed.over = pad ? Number(pad.dataset.pad) : null;
  }

  function letGo() {
    if (!pressed) return;
    const { note, gesture, over, started, timer } = pressed;
    clearTimeout(timer);
    pressed = null;
    // The click that follows a hold or a drag isn't one.
    isClickSuppressed = gesture !== 'click';
    setTimeout(() => (isClickSuppressed = false));
    if (gesture === 'hold') started.then(() => onrelease(note));
    if (gesture === 'drag' && over !== null && over !== note) onmove(note, over);
  }

  function click(note) {
    if (!isClickSuppressed) onopen(note);
  }
</script>

<svelte:window onpointermove={drag} onpointerup={letGo} onpointercancel={letGo} />

{#snippet picture(thumbnail)}
  <svg viewBox="0 -6 {WIDTH} {HEIGHT + 12}" class="thumbnail" aria-hidden="true">
    {#if thumbnail?.levels}
      <polyline class="level" points={curvePoints(thumbnail.levels, { width: WIDTH, height: HEIGHT })} />
    {:else if thumbnail?.frames}
      {@const frame = thumbnail.frames[tick % thumbnail.frames.length]}
      {#each lights as light (`${light.name}:${light.index}`)}
        <circle cx={light.x * WIDTH} cy={(1 - light.y) * HEIGHT} r={light.radius} fill={frame[light.name]?.[light.index] ?? '#000000'} />
      {/each}
    {/if}
  </svg>
{/snippet}

<section class="bank" aria-label="Pads">
  <div class="banks" role="group" aria-label="Banks">
    {#each BANKS as start (start)}
      {@const count = cueCount(start)}
      <button
        data-bank={start}
        class:filled={count}
        aria-pressed={bank === start}
        aria-label="Notes {range(start)}{count ? `, ${count} ${count === 1 ? 'cue' : 'cues'}` : ''}"
        onclick={() => (bank = start)}
      >
        {range(start)}
      </button>
    {/each}
  </div>
  {#if thumbnailError}<p class="error" role="alert">Thumbnails: {thumbnailError}</p>{/if}
  <div class="pads">
    {#each notes as note (note)}
      {@const cue = cues[note]}
      <button
        class="pad"
        class:empty={!cue}
        class:playing={isPlaying(note)}
        class:held={pressed?.gesture === 'hold' && pressed.note === note}
        class:target={pressed?.gesture === 'drag' && pressed.over === note && note !== pressed.note}
        data-pad={note}
        aria-current={selected === note || undefined}
        aria-label={cue ? `Pad ${note}: ${cue.name}` : `Pad ${note}: empty`}
        disabled={!editable}
        onpointerdown={(event) => press(event, note)}
        onclick={() => click(note)}
      >
        {#if cue}
          {@render picture(thumbnailOf(note))}
        {:else}
          <span class="thumbnail add">+</span>
        {/if}
        <span class="label">
          <span class="name">{cue?.name ?? 'New cue'}</span>
          <span class="note">{note}</span>
        </span>
      </button>
    {/each}
  </div>
  <p class="hint">
    {#if editable}
      Click a pad to open its cue, or an empty one to make one. Hold a pad to audition it; drag it to an empty pad to move it.
    {:else}
      Show mode: the pads are locked.
    {/if}
  </p>
</section>

<style>
  .bank {
    margin-bottom: 16px;
  }

  .banks {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 10px;
  }

  .banks button {
    padding: 2px 8px;
    font: 12px var(--mono);
    color: var(--muted);
  }

  .banks button.filled {
    color: var(--text);
  }

  .banks button.filled::after {
    content: '';
    display: inline-block;
    width: 6px;
    height: 6px;
    margin-left: 5px;
    vertical-align: middle;
    background: var(--accent);
    border-radius: 50%;
  }

  .banks button[aria-pressed='true'] {
    outline: 2px solid var(--accent);
  }

  .pads {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 8px;
  }

  .pad {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px;
    text-align: left;
    background: #1c1e24;
    border-radius: 8px;
    user-select: none;
    touch-action: none;
  }

  .pad.empty {
    background: none;
    border-style: dashed;
  }

  .pad[aria-current] {
    outline: 2px solid var(--accent);
  }

  .pad.playing {
    border-color: var(--accent);
    box-shadow: 0 0 12px rgb(255 190 80 / 0.45);
  }

  .pad.held {
    transform: scale(0.97);
  }

  .pad.target {
    border: 1px solid var(--accent);
    background: #2a2618;
  }

  .thumbnail {
    display: block;
    width: 100%;
    aspect-ratio: 160 / 102;
    background: #101114;
    border-radius: 4px;
  }

  .add {
    display: grid;
    place-items: center;
    font-size: 22px;
    color: var(--muted);
    background: none;
  }

  .level {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
  }

  circle {
    stroke: #3a3d47;
    stroke-width: 0.5;
  }

  .label {
    display: flex;
    gap: 6px;
    align-items: baseline;
    min-width: 0;
    font-size: 13px;
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty .name {
    color: var(--muted);
  }

  .note {
    font: 12px var(--mono);
    color: var(--muted);
  }
</style>

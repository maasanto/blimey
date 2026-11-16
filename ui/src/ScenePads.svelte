<script module>
  /** What a pad dragged out carries: its scene's number. */
  export const SCENE_DRAG_TYPE = 'application/x-blimey-scene';
</script>

<script>
  import { engine, request } from './engine.svelte.js';
  import { holdToAudition } from './audition.js';
  import { curvePoints } from './envelope.svelte.js';

  /**
   * `cues`: the song file's scenes, number → scene. `isPlaying(number)`: whether the engine
   * plays that scene. `onopen(number)` opens a scene in the editor, `onnew()` a new one, and
   * without it there is no New scene pad; `onhold(number)` starts an audition and resolves once
   * it is sent, `onrelease(number)` ends it; `onaudition(number)` starts or ends one from the
   * keyboard. `editable`: false in show mode, where scenes still open, read-only, but don't
   * audition. `draggable`: pads drag out their scene's number, as `SCENE_DRAG_TYPE`. `tempo`:
   * the song's `bpm`, which thumbnails are rendered at.
   */
  let { songName, cues = {}, tempo, editable, isPlaying, onopen, onnew, onhold, onrelease, onaudition, draggable = false } = $props();

  /** Thumbnails are 8 frames over one cycle: about a second a loop, as in the pixel editor. */
  const THUMBNAIL_FRAME_MS = 125;
  // The 0–1 canvas at 16:9, as on the front view.
  const WIDTH = 160;
  const HEIGHT = 90;
  const FIXTURE_RADIUS = 6;
  /** Room around what is drawn, so pixels and curves on the edges show whole. */
  const MARGIN = 2 * FIXTURE_RADIUS;
  /** How wide a pad's picture may be for its height: a single row of washes, or a square grid. */
  const WIDEST = 3;
  const NARROWEST = 1;
  /** A pad never zooms in closer than this, so a small rig's pixels and a curve's stroke keep their scale. */
  const NARROWEST_FRAME = WIDTH / 2;
  /** How much of the space between a grid's pixels each one fills. */
  const GRID_PIXEL_FILL = 0.9;

  let tick = $state(0);
  /** `GET …/thumbnails` for `song`: number → thumbnail, replaced whole by the next one. */
  let thumbnails = $state.raw({ song: null, byNote: {} });
  let thumbnailError = $state('');
  /** The press on a pad: a click until it is held. */
  let pressed = $state(null);
  let isClickSuppressed = false;

  const numbers = $derived(
    Object.keys(cues)
      .map(Number)
      .sort((a, b) => a - b),
  );

  /**
   * Every pixel of the rig on the thumbnail. A fixture's pixels are drawn larger than life, so
   * a wash reads as one bar of colour; a grid's are sized to their spacing, so it reads as a grid.
   */
  const lights = $derived([
    ...(engine.rig?.fixtures ?? []).flatMap(({ name, pixels }) => pixels.map((pixel, index) => ({ name, index, ...pixel, radius: FIXTURE_RADIUS }))),
    ...(engine.rig?.grids ?? []).flatMap(({ name, pixels }) => {
      const spacing = Math.min(gap(pixels.map((pixel) => pixel.x)) * WIDTH, gap(pixels.map((pixel) => pixel.y)) * HEIGHT);
      const radius = Math.min(FIXTURE_RADIUS, (GRID_PIXEL_FILL * spacing) / 2);
      return pixels.map((pixel, index) => ({ name, index, ...pixel, radius }));
    }),
  ]);

  function gap(positions) {
    const sorted = [...new Set(positions)].sort((a, b) => a - b);
    return Math.min(...sorted.slice(1).map((position, index) => position - sorted[index]));
  }

  /**
   * The part of the canvas a pad shows, as an SVG viewBox: the rig's lit extent with a margin,
   * widened or heightened to a sensible ratio, so a single row of washes fills its pad.
   */
  const frame = $derived.by(() => {
    if (!lights.length) return { x: 0, y: -MARGIN, width: WIDTH, height: HEIGHT + 2 * MARGIN };
    const xs = lights.map((light) => light.x * WIDTH);
    const ys = lights.map((light) => (1 - light.y) * HEIGHT);
    let x = Math.min(...xs) - MARGIN;
    let y = Math.min(...ys) - MARGIN;
    let width = Math.max(...xs) + MARGIN - x;
    let height = Math.max(...ys) + MARGIN - y;
    if (width < NARROWEST_FRAME) {
      x -= (NARROWEST_FRAME - width) / 2;
      width = NARROWEST_FRAME;
    }
    if (width / height > WIDEST) {
      const wanted = width / WIDEST;
      y -= (wanted - height) / 2;
      height = wanted;
    } else if (width / height < NARROWEST) {
      const wanted = height * NARROWEST;
      x -= (wanted - width) / 2;
      width = wanted;
    }
    return { x, y, width, height };
  });
  const viewBox = $derived(`${frame.x} ${frame.y} ${frame.width} ${frame.height}`);
  const pictureRatio = $derived(`${frame.width} / ${frame.height}`);

  /** The numbers of the pads on screen: only those animate, the others hold their first frame. */
  let inView = $state.raw(new Set());
  const watcher = new IntersectionObserver((entries) => {
    const shown = new Set(inView);
    for (const { target, isIntersecting } of entries) {
      const number = Number(target.dataset.pad);
      if (isIntersecting) shown.add(number);
      else shown.delete(number);
    }
    inView = shown;
  });
  $effect(() => () => watcher.disconnect());

  function watch(pad) {
    watcher.observe(pad);
    return () => watcher.unobserve(pad);
  }

  // Only while a pad on screen moves: still pads redraw nothing.
  const isAnimated = $derived(numbers.some((number) => inView.has(number) && thumbnailOf(number)?.frames?.length > 1));
  $effect(() => {
    if (!isAnimated) return;
    const timer = setInterval(() => (tick += 1), THUMBNAIL_FRAME_MS);
    return () => clearInterval(timer);
  });

  // A reload gives the song and the rig new objects for the same content: compared by content,
  // a save that leaves the scenes as they were doesn't render them all again.
  const cuesContent = $derived(JSON.stringify(cues));
  // Through a derived, as the prop's getter follows the whole song.
  const renderedTempo = $derived(tempo);
  // Groups come in no fixed order, so they are sorted to compare.
  const rigContent = $derived(JSON.stringify([engine.rig?.fixtures, engine.rig?.grids, Object.entries(engine.rig?.groups ?? {}).sort()]));

  // Rendered by the engine from the saved song, again whenever its scenes, tempo or the rig change.
  $effect(() => {
    const song = songName;
    cuesContent;
    renderedTempo;
    rigContent;
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

  const thumbnailOf = (number) => (thumbnails.song === songName ? thumbnails.byNote[number] : undefined);

  function press(event, number) {
    // One press at a time: another finger or pointer is ignored until this one lets go.
    if (!editable || pressed !== null || event.button !== 0) return;
    pressed = { pointer: event.pointerId, number, gesture: 'click' };
    pressed.hold = holdToAudition({ isPlaying: () => isPlaying(number), onhold: () => onhold(number), onheld: () => (pressed.gesture = 'hold') });
  }

  function letGo(event) {
    if (pressed === null || event.pointerId !== pressed.pointer) return;
    const { number, gesture, hold } = pressed;
    pressed = null;
    // The click that follows a hold isn't one.
    isClickSuppressed = gesture !== 'click';
    setTimeout(() => (isClickSuppressed = false));
    hold.release(() => onrelease(number));
  }

  function click(number) {
    if (!isClickSuppressed) onopen(number);
  }

  /** A pad dragged out is not held: its audition ends, or doesn't start. */
  function dragOut(event, number) {
    pressed?.hold.release(() => onrelease(number));
    pressed = null;
    event.dataTransfer.setData(SCENE_DRAG_TYPE, String(number));
    event.dataTransfer.effectAllowed = 'copy';
  }
</script>

<svelte:window onpointerup={letGo} onpointercancel={letGo} />

{#snippet picture(thumbnail, isMoving)}
  <svg {viewBox} class="thumbnail" style:aspect-ratio={pictureRatio} aria-hidden="true">
    {#if thumbnail?.levels}
      <polyline class="level" points={curvePoints(thumbnail.levels, { x: frame.x + MARGIN, y: frame.y + MARGIN, width: frame.width - 2 * MARGIN, height: frame.height - 2 * MARGIN })} />
    {:else if thumbnail?.frames}
      {@const colours = thumbnail.frames[isMoving ? tick % thumbnail.frames.length : 0]}
      {#each lights as light (`${light.name}:${light.index}`)}
        <circle cx={light.x * WIDTH} cy={(1 - light.y) * HEIGHT} r={light.radius} fill={colours[light.name]?.[light.index] ?? '#000000'} />
      {/each}
    {/if}
  </svg>
{/snippet}

<section aria-label="Scenes">
  {#if thumbnailError}<p class="error" role="alert">Thumbnails: {thumbnailError}</p>{/if}
  <ol class="pads">
    {#each numbers as number (number)}
      {@const isOn = isPlaying(number)}
      <li class="slot">
        <button
          class="pad"
          class:playing={isOn}
          class:held={pressed?.gesture === 'hold' && pressed.number === number}
          data-pad={number}
          aria-label="Scene {number}: {cues[number].name}"
          onpointerdown={(event) => press(event, number)}
          onclick={() => click(number)}
          oncontextmenu={(event) => event.preventDefault()}
          {draggable}
          ondragstart={(event) => dragOut(event, number)}
          {@attach watch}
        >
          {@render picture(thumbnailOf(number), inView.has(number))}
          <span class="label">
            <span class="name">{cues[number].name}</span>
            <span class="number">{number}</span>
          </span>
        </button>
        <button
          class="audition"
          aria-label="{isOn ? 'Release' : 'Audition'} scene {number}"
          title={isOn ? 'Release' : 'Audition'}
          disabled={!editable}
          onclick={() => onaudition(number)}>{isOn ? '■' : '▶'}</button
        >
      </li>
    {/each}
    {#if onnew}
      <li class="slot">
        <button class="pad new" disabled={!editable} onclick={onnew}>
          <span class="thumbnail add" style:aspect-ratio={pictureRatio} aria-hidden="true">+</span>
          <span class="label"><span class="name">New scene</span></span>
        </button>
      </li>
    {/if}
  </ol>
  <p class="hint">
    {#if editable}
      {#if draggable}Drag a scene onto the section to place it.{/if}
      Click a scene to edit it. Hold it, or press its ▶, to audition it on the rig.
    {:else}
      Show mode: scenes open read-only and don't audition.
    {/if}
  </p>
</section>

<style>
  .pads {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 10px;
    padding: 0;
    margin: 0 0 16px;
    list-style: none;
  }

  .slot {
    position: relative;
    display: flex;
  }

  .pad {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    padding: 6px;
    text-align: left;
    background: var(--panel);
    border-radius: var(--radius);
    user-select: none;
    -webkit-user-select: none;
    -webkit-touch-callout: none;
    transition:
      border-color 0.15s,
      box-shadow 0.15s,
      transform 0.1s;
  }

  .pad.new {
    background: none;
    border-style: dashed;
  }

  .pad.new:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--accent);
  }

  .pad.playing {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent), 0 0 16px rgb(255 190 80 / 0.35);
  }

  .pad.playing .number {
    color: var(--accent);
  }

  .pad.held {
    transform: scale(0.97);
  }

  .audition {
    position: absolute;
    top: 10px;
    right: 10px;
    width: 28px;
    height: 24px;
    padding: 0;
    font-size: 11px;
    line-height: 1;
    background: var(--bg-veil);
    opacity: 0.7;
    transition: opacity 0.15s;
  }

  .slot:hover .audition,
  .audition:focus-visible,
  .playing + .audition {
    opacity: 1;
  }

  .playing + .audition {
    color: var(--accent);
    border-color: var(--accent);
  }

  .thumbnail {
    display: block;
    width: 100%;
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

  .new .name {
    color: var(--muted);
  }

  .number {
    font: 12px var(--mono);
    color: var(--muted);
  }
</style>

<script module>
  // Once opened, the fine-tuning fields stay open across areas and scenes for the session.
  let isTuningOpen = $state(false);
</script>

<script>
  import { beatsIn } from './duration.js';
  import { curveLevels, curvePoints } from './envelope.svelte.js';
  import { FULL_STAGE, grooves, isSameRhythm, mutedBy, withAreaField } from './layers.js';
  import PatternGrid from './PatternGrid.svelte';
  import SwellEditor from './SwellEditor.svelte';
  import { Thumbnails, THUMBNAIL_DEBOUNCE_MS, frameAt, paint, thumbnailSide } from './thumbnails.svelte.js';

  /**
   * `layer`: the selected layer of the pixel cue, as in the song file. `palettes`: name → #rrggbb.
   * `groups`: the rig's groups. `caught`: what its area catches, fixture or grid name → pixel
   * indices, or null before the rig is known. `seed`: the cue's. `now`: how long the thumbnails
   * have played, in ms. `song`: the song whose bars they are rendered in. `editable`: false shows the recipe without changing it. `barBeats`: beats in the song's bar.
   * `saved`: this layer as saved, which an audition plays, or null. `audition`: the saved cue's
   * `beats` and `loopBeats` while it plays (see the stream's `cues`), and `onaudition` and
   * `onrestart`, as the swell and pattern editors take them.
   */
  let { layer = $bindable(), palettes = {}, groups = {}, caught, seed, now, song, editable, barBeats, saved, audition } = $props();

  // docs/api.md, Pixel cues.
  const SHAPES = ['fill', 'bar', 'sweep', 'circle', 'split', 'gradient', 'one-by-one', 'diamond', 'diagonal', 'stripes', 'checker', 'wedge'];
  const REPEATING_SHAPES = ['bar', 'circle', 'split', 'diamond', 'diagonal', 'wedge'];
  const EFFECTS = ['static', 'move', 'pulse', 'chase', 'sparkle', 'bounce', 'strobe', 'noise'];
  const DIRECTIONS = ['left-to-right', 'right-to-left', 'bottom-to-top', 'top-to-bottom'];
  const BLENDS = ['over', 'max', 'add', 'mask'];
  const DEFAULT_BLEND = 'max';
  const SPEED_UNITS = ['beats', 'bars', 's'];
  const AREA_FIELDS = [['x', 'Area left'], ['y', 'Area bottom'], ['width', 'Area width'], ['height', 'Area height']];

  /** A custom rhythm starts as an empty grid. */
  const NO_HITS = { pattern: { length: '1 bar', hits: [] } };

  /** A picker thumbnail's width and height in CSS pixels, as in the styles below. */
  const PICKER_PX = 40;
  const thumbnails = new Thumbnails();
  const speedText = $derived(layer.params?.speed ?? '1 bar');
  const speed = $derived(speedOf(speedText));
  const rhythm = $derived(layer.swell ? { swell: layer.swell } : layer.pattern ? { pattern: layer.pattern } : null);
  const grooveChoices = $derived(grooves(barBeats));
  const groove = $derived.by(() => {
    if (!rhythm) return 'steady';
    return Object.keys(grooveChoices).find((name) => isSameRhythm(grooveChoices[name], rhythm)) ?? 'custom';
  });
  const rhythmChoices = $derived([
    ['steady', {}],
    ...Object.entries(grooveChoices),
    ['custom', groove === 'custom' ? rhythm : NO_HITS],
  ]);
  const area = $derived(layer.area ?? FULL_STAGE);
  /** Groups with a fixture in the area, what it catches, and what it already mutes: each can be toggled. */
  const mutable = $derived.by(() => {
    const held = caught ?? new Map();
    const groupsHeld = Object.entries(groups).filter(([, members]) => members.some((member) => held.has(member)));
    return [...new Set([...groupsHeld.map(([group]) => group), ...held.keys(), ...(layer.mute ?? [])])];
  });

  $effect(() => {
    const own = $state.snapshot({ shape: layer.shape, effect: layer.effect, params: layer.params });
    const variants = [
      ...SHAPES.map((shape) => [`shape:${shape}`, { ...own, shape }]),
      ...EFFECTS.map((effect) => [`effect:${effect}`, { ...own, effect }]),
    ];
    const timer = setTimeout(() => thumbnails.load(variants, seed, song, thumbnailSide(PICKER_PX)), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  /** Gives the layer the rhythm `name`; a custom one already there stays to edit. */
  function pickRhythm(name, choice) {
    if (name === groove) return;
    delete layer.swell;
    delete layer.pattern;
    Object.assign(layer, structuredClone(choice));
  }

  function setParam(key, value) {
    layer.params ??= {};
    if (value === undefined) delete layer.params[key];
    else layer.params[key] = value;
  }

  /** Sets an optional key, leaving it out of the file at its default. */
  function setOptional(key, value, defaultValue) {
    if (value === defaultValue) delete layer[key];
    else layer[key] = value;
  }

  const hex = (color) => palettes[color] ?? (color?.startsWith('#') ? color : '#000000');

  /** `speed` as a slider value and unit; milliseconds show as seconds. */
  function speedOf(text) {
    const match = /^\s*([0-9.]+)\s*(ms|s|beats?|bars?)\s*$/.exec(text);
    if (!match) return { value: 1, unit: 'bars' };
    const value = Number(match[1]);
    if (match[2] === 'ms') return { value: value / 1000, unit: 's' };
    return { value, unit: match[2] === 's' ? 's' : match[2].replace(/s?$/, 's') };
  }

  function setSpeed({ value, unit }) {
    setParam('speed', unit === 's' ? `${value}s` : `${value} ${value === 1 ? unit.slice(0, -1) : unit}`);
  }

  function setArea(key, input) {
    if (Number.isFinite(input.valueAsNumber)) layer.area = withAreaField(area, key, input.valueAsNumber);
    // A value limited back to what the field held changes nothing for Svelte to redraw.
    input.value = area[key];
  }

  function toggleMute(name) {
    const mute = (layer.mute ?? []).filter((muted) => muted !== name);
    if (mute.length === (layer.mute?.length ?? 0)) mute.push(name);
    if (mute.length) layer.mute = mute;
    else delete layer.mute;
  }
</script>

{#snippet picker(label, kind, options)}
  <div class="picker" role="group" aria-label={label}>
    {#each options as option (option)}
      {@const thumbnail = thumbnails.byKey[`${kind}:${option}`]}
      {@const frame = frameAt(thumbnail, now)}
      <button
        class="choice"
        aria-label="{label}: {option}"
        aria-pressed={(layer[kind] ?? (kind === 'shape' ? 'bar' : 'static')) === option}
        onclick={() => (layer[kind] = option)}
      >
        <canvas width={thumbnail?.columns ?? 1} height={thumbnail?.rows ?? 1} {@attach (canvas) => paint(canvas, thumbnail, frame)}></canvas>
        <span>{option}</span>
      </button>
    {/each}
  </div>
{/snippet}

<!-- A rhythm's level over one loop or its swell, from the engine's own curves. -->
{#snippet rhythmPicture({ swell, pattern })}
  <svg class="rhythm" viewBox="0 -0.1 1 1.2" preserveAspectRatio="none" aria-hidden="true">
    {#if swell}
      {@const levels = curveLevels(swell.curve ?? 'ramp-up')}
      {#if levels}<polyline points={curvePoints(levels, { width: 1, height: 1 })} />{/if}
    {:else if pattern}
      {@const loopBeats = beatsIn(pattern.length, barBeats) || barBeats}
      {#each pattern.hits as hit, index (index)}
        {@const levels = curveLevels(hit.shape)}
        {#if levels}
          <polyline
            points={curvePoints(levels, {
              x: (hit.at - 1) / loopBeats,
              width: Math.min(beatsIn(hit.length, barBeats), loopBeats) / loopBeats,
              height: 1,
              scale: parseFloat(hit.level) / 100,
            })}
          />
        {/if}
      {/each}
    {:else}
      <polyline points="0,0 1,0" />
    {/if}
  </svg>
{/snippet}

{#snippet colour(label, key, isOptional)}
  <div class="colour">
    <span class="name caps">{label}</span>
    <div class="swatches">
      {#if isOptional}
        <button class="swatch none" title="none" aria-label="{label}: none" aria-pressed={!layer.params?.[key]} onclick={() => setParam(key, undefined)}></button>
      {/if}
      {#each Object.entries(palettes) as [name, rgb] (name)}
        <button class="swatch" style:background={rgb} title={name} aria-label="{label}: {name}" aria-pressed={layer.params?.[key] === name} onclick={() => setParam(key, name)}></button>
      {/each}
    </div>
    <input type="color" aria-label={label} value={hex(layer.params?.[key])} oninput={(event) => setParam(key, event.currentTarget.value)} />
  </div>
{/snippet}

<fieldset class="inspector" aria-label="Selected area" disabled={!editable}>
  {#if thumbnails.error}<p class="error" role="alert">Thumbnails: {thumbnails.error}</p>{/if}
  {@render colour('Colour', 'color', false)}
  {@render colour('Background', 'background', true)}
  <span class="name caps">Shape</span>
  {@render picker('Shape', 'shape', SHAPES)}
  <span class="name caps">Motion</span>
  {@render picker('Motion', 'effect', EFFECTS)}
  <span class="name caps">Rhythm</span>
  <div class="picker" role="group" aria-label="Rhythm">
    {#each rhythmChoices as [name, choice] (name)}
      <button class="choice" aria-label="Rhythm: {name}" aria-pressed={groove === name} onclick={() => pickRhythm(name, choice)}>
        {@render rhythmPicture(choice)}
        <span>{name}</span>
      </button>
    {/each}
  </div>
  <!-- A fresh editor per rhythm, so no hit stays selected from the one before. -->
  {#key layer.swell ?? layer.pattern}
    {#if layer.swell}
      <SwellEditor bind:cue={layer.swell} saved={saved?.swell ?? null} {barBeats} beats={audition.beats} onaudition={audition.onaudition} onrestart={audition.onrestart} />
    {:else if layer.pattern}
      <PatternGrid bind:cue={layer.pattern} saved={saved?.pattern ?? null} {barBeats} beats={audition.loopBeats} onaudition={audition.onaudition} />
    {/if}
  {/key}
  <div class="fields">
    <!-- First, so its full row never leaves a hole in the grid. Negated so that right, and up, is a shorter period: a faster pattern. -->
    <label class="speed">
      Speed
      <input type="range" aria-label="Speed" aria-valuetext={speedText} min="-16" max="-0.25" step="0.25" value={-speed.value} oninput={(event) => setSpeed({ ...speed, value: -Number(event.currentTarget.value) })} />
      <select aria-label="Speed unit" value={speed.unit} onchange={(event) => setSpeed({ ...speed, unit: event.currentTarget.value })}>
        {#each SPEED_UNITS as unit (unit)}<option>{unit}</option>{/each}
      </select>
      <output>{speedText}</output>
    </label>
    <label>
      Direction
      <select aria-label="Direction" value={layer.params?.direction ?? DIRECTIONS[0]} onchange={(event) => setParam('direction', event.currentTarget.value)}>
        {#each DIRECTIONS as direction (direction)}<option>{direction}</option>{/each}
      </select>
    </label>
    <label>
      Width
      <input type="range" aria-label="Width" min="0.05" max="1" step="0.05" value={layer.params?.width ?? 1} oninput={(event) => setParam('width', Number(event.currentTarget.value))} />
      <output>{layer.params?.width ?? 1}</output>
    </label>
    {#if REPEATING_SHAPES.includes(layer.shape ?? 'bar')}
      <label>
        Repeat
        <input type="range" aria-label="Repeat" min="1" max="8" step="1" value={layer.params?.repeat ?? 1} oninput={(event) => setParam('repeat', Number(event.currentTarget.value) === 1 ? undefined : Number(event.currentTarget.value))} />
        <output>{layer.params?.repeat ?? 1}</output>
      </label>
    {/if}
    <label>
      Level
      <input type="range" aria-label="Level" min="0" max="100" step="5" value={parseFloat(layer.level ?? '100')} oninput={(event) => setOptional('level', `${event.currentTarget.value}%`, '100%')} />
      <output>{layer.level ?? '100%'}</output>
    </label>
    <label>
      Blend
      <select aria-label="Blend" value={layer.blend ?? DEFAULT_BLEND} onchange={(event) => setOptional('blend', event.currentTarget.value, DEFAULT_BLEND)}>
        {#each BLENDS as blend (blend)}<option>{blend}</option>{/each}
      </select>
    </label>
  </div>
  <div class="mute" role="group" aria-label="Mute">
    <span class="name caps">Mute</span>
    {#each mutable as name (name)}
      {@const muted = mutedBy(layer, groups, name)}
      <button
        aria-pressed={!!muted}
        aria-label="Mute {name}"
        title={muted && muted !== name ? `Muted with ${muted}` : muted ? 'Muted: click to light it again' : 'Click to leave this one alone'}
        disabled={!!muted && muted !== name}
        onclick={() => toggleMute(name)}>{name}</button
      >
    {:else}
      <span class="hint">nothing caught to mute</span>
    {/each}
  </div>
  <details class="more" bind:open={isTuningOpen}>
    <summary>Position and phase</summary>
    <div class="fields area">
      {#each AREA_FIELDS as [key, label] (key)}
        <label>{label} <input type="number" min="0" max="1" step="0.025" value={area[key]} onchange={(event) => setArea(key, event.currentTarget)} /></label>
      {/each}
      <label>
        Phase
        <input type="range" aria-label="Phase" min="0" max="1" step="0.05" value={layer.params?.phase ?? 0} oninput={(event) => setParam('phase', Number(event.currentTarget.value) || undefined)} />
        <output>{layer.params?.phase ?? 0}</output>
      </label>
    </div>
  </details>
</fieldset>

<style>
  .inspector {
    min-width: 0;
    padding: 12px 0 0;
    margin: 0;
    border: 0;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .picker + .name,
  .mute,
  .more {
    margin-top: 6px;
  }

  .colour,
  .mute {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
  }

  .colour .name,
  .mute .name {
    width: 9.5em;
  }

  .swatches,
  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .swatch {
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
  }

  .swatch.none {
    background: repeating-linear-gradient(45deg, #111 0 4px, #333 4px 8px);
  }

  .swatch[aria-pressed='true'],
  .choice[aria-pressed='true'] {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .choice[aria-pressed='true'] {
    color: var(--accent);
  }

  .mute button {
    padding: 2px 10px;
    font-size: 13px;
  }

  input[type='color'] {
    width: 40px;
    height: 26px;
    padding: 0;
    border: 0;
    background: none;
  }

  .choice {
    display: flex;
    flex-direction: column;
    gap: 3px;
    align-items: center;
    padding: 4px;
    font-size: 11px;
  }

  canvas,
  .rhythm {
    width: 40px;
    height: 40px;
    background: #000;
  }

  .rhythm polyline {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2px;
    vector-effect: non-scaling-stroke;
  }

  .fields {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(14em, 1fr));
    gap: 6px 14px;
  }

  .fields label {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 14px;
  }

  .fields input[type='range'] {
    flex: 1;
    min-width: 0;
  }

  .speed {
    grid-column: 1 / -1;
  }

  /* Wide enough for the longest value the slider gives, "15.75 beats": a reading that changed the
     slider's width under the pointer would move the value with it, and the drag would flicker. */
  .speed output {
    flex: none;
    min-width: 11ch;
  }

  .area {
    grid-template-columns: repeat(auto-fill, minmax(11em, 1fr));
    white-space: nowrap;
  }

  input[type='number'] {
    width: 5em;
  }

  output {
    min-width: 3.5em;
    font: 12px var(--mono);
  }

  .mute button[aria-pressed='true'] {
    color: #ff7b7b;
    text-decoration: line-through;
  }
</style>

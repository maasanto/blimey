<script>
  import { FULL_STAGE, mutedBy, withAreaField } from './layers.js';
  import { Thumbnails, THUMBNAIL_DEBOUNCE_MS, paint } from './thumbnails.svelte.js';

  /**
   * `layer`: the selected layer of the pixel cue, as in the song file. `palettes`: name → #rrggbb.
   * `groups`: the rig's groups. `caught`: what its area catches, fixture or grid name → pixel
   * indices, or null before the rig is known. `seed`: the cue's. `tick`: the thumbnails' frame.
   * `editable`: false shows the recipe without changing it.
   */
  let { layer = $bindable(), palettes = {}, groups = {}, caught, seed, tick, editable } = $props();

  // docs/api.md, Pixel cues.
  const SHAPES = ['fill', 'bar', 'sweep', 'circle', 'split', 'gradient', 'one-by-one'];
  const EFFECTS = ['static', 'move', 'pulse', 'chase', 'sparkle'];
  const DIRECTIONS = ['left-to-right', 'right-to-left', 'bottom-to-top', 'top-to-bottom'];
  const BLENDS = ['over', 'max', 'add', 'mask'];
  const DEFAULT_BLEND = 'max';
  const SPEED_UNITS = ['beats', 'bars', 's'];
  const AREA_FIELDS = [['x', 'Area left'], ['y', 'Area bottom'], ['width', 'Area width'], ['height', 'Area height']];

  const thumbnails = new Thumbnails();
  const speed = $derived(speedOf(layer));
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
    const timer = setTimeout(() => thumbnails.load(variants, seed), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

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
  function speedOf({ params }) {
    const match = /^\s*([0-9.]+)\s*(ms|s|beats?|bars?)\s*$/.exec(params?.speed ?? '1 bar');
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
      <button
        class="choice"
        aria-label="{label}: {option}"
        aria-pressed={(layer[kind] ?? (kind === 'shape' ? 'bar' : 'static')) === option}
        onclick={() => (layer[kind] = option)}
      >
        <canvas width="12" height="12" {@attach (canvas) => paint(canvas, thumbnails.byKey[`${kind}:${option}`], tick)}></canvas>
        <span>{option}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet colour(label, key, isOptional)}
  <div class="colour">
    <span class="name">{label}</span>
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
  <span class="name">Shape</span>
  {@render picker('Shape', 'shape', SHAPES)}
  <span class="name">Motion</span>
  {@render picker('Motion', 'effect', EFFECTS)}
  <div class="fields">
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
    <label>
      Speed
      <input type="range" aria-label="Speed" min="0.25" max="16" step="0.25" value={speed.value} oninput={(event) => setSpeed({ ...speed, value: Number(event.currentTarget.value) })} />
      <select aria-label="Speed unit" value={speed.unit} onchange={(event) => setSpeed({ ...speed, unit: event.currentTarget.value })}>
        {#each SPEED_UNITS as unit (unit)}<option>{unit}</option>{/each}
      </select>
      <output>{layer.params?.speed ?? '1 bar'}</output>
    </label>
    <label>
      Phase
      <input type="range" aria-label="Phase" min="0" max="1" step="0.05" value={layer.params?.phase ?? 0} oninput={(event) => setParam('phase', Number(event.currentTarget.value) || undefined)} />
      <output>{layer.params?.phase ?? 0}</output>
    </label>
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
  <div class="fields area">
    {#each AREA_FIELDS as [key, label] (key)}
      <label>{label} <input type="number" min="0" max="1" step="0.025" value={area[key]} onchange={(event) => setArea(key, event.currentTarget)} /></label>
    {/each}
  </div>
  <div class="mute" role="group" aria-label="Mute">
    <span class="name">Mute</span>
    {#each mutable as name (name)}
      {@const muted = mutedBy(layer, groups, name)}
      <button
        aria-pressed={!!muted}
        aria-label="Mute {name}"
        title={muted && muted !== name ? `Muted with ${muted}` : undefined}
        disabled={!!muted && muted !== name}
        onclick={() => toggleMute(name)}>{name}</button
      >
    {:else}
      <span class="hint">nothing caught to mute</span>
    {/each}
  </div>
</fieldset>

<style>
  .inspector {
    min-width: 0;
    padding: 0;
    margin: 0;
    border: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .name {
    color: var(--muted);
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
    width: 6em;
  }

  .swatches,
  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .swatch {
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 50%;
  }

  .swatch.none {
    background: #000;
  }

  .swatch[aria-pressed='true'],
  .choice[aria-pressed='true'] {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
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

  canvas {
    width: 40px;
    height: 40px;
    image-rendering: pixelated;
    background: #000;
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
  }

  .fields input[type='range'] {
    flex: 1;
    min-width: 0;
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

<script module>
  // Defaults from C9's preview sheet (engine/examples/pixel_sheet.rs): amber, a quarter of the
  // canvas wide, seed 7. Speed is the engine's default, one bar.
  const DEFAULT_COLOUR = '#ffa01e';
  const DEFAULT_WIDTH = 0.25;
  const DEFAULT_SEED = 7;

  /** A new pixel cue, keeping the name and times already in the cue form. */
  export function newPixelCue({ name = '', fade_in, release } = {}) {
    return {
      name,
      type: 'pixel',
      ...(fade_in ? { fade_in } : {}),
      ...(release ? { release } : {}),
      shape: 'bar',
      effect: 'static',
      params: { color: DEFAULT_COLOUR, width: DEFAULT_WIDTH },
      seed: DEFAULT_SEED,
    };
  }
</script>

<script>
  import { request } from './engine.svelte.js';

  /**
   * `cue`: the pixel cue being edited, as in the song file. `palettes`: name → #rrggbb.
   * `cues`: the song's cues as `{ note, cue }`, to link a swell or pattern to this scene.
   * `savedName`: the scene's name in the song file, which links go by; null until it is saved.
   * `onlink(note)`: makes that cue drive this scene's intensity.
   */
  let { cue = $bindable(), palettes = {}, cues = [], savedName = null, paused = false, onlink } = $props();

  // docs/api.md, Pixel cues.
  const SHAPES = ['bar', 'sweep', 'circle', 'split', 'gradient', 'one-by-one'];
  const EFFECTS = ['static', 'move', 'pulse', 'chase', 'sparkle'];
  const DIRECTIONS = ['left-to-right', 'right-to-left', 'bottom-to-top', 'top-to-bottom'];
  const BLENDS = ['max', 'add', 'mask'];
  const SPEED_UNITS = ['beats', 'bars', 's'];
  /** Thumbnails are 8 frames over one cycle: about a second a loop. */
  const THUMBNAIL_FRAME_MS = 125;
  /** Waits for a slider to settle before rendering 11 thumbnails a layer. */
  const THUMBNAIL_DEBOUNCE_MS = 150;

  let tick = $state(0);
  /** `"<layer>:<shape or effect>"` → `POST /api/pixel-thumbnail` result, replaced whole. */
  let thumbnails = $state.raw({});
  /** Why thumbnails are missing, from the engine. */
  let thumbnailError = $state('');
  let linking = $state('');

  const layers = $derived([
    { key: 'layer', label: 'Layer 1', layer: cue },
    ...(cue.layer_2 ? [{ key: 'layer_2', label: 'Layer 2', layer: cue.layer_2 }] : []),
  ]);
  const linkable = $derived(cues.filter(({ cue: other }) => ['swell', 'pattern'].includes(other.type)));
  const isLinked = (other) => savedName !== null && other.target?.[savedName] === 'intensity';

  $effect(() => {
    const timer = setInterval(() => (tick += 1), THUMBNAIL_FRAME_MS);
    return () => clearInterval(timer);
  });

  // Rendered by the engine on a sample grid, so the pickers show what the lights will do.
  let generation = 0;
  $effect(() => {
    const wanted = layers.map(({ key, layer }) => ({ key, layer: $state.snapshot({ shape: layer.shape, effect: layer.effect, params: layer.params }) }));
    const seed = cue.seed;
    const timer = setTimeout(() => loadThumbnails(wanted, seed), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  function loadThumbnails(wanted, seed) {
    generation += 1;
    const current = generation;
    thumbnailError = '';
    for (const { key, layer } of wanted) {
      const variants = [
        ...SHAPES.map((shape) => [`${key}:${shape}`, { ...layer, shape }]),
        ...EFFECTS.map((effect) => [`${key}:${effect}`, { ...layer, effect }]),
      ];
      for (const [id, variant] of variants) {
        request('POST', '/api/pixel-thumbnail', { name: 'thumbnail', type: 'pixel', seed, ...variant })
          .then((thumbnail) => {
            if (current === generation) thumbnails = { ...thumbnails, [id]: thumbnail };
          })
          .catch((failure) => {
            if (current !== generation) return;
            thumbnails = { ...thumbnails, [id]: null };
            thumbnailError = failure.message;
          });
      }
    }
  }

  function paint(canvas, thumbnail) {
    const context = canvas.getContext('2d');
    if (!thumbnail) return context.clearRect(0, 0, canvas.width, canvas.height);
    const frame = thumbnail.frames[tick % thumbnail.frames.length];
    frame.forEach((colour, index) => {
      context.fillStyle = colour;
      context.fillRect(index % thumbnail.columns, Math.floor(index / thumbnail.columns), 1, 1);
    });
  }

  function setParam(layer, key, value) {
    layer.params ??= {};
    if (value === undefined) delete layer.params[key];
    else layer.params[key] = value;
  }

  const hex = (color) => palettes[color] ?? (color?.startsWith('#') ? color : '#000000');

  /** `speed` as a slider value and unit; milliseconds show as seconds. */
  function speedOf(layer) {
    const match = /^\s*([0-9.]+)\s*(ms|s|beats?|bars?)\s*$/.exec(layer.params?.speed ?? '1 bar');
    if (!match) return { value: 1, unit: 'bars' };
    const value = Number(match[1]);
    if (match[2] === 'ms') return { value: value / 1000, unit: 's' };
    return { value, unit: match[2] === 's' ? 's' : match[2].replace(/s?$/, 's') };
  }

  function setSpeed(layer, { value, unit }) {
    const text = unit === 's' ? `${value}s` : `${value} ${value === 1 ? unit.slice(0, -1) : unit}`;
    setParam(layer, 'speed', text);
  }

  function toggleLayer2(on) {
    if (on) cue.layer_2 = { shape: 'circle', effect: 'static', params: { color: DEFAULT_COLOUR, width: DEFAULT_WIDTH }, blend: 'max' };
    else delete cue.layer_2;
  }
</script>

{#snippet picker(key, label, kind, options, layer)}
  <div class="picker" role="group" aria-label="{label} {kind}">
    {#each options as option (option)}
      <button
        class="choice"
        aria-label="{label} {kind}: {option}"
        aria-pressed={(layer[kind] ?? options[0]) === option}
        onclick={() => (layer[kind] = option)}
      >
        <canvas width="12" height="12" {@attach (canvas) => paint(canvas, thumbnails[`${key}:${option}`])}></canvas>
        <span>{option}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet colour(label, layer, key, optional)}
  <div class="colour">
    <span class="name">{label}</span>
    <div class="swatches">
      {#if optional}
        <button class="swatch none" title="black" aria-label="{label}: black" aria-pressed={!layer.params?.[key]} onclick={() => setParam(layer, key, undefined)}></button>
      {/if}
      {#each Object.entries(palettes) as [name, rgb] (name)}
        <button class="swatch" style:background={rgb} title={name} aria-label="{label}: {name}" aria-pressed={layer.params?.[key] === name} onclick={() => setParam(layer, key, name)}></button>
      {/each}
    </div>
    <input type="color" aria-label={label} value={hex(layer.params?.[key])} oninput={(event) => setParam(layer, key, event.currentTarget.value)} />
  </div>
{/snippet}

<fieldset class="pixel">
  <legend>Pixel scene</legend>
  {#if thumbnailError}<p class="error" role="alert">Thumbnails: {thumbnailError}</p>{/if}
  {#each layers as { key, label, layer } (key)}
    {@const speed = speedOf(layer)}
    <section class="layer" data-layer={key}>
      <h3>{label}</h3>
      {@render picker(key, label, 'shape', SHAPES, layer)}
      {@render picker(key, label, 'effect', EFFECTS, layer)}
      {@render colour(`${label} colour`, layer, 'color', false)}
      {@render colour(`${label} background`, layer, 'background', true)}
      <div class="sliders">
        <label>
          Width
          <input type="range" min="0.05" max="1" step="0.05" aria-label="{label} width" value={layer.params?.width ?? 1} oninput={(event) => setParam(layer, 'width', Number(event.currentTarget.value))} />
          <output>{layer.params?.width ?? 1}</output>
        </label>
        <label>
          Speed
          <input type="range" min="0.25" max="16" step="0.25" aria-label="{label} speed" value={speed.value} oninput={(event) => setSpeed(layer, { ...speed, value: Number(event.currentTarget.value) })} />
          <select aria-label="{label} speed unit" value={speed.unit} onchange={(event) => setSpeed(layer, { ...speed, unit: event.currentTarget.value })}>
            {#each SPEED_UNITS as unit (unit)}<option>{unit}</option>{/each}
          </select>
          <output>{layer.params?.speed ?? '1 bar'}</output>
        </label>
        <label>
          Phase
          <input type="range" min="0" max="1" step="0.05" aria-label="{label} phase" value={layer.params?.phase ?? 0} oninput={(event) => setParam(layer, 'phase', Number(event.currentTarget.value))} />
          <output>{layer.params?.phase ?? 0}</output>
        </label>
        <label>
          Direction
          <select aria-label="{label} direction" value={layer.params?.direction ?? DIRECTIONS[0]} onchange={(event) => setParam(layer, 'direction', event.currentTarget.value)}>
            {#each DIRECTIONS as direction (direction)}<option>{direction}</option>{/each}
          </select>
        </label>
        {#if key === 'layer_2'}
          <label>
            Blend
            <select aria-label="Blend" value={layer.blend ?? 'max'} onchange={(event) => (layer.blend = event.currentTarget.value)}>
              {#each BLENDS as blend (blend)}<option>{blend}</option>{/each}
            </select>
          </label>
        {/if}
      </div>
    </section>
  {/each}

  <div class="row">
    <label><input type="checkbox" checked={!!cue.layer_2} onchange={(event) => toggleLayer2(event.currentTarget.checked)} /> Layer 2</label>
    <label>
      RGBW
      <select aria-label="RGBW" value={cue.rgbw ?? 'saturated'} onchange={(event) => (cue.rgbw = event.currentTarget.value)}>
        <option>saturated</option>
        <option>white-rich</option>
      </select>
    </label>
    <label>Seed <input type="number" min="0" step="1" value={cue.seed ?? 0} oninput={(event) => (cue.seed = Number(event.currentTarget.value))} /></label>
  </div>
  {#if paused}
    <p class="hint">Off the output while a cue is auditioned. Change the scene to show it again.</p>
  {:else}
    <p class="hint">Shows on the front view and the lights as you edit, at the song's tempo.</p>
  {/if}

  <div class="link">
    <span class="name">Intensity</span>
    {#each linkable.filter(({ cue: other }) => isLinked(other)) as { note, cue: other } (note)}
      <span class="linked">driven by cue {note}, {other.name} ({other.type})</span>
    {:else}
      <span class="hint">velocity only</span>
    {/each}
    <select aria-label="Swell or pattern to link" bind:value={linking}>
      <option value="">Swell or pattern…</option>
      {#each linkable.filter(({ cue: other }) => !isLinked(other)) as { note, cue: other } (note)}
        <option value={String(note)}>{note}: {other.name} ({other.type})</option>
      {/each}
    </select>
    <button disabled={!linking || savedName === null} onclick={() => {
        onlink(Number(linking));
        linking = '';
      }}>Link to intensity</button>
  </div>
  <p class="hint">
    Linking saves that cue with this scene's saved name as its target, in place of the target it had.
    {#if savedName === null}Save this scene first.{:else if savedName !== cue.name}Links follow the saved name, {savedName}, until you save.{/if}
  </p>
</fieldset>

<style>
  .pixel {
    padding: 12px;
    margin: 0 0 16px;
    border: 1px solid var(--line);
    border-radius: 8px;
  }

  legend {
    padding: 0 6px;
    color: var(--muted);
  }

  h3 {
    margin: 4px 0 8px;
    font-size: 14px;
  }

  .layer {
    padding-bottom: 10px;
    margin-bottom: 10px;
    border-bottom: 1px solid var(--line);
  }

  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 8px;
  }

  .choice {
    display: flex;
    flex-direction: column;
    gap: 3px;
    align-items: center;
    padding: 4px;
    font-size: 11px;
  }

  .choice[aria-pressed='true'] {
    outline: 2px solid var(--accent);
  }

  canvas {
    width: 48px;
    height: 48px;
    image-rendering: pixelated;
    background: #000;
  }

  .colour,
  .link {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
    margin-bottom: 6px;
  }

  .name {
    width: 9em;
    color: var(--muted);
  }

  .swatches {
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

  .swatch[aria-pressed='true'] {
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

  .sliders {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(15em, 1fr));
    gap: 6px 14px;
  }

  .sliders label {
    display: flex;
    gap: 6px;
    align-items: center;
  }

  .sliders input[type='range'] {
    flex: 1;
    min-width: 0;
  }

  output {
    min-width: 3.5em;
    font: 12px var(--mono);
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
  }

  input[type='number'] {
    width: 5em;
  }

  .linked {
    color: var(--accent);
  }
</style>

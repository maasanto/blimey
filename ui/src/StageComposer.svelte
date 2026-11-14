<script>
  import { engine } from './engine.svelte.js';
  import AreaInspector from './AreaInspector.svelte';
  import FrontView, { HEIGHT, WIDTH, screenX, screenY } from './FrontView.svelte';
  import IntensityLink from './IntensityLink.svelte';
  import {
    FULL_STAGE,
    INGREDIENTS,
    MAX_LAYERS,
    catches,
    caughtPixels,
    describeLayer,
    describeLook,
    isSameArea,
    kindOf,
    moveArea,
    mutedBy,
    newLayer,
    resizeArea,
    stageEdges,
  } from './layers.js';
  import { Thumbnails, THUMBNAIL_DEBOUNCE_MS, THUMBNAIL_FRAME_MS, paint } from './thumbnails.svelte.js';

  /**
   * `cue`: the pixel cue being edited, in the `layers` form. `hidden`: per layer, whether it is
   * left off the preview while editing; it is saved all the same. `editable`: false in show mode.
   * `paused`: the preview is off the output for an audition. `cues`, `savedName` and `onlink`
   * link a swell or pattern to the cue's intensity. `song` and `barBeats`: the song the cue is
   * in and its bar. `savedLayers`: the cue's layers as saved; `audition`: as the inspector takes it.
   */
  let {
    cue = $bindable(),
    hidden = $bindable(),
    palettes = {},
    editable,
    paused,
    cues,
    savedName,
    onlink,
    song,
    barBeats,
    savedLayers,
    audition,
  } = $props();

  /** An ingredient dropped on the stage makes an area this size, in canvas units, around the drop. */
  const DROP_SIZE = 0.4;
  /** A press that moves this far, in screen pixels, carries an ingredient or drags an area; less is a click. */
  const DRAG_PX = 6;
  /** Side of a resize handle, in front view units. */
  const HANDLE = 26;
  const HANDLES = ['top-left', 'top', 'top-right', 'right', 'bottom-right', 'bottom', 'bottom-left', 'left'];

  let selected = $state(null);
  let frontView;
  /** A gesture under way: an ingredient carried to the stage, or an area moved or resized. */
  let gesture = $state(null);
  let isClickSuppressed = false;
  let tick = $state(0);
  const tray = new Thumbnails();

  const rig = $derived(engine.rig);
  const edges = $derived(rig ? stageEdges(rig) : { x: [0, 1], y: [0, 1] });
  const selectedLayer = $derived(selected === null ? null : cue.layers[selected]);
  const caught = $derived(selectedLayer && rig ? caughtPixels(rig, selectedLayer.area ?? FULL_STAGE) : null);
  const crossedOut = $derived(
    new Set(caught ? [...caught.keys()].filter((name) => mutedBy(selectedLayer, rig.groups, name)) : []),
  );
  const isFull = $derived(cue.layers.length >= MAX_LAYERS);
  const newColour = $derived(Object.keys(palettes)[0] ?? '#ffffff');

  $effect(() => {
    const timer = setInterval(() => (tick += 1), THUMBNAIL_FRAME_MS);
    return () => clearInterval(timer);
  });

  $effect(() => {
    const variants = INGREDIENTS.map((ingredient) => [ingredient, newLayer(ingredient, newColour, undefined, barBeats)]);
    const { seed } = cue;
    const timer = setTimeout(() => tray.load(variants, seed, song), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  function addLayer(ingredient, area) {
    if (isFull) return;
    cue.layers.push(newLayer(ingredient, newColour, area, barBeats));
    hidden.push(false);
    selected = cue.layers.length - 1;
  }

  function swapLayers(index, other) {
    for (const list of [cue.layers, hidden]) [list[index], list[other]] = [list[other], list[index]];
    if (selected === index) selected = other;
    else if (selected === other) selected = index;
  }

  function deleteLayer(index) {
    cue.layers.splice(index, 1);
    hidden.splice(index, 1);
    if (selected === index) selected = null;
    else if (selected > index) selected -= 1;
  }

  const isOnStage = ({ x, y }) => x >= 0 && x <= 1 && y >= 0 && y <= 1;
  const areaOf = (layer) => layer.area ?? FULL_STAGE;

  /** Starts a press of the pointer in `event`: a click until it moves `DRAG_PX`. */
  function press(event, fields) {
    gesture = { pointer: event.pointerId, x: event.clientX, y: event.clientY, isMoving: false, ...fields };
  }

  /** Starts moving or resizing the area of layer `index`, keeping its area as it was to restore. */
  function pressArea(event, index, fields) {
    const original = $state.snapshot(cue.layers[index].area);
    press(event, { index, from: frontView.canvasPoint(event), original, ...fields });
  }

  function carry(event, ingredient) {
    if (!editable || isFull || event.button !== 0) return;
    press(event, { kind: 'carry', ingredient, area: null });
  }

  /**
   * A click on the stage selects the topmost area under it, and a second one the area beneath,
   * in show mode too, to read its recipe.
   */
  function pressStage(event) {
    if (event.button !== 0) return;
    const point = frontView.canvasPoint(event);
    const under = cue.layers.map((_, index) => index).filter((index) => catches(areaOf(cue.layers[index]), point)).reverse();
    const isSelectedUnder = under.includes(selected);
    const index = isSelectedUnder ? selected : under[0];
    selected = index ?? null;
    if (index === undefined) return;
    const deeper = isSelectedUnder ? under[(under.indexOf(selected) + 1) % under.length] : null;
    pressArea(event, index, { kind: 'move', deeper });
  }

  function grabHandle(event, handle) {
    if (!editable || event.button !== 0) return;
    pressArea(event, selected, { kind: 'resize', sides: handle.split('-') });
  }

  /** Puts the area of the layer under the gesture back as it was before the press. */
  function restoreArea({ index, original }) {
    if (original) cue.layers[index].area = original;
    else delete cue.layers[index].area;
  }

  function follow(event) {
    if (gesture?.pointer !== event.pointerId) return;
    gesture.isMoving ||= Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) >= DRAG_PX;
    if (!gesture.isMoving) return;
    const point = frontView.canvasPoint(event);
    if (gesture.kind === 'carry') {
      const corner = { x: 0, y: 0, width: DROP_SIZE, height: DROP_SIZE };
      gesture.area = isOnStage(point) ? moveArea(corner, point.x - DROP_SIZE / 2, point.y - DROP_SIZE / 2, edges) : null;
      return;
    }
    if (!editable) return;
    const [dx, dy] = [point.x - gesture.from.x, point.y - gesture.from.y];
    const start = gesture.original ?? FULL_STAGE;
    const next = gesture.kind === 'move' ? moveArea(start, dx, dy, edges) : resizeArea(start, gesture.sides, dx, dy, edges);
    const layer = cue.layers[gesture.index];
    if (isSameArea(next, start)) restoreArea(gesture);
    else if (!isSameArea(next, areaOf(layer))) layer.area = next;
  }

  /** A cancelled press drops a carried ingredient and puts a moved or resized area back. */
  function cancel(event) {
    if (gesture?.pointer !== event.pointerId) return;
    if (gesture.kind !== 'carry') restoreArea(gesture);
    gesture = null;
  }

  function letGo(event) {
    if (gesture?.pointer !== event.pointerId) return;
    const { kind, ingredient, isMoving, area, deeper } = gesture;
    gesture = null;
    if (kind === 'carry' && isMoving) {
      // The click that ends a carry isn't one.
      isClickSuppressed = true;
      setTimeout(() => (isClickSuppressed = false));
      if (area) addLayer(ingredient, area);
    }
    if (kind === 'move' && !isMoving && deeper !== null) selected = deeper;
  }

  function handleAt(area, handle) {
    const sides = handle.split('-');
    const x = sides.includes('left') ? area.x : sides.includes('right') ? area.x + area.width : area.x + area.width / 2;
    const y = sides.includes('bottom') ? area.y : sides.includes('top') ? area.y + area.height : area.y + area.height / 2;
    return { x: screenX(x) - HANDLE / 2, y: screenY(y) - HANDLE / 2 };
  }
</script>

<svelte:window onpointermove={follow} onpointerup={letGo} onpointercancel={cancel} />

{#snippet box(area, className, attributes = {})}
  <rect
    class={className}
    x={screenX(area.x)}
    y={screenY(area.y + area.height)}
    width={area.width * WIDTH}
    height={area.height * HEIGHT}
    {...attributes}
  />
{/snippet}

{#snippet areas()}
  <rect class="hit" width={WIDTH} height={HEIGHT} role="presentation" onpointerdown={pressStage} />
  {#each cue.layers as layer, index (layer)}
    {@const area = areaOf(layer)}
    {@const isSameCorner = (other) => areaOf(other).x === area.x && areaOf(other).y + areaOf(other).height === area.y + area.height}
    {@const labelsAbove = cue.layers.slice(0, index).filter(isSameCorner).length}
    <g class="area" class:selected={index === selected} class:hidden={hidden[index]} data-area={index}>
      {@render box(area, 'outline')}
      <text x={screenX(area.x) + 14} y={screenY(area.y + area.height) + 38 + 36 * labelsAbove}>
        {index + 1}. {kindOf(layer)}{hidden[index] ? ' (hidden)' : ''}
      </text>
    </g>
  {/each}
  {#if selectedLayer && editable}
    {#each HANDLES as handle (handle)}
      {@const at = handleAt(areaOf(selectedLayer), handle)}
      <rect
        class="handle"
        data-handle={handle}
        x={at.x}
        y={at.y}
        width={HANDLE}
        height={HANDLE}
        role="presentation"
        onpointerdown={(event) => grabHandle(event, handle)}
      />
    {/each}
  {/if}
  {#if gesture?.area}{@render box(gesture.area, 'landing')}{/if}
{/snippet}

<fieldset class="composer">
  <legend>Look on the stage</legend>
  <div class="tray" role="group" aria-label="Ingredients">
    {#each INGREDIENTS as ingredient (ingredient)}
      <button
        class="ingredient"
        disabled={!editable || isFull}
        onpointerdown={(event) => carry(event, ingredient)}
        onclick={() => !isClickSuppressed && addLayer(ingredient)}
      >
        <canvas width="12" height="12" {@attach (canvas) => paint(canvas, tray.byKey[ingredient], tick)}></canvas>
        <span>{ingredient}</span>
      </button>
    {/each}
  </div>
  {#if tray.error}<p class="error" role="alert">Thumbnails: {tray.error}</p>{/if}
  <p class="hint">
    {#if isFull}
      A look holds {MAX_LAYERS} areas at most: delete one to add another.
    {:else}
      Drag an ingredient onto the stage, or click it to cover the whole stage. Drag an area to move it and its handles to resize it; click again to reach the area beneath.
    {/if}
  </p>

  <FrontView bind:this={frontView} overlay={areas} {caught} {crossedOut} />

  <p class="sentence" aria-live="polite">{describeLook(cue.layers)}</p>
  {#if caught && !caught.size}
    <p class="hint" role="status">This area catches no pixels: move it over some fixtures.</p>
  {/if}
  {#if paused}
    <p class="hint">Off the output while a cue is auditioned. Change the look to show it again.</p>
  {/if}

  <div class="panels">
    <ol class="layers" aria-label="Layers, top first">
      {#each cue.layers.map((layer, index) => ({ layer, index })).reverse() as { layer, index } (layer)}
        {@const description = describeLayer(layer)}
        <li class:hidden={hidden[index]}>
          <button class="select" aria-pressed={index === selected} onclick={() => (selected = index)}>
            <i style:background={palettes[layer.params?.color] ?? layer.params?.color ?? '#ffffff'}></i>
            {description}
          </button>
          <button aria-label="Bring forward: {description}" disabled={!editable || index === cue.layers.length - 1} onclick={() => swapLayers(index, index + 1)}>↑</button>
          <button aria-label="Send back: {description}" disabled={!editable || index === 0} onclick={() => swapLayers(index, index - 1)}>↓</button>
          <button aria-label="Hide while editing: {description}" disabled={!editable} aria-pressed={!!hidden[index]} onclick={() => (hidden[index] = !hidden[index])}>
            {hidden[index] ? 'Show' : 'Hide'}
          </button>
          <button aria-label="Delete: {description}" disabled={!editable} onclick={() => deleteLayer(index)}>×</button>
        </li>
      {/each}
    </ol>

    {#if selectedLayer}
      <!-- A fresh inspector per area, so no picker shows the previous area's thumbnails. -->
      {#key selected}
        <AreaInspector
          bind:layer={cue.layers[selected]}
          {palettes}
          groups={rig?.groups ?? {}}
          {caught}
          seed={cue.seed}
          {tick}
          {editable}
          {song}
          {barBeats}
          saved={savedLayers[selected] ?? null}
          {audition}
        />
      {/key}
    {:else}
      <p class="hint">Click an area on the stage or in the list to open its recipe.</p>
    {/if}
  </div>

  <fieldset class="whole-cue" disabled={!editable}>
    <div class="row">
      <label>
        RGBW
        <select value={cue.rgbw ?? 'saturated'} onchange={(event) => (cue.rgbw = event.currentTarget.value)}>
          <option>saturated</option>
          <option>white-rich</option>
        </select>
      </label>
      <label>Seed <input type="number" min="0" step="1" value={cue.seed ?? 0} oninput={(event) => (cue.seed = Number(event.currentTarget.value))} /></label>
    </div>
    <IntensityLink {cues} {savedName} name={cue.name} {onlink} />
  </fieldset>
</fieldset>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    padding: 12px;
    margin: 0 0 16px;
    border: 1px solid var(--line);
    border-radius: 8px;
  }

  .whole-cue {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
    padding: 0;
    margin: 0;
    border: 0;
  }

  legend {
    padding: 0 6px;
    color: var(--muted);
  }

  p {
    margin: 0;
  }

  .tray {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .ingredient {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 10px 4px 4px;
    cursor: grab;
    touch-action: none;
  }

  canvas {
    width: 32px;
    height: 32px;
    image-rendering: pixelated;
    background: #000;
    border-radius: 3px;
  }

  .sentence {
    font-size: 16px;
  }

  .hit {
    fill: transparent;
  }

  .area {
    pointer-events: none;
  }

  .outline {
    fill: rgb(255 255 255 / 0.03);
    stroke: rgb(255 255 255 / 0.45);
    stroke-width: 3;
    stroke-dasharray: 14 10;
  }

  .area.selected .outline {
    stroke: var(--accent);
    stroke-dasharray: none;
  }

  .area.hidden .outline {
    stroke-opacity: 0.2;
  }

  .area text {
    fill: var(--text);
    font-size: 30px;
  }

  .area.selected text {
    fill: var(--accent);
  }

  .handle {
    fill: var(--accent);
    cursor: move;
  }

  .landing {
    fill: rgb(255 190 80 / 0.12);
    stroke: var(--accent);
    stroke-width: 3;
    stroke-dasharray: 8 8;
    pointer-events: none;
  }

  .panels {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .layers {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 0;
    margin: 0;
    list-style: none;
  }

  .layers li {
    display: flex;
    gap: 4px;
  }

  .layers li.hidden .select {
    opacity: 0.5;
  }

  .layers button {
    padding: 3px 8px;
  }

  .select {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .select[aria-pressed='true'] {
    outline: 2px solid var(--accent);
  }

  .select i {
    display: inline-block;
    width: 10px;
    height: 10px;
    margin-right: 4px;
    border-radius: 50%;
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
</style>

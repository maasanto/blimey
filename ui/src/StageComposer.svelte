<script module>
  // Once opened, the whole-look settings stay open across areas and scenes for the session.
  let isWholeLookOpen = $state(false);
</script>

<script>
  import { untrack } from 'svelte';
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
  import { Thumbnails, THUMBNAIL_DEBOUNCE_MS, frameAt, paint, thumbnailSide } from './thumbnails.svelte.js';

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
  /** How far an arrow key moves an area, in canvas units: the grid areas snap to. */
  const NUDGE = 0.025;
  const HANDLES = ['top-left', 'top', 'top-right', 'right', 'bottom-right', 'bottom', 'bottom-left', 'left'];

  let selected = $state(null);
  let frontView;
  /** The composer's element, which takes the focus on a press on the stage, so the keyboard then works the area. */
  let composer;
  /** A gesture under way: an ingredient carried to the stage, or an area moved or resized. */
  let gesture = $state(null);
  let isClickSuppressed = false;
  /** Milliseconds the thumbnails have played for: each runs over its own cycle, as fast as its speed. */
  let now = $state(0);
  /** The clock the area's pictures play on: the preview's on the lights, so they stay in step with it, else `now`. */
  const areaClock = $derived(engine.frame?.preview_ms ?? now);
  /** A tray thumbnail's width and height in CSS pixels, as in the styles below. */
  const TRAY_PX = 32;
  /** Areas are the editor's largest pictures: the engine's finest grid, `MAX_THUMBNAIL_SIDE`. */
  const FINEST_SIDE = 64;
  const tray = new Thumbnails();
  /** Each layer's shape, by index, to draw in its area. */
  const layerShapes = new Thumbnails();

  const rig = $derived(engine.rig);
  const edges = $derived(rig ? stageEdges(rig) : { x: [0, 1], y: [0, 1] });
  const selectedLayer = $derived(selected === null ? null : cue.layers[selected]);
  /**
   * Each layer as saved, by the layer being edited, so it follows it through reordering; a new
   * layer has none. Paired up when the cue opens or saves, when the two lists are in step.
   */
  const savedOf = $derived.by(() => {
    const saved = savedLayers;
    return untrack(() => new Map(cue.layers.map((layer, index) => [layer, saved[index]])));
  });
  const caught = $derived(selectedLayer && rig ? caughtPixels(rig, selectedLayer.area ?? FULL_STAGE) : null);
  const crossedOut = $derived(
    new Set(caught ? [...caught.keys()].filter((name) => mutedBy(selectedLayer, rig.groups, name)) : []),
  );
  const isFull = $derived(cue.layers.length >= MAX_LAYERS);
  const newColour = $derived(Object.keys(palettes)[0] ?? '#ffffff');

  $effect(() => {
    let request = requestAnimationFrame(function play(time) {
      now = time;
      request = requestAnimationFrame(play);
    });
    return () => cancelAnimationFrame(request);
  });

  $effect(() => {
    const variants = INGREDIENTS.map((ingredient) => [ingredient, newLayer(ingredient, newColour, undefined, barBeats)]);
    const { seed } = cue;
    const timer = setTimeout(() => tray.load(variants, seed, song, thumbnailSide(TRAY_PX)), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  // A layer runs in its area's own 0–1 space, so rendered alone on the whole sample grid it is
  // its area's picture. Left as `max`, a mask shows where it lets the layers below through.
  // A string, so moving an area renders nothing again.
  const shapeRecipes = $derived(JSON.stringify(cue.layers.map(({ area, blend, ...layer }) => layer)));

  $effect(() => {
    const variants = JSON.parse(shapeRecipes).map((layer, index) => [index, layer]);
    const { seed } = cue;
    const shapeSong = song;
    const timer = setTimeout(() => layerShapes.load(variants, seed, shapeSong, FINEST_SIDE), THUMBNAIL_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  });

  function addLayer(ingredient, area) {
    if (isFull) return;
    cue.layers.push(newLayer(ingredient, newColour, area, barBeats));
    hidden.push(false);
    selected = cue.layers.length - 1;
  }

  /** Puts a copy of layer `index` just above it and selects the copy. */
  function duplicateLayer(index) {
    if (isFull) return;
    cue.layers.splice(index + 1, 0, $state.snapshot(cue.layers[index]));
    hidden.splice(index + 1, 0, false);
    selected = index + 1;
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
    composer.focus({ preventScroll: true });
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

  /**
   * Arrows nudge the selected area a grid step (a tenth with Shift), Delete removes it, Escape
   * drops the selection: plain keys only, with the focus on the stage or nowhere in particular,
   * so typing in a field, a button's own keys and the browser's shortcuts are left alone.
   */
  function keyboard(event) {
    const isFocusElsewhere = event.target !== document.body && event.target !== composer;
    const isBrowserShortcut = event.altKey || event.ctrlKey || event.metaKey;
    if (!editable || selected === null || isFocusElsewhere || isBrowserShortcut || document.querySelector('dialog[open]')) return;
    const direction = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, 1], ArrowDown: [0, -1] }[event.key];
    if (direction) {
      const step = event.shiftKey ? 0.1 : NUDGE;
      const area = areaOf(selectedLayer);
      const moved = moveArea(area, direction[0] * step, direction[1] * step, edges);
      if (!isSameArea(moved, area)) selectedLayer.area = moved;
    } else if (event.key === 'Delete' || event.key === 'Backspace') {
      deleteLayer(selected);
    } else if (event.key === 'Escape') {
      selected = null;
    } else {
      return;
    }
    event.preventDefault();
  }
</script>

<svelte:window onpointermove={follow} onpointerup={letGo} onpointercancel={cancel} onkeydown={keyboard} />

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

{#snippet shapes()}
  {#each cue.layers as layer, index (layer)}
    {@const thumbnail = layerShapes.byKey[index]}
    {#if thumbnail && !hidden[index]}
      {@const area = areaOf(layer)}
      {@const frame = frameAt(thumbnail, areaClock)}
      <!-- One canvas the browser scales up smoothly, so the shape reads as a shape, not as cells. -->
      <foreignObject class="shape" data-shape={index} x={screenX(area.x)} y={screenY(area.y + area.height)} width={area.width * WIDTH} height={area.height * HEIGHT}>
        <canvas width={thumbnail.columns} height={thumbnail.rows} {@attach (canvas) => paint(canvas, thumbnail, frame)}></canvas>
      </foreignObject>
    {/if}
  {/each}
{/snippet}

{#snippet areas()}
  <rect class="hit" width={WIDTH} height={HEIGHT} role="presentation" onpointerdown={pressStage} />
  {#if !cue.layers.length && editable}
    <text class="empty" x={WIDTH / 2} y={HEIGHT * 0.08}>Drag an ingredient here, or click one to fill the stage</text>
  {/if}
  {#each cue.layers as layer, index (layer)}
    {@const area = areaOf(layer)}
    {@const isSameCorner = (other) => areaOf(other).x === area.x && areaOf(other).y + areaOf(other).height === area.y + area.height}
    {@const labelsAbove = cue.layers.slice(0, index).filter(isSameCorner).length}
    <g class="area" class:selected={index === selected} class:hidden={hidden[index]} data-area={index} role="presentation" onpointerdown={pressStage}>
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

<div class="composer" role="group" aria-label="Look on the stage" tabindex="-1" bind:this={composer}>
  <div class="column">
    <div class="tray" role="group" aria-label="Ingredients">
      {#each INGREDIENTS as ingredient (ingredient)}
        {@const frame = frameAt(tray.byKey[ingredient], now)}
        <button
          class="ingredient"
          disabled={!editable || isFull}
          onpointerdown={(event) => carry(event, ingredient)}
          onclick={() => !isClickSuppressed && addLayer(ingredient)}
        >
          <canvas width={tray.byKey[ingredient]?.columns ?? 1} height={tray.byKey[ingredient]?.rows ?? 1} {@attach (canvas) => paint(canvas, tray.byKey[ingredient], frame)}></canvas>
          <span>{ingredient}</span>
        </button>
      {/each}
      <span class="hint">
        {#if isFull}
          A look holds {MAX_LAYERS} areas at most: delete one to add another.
        {:else}
          Drag onto the stage, or click for the whole stage.
        {/if}
      </span>
    </div>
    {#if tray.error}<p class="error" role="alert">Thumbnails: {tray.error}</p>{/if}
    {#if layerShapes.error}<p class="error" role="alert">Area shapes: {layerShapes.error}</p>{/if}

    <FrontView bind:this={frontView} locked overlay={areas} underlay={shapes} {caught} {crossedOut} />

    <p class="sentence" class:hint={!cue.layers.length} aria-live="polite">{describeLook(cue.layers)}</p>
    {#if caught && !caught.size}
      <p class="hint" role="status">This area catches no pixels: move it over some fixtures.</p>
    {:else if selectedLayer && editable}
      <p class="hint">
        Drag the area to move it, its handles to resize it; click again for the area beneath.
        <kbd>←</kbd><kbd>↑</kbd><kbd>→</kbd><kbd>↓</kbd> nudge · <kbd>⌫</kbd> delete · <kbd>esc</kbd> deselect
      </p>
    {/if}
    {#if paused}
      <p class="hint">Off the output while a scene is auditioned. Change the look to show it again.</p>
    {/if}
  </div>

  <div class="column">
    {#if cue.layers.length}
      <ol class="layers" aria-label="Layers, top first">
        {#each cue.layers.map((layer, index) => ({ layer, index })).reverse() as { layer, index } (layer)}
          {@const description = describeLayer(layer)}
          <li class:hidden={hidden[index]}>
            <button class="select" aria-pressed={index === selected} onclick={() => (selected = index)}>
              <i style:background={palettes[layer.params?.color] ?? layer.params?.color ?? '#ffffff'}></i>
              {description}
            </button>
            <button class="quiet" aria-label="Bring forward: {description}" title="Bring forward" disabled={!editable || index === cue.layers.length - 1} onclick={() => swapLayers(index, index + 1)}>↑</button>
            <button class="quiet" aria-label="Send back: {description}" title="Send back" disabled={!editable || index === 0} onclick={() => swapLayers(index, index - 1)}>↓</button>
            <button
              class="quiet"
              aria-label="Hide while editing: {description}"
              title="Hidden areas leave the preview but are still saved"
              disabled={!editable}
              aria-pressed={!!hidden[index]}
              onclick={() => (hidden[index] = !hidden[index])}
            >
              {hidden[index] ? 'Show' : 'Hide'}
            </button>
            <button class="quiet" aria-label="Duplicate: {description}" title="Duplicate" disabled={!editable || isFull} onclick={() => duplicateLayer(index)}>⧉</button>
            <button class="quiet danger" aria-label="Delete: {description}" title="Delete" disabled={!editable} onclick={() => deleteLayer(index)}>×</button>
          </li>
        {/each}
      </ol>
    {/if}

    {#if selectedLayer}
      <!-- A fresh inspector per area, so no picker shows the previous area's thumbnails. -->
      {#key selected}
        <AreaInspector
          bind:layer={cue.layers[selected]}
          {palettes}
          groups={rig?.groups ?? {}}
          {caught}
          seed={cue.seed}
          now={areaClock}
          {song}
          {editable}
          {barBeats}
          saved={savedOf.get(selectedLayer) ?? null}
          {audition}
        />
      {/key}
    {:else if cue.layers.length}
      <p class="hint">Select an area on the stage or in the list to edit its recipe.</p>
    {:else if editable}
      <p class="hint">The look is empty. Add an ingredient to start.</p>
    {/if}

    <details class="more" bind:open={isWholeLookOpen}>
      <summary>Whole look: colour mixing, seed, intensity link</summary>
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
    </details>
  </div>
</div>

<style>
  /* The stage on the left, the layers and the recipe of the selected area beside it. */
  .composer {
    display: grid;
    grid-template-columns: minmax(0, 1.6fr) minmax(320px, 1fr);
    gap: 24px;
    align-items: start;
    min-width: 0;
    outline: none;
  }

  .column {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
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

  p {
    margin: 0;
  }

  .tray {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 8px;
    align-items: center;
  }

  .ingredient {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 10px 4px 4px;
    cursor: grab;
    touch-action: none;
  }

  .ingredient:active {
    cursor: grabbing;
  }

  canvas {
    width: 32px;
    height: 32px;
    background: #000;
    border-radius: 3px;
  }

  .sentence {
    font-size: 16px;
  }

  .sentence.hint {
    font-size: 13px;
  }

  .hit {
    fill: transparent;
  }

  /* Faint, and dark cells let the stage through, so the fixtures still read as the output. */
  .shape {
    opacity: 0.4;
    mix-blend-mode: screen;
    pointer-events: none;
  }

  .shape canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  .empty {
    fill: var(--dim);
    font-size: 34px;
    text-anchor: middle;
    pointer-events: none;
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
    fill: rgb(255 190 80 / 0.05);
    stroke: var(--accent);
    stroke-dasharray: none;
    pointer-events: visiblePainted;
    cursor: move;
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
    stroke: var(--bg);
    stroke-width: 3;
  }

  .handle[data-handle='top'],
  .handle[data-handle='bottom'] {
    cursor: ns-resize;
  }

  .handle[data-handle='left'],
  .handle[data-handle='right'] {
    cursor: ew-resize;
  }

  .handle[data-handle='top-left'],
  .handle[data-handle='bottom-right'] {
    cursor: nwse-resize;
  }

  .handle[data-handle='top-right'],
  .handle[data-handle='bottom-left'] {
    cursor: nesw-resize;
  }

  .landing {
    fill: rgb(255 190 80 / 0.12);
    stroke: var(--accent);
    stroke-width: 3;
    stroke-dasharray: 8 8;
    pointer-events: none;
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
    background: var(--panel);
  }

  .select[aria-pressed='true'] {
    color: var(--accent);
    border-color: var(--accent);
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

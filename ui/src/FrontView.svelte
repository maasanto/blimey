<script module>
  // The 0–1 canvas (docs/api.md) drawn as the audience sees the stage, at a 16:9 ratio.
  export const WIDTH = 1600;
  export const HEIGHT = 900;
  /** Screen position of a canvas point: y runs up from the floor, the screen's runs down. */
  export const screenX = (x) => x * WIDTH;
  export const screenY = (y) => (1 - y) * HEIGHT;
</script>

<script>
  import { engine, request } from './engine.svelte.js';
  import { clamp, tidy } from './layers.js';

  /**
   * While a look is composed on the stage: `overlay` draws above the rig and fixtures stay put;
   * `underlay` draws on the stage beneath the rig; `caught` outlines pixels, fixture or grid name →
   * a set of pixel indices; `crossedOut` names the fixtures and grids to cross out.
   */
  let { overlay, underlay, caught, crossedOut } = $props();

  /** Room around the canvas, so fixtures on an edge stay whole. */
  const MARGIN = 70;
  const PIXEL_RADIUS = 18;
  const GRID = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9];

  let svg;
  let error = $state('');
  /** The fixture being dragged: offset in canvas units, kept until the saved rig comes back. */
  let drag = $state(null);
  const editable = $derived(engine.connected && engine.rig?.mode === 'edit');

  $effect(() => {
    engine.rig;
    drag = null;
  });

  /** The canvas point under a pointer event, which may be off the canvas. */
  export function canvasPoint(event) {
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(svg.getScreenCTM().inverse());
    return { x: point.x / WIDTH, y: 1 - point.y / HEIGHT };
  }

  function grab(event, fixture) {
    if (!editable || drag?.saving || overlay) return;
    error = '';
    const xs = fixture.pixels.map((pixel) => pixel.x);
    const { y } = fixture.pixels[0];
    drag = {
      name: fixture.name,
      from: canvasPoint(event),
      // Keeps every pixel on the canvas.
      limits: { left: -Math.min(...xs), right: 1 - Math.max(...xs), down: -y, up: 1 - y },
      dx: 0,
      dy: 0,
      saving: false,
    };
  }

  function move(event) {
    if (!drag || drag.saving) return;
    const at = canvasPoint(event);
    drag.dx = clamp(at.x - drag.from.x, drag.limits.left, drag.limits.right);
    drag.dy = clamp(at.y - drag.from.y, drag.limits.down, drag.limits.up);
  }

  async function drop() {
    if (!drag || drag.saving) return;
    if (drag.dx === 0 && drag.dy === 0) {
      drag = null;
      return;
    }
    drag.saving = true;
    try {
      // An unchanged rig brings no new rig message to end the drag.
      if (!(await save(drag.name, drag.dx, drag.dy))) drag = null;
    } catch (failure) {
      error = failure.message;
      drag = null;
    }
  }

  /**
   * Moves a fixture in rig.yaml through the engine; the stream then sends the new rig.
   * Returns false, saving nothing, when the move rounds or clamps back to where it was.
   */
  async function save(name, dx, dy) {
    const rig = await request('GET', '/api/show/rig.yaml');
    const { canvas } = rig.fixtures.find((fixture) => fixture.name === name);
    const x = tidy(clamp(canvas.x + dx, 0, 1 - (canvas.width ?? 0)));
    const y = tidy(clamp(canvas.y + dy, 0, 1));
    if (x === canvas.x && y === canvas.y) return false;
    Object.assign(canvas, { x, y });
    await request('PUT', '/api/show/rig.yaml', rig);
    return true;
  }

  /** A grid pixel's tile on screen: the spacing between its neighbours, less a gap. */
  function gridCell(pixels) {
    const spacing = (positions) => {
      const sorted = [...new Set(positions)].sort((a, b) => a - b);
      return Math.min(...sorted.slice(1).map((position, index) => position - sorted[index]));
    };
    const width = spacing(pixels.map((pixel) => screenX(pixel.x)));
    const height = spacing(pixels.map((pixel) => screenY(pixel.y)));
    // A single row or column takes its spacing from the other way; a single pixel, a fixture's size.
    const fallback = Math.min(width, height, 2 * PIXEL_RADIUS);
    return {
      width: 0.85 * (Number.isFinite(width) ? width : fallback),
      height: 0.85 * (Number.isFinite(height) ? height : fallback),
    };
  }

  const offset = (fixture) =>
    drag?.name === fixture.name ? `translate(${drag.dx * WIDTH} ${-drag.dy * HEIGHT})` : undefined;
</script>

<svelte:window onpointermove={move} onpointerup={drop} />

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if !overlay}
  <p class="hint">
    {#if editable}
      Drag a fixture to move it. It is saved to <code>rig.yaml</code> when you let go.
    {:else}
      Show mode: positions are locked. Start the engine with <code>--edit</code> to move fixtures.
    {/if}
  </p>
{/if}

{#snippet cross(left, top, right, bottom)}
  <path class="crossed" d="M{left} {top}L{right} {bottom}M{left} {bottom}L{right} {top}" />
{/snippet}

<svg
  bind:this={svg}
  viewBox="{-MARGIN} {-MARGIN} {WIDTH + 2 * MARGIN} {HEIGHT + 2 * MARGIN}"
  class:editable={editable && !overlay}
  role="img"
  aria-label="Front view"
>
  <rect width={WIDTH} height={HEIGHT} class="stage" />
  {#each GRID as line (line)}
    <line x1={screenX(line)} x2={screenX(line)} y2={HEIGHT} class="grid" />
    <line y1={screenY(line)} y2={screenY(line)} x2={WIDTH} class="grid" />
  {/each}
  <line x2={WIDTH} y1={HEIGHT} y2={HEIGHT} class="floor" />
  <text x={WIDTH / 2} y={-MARGIN / 2} class="side">Audience view</text>
  <text x="0" y={HEIGHT + MARGIN / 2} class="side" text-anchor="start">Stage right</text>
  <text x={WIDTH / 2} y={HEIGHT + MARGIN / 2} class="side">Floor</text>
  <text x={WIDTH} y={HEIGHT + MARGIN / 2} class="side" text-anchor="end">Stage left</text>
  {@render underlay?.()}

  {#each engine.rig?.fixtures ?? [] as fixture (fixture.name)}
    {@const colours = engine.frame?.pixels[fixture.name] ?? []}
    {@const first = fixture.pixels[0]}
    {@const last = fixture.pixels.at(-1)}
    {@const body = {
      left: screenX(first.x) - PIXEL_RADIUS - 6,
      top: screenY(first.y) - PIXEL_RADIUS - 6,
      right: screenX(last.x) + PIXEL_RADIUS + 6,
      bottom: screenY(first.y) + PIXEL_RADIUS + 6,
    }}
    {@const caughtPixels = caught?.get(fixture.name)}
    <g
      class="fixture"
      data-fixture={fixture.name}
      data-crossed-out={crossedOut?.has(fixture.name) || undefined}
      transform={offset(fixture)}
      onpointerdown={(event) => grab(event, fixture)}
      role="button"
      tabindex="-1"
    >
      <rect x={body.left} y={body.top} width={body.right - body.left} height={body.bottom - body.top} rx="10" class="body" />
      {#each fixture.pixels as pixel, index (index)}
        <circle
          data-pixel={index}
          class:caught={caughtPixels?.has(index)}
          cx={screenX(pixel.x)}
          cy={screenY(pixel.y)}
          r={PIXEL_RADIUS}
          fill={colours[index] ?? '#000000'}
        />
      {/each}
      {#if crossedOut?.has(fixture.name)}{@render cross(body.left, body.top, body.right, body.bottom)}{/if}
      <text x={screenX((first.x + last.x) / 2)} y={screenY(first.y) + PIXEL_RADIUS + 32} class="label">
        {fixture.name}
      </text>
    </g>
  {/each}

  <!-- Grids are placed in rig.yaml by their canvas rectangle, so they don't drag. -->
  {#each engine.rig?.grids ?? [] as grid (grid.name)}
    {@const colours = engine.frame?.pixels[grid.name] ?? []}
    {@const cell = gridCell(grid.pixels)}
    {@const xs = grid.pixels.map((pixel) => screenX(pixel.x))}
    {@const bottom = Math.max(...grid.pixels.map((pixel) => screenY(pixel.y)))}
    {@const top = Math.min(...grid.pixels.map((pixel) => screenY(pixel.y)))}
    {@const caughtPixels = caught?.get(grid.name)}
    <g class="pixel-grid" data-grid={grid.name} data-crossed-out={crossedOut?.has(grid.name) || undefined}>
      {#each grid.pixels as pixel, index (index)}
        <rect
          data-pixel={index}
          class:caught={caughtPixels?.has(index)}
          x={screenX(pixel.x) - cell.width / 2}
          y={screenY(pixel.y) - cell.height / 2}
          width={cell.width}
          height={cell.height}
          fill={colours[index] ?? '#000000'}
        />
      {/each}
      {#if crossedOut?.has(grid.name)}
        {@render cross(Math.min(...xs) - cell.width / 2, top - cell.height / 2, Math.max(...xs) + cell.width / 2, bottom + cell.height / 2)}
      {/if}
      <text x={(Math.min(...xs) + Math.max(...xs)) / 2} y={bottom + cell.height / 2 + 26} class="label">{grid.name}</text>
    </g>
  {/each}

  {@render overlay?.()}
</svg>

<style>
  svg {
    display: block;
    width: 100%;
    height: auto;
    user-select: none;
    touch-action: none;
  }

  .stage {
    fill: #101114;
  }

  .grid {
    stroke: #23252b;
    stroke-width: 1;
  }

  .floor {
    stroke: var(--dim);
    stroke-width: 4;
  }

  .side {
    fill: var(--dim);
    font-size: 18px;
    text-anchor: middle;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .body {
    fill: #1c1e24;
    stroke: #3a3d47;
  }

  circle {
    stroke: #3a3d47;
    stroke-width: 2;
  }

  .pixel-grid rect {
    stroke: #23252b;
    stroke-width: 1;
  }

  circle.caught,
  .pixel-grid rect.caught {
    stroke: var(--accent);
    stroke-width: 6;
  }

  .crossed {
    stroke: #ff7b7b;
    stroke-width: 6;
    stroke-linecap: round;
  }

  .label {
    fill: var(--text);
    font-size: 22px;
    text-anchor: middle;
  }

  .editable .fixture {
    cursor: grab;
  }

  .editable .fixture:hover .body {
    stroke: var(--accent);
  }
</style>

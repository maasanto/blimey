<script>
  import { engine, request } from './engine.svelte.js';

  // The 0–1 canvas drawn at a 16:9 stage ratio.
  const WIDTH = 1600;
  const HEIGHT = 900;
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

  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
  const round = (value) => Math.round(value * 1000) / 1000;

  function canvasPoint(event) {
    const box = svg.getBoundingClientRect();
    return { x: (event.clientX - box.left) / box.width, y: (event.clientY - box.top) / box.height };
  }

  function grab(event, fixture) {
    if (!editable || drag?.saving) return;
    error = '';
    const xs = fixture.pixels.map((pixel) => pixel.x);
    drag = {
      name: fixture.name,
      from: canvasPoint(event),
      // Keeps every pixel on the canvas.
      limits: { left: -Math.min(...xs), right: 1 - Math.max(...xs), up: -fixture.pixels[0].y, down: 1 - fixture.pixels[0].y },
      dx: 0,
      dy: 0,
      saving: false,
    };
  }

  function move(event) {
    if (!drag || drag.saving) return;
    const at = canvasPoint(event);
    drag.dx = clamp(at.x - drag.from.x, drag.limits.left, drag.limits.right);
    drag.dy = clamp(at.y - drag.from.y, drag.limits.up, drag.limits.down);
  }

  async function drop() {
    if (!drag || drag.saving) return;
    if (drag.dx === 0 && drag.dy === 0) {
      drag = null;
      return;
    }
    drag.saving = true;
    try {
      await save(drag.name, drag.dx, drag.dy);
    } catch (failure) {
      error = failure.message;
      drag = null;
    }
  }

  /** Moves a fixture in rig.yaml through the engine; the stream then sends the new rig. */
  async function save(name, dx, dy) {
    const rig = await request('GET', '/api/show/rig.yaml');
    const { canvas } = rig.fixtures.find((fixture) => fixture.name === name);
    canvas.x = round(clamp(canvas.x + dx, 0, 1 - (canvas.width ?? 0)));
    canvas.y = round(clamp(canvas.y + dy, 0, 1));
    await request('PUT', '/api/show/rig.yaml', rig);
  }

  const offset = (fixture) =>
    drag?.name === fixture.name ? `translate(${drag.dx * WIDTH} ${drag.dy * HEIGHT})` : undefined;
</script>

<svelte:window onpointermove={move} onpointerup={drop} />

{#if error}<p class="error" role="alert">{error}</p>{/if}
<p class="hint">
  {#if editable}
    Drag a fixture to move it. It is saved to <code>rig.yaml</code> when you let go.
  {:else}
    Show mode: positions are locked. Start the engine with <code>--edit</code> to move fixtures.
  {/if}
</p>

<svg bind:this={svg} viewBox="0 0 {WIDTH} {HEIGHT}" class:editable role="img" aria-label="Stage plan">
  <rect width={WIDTH} height={HEIGHT} class="stage" />
  {#each GRID as line (line)}
    <line x1={line * WIDTH} x2={line * WIDTH} y2={HEIGHT} class="grid" />
    <line y1={line * HEIGHT} y2={line * HEIGHT} x2={WIDTH} class="grid" />
  {/each}
  <text x={WIDTH / 2} y="28" class="side">Upstage</text>
  <text x={WIDTH / 2} y={HEIGHT - 14} class="side">Downstage · audience</text>

  {#each engine.rig?.fixtures ?? [] as fixture (fixture.name)}
    {@const colours = engine.frame?.pixels[fixture.name] ?? []}
    {@const first = fixture.pixels[0]}
    {@const last = fixture.pixels.at(-1)}
    <g
      class="fixture"
      data-fixture={fixture.name}
      transform={offset(fixture)}
      onpointerdown={(event) => grab(event, fixture)}
      role="button"
      tabindex="-1"
    >
      <rect
        x={first.x * WIDTH - PIXEL_RADIUS - 6}
        y={first.y * HEIGHT - PIXEL_RADIUS - 6}
        width={(last.x - first.x) * WIDTH + 2 * PIXEL_RADIUS + 12}
        height={2 * PIXEL_RADIUS + 12}
        rx="10"
        class="body"
      />
      {#each fixture.pixels as pixel, index (index)}
        <circle
          data-pixel={index}
          cx={pixel.x * WIDTH}
          cy={pixel.y * HEIGHT}
          r={PIXEL_RADIUS}
          fill={colours[index] ?? '#000000'}
        />
      {/each}
      <text x={((first.x + last.x) / 2) * WIDTH} y={first.y * HEIGHT + PIXEL_RADIUS + 32} class="label">
        {fixture.name}
      </text>
    </g>
  {/each}
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

  .side {
    fill: #5d6170;
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

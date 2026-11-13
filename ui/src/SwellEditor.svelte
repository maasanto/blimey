<script>
  import { formatLength, lengthInBeats, parseLength } from './duration.js';
  import { SAMPLES, SHAPES, curveLevels, curvePoints, envelope } from './envelope.svelte.js';
  import TargetPicker from './TargetPicker.svelte';

  /**
   * `cue` is the swell as in the song file; `saved` is the cue as saved, which is what an audition
   * plays, or null before the first save. `beats` is how far the saved cue has played while it is
   * auditioned (the stream's `beats`); `onaudition` starts or stops that, `onrestart` plays it
   * again from the top.
   */
  let { cue = $bindable(), saved, targets, palettes, barBeats, beats, onaudition, onrestart } = $props();

  const WIDTH = 600;
  const HEIGHT = 160;
  /** Room around the curve, so points on its edges show whole. */
  const POINT_MARGIN = 10;
  /** A looped audition holds the swell's end this long before playing it again. */
  const HOLD_BARS = 1;
  /** Points sit on hundredths, so the file stays readable and times always rise. */
  const STEP = 0.01;

  let svg = $state();
  let dragging = $state(null);

  const curve = $derived(cue.curve ?? 'ramp-up');
  const points = $derived(typeof curve === 'object' ? curve.points : null);
  // While a dragged curve is on its way to the engine, the last one drawn stays up.
  let lastLevels = [];
  const levels = $derived.by(() => (lastLevels = curveLevels(curve) ?? lastLevels));
  const length = $derived(parseLength(cue.duration) ?? { count: 4, unit: 'bars' });
  const units = $derived([...new Set(['beats', 'bars', length.unit])]);
  const swellBeats = $derived(lengthInBeats(length, barBeats));
  /** The saved swell's length in beats: null for one timed in s or ms, which has no playhead. */
  const savedBeats = $derived(saved ? lengthInBeats(parseLength(saved.duration) ?? { count: 0, unit: 's' }, barBeats) : null);
  const progress = $derived(beats === undefined || !savedBeats ? null : Math.min(Math.max(beats / savedBeats, 0), 1));

  // Only an audition started here loops: a cue fired from Reaper or the cue list plays as fired.
  let looping = $state(false);
  let restarted = false;
  $effect(() => {
    if (!looping || beats === undefined || !savedBeats) return;
    if (beats < savedBeats) restarted = false;
    else if (!restarted && beats >= savedBeats + HOLD_BARS * barBeats) {
      restarted = true;
      onrestart();
    }
  });

  function toggleAudition() {
    looping = beats === undefined;
    onaudition();
  }

  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
  const round = (value) => Math.round(value / STEP) * STEP;
  const tidy = (value) => Number(round(value).toFixed(2));

  function curvePoint(event) {
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(svg.getScreenCTM().inverse());
    return [point.x / WIDTH, 1 - point.y / HEIGHT];
  }

  function chooseCurve(choice) {
    if (choice !== 'custom') {
      cue.curve = choice;
      return;
    }
    // Custom points start from the shape on screen.
    const start = [0, 0.25, 0.5, 0.75, 1].map((time) => [time, tidy(levels[Math.round(time * (SAMPLES - 1))] ?? time)]);
    cue.curve = { points: start };
  }

  function drag(event) {
    if (dragging === null) return;
    const [time, level] = curvePoint(event);
    const earliest = dragging === 0 ? 0 : points[dragging - 1][0] + STEP;
    const latest = dragging === points.length - 1 ? 1 : points[dragging + 1][0] - STEP;
    // Neighbours closer than two steps, as a file may have them, leave no room to slide: the
    // point only moves up and down, so times still rise.
    const moved = earliest > latest ? points[dragging][0] : tidy(clamp(time, earliest, latest));
    points[dragging] = [moved, tidy(clamp(level, 0, 1))];
  }

  function addPoint(event) {
    const [time, level] = curvePoint(event).map((value) => tidy(clamp(value, 0, 1)));
    if (points.some(([other]) => other === time)) return;
    const after = points.findIndex(([other]) => other > time);
    points.splice(after === -1 ? points.length : after, 0, [time, level]);
  }

  function removePoint(index) {
    if (points.length > 1) points.splice(index, 1);
  }
</script>

<svelte:window onpointermove={drag} onpointerup={() => (dragging = null)} />

<fieldset class="editor">
  <legend>Swell</legend>
  <div class="row">
    <TargetPicker bind:target={cue.target} {targets} {palettes} dimmer={{ dimmer: '100%' }} />
  </div>
  <div class="row">
    <label>
      Curve
      <select aria-label="Curve" value={points ? 'custom' : curve} onchange={(event) => chooseCurve(event.currentTarget.value)}>
        {#each SHAPES as shape (shape)}<option>{shape}</option>{/each}
        <option value="custom">custom points</option>
      </select>
    </label>
    <label>
      Swell length
      <input
        aria-label="Swell length"
        type="number"
        min="0"
        step="any"
        value={length.count}
        oninput={(event) => (cue.duration = formatLength(Number(event.currentTarget.value), length.unit))}
      />
    </label>
    <select
      aria-label="Swell length unit"
      value={length.unit}
      onchange={(event) => (cue.duration = formatLength(length.count, event.currentTarget.value))}
    >
      {#each units as unit (unit)}<option>{unit}</option>{/each}
    </select>
  </div>

  <svg
    bind:this={svg}
    viewBox="{-POINT_MARGIN} {-POINT_MARGIN} {WIDTH + 2 * POINT_MARGIN} {HEIGHT + 2 * POINT_MARGIN}"
    class="curve" data-curve={JSON.stringify(curve)}>
    <rect
      width={WIDTH}
      height={HEIGHT}
      class="background"
      role="button"
      tabindex="-1"
      aria-label="Add a point"
      ondblclick={(event) => points && addPoint(event)}
    />
    {#if swellBeats}
      {#each Array.from({ length: Math.ceil(swellBeats / barBeats) - 1 }, (_, bar) => bar) as bar (bar)}
        <line class="bar-line" x1={((bar + 1) * barBeats * WIDTH) / swellBeats} x2={((bar + 1) * barBeats * WIDTH) / swellBeats} y2={HEIGHT} />
      {/each}
    {/if}
    {#if levels.length}<polyline class="level" points={curvePoints(levels, { width: WIDTH, height: HEIGHT })} />{/if}
    {#each points ?? [] as [time, level], index (index)}
      <circle
        class="point"
        data-point={index}
        cx={time * WIDTH}
        cy={(1 - level) * HEIGHT}
        r="7"
        role="button"
        tabindex="-1"
        aria-label="Point {index + 1} at {time}, {level}"
        onpointerdown={() => (dragging = index)}
        ondblclick={() => removePoint(index)}
      />
    {/each}
    {#if progress !== null}<line class="playhead" x1={progress * WIDTH} x2={progress * WIDTH} y2={HEIGHT} />{/if}
  </svg>
  {#if envelope.error}<p class="error" role="alert">{envelope.error}</p>{/if}
  {#if points}<p class="hint">Drag a point to shape the curve; double-click the curve to add one, a point to remove it.</p>{/if}

  <div class="row">
    <button disabled={!saved} onclick={toggleAudition}>{beats === undefined ? 'Audition loop' : 'Stop audition'}</button>
    <span class="hint">
      {#if !saved}
        Save the cue to audition it.
      {:else if savedBeats}
        Plays the saved cue, again each time its end has held for a bar. Save to hear changes.
      {:else}
        Plays the saved cue once: a swell timed in s or ms has no playhead or loop. Save to hear changes.
      {/if}
    </span>
  </div>
</fieldset>

<style>
  .curve {
    display: block;
    width: 100%;
    max-width: 900px;
    margin: 10px 0;
    touch-action: none;
  }

  .background {
    fill: #0f1013;
    stroke: var(--line);
  }

  .bar-line {
    stroke: var(--line);
  }

  .level {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.5;
    pointer-events: none;
  }

  .point {
    fill: #16171b;
    stroke: var(--accent);
    stroke-width: 2;
    cursor: grab;
  }

  .playhead {
    stroke: #ffffff;
    stroke-width: 2;
    pointer-events: none;
  }
</style>

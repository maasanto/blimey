<script>
  import { formatLength, lengthInBeats, parseLength } from './duration.js';
  import { SHAPES, curveLevels, curvePoints, envelope } from './envelope.svelte.js';
  import TargetPicker from './TargetPicker.svelte';

  /**
   * `cue` is the pattern as in the song file; `saved` is the cue as saved, which is what an
   * audition plays, or null before the first save. `beats` is how far the saved cue's loop has
   * played while it is auditioned (the stream's `beats`); `onaudition` starts or stops that.
   */
  let { cue = $bindable(), saved, targets, palettes, barBeats, beats, onaudition } = $props();

  /** Hits sit on quarter beats (docs/api.md). */
  const STEPS_PER_BEAT = 4;
  const MAX_BARS = 4;
  const LANE_HEIGHT = 1;
  const NEW_HIT = { shape: 'ramp-up', length: '0.5 beats', level: '100%' };

  /** Index in `cue.hits` of the hit being edited. */
  let selected = $state(null);
  let copyFrom = $state(1);
  let copyTo = $state(2);

  const bars = $derived(parseLength(cue.length)?.count ?? 1);
  const loopBeats = $derived(bars * barBeats);
  const steps = $derived(Array.from({ length: loopBeats * STEPS_PER_BEAT }, (_, step) => 1 + step / STEPS_PER_BEAT));
  const hit = $derived(cue.hits[selected]);
  const hitLength = $derived(parseLength(hit?.length) ?? { count: 0.5, unit: 'beats' });
  const savedLoopBeats = $derived(saved ? (parseLength(saved.length)?.count ?? 1) * barBeats : null);
  const playhead = $derived(beats === undefined || beats < 0 || !savedLoopBeats ? null : beats % savedLoopBeats);
  const barLines = $derived(Array.from({ length: bars }, (_, bar) => bar * barBeats));
  const beatLines = $derived(Array.from({ length: Math.ceil(loopBeats) }, (_, beat) => beat));

  const hitIndex = (at) => cue.hits.findIndex((other) => other.at === at);
  const barOf = (at) => Math.floor((at - 1) / barBeats) + 1;
  const percent = (level) => parseFloat(level);

  /** Adds `hit` before the first hit after it, leaving the order of the others as loaded. */
  function insert(hit) {
    const after = cue.hits.findIndex((other) => other.at > hit.at);
    const index = after === -1 ? cue.hits.length : after;
    cue.hits.splice(index, 0, hit);
    return index;
  }

  /** Selects the hit on `at`, or places one there like the hit selected before. */
  function pick(at) {
    const existing = hitIndex(at);
    if (existing !== -1) {
      selected = existing;
      return;
    }
    const { shape, length, level } = hit ?? NEW_HIT;
    selected = insert({ at, shape: $state.snapshot(shape), length, level });
  }

  function remove() {
    cue.hits.splice(selected, 1);
    selected = null;
  }

  /** Drops the hits past the new loop's end and shortens those now longer than the loop. */
  function setBars(count) {
    const beats = count * barBeats;
    cue.length = formatLength(count, 'bars');
    cue.hits = cue.hits.filter((other) => other.at - 1 < beats);
    for (const other of cue.hits) {
      if (lengthInBeats(parseLength(other.length) ?? { count: 0, unit: 'beats' }, barBeats) > beats) {
        other.length = formatLength(count, 'bars');
      }
    }
    selected = null;
  }

  function copyBar() {
    const shift = (copyTo - copyFrom) * barBeats;
    const copies = cue.hits.filter((other) => barOf(other.at) === copyFrom).map((other) => ({ ...$state.snapshot(other), at: other.at + shift }));
    cue.hits = cue.hits.filter((other) => barOf(other.at) !== copyTo);
    copies.forEach(insert);
    selected = null;
  }

  /** The hit's drawn length in beats, at most the loop. */
  const drawnBeats = (other) => Math.min(lengthInBeats(parseLength(other.length) ?? { count: 0, unit: 'beats' }, barBeats) ?? 0, loopBeats);
</script>

<fieldset class="editor">
  <legend>Pattern</legend>
  <div class="row">
    <TargetPicker bind:target={cue.target} {targets} {palettes} dimmer="dimmer" />
    <label>
      Bars
      <select value={bars} onchange={(event) => setBars(Number(event.currentTarget.value))}>
        {#each Array.from({ length: MAX_BARS }, (_, index) => index + 1) as count (count)}<option value={count}>{count}</option>{/each}
      </select>
    </label>
    {#if bars > 1}
      <label>
        Copy bar
        <select bind:value={copyFrom}>
          {#each Array.from({ length: bars }, (_, index) => index + 1) as bar (bar)}<option value={bar}>{bar}</option>{/each}
        </select>
      </label>
      <label>
        to bar
        <select bind:value={copyTo}>
          {#each Array.from({ length: bars }, (_, index) => index + 1) as bar (bar)}<option value={bar}>{bar}</option>{/each}
        </select>
      </label>
      <button disabled={copyFrom === copyTo || copyTo > bars || copyFrom > bars} onclick={copyBar}>Copy</button>
    {/if}
  </div>

  <div class="grid" style:--steps={steps.length}>
    <svg class="lane" viewBox="0 0 {loopBeats} {LANE_HEIGHT}" preserveAspectRatio="none" aria-hidden="true">
      {#each beatLines as beat (beat)}
        <line class="beat-line" x1={beat} x2={beat} y2={LANE_HEIGHT} />
      {/each}
      {#each barLines as bar (bar)}
        <line class="beat-line bar-line" x1={bar} x2={bar} y2={LANE_HEIGHT} />
      {/each}
      {#each cue.hits as other, index (index)}
        {@const levels = curveLevels(other.shape)}
        {@const offset = other.at - 1}
        {#if levels}
          <!-- Drawn again one loop earlier, so a hit running past the loop's end wraps to its start. -->
          {#each [offset, offset - loopBeats] as x (x)}
            <polyline
              class="hit-level"
              class:selected={index === selected}
              points={curvePoints(levels, { x, width: drawnBeats(other), height: LANE_HEIGHT, scale: percent(other.level) / 100 })}
            />
          {/each}
        {/if}
      {/each}
      {#if playhead !== null}<line class="playhead" x1={playhead} x2={playhead} y2={LANE_HEIGHT} />{/if}
    </svg>
    <div class="steps">
      {#each steps as at (at)}
        {@const index = hitIndex(at)}
        <button
          class="step"
          class:beat={(at - 1) % 1 === 0}
          class:bar={(at - 1) % barBeats === 0}
          class:selected={index !== -1 && index === selected}
          aria-label="at {at}"
          aria-pressed={index !== -1}
          title="Beat {at}"
          onclick={() => pick(at)}
        ></button>
      {/each}
    </div>
  </div>

  {#if envelope.error}<p class="error" role="alert">{envelope.error}</p>{/if}
  {#if hit}
    <div class="row">
      <strong>Hit at {hit.at}</strong>
      <label>
        Hit shape
        <select value={typeof hit.shape === 'object' ? 'custom' : hit.shape} onchange={(event) => (hit.shape = event.currentTarget.value)}>
          {#each SHAPES as shape (shape)}<option>{shape}</option>{/each}
          {#if typeof hit.shape === 'object'}<option value="custom" disabled>custom points</option>{/if}
        </select>
      </label>
      <label>
        Hit length
        <input
          type="number"
          min="0.25"
          step="0.25"
          value={hitLength.count}
          oninput={(event) => (hit.length = formatLength(Number(event.currentTarget.value), hitLength.unit))}
        />
      </label>
      <select aria-label="Hit length unit" value={hitLength.unit} onchange={(event) => (hit.length = formatLength(hitLength.count, event.currentTarget.value))}>
        <option>beats</option>
        <option>bars</option>
      </select>
      <label>
        Hit level
        <input type="range" min="0" max="100" value={percent(hit.level)} oninput={(event) => (hit.level = `${event.currentTarget.value}%`)} />
        <output>{hit.level}</output>
      </label>
      <button onclick={remove}>Remove hit</button>
    </div>
  {:else}
    <p class="hint">Click a step to place a hit, or a hit to edit it. A new hit takes the shape, length and level of the one edited last.</p>
  {/if}

  <div class="row">
    <button disabled={!saved} onclick={onaudition}>{beats === undefined ? 'Audition loop' : 'Stop audition'}</button>
    <span class="hint">
      {saved ? 'Loops the saved cue from the next bar line. Save to hear changes.' : 'Save the cue to audition it.'}
    </span>
  </div>
</fieldset>

<style>
  .grid {
    margin: 10px 0;
  }

  .lane {
    display: block;
    width: 100%;
    height: 80px;
    background: #0f1013;
  }

  .beat-line {
    stroke: var(--line);
    stroke-width: 1px;
    vector-effect: non-scaling-stroke;
  }

  .bar-line {
    stroke: var(--muted);
  }

  .hit-level {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2px;
    vector-effect: non-scaling-stroke;
  }

  .hit-level.selected {
    stroke: #ffffff;
  }

  .playhead {
    stroke: #ffffff;
    stroke-width: 2px;
    vector-effect: non-scaling-stroke;
  }

  .steps {
    display: grid;
    grid-template-columns: repeat(var(--steps), minmax(0, 1fr));
    gap: 2px;
    margin-top: 4px;
  }

  .step {
    height: 28px;
    padding: 0;
    background: #1d1f25;
    border-color: transparent;
    border-radius: 3px;
  }

  .step.beat {
    background: #272a32;
  }

  .step.bar {
    border-left: 2px solid var(--muted);
  }

  .step[aria-pressed='true'] {
    background: var(--accent);
  }

  .step.selected {
    outline: 2px solid #ffffff;
    outline-offset: 1px;
  }
</style>

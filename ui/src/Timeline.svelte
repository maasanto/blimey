<script>
  import { request } from './engine.svelte.js';
  import { beatsIn, beatsPerBar, formatLength } from './duration.js';
  import ScenePads, { SCENE_DRAG_TYPE } from './ScenePads.svelte';

  /**
   * The running order of `song`, the song file `songName` as JSON (docs/api.md, Sections and
   * arrangement). Every edit saves at once through `PUT /api/songs/<song>/arrangement`, then
   * `onsaved(message)` reloads the song; `attempt(action)` shows why an action failed.
   * `onhold(scene, velocity)` starts an audition and resolves once it is sent, `onrelease(scene)`
   * ends it, `onaudition(scene)` starts or ends one from the keyboard; `onopenscene(scene)` opens
   * a scene's editor. `editable`: false in show mode, where the timeline is read-only.
   */
  let { songName, song, editable, isPlaying, attempt, onsaved, onhold, onrelease, onaudition, onopenscene } = $props();

  const PLACEMENT_DRAG_TYPE = 'application/x-blimey-placement';
  /** A press on a block held this long auditions its scene, as on a scene pad. */
  const HOLD_MS = 250;
  /** The narrowest beat on the section editor's ruler; a short section widens its beats to fill it. */
  const MIN_BEAT_PX = 24;
  /** The lane names' column, left of the ruler. */
  const LANE_LABEL_PX = 120;
  const OVERVIEW_LANE_PX = 6;
  const NEW_SECTION_BARS = 4;
  const FULL_LEVEL = 100;
  const MAX_VELOCITY = 127;
  const DEFAULT_BPM = 120;

  let openName = $state(null);
  let newName = $state('');
  /** The selected block of the open section, by scene and start. */
  let selected = $state(null);
  /** The block being moved or resized, as shown until it saves: `{ block, start, length }`. */
  let dragged = $state.raw(null);
  /** The press on a block, not reactive: the pointer, the block, how it moves, the audition. */
  let press = null;
  let editorWidth = $state(0);

  const songSignature = $derived(song.time_signature ?? '4/4');
  const sections = $derived(Object.entries(song.sections ?? {}).map(([name, file]) => sectionOf(name, file)));
  const byName = $derived(Object.fromEntries(sections.map((section) => [section.name, section])));
  const arrangement = $derived(song.arrangement ?? []);
  const openSection = $derived(byName[openName] ?? byName[arrangement[0]] ?? sections[0] ?? null);
  const selectedBlock = $derived(
    selected?.section === openSection?.name ? openSection?.blocks.find((block) => block.scene === selected.scene && block.start === selected.start) : undefined,
  );
  const beatPx = $derived(openSection ? Math.max(MIN_BEAT_PX, Math.floor((editorWidth - LANE_LABEL_PX) / openSection.beats)) : MIN_BEAT_PX);
  const lanes = $derived(openSection ? scenesIn([openSection]) : []);
  /** Each placed section and the song's bar it starts on. */
  const placements = $derived.by(() => {
    let bar = 1;
    return arrangement.map((name, index) => {
      const placed = { index, name, section: byName[name], bar };
      bar += placed.section.bars;
      return placed;
    });
  });
  const overviewLanes = $derived(scenesIn(placements.map(({ section }) => section)));
  const totalBars = $derived(placements.reduce((sum, { section }) => sum + section.bars, 0));
  const totalSeconds = $derived((placements.reduce((sum, { section }) => sum + section.beats, 0) * 60) / (song.bpm ?? DEFAULT_BPM));

  /** A section of the file, its blocks in beats from its first beat, in playing order. */
  function sectionOf(name, { bars, time_signature, blocks = [] }) {
    const barBeats = beatsPerBar(time_signature ?? songSignature);
    const inBeats = ({ scene, bar, beat = 1, length, level }) => ({
      scene,
      start: (bar - 1) * barBeats + beat - 1,
      length: beatsIn(length, barBeats),
      level: level ? parseFloat(level) : FULL_LEVEL,
    });
    return { name, bars, time_signature, barBeats, beats: bars * barBeats, blocks: blocks.map(inBeats).sort(inPlayingOrder) };
  }

  const inPlayingOrder = (a, b) => a.start - b.start || a.scene - b.scene;
  const scenesIn = (placed) => [...new Set(placed.flatMap((section) => section.blocks.map((block) => block.scene)))].sort((a, b) => a - b);
  const clamp = (value, lowest, highest) => Math.min(Math.max(value, lowest), highest);

  /** As the file writes a section: keys in the order of docs/api.md, empty ones left out. */
  function sectionFile({ bars, time_signature, blocks }) {
    const barBeats = beatsPerBar(time_signature ?? songSignature);
    return {
      bars,
      ...(time_signature && { time_signature }),
      ...(blocks.length && { blocks: blocks.toSorted(inPlayingOrder).map((block) => blockFile(block, barBeats)) }),
    };
  }

  /** As the file writes a block: whole bars in bars, the rest in beats. */
  function blockFile({ scene, start, length, level }, barBeats) {
    const bar = Math.floor(start / barBeats) + 1;
    const beat = start - (bar - 1) * barBeats + 1;
    const bars = length / barBeats;
    return {
      scene,
      bar,
      ...(beat !== 1 && { beat }),
      length: Number.isInteger(bars) ? formatLength(bars, 'bars') : formatLength(length, 'beats'),
      ...(level !== FULL_LEVEL && { level: `${level}%` }),
    };
  }

  const sceneName = (scene) => song.cues?.[scene]?.name ?? `scene ${scene}`;
  const velocityOf = (level) => Math.max(1, Math.round((level / FULL_LEVEL) * MAX_VELOCITY));

  function describe(block, section) {
    const { bar, beat = 1, length } = blockFile(block, section.barBeats);
    return `${sceneName(block.scene)}, scene ${block.scene}: bar ${bar} beat ${beat}, ${length}, ${block.level}%`;
  }

  function minutes(seconds) {
    const whole = Math.round(seconds);
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
  }

  async function save(nextSections, nextArrangement = arrangement) {
    const sectionsFile = Object.fromEntries(nextSections.map((section) => [section.name, sectionFile(section)]));
    await request('PUT', `/api/songs/${songName}/arrangement`, { sections: sectionsFile, arrangement: [...nextArrangement] });
    await onsaved(`Saved the timeline of ${songName}. Undo takes it back.`);
  }

  /**
   * Runs `action` once the edit before it has saved and the song reloaded, so that each edit
   * starts from what the file holds: edits name sections and blocks rather than hold them.
   */
  let lastEdit = Promise.resolve();
  function edit(action) {
    lastEdit = lastEdit.catch(() => {}).then(action);
    return attempt(() => lastEdit);
  }

  const withSection = (name, change) => sections.map((section) => (section.name === name ? { ...section, ...change } : section));

  /** Saves what a field was changed to, or puts back what it showed when that is refused. */
  function change(event, shown, action) {
    const field = event.currentTarget;
    const value = field.value.trim();
    edit(() =>
      action(value).catch((failure) => {
        field.value = shown;
        throw failure;
      }),
    );
  }

  function refuseName(name) {
    if (!name) throw new Error('Give the section a name.');
    if (byName[name]) throw new Error(`There is already a section called ${name}.`);
  }

  async function createSection(name) {
    refuseName(name);
    await save([...sections, { name, bars: NEW_SECTION_BARS, blocks: [] }]);
    openName = name;
  }

  async function renameSection(from, name) {
    if (name === from) return;
    refuseName(name);
    await save(
      withSection(from, { name }),
      arrangement.map((placed) => (placed === from ? name : placed)),
    );
    openName = name;
  }

  async function duplicateSection(original) {
    let name = `${original}-copy`;
    for (let copy = 2; byName[name]; copy += 1) name = `${original}-copy-${copy}`;
    await save([...sections, { ...byName[original], name }]);
    openName = name;
  }

  async function deleteSection(name) {
    if (arrangement.includes(name)) throw new Error(`${name} plays in the arrangement: take it out of the arrangement before deleting it.`);
    await save(sections.filter((section) => section.name !== name));
  }

  const place = (name) => save(sections, [...arrangement, name]);
  const unplace = (index) => save(sections, arrangement.toSpliced(index, 1));
  const movePlacement = (from, to) => from !== to && save(sections, arrangement.toSpliced(from, 1).toSpliced(to, 0, arrangement[from]));

  const isBlock = (block) => (other) => other.scene === block.scene && other.start === block.start;
  const withBlocks = (name, change) => save(withSection(name, { blocks: change(byName[name].blocks) }));
  const deleteBlock = (name, block) => withBlocks(name, (blocks) => blocks.filter((other) => !isBlock(block)(other)));

  function changeBlock(name, block, change) {
    selected = { section: name, scene: block.scene, start: change.start ?? block.start };
    return withBlocks(name, (blocks) => blocks.map((other) => (isBlock(block)(other) ? { ...other, ...change } : other)));
  }

  /** A scene dropped on the section: a bar long from the beat it landed on, or to the end. */
  function dropScene(event, section) {
    if (!editable || !event.dataTransfer.types.includes(SCENE_DRAG_TYPE)) return;
    event.preventDefault();
    const scene = Number(event.dataTransfer.getData(SCENE_DRAG_TYPE));
    const beat = Math.floor((event.clientX - event.currentTarget.getBoundingClientRect().left) / beatPx);
    const start = clamp(beat, 0, Math.ceil(section.beats) - 1);
    const placed = { scene, start, length: Math.min(section.barBeats, section.beats - start), level: FULL_LEVEL };
    selected = { section: section.name, scene, start };
    edit(() => withBlocks(section.name, (blocks) => [...blocks, placed]));
  }

  const accepts = (event, type) => editable && event.dataTransfer.types.includes(type) && event.preventDefault();

  function dropPlacement(event, index) {
    if (!editable || !event.dataTransfer.types.includes(PLACEMENT_DRAG_TYPE)) return;
    event.preventDefault();
    const from = Number(event.dataTransfer.getData(PLACEMENT_DRAG_TYPE));
    edit(() => movePlacement(from, index));
  }

  /** `mode` is `move` for the block's body, `resize` for its end. */
  function pressBlock(event, section, block, mode) {
    if (!editable || press !== null || event.button !== 0) return;
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    selected = { section: section.name, scene: block.scene, start: block.start };
    press = { pointer: event.pointerId, x: event.clientX, section, block, mode, started: null };
    press.timer = setTimeout(() => {
      // A scene already playing, from Reaper or a pad, is left to play.
      if (!isPlaying(block.scene)) press.started = onhold(block.scene, velocityOf(block.level));
    }, HOLD_MS);
  }

  /** Moves or resizes the pressed block a whole beat at a time, inside its section. */
  function slide(event) {
    if (press?.pointer !== event.pointerId) return;
    const { x, section, block, mode } = press;
    const beats = Math.round((event.clientX - x) / beatPx);
    const start = mode === 'move' ? clamp(block.start + beats, 0, section.beats - block.length) : block.start;
    const length = mode === 'resize' ? clamp(block.length + beats, Math.min(1, block.length), section.beats - block.start) : block.length;
    if (!dragged && start === block.start && length === block.length) return;
    clearTimeout(press.timer);
    dragged = { block, start, length };
  }

  async function letGo(event) {
    if (press?.pointer !== event.pointerId) return;
    const { section, block, started, timer } = press;
    clearTimeout(timer);
    press = null;
    started?.then(() => onrelease(block.scene));
    const moved = dragged;
    if (!moved) return;
    await edit(() => changeBlock(section.name, block, { start: moved.start, length: moved.length }));
    dragged = null;
  }

  /** Delete removes the focused block; ← and → move it a beat, with Shift they resize it. */
  function blockKey(event, section, block) {
    const step = { ArrowLeft: -1, ArrowRight: 1 }[event.key];
    if (!editable || (!step && event.key !== 'Delete' && event.key !== 'Backspace')) return;
    event.preventDefault();
    if (!step) edit(() => deleteBlock(section.name, block));
    else if (event.shiftKey) edit(() => changeBlock(section.name, block, { length: block.length + step }));
    else edit(() => changeBlock(section.name, block, { start: block.start + step }));
  }

  const shownOf = (block) => (dragged?.block === block ? dragged : block);
</script>

<section class="arrangement" aria-label="Arrangement">
  <div class="heading">
    <h2>Arrangement</h2>
    <span class="hint" data-testid="song-length">{totalBars} bars · {minutes(totalSeconds)}</span>
  </div>
  {#if placements.length}
    <ol class="tiles">
      {#each placements as placed (placed.index)}
        <li
          class="tile"
          class:open={placed.name === openSection?.name}
          draggable={editable}
          ondragstart={(event) => event.dataTransfer.setData(PLACEMENT_DRAG_TYPE, String(placed.index))}
          ondragover={(event) => accepts(event, PLACEMENT_DRAG_TYPE)}
          ondrop={(event) => dropPlacement(event, placed.index)}
        >
          <button class="quiet" aria-label="{placed.name} from bar {placed.bar}" onclick={() => (openName = placed.name)}>
            {placed.name} <span class="hint">{placed.section.bars} bars</span>
          </button>
          <button class="quiet" aria-label="Take {placed.name} at bar {placed.bar} out of the arrangement" disabled={!editable} onclick={() => edit(() => unplace(placed.index))}>×</button>
        </li>
      {/each}
    </ol>

    <div class="overview" role="group" aria-label="Overview" style:--lanes={overviewLanes.length}>
      {#each placements as placed (placed.index)}
        <button class="placement" class:open={placed.name === openSection?.name} style:flex-grow={placed.section.beats} data-placement={placed.name} onclick={() => (openName = placed.name)}>
          <span class="at">{placed.bar} · {placed.name}</span>
          {#each placed.section.blocks as block (`${block.scene}:${block.start}`)}
            <span
              class="mini"
              title={describe(block, placed.section)}
              style:left="{(block.start / placed.section.beats) * 100}%"
              style:width="{(block.length / placed.section.beats) * 100}%"
              style:top="{overviewLanes.indexOf(block.scene) * OVERVIEW_LANE_PX}px"
              style:opacity={0.35 + (0.65 * block.level) / FULL_LEVEL}
            ></span>
          {/each}
        </button>
      {/each}
    </div>
  {:else}
    <p class="hint">Add sections to the arrangement with their +, in the order they play.</p>
  {/if}
</section>

<div class="workspace">
  <section class="library" aria-label="Sections">
    <h2>Sections</h2>
    <ul>
      {#each sections as section (section.name)}
        <li class:open={section === openSection}>
          <button class="quiet name" onclick={() => (openName = section.name)}>
            {section.name} <span class="hint">{section.bars} bars{section.time_signature ? ` · ${section.time_signature}` : ''}</span>
          </button>
          <button class="quiet" aria-label="Add {section.name} to the arrangement" disabled={!editable} onclick={() => edit(() => place(section.name))}>+</button>
        </li>
      {/each}
    </ul>
    <form
      class="row"
      onsubmit={(event) => {
        event.preventDefault();
        const name = newName.trim();
        edit(async () => {
          await createSection(name);
          newName = '';
        });
      }}
    >
      <input aria-label="New section name" placeholder="chorus" disabled={!editable} bind:value={newName} />
      <button disabled={!editable || !newName.trim()}>New section</button>
    </form>
  </section>

  <section class="section-editor" aria-label="Section editor" bind:clientWidth={editorWidth}>
    {#if openSection}
      {@const section = openSection}
      <fieldset class="row" disabled={!editable}>
        {#key song}
          <label>Name <input value={section.name} onchange={(event) => change(event, section.name, (name) => renameSection(section.name, name))} /></label>
          <label>
            Bars
            <input type="number" min="1" value={section.bars} onchange={(event) => change(event, section.bars, (bars) => save(withSection(section.name, { bars: Number(bars) })))} />
          </label>
          <label>
            Time signature
            <input
              value={section.time_signature ?? ''}
              placeholder={songSignature}
              onchange={(event) => change(event, section.time_signature ?? '', (signature) => save(withSection(section.name, { time_signature: signature || undefined })))}
            />
          </label>
        {/key}
        <button onclick={() => edit(() => duplicateSection(section.name))}>Duplicate</button>
        <button class="danger" onclick={() => edit(() => deleteSection(section.name))}>Delete</button>
      </fieldset>

      <div class="strip">
        <ScenePads {songName} cues={song.cues} {editable} {isPlaying} draggable={editable} onopen={onopenscene} onhold={(scene) => onhold(scene, MAX_VELOCITY)} {onrelease} {onaudition} />
      </div>

      <div class="lanes" style:--beat="{beatPx}px" style:--bar="{beatPx * section.barBeats}px" style:--label="{LANE_LABEL_PX}px">
        <div class="lane">
          <span></span>
          <div class="ruler track" style:width="{section.beats * beatPx}px" data-testid="ruler">
            {#each { length: section.bars } as _, bar (bar)}
              <span class="bar-number" style:left="{bar * section.barBeats * beatPx}px">{bar + 1}</span>
            {/each}
          </div>
        </div>
        {#each [...lanes, null] as scene (scene)}
          <div class="lane" data-lane={scene ?? 'new'}>
            <span class="lane-name">{scene === null ? 'Drop a scene here' : `${scene} · ${sceneName(scene)}`}</span>
            <div class="track" style:width="{section.beats * beatPx}px" role="presentation" ondragover={(event) => accepts(event, SCENE_DRAG_TYPE)} ondrop={(event) => dropScene(event, section)}>
              {#each section.blocks.filter((block) => block.scene === scene) as block (block.start)}
                {@const shown = shownOf(block)}
                <button
                  class="block"
                  class:selected={block === selectedBlock}
                  class:playing={isPlaying(block.scene)}
                  data-block={block.scene}
                  aria-label={describe(block, section)}
                  style:left="{shown.start * beatPx}px"
                  style:width="{shown.length * beatPx}px"
                  style:--level={block.level / FULL_LEVEL}
                  onpointerdown={(event) => pressBlock(event, section, block, 'move')}
                  onpointermove={slide}
                  onpointerup={letGo}
                  onpointercancel={letGo}
                  onkeydown={(event) => blockKey(event, section, block)}
                  oncontextmenu={(event) => event.preventDefault()}
                >
                  <span class="block-name">{sceneName(block.scene)}{block.level === FULL_LEVEL ? '' : ` ${block.level}%`}</span>
                  <span class="handle" aria-hidden="true" onpointerdown={(event) => pressBlock(event, section, block, 'resize')}></span>
                </button>
              {/each}
            </div>
          </div>
        {/each}
      </div>

      {#if selectedBlock}
        {@const block = selectedBlock}
        <fieldset class="row" disabled={!editable}>
          <span>{describe(block, section)}</span>
          {#key song}
            <label>Level <input type="number" min="0" max="100" value={block.level} onchange={(event) => change(event, block.level, (level) => changeBlock(section.name, block, { level: Number(level) }))} /></label>
            <span class="hint">%</span>
          {/key}
          <button class="danger" onclick={() => edit(() => deleteBlock(section.name, block))}>Delete block</button>
        </fieldset>
      {/if}
      <p class="hint">
        Drag a block to move it, its end to resize it, a beat at a time; hold it to audition its scene. The arrangement and the overview light up
        everywhere this section plays.
      </p>
    {:else}
      <p class="hint">Create a section, then drag scenes onto it.</p>
    {/if}
  </section>
</div>

<style>
  fieldset {
    margin: 0;
    padding: 0;
    border: 0;
  }

  .row,
  .heading {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 12px;
    align-items: center;
  }

  .heading h2 {
    margin: 0;
  }

  .tiles {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 0;
    margin: 8px 0 12px;
    list-style: none;
  }

  .tile {
    display: flex;
    align-items: center;
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    cursor: grab;
  }

  .tile.open,
  .placement.open,
  .library li.open {
    border-color: var(--accent);
  }

  .overview {
    display: flex;
    gap: 2px;
    margin-bottom: 20px;
  }

  .placement {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: flex-start;
    flex-basis: 0;
    min-width: 0;
    height: calc(28px + var(--lanes) * 6px);
    padding: 0;
    overflow: hidden;
    text-align: left;
    background: var(--well);
    border-radius: 4px;
  }

  .at {
    display: block;
    padding: 2px 6px;
    overflow: hidden;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .mini {
    position: absolute;
    height: 5px;
    margin-top: 22px;
    background: var(--accent);
    border-radius: 2px;
  }

  .workspace {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    gap: 20px;
    align-items: start;
  }

  .library ul {
    padding: 0;
    margin: 0 0 10px;
    list-style: none;
  }

  .library li {
    display: flex;
    margin-bottom: 4px;
    border: 1px solid transparent;
    border-radius: 6px;
  }

  .library .name {
    flex: 1;
    text-align: left;
  }

  .library input {
    width: 8em;
  }

  .section-editor {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }

  .section-editor input[type='number'] {
    width: 5em;
  }

  .strip :global(.pads) {
    grid-auto-columns: 132px;
    grid-auto-flow: column;
    grid-template-columns: none;
    padding-bottom: 4px;
    margin-bottom: 4px;
    overflow-x: auto;
  }

  .lanes {
    overflow-x: auto;
  }

  .lane {
    display: grid;
    grid-template-columns: var(--label) max-content;
    align-items: center;
    min-height: 32px;
  }

  .lane-name {
    overflow: hidden;
    padding-right: 8px;
    font-size: 13px;
    color: var(--muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .track {
    position: relative;
    height: 28px;
    background-color: var(--well);
    background-image:
      repeating-linear-gradient(to right, var(--line-strong) 0 1px, transparent 1px var(--bar)),
      repeating-linear-gradient(to right, var(--line) 0 1px, transparent 1px var(--beat));
    border-radius: 4px;
  }

  .ruler {
    height: 20px;
    background-image: repeating-linear-gradient(to right, var(--muted) 0 1px, transparent 1px var(--bar)),
      repeating-linear-gradient(to right, transparent 0 calc(100% - 6px), var(--line-strong) calc(100% - 6px));
    background-color: transparent;
  }

  .bar-number {
    position: absolute;
    top: 2px;
    padding-left: 4px;
    font: 11px var(--mono);
    color: var(--muted);
  }

  .block {
    position: absolute;
    top: 2px;
    bottom: 2px;
    display: flex;
    align-items: center;
    padding: 0 0 0 6px;
    overflow: hidden;
    color: var(--accent-ink);
    background: rgb(255 190 80 / calc(0.35 + 0.65 * var(--level)));
    border: 1px solid var(--accent);
    border-radius: 4px;
    touch-action: none;
    user-select: none;
    -webkit-user-select: none;
  }

  .block.selected {
    box-shadow: 0 0 0 2px var(--text);
  }

  .block.playing {
    box-shadow: 0 0 12px rgb(255 190 80 / 0.6);
  }

  .block-name {
    flex: 1;
    overflow: hidden;
    font-size: 12px;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .handle {
    align-self: stretch;
    width: 8px;
    cursor: ew-resize;
    background: rgb(0 0 0 / 0.2);
  }
</style>

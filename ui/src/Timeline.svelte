<script>
  import { request } from './engine.svelte.js';
  import { FULL_VELOCITY, holdToAudition } from './audition.js';
  import { beatsIn, beatsPerBar, DEFAULT_BPM, formatLength, parseLength } from './duration.js';
  import { PEAKS_PER_SECOND, Playback } from './playback.svelte.js';
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
  /** The narrowest beat on the section editor's ruler; a short section widens its beats to fill it. */
  const MIN_BEAT_PX = 24;
  /** The lane names' column, left of the ruler. */
  const LANE_LABEL_PX = 120;
  /** The overview: each placement's bar and name, over a thin lane per scene. */
  const OVERVIEW_LABEL_PX = 22;
  const OVERVIEW_LANE_PX = 6;
  const NEW_SECTION_BARS = 4;
  const FULL_LEVEL = 100;
  /** How strongly a block at 0% is drawn, so it still shows; 100% is drawn solid. */
  const DIMMEST_SHADE = 0.35;
  const TIME_SIGNATURE = /^[1-9][0-9]*\/[1-9][0-9]*$/;
  const WAVEFORM_PX = 40;

  let openName = $state(null);
  let newName = $state('');
  /** The selected block: its section, and its scene and rank there, as `findBlock` takes them. */
  let selected = $state(null);
  /** The block being moved or resized, as shown until it saves: its section, scene, rank, start and length. */
  let dragged = $state.raw(null);
  /** The press on a block, not reactive: the pointer, the block, how it moves, the audition. */
  let press = null;
  let editorWidth = $state(0);

  const songSignature = $derived(song.time_signature ?? '4/4');
  const sections = $derived(Object.entries(song.sections ?? {}).map(([name, file]) => sectionOf(name, file)));
  const byName = $derived(Object.fromEntries(sections.map((section) => [section.name, section])));
  const arrangement = $derived(song.arrangement ?? []);
  const openSection = $derived(byName[openName] ?? byName[arrangement[0]] ?? sections[0] ?? null);
  const selectedBlock = $derived(selected?.section === openSection?.name ? findBlock(openSection, selected) : undefined);
  const beatPx = $derived(openSection ? Math.max(MIN_BEAT_PX, Math.floor((editorWidth - LANE_LABEL_PX) / openSection.beats)) : MIN_BEAT_PX);
  const lanes = $derived(openSection ? scenesIn([openSection]) : []);
  /** Each placed section and the song's bar and beat, from 0, it starts on. */
  const placements = $derived.by(() => {
    let bar = 1;
    let startBeat = 0;
    return arrangement.map((name, index) => {
      const placed = { index, name, section: byName[name], bar, startBeat };
      bar += placed.section.bars;
      startBeat += placed.section.beats;
      return placed;
    });
  });
  const overviewLanes = $derived(scenesIn(placements.map(({ section }) => section)));
  const totalBars = $derived(placements.reduce((sum, { section }) => sum + section.bars, 0));
  const bpm = $derived(song.bpm ?? DEFAULT_BPM);
  const totalBeats = $derived(placements.reduce((sum, { section }) => sum + section.beats, 0));
  const totalSeconds = $derived((totalBeats * 60) / bpm);

  const songPath = $derived(`/api/songs/${encodeURIComponent(songName)}`);
  const playback = new Playback(sendMidi);
  $effect(() => () => playback.close());

  /**
   * Sends MIDI one message at a time, each once the one before is in, so the engine takes them
   * in order: parallel requests could land a note before its song. A failure stops playback.
   */
  let sending = Promise.resolve();
  function sendMidi(message) {
    sending = sending
      .then(() => request('POST', '/api/midi', { message }))
      .catch((failure) => {
        playback.pause();
        attempt(() => Promise.reject(failure));
      });
  }

  let hasEvents = $state(false);
  const canPlay = $derived(editable && hasEvents && totalSeconds > 0);
  /** Where bar 1 falls in the audio while the waveform is dragged, in seconds; null otherwise. */
  let draggedOffset = $state(null);
  /** The waveform drag, not reactive: the pointer, where it pressed and the offset then. */
  let offsetPress = null;
  let overviewWidth = $state(0);
  let waveform = $state();
  let overview = $state();
  const savedOffset = $derived(secondsOf(song.audio?.offset));
  const offset = $derived(draggedOffset ?? savedOffset);
  const playheadBeat = $derived((playback.position * bpm) / 60);
  const playingPlacement = $derived(placements.find((placed) => playheadBeat >= placed.startBeat && playheadBeat < placed.startBeat + placed.section.beats) ?? null);
  const audioFile = $derived(song.audio?.file ?? null);

  /** Counts event loads, so a slow one that a newer one replaced is dropped. */
  let eventLoads = 0;
  // After every save: an edit made while playing takes effect from the next event.
  $effect(() => {
    song;
    const load = ++eventLoads;
    const seconds = totalSeconds;
    request('GET', `${songPath}/arrangement/events`)
      .then((events) => {
        if (load !== eventLoads) return;
        playback.load(events, seconds);
        hasEvents = true;
      })
      .catch((failure) => {
        if (load === eventLoads) attempt(() => Promise.reject(failure));
      });
  });

  $effect(() => {
    playback.loadAudio(audioFile && `${songPath}/audio`).catch((failure) => attempt(() => Promise.reject(failure)));
  });

  $effect(() => {
    playback.offset = offset;
  });

  // Show mode doesn't play: the stage follows Reaper.
  $effect(() => {
    if (!editable) playback.pause();
  });

  $effect(() => {
    if (waveform && overviewWidth) drawWaveform(waveform, playback.audio?.peaks, offset);
  });

  function secondsOf(text) {
    const length = parseLength(text) ?? { count: 0, unit: 's' };
    return length.unit === 'ms' ? length.count / 1000 : length.count;
  }

  const togglePlayback = () => attempt(() => (playback.isPlaying ? playback.pause() : playback.play()));
  function seekToBeat(beat) {
    if (canPlay) playback.seek((beat * 60) / bpm);
  }

  /** Seeks to where the click landed on placement `placed` of the overview. */
  function seekInPlacement(event, placed) {
    const box = event.currentTarget.getBoundingClientRect();
    seekToBeat(placed.startBeat + clamp((event.clientX - box.left) / box.width, 0, 1) * placed.section.beats);
  }

  /** Seeks to where the click landed on the section editor's ruler: in the placement playing, or the section's first. */
  function seekInSection(event, section) {
    const placed = playingPlacement?.name === section.name ? playingPlacement : placements.find((other) => other.name === section.name);
    if (!placed) return;
    const beat = (event.clientX - event.currentTarget.getBoundingClientRect().left) / beatPx;
    seekToBeat(placed.startBeat + clamp(beat, 0, section.beats));
  }

  /** Space plays and pauses, unless typing or on a control, which Space presses. */
  function spaceBar(event) {
    const isOnControl = event.target.closest?.('input, textarea, select, button, a, [role="button"], [contenteditable]');
    if (event.key !== ' ' || event.repeat || !canPlay || isOnControl) return;
    event.preventDefault();
    togglePlayback();
  }

  function whereLabel() {
    if (!playingPlacement) return `End · ${minutes(playback.position)}`;
    const { bar, startBeat, section } = playingPlacement;
    return `Bar ${bar + Math.floor((playheadBeat - startBeat) / section.barBeats)} · ${minutes(playback.position)}`;
  }

  /** Each placement with where its beats lie on the overview, from its left edge, in CSS pixels. */
  function placementBoxes() {
    const buttons = overview.querySelectorAll('.placement');
    const overviewLeft = overview.getBoundingClientRect().left;
    return placements.map((placed, index) => {
      const button = buttons[index];
      return { ...placed, box: { left: button.getBoundingClientRect().left - overviewLeft + button.clientLeft, width: button.clientWidth } };
    });
  }

  /**
   * The audio's peaks under each placement of the overview, bar 1 at `offset` seconds into it,
   * one column per pixel: the gaps and borders between placements show none, so the audio stays
   * on its bars.
   */
  function drawWaveform(canvas, peaks, offset) {
    const context = canvas.getContext('2d');
    const { height } = canvas;
    context.clearRect(0, 0, canvas.width, height);
    if (!peaks || !totalBeats) return;
    context.fillStyle = getComputedStyle(canvas).color;
    const scale = canvas.width / overviewWidth;
    for (const { startBeat, section, box } of placementBoxes()) {
      const left = Math.round(box.left * scale);
      const width = Math.round(box.width * scale);
      const beatWidth = width / section.beats;
      const peakAt = (x) => Math.floor((offset + ((startBeat + x / beatWidth) * 60) / bpm) * PEAKS_PER_SECOND);
      for (let x = 0; x < width; x += 1) {
        const loudest = Math.max(0, ...peaks.subarray(peakAt(x), Math.max(peakAt(x) + 1, peakAt(x + 1))));
        const bar = Math.max(1, loudest * height);
        context.fillRect(left + x, (height - bar) / 2, 1, bar);
      }
    }
  }

  function pressWaveform(event) {
    if (!editable || !playback.audio || event.button !== 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const [{ section, box }] = placementBoxes();
    offsetPress = { pointer: event.pointerId, x: event.clientX, from: offset, pxPerBeat: box.width / section.beats };
  }

  /** Dragging the waveform right moves the audio later against the bars: bar 1 falls earlier in it. */
  function dragWaveform(event) {
    if (offsetPress?.pointer !== event.pointerId) return;
    const seconds = ((event.clientX - offsetPress.x) / offsetPress.pxPerBeat) * (60 / bpm);
    draggedOffset = Math.max(0, offsetPress.from - seconds);
  }

  function cancelWaveformDrag(event) {
    if (offsetPress?.pointer !== event.pointerId) return;
    offsetPress = null;
    draggedOffset = null;
  }

  async function dropWaveform(event) {
    if (offsetPress?.pointer !== event.pointerId) return;
    offsetPress = null;
    const seconds = Math.round((draggedOffset ?? savedOffset) * 1000) / 1000;
    if (seconds !== savedOffset) {
      await edit(async () => {
        await request('PUT', `${songPath}/audio/offset`, { offset: formatLength(seconds, 's') });
        await onsaved(`Bar 1 now falls ${seconds} s into the audio of ${songName}. Undo takes it back.`);
      });
    }
    draggedOffset = null;
  }

  /** A section of the file, its blocks in beats from its first beat, in playing order. */
  function sectionOf(name, { bars, time_signature, blocks = [] }) {
    const barBeats = beatsPerBar(time_signature ?? songSignature);
    const inBeats = blocks.map((block) => blockOf(block, barBeats)).sort(inPlayingOrder);
    return { name, bars, time_signature, barBeats, beats: bars * barBeats, blocks: ranked(inBeats) };
  }

  /** A block of the file in beats from its section's first beat, in bars of `barBeats` beats. */
  function blockOf({ scene, bar, beat = 1, length, level }, barBeats) {
    return { scene, start: (bar - 1) * barBeats + beat - 1, length: beatsIn(length, barBeats), level: level ? parseFloat(level) : FULL_LEVEL };
  }

  /**
   * Each block with `nth`, its rank among the blocks of its scene. Blocks of one scene never
   * overlap, so a move keeps the rank: it names the block across saves, as a key and a selection.
   */
  function ranked(blocks) {
    const counts = {};
    return blocks.map((block) => {
      const nth = counts[block.scene] ?? 0;
      counts[block.scene] = nth + 1;
      return { ...block, nth };
    });
  }

  const inPlayingOrder = (a, b) => a.start - b.start || a.scene - b.scene;
  const scenesIn = (placed) => [...new Set(placed.flatMap((section) => section.blocks.map((block) => block.scene)))].sort((a, b) => a - b);
  const clamp = (value, lowest, highest) => Math.min(Math.max(value, lowest), highest);
  const findBlock = (section, { scene, nth }) => section?.blocks.find((block) => block.scene === scene && block.nth === nth);
  const isDragged = (section, block) => dragged?.section === section.name && dragged.scene === block.scene && dragged.nth === block.nth;
  const shownOf = (section, block) => (isDragged(section, block) ? dragged : block);

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
  const velocityOf = (level) => Math.max(1, Math.round((level / FULL_LEVEL) * FULL_VELOCITY));
  const shadeOf = (level) => DIMMEST_SHADE + ((1 - DIMMEST_SHADE) * level) / FULL_LEVEL;

  function describe(block, section) {
    const { bar, beat = 1, length } = blockFile(block, section.barBeats);
    return `${sceneName(block.scene)}, scene ${block.scene}: bar ${bar} beat ${beat}, ${length}, ${block.level}%`;
  }

  function minutes(seconds) {
    const whole = Math.round(seconds);
    return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
  }

  const timelineFile = (nextSections, nextArrangement) => ({
    sections: Object.fromEntries(nextSections.map((section) => [section.name, sectionFile(section)])),
    arrangement: [...nextArrangement],
  });

  /** Saves the timeline as given; resolves false, saving nothing, when the file holds that already. */
  async function save(nextSections, nextArrangement = arrangement) {
    const next = timelineFile(nextSections, nextArrangement);
    if (JSON.stringify(next) === JSON.stringify(timelineFile(sections, arrangement))) return false;
    await request('PUT', `/api/songs/${encodeURIComponent(songName)}/arrangement`, next);
    await onsaved(`Saved the timeline of ${songName}. Undo takes it back.`);
    return true;
  }

  /**
   * Runs `action(...names)` once the edit before it has saved and the song reloaded, so that
   * each edit starts from what the file holds. `names` are taken now, as the user meant them:
   * sections, blocks and placements by name, found again when the edit runs.
   */
  let lastEdit = Promise.resolve();
  function edit(action, ...names) {
    lastEdit = lastEdit.catch(() => {}).then(() => action(...names));
    return attempt(() => lastEdit);
  }

  const withSection = (name, change) => sections.map((section) => (section.name === name ? { ...section, ...change } : section));

  /** Saves what a field was changed to with `action(...names, value)`, or puts back what it showed when that is refused. */
  function change(event, shown, action, ...names) {
    const field = event.currentTarget;
    const value = field.value.trim();
    edit(() =>
      action(...names, value).catch((failure) => {
        field.value = shown;
        throw failure;
      }),
    );
  }

  const setBars = (name, text) => save(withSection(name, { bars: Number(text) }));

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

  /**
   * Gives section `name` the time signature `text`, or the song's for an empty one. Its blocks
   * keep their bar, beat and length in bars, so a two-bar block stays two bars; one that no
   * longer fits is refused, naming it.
   */
  async function setTimeSignature(name, text) {
    if (text && !TIME_SIGNATURE.test(text)) throw new Error(`"${text}" is not a time signature: write beats over the note value, as in 3/4 or 6/8.`);
    const section = byName[name];
    const time_signature = text || undefined;
    const signature = time_signature ?? songSignature;
    const barBeats = beatsPerBar(signature);
    const blocks = section.blocks.map((block) => {
      const file = blockFile(block, section.barBeats);
      const { start, length } = blockOf(file, barBeats);
      if ((file.beat ?? 1) >= barBeats + 1 || start + length > section.bars * barBeats) {
        throw new Error(
          `${sceneName(block.scene)} (scene ${block.scene}) at bar ${file.bar} beat ${file.beat ?? 1} doesn't fit in ${section.bars} bars of ${signature}: move or shorten it first.`,
        );
      }
      return { ...block, start, length };
    });
    await save(withSection(name, { time_signature, blocks }));
  }

  const place = (name) => save(sections, [...arrangement, name]);
  /** Takes placement `index` out, if `name` is still there: an earlier edit may have moved it. */
  const unplace = (index, name) => arrangement[index] === name && save(sections, arrangement.toSpliced(index, 1));

  /** Moves placement `from` to `to`, if both still hold the sections they did when dragged. */
  function movePlacement(from, to) {
    if (from.index === to.index || arrangement[from.index] !== from.name || arrangement[to.index] !== to.name) return;
    return save(sections, arrangement.toSpliced(from.index, 1).toSpliced(to.index, 0, from.name));
  }

  const withBlocks = (name, change) => save(withSection(name, { blocks: change(byName[name].blocks) }));

  async function deleteBlock(name, ref) {
    const block = findBlock(byName[name], ref);
    if (!block) return;
    await withBlocks(name, (blocks) => blocks.filter((other) => other !== block));
    if (selected?.section === name && selected.scene === ref.scene && selected.nth === ref.nth) selected = null;
  }

  /** Changes the block `ref` names, as it is when the edit runs, by what `change(block, section)` returns. */
  async function changeBlock(name, ref, change) {
    const section = byName[name];
    const block = findBlock(section, ref);
    if (!block) return;
    const changed = { ...block, ...change(block, section) };
    await withBlocks(name, (blocks) => blocks.map((other) => (other === block ? changed : other)));
  }

  /** A block moved or resized by `beats`, a whole beat at a time, inside its section. */
  const GESTURES = {
    move: (block, section, beats) => ({ start: clamp(block.start + beats, 0, section.beats - block.length), length: block.length }),
    resize: (block, section, beats) => ({ start: block.start, length: clamp(block.length + beats, Math.min(1, block.length), section.beats - block.start) }),
  };

  /** A scene dropped on the section: a bar long from the beat it landed on, or to the end. */
  function dropScene(event, section) {
    if (!editable || !event.dataTransfer.types.includes(SCENE_DRAG_TYPE)) return;
    event.preventDefault();
    const scene = Number(event.dataTransfer.getData(SCENE_DRAG_TYPE));
    const beat = Math.floor((event.clientX - event.currentTarget.getBoundingClientRect().left) / beatPx);
    const start = clamp(beat, 0, Math.ceil(section.beats) - 1);
    const placed = { scene, start, length: Math.min(section.barBeats, section.beats - start), level: FULL_LEVEL };
    const { name } = section;
    edit(async () => {
      await withBlocks(name, (blocks) => [...blocks, placed]);
      const added = byName[name].blocks.find((block) => block.scene === scene && block.start === start);
      selected = { section: name, scene, nth: added.nth };
    });
  }

  const accepts = (event, type) => editable && event.dataTransfer.types.includes(type) && event.preventDefault();

  function dropPlacement(event, to) {
    if (!editable || !event.dataTransfer.types.includes(PLACEMENT_DRAG_TYPE)) return;
    event.preventDefault();
    const from = JSON.parse(event.dataTransfer.getData(PLACEMENT_DRAG_TYPE));
    edit(() => movePlacement(from, to));
  }

  /** `mode` is `move` for the block's body, `resize` for its end. */
  function pressBlock(event, section, block, mode) {
    if (!editable || press !== null || event.button !== 0) return;
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    selected = { section: section.name, scene: block.scene, nth: block.nth };
    dragged = null;
    const hold = holdToAudition({ isPlaying: () => isPlaying(block.scene), onhold: () => onhold(block.scene, velocityOf(block.level)) });
    press = { pointer: event.pointerId, x: event.clientX, section, block, mode, hold };
  }

  function slide(event) {
    if (press?.pointer !== event.pointerId) return;
    const { x, section, block, mode, hold } = press;
    const { start, length } = GESTURES[mode](block, section, Math.round((event.clientX - x) / beatPx));
    if (!dragged && start === block.start && length === block.length) return;
    hold.cancel();
    dragged = { section: section.name, scene: block.scene, nth: block.nth, start, length };
  }

  async function letGo(event) {
    if (press?.pointer !== event.pointerId) return;
    const { section, block, hold } = press;
    press = null;
    hold.release(() => onrelease(block.scene));
    const moved = dragged;
    if (!moved || (moved.start === block.start && moved.length === block.length)) {
      dragged = null;
      return;
    }
    // Shown where it was dropped until the save reloads, unless a new press has started since.
    await edit(() => changeBlock(section.name, block, () => ({ start: moved.start, length: moved.length })));
    if (dragged === moved) dragged = null;
  }

  /** Delete removes the focused block; ← and → move it a beat, with Shift they resize it. */
  function blockKey(event, section, block) {
    const step = { ArrowLeft: -1, ArrowRight: 1 }[event.key];
    if (!editable || (!step && event.key !== 'Delete' && event.key !== 'Backspace')) return;
    event.preventDefault();
    const ref = { scene: block.scene, nth: block.nth };
    const gesture = GESTURES[event.shiftKey ? 'resize' : 'move'];
    if (!step) edit(() => deleteBlock(section.name, ref));
    else edit(() => changeBlock(section.name, ref, (current, inSection) => gesture(current, inSection, step)));
  }

  async function setLevel(name, ref, text) {
    if (text === '') throw new Error('Give the level as a percentage, from 0 to 100.');
    await changeBlock(name, ref, () => ({ level: Number(text) }));
  }
</script>

<svelte:window onkeydown={spaceBar} />

<section class="arrangement" aria-label="Arrangement">
  <div class="heading">
    <h2>Arrangement</h2>
    <span class="hint" data-testid="song-length">{totalBars} bars · {minutes(totalSeconds)}</span>
  </div>
  {#if placements.length}
    <div class="row transport">
      <button class="quiet" aria-label="Back to start" title="Back to start" disabled={!canPlay} onclick={() => playback.seek(0)}>⏮</button>
      <button class="primary" title="Space" disabled={!canPlay} onclick={togglePlayback}>{playback.isPlaying ? 'Pause' : 'Play'}</button>
      <span class="where" data-testid="playhead">{whereLabel()}</span>
      <span class="hint" data-testid="audio">
        {#if !audioFile}
          No audio for this song: it plays with a click. To play its bounce, put the file in the show's audio/ folder and name it in the song file.
        {:else if playback.isAudioMissing}
          No file at {audioFile}: the song plays silent, with a click.
        {:else if !playback.audio}
          Loading {audioFile}…
        {:else}
          Bar 1 falls {Math.round(offset * 1000) / 1000} s into {audioFile}{editable ? ': drag the waveform to move it against the bars.' : '.'}
        {/if}
      </span>
    </div>

    <ol class="tiles">
      {#each placements as placed (placed.index)}
        <li
          class="tile"
          class:open={placed.name === openSection?.name}
          draggable={editable}
          ondragstart={(event) => event.dataTransfer.setData(PLACEMENT_DRAG_TYPE, JSON.stringify({ index: placed.index, name: placed.name }))}
          ondragover={(event) => accepts(event, PLACEMENT_DRAG_TYPE)}
          ondrop={(event) => dropPlacement(event, { index: placed.index, name: placed.name })}
        >
          <button class="quiet" aria-label="{placed.name} from bar {placed.bar}" onclick={() => (openName = placed.name)}>
            {placed.name} <span class="hint">{placed.section.bars} bars</span>
          </button>
          <button class="quiet" aria-label="Take {placed.name} at bar {placed.bar} out of the arrangement" disabled={!editable} onclick={() => edit(unplace, placed.index, placed.name)}>×</button>
        </li>
      {/each}
    </ol>

    <div
      class="overview"
      role="group"
      aria-label="Overview"
      style:--lanes={overviewLanes.length}
      style:--overview-label="{OVERVIEW_LABEL_PX}px"
      style:--overview-lane="{OVERVIEW_LANE_PX}px"
      bind:clientWidth={overviewWidth}
      bind:this={overview}
    >
      {#each placements as placed (placed.index)}
        <button
          class="placement"
          class:open={placed.name === openSection?.name}
          style:flex-grow={placed.section.beats}
          data-placement={placed.name}
          onclick={(event) => {
            openName = placed.name;
            seekInPlacement(event, placed);
          }}
        >
          <span class="at">{placed.bar} · {placed.name}</span>
          {#if placed.index === playingPlacement?.index}
            <span class="playhead" style:left="{((playheadBeat - placed.startBeat) / placed.section.beats) * 100}%"></span>
          {/if}
          {#each placed.section.blocks as block (`${block.scene}:${block.nth}`)}
            <span
              class="mini"
              title={describe(block, placed.section)}
              style:left="{(block.start / placed.section.beats) * 100}%"
              style:width="{(block.length / placed.section.beats) * 100}%"
              style:--row={overviewLanes.indexOf(block.scene)}
              style:opacity={shadeOf(block.level)}
            ></span>
          {/each}
        </button>
      {/each}
    </div>
    {#if playback.audio}
      <canvas
        class="waveform"
        class:draggable={editable}
        aria-label="Waveform"
        bind:this={waveform}
        width={overviewWidth * devicePixelRatio}
        height={WAVEFORM_PX * devicePixelRatio}
        onpointerdown={pressWaveform}
        onpointermove={dragWaveform}
        onpointerup={dropWaveform}
        onpointercancel={cancelWaveformDrag}
      ></canvas>
    {/if}
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
          <button class="quiet" aria-label="Add {section.name} to the arrangement" disabled={!editable} onclick={() => edit(place, section.name)}>+</button>
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
          <label>Name <input value={section.name} onchange={(event) => change(event, section.name, renameSection, section.name)} /></label>
          <label>
            Bars
            <input type="number" min="1" value={section.bars} onchange={(event) => change(event, section.bars, setBars, section.name)} />
          </label>
          <label>
            Time signature
            <input
              value={section.time_signature ?? ''}
              placeholder={songSignature}
              onchange={(event) => change(event, section.time_signature ?? '', setTimeSignature, section.name)}
            />
          </label>
        {/key}
        <button onclick={() => edit(duplicateSection, section.name)}>Duplicate</button>
        <button class="danger" onclick={() => edit(deleteSection, section.name)}>Delete</button>
      </fieldset>

      <div class="strip">
        <ScenePads {songName} cues={song.cues} tempo={song.bpm} {editable} {isPlaying} draggable={editable} onopen={onopenscene} onhold={(scene) => onhold(scene, FULL_VELOCITY)} {onrelease} {onaudition} />
      </div>

      <div class="lanes" style:--beat="{beatPx}px" style:--bar="{beatPx * section.barBeats}px" style:--label="{LANE_LABEL_PX}px">
        {#if playingPlacement?.name === section.name}
          <span class="playhead" style:left="{LANE_LABEL_PX + (playheadBeat - playingPlacement.startBeat) * beatPx}px"></span>
        {/if}
        <div class="lane">
          <span></span>
          <!-- Seeking by keyboard goes through the overview's buttons. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div class="ruler track" class:seekable={canPlay} style:width="{section.beats * beatPx}px" data-testid="ruler" role="presentation" onclick={(event) => seekInSection(event, section)}>
            {#each { length: section.bars } as _, bar (bar)}
              <span class="bar-number" style:left="{bar * section.barBeats * beatPx}px">{bar + 1}</span>
            {/each}
          </div>
        </div>
        {#each [...lanes, null] as scene (scene)}
          <div class="lane" data-lane={scene ?? 'new'}>
            <span class="lane-name">{scene === null ? 'Drop a scene here' : `${scene} · ${sceneName(scene)}`}</span>
            <div class="track" style:width="{section.beats * beatPx}px" role="presentation" ondragover={(event) => accepts(event, SCENE_DRAG_TYPE)} ondrop={(event) => dropScene(event, section)}>
              {#each section.blocks.filter((block) => block.scene === scene) as block (block.nth)}
                {@const shown = shownOf(section, block)}
                <button
                  class="block"
                  class:selected={block === selectedBlock}
                  class:playing={isPlaying(block.scene)}
                  data-block={block.scene}
                  aria-label={describe(block, section)}
                  style:left="{shown.start * beatPx}px"
                  style:width="{shown.length * beatPx}px"
                  style:--shade={shadeOf(block.level)}
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
            <label>Level <input type="number" min="0" max="100" value={block.level} onchange={(event) => change(event, block.level, setLevel, section.name, { scene: block.scene, nth: block.nth })} /></label>
            <span class="hint">%</span>
          {/key}
          <button class="danger" onclick={() => edit(deleteBlock, section.name, { scene: block.scene, nth: block.nth })}>Delete block</button>
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
    height: calc(var(--overview-label) + (var(--lanes) + 1) * var(--overview-lane));
    padding: 0;
    overflow: hidden;
    text-align: left;
    background: var(--well);
    border-radius: 4px;
  }

  .at {
    display: block;
    box-sizing: border-box;
    height: var(--overview-label);
    padding: 2px 6px;
    overflow: hidden;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }

  .mini {
    position: absolute;
    top: calc(var(--overview-label) + var(--row) * var(--overview-lane));
    height: calc(var(--overview-lane) - 1px);
    background: var(--accent);
    border-radius: 2px;
  }

  .transport {
    margin-bottom: 8px;
  }

  .where {
    min-width: 9em;
    font: 13px var(--mono);
  }

  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 1;
    width: 2px;
    margin-left: -1px;
    pointer-events: none;
    background: var(--text);
  }

  .waveform {
    display: block;
    width: 100%;
    height: 40px;
    margin: -16px 0 20px;
    color: var(--muted);
    background: var(--well);
    border-radius: 4px;
  }

  .waveform.draggable {
    cursor: ew-resize;
    touch-action: none;
  }

  .ruler.seekable {
    cursor: pointer;
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
    position: relative;
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
    background: rgb(255 190 80 / var(--shade));
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

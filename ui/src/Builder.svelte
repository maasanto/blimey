<script module>
  // Kept while other pages are open, so the look stays live on the output while you check it
  // on the front view, and is still here when you come back.
  let songName = $state('');
  /** The look being built, in order: a later entry wins on the fixtures they share. */
  let look = $state([]);
  /** The look as it was when an audition took it off the output, until it is edited again. */
  let pausedLook = $state(null);
  /**
   * The cue being edited, as in the song file, and its note. `replaces` is the cue as loaded,
   * `{ note, cue }`, so the engine can refuse the save if the file changed since.
   */
  let form = $state(newForm());

  function newForm() {
    return { note: null, replaces: null, cue: { name: '', type: 'scene' } };
  }
</script>

<script>
  import { engine, request } from './engine.svelte.js';
  import FrontView from './FrontView.svelte';

  // Channel 2, where cues and song select live (docs/api.md, Playback).
  const NOTE_ON = 0x91;
  const NOTE_OFF = 0x81;
  const PROGRAM_CHANGE = 0xc1;
  const FULL_VELOCITY = 127;
  const DEFAULT_BPM = 120;
  /** Cue types with a `fade_in` (schema/song.schema.json). */
  const FADE_IN_TYPES = ['scene', 'pixel'];
  /** Well inside the engine's 5 s preview lifetime (docs/api.md), so the look never flickers off. */
  const PREVIEW_REFRESH_MS = 2000;

  let error = $state('');
  let notice = $state('');
  /** `GET /api/show`: palettes and songs. */
  let summary = $state(null);
  /** The selected song file, as JSON. */
  let song = $state(null);
  let tempo = $state(DEFAULT_BPM);
  let adding = $state('');
  /** `GET /api/history`: how many saves can be undone and redone. */
  let steps = $state({ undo: 0, redo: 0 });
  /** The new-song form while it is open. */
  let newSong = $state(null);

  const editable = $derived(engine.connected && engine.rig?.mode === 'edit');
  const targets = $derived(engine.rig ? [...Object.keys(engine.rig.groups), ...engine.rig.fixtures.map((fixture) => fixture.name)] : []);
  const values = $derived(Object.fromEntries($state.snapshot(look).map(({ target, settings }) => [target, settings])));
  const isPaused = $derived(JSON.stringify(values) === pausedLook);
  const cues = $derived(Object.entries(song?.cues ?? {}).map(([note, cue]) => ({ note: Number(note), cue })));

  async function attempt(action) {
    error = '';
    notice = '';
    try {
      await action();
    } catch (failure) {
      error = failure.message;
    }
  }

  const songFile = () => `/api/show/songs/${songName}.yaml`;

  async function loadSong() {
    song = songName ? await request('GET', songFile()) : null;
    tempo = song?.bpm ?? DEFAULT_BPM;
  }

  async function loadSummary() {
    summary = await request('GET', '/api/show');
    if (!summary.songs.some((entry) => entry.name === songName)) songName = summary.songs[0]?.name ?? '';
  }

  async function loadSteps() {
    steps = await request('GET', '/api/history');
  }

  attempt(loadSummary);

  // The front view beside the builder saves `rig.yaml` too; every saved rig comes back on the
  // stream, so the step counts follow it.
  $effect(() => {
    if (engine.rig) loadSteps().catch((failure) => (error = failure.message));
  });

  /** After a save: the song as written, and one more step to undo. */
  const reloadAfterSave = () => Promise.all([loadSong(), loadSteps()]);

  // Not through `attempt`: switching songs after a save or a new song keeps its notice.
  $effect(() => {
    songName;
    loadSong().catch((failure) => (error = failure.message));
  });

  // One preview request at a time, always the latest look, so the output never steps back.
  let queuedPreview = null;
  let previewing = false;
  async function preview(next) {
    queuedPreview = next;
    if (previewing) return;
    previewing = true;
    while (queuedPreview) {
      const values = queuedPreview;
      queuedPreview = null;
      try {
        await (Object.keys(values).length ? request('PUT', '/api/preview', values) : request('DELETE', '/api/preview'));
      } catch (failure) {
        error = failure.message;
      }
    }
    previewing = false;
  }

  // The engine drops a preview it hasn't heard of for a while, so a closed or crashed builder
  // leaves no look on stage: keep this one alive while it is shown.
  $effect(() => {
    if (!editable || isPaused) return;
    const current = values;
    preview(current);
    if (!Object.keys(current).length) return;
    const heartbeat = setInterval(() => preview(current), PREVIEW_REFRESH_MS);
    return () => clearInterval(heartbeat);
  });

  /** Takes the look off the output when the browser page goes; nobody is left to hear back. */
  const clearPreview = () => editable && fetch('/api/preview', { method: 'DELETE', keepalive: true });

  const hex = (color) => summary?.palettes[color] ?? (color?.startsWith('#') ? color : '#000000');

  function add() {
    look.push({ target: adding, settings: { color: '#ffffff', dimmer: '100%' } });
    adding = '';
  }

  function edit({ note, cue }) {
    form = { note, replaces: { note, cue: $state.snapshot(cue) }, cue: $state.snapshot(cue) };
    if (cue.type === 'scene') {
      look = Object.entries($state.snapshot(cue.values)).map(([target, settings]) => ({ target, settings }));
    }
  }

  async function saveCue() {
    const cue = $state.snapshot(form.cue);
    for (const optional of ['fade_in', 'release']) if (!cue[optional]) delete cue[optional];
    if (cue.type === 'scene') cue.values = values;
    await request('PUT', `/api/songs/${songName}/cues/${form.note}`, { cue, replaces: form.replaces });
    notice = `Saved cue ${form.note} in ${songName}.`;
    await reloadAfterSave();
    form.replaces = { note: form.note, cue: $state.snapshot(song.cues[form.note]) };
  }

  async function deleteCue(note) {
    await request('DELETE', `/api/songs/${songName}/cues/${note}`);
    if (form.replaces?.note === note) form = newForm();
    notice = `Deleted cue ${note}. Undo brings it back.`;
    await reloadAfterSave();
  }

  async function setTempo() {
    await request('PUT', `/api/songs/${songName}/bpm`, { bpm: tempo });
    notice = `${songName} runs at ${tempo} BPM when no MIDI clock comes in.`;
    await reloadAfterSave();
  }

  /** `undo` or `redo` one save; either may add or remove a song. */
  async function step(action) {
    const { file, undo, redo } = await request('POST', `/api/${action}`);
    steps = { undo, redo };
    notice = action === 'undo' ? `Undid the last save of ${file}.` : `Redid the save of ${file}.`;
    // The cue being edited, or its whole song, may have changed or gone.
    form = newForm();
    const shown = songName;
    await loadSummary();
    // A song that went is replaced in the picker, and the switch loads the new one.
    if (songName === shown) await loadSong();
  }

  function openNewSong() {
    const used = new Set(summary?.songs.map((entry) => entry.program));
    let program = 0;
    while (used.has(program)) program += 1;
    newSong = { name: '', program, bpm: null, time_signature: '' };
  }

  async function createSong() {
    const { name, program, bpm, time_signature } = $state.snapshot(newSong);
    await request('POST', '/api/songs', {
      name,
      program,
      ...(bpm ? { bpm } : {}),
      ...(time_signature ? { time_signature } : {}),
    });
    newSong = null;
    form = newForm();
    songName = name;
    notice = `Created song ${name}. Undo removes it.`;
    await Promise.all([loadSummary(), loadSteps()]);
  }

  const isPlaying = (note) =>
    engine.frame?.song === songName && engine.frame.cues.some((cue) => cue.note === note && !cue.released);

  /** Fires or releases a saved cue through the engine, as Reaper would. */
  async function audition(note) {
    const midi = (message) => request('POST', '/api/midi', { message });
    if (isPlaying(note)) return midi([NOTE_OFF, note, 0]);
    if (engine.frame?.song !== songName) {
      await midi([PROGRAM_CHANGE, summary.songs.find((entry) => entry.name === songName).program]);
    }
    // The engine takes the look off the output so the cue shows; editing the look brings it back.
    if (Object.keys(values).length) pausedLook = JSON.stringify(values);
    await midi([NOTE_ON, note, FULL_VELOCITY]);
  }
</script>

<svelte:window onpagehide={clearPreview} />

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" aria-live="polite">{notice}</p>{/if}
{#if engine.rig && !editable}
  <p class="hint">
    Show mode: the builder is read-only. Start the engine with <code>--edit</code> to build looks, save cues and audition them.
  </p>
{/if}

<div class="bar">
  <label>
    Song
    <select
      bind:value={songName}
      onchange={() => {
        form = newForm();
        error = '';
      }}
    >
      {#each summary?.songs ?? [] as entry (entry.name)}
        <option value={entry.name}>{entry.name} (program {entry.program})</option>
      {/each}
    </select>
  </label>
  <button disabled={!editable || newSong !== null} onclick={openNewSong}>New song</button>
  <fieldset disabled={!editable || !song}>
    <label>
      Metronome
      <input type="number" min="1" step="any" bind:value={tempo} />
      BPM
    </label>
    <button onclick={() => attempt(setTempo)}>Set tempo</button>
  </fieldset>
  <span class="history">
    <button disabled={!editable || !steps.undo} onclick={() => attempt(() => step('undo'))}>Undo ({steps.undo})</button>
    <button disabled={!editable || !steps.redo} onclick={() => attempt(() => step('redo'))}>Redo ({steps.redo})</button>
  </span>
</div>
{#if newSong}
  <fieldset class="new-song" disabled={!editable}>
    <legend>New song</legend>
    <div class="row">
      <label>Song name <input bind:value={newSong.name} placeholder="encore" /></label>
      <label>Program <input type="number" min="0" max="127" bind:value={newSong.program} /></label>
      <label>Tempo <input type="number" min="1" step="any" bind:value={newSong.bpm} placeholder="120" /> BPM</label>
      <label>Time signature <input bind:value={newSong.time_signature} placeholder="4/4" /></label>
    </div>
    <p class="hint">
      The name is letters, digits, - and _. The program is the channel-2 Program Change that selects the song, one per song.
    </p>
    <div class="row">
      <button disabled={!newSong.name || newSong.program == null} onclick={() => attempt(createSong)}>Create song</button>
      <button onclick={() => (newSong = null)}>Cancel</button>
    </div>
  </fieldset>
{:else if summary && !summary.songs.length}
  <p class="hint">This show has no songs yet: create one with New song to save cues.</p>
{/if}

<div class="workspace">
  <div>
    <fieldset class="look" disabled={!editable}>
      <legend>Look</legend>
      {#each look as entry, index (entry.target)}
        <div class="entry" data-target={entry.target}>
          <span class="target">
            {entry.target}
            {#if !entry.settings.color}<small class="hint">colour from below</small>{/if}
          </span>
          <div class="swatches">
            {#each Object.entries(summary?.palettes ?? {}) as [name, colour] (name)}
              <button
                class="swatch"
                style:background={colour}
                title={name}
                aria-label={name}
                aria-pressed={entry.settings.color === name}
                onclick={() => (entry.settings.color = name)}
              ></button>
            {/each}
          </div>
          <input
            type="color"
            aria-label="{entry.target} colour"
            value={hex(entry.settings.color)}
            oninput={(event) => (entry.settings.color = event.currentTarget.value)}
          />
          <label class="dimmer">
            <input
              type="range"
              min="0"
              max="100"
              aria-label="{entry.target} dimmer"
              value={parseFloat(entry.settings.dimmer ?? '100')}
              oninput={(event) => (entry.settings.dimmer = `${event.currentTarget.value}%`)}
            />
            <output>{entry.settings.dimmer ?? 'unset'}</output>
          </label>
          <button aria-label="Remove {entry.target}" onclick={() => look.splice(index, 1)}>×</button>
        </div>
      {:else}
        <p class="hint">Add a fixture or group to start a look. It shows on the front view and the lights as you edit.</p>
      {/each}
      {#if look.length && isPaused}
        <p class="hint">Off the output while a cue is auditioned. Change the look to show it again.</p>
      {/if}
      <div class="row">
        <select bind:value={adding} aria-label="Fixture or group to add">
          <option value="">Fixture or group…</option>
          {#each targets.filter((target) => !look.some((entry) => entry.target === target)) as target (target)}
            <option>{target}</option>
          {/each}
        </select>
        <button disabled={!adding} onclick={add}>Add</button>
        <button disabled={!look.length} onclick={() => (look = [])}>Clear look</button>
      </div>
    </fieldset>

    <fieldset class="cue" disabled={!editable || !song}>
      <legend>{form.replaces === null ? 'New cue' : `Cue ${form.replaces.note}`}</legend>
      <div class="row">
        <label>Note <input type="number" min="0" max="127" bind:value={form.note} /></label>
        <label>Name <input bind:value={form.cue.name} /></label>
        <span>Type: {form.cue.type}</span>
      </div>
      <div class="row">
        {#if FADE_IN_TYPES.includes(form.cue.type)}
          <label>Fade in <input bind:value={form.cue.fade_in} placeholder="0s" /></label>
        {/if}
        <label>Release <input bind:value={form.cue.release} placeholder="0s" /></label>
      </div>
      <p class="hint">
        {#if form.cue.type === 'scene'}
          Saves the look above as this scene. Times take ms, s, beats or bars.
        {:else}
          Keeps the rest of this {form.cue.type} cue as it is; the look above is for scenes.
        {/if}
      </p>
      <div class="row">
        <button disabled={form.note == null || !form.cue.name} onclick={() => attempt(saveCue)}>Save cue</button>
        <button onclick={() => (form = newForm())}>New cue</button>
      </div>
    </fieldset>
  </div>

  <div><FrontView /></div>
</div>

{#if song}
  <h2>Cues in {songName}</h2>
  <table>
    <thead>
      <tr><th>Note</th><th>Name</th><th>Type</th><th>Fade in</th><th>Release</th><th></th></tr>
    </thead>
    <tbody>
      {#each cues as { note, cue } (note)}
        <tr data-note={note} aria-current={form.replaces?.note === note || undefined}>
          <td>{note}</td>
          <td>{cue.name}</td>
          <td>{cue.type}</td>
          <td>{cue.fade_in ?? '—'}</td>
          <td>{cue.release ?? '—'}</td>
          <td class="actions">
            <button disabled={!editable} onclick={() => edit({ note, cue })}>Edit</button>
            <button disabled={!editable} onclick={() => attempt(() => audition(note))}>
              {isPlaying(note) ? 'Release' : 'Audition'}
            </button>
            <button disabled={!editable} onclick={() => attempt(() => deleteCue(note))}>Delete</button>
          </td>
        </tr>
      {:else}
        <tr><td colspan="6" class="hint">No cues yet.</td></tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  fieldset {
    margin: 0;
    padding: 0;
    border: 0;
  }

  .bar,
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: center;
  }

  .bar {
    margin-bottom: 16px;
  }

  .bar fieldset {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .history {
    display: flex;
    gap: 8px;
    margin-left: auto;
  }

  .workspace {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
    gap: 20px;
    align-items: start;
  }

  .look,
  .cue,
  .new-song {
    padding: 12px;
    margin-bottom: 16px;
    border: 1px solid var(--line);
    border-radius: 8px;
  }

  legend {
    padding: 0 6px;
    color: var(--muted);
  }

  .entry {
    display: grid;
    grid-template-columns: 7em minmax(0, 1fr) auto;
    gap: 6px 10px;
    align-items: center;
    padding: 8px 0;
    border-bottom: 1px solid var(--line);
  }

  .target {
    font-weight: 600;
  }

  .target small {
    display: block;
    font-weight: normal;
  }

  .swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    grid-column: 2;
  }

  .swatch {
    width: 22px;
    height: 22px;
    padding: 0;
    border-radius: 50%;
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

  .dimmer {
    display: flex;
    gap: 8px;
    align-items: center;
    grid-column: 2;
  }

  .dimmer input {
    flex: 1;
  }

  .dimmer output {
    min-width: 4em;
    font: 13px var(--mono);
  }

  .look .row {
    margin-top: 10px;
  }

  .cue .row + .row {
    margin-top: 8px;
  }

  input[type='number'] {
    width: 5em;
  }

  .notice {
    color: var(--accent);
  }

  table {
    border-collapse: collapse;
    width: 100%;
  }

  th,
  td {
    padding: 6px 10px;
    text-align: left;
    border-bottom: 1px solid var(--line);
  }

  tr[aria-current] {
    background: #1f2128;
  }

  .actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
  }
</style>

<script module>
  // Kept while other pages are open, so the scene being edited is still here when you come back.
  let songName = $state('');
  /** The preview as it was when an audition took it off the output, until it is edited again. */
  let pausedLook = $state(null);
  /** The scene open in the editor, `{ song, number }` as in its URL, or null on the scenes home. */
  let opened = $state.raw(null);
  /**
   * The scene being edited, as in the song file, and its number: a look on the stage unless made
   * a swell or pattern. `replaces` is the scene as loaded, `{ note, cue }` as the save endpoint
   * takes it, so the engine can refuse the save if the file changed since. `hidden` marks the
   * look's layers left off the preview while editing. `sceneKept` says why a fixed look opened on
   * the stage stays one, read-only, or is null. `unchanged` is the form as opened or last saved,
   * to tell unsaved edits.
   */
  let form = $state(newForm());
  let isShowing = false;

  function newForm() {
    return { number: null, replaces: null, cue: { name: '', type: 'pixel', layers: [] }, hidden: [], sceneKept: null, unchanged: null };
  }

  function withoutEmptyTimes(edited) {
    const cue = $state.snapshot(edited);
    for (const optional of ['fade_in', 'release']) if (!cue[optional]) delete cue[optional];
    return cue;
  }

  /** The scene as saved from `edited`: a fixed look that stays one keeps its values. */
  function cueToSave(edited) {
    if (!edited.sceneKept) return withoutEmptyTimes(edited.cue);
    const { name, fade_in, release } = edited.cue;
    return withoutEmptyTimes({ ...edited.replaces.cue, name, fade_in, release });
  }

  function unchangedOf(edited) {
    return JSON.stringify({ number: edited.number, cue: cueToSave(edited) });
  }

  /** Whether the scene in the editor has edits not saved yet: checked on leaving, not on every edit. */
  function isDirty() {
    return opened !== null && form.unchanged !== null && unchangedOf(form) !== form.unchanged;
  }

  const LEAVE_QUESTION = 'This scene has unsaved changes.';

  // Here rather than on the component: edits are kept while other pages are open, and a reload
  // or a closed tab would lose them all the same.
  addEventListener('beforeunload', (event) => {
    if (isDirty()) event.preventDefault();
  });
</script>

<script>
  import { engine, request } from './engine.svelte.js';
  import { FULL_VELOCITY, NOTE_OFF, NOTE_ON, PROGRAM_CHANGE } from './audition.js';
  import { beatsPerBar, DEFAULT_BPM } from './duration.js';
  import FrontView from './FrontView.svelte';
  import { asLayers, sceneAsPixelCue } from './layers.js';
  import PatternGrid from './PatternGrid.svelte';
  import ScenePads from './ScenePads.svelte';
  import StageComposer from './StageComposer.svelte';
  import SwellEditor from './SwellEditor.svelte';
  import Timeline from './Timeline.svelte';

  /** The highest scene or song number the engine takes: the highest MIDI note and program. */
  const HIGHEST_NUMBER = 127;
  /** Well inside the engine's 5 s preview lifetime (docs/api.md), so the look never flickers off. */
  const PREVIEW_REFRESH_MS = 2000;
  /** What each type of scene is called in the builder. */
  const KINDS = { pixel: 'Look', swell: 'Swell', pattern: 'Pattern', scene: 'Fixed look' };

  let error = $state('');
  let notice = $state('');
  /** `GET /api/show`: palettes and songs. */
  let summary = $state(null);
  /** The selected song file, as JSON. */
  let song = $state(null);
  let tempo = $state(DEFAULT_BPM);
  /** `GET /api/history`: how many saves can be undone and redone. */
  let steps = $state({ undo: 0, redo: 0 });
  /** The new-song form while it is open. */
  let newSong = $state(null);
  /** The song whose timeline `#builder/<song>/timeline` opens, or null. */
  let timelineSong = $state(null);
  /** The name of the song `song` holds: until a switch loads, `songName` names the next one. */
  let songLoaded = $state('');

  const editable = $derived(engine.connected && engine.rig?.mode === 'edit');
  /** The scene's look can be edited on the stage: not a fixed look that stays one. */
  const isLookEditable = $derived(editable && !form.sceneKept);
  const targets = $derived(engine.rig ? [...Object.keys(engine.rig.groups), ...engine.rig.fixtures.map((fixture) => fixture.name)] : []);
  const isPixel = $derived(form.cue.type === 'pixel');
  const shownLayers = $derived(isPixel ? form.cue.layers.filter((_, index) => !form.hidden[index]) : []);
  /** What the output previews while editing: the look's shown layers; null for nothing. */
  const shown = $derived(
    opened && shownLayers.length && songName && !form.sceneKept
      ? { path: '/api/preview/pixel', body: { song: songName, cue: { ...withoutEmptyTimes(form.cue), layers: $state.snapshot(shownLayers) } } }
      : null,
  );
  const isPaused = $derived(JSON.stringify(shown) === pausedLook);
  const cues = $derived(Object.entries(song?.cues ?? {}).map(([note, cue]) => ({ note: Number(note), cue })));
  const songs = $derived([...(summary?.songs ?? [])].sort((a, b) => a.program - b.program));
  const barBeats = $derived(beatsPerBar(song?.time_signature));
  /** The scene being edited as the stream reports it, while its saved version is auditioned. */
  const editedStatus = $derived(
    engine.frame?.song === songName ? engine.frame.cues.find((cue) => cue.note === form.replaces?.note && !cue.released) : undefined,
  );
  /** The look's layers as saved, which an audition plays. */
  const savedLayers = $derived(form.replaces?.cue.type === 'pixel' ? asLayers(form.replaces.cue).layers : []);
  /**
   * Whether the scene has unsaved edits, for the status and the title: checked on a timer, as
   * `isDirty()` serializes the whole scene, which a drag on the stage would do on every pointer move.
   */
  let isDirtyNow = $state(false);
  const DIRTY_CHECK_MS = 500;
  $effect(() => {
    if (!opened) return;
    const timer = setInterval(() => (isDirtyNow = isDirty()), DIRTY_CHECK_MS);
    return () => clearInterval(timer);
  });
  /** The name of the scene already on the number typed, when it is another scene's. */
  const numberTakenBy = $derived(form.number !== form.replaces?.note ? (song?.cues?.[form.number]?.name ?? '') : '');
  /** Why the scene can't be saved yet, or an empty string. */
  const saveBlocker = $derived.by(() => {
    if (form.number == null) return 'Give the scene a number.';
    if (numberTakenBy) return `Scene ${form.number} is taken by ${numberTakenBy}.`;
    if (!form.cue.name) return 'Name the scene to save it.';
    if (form.cue.type === 'scene' && !form.sceneKept) return 'Waiting for the rig.';
    if (isPixel && !form.sceneKept && !form.cue.layers.length) return 'Put an area on the stage to save it.';
    return '';
  });
  const canSave = $derived(editable && song && !saveBlocker);

  $effect(() => {
    document.title = opened && isDirtyNow ? `• ${form.replaces ? `Scene ${form.replaces.note}` : 'New scene'} — Blimey` : 'Blimey';
    return () => (document.title = 'Blimey');
  });

  /** What a new scene of each type starts as. */
  const newCue = {
    swell: () => ({ type: 'swell', target: { [targets[0]]: { dimmer: '100%' } }, duration: '4 bars' }),
    pattern: () => ({ type: 'pattern', length: '1 bar', target: { [targets[0]]: 'dimmer' }, hits: [] }),
    pixel: () => ({ type: 'pixel', layers: [] }),
  };

  /** Starts the new scene over as `type`, keeping what the form shows for every type. */
  function changeType(type) {
    const { name, fade_in, release } = form.cue;
    const kept = { name, ...(release && { release }), ...(fade_in && type === 'pixel' && { fade_in }) };
    form.cue = { ...kept, ...newCue[type]() };
    form.hidden = [];
  }

  async function attempt(action) {
    error = '';
    notice = '';
    try {
      await action();
    } catch (failure) {
      error = failure.message;
    }
  }

  /** The lowest number from `from` up that isn't in `taken`; undefined when all are. */
  function freeNumber(taken, from = 1) {
    for (let number = from; number <= HIGHEST_NUMBER; number += 1) if (!taken.has(number)) return number;
  }

  const sceneNumbers = () => new Set(cues.map(({ note }) => note));
  const songFile = (name) => `/api/show/songs/${name}.yaml`;
  const sceneHash = ({ song, number }) => `#builder/${song}/${number}`;

  /** The scene `#builder/<song>/<number>` names, or null for the scenes home. */
  function sceneInUrl() {
    const [, name, number] = location.hash.slice(1).split('/');
    return name && /^\d+$/.test(number ?? '') ? { song: name, number: Number(number) } : null;
  }

  const timelineHash = (name) => `#builder/${name}/timeline`;

  function timelineInUrl() {
    const [, name, screen] = location.hash.slice(1).split('/');
    return name && screen === 'timeline' ? name : null;
  }

  /** Loads the selected song; a response for a song switched away from meanwhile is dropped. */
  async function loadSong() {
    const name = songName;
    const loaded = name ? await request('GET', songFile(name)) : null;
    if (name === songName) showSong(loaded, name);
  }

  function showSong(loaded, name) {
    song = loaded;
    songLoaded = name;
    tempo = song?.bpm ?? DEFAULT_BPM;
  }

  /** A timeline URL naming a song the show doesn't have goes home, saying why. */
  function refuseUnknownTimeline() {
    if (!summary || !timelineSong || summary.songs.some((entry) => entry.name === timelineSong)) return;
    error = `There is no song ${timelineSong} in the show.`;
    timelineSong = null;
    location.replace('#builder');
  }

  async function loadSummary() {
    summary = await request('GET', '/api/show');
    refuseUnknownTimeline();
    if (!summary.songs.some((entry) => entry.name === songName)) songName = summary.songs[0]?.name ?? '';
  }

  async function loadSteps() {
    steps = await request('GET', '/api/history');
  }

  attempt(loadSummary);

  // The front view in the editor saves `rig.yaml` too; every saved rig comes back on the
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

  /**
   * Shows what the URL names, the scenes home or a scene in the editor, unless leaving the
   * editor would lose unsaved edits the user wants to keep: then the URL goes back to the scene.
   */
  // Whether the Builder tab is on screen: a URL followed while it opens is where it opens, not a leave.
  $effect(() => {
    isShowing = true;
    return () => (isShowing = false);
  });

  function followUrl() {
    if (!location.hash.startsWith('#builder')) return;
    // A scene with unsaved edits stays on screen, and the URL goes back to it below.
    if (!isDirty()) {
      timelineSong = timelineInUrl();
      refuseUnknownTimeline();
      if (timelineSong) songName = timelineSong;
    }
    const next = sceneInUrl();
    if (next?.song === opened?.song && next?.number === opened?.number) return;
    if (isDirty()) {
      const destination = location.hash;
      // Replacing fires `hashchange` again, so the Builder tab follows; the check above then finds the scene open.
      location.replace(sceneHash(opened));
      // A tab opened on another URL while a scene has edits shows that scene again, as any tab switch does.
      if (isShowing) askToLeave({ hash: destination, isReplaced: true });
      return;
    }
    opened = next?.number > HIGHEST_NUMBER ? null : next;
    form = newForm();
    isDirtyNow = false;
    if (next && !opened) {
      error = `There is no scene ${next.number}: scene numbers go from 0 to ${HIGHEST_NUMBER}.`;
      location.replace('#builder');
    } else if (next) {
      attempt(() =>
        openScene(next).catch((failure) => {
          if (opened === next) location.replace('#builder');
          throw failure;
        }),
      );
    }
  }

  function leaveEditor(event) {
    if (!isDirty()) return;
    event.preventDefault();
    askToLeave({ hash: '#builder', isReplaced: false });
  }

  /**
   * Where to go once the unsaved edits are saved or dropped, while the leave prompt is up: the
   * `hash`, and whether the history entry it was on already holds the scene, so going there
   * replaces it rather than adding one. Read only by the prompt's buttons, so never cleared.
   */
  let leaving = null;
  let leavePrompt = $state();

  function askToLeave(destination) {
    leaving = destination;
    if (!leavePrompt.open) leavePrompt.showModal();
  }

  function goOn({ hash, isReplaced }) {
    if (isReplaced) location.replace(hash);
    else location.hash = hash;
  }

  function leaveWithoutSaving() {
    form.unchanged = null;
    const destination = leaving;
    leavePrompt.close();
    goOn(destination);
  }

  async function saveAndLeave() {
    const destination = leaving;
    try {
      await save();
    } finally {
      leavePrompt.close();
    }
    goOn(destination);
  }

  /** The save under way, so a second request while it runs joins it rather than racing it. */
  let saving = null;
  function save() {
    saving ??= saveScene().finally(() => (saving = null));
    return saving;
  }

  /**
   * Opens the scene `number` of `song`, or a new scene with that number. A look opens in the
   * `layers` form; a fixed look opens as it is, until the rig is known to convert it.
   */
  async function openScene(scene) {
    const loaded = await request('GET', songFile(scene.song));
    if (opened !== scene) return;
    songName = scene.song;
    showSong(loaded, scene.song);
    const cue = song.cues?.[scene.number];
    if (!cue) {
      form = { ...newForm(), number: scene.number };
    } else {
      const edited = cue.type === 'pixel' ? asLayers($state.snapshot(cue)) : $state.snapshot(cue);
      form = { ...newForm(), number: scene.number, replaces: { note: scene.number, cue: $state.snapshot(cue) }, cue: edited };
    }
    form.unchanged = unchangedOf(form);
  }

  // A fixed look opens on the stage once the rig is known: converting it needs its groups and fixtures.
  $effect(() => {
    if (form.cue.type !== 'scene' || !engine.rig) return;
    const { cue, reason } = sceneAsPixelCue($state.snapshot(form.cue), engine.rig);
    form.cue = cue;
    form.sceneKept = reason;
    form.unchanged = unchangedOf(form);
  });

  // One preview request at a time, always the latest, so the output never steps back.
  let queuedPreview;
  let isQueued = false;
  let previewing = false;
  async function preview(next) {
    queuedPreview = next;
    isQueued = true;
    if (previewing) return;
    previewing = true;
    while (isQueued) {
      const current = queuedPreview;
      isQueued = false;
      try {
        await (current ? request('PUT', current.path, current.body) : request('DELETE', '/api/preview'));
      } catch (failure) {
        error = failure.message;
      }
    }
    previewing = false;
  }

  // The engine drops a preview it hasn't heard of for a while, so a closed or crashed builder
  // leaves nothing on stage: keep this one alive while it is shown.
  $effect(() => {
    if (!editable || isPaused) return;
    const current = shown;
    preview(current);
    if (!current) return;
    const heartbeat = setInterval(() => preview(current), PREVIEW_REFRESH_MS);
    return () => clearInterval(heartbeat);
  });

  /** Takes the look off the output when the browser page goes; nobody is left to hear back. */
  const clearPreview = () => editable && fetch('/api/preview', { method: 'DELETE', keepalive: true });

  /** Saves the scene under its number, which moves it there if it had another. */
  async function saveScene() {
    const { number } = form;
    const sent = unchangedOf(form);
    try {
      await request('PUT', `/api/songs/${songName}/cues/${number}`, { cue: cueToSave(form), replaces: form.replaces });
    } catch (failure) {
      if (!failure.message.includes('already fires cue')) throw failure;
      throw new Error(`Scene ${number} is already ${song.cues?.[number]?.name ?? 'taken'}: pick a free number, or open that scene.`);
    }
    // Saved as of now, not once the song reloads: leaving in between is no leave with unsaved edits.
    form.unchanged = sent;
    isDirtyNow = unchangedOf(form) !== sent;
    notice = `Saved scene ${number} in ${songName}.`;
    const saved = form;
    await reloadAfterSave();
    // Left for the scenes home, or another scene, while the song reloaded.
    if (form !== saved) return;
    const isEditedSince = unchangedOf(form) !== form.unchanged;
    form.replaces = { note: number, cue: $state.snapshot(song.cues[number]) };
    if (!isEditedSince) form.unchanged = unchangedOf(form);
    if (number !== opened.number) {
      opened = { song: songName, number };
      location.replace(sceneHash(opened));
    }
  }

  /**
   * Makes the swell or pattern `number` drive the intensity of the look being edited, by the
   * look's name as saved: the engine refuses a name no saved look has.
   */
  async function linkIntensity(number) {
    const { name } = form.replaces.cue;
    const linked = $state.snapshot(song.cues[number]);
    const cue = { ...linked, target: { [name]: 'intensity' } };
    await request('PUT', `/api/songs/${songName}/cues/${number}`, { cue, replaces: { note: number, cue: linked } });
    notice = `Scene ${number} now drives the intensity of ${name}.`;
    await reloadAfterSave();
  }

  async function deleteScene(number) {
    await request('DELETE', `/api/songs/${songName}/cues/${number}`);
    notice = `Deleted scene ${number}. Undo brings it back.`;
    form = newForm();
    location.replace('#builder');
    await reloadAfterSave();
  }

  /** Copies the saved scene to the next free number above it, or the lowest free one. */
  async function duplicateScene(number) {
    const taken = sceneNumbers();
    const free = freeNumber(taken, number + 1) ?? freeNumber(taken);
    if (free === undefined) throw new Error(`Every number up to ${HIGHEST_NUMBER} is taken: delete a scene first.`);
    const original = $state.snapshot(song.cues[number]);
    await request('PUT', `/api/songs/${songName}/cues/${free}`, { cue: { ...original, name: `${original.name}-copy` }, replaces: null });
    notice = `Duplicated scene ${number} as scene ${free}.`;
    await reloadAfterSave();
  }

  function openNewScene() {
    const number = freeNumber(sceneNumbers());
    if (number === undefined) error = `Every number up to ${HIGHEST_NUMBER} is taken: delete a scene first.`;
    else location.hash = sceneHash({ song: songName, number });
  }

  async function setTempo() {
    await request('PUT', `/api/songs/${songName}/bpm`, { bpm: tempo });
    notice = `${songName} runs at ${tempo} BPM when no clock comes in.`;
    await reloadAfterSave();
  }

  /** `undo` or `redo` one save; either may add or remove a song. */
  async function step(action) {
    const { file, undo, redo } = await request('POST', `/api/${action}`);
    steps = { undo, redo };
    notice = action === 'undo' ? `Undid the last save of ${file}.` : `Redid the save of ${file}.`;
    const shownSong = songName;
    await loadSummary();
    // A song that went is replaced in the picker, and the switch loads the new one.
    if (songName === shownSong) await loadSong();
  }

  function openNewSong() {
    newSong = { name: '', program: freeNumber(new Set(summary?.songs.map((entry) => entry.program))) ?? null, bpm: null, time_signature: '' };
  }

  async function createSong() {
    const { name, program, bpm, time_signature } = $state.snapshot(newSong);
    try {
      await request('POST', '/api/songs', { name, program, ...(bpm ? { bpm } : {}), ...(time_signature ? { time_signature } : {}) });
    } catch (failure) {
      if (!failure.message.includes('already selects song')) throw failure;
      const taken = summary.songs.find((entry) => entry.program === program);
      throw new Error(`Song ${program} is already ${taken?.name ?? 'taken'}: pick a free number.`);
    }
    newSong = null;
    songName = name;
    notice = `Created song ${program} · ${name}. Undo removes it.`;
    await Promise.all([loadSummary(), loadSteps()]);
  }

  const isPlaying = (number) =>
    engine.frame?.song === songName && engine.frame.cues.some((cue) => cue.note === number && !cue.released);

  const midi = (message) => request('POST', '/api/midi', { message });

  /** Plays the swell being edited again from the top, for a looped audition. */
  const restartEdited = () => midi([NOTE_ON, form.replaces.note, FULL_VELOCITY]).catch((failure) => (error = failure.message));

  /** Fires or releases a saved scene through the engine, as Reaper would. */
  async function audition(number) {
    if (isPlaying(number)) return release(number);
    await fire(number);
  }

  const release = (number) => midi([NOTE_OFF, number, 0]);

  async function fire(number, velocity = FULL_VELOCITY) {
    if (engine.frame?.song !== songName) {
      await midi([PROGRAM_CHANGE, summary.songs.find((entry) => entry.name === songName).program]);
    }
    // The engine takes the preview off the output so the scene shows; editing brings it back.
    if (shown) pausedLook = JSON.stringify(shown);
    await midi([NOTE_ON, number, velocity]);
  }

  /** ⌘S / Ctrl+S saves the open scene, ⌘↩ / Ctrl+↩ auditions or releases it. */
  function shortcut(event) {
    if (!opened || !(event.metaKey || event.ctrlKey) || event.altKey || event.repeat || leavePrompt?.open) return;
    if (event.key === 's') {
      event.preventDefault();
      if (canSave) attempt(save);
    } else if (event.key === 'Enter' && editable && form.replaces !== null) {
      event.preventDefault();
      attempt(() => audition(form.replaces.note));
    }
  }

  followUrl();
</script>

<svelte:window onpagehide={clearPreview} onhashchange={followUrl} onkeydown={shortcut} />

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" aria-live="polite">{notice}</p>{/if}
{#if engine.rig && !editable}
  <p class="hint">
    Show mode: the builder is read-only. Start the engine with <code>--edit</code> to build looks, save scenes and audition them.
  </p>
{/if}

{#snippet history()}
  <span class="history">
    <button class="quiet" disabled={!editable || !steps.undo} onclick={() => attempt(() => step('undo'))}>Undo ({steps.undo})</button>
    <button class="quiet" disabled={!editable || !steps.redo} onclick={() => attempt(() => step('redo'))}>Redo ({steps.redo})</button>
  </span>
{/snippet}

{#if opened}
  <fieldset class="scene" disabled={!editable || !song}>
    <div class="bar">
      <a class="back" href="#builder" onclick={leaveEditor}>← Scenes</a>
      <strong class="title">{#if form.replaces !== null}Scene {form.replaces.note}{:else}New scene{/if}</strong>
      {#if form.replaces !== null}<span class="kind">{KINDS[form.replaces.cue.type]}</span>{/if}
      <span class="status" class:dirty={isDirtyNow} aria-live="polite">
        {#if saveBlocker}{saveBlocker}{:else if isDirtyNow}Unsaved changes{:else if form.replaces !== null}Saved{/if}
      </span>
      {#if form.replaces !== null}
        <button class:playing={isPlaying(form.replaces.note)} title="⌘↩" onclick={() => attempt(() => audition(form.replaces.note))}>
          {isPlaying(form.replaces.note) ? 'Release' : 'Audition'}
        </button>
        <button onclick={() => attempt(() => duplicateScene(form.replaces.note))}>Duplicate</button>
        <button class="danger" onclick={() => attempt(() => deleteScene(form.replaces.note))}>Delete</button>
      {/if}
      <button class="primary" title="⌘S" disabled={!!saveBlocker} onclick={() => attempt(save)}>Save</button>
    </div>
    <div class="row">
      <label>Name <input bind:value={form.cue.name} /></label>
      <label>
        Number
        <input type="number" min="0" max={HIGHEST_NUMBER} title="A new number moves the scene there on save" aria-invalid={!!numberTakenBy || undefined} bind:value={form.number} />
        {#if numberTakenBy}<span class="error" role="status">Taken by {numberTakenBy}</span>{/if}
      </label>
      {#if isPixel}
        <label>Fade in <input bind:value={form.cue.fade_in} placeholder="0s" title="ms, s, beats or bars" /></label>
      {/if}
      <label>Release <input bind:value={form.cue.release} placeholder="0s" title="ms, s, beats or bars" /></label>
      {#if form.replaces === null}
        <span class="hint">
          {#if isPixel}
            To drive other scenes instead, make a
            <button class="link" onclick={() => changeType('swell')}>swell scene</button> or a
            <button class="link" onclick={() => changeType('pattern')}>pattern scene</button>.
          {:else}
            <button class="link" onclick={() => changeType('pixel')}>Make a look instead</button>
          {/if}
        </span>
      {/if}
    </div>
    {#if form.sceneKept}
      <p class="hint" role="status">
        This fixed look stays as it is, read-only on the stage: {form.sceneKept}. It plays as before; you can still rename, renumber, audition,
        duplicate or delete it.
      </p>
    {:else if form.cue.type === 'scene'}
      <p class="hint">The fixed look opens on the stage once the engine sends the rig.</p>
    {:else if form.replaces?.cue.type === 'scene'}
      <p class="hint">Saving makes this fixed look a look on the stage.</p>
    {/if}
  </fieldset>

  <dialog class="leave" bind:this={leavePrompt} aria-labelledby="leave-question">
    <p id="leave-question">{LEAVE_QUESTION}</p>
    <div class="row">
      <button class="primary" disabled={!canSave} onclick={() => attempt(saveAndLeave)}>Save and leave</button>
      <button class="danger" onclick={leaveWithoutSaving}>Leave without saving</button>
      <button class="quiet" onclick={() => leavePrompt.close()}>Stay</button>
    </div>
    {#if saveBlocker}<p class="hint">{saveBlocker}</p>{/if}
  </dialog>

  {#if isPixel && song}
    <!-- A scene opened, or a new one, starts with nothing selected. -->
    {#key form}
      <StageComposer
        bind:cue={form.cue}
        bind:hidden={form.hidden}
        palettes={summary?.palettes}
        editable={isLookEditable}
        paused={isPaused}
        {cues}
        savedName={form.replaces?.cue.name ?? null}
        onlink={(number) => attempt(() => linkIntensity(number))}
        song={songName}
        {barBeats}
        {savedLayers}
        audition={{
          beats: editedStatus?.beats,
          loopBeats: editedStatus?.loop_beats,
          onaudition: () => attempt(() => audition(form.replaces.note)),
          onrestart: restartEdited,
        }}
      />
    {/key}
  {:else}
    <div class="rhythm">
      <fieldset disabled={!editable || !song}>
        {#key form.replaces?.note}
          {@const editorProps = {
            targets,
            // Saved looks only: the engine resolves a link by the name in the file.
            pixelCues: [...new Set(cues.filter(({ cue }) => cue.type === 'pixel').map(({ cue }) => cue.name))],
            palettes: summary?.palettes,
            barBeats,
            beats: editedStatus?.beats,
            saved: form.replaces?.cue ?? null,
            onaudition: () => attempt(() => audition(form.replaces.note)),
          }}
          {#if form.cue.type === 'swell'}
            <SwellEditor bind:cue={form.cue} {...editorProps} onrestart={restartEdited} />
          {:else if form.cue.type === 'pattern'}
            <PatternGrid bind:cue={form.cue} {...editorProps} />
          {/if}
        {/key}
      </fieldset>
      <!-- An overlay locks the rig: this front view previews the scene, it doesn't move fixtures. -->
      {#snippet preview()}{/snippet}
      <div><FrontView overlay={preview} /></div>
    </div>
  {/if}
{:else if timelineSong}
  <div class="bar">
    <a class="back" href="#builder">← Scenes</a>
    <strong class="title">Song {summary?.songs.find((entry) => entry.name === timelineSong)?.program ?? ''} · {timelineSong}: timeline</strong>
    <button disabled={!song?.arrangement?.length || songLoaded !== timelineSong} onclick={() => location.assign(`/api/songs/${encodeURIComponent(timelineSong)}/arrangement.mid`)}
      >Export for Reaper</button
    >
    {@render history()}
  </div>
  <!-- Keyed by song: a switch to another song's timeline starts with nothing open or selected. -->
  {#if song && songLoaded === timelineSong}
    {#key timelineSong}
      <Timeline
        {songName}
        {song}
        {editable}
        {isPlaying}
        {attempt}
        onsaved={async (message) => {
          notice = message;
          await reloadAfterSave();
        }}
        onhold={(number, velocity) => attempt(() => fire(number, velocity))}
        onrelease={(number) => attempt(() => release(number))}
        onaudition={(number) => attempt(() => audition(number))}
        onopenscene={(number) => (location.hash = sceneHash({ song: songName, number }))}
      />
    {/key}
  {/if}
{:else}
  <div class="bar">
    <label>
      Song
      <select bind:value={songName} onchange={() => (error = '')}>
        {#each songs as entry (entry.name)}
          <option value={entry.name}>Song {entry.program} · {entry.name}</option>
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
    <button disabled={!song} onclick={() => (location.hash = timelineHash(songName))}>Timeline</button>
    {@render history()}
  </div>
  {#if newSong}
    <fieldset class="new-song" disabled={!editable}>
      <legend>New song</legend>
      <div class="row">
        <label>Song name <input bind:value={newSong.name} placeholder="encore" /></label>
        <label>Number <input type="number" min="0" max={HIGHEST_NUMBER} bind:value={newSong.program} /></label>
        <label>Tempo <input type="number" min="1" step="any" bind:value={newSong.bpm} placeholder="120" /> BPM</label>
        <label>Time signature <input bind:value={newSong.time_signature} placeholder="4/4" /></label>
      </div>
      <p class="hint">The name is letters, digits, - and _. Each song has a number of its own.</p>
      <div class="row">
        <button disabled={!newSong.name || newSong.program == null} onclick={() => attempt(createSong)}>Create song</button>
        <button onclick={() => (newSong = null)}>Cancel</button>
      </div>
    </fieldset>
  {:else if summary && !summary.songs.length}
    <p class="hint">This show has no songs yet: create one with New song to save scenes.</p>
  {/if}

  {#if song}
    <ScenePads
      {songName}
      cues={song.cues}
      tempo={song.bpm}
      {editable}
      {isPlaying}
      onopen={(number) => (location.hash = sceneHash({ song: songName, number }))}
      onnew={openNewScene}
      onhold={(number) => attempt(() => fire(number))}
      onrelease={(number) => attempt(() => release(number))}
      onaudition={(number) => attempt(() => audition(number))}
    />
  {/if}
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
    gap: 4px;
    margin-left: auto;
  }

  .back {
    color: var(--accent);
    text-decoration: none;
  }

  .back:hover {
    text-decoration: underline;
  }

  .title {
    font-size: 18px;
  }

  .kind {
    color: var(--muted);
  }

  .status {
    flex: 1;
    font-size: 13px;
    color: var(--muted);
    text-align: right;
  }

  .status.dirty {
    color: var(--accent);
  }

  .playing {
    color: var(--accent);
    border-color: var(--accent);
  }

  .scene {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-bottom: 14px;
    margin-bottom: 14px;
    border-bottom: 1px solid var(--line);
  }

  .scene .bar {
    margin: 0;
  }

  .scene .row {
    gap: 8px 16px;
  }

  .scene p {
    margin: 0;
  }

  .scene label .error {
    font-size: 13px;
  }

  input[aria-invalid='true'] {
    border-color: var(--danger);
  }

  .leave {
    min-width: 24em;
    padding: 20px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
  }

  .leave::backdrop {
    background: rgb(0 0 0 / 0.6);
  }

  .leave p {
    margin: 0 0 14px;
  }

  .leave .hint {
    margin: 10px 0 0;
  }

  .rhythm {
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
    gap: 20px;
    align-items: start;
  }

  .new-song {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    margin-bottom: 16px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
  }

  .new-song legend {
    padding: 0 6px;
    color: var(--muted);
  }

  input[type='number'] {
    width: 5em;
  }

  .notice {
    color: var(--accent);
  }
</style>

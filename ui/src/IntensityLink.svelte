<script>
  /**
   * `cues`: the song's cues as `{ note, cue }`. `savedName`: the pixel cue's name in the song
   * file, which links go by; null until it is saved. `name`: its name as edited.
   * `onlink(note)`: makes that swell or pattern drive the pixel cue's intensity.
   */
  let { cues = [], savedName = null, name, onlink } = $props();

  let linking = $state('');

  const linkable = $derived(cues.filter(({ cue }) => ['swell', 'pattern'].includes(cue.type)));
  const isLinked = (other) => savedName !== null && other.target?.[savedName] === 'intensity';
</script>

<div class="link">
  <span class="name">Intensity</span>
  {#each linkable.filter(({ cue }) => isLinked(cue)) as { note, cue } (note)}
    <span class="linked">driven by scene {note}, {cue.name} ({cue.type})</span>
  {:else}
    <span class="hint">not linked</span>
  {/each}
  <select aria-label="Swell or pattern to link" bind:value={linking}>
    <option value="">Swell or pattern…</option>
    {#each linkable.filter(({ cue }) => !isLinked(cue)) as { note, cue } (note)}
      <option value={String(note)}>Scene {note}: {cue.name} ({cue.type})</option>
    {/each}
  </select>
  <button
    disabled={!linking || savedName === null}
    onclick={() => {
      onlink(Number(linking));
      linking = '';
    }}>Link to intensity</button
  >
</div>
<p class="hint">
  Linking saves that scene with this look's saved name as its target, in place of the target it had.
  {#if savedName === null}Save this look first.{:else if savedName !== name}Links follow the saved name, {savedName}, until you save.{/if}
</p>

<style>
  .link {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 10px;
    align-items: center;
  }

  .name {
    color: var(--muted);
  }

  .linked {
    color: var(--accent);
  }
</style>

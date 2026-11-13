<script>
  /**
   * A cue's `target` with one entry: a fixture or group, and whether the cue drives its dimmer
   * or its colour. `dimmer` is the entry written for a dimmer target: `{ dimmer: '100%' }` for a
   * swell, `'dimmer'` for a pattern. A target with more than that is kept as it is.
   */
  let { target = $bindable(), targets, palettes, dimmer } = $props();

  const entries = $derived(Object.entries(target ?? {}));
  const name = $derived(entries[0]?.[0]);
  const value = $derived(entries[0]?.[1]);
  const settings = (entry) => (typeof entry === 'object' && entry !== null ? Object.keys(entry).join() : null);
  const drives = $derived.by(() => {
    if (entries.length !== 1) return null;
    if (value === 'dimmer' || settings(value) === 'dimmer') return 'dimmer';
    return settings(value) === 'color' ? 'colour' : null;
  });
  const colours = $derived(Object.keys(palettes ?? {}));

  function drive(kind) {
    target = { [name]: kind === 'dimmer' ? structuredClone(dimmer) : { color: colours[0] ?? '#ffffff' } };
  }
</script>

{#if drives}
  <label>
    Target
    <select aria-label="Target" value={name} onchange={(event) => (target = { [event.currentTarget.value]: value })}>
      {#if !targets.includes(name)}<option>{name}</option>{/if}
      {#each targets as option (option)}<option>{option}</option>{/each}
    </select>
  </label>
  <label>
    Drives
    <select value={drives} onchange={(event) => drive(event.currentTarget.value)}>
      <option value="dimmer">dimmer</option>
      <option value="colour">colour</option>
    </select>
  </label>
  {#if drives === 'dimmer' && typeof value === 'object'}
    <label>
      Target level
      <input
        type="range"
        min="0"
        max="100"
        value={parseFloat(value.dimmer)}
        oninput={(event) => (target = { [name]: { dimmer: `${event.currentTarget.value}%` } })}
      />
      <output>{value.dimmer}</output>
    </label>
  {/if}
  {#if drives === 'colour'}
    <label>
      Target colour
      <select value={value.color} onchange={(event) => (target = { [name]: { color: event.currentTarget.value } })}>
        {#if !colours.includes(value.color)}<option>{value.color}</option>{/if}
        {#each colours as colour (colour)}<option>{colour}</option>{/each}
      </select>
    </label>
  {/if}
{:else}
  <span class="hint">
    Target <code>{JSON.stringify(target)}</code> is more than one fixture or setting; it is kept as it is.
    <button onclick={() => (target = { [targets[0]]: structuredClone(dimmer) })}>Pick one target</button>
  </span>
{/if}

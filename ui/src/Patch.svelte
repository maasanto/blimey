<script>
  import { engine, request } from './engine.svelte.js';

  let error = $state('');
  const canTest = $derived(engine.connected && engine.rig?.mode === 'edit');

  async function test(fixture) {
    error = '';
    try {
      await request('POST', `/api/fixtures/${encodeURIComponent(fixture)}/test`);
    } catch (failure) {
      error = failure.message;
    }
  }
</script>

{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if engine.rig && !canTest}
  <p class="hint">Show mode: fixture tests are off. Start the engine with <code>--edit</code> to test fixtures.</p>
{/if}

<table>
  <thead>
    <tr><th>Fixture</th><th>Profile</th><th>Mode</th><th>DMX</th><th>Channels</th><th>Pixels</th><th></th></tr>
  </thead>
  <tbody>
    {#each engine.rig?.fixtures ?? [] as fixture (fixture.name)}
      <tr data-fixture={fixture.name}>
        <td>{fixture.name}</td>
        <td>{fixture.profile}</td>
        <td>{fixture.mode}</td>
        <td>{fixture.address}–{fixture.address + fixture.channels.length - 1}</td>
        <td>
          <details>
            <summary>{fixture.channels.length}</summary>
            <ol start={fixture.address}>
              {#each fixture.channels as channel, index (index)}<li>{channel}</li>{/each}
            </ol>
          </details>
        </td>
        <td>{fixture.pixels.length}</td>
        <td>
          <button disabled={!canTest} onclick={() => test(fixture.name)}>Test</button>
        </td>
      </tr>
    {/each}
  </tbody>
</table>

{#if engine.rig && Object.keys(engine.rig.groups).length}
  <h2>Groups</h2>
  <dl>
    {#each Object.entries(engine.rig.groups) as [group, members] (group)}
      <dt>{group}</dt>
      <dd>{members.join(', ')}</dd>
    {/each}
  </dl>
{/if}

<style>
  table {
    border-collapse: collapse;
    width: 100%;
  }

  th,
  td {
    padding: 6px 10px;
    text-align: left;
    vertical-align: top;
    border-bottom: 1px solid var(--line);
  }

  ol {
    margin: 4px 0;
    font: 12px/1.4 var(--mono);
  }
</style>

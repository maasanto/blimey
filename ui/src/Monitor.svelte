<script>
  import { engine } from './engine.svelte.js';

  const DMX_CHANNELS = 512;
  const dmx = $derived(engine.frame?.dmx ?? Array(DMX_CHANNELS).fill(0));
  const percent = (level) => `${Math.round(level * 100)}%`;
  const seconds = (ms) => `${(ms / 1000).toFixed(2)} s`;

  const TRANSPORT = { 0xfa: 'start', 0xfb: 'continue', 0xfc: 'stop' };

  /** A MIDI message in words, as the engine reads it (docs/api.md, Playback). */
  function describe([status, first, second]) {
    if (status in TRANSPORT) return TRANSPORT[status];
    if (status === 0xf2) return `song position ${first + (second << 7)} sixteenths`;
    const channel = `ch ${(status & 0x0f) + 1}`;
    switch (status & 0xf0) {
      case 0x90:
        return second > 0 ? `${channel} note on ${first}, velocity ${second}` : `${channel} note off ${first}`;
      case 0x80:
        return `${channel} note off ${first}`;
      case 0xb0:
        return `${channel} CC ${first} = ${second}`;
      case 0xc0:
        return `${channel} program change ${first}`;
      default:
        return [status, first, second].filter((byte) => byte !== undefined).map((byte) => byte.toString(16).padStart(2, '0')).join(' ');
    }
  }
</script>

<section>
  <h2>Engine</h2>
  <dl>
    <dt>Connection</dt>
    <dd>{engine.connected ? 'Connected' : 'Reconnecting…'}</dd>
    <dt>Engine time</dt>
    <dd data-testid="engine-time">{engine.frame ? seconds(engine.frame.at_ms) : '—'}</dd>
    <dt>Frames received</dt>
    <dd>{engine.framesPerSecond} / s</dd>
    <dt>Fixtures</dt>
    <dd>{engine.rig?.fixtures.length ?? '—'}</dd>
  </dl>
</section>

<section>
  <h2>Playback</h2>
  <dl>
    <dt>Song</dt>
    <dd data-testid="song">{engine.frame ? (engine.frame.song ?? 'None selected') : '—'}</dd>
    <dt>Master</dt>
    <dd data-testid="master">{engine.frame ? percent(engine.frame.master) : '—'}</dd>
    <dt>Live parameters</dt>
    <dd>
      <ol class="parameters">
        {#each engine.frame?.live_parameters ?? [] as level, index (index)}
          <li title="CC {20 + index}">{index + 1}: {percent(level)}</li>
        {/each}
      </ol>
    </dd>
    <dt>Active cues</dt>
    <dd data-testid="cues">
      {#if engine.frame?.cues.length}
        <ol class="cues">
          {#each engine.frame.cues as cue, index (index)}
            <li>
              <span class="note">{cue.note}</span>
              {cue.name} · {percent(cue.intensity)}{cue.released ? ' · releasing' : ''}
            </li>
          {/each}
        </ol>
      {:else}
        None
      {/if}
    </dd>
  </dl>
</section>

<section>
  <h2>MIDI in</h2>
  {#if engine.midi.length}
    <ol class="midi" data-testid="midi-log">
      {#each engine.midi as event, index (index)}
        <li><span class="time">{seconds(event.at_ms)}</span>{describe(event.message)}</li>
      {/each}
    </ol>
  {:else}
    <p class="pending" data-testid="midi-log">No MIDI received since this page opened.</p>
  {/if}
</section>

<section>
  <h2>DMX output</h2>
  <ol class="universe">
    {#each dmx as value, index (index)}
      <li data-channel={index + 1} title="DMX {index + 1}" style:--level={value / 255}>{value}</li>
    {/each}
  </ol>
</section>

<style>
  ol {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .parameters {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    font: 13px/1.5 var(--mono);
  }

  .note,
  .time {
    display: inline-block;
    min-width: 5em;
    color: var(--muted);
    font-family: var(--mono);
  }

  .midi {
    max-height: 12em;
    overflow-y: auto;
    font: 13px/1.5 var(--mono);
  }

  .universe {
    display: grid;
    grid-template-columns: repeat(32, 1fr);
    gap: 2px;
    padding-left: 3.5em;
    font: 11px/1 var(--mono);
  }

  .universe li {
    position: relative;
    padding: 4px 0;
    text-align: center;
    background: rgb(255 190 80 / calc(var(--level) * 0.85));
    border-radius: 2px;
  }

  .universe li:nth-child(32n + 1)::before {
    content: attr(data-channel);
    position: absolute;
    right: calc(100% + 0.6em);
    color: var(--muted);
  }

  .universe li:nth-child(16n + 1) {
    box-shadow: inset 2px 0 var(--line);
  }
</style>

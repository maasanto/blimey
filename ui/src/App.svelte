<script>
  import { engine } from './engine.svelte.js';
  import Builder from './Builder.svelte';
  import Monitor from './Monitor.svelte';
  import Patch from './Patch.svelte';
  import FrontView from './FrontView.svelte';

  const pages = {
    monitor: { title: 'Monitor', component: Monitor },
    builder: { title: 'Builder', component: Builder },
    patch: { title: 'Patch', component: Patch },
    front: { title: 'Front view', component: FrontView },
  };

  /** The page a hash names: `#builder/<song>/<number>` is the builder too. */
  function pageInUrl() {
    const name = location.hash.slice(1).split('/')[0];
    return name in pages ? name : 'monitor';
  }
  let page = $state(pageInUrl());
  const Page = $derived(pages[page].component);
  /** The Builder tab goes back where it was left: the scenes home or a scene in its editor. */
  let builderHash = $state(pageInUrl() === 'builder' ? location.hash : '#builder');

  function followUrl() {
    page = pageInUrl();
    if (page === 'builder') builderHash = location.hash;
  }
</script>

<svelte:window onhashchange={followUrl} />

<header>
  <h1>Blimey</h1>
  <nav>
    {#each Object.entries(pages) as [name, { title }] (name)}
      <a href={name === 'builder' ? builderHash : `#${name}`} aria-current={page === name ? 'page' : undefined}>{title}</a>
    {/each}
  </nav>
  {#if engine.rig}
    <span class="mode" data-mode={engine.rig.mode}>{engine.rig.mode} mode</span>
  {/if}
  <span class="connection" data-connected={engine.connected}>
    {engine.connected ? 'Connected' : 'Reconnecting…'}
  </span>
</header>

{#if !engine.connected}
  <p class="banner" role="status">The engine is not reachable. Reconnecting…</p>
{/if}

<main>
  <Page />
</main>

<script>
  import { engine } from './engine.svelte.js';
  import Monitor from './Monitor.svelte';
  import Patch from './Patch.svelte';
  import StagePlan from './StagePlan.svelte';

  const pages = {
    monitor: { title: 'Monitor', component: Monitor },
    patch: { title: 'Patch', component: Patch },
    stage: { title: 'Stage plan', component: StagePlan },
  };

  const pageInUrl = () => (location.hash.slice(1) in pages ? location.hash.slice(1) : 'monitor');
  let page = $state(pageInUrl());
  const Page = $derived(pages[page].component);
</script>

<svelte:window onhashchange={() => (page = pageInUrl())} />

<header>
  <h1>Blimey</h1>
  <nav>
    {#each Object.entries(pages) as [name, { title }] (name)}
      <a href="#{name}" aria-current={page === name ? 'page' : undefined}>{title}</a>
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

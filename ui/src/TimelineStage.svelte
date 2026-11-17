<script module>
  // Kept across songs: the timeline is built anew for each one.
  let isShown = $state(true);
  let isLarge = $state(false);
</script>

<script>
  import FrontView from './FrontView.svelte';
</script>

<!-- The output as the engine streams it: playback, held blocks and scene pads. Locked, as a preview. -->
<aside class="stage-view" class:shown={isShown} class:large={isLarge} aria-label="Stage">
  <div class="row">
    <button class="quiet" aria-pressed={isLarge} disabled={!isShown} onclick={() => (isLarge = !isLarge)}>Large</button>
    <button class="quiet" aria-expanded={isShown} onclick={() => (isShown = !isShown)}>Stage</button>
  </div>
  {#if isShown}<FrontView locked />{/if}
</aside>

<style>
  /* In sight while the page scrolls. */
  .stage-view {
    position: sticky;
    top: 20px;
    margin-left: auto;
    width: fit-content;
  }

  .stage-view.shown {
    width: 320px;
  }

  /* Large, but never more than a third of the window beside the section editor. */
  .stage-view.shown.large {
    width: min(560px, 33vw);
  }

  /* Timeline's breakpoint: under the section editor, it can take the room. */
  @media (max-width: 1100px) {
    .stage-view.shown.large {
      width: min(560px, 100%);
    }
  }

  .row {
    display: flex;
    gap: 4px;
    justify-content: flex-end;
  }

  [aria-pressed='true'],
  [aria-expanded='true'] {
    color: var(--accent);
  }
</style>

<script lang="ts">
  // Every published guide (#94).
  import { onMount } from 'svelte';
  import { Logo } from '../design';
  import type { PublicGuideList } from '../generated/PublicGuideList';

  let list: PublicGuideList | null = $state(null);
  let failed = $state(false);
  onMount(async () => {
    document.title = 'Guides · POP';
    try {
      const res = await fetch('/api/public/guides');
      if (res.ok) list = (await res.json()) as PublicGuideList;
      else failed = true;
    } catch {
      failed = true;
    }
  });
</script>

<main class="page">
  <header><Logo /><h1>Guides</h1><a class="app" href="/">Printers</a></header>
  {#if list}
    {#if list.guides.length === 0}
      <p class="muted">No guides are published yet.</p>
    {:else}
      <ul>
        {#each list.guides as g (g.slug)}
          <li>
            <a href="/guides/{g.slug}">{g.title}</a>
            {#if g.summary}<span class="muted">{g.summary}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if list.license}
      <p class="muted license">Shared under {list.license}.</p>
    {/if}
  {:else if failed}
    <p class="muted">POP didn't answer. Try again in a moment.</p>
  {:else}
    <p class="muted" role="status">Loading…</p>
  {/if}
</main>

<style>
  .page {
    box-sizing: border-box;
    min-height: 100vh;
    max-width: 760px;
    margin: 0 auto;
    padding: 24px 16px 48px;
    display: flex;
    flex-direction: column;
    gap: 20px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 14px;
  }
  .app {
    margin-left: auto;
    padding: 8px 0;
    color: var(--pop-butter);
    font-size: 16px;
  }
  h1 {
    margin: 0;
    font-size: 32px;
    font-weight: 600;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  li {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 18px;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
  }
  li a {
    color: var(--pop-text);
    font-size: 22px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  .license {
    font-size: 15px;
  }
</style>

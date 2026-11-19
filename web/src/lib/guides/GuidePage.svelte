<script lang="ts">
  // A published guide, for anyone (#94), and at the kiosk (#56).
  import { onMount } from 'svelte';
  import { Logo, Version } from '../design';
  import GuideView from './GuideView.svelte';
  import type { PublicGuide } from '../generated/PublicGuide';

  let { slug, kiosk = false }: { slug: string; kiosk?: boolean } = $props();

  let guide: PublicGuide | null = $state(null);
  let missing = $state(false);
  let failed = $state(false);

  onMount(async () => {
    try {
      // An old slug redirects; fetch follows it.
      const res = await fetch(`/api/public/guides/${encodeURIComponent(slug)}`);
      if (res.status === 404) missing = true;
      else if (!res.ok) failed = true;
      else guide = (await res.json()) as PublicGuide;
    } catch {
      failed = true;
    }
  });
  $effect(() => {
    if (guide) document.title = `${guide.title} · POP`;
  });
  const imageUrl = (id: string) => `/api/public/guides/${encodeURIComponent(guide?.slug ?? slug)}/images/${encodeURIComponent(id)}`;
</script>

<main class="page" class:kiosk>
  <header>
    <a class="home" href={kiosk ? '/kiosk' : '/guides'}>
      <Logo />
      <span>{kiosk ? 'Back to the printers' : 'All guides'}</span>
    </a>
    {#if !kiosk}<a class="app" href="/">Printers</a>{/if}
  </header>
  {#if guide}
    <h1>{guide.title}</h1>
    <GuideView guide={guide} title={guide.title} {imageUrl} layout={kiosk ? 'kiosk' : 'page'} />
    {#if guide.license}
      <p class="license muted">Shared under {guide.license}.</p>
    {/if}
  {:else if missing}
    <h1>No such guide</h1>
    <p class="muted">It isn't published, or it doesn't exist. <a href="/guides">See every guide</a>.</p>
  {:else if failed}
    <h1>This guide didn't load</h1>
    <p class="muted">POP didn't answer. Try again in a moment.</p>
  {:else}
    <p class="muted" role="status">Loading…</p>
  {/if}
  <Version />
</main>

<style>
  .page {
    box-sizing: border-box;
    min-height: 100vh;
    max-width: 960px;
    margin: 0 auto;
    padding: 24px 16px 48px;
    display: flex;
    flex-direction: column;
    gap: 20px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  .page.kiosk {
    max-width: none;
    padding: 28px 56px 32px;
  }
  header {
    display: flex;
    flex-wrap: wrap;
  }
  .app {
    margin-left: auto;
    padding: 8px 0;
    color: var(--pop-butter);
    font-size: 16px;
  }
  .home {
    display: flex;
    align-items: center;
    gap: 12px;
    color: var(--pop-text);
    text-decoration: none;
    font-size: 18px;
    min-height: 48px;
  }
  h1 {
    margin: 0;
    font-size: 32px;
    font-weight: 600;
    text-wrap: balance;
  }
  .muted {
    color: var(--pop-muted);
  }
  a {
    color: var(--pop-butter);
  }
  .license {
    font-size: 15px;
  }
</style>

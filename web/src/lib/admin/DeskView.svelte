<script lang="ts">
  // The admin desk (#86), as given: Admin.svelte fetches it and acts. The
  // "New in BAM!" banners (#23), then every printer in BAM!'s category.
  import { Button, Icon, Logo, PrinterDrawing, StatusSquare } from '../design';
  import type { AdminPrinter } from '../generated/AdminPrinter';
  import type { GroupNotice } from '../generated/GroupNotice';
  import type { TowerPrinter } from '../generated/TowerPrinter';
  import type { User } from '../generated/User';
  import { connectsBy, fieldsSay, needsYou, now, offered, training, type Now } from './desk';

  let {
    me,
    printers,
    live,
    told = [],
    problem = null,
    busy = null,
    onadd = () => {},
    ondecline = () => {},
    onsignout = () => {},
  }: {
    me: User;
    /** Every printer in BAM!'s category; null while loading. */
    printers: AdminPrinter[] | null;
    /** The tower's printers, as the live stream has them. */
    live: TowerPrinter[];
    /** What the group chat was told about printers that need a hand (#55). */
    told?: GroupNotice[];
    /** What went wrong last, in POP's words. */
    problem?: string | null;
    /** The printer an action is waiting on, by BAM! id. */
    busy?: string | null;
    onadd?: (id: string) => void;
    ondecline?: (id: string) => void;
    onsignout?: () => void;
  } = $props();

  const TONE: Record<Now['tone'], string> = {
    needs_hand: 'var(--pop-coral)',
    printing: 'var(--pop-sage)',
    done: 'var(--pop-butter)',
    free: 'var(--pop-text)',
    offline: 'var(--pop-dim)',
    waiting: 'var(--pop-butter)',
    off: 'var(--pop-dim)',
  };
  const fresh = $derived(printers ? offered(printers) : []);
  const attention = $derived(needsYou(printers ?? [], live, told));
</script>

<div class="desk" data-testid="desk">
  <header>
    <div class="brand"><Logo size={40} /><span class="name">POP</span><span class="muted">Printer admin</span></div>
    <div class="who">
      <span class="muted">Signed in as {me.name}</span>
      <Button size="sm" onclick={onsignout}>Sign out</Button>
    </div>
  </header>

  {#if problem}
    <p class="problem" role="alert">{problem}</p>
  {/if}

  {#each fresh as p (p.bam_id)}
    <section class="banner" data-testid="new">
      <PrinterDrawing kind="enclosed" body="var(--pop-sage)" nozzle="var(--pop-butter)" size={64} part={false} label="A new printer" />
      <div class="say">
        <span class="title">New in BAM!: {p.name}</span>
        {#if p.ready}
          <span class="muted">{fieldsSay(p)}</span>
        {:else}
          <span class="muted">Fix it in BAM! before it can be added: {p.problems.map((x) => `${x.field}: ${x.problem}`).join('; ')}</span>
        {/if}
      </div>
      <Button variant="primary" size="sm" disabled={!p.ready || busy !== null} onclick={() => onadd(p.bam_id)}>Add to the tower</Button>
      <Button size="sm" disabled={busy !== null} onclick={() => ondecline(p.bam_id)}>Not this one</Button>
    </section>
  {/each}

  <div class="main">
  <section class="printers">
    <div class="head"><h2>Printers</h2><span class="muted">Everything in BAM!’s printer category</span></div>
    {#if printers === null}
      <p class="muted" role="status">Loading…</p>
    {:else if printers.length === 0}
      <p class="muted">BAM! has no printers in its printer category yet.</p>
    {:else}
      <div class="scroll">
        <table>
          <thead>
            <tr><th>Printer</th><th>BAM! category</th><th>Connects by</th><th>Training</th><th>Camera</th><th>Now</th></tr>
          </thead>
          <tbody>
            {#each printers as p (p.bam_id)}
              {@const n = now(p, live.find((t) => t.id === p.bam_id))}
              <tr data-testid="row">
                <td class="pname">{p.name}</td>
                <td class="muted">{p.categories.join(', ')}</td>
                <td>{connectsBy(p)}</td>
                <td>{training(p)}</td>
                <td>{p.camera ? 'Yes' : 'No'}</td>
                <td><span style:color={TONE[n.tone]}><StatusSquare color={TONE[n.tone]} label={n.text} /></span></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    <span class="muted foot">Change a printer’s category or fields in BAM! and POP follows.</span>
  </section>

  <aside class="needs" aria-labelledby="needs-you">
    <h2 id="needs-you">Needs you</h2>
    {#each attention as a (a.key)}
      <div class="item" data-testid="attention" data-kind={a.kind}>
        <Icon name="warn" size={22} color={a.kind === 'needs_hand' ? 'var(--pop-coral)' : 'var(--pop-butter)'} />
        <div class="say"><span class="what">{a.title}</span><span class="muted">{a.detail}</span></div>
      </div>
    {:else}
      <p class="muted" data-testid="all-clear">Nothing needs you right now.</p>
    {/each}
  </aside>
  </div>
</div>

<style>
  .desk {
    box-sizing: border-box;
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    gap: 22px;
    padding: 28px max(16px, 3vw) 40px;
    background: var(--pop-ground);
    color: var(--pop-text);
    font-family: var(--pop-font);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 16px;
  }
  .brand,
  .who {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .name {
    font-size: 22px;
    font-weight: 600;
  }
  .muted {
    color: var(--pop-muted);
  }
  .problem {
    margin: 0;
    padding: 10px 14px;
    background: var(--pop-coral);
    color: var(--pop-ink);
    border-radius: 4px;
  }
  .banner {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 18px;
    padding: 16px 22px;
    background: var(--pop-panel);
    border: 3px solid var(--pop-ink);
    border-radius: var(--pop-radius);
    box-shadow: var(--pop-shadow);
  }
  .say {
    flex: 1;
    min-width: 16rem;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 15px;
  }
  .title {
    font-size: 19px;
    font-weight: 600;
  }
  .main {
    display: flex;
    flex-wrap: wrap;
    gap: 22px;
    align-items: flex-start;
  }
  .main > .printers {
    flex: 1 1 40rem;
    min-width: 0;
  }
  .needs {
    flex: 0 1 26rem;
    min-width: 16rem;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .item {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    padding: 12px 14px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .item .say {
    min-width: 0;
    font-size: 13.5px;
  }
  .what {
    font-size: 16px;
    font-weight: 500;
  }
  .printers {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 20px 22px;
    background: var(--pop-panel);
    border-radius: var(--pop-radius);
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 8px;
    font-size: 14px;
  }
  h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
  }
  .scroll {
    overflow-x: auto;
  }
  table {
    border-collapse: collapse;
    width: 100%;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 14px;
    color: var(--pop-muted);
    padding: 0 12px 10px;
    white-space: nowrap;
  }
  td {
    padding: 13px 12px;
    border-top: 2px solid var(--pop-panel-2);
    font-size: 16px;
  }
  .pname {
    font-weight: 500;
  }
  .foot {
    font-size: 13.5px;
  }
</style>

<!--
  The design system at a glance (#48), at /design: every component of the
  round 6 look, with the kind of content the screens will give it.
-->
<script lang="ts">
  import {
    Button,
    Chip,
    Fern,
    Icon,
    Logo,
    Panel,
    Pine,
    PrinterDrawing,
    ProgressBar,
    Shroom,
    Spool,
    StatusSquare,
    icons,
    type IconName,
  } from './lib/design';

  let progress = $state(0.42);
  const printers = [
    { name: 'X1', kind: 'enclosed' as const, body: '#F7B294', nozzle: '#F1DB98', part: true, state: 'Printing' },
    { name: 'MK4', kind: 'open' as const, body: '#C3D7BB', nozzle: '#F7B294', part: true, state: 'Printing' },
    { name: 'P1S', kind: 'enclosed' as const, body: '#7fb6d9', nozzle: '#FEF7E4', part: false, state: 'Ready' },
    { name: 'Ender', kind: 'open' as const, body: '#c9a0dc', nozzle: '#F1DB98', part: false, state: 'Needs a hand' },
  ];
  const spools = [
    { name: 'Coral PLA', color: '#F7B294', frac: 1 },
    { name: 'Sky PETG', color: '#7fb6d9', frac: 0.45 },
    { name: 'Sage PLA', color: '#C3D7BB', frac: 0.1 },
  ];
</script>

<main>
  <header>
    <Logo />
    <h1>Design system</h1>
    <p class="muted">Round 6, as components. Every color, drawing and control the screens use.</p>
  </header>

  <section>
    <h2>Buttons</h2>
    <div class="row">
      <Button variant="primary">Send it to the X1</Button>
      <Button variant="secondary">Tag out</Button>
      <Button>Cancel</Button>
      <Button variant="primary" size="sm">Confirm</Button>
      <Button variant="secondary" size="sm" aria-label="Back"><Icon name="back" /></Button>
      <Button disabled>Not now</Button>
    </div>
  </section>

  <section>
    <h2>Printers</h2>
    <div class="row">
      {#each printers as p (p.name)}
        <Panel>
          <div class="printer">
            <PrinterDrawing kind={p.kind} body={p.body} nozzle={p.nozzle} part={p.part} label="{p.name}, {p.state}" />
            <strong>{p.name}</strong>
            <StatusSquare
              color={p.state === 'Printing' ? 'var(--pop-moss)' : p.state === 'Ready' ? 'var(--pop-sage)' : 'var(--pop-coral)'}
              label={p.state}
            />
          </div>
        </Panel>
      {/each}
    </div>
  </section>

  <section>
    <h2>Progress</h2>
    <Panel variant="flat">
      <div class="stack">
        <div class="between"><span>Benchy on the X1</span><span class="muted">{Math.round(progress * 100)}%</span></div>
        <ProgressBar value={progress} label="Benchy on the X1" />
        <div class="row">
          <Button size="sm" onclick={() => (progress = Math.max(0, progress - 0.1))}>Back 10%</Button>
          <Button size="sm" onclick={() => (progress = Math.min(1, progress + 0.1))}>Forward 10%</Button>
        </div>
      </div>
    </Panel>
  </section>

  <section>
    <h2>Filament</h2>
    <div class="row">
      {#each spools as s (s.name)}
        <div class="spool">
          <Spool color={s.color} frac={s.frac} label="{s.name}, {Math.round(s.frac * 100)}% left" />
          <span>{s.name}</span>
        </div>
      {/each}
    </div>
  </section>

  <section>
    <h2>Chips and status</h2>
    <div class="row">
      <Chip>PLA</Chip>
      <Chip>0.4 mm</Chip>
      <Chip>Yours</Chip>
      <StatusSquare color="var(--pop-moss)" label="Printing" />
      <StatusSquare color="var(--pop-coral)" label="Needs a hand" />
      <StatusSquare color="var(--pop-sage)" label="Ready" />
    </div>
  </section>

  <section>
    <h2>Icons</h2>
    <div class="row">
      {#each Object.keys(icons) as name (name)}
        <span class="icon"><Icon name={name as IconName} /> <span class="muted">{name}</span></span>
      {/each}
    </div>
  </section>

  <section>
    <h2>The forest</h2>
    <Panel variant="flat">
      <svg viewBox="0 0 660 240" class="forest" role="img" aria-label="Pines, ferns and mushrooms">
        <Pine x={80} b={230} h={170} fill="#2a3a3a" />
        <Pine x={180} b={230} h={210} fill="#33483a" />
        <Pine x={300} b={230} h={140} fill="#C3D7BB" ink />
        <Fern x={390} b={230} h={70} color="#C3D7BB" />
        <Fern x={440} b={230} h={40} color="#33483a" />
        <Shroom x={530} b={230} s={22} cap="#F7B294" />
        <Shroom x={580} b={230} s={14} cap="#F1DB98" />
      </svg>
    </Panel>
  </section>
</main>

<style>
  main {
    padding: 32px 40px;
    display: flex;
    flex-direction: column;
    gap: 32px;
    max-width: 1200px;
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h1 {
    font-size: 32px;
    font-weight: 600;
  }
  h2 {
    font-size: 22px;
    font-weight: 600;
    margin-bottom: 14px;
  }
  .muted {
    color: var(--pop-muted);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 16px;
  }
  .printer {
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }
  .stack {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .between {
    display: flex;
    justify-content: space-between;
  }
  .spool,
  .icon {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .forest {
    display: block;
    width: 100%;
    max-width: 660px;
    background: var(--pop-ground);
  }
</style>

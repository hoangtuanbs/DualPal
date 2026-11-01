<script lang="ts">
  import { api, errorMessage, type Insights } from "./api";
  import { formatBytes, formatDate } from "./format";

  /** Charts and totals of the work done over the last days. */
  let { nameOf, onclose }: { nameOf: (fs: string) => string; onclose: () => void } = $props();

  let days = $state(14);
  let data = $state<Insights | null>(null);
  let error = $state<string | null>(null);
  let metric = $state<"bytes" | "operations">("bytes");

  $effect(() => {
    const d = days;
    api.activityInsights(d).then(
      (i) => {
        data = i;
        error = null;
      },
      (e) => (error = errorMessage(e)),
    );
  });

  const KIND: Record<string, string> = {
    copy: "Copy",
    move: "Move",
    trash: "Trash",
    delete: "Delete",
    compress: "Compress",
    setMode: "Permissions",
    createDir: "New folder",
    rename: "Rename",
    batchRename: "Multi Rename",
    sync: "Sync",
  };

  const peak = $derived(Math.max(1, ...(data?.days ?? []).map((d) => (metric === "bytes" ? d.bytes : d.operations))));
  const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric" });

  function duration(ms: number) {
    const s = Math.round(ms / 1000);
    if (s < 60) return `${s} s`;
    const m = Math.round(s / 60);
    if (m < 60) return `${m} min`;
    return `${(m / 60).toFixed(1)} h`;
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="backdrop">
  <div class="sheet" role="dialog" aria-modal="true" aria-label="Activity insights">
    <header>
      <h2>Activity insights</h2>
      <div class="seg" role="group" aria-label="Period">
        {#each [7, 14, 30, 90] as d (d)}
          <button aria-pressed={days === d} onclick={() => (days = d)}>{d} days</button>
        {/each}
      </div>
      <span class="spacer"></span>
      <!-- svelte-ignore a11y_autofocus -->
      <button class="close" onclick={onclose} aria-label="Close" autofocus>×</button>
    </header>

    {#if error}
      <p class="empty err">{error}</p>
    {:else if !data}
      <p class="empty">Loading…</p>
    {:else if data.operations === 0}
      <p class="empty">Nothing done in the last {days} days. Copies, moves, syncs and other operations show up here.</p>
    {:else}
      <div class="body">
        <div class="tiles">
          <div class="tile"><span class="big">{data.operations}</span><span>operations</span></div>
          <div class="tile"><span class="big">{formatBytes(data.bytes)}</span><span>transferred</span></div>
          <div class="tile"><span class="big">{data.files.toLocaleString()}</span><span>items handled</span></div>
          <div class="tile">
            <span class="big">{data.throughput ? `${formatBytes(data.throughput)}/s` : "—"}</span><span>average transfer speed</span>
          </div>
          <div class="tile" class:bad={data.failed > 0}>
            <span class="big">{Math.round((data.completed / data.operations) * 100)}%</span>
            <span>succeeded ({data.failed} failed, {data.cancelled} cancelled)</span>
          </div>
          <div class="tile"><span class="big">{duration(data.busyMs)}</span><span>spent working</span></div>
        </div>

        <section>
          <div class="section-head">
            <h3>Per day</h3>
            <div class="seg small" role="group" aria-label="Measure">
              <button aria-pressed={metric === "bytes"} onclick={() => (metric = "bytes")}>Data</button>
              <button aria-pressed={metric === "operations"} onclick={() => (metric = "operations")}>Operations</button>
            </div>
          </div>
          <div class="chart" role="img" aria-label="Activity per day">
            {#each data.days as d (d.startMs)}
              {@const v = metric === "bytes" ? d.bytes : d.operations}
              <div class="col" title="{formatDate(d.startMs).split(',')[0]}: {d.operations} operations, {formatBytes(d.bytes)}{d.failed ? `, ${d.failed} failed` : ''}">
                <div class="bar-wrap">
                  <div class="bar" class:has-failed={d.failed > 0} style="height: {(v / peak) * 100}%"></div>
                </div>
                {#if data.days.length <= 31}<span class="day">{dayFmt.format(new Date(d.startMs))}</span>{/if}
              </div>
            {/each}
          </div>
        </section>

        <div class="split">
          <section>
            <h3>By kind</h3>
            {#each data.byKind as g (g.key)}
              <div class="line">
                <span class="label">{KIND[g.key] ?? g.key}</span>
                <span class="meter"><span style="width: {(g.operations / data.operations) * 100}%"></span></span>
                <span class="val">{g.operations}{g.bytes ? ` · ${formatBytes(g.bytes)}` : ""}{g.failed ? ` · ${g.failed} failed` : ""}</span>
              </div>
            {/each}
          </section>
          <section>
            <h3>By place</h3>
            {#each data.byPlace as g (g.key)}
              <div class="line">
                <span class="label">{nameOf(g.key)}</span>
                <span class="meter"><span style="width: {(g.operations / data.operations) * 100}%"></span></span>
                <span class="val">{g.operations}{g.bytes ? ` · ${formatBytes(g.bytes)}` : ""}</span>
              </div>
            {/each}
          </section>
        </div>

        <div class="split">
          <section>
            <h3>Largest transfers</h3>
            {#each data.largest as r (r.id)}
              <div class="rec"><span class="label" title={r.summary}>{r.summary}</span><span class="val">{formatBytes(r.bytesDone)}</span></div>
            {:else}
              <p class="muted">None yet.</p>
            {/each}
          </section>
          <section>
            <h3>Recent failures</h3>
            {#each data.failures as r (r.id)}
              <div class="rec">
                <span class="label" title="{r.summary}: {r.error}">{r.summary}<br /><small>{r.error}</small></span>
                <span class="val">{formatDate(r.finishedMs)}</span>
              </div>
            {:else}
              <p class="muted">No failures. 🎉</p>
            {/each}
          </section>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
    z-index: 9;
  }
  .sheet {
    width: min(860px, calc(100vw - 48px));
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  h3 {
    margin: 0 0 8px;
    font-size: 12px;
    color: var(--muted);
  }
  .spacer {
    flex: 1;
  }
  .close {
    font-size: 16px;
    padding: 2px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  .seg {
    display: flex;
    gap: 1px;
    border-radius: 7px;
    overflow: hidden;
    background: var(--border);
    font-size: 12px;
  }
  .seg button {
    padding: 3px 10px;
    background: var(--control);
  }
  .seg button[aria-pressed="true"] {
    background: var(--accent);
    color: white;
  }
  .body {
    overflow-y: auto;
    padding: 14px 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 10px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--stripe);
    font-size: 12px;
    color: var(--muted);
  }
  .tile .big {
    font-size: 20px;
    font-weight: 600;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .tile.bad .big {
    color: var(--danger);
  }
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .section-head h3 {
    margin: 0;
  }
  .chart {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    height: 150px;
  }
  .col {
    flex: 1;
    min-width: 0;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
  }
  .bar-wrap {
    flex: 1;
    width: 100%;
    display: flex;
    align-items: flex-end;
  }
  .bar {
    width: 100%;
    min-height: 2px;
    border-radius: 3px 3px 0 0;
    background: var(--accent);
  }
  .bar.has-failed {
    background: linear-gradient(to top, var(--danger) 0 4px, var(--accent) 4px);
  }
  .day {
    font-size: 10px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    max-width: 100%;
  }
  .split {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 24px;
  }
  .line,
  .rec {
    display: grid;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    padding: 3px 0;
  }
  .line {
    grid-template-columns: 110px 1fr auto;
  }
  .rec {
    grid-template-columns: minmax(0, 1fr) auto;
    border-bottom: 1px solid var(--stripe);
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rec small {
    color: var(--danger);
  }
  .meter {
    height: 6px;
    border-radius: 3px;
    background: var(--stripe);
    overflow: hidden;
  }
  .meter span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .val {
    color: var(--muted);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    padding: 32px;
    text-align: center;
    color: var(--muted);
  }
  .err {
    color: var(--danger);
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
    margin: 0;
  }
</style>

<script lang="ts">
  import type { ActivityRecord, ConflictPolicy, OperationInfo } from "./api";
  import { formatBytes, formatTime } from "./format";

  let {
    operations,
    activity,
    oncancel,
    onretry,
    onclear,
    onclearactivity,
  }: {
    operations: OperationInfo[];
    activity: ActivityRecord[];
    oncancel: (id: string) => void;
    onretry: (id: string, conflict: ConflictPolicy) => void;
    onclear: () => void;
    onclearactivity: () => void;
  } = $props();

  let tab = $state<"operations" | "activity">("operations");
  const running = $derived(operations.filter((o) => !isFinished(o)).length);

  function isFinished(o: { status: string }) {
    return o.status === "completed" || o.status === "failed" || o.status === "cancelled";
  }

  function percent(o: OperationInfo) {
    const p = o.progress;
    if (p.bytesTotal > 0) return Math.min(100, (p.bytesDone / p.bytesTotal) * 100);
    if (p.filesTotal > 0) return Math.min(100, (p.filesDone / p.filesTotal) * 100);
    return o.status === "completed" ? 100 : 0;
  }

  function detail(o: OperationInfo) {
    const p = o.progress;
    const parts: string[] = [];
    if (p.filesTotal) parts.push(`${p.filesDone} of ${p.filesTotal} ${p.filesTotal === 1 ? "item" : "items"}`);
    if (p.bytesTotal) parts.push(`${formatBytes(p.bytesDone)} of ${formatBytes(p.bytesTotal)}`);
    if (p.skipped) parts.push(`${p.skipped} skipped`);
    return parts.join(" · ");
  }

  const statusLabel: Record<string, string> = {
    queued: "Waiting",
    running: "Running",
    completed: "Done",
    failed: "Failed",
    cancelled: "Cancelled",
  };
</script>

<section class="panel">
  <div class="tabs" role="tablist">
    <button role="tab" aria-selected={tab === "operations"} onclick={() => (tab = "operations")}>
      Operations {#if running}<span class="count">{running}</span>{/if}
    </button>
    <button role="tab" aria-selected={tab === "activity"} onclick={() => (tab = "activity")}>Activity</button>
    <span class="spacer"></span>
    {#if tab === "operations"}
      <button class="link" onclick={onclear} disabled={!operations.some(isFinished)}>Clear finished</button>
    {:else}
      <button class="link" onclick={onclearactivity} disabled={activity.length === 0}>Clear history</button>
    {/if}
  </div>

  <div class="body">
    {#if tab === "operations"}
      {#if operations.length === 0}
        <p class="empty">No operations yet. Copy (F5), move (F6) or trash (F8) files to see their progress here.</p>
      {/if}
      {#each [...operations].reverse() as op (op.id)}
        <div class="op {op.status}">
          <div class="line">
            <span class="summary" title={op.summary}>{op.summary}</span>
            <span class="status">{statusLabel[op.status]}</span>
            {#if !isFinished(op)}
              <button class="link" onclick={() => oncancel(op.id)}>Cancel</button>
            {/if}
          </div>
          {#if op.status === "running" || op.status === "queued"}
            <div class="bar"><div style:width="{percent(op)}%"></div></div>
          {/if}
          <div class="meta">
            {#if op.error}
              <span class="error">{op.error.message}</span>
              {#if op.error.kind === "alreadyExists" && (op.kind === "copy" || op.kind === "move")}
                <button class="link" onclick={() => onretry(op.id, "overwrite")}>Replace existing</button>
                <button class="link" onclick={() => onretry(op.id, "skip")}>Skip existing</button>
              {/if}
            {:else if op.progress.current && op.status === "running"}
              <span class="current" title={op.progress.current}>{op.progress.current}</span>
            {/if}
            <span class="detail">{detail(op)}</span>
          </div>
        </div>
      {/each}
    {:else}
      {#if activity.length === 0}
        <p class="empty">Finished operations are listed here.</p>
      {/if}
      <table>
        <tbody>
          {#each activity as r (r.id)}
            <tr class={r.status}>
              <td class="time">{formatTime(r.finishedMs)}</td>
              <td class="summary" title={r.error ?? r.summary}>{r.summary}</td>
              <td class="status">{statusLabel[r.status]}</td>
              <td class="num">{r.filesDone} {r.filesDone === 1 ? "item" : "items"}</td>
              <td class="num">{r.bytesDone ? formatBytes(r.bytesDone) : ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-top: 1px solid var(--border);
    background: var(--chrome);
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
  }
  .tabs [role="tab"] {
    padding: 3px 10px;
    border-radius: 6px;
    font-weight: 500;
    color: var(--muted);
  }
  .tabs [aria-selected="true"] {
    background: var(--control);
    color: var(--text);
  }
  .count {
    display: inline-block;
    min-width: 16px;
    padding: 0 4px;
    margin-left: 4px;
    border-radius: 8px;
    background: var(--accent);
    color: white;
    font-size: 11px;
  }
  .spacer {
    flex: 1;
  }
  .link {
    color: var(--accent);
    font-size: 12px;
    padding: 2px 4px;
  }
  .link:disabled {
    color: var(--muted);
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 4px 12px 8px;
  }
  .empty {
    color: var(--muted);
    margin: 8px 0;
  }
  .op {
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .line,
  .meta {
    display: flex;
    gap: 10px;
    align-items: baseline;
    min-width: 0;
  }
  .summary,
  .current {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 12px;
    color: var(--muted);
  }
  .status {
    font-size: 12px;
    color: var(--muted);
  }
  .failed .status,
  .error {
    color: var(--danger);
  }
  .completed .status {
    color: var(--success);
  }
  .error {
    flex: 1;
  }
  .bar {
    height: 4px;
    margin: 5px 0 3px;
    border-radius: 2px;
    background: var(--control);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
    transition: width 0.1s linear;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    table-layout: fixed;
  }
  td {
    padding: 4px 6px 4px 0;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .time {
    width: 80px;
    color: var(--muted);
  }
  td.status {
    width: 70px;
  }
  .num {
    width: 80px;
    text-align: right;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
</style>

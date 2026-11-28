<script lang="ts">
  import type { ActivityRecord, ConflictPolicy, FailedItem, OperationInfo } from "./api";
  import { KB, MB, failureText, formatRate, type TransferDefaults } from "./transfers";
  import { copyText } from "./about";
  import { formatBytes, formatTime } from "./format";
  import { fsNames } from "./fs-names.svelte";

  let {
    operations,
    activity,
    oncancel,
    onretry,
    onclear,
    onclearactivity,
    oninsights,
    onpause = () => {},
    onresume = () => {},
    queuePaused = false,
    onpauseall = () => {},
    bandwidthLimit = 0,
    onlimit = () => {},
    onoplimit = () => {},
    defaults = { verify: false, keepGoing: false },
    ondefaults = () => {},
    loadfailures = async () => [],
    onsavefailures = () => {},
    onretryfailed = () => {},
  }: {
    operations: OperationInfo[];
    activity: ActivityRecord[];
    oncancel: (id: string) => void;
    onretry: (id: string, conflict: ConflictPolicy) => void;
    onclear: () => void;
    onclearactivity: () => void;
    oninsights: () => void;
    onpause?: (id: string) => void;
    onresume?: (id: string) => void;
    /** The whole queue is paused. */
    queuePaused?: boolean;
    onpauseall?: (paused: boolean) => void;
    /** Global bandwidth limit in bytes per second, 0 for none. */
    bandwidthLimit?: number;
    onlimit?: (bytesPerSec: number) => void;
    /** Ask for this operation's own limit. */
    onoplimit?: (id: string) => void;
    /** Options for new copies and moves. */
    defaults?: TransferDefaults;
    ondefaults?: (d: TransferDefaults) => void;
    loadfailures?: (id: string) => Promise<FailedItem[]>;
    onsavefailures?: (id: string) => void;
    onretryfailed?: (id: string) => void;
  } = $props();

  let tab = $state<"operations" | "activity">("operations");
  const running = $derived(operations.filter((o) => !isFinished(o)).length);

  // The limit field shows the saved limit until the user types.
  let limitUnit = $state<"KB/s" | "MB/s">("KB/s");
  // A number input binds a number, or null when empty.
  let limitText = $state<number | string | null>("");
  $effect(() => {
    const b = bandwidthLimit;
    if (!b) {
      limitText = "";
    } else if (b % MB === 0) {
      limitUnit = "MB/s";
      limitText = String(b / MB);
    } else {
      limitUnit = "KB/s";
      limitText = String(Math.round((b / KB) * 10) / 10);
    }
  });

  function applyLimit() {
    const n = Number(String(limitText ?? "").trim() || "0");
    if (!Number.isFinite(n) || n < 0) return;
    onlimit(Math.round(n * (limitUnit === "MB/s" ? MB : KB)));
  }

  /** Failed items of each operation whose list is open. */
  let failures = $state<Record<string, FailedItem[] | "loading">>({});
  let copied = $state<string | null>(null);

  async function toggleFailures(id: string) {
    if (failures[id]) {
      delete failures[id];
      return;
    }
    failures[id] = "loading";
    try {
      failures[id] = await loadfailures(id);
    } catch {
      failures[id] = [];
    }
  }

  async function copyFailures(id: string) {
    const list = failures[id];
    if (!Array.isArray(list)) return;
    copied = (await copyText(failureText(list))) ? id : null;
  }

  function isFinished(o: { status: string }) {
    return o.status === "completed" || o.status === "failed" || o.status === "cancelled";
  }

  function isTransfer(o: OperationInfo) {
    return o.kind === "copy" || o.kind === "move";
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
    const limit = o.options?.limit;
    if (!isFinished(o) && limit != null) parts.push(limit === 0 ? "no limit" : `limit ${formatRate(limit)}`);
    if (o.options?.verify) parts.push("checked");
    return parts.join(" · ");
  }

  const statusLabel: Record<string, string> = {
    queued: "Waiting",
    running: "Running",
    paused: "Paused",
    completed: "Done",
    failed: "Failed",
    cancelled: "Cancelled",
  };

  function status(o: OperationInfo) {
    if (o.waitingFor && !isFinished(o)) return `Waiting for ${fsNames[o.waitingFor] ?? o.waitingFor}`;
    if (queuePaused && o.status === "queued") return "Paused";
    const n = o.failed ?? 0;
    if (o.status === "completed" && n > 0) return `Done with ${n} ${n === 1 ? "error" : "errors"}`;
    return statusLabel[o.status];
  }
</script>
<section class="panel" data-testid="ops-panel">
  <div class="tabs" role="tablist">
    <button role="tab" aria-selected={tab === "operations"} onclick={() => (tab = "operations")}>
      Operations {#if running}<span class="count">{running}</span>{/if}
    </button>
    <button role="tab" aria-selected={tab === "activity"} onclick={() => (tab = "activity")}>Activity</button>
    <span class="spacer"></span>
    {#if tab === "operations"}
      <button class="link" onclick={() => onpauseall(!queuePaused)} data-testid="ops-pause-all">
        {queuePaused ? "Resume all" : "Pause all"}
      </button>
      <button class="link" onclick={onclear} disabled={!operations.some(isFinished)}>Clear finished</button>
    {:else}
      <button class="link" onclick={oninsights}>Insights…</button>
      <button class="link" onclick={onclearactivity} disabled={activity.length === 0}>Clear history</button>
    {/if}
  </div>

  {#if tab === "operations"}
    <div class="options" data-testid="ops-options">
      <label class="limit">
        Limit
        <input
          type="number"
          min="0"
          step="any"
          placeholder="Off"
          aria-label="Bandwidth limit"
          bind:value={limitText}
          onchange={applyLimit}
        />
        <select aria-label="Limit unit" bind:value={limitUnit} onchange={() => limitText !== "" && limitText != null && applyLimit()}>
          <option>KB/s</option>
          <option>MB/s</option>
        </select>
      </label>
      <label>
        <input
          type="checkbox"
          checked={defaults.verify}
          onchange={(e) => ondefaults({ ...defaults, verify: e.currentTarget.checked })}
        />
        Verify copies
      </label>
      <label>
        <input
          type="checkbox"
          checked={defaults.keepGoing}
          onchange={(e) => ondefaults({ ...defaults, keepGoing: e.currentTarget.checked })}
        />
        Don't stop on errors
      </label>
    </div>
  {/if}

  <div class="body">
    {#if tab === "operations"}
      {#if operations.length === 0}
        <p class="empty">No operations yet. Copy (F5), move (F6) or trash (F8) files to see their progress here.</p>
      {/if}
      {#each [...operations].reverse() as op (op.id)}
        {@const failedList = failures[op.id]}
        <div class="op {op.status}" class:some-failed={(op.failed ?? 0) > 0}>
          <div class="line">
            <span class="summary" title={op.summary}>{op.summary}</span>
            <span class="status">{status(op)}</span>
            {#if op.status === "paused"}
              <button class="link" onclick={() => onresume(op.id)}>Resume</button>
            {:else if !isFinished(op) && isTransfer(op)}
              <button class="link" onclick={() => onpause(op.id)}>Pause</button>
            {/if}
            {#if !isFinished(op) && isTransfer(op) && !op.restored}
              <button class="link" onclick={() => onoplimit(op.id)}>Limit…</button>
            {/if}
            {#if !isFinished(op)}
              <button class="link" onclick={() => oncancel(op.id)}>{op.restored ? "Discard" : "Cancel"}</button>
            {/if}
          </div>
          {#if op.status === "running" || op.status === "queued" || op.status === "paused"}
            <div class="bar"><div style:width="{percent(op)}%"></div></div>
          {/if}
          <div class="meta">
            {#if op.error}
              <span class="error">{op.error.message}</span>
              {#if op.error.kind === "alreadyExists" && isTransfer(op)}
                <button class="link" onclick={() => onretry(op.id, "overwrite")}>Replace existing</button>
                <button class="link" onclick={() => onretry(op.id, "skip")}>Skip existing</button>
              {/if}
            {:else if op.restored && op.status === "paused"}
              <span class="current">Not finished when Dual Pal closed</span>
            {:else if op.progress.current && op.status === "running"}
              <span class="current" title={op.progress.current}>{op.progress.current}</span>
            {/if}
            {#if (op.failed ?? 0) > 0}
              <button class="link" onclick={() => toggleFailures(op.id)} aria-expanded={!!failedList}>
                {failedList ? "Hide errors" : "Show errors"}
              </button>
            {/if}
            <span class="detail">{detail(op)}</span>
          </div>
          {#if failedList}
            <div class="failures" data-testid="ops-failures">
              {#if failedList === "loading"}
                <p class="empty">Loading…</p>
              {:else}
                <ul>
                  {#each failedList as f, i (i)}
                    <li><span class="path" title={f.source.path}>{f.source.path}</span> <span class="error">{f.error.message}</span></li>
                  {/each}
                </ul>
                <div class="actions">
                  <button class="link" onclick={() => copyFailures(op.id)}>{copied === op.id ? "Copied" : "Copy list"}</button>
                  <button class="link" onclick={() => onsavefailures(op.id)}>Save list…</button>
                  {#if isFinished(op)}
                    <button class="link" onclick={() => onretryfailed(op.id)}>Retry failed</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
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
  .some-failed .status,
  .error {
    color: var(--danger);
  }
  .paused .status {
    color: var(--muted);
  }
  .options {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 14px;
    padding: 3px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    color: var(--muted);
  }
  .options label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .options input[type="number"] {
    width: 64px;
  }
  .failures {
    margin: 4px 0 2px;
    font-size: 12px;
  }
  .failures ul {
    margin: 0;
    padding: 0;
    list-style: none;
    max-height: 140px;
    overflow: auto;
  }
  .failures li {
    display: flex;
    gap: 8px;
    min-width: 0;
    padding: 1px 0;
  }
  .failures .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .failures .error {
    flex: none;
    max-width: 50%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 6px;
    margin-top: 2px;
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

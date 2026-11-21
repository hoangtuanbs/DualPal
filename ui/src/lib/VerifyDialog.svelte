<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorKind, errorMessage, type ChecksumAlgorithm, type ChecksumProgress, type Location, type VerifyEntry, type VerifyReport } from "./api";
  import { checksumAlgorithmOf, sortVerifyEntries } from "./checksum";

  /** Check the files listed in the checksum file `file` against their digests. */
  let {
    file,
    onclose,
  }: {
    file: Location;
    onclose: () => void;
  } = $props();

  const name = $derived(file.path.split(/[\\/]/).pop() ?? file.path);
  const algorithm = $derived(checksumAlgorithmOf(name));

  let report = $state<VerifyReport | null>(null);
  let progress = $state<ChecksumProgress | null>(null);
  let running = $state(false);
  let error = $state("");
  // Each run gets a new request id; progress and answers for older runs are ignored.
  let latest = 0;

  const sorted = $derived<VerifyEntry[]>(report ? sortVerifyEntries(report.entries) : []);
  const label: Record<string, string> = { failed: "FAILED", missing: "MISSING", ok: "OK" };

  onMount(() => {
    const off = api.onChecksumProgress((p) => {
      if (p.request === latest && running) progress = p;
    });
    if (algorithm) start(algorithm);
    else error = "Not a checksum file. Use a .md5, .sha1, .sha256 or .xxh128 file.";
    return () => {
      if (running) void api.cancelChecksum(latest);
      off.then((f) => f()).catch(() => {});
    };
  });

  function start(alg: ChecksumAlgorithm) {
    const id = ++latest;
    report = null;
    progress = null;
    error = "";
    running = true;
    api
      .checksumVerify(file, alg, id)
      .then((found) => {
        if (id === latest) {
          report = found;
          running = false;
        }
      })
      .catch((e) => {
        if (id === latest && errorKind(e) !== "cancelled") error = errorMessage(e);
      })
      .finally(() => {
        if (id === latest) running = false;
      });
  }

  function stop() {
    if (!running) return;
    void api.cancelChecksum(latest);
    latest++;
    running = false;
    progress = null;
    error = "Stopped. The results so far are not complete.";
  }

  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      if (running) stop();
      else onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="scrim" role="presentation" onclick={onclose}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Verify checksums" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={keydown}>
    <header>
      <h2>Verify Checksums</h2>
      <span class="scope" title={file.path}>{name}{algorithm ? ` (${algorithm.toUpperCase()})` : ""}</span>
    </header>

    <div class="status" role="status" aria-live="polite">
      {#if error}
        <span class="error">{error}</span>
      {:else if running}
        <span class="spinner" aria-hidden="true"></span>
        {#if progress}
          Checking {progress.filesDone} of {progress.filesTotal}
          {#if progress.current}· {progress.current}{/if}
        {:else}
          Reading the list…
        {/if}
        <button class="link" onclick={stop}>Cancel</button>
      {:else if report}
        <span class="counts">
          <strong class="ok-count">{report.ok} OK</strong>
          · <strong class:bad={report.failed > 0}>{report.failed} failed</strong>
          · <strong class:bad={report.missing > 0}>{report.missing} missing</strong>
        </span>
      {/if}
    </div>
    {#if running && progress}
      <progress class="bar" max={Math.max(progress.filesTotal, 1)} value={progress.filesDone}></progress>
    {/if}

    {#if sorted.length}
      <ul class="rows" aria-label="Results">
        {#each sorted as e (e.path)}
          <li class={e.status}>
            <span class="badge">{label[e.status]}</span>
            <span class="path" title={e.path}>{e.path}</span>
            {#if e.error}
              <span class="detail error">{e.error}</span>
            {:else if e.status === "failed" && e.actual}
              <span class="detail">is {e.actual}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if report && !report.entries.length}
      <p class="empty">The checksum file lists no files.</p>
    {/if}

    <footer>
      <button type="button" class="primary" onclick={onclose}>Close</button>
    </footer>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.2);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 8vh;
    z-index: 11;
  }
  .dialog {
    width: min(720px, calc(100vw - 32px));
    max-height: 84vh;
    display: flex;
    flex-direction: column;
    background: var(--pane-bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 14px 16px 8px;
    min-width: 0;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    white-space: nowrap;
  }
  .scope {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--muted);
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 16px 8px;
    font-size: 13px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .counts strong {
    color: var(--text);
    font-weight: 600;
  }
  .counts strong.ok-count {
    color: var(--accent);
  }
  .counts strong.bad {
    color: var(--danger);
  }
  .error {
    color: var(--danger);
    white-space: normal;
  }
  .link {
    border: none;
    background: none;
    padding: 0 2px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .spinner {
    width: 10px;
    height: 10px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .bar {
    display: block;
    width: calc(100% - 32px);
    margin: 0 16px 8px;
    height: 6px;
    accent-color: var(--accent);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 6px;
    min-height: 120px;
    max-height: 56vh;
    overflow-y: auto;
    border-top: 1px solid var(--border);
  }
  .rows li {
    display: grid;
    grid-template-columns: 72px minmax(0, 1fr) minmax(0, 1fr);
    gap: 10px;
    align-items: center;
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 13px;
  }
  .badge {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--accent);
  }
  .rows li.failed .badge,
  .rows li.missing .badge {
    color: var(--danger);
  }
  .rows li.ok .badge {
    color: var(--muted);
  }
  .path,
  .detail {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .detail {
    font-size: 12px;
    color: var(--muted);
  }
  .empty {
    margin: 0;
    padding: 10px 16px;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
  footer {
    display: flex;
    justify-content: flex-end;
    padding: 10px 16px 14px;
  }
  button {
    padding: 5px 14px;
    border-radius: 6px;
    font-weight: 500;
  }
  .primary {
    background: var(--accent);
    color: white;
  }
</style>

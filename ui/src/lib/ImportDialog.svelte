<script lang="ts">
  import { api, errorMessage, type ImportReport, type ImportSource, type ImportedConnection } from "./api";
  import {
    allSelected,
    chosen,
    describeEndpoint,
    importButtonLabel,
    passwordNote,
    summary,
    toggled,
  } from "./import-selection";

  /** Saved servers are added to the list; `onclose` closes the dialog. */
  let {
    onsaved,
    onclose,
  }: {
    /** Called after each server is saved, so the list can reload. */
    onsaved: () => void;
    onclose: () => void;
  } = $props();

  const SOURCES: { id: ImportSource; label: string; file: string }[] = [
    { id: "winscp", label: "WinSCP", file: "WinSCP.ini or a session export (.ini)" },
    { id: "filezilla", label: "FileZilla", file: "sitemanager.xml" },
    { id: "cyberduck", label: "Cyberduck", file: "a bookmark file (.duck)" },
  ];

  /** Larger than any real server list; guards against picking the wrong file. */
  const MAX_BYTES = 5 * 1024 * 1024;

  let source = $state<ImportSource>("winscp");
  let fileName = $state("");
  let report = $state<ImportReport | null>(null);
  let selection = $state<Set<number>>(new Set());
  let error = $state<string | null>(null);
  let busy = $state(false);
  let input: HTMLInputElement | undefined = $state();

  const sourceInfo = $derived(SOURCES.find((s) => s.id === source) ?? SOURCES[0]);

  /** Counts file reads, so a read that finishes late cannot replace a newer one. */
  let reads = 0;

  function changeSource() {
    reads++;
    report = null;
    fileName = "";
    error = null;
  }

  async function readFile(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const file = el.files?.[0];
    el.value = "";
    if (!file) return;
    const read = ++reads;
    report = null;
    error = null;
    fileName = file.name;
    if (file.size > MAX_BYTES) {
      error = "That file is too large to be a list of servers.";
      return;
    }
    try {
      const next = await api.importConnections(source, await file.text());
      if (read !== reads) return;
      report = next;
      selection = allSelected(next);
    } catch (err) {
      if (read !== reads) return;
      error = errorMessage(err);
    }
  }

  /** Save the ticked servers through the normal save path, one by one. */
  async function importChosen() {
    if (!report) return;
    const items: ImportedConnection[] = chosen(report, selection);
    busy = true;
    error = null;
    const saved: ImportedConnection[] = [];
    try {
      for (const item of items) {
        await api.saveConnection(item.connection, item.secret);
        saved.push(item);
        onsaved();
      }
      onclose();
    } catch (err) {
      // Keep the dialog open with the servers that were not saved yet.
      // Keep only the servers that were ticked and not saved yet, ticked as they were.
      const done = new Set(saved);
      const pending = new Set(items.filter((c) => !done.has(c)));
      const left = { ...report, connections: report.connections.filter((c) => !done.has(c)) };
      report = left;
      selection = new Set(left.connections.flatMap((c, i) => (pending.has(c) ? [i] : [])));
      error = `Saved ${saved.length} of ${items.length}. ${errorMessage(err)}`;
      if (saved.length) onsaved();
    } finally {
      busy = false;
    }
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      // Not while servers are being saved: the save finishes first.
      if (!busy) onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onkeydown={keydown}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="import-title" data-testid="import-dialog">
    <h2 id="import-title">Import servers</h2>
    <p class="hint">
      Bring saved SFTP, FTP, WebDAV and S3 servers from another app, so you do not have to type them again.
      Nothing is saved until you press Import.
    </p>

    <label class="row">
      <span>From</span>
      <select bind:value={source} onchange={changeSource} disabled={busy}>
        {#each SOURCES as s (s.id)}
          <option value={s.id}>{s.label}</option>
        {/each}
      </select>
    </label>

    <div class="row">
      <span>File</span>
      <div class="file">
        <button type="button" onclick={() => input?.click()} disabled={busy}>Choose {sourceInfo.label} file…</button>
        <small>{fileName || sourceInfo.file}</small>
      </div>
    </div>
    <input
      bind:this={input}
      type="file"
      accept=".ini,.xml,.duck,.plist,text/plain,text/xml,application/xml"
      hidden
      onchange={readFile}
    />

    {#if report}
      <p class="summary">{summary(report)}</p>

      {#if report.connections.length}
        <fieldset class="list" disabled={busy}>
          <legend>Servers to import</legend>
          <div class="bulk">
            <button type="button" onclick={() => (selection = allSelected(report!))}>All</button>
            <button type="button" onclick={() => (selection = new Set())}>None</button>
          </div>
          <ul>
            {#each report.connections as item, i (i)}
              <li>
                <label class="pick">
                  <input
                    type="checkbox"
                    checked={selection.has(i)}
                    onchange={() => (selection = toggled(selection, i))}
                  />
                  <span>
                    <strong>{item.connection.name}</strong>
                    <small>{describeEndpoint(item.connection)}</small>
                    {#if passwordNote(item)}<small class="warn">{passwordNote(item)}</small>{/if}
                  </span>
                </label>
              </li>
            {/each}
          </ul>
        </fieldset>
      {/if}

      {#if report.skipped.length}
        <details>
          <summary>{report.skipped.length} skipped</summary>
          <ul class="skipped">
            {#each report.skipped as s, i (i)}
              <li><strong>{s.name}</strong>: {s.reason}</li>
            {/each}
          </ul>
        </details>
      {/if}
    {/if}

    {#if error}<p class="error" role="alert">{error}</p>{/if}

    <div class="buttons">
      <button type="button" onclick={onclose} disabled={busy}>Cancel</button>
      <button
        type="button"
        class="primary"
        disabled={busy || !report || selection.size === 0}
        onclick={importChosen}
      >
        {importButtonLabel(selection.size)}
      </button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.3);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    width: min(520px, calc(100vw - 32px));
    max-height: calc(100vh - 40px);
    overflow-y: auto;
    box-sizing: border-box;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 18px 20px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.3);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .row {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr);
    align-items: center;
    gap: 10px;
  }
  .row > span {
    color: var(--muted);
    text-align: right;
    font-size: 12px;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .file small {
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  select {
    font: inherit;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--control);
    color: inherit;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
  .summary {
    margin: 0;
    font-weight: 600;
  }
  .list {
    margin: 0;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    min-width: 0;
  }
  .list legend {
    padding: 0 4px;
    font-size: 12px;
    color: var(--muted);
  }
  .bulk {
    display: flex;
    gap: 6px;
    margin-bottom: 6px;
  }
  .bulk button,
  .buttons button {
    padding: 4px 12px;
    border-radius: 6px;
    background: var(--control);
    border: 1px solid var(--border);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .pick {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    cursor: pointer;
  }
  .pick span {
    display: grid;
    min-width: 0;
  }
  .pick small {
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .warn {
    color: var(--danger);
  }
  details {
    font-size: 12px;
  }
  .skipped {
    margin-top: 6px;
    color: var(--muted);
  }
  .error {
    margin: 0;
    color: var(--danger);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  .buttons .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }
  .buttons button:disabled {
    opacity: 0.5;
  }
  @media (max-width: 520px) {
    .row {
      grid-template-columns: 1fr;
      gap: 4px;
    }
    .row > span {
      text-align: left;
    }
  }
</style>

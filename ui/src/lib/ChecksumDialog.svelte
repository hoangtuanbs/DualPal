<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorKind, errorMessage, type ChecksumAlgorithm, type ChecksumEntry, type ChecksumProgress, type Location } from "./api";
  import { copyText } from "./about";
  import { formatBytes } from "./format";
  import { CHECKSUM_ALGORITHMS, checksumText, defaultChecksumName, folderNameOf, withChecksumExtension } from "./checksum";

  /** Hash the marked items (or the cursor item) of `folder`, folders recursively. */
  let {
    folder,
    names,
    onclose,
  }: {
    folder: Location;
    /** Names of the items, directly inside `folder`. */
    names: string[];
    onclose: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let algorithm = $state<ChecksumAlgorithm>("sha256");
  let entries = $state<ChecksumEntry[]>([]);
  let progress = $state<ChecksumProgress | null>(null);
  let running = $state(false);
  let error = $state("");
  let notice = $state("");
  // svelte-ignore state_referenced_locally
  let saveName = $state(defaultChecksumName(names, folderNameOf(folder.path), "sha256"));
  // Each run gets a new request id; progress and answers for older runs are ignored.
  let latest = 0;

  const scope = $derived(`${names.length} ${names.length === 1 ? "item" : "items"}`);
  const hashed = $derived(entries.filter((e) => e.hash).length);

  onMount(() => {
    const off = api.onChecksumProgress((p) => {
      if (p.request === latest && running) progress = p;
    });
    start();
    return () => {
      if (running) void api.cancelChecksum(latest);
      off.then((f) => f()).catch(() => {});
    };
  });

  function start() {
    if (running) void api.cancelChecksum(latest);
    const id = ++latest;
    entries = [];
    progress = null;
    error = "";
    notice = "";
    running = true;
    api
      .checksumCalculate(folder, names, algorithm, id)
      .then((found) => {
        if (id === latest) {
          entries = found;
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
    // A new id makes the answer of the stopped run count as stale.
    latest++;
    running = false;
    progress = null;
    notice = "Stopped.";
  }

  function changeAlgorithm(next: ChecksumAlgorithm) {
    algorithm = next;
    saveName = withChecksumExtension(saveName, next);
    start();
  }

  async function copy() {
    notice = (await copyText(checksumText(entries)))
      ? `Copied ${hashed} ${hashed === 1 ? "line" : "lines"}.`
      : "Could not copy to the clipboard.";
  }

  async function save() {
    const name = saveName.trim();
    if (!name || running || !hashed) return;
    error = "";
    notice = "";
    try {
      const file = await api.checksumSave(folder, name, algorithm, entries, false);
      notice = `Saved ${file.path.split(/[\\/]/).pop()}`;
      saveName = file.path.split(/[\\/]/).pop() ?? name;
    } catch (e) {
      error = errorKind(e) === "alreadyExists" ? `${name} already exists. Choose another name.` : errorMessage(e);
    }
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
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Calculate checksums" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={keydown}>
    <header>
      <h2>Calculate Checksums</h2>
      <span class="scope" title={folder.path}>{scope} in {folder.path}</span>
      <select
        class="algorithm"
        aria-label="Algorithm"
        value={algorithm}
        onchange={(e) => changeAlgorithm((e.currentTarget as HTMLSelectElement).value as ChecksumAlgorithm)}
      >
        {#each CHECKSUM_ALGORITHMS as a (a.id)}
          <option value={a.id}>{a.label}</option>
        {/each}
      </select>
    </header>

    <div class="status" role="status" aria-live="polite">
      {#if error}
        <span class="error">{error}</span>
      {:else if running}
        <span class="spinner" aria-hidden="true"></span>
        {#if progress}
          {progress.filesDone} of {progress.filesTotal} {progress.filesTotal === 1 ? "file" : "files"}
          · {formatBytes(progress.bytesDone)} of {formatBytes(progress.bytesTotal)}
        {:else}
          Counting files…
        {/if}
        <button class="link" onclick={stop}>Cancel</button>
      {:else if notice}
        {notice}
      {:else}
        {hashed} {hashed === 1 ? "file" : "files"} hashed{entries.length > hashed ? `, ${entries.length - hashed} could not be read` : ""}
      {/if}
    </div>
    {#if running && progress}
      <progress class="bar" max={Math.max(progress.bytesTotal, 1)} value={progress.bytesDone}></progress>
      <p class="current" title={progress.current}>{progress.current}</p>
    {/if}

    {#if entries.length}
      <ul class="rows" aria-label="Checksums">
        {#each entries as e (e.path)}
          <li>
            <span class="path" title={e.path}>{e.path}</span>
            {#if e.hash}
              <code class="hash">{e.hash}</code>
            {:else}
              <span class="error">{e.error}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    <footer>
      <form class="save" onsubmit={(e) => { e.preventDefault(); void save(); }}>
        <input aria-label="Checksum file name" spellcheck="false" autocomplete="off" bind:value={saveName} disabled={running} />
        <button type="submit" class="secondary" disabled={running || !hashed || !saveName.trim()}>Save as checksum file</button>
      </form>
      <div class="buttons">
        <button type="button" class="secondary" disabled={running || !hashed} onclick={copy}>Copy</button>
        <button type="button" class="primary" onclick={onclose}>Close</button>
      </div>
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
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    padding: 14px 16px 8px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
  }
  .scope {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--muted);
  }
  .algorithm {
    padding: 4px 6px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--control);
    color: var(--text);
    font: inherit;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 16px 8px;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
    margin: 0 16px;
    height: 6px;
    accent-color: var(--accent);
  }
  .current {
    margin: 4px 16px 8px;
    font-size: 12px;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 6px;
    flex: 1;
    min-height: 120px;
    overflow-y: auto;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }
  .rows li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.4fr);
    gap: 10px;
    align-items: center;
    padding: 4px 10px;
    border-radius: 6px;
  }
  .rows li:hover {
    background: var(--hover, rgb(0 0 0 / 0.04));
  }
  .path,
  .hash {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hash {
    font-size: 12px;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  footer {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 10px 16px 14px;
  }
  .save {
    display: flex;
    gap: 8px;
    flex: 1;
    min-width: 260px;
  }
  .save input {
    flex: 1;
    min-width: 0;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--control);
    color: var(--text);
    font: inherit;
  }
  .buttons {
    display: flex;
    gap: 8px;
  }
  button {
    padding: 5px 14px;
    border-radius: 6px;
    font-weight: 500;
    white-space: nowrap;
  }
  button:disabled {
    opacity: 0.5;
  }
  .secondary {
    background: var(--control);
  }
  .primary {
    background: var(--accent);
    color: white;
  }
</style>

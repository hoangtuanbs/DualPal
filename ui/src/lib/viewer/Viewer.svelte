<script lang="ts">
  // The F3 viewer: a file from any place as text, hex or (for images, PDFs,
  // audio and video) the file itself. Big files are read a page at a time.
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { onDestroy, tick } from "svelte";
  import { api, errorMessage, type Entry, type Location } from "../api";
  import { category, glyph } from "../fileTypes";
  import { formatBytes } from "../format";
  import { encode, ENCODINGS, parseHex } from "./encoding";
  import HexView from "./HexView.svelte";
  import { backendSource, newRequest, type ByteSource } from "./source";
  import TextView from "./TextView.svelte";
  import { viewerApi, type TextEncoding, type ViewerFile } from "./viewer-api";

  let {
    location,
    entry,
    onclose,
  }: {
    location: Location;
    entry: Entry;
    onclose: () => void;
  } = $props();

  type Mode = "text" | "hex" | "media";
  // svelte-ignore state_referenced_locally
  const cat = category(entry);
  const media = cat === "image" || cat === "pdf" || cat === "video" || cat === "audio";
  const MEDIA_LABEL: Record<string, string> = { image: "Image", pdf: "PDF", video: "Video", audio: "Audio" };
  /** Largest server file downloaded to show as media. */
  const REMOTE_MEDIA_LIMIT = 64 * 1024 * 1024;

  let file = $state<ViewerFile | null>(null);
  let source = $state<ByteSource | null>(null);
  let error = $state<string | null>(null);
  let mode = $state<Mode>(media ? "media" : "text");
  let encoding = $state<TextEncoding>("utf-8");
  let wrap = $state(stored("dualpal.viewer.wrap", false));
  let lineNumbers = $state(stored("dualpal.viewer.lineNumbers", true));
  let offset = $state(0);

  let textView: TextView | undefined = $state();
  let hexView: HexView | undefined = $state();
  let searchField: HTMLInputElement | undefined = $state();

  function stored(key: string, fallback: boolean): boolean {
    try {
      const v = localStorage.getItem(key);
      return v === null ? fallback : v === "1";
    } catch {
      return fallback;
    }
  }
  function store(key: string, value: boolean) {
    try {
      localStorage.setItem(key, value ? "1" : "0");
    } catch {
      // Private window or blocked storage: the choice lasts this session.
    }
  }

  // svelte-ignore state_referenced_locally
  const opening = viewerApi.open(location).then(
    (f) => {
      file = f;
      encoding = f.detected.encoding;
      source = backendSource(f, location.fs === "local");
      if (!media && f.detected.binary) mode = "hex";
      void focusView();
      return f;
    },
    (e) => {
      error = errorMessage(e);
      return null;
    },
  );
  onDestroy(() => {
    if (running !== null) viewerApi.cancel(running).catch(() => {});
    void opening.then((f) => f && viewerApi.close(f.session)).catch(() => {});
  });

  const bom = $derived(file && encoding === file.detected.encoding ? file.detected.bom : 0);

  // ---- media ------------------------------------------------------------------
  let mediaSrc = $state<string | null>(null);
  let mediaNote = $state<string | null>(null);
  $effect(() => {
    if (mode !== "media" || mediaSrc || !media) return;
    if (location.fs === "local") {
      mediaSrc = convertFileSrc(location.path);
    } else if (entry.size > REMOTE_MEDIA_LIMIT) {
      mediaNote = `Files on servers over ${formatBytes(REMOTE_MEDIA_LIMIT)} are not downloaded to show them.`;
    } else {
      mediaNote = "Downloading…";
      api.previewPath(location).then(
        (p) => {
          mediaSrc = convertFileSrc(p);
          mediaNote = null;
        },
        (e) => (mediaNote = errorMessage(e)),
      );
    }
  });

  // ---- modes ------------------------------------------------------------------
  async function focusView() {
    await tick();
    (document.querySelector(mode === "hex" ? "[data-testid=hex-view]" : "[data-testid=text-view]") as HTMLElement | null)?.focus();
  }

  function setMode(m: Mode) {
    if (m === "media" && !media) return;
    const at = mode === "hex" ? hexView?.topOffset() : mode === "text" ? textView?.topOffset() : undefined;
    mode = m;
    // Keep the place when switching between text and hex.
    void tick().then(() => {
      if (at === undefined) return;
      if (m === "hex") hexView?.scrollToOffset(at, 0);
      else if (m === "text") void textView?.scrollToOffset(at, 0);
    });
    void focusView();
  }

  // ---- search -----------------------------------------------------------------
  let searchOpen = $state(false);
  let query = $state("");
  let ignoreCase = $state(true);
  let match = $state<{ offset: number; length: number } | null>(null);
  let status = $state("");
  let running: number | null = null;
  let scanned = $state<number | null>(null);
  let searching = $state(false);

  $effect(() => {
    let off: (() => void) | undefined;
    void viewerApi
      .onSearchProgress((p) => {
        if (p.request === running) scanned = p.scanned;
      })
      .then((u) => (off = u), () => {});
    return () => {
      try {
        off?.();
      } catch {
        // The window is already gone.
      }
    };
  });

  async function openSearch() {
    if (mode === "media") setMode("text");
    searchOpen = true;
    await tick();
    searchField?.focus();
    searchField?.select();
  }

  function closeSearch() {
    cancelSearch();
    searchOpen = false;
    match = null;
    status = "";
    void focusView();
  }

  function cancelSearch() {
    if (running !== null) viewerApi.cancel(running).catch(() => {});
    running = null;
    searching = false;
    scanned = null;
  }

  /** The search term as bytes: hex bytes in hex mode when it reads as hex. */
  function needle(): { bytes: Uint8Array; text: boolean } | null {
    if (mode === "hex") {
      const hex = parseHex(query);
      if (hex) return { bytes: hex, text: false };
    }
    const bytes = encode(query, encoding);
    return bytes ? { bytes, text: true } : null;
  }

  async function find(backward: boolean) {
    if (!file || !query) return;
    const n = needle();
    if (!n || !n.bytes.length) {
      status = `“${query}” cannot be written in ${ENCODINGS.find((e) => e.id === encoding)?.label ?? encoding}.`;
      return;
    }
    const view = mode === "hex" ? hexView : textView;
    const top = view?.topOffset() ?? 0;
    const from = backward ? (match?.offset ?? top) : match ? match.offset + 1 : top;
    cancelSearch();
    const request = (running = newRequest());
    searching = true;
    status = "Searching…";
    try {
      const at = await viewerApi.search(file.session, request, n.bytes, from, backward, n.text && ignoreCase);
      if (running !== request) return;
      if (at === null) {
        status = backward ? "No more matches above." : "No more matches below.";
      } else {
        match = { offset: at, length: n.bytes.length };
        status = "";
        if (mode === "hex") hexView?.scrollToOffset(at);
        else await textView?.scrollToOffset(at);
      }
    } catch (e) {
      if (running === request) status = errorMessage(e);
    } finally {
      if (running === request) {
        running = null;
        searching = false;
        scanned = null;
      }
    }
  }

  function searchKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      void find(e.shiftKey);
    }
  }

  // ---- keys -------------------------------------------------------------------
  function onkeydown(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    const typing = !!t && (t.tagName === "INPUT" || t.tagName === "SELECT" || t.tagName === "TEXTAREA");
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Escape") {
      e.preventDefault();
      if (searchOpen) closeSearch();
      else onclose();
      return;
    }
    if (e.key === "F3") {
      e.preventDefault();
      if (searchOpen && query) void find(e.shiftKey);
      else onclose();
      return;
    }
    if (mod && !e.shiftKey && e.key.toLowerCase() === "f") {
      e.preventDefault();
      void openSearch();
      return;
    }
    if (typing || mod || e.altKey) return;
    if (e.key === "1" || e.key === "2" || e.key === "3") {
      e.preventDefault();
      setMode(e.key === "1" ? "text" : e.key === "2" ? "hex" : "media");
    }
  }

  const progress = $derived(file && file.size ? Math.min(100, Math.round((offset / file.size) * 100)) : 0);
</script>

<svelte:window {onkeydown} />

<div class="backdrop">
  <div class="viewer" role="dialog" aria-modal="true" aria-label="View {entry.name}" data-testid="viewer">
    <header>
      <span class="icon" aria-hidden="true">{glyph(entry)}</span>
      <h2 title={location.path}>{entry.name}</h2>
      <span class="muted">{formatBytes(file?.size ?? entry.size)}</span>
      <div class="spacer"></div>
      <div class="modes" role="group" aria-label="View as">
        <button aria-pressed={mode === "text"} onclick={() => setMode("text")} title="Text (1)">Text</button>
        <button aria-pressed={mode === "hex"} onclick={() => setMode("hex")} title="Hex (2)">Hex</button>
        {#if media}
          <button aria-pressed={mode === "media"} onclick={() => setMode("media")} title="{MEDIA_LABEL[cat]} (3)">{MEDIA_LABEL[cat]}</button>
        {/if}
      </div>
      {#if mode === "text"}
        <select aria-label="Encoding" bind:value={encoding} onchange={() => (match = null)}>
          {#each ENCODINGS as enc (enc.id)}
            <option value={enc.id}>{enc.label}</option>
          {/each}
        </select>
        <button
          aria-pressed={wrap}
          onclick={() => {
            wrap = !wrap;
            store("dualpal.viewer.wrap", wrap);
          }}
          title="Wrap long lines">Wrap</button
        >
        <button
          aria-pressed={lineNumbers}
          onclick={() => {
            lineNumbers = !lineNumbers;
            store("dualpal.viewer.lineNumbers", lineNumbers);
          }}
          title="Line numbers">Line numbers</button
        >
      {/if}
      {#if mode !== "media"}
        <button onclick={openSearch} title="Find (Ctrl/⌘ F)" aria-label="Find">Find…</button>
      {/if}
      <button class="close" onclick={onclose} aria-label="Close" title="Close (Esc or F3)">×</button>
    </header>

    {#if searchOpen}
      <div class="search" role="search">
        <input
          bind:this={searchField}
          bind:value={query}
          onkeydown={searchKey}
          oninput={() => (match = null)}
          placeholder={mode === "hex" ? "Text, or hex bytes like 4F 4B" : "Find text"}
          aria-label="Find in file"
          spellcheck="false"
          autocomplete="off"
          data-testid="viewer-search"
        />
        {#if mode === "text"}
          <label><input type="checkbox" bind:checked={ignoreCase} /> Ignore case</label>
        {/if}
        <button onclick={() => find(true)} disabled={!query} title="Previous (Shift F3, Shift Enter)">Previous</button>
        <button onclick={() => find(false)} disabled={!query} title="Next (F3, Enter)">Next</button>
        {#if searching}
          <button onclick={cancelSearch}>Stop</button>
        {/if}
        <span class="status" role="status" data-testid="viewer-search-status"
          >{status}{#if searching && scanned !== null && file}{" "}{formatBytes(scanned)} read{/if}</span
        >
        <div class="spacer"></div>
        <button class="close" onclick={closeSearch} aria-label="Close find" title="Close (Esc)">×</button>
      </div>
    {/if}

    <div class="body">
      {#if error}
        <p class="message" role="alert">Could not open “{entry.name}”: {error}</p>
      {:else if mode === "media"}
        <div class="media">
          {#if mediaSrc && cat === "image"}
            <img src={mediaSrc} alt={entry.name} />
          {:else if mediaSrc && cat === "video"}
            <!-- svelte-ignore a11y_media_has_caption -->
            <video src={mediaSrc} controls></video>
          {:else if mediaSrc && cat === "audio"}
            <audio src={mediaSrc} controls></audio>
          {:else if mediaSrc && cat === "pdf"}
            <iframe src={mediaSrc} title={entry.name}></iframe>
          {:else}
            <p class="message">{mediaNote ?? ""}</p>
          {/if}
        </div>
      {:else if source && mode === "hex"}
        <HexView bind:this={hexView} {source} highlight={match} onmove={(o) => (offset = o)} />
      {:else if source}
        <TextView bind:this={textView} {source} {encoding} {bom} {wrap} {lineNumbers} highlight={match} onmove={(o) => (offset = o)} />
      {:else}
        <p class="message">Opening…</p>
      {/if}
    </div>

    <footer>
      <span>{mode === "media" ? MEDIA_LABEL[cat] : mode === "hex" ? "Hex" : (ENCODINGS.find((e) => e.id === encoding)?.label ?? encoding)}</span>
      {#if mode !== "media" && file}
        <span data-testid="viewer-offset">Byte {offset.toLocaleString()} of {file.size.toLocaleString()} ({progress}%)</span>
      {/if}
      <div class="spacer"></div>
      <span class="muted">1 Text · 2 Hex{media ? ` · 3 ${MEDIA_LABEL[cat]}` : ""} · Esc closes</span>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: stretch;
    padding: 16px;
    z-index: 9;
  }
  .viewer {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header,
  .search,
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
    flex-wrap: wrap;
  }
  footer {
    border-bottom: none;
    border-top: 1px solid var(--border);
    padding: 4px 12px;
    font-size: 12px;
  }
  h2 {
    margin: 0;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40vw;
  }
  .muted,
  .status {
    color: var(--muted);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .modes {
    display: flex;
    gap: 2px;
  }
  button[aria-pressed="true"] {
    background: var(--accent);
    color: var(--selection-text);
  }
  .close {
    font-size: 16px;
    line-height: 1;
    padding: 2px 8px;
  }
  .search input:not([type]) {
    min-width: 240px;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .media {
    flex: 1;
    display: grid;
    place-items: center;
    min-height: 0;
    padding: 12px;
  }
  .media img,
  .media video {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .media iframe {
    width: 100%;
    height: 100%;
    border: none;
  }
  .message {
    margin: auto;
    color: var(--muted);
  }
</style>

<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { untrack } from "svelte";
  import { api, errorMessage, type Entry, type Location } from "./api";
  import { category, extension, glyph } from "./fileTypes";
  import { formatBytes, formatDate } from "./format";

  let {
    entry,
    fs,
    onsetmode,
    onsaved,
  }: {
    entry: Entry | null;
    /** File system id of the pane the entry comes from. */
    fs: string;
    onsetmode: (location: Location, mode: number) => void;
    /** A file was saved from the quick editor. */
    onsaved: () => void;
  } = $props();

  /** Largest file the quick editor opens. */
  const EDIT_LIMIT = 1_000_000;

  const cat = $derived(entry ? category(entry) : null);
  const local = $derived(fs === "local");
  const src = $derived(entry && local ? convertFileSrc(entry.path) : null);
  const loc = $derived(entry ? { fs, path: entry.path } : null);

  const KIND: Record<string, string> = {
    folder: "Folder",
    image: "Image",
    video: "Video",
    audio: "Audio",
    pdf: "PDF document",
    archive: "Archive",
    code: "Source code",
    text: "Text document",
    app: "Application",
    file: "Document",
  };
  const kindLabel = $derived.by(() => {
    if (!entry || !cat) return "";
    const ext = extension(entry.name).toUpperCase();
    const base = cat === "file" && ext ? `${ext} file` : cat === "image" && ext ? `${ext} image` : KIND[cat];
    return entry.kind === "dir" ? base : `${base} – ${formatBytes(entry.size)}`;
  });

  // ---- quick text editor --------------------------------------------------
  let text = $state("");
  let original = $state("");
  let textState = $state<"none" | "loading" | "ready" | "binary" | "tooLarge" | "error">("none");
  let textError = $state("");
  let saving = $state(false);
  let savedAt = $state<number | null>(null);
  const dirty = $derived(textState === "ready" && text !== original);

  const wantsText = $derived(
    !!entry &&
      entry.kind === "file" &&
      (cat === "text" || cat === "code" || (cat === "file" && entry.size <= 256_000)),
  );

  let loadedPath: string | null = null;

  $effect(() => {
    const l = loc;
    const e = entry;
    // The pane reloads its listing after any operation; keep unsaved edits
    // to the same file instead of reloading over them.
    const sameFile = !!l && l.path === loadedPath;
    if (sameFile && untrack(() => dirty || saving)) return;
    loadedPath = l?.path ?? null;
    if (!sameFile) savedAt = null;
    if (!l || !e || !wantsText) {
      textState = "none";
      return;
    }
    if (e.size > EDIT_LIMIT) {
      textState = "tooLarge";
      return;
    }
    textState = "loading";
    api.readText(l, EDIT_LIMIT).then(
      (t) => {
        if (loc?.path !== l.path) return; // selection moved on
        if (t.binary) textState = "binary";
        else if (t.truncated) textState = "tooLarge";
        else {
          text = original = t.text;
          textState = "ready";
        }
      },
      (err) => {
        textError = errorMessage(err);
        textState = "error";
      },
    );
  });

  async function save() {
    if (!loc || !dirty) return;
    saving = true;
    try {
      await api.writeText(loc, text);
      original = text;
      savedAt = Date.now();
      onsaved();
    } catch (err) {
      textError = errorMessage(err);
    } finally {
      saving = false;
    }
  }

  function editorKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      save();
    }
  }

  // ---- permissions ----------------------------------------------------------
  const WHO = [
    ["Owner", 6],
    ["Group", 3],
    ["Others", 0],
  ] as const;
  const BITS = [
    ["R", 4],
    ["W", 2],
    ["X", 1],
  ] as const;

  function symbolic(mode: number) {
    const s = (shift: number) =>
      `${mode & (4 << shift) ? "r" : "-"}${mode & (2 << shift) ? "w" : "-"}${mode & (1 << shift) ? "x" : "-"}`;
    return s(6) + s(3) + s(0);
  }

  function toggleBit(bit: number) {
    if (!entry || entry.mode == null || !loc) return;
    onsetmode(loc, entry.mode ^ bit);
  }

  let octal = $state("");
  $effect(() => {
    octal = entry?.mode != null ? (entry.mode & 0o777).toString(8).padStart(3, "0") : "";
  });

  function applyOctal() {
    if (!entry || entry.mode == null || !loc) return;
    if (!/^[0-7]{3,4}$/.test(octal)) {
      octal = (entry.mode & 0o777).toString(8).padStart(3, "0");
      return;
    }
    const mode = parseInt(octal, 8);
    if (mode !== (entry.mode & 0o7777)) onsetmode(loc, (entry.mode & ~0o7777) | mode);
  }
</script>

<aside class="inspector" aria-label="Info">
  {#if !entry}
    <p class="empty">Select an item to see its preview and details.</p>
  {:else}
    <div class="preview">
      {#if cat === "image" && src}
        <img src={src} alt={entry.name} />
      {:else if cat === "video" && src}
        <!-- svelte-ignore a11y_media_has_caption -->
        <video src={src} controls preload="metadata"></video>
      {:else if cat === "audio" && src}
        <div class="audio">
          <span class="big" aria-hidden="true">{glyph(entry)}</span>
          <audio src={src} controls preload="metadata"></audio>
        </div>
      {:else if cat === "pdf" && src}
        <iframe src={src} title={entry.name}></iframe>
      {:else if textState === "ready" || textState === "loading"}
        <textarea
          class="editor"
          bind:value={text}
          onkeydown={editorKey}
          spellcheck="false"
          disabled={textState === "loading"}
          aria-label="Edit {entry.name}"
        ></textarea>
      {:else}
        <span class="big" aria-hidden="true">{glyph(entry)}</span>
      {/if}
    </div>

    {#if textState === "ready"}
      <div class="editbar">
        <span class="status">
          {#if saving}Saving…{:else if dirty}Edited{:else if savedAt}Saved{:else}Quick edit{/if}
        </span>
        <button onclick={() => (text = original)} disabled={!dirty}>Revert</button>
        <button class="primary" onclick={save} disabled={!dirty || saving} title="Save (Ctrl/⌘ S)">Save</button>
      </div>
    {:else if textState === "tooLarge"}
      <p class="note">Too large to edit here (over {formatBytes(EDIT_LIMIT)}).</p>
    {:else if textState === "error"}
      <p class="note error">{textError}</p>
    {/if}

    <h2 title={entry.name}>{entry.name}</h2>
    <p class="kind">{kindLabel}</p>

    <h3>General</h3>
    <dl>
      <dt>Path</dt>
      <dd class="path" title={entry.path}>{entry.path}</dd>
      {#if entry.kind !== "dir"}
        <dt>Size</dt>
        <dd>{formatBytes(entry.size)} ({entry.size.toLocaleString()} bytes)</dd>
      {/if}
      <dt>Modified</dt>
      <dd>{formatDate(entry.modifiedMs)}</dd>
      {#if entry.createdMs}
        <dt>Created</dt>
        <dd>{formatDate(entry.createdMs)}</dd>
      {/if}
      {#if extension(entry.name)}
        <dt>Extension</dt>
        <dd>{extension(entry.name)}</dd>
      {/if}
      <dt>Hidden</dt>
      <dd>{entry.hidden ? "Yes" : "No"}</dd>
      {#if entry.symlink}
        <dt>Link</dt>
        <dd>Symbolic link</dd>
      {/if}
      {#if entry.readonly}
        <dt>Read-only</dt>
        <dd>Yes</dd>
      {/if}
    </dl>

    {#if entry.mode != null}
      <h3>Permissions</h3>
      <table class="perms">
        <thead>
          <tr><th></th>{#each BITS as [label]}<th>{label}</th>{/each}</tr>
        </thead>
        <tbody>
          {#each WHO as [who, shift]}
            <tr>
              <th scope="row">{who}</th>
              {#each BITS as [label, bit]}
                <td>
                  <input
                    type="checkbox"
                    checked={(entry.mode & (bit << shift)) !== 0}
                    onchange={() => toggleBit(bit << shift)}
                    aria-label="{who} {label}"
                  />
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="octal">
        <span>Octal</span>
        <code>{symbolic(entry.mode)}</code>
        <input
          bind:value={octal}
          onkeydown={(e) => e.key === "Enter" && applyOctal()}
          onblur={applyOctal}
          maxlength="4"
          aria-label="Octal permissions"
        />
      </div>
    {/if}
  {/if}
</aside>

<style>
  .inspector {
    border-left: 1px solid var(--border);
    background: var(--sidebar);
    overflow-y: auto;
    padding: 12px 14px 16px;
    min-height: 0;
    min-width: 0;
  }
  .empty {
    color: var(--muted);
    margin-top: 40px;
    text-align: center;
  }
  .preview {
    display: grid;
    place-items: center;
    min-height: 120px;
    margin-bottom: 8px;
  }
  .preview img {
    max-width: 100%;
    max-height: 240px;
    border-radius: 6px;
    box-shadow: 0 2px 10px rgb(0 0 0 / 0.25);
  }
  .preview video {
    width: 100%;
    max-height: 240px;
    border-radius: 6px;
    background: black;
  }
  .audio {
    display: grid;
    gap: 12px;
    justify-items: center;
    width: 100%;
  }
  .audio audio {
    width: 100%;
  }
  .preview iframe {
    width: 100%;
    height: 320px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: white;
  }
  .big {
    font-size: 72px;
    line-height: 1;
  }
  .editor {
    width: 100%;
    height: 260px;
    box-sizing: border-box;
    resize: vertical;
    padding: 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--pane-bg);
    color: var(--text);
    font: 12px/1.45 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    tab-size: 4;
  }
  .editbar {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
  }
  .editbar .status {
    flex: 1;
    font-size: 12px;
    color: var(--muted);
  }
  .editbar button {
    padding: 3px 10px;
    border-radius: 6px;
    background: var(--control);
  }
  .editbar .primary {
    background: var(--accent);
    color: white;
  }
  .note {
    font-size: 12px;
    color: var(--muted);
  }
  .note.error {
    color: var(--danger);
  }
  h2 {
    margin: 6px 0 0;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    margin: 2px 0 10px;
    color: var(--muted);
  }
  h3 {
    margin: 14px 0 6px;
    font-size: 13px;
    border-bottom: 1px solid var(--border);
    padding-bottom: 4px;
  }
  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 5px 10px;
    margin: 0;
    font-size: 12px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    text-align: right;
    overflow-wrap: anywhere;
  }
  .perms {
    width: 100%;
    font-size: 12px;
    border-collapse: collapse;
  }
  .perms th {
    font-weight: 500;
    color: var(--muted);
    text-align: left;
  }
  .perms thead th,
  .perms td {
    text-align: center;
    width: 36px;
  }
  .perms td,
  .perms th {
    padding: 3px 0;
  }
  .octal {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-size: 12px;
  }
  .octal span {
    flex: 1;
    color: var(--muted);
  }
  .octal input {
    width: 48px;
    padding: 2px 6px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--control);
    color: var(--text);
    font: 12px ui-monospace, Menlo, Consolas, monospace;
    text-align: center;
  }
</style>

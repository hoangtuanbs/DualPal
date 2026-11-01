<script lang="ts">
  import { onMount } from "svelte";
  import {
    api,
    errorMessage,
    type Entry,
    type Location,
    type RenamePreset,
    type RenamePreview,
    type RenameRule,
  } from "./api";

  /** Rename many items at once with rules and saved presets. */
  let {
    items,
    folder,
    others,
    onclose,
    onrename,
  }: {
    items: Entry[];
    folder: Location;
    /** Names in the folder that are not being renamed. */
    others: string[];
    onclose: () => void;
    onrename: (items: { location: Location; newName: string }[]) => void;
  } = $props();

  const NEW: Record<RenameRule["type"], () => RenameRule> = {
    replace: () => ({ type: "replace", find: "", replace: "", regex: false, caseSensitive: false, scope: "name" }),
    case: () => ({ type: "case", case: "lower", scope: "whole" }),
    insert: () => ({ type: "insert", text: "", at: "start", scope: "name" }),
    number: () => ({ type: "number", start: 1, step: 1, pad: 3, at: "start", separator: " " }),
    remove: () => ({ type: "remove", count: 1, from: "start", offset: 0, scope: "name" }),
    extension: () => ({ type: "extension", ext: "" }),
  };
  const LABEL: Record<RenameRule["type"], string> = {
    replace: "Replace text",
    case: "Change case",
    insert: "Insert text",
    number: "Add number",
    remove: "Remove characters",
    extension: "Change extension",
  };

  let rules = $state<RenameRule[]>([NEW.replace()]);
  let presets = $state<RenamePreset[]>([]);
  let presetName = $state("");
  let saving = $state<string | null>(null);
  let preview = $state<RenamePreview[]>([]);
  let error = $state<string | null>(null);

  // svelte-ignore state_referenced_locally
  const parent = folder.path.split(/[\\/]/).filter(Boolean).pop() ?? "";
  // svelte-ignore state_referenced_locally
  const inputs = items.map((e) => ({ name: e.name, dir: e.kind === "dir", modifiedMs: e.modifiedMs, parent }));

  onMount(() => {
    api.renamePresets().then((p) => (presets = p));
  });

  // Preview on every change, a moment after typing stops.
  $effect(() => {
    const r = $state.snapshot(rules) as RenameRule[];
    const timer = setTimeout(async () => {
      try {
        preview = await api.renamePreview(r, inputs, others);
        error = null;
      } catch (e) {
        error = errorMessage(e);
      }
    }, 120);
    return () => clearTimeout(timer);
  });

  const changes = $derived(preview.filter((p) => p.newName !== p.name));
  const problems = $derived(preview.filter((p) => p.issue && p.newName !== p.name).length);

  function applyPreset(name: string) {
    const p = presets.find((x) => x.name === name);
    if (!p) return;
    presetName = name;
    rules = structuredClone($state.snapshot(p.rules)) as RenameRule[];
  }

  async function savePreset(name: string) {
    try {
      presets = await api.saveRenamePreset({ name, rules: $state.snapshot(rules) as RenameRule[], builtin: false });
      presetName = name;
      saving = null;
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function deletePreset() {
    presets = await api.removeRenamePreset(presetName);
    presetName = "";
  }

  function move(i: number, d: number) {
    const j = i + d;
    if (j < 0 || j >= rules.length) return;
    [rules[i], rules[j]] = [rules[j], rules[i]];
  }

  function rename() {
    if (problems || !changes.length) return;
    const byName = new Map(items.map((e) => [e.name, e]));
    onrename(
      changes.map((p) => ({ location: { fs: folder.fs, path: byName.get(p.name)!.path }, newName: p.newName })),
    );
  }

  const isUserPreset = $derived(presets.some((p) => p.name === presetName && !p.builtin));

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (saving !== null) saving = null;
      else onclose();
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<div class="backdrop">
  <div class="sheet" role="dialog" aria-modal="true" aria-label="Multi Rename">
    <header>
      <h2>Rename {items.length} items</h2>
      <label class="preset">
        Preset
        <select value={presetName} onchange={(e) => applyPreset((e.currentTarget as HTMLSelectElement).value)}>
          <option value="" disabled>Choose…</option>
          {#each presets as p (p.name)}
            <option value={p.name}>{p.name}</option>
          {/each}
        </select>
      </label>
      {#if saving !== null}
        <form
          class="save"
          onsubmit={(e) => {
            e.preventDefault();
            if (saving?.trim()) savePreset(saving.trim());
          }}
        >
          <!-- svelte-ignore a11y_autofocus -->
          <input bind:value={saving} placeholder="Preset name" autofocus spellcheck="false" />
          <button class="btn" type="submit" disabled={!saving.trim()}>Save</button>
          <button class="btn" type="button" onclick={() => (saving = null)}>Cancel</button>
        </form>
      {:else}
        <button class="btn" onclick={() => (saving = isUserPreset ? presetName : "")}>Save Preset…</button>
        {#if isUserPreset}<button class="link" onclick={deletePreset}>Delete preset</button>{/if}
      {/if}
      <span class="spacer"></span>
      <button class="close" onclick={onclose} aria-label="Close">×</button>
    </header>

    <div class="body">
      <section class="rules" aria-label="Rules">
        {#each rules as rule, i (i)}
          <div class="rule">
            <div class="rule-head">
              <strong>{i + 1}. {LABEL[rule.type]}</strong>
              <span class="spacer"></span>
              <button onclick={() => move(i, -1)} disabled={i === 0} aria-label="Move up">↑</button>
              <button onclick={() => move(i, 1)} disabled={i === rules.length - 1} aria-label="Move down">↓</button>
              <button onclick={() => rules.splice(i, 1)} aria-label="Remove rule">×</button>
            </div>
            {#if rule.type === "replace"}
              <div class="fields">
                <label>Find <input bind:value={rule.find} spellcheck="false" /></label>
                <label>Replace with <input bind:value={rule.replace} spellcheck="false" /></label>
              </div>
              <div class="fields">
                <label class="check"><input type="checkbox" bind:checked={rule.regex} /> Regular expression</label>
                <label class="check"><input type="checkbox" bind:checked={rule.caseSensitive} /> Match case</label>
                {@render scope(rule)}
              </div>
            {:else if rule.type === "case"}
              <div class="fields">
                <select bind:value={rule.case}>
                  <option value="lower">lower case</option>
                  <option value="upper">UPPER CASE</option>
                  <option value="title">Title Case</option>
                  <option value="sentence">Sentence case</option>
                </select>
                {@render scope(rule)}
              </div>
            {:else if rule.type === "insert"}
              <div class="fields">
                <label class="wide">Text <input bind:value={rule.text} spellcheck="false" placeholder={"e.g. {date} or {parent}_"} /></label>
              </div>
              <div class="fields">
                {@render at(rule)}
                {@render scope(rule)}
              </div>
              <p class="hint">Tokens: {"{name} {ext} {parent} {date} {n} {n:3}"}</p>
            {:else if rule.type === "number"}
              <div class="fields">
                <label>Start <input type="number" bind:value={rule.start} /></label>
                <label>Step <input type="number" bind:value={rule.step} /></label>
                <label>Digits <input type="number" min="0" max="12" bind:value={rule.pad} /></label>
              </div>
              <div class="fields">
                <label>Separator <input class="short" bind:value={rule.separator} spellcheck="false" /></label>
                {@render at(rule)}
              </div>
            {:else if rule.type === "remove"}
              <div class="fields">
                <label>Remove <input type="number" min="0" bind:value={rule.count} /></label>
                <label>after skipping <input type="number" min="0" bind:value={rule.offset} /></label>
              </div>
              <div class="fields">
                <select bind:value={rule.from}>
                  <option value="start">from the start</option>
                  <option value="end">from the end</option>
                </select>
                {@render scope(rule)}
              </div>
            {:else if rule.type === "extension"}
              <div class="fields">
                <label class="wide">New extension <input bind:value={rule.ext} spellcheck="false" placeholder="empty removes it" /></label>
              </div>
            {/if}
          </div>
        {/each}
        <label class="add">
          <select
            value=""
            onchange={(e) => {
              const t = (e.currentTarget as HTMLSelectElement).value as RenameRule["type"];
              if (t) rules.push(NEW[t]());
              (e.currentTarget as HTMLSelectElement).value = "";
            }}
          >
            <option value="" disabled>+ Add rule…</option>
            {#each Object.entries(LABEL) as [t, label] (t)}
              <option value={t}>{label}</option>
            {/each}
          </select>
        </label>
      </section>

      <section class="names" aria-label="Preview">
        <div class="prow head"><span>Now</span><span>Will be</span></div>
        {#each preview as p (p.name)}
          <div class="prow" class:same={p.newName === p.name} class:bad={p.issue && p.newName !== p.name}>
            <span class="old" title={p.name}>{p.name}</span>
            <span class="new" title={p.issue ?? p.newName}>
              {p.newName}
              {#if p.issue && p.newName !== p.name}<em>{p.issue}</em>{/if}
            </span>
          </div>
        {/each}
      </section>
    </div>

    <footer>
      <span class="msg" class:err={error || problems}>
        {#if error}{error}{:else if problems}{problems} {problems === 1 ? "name needs" : "names need"} fixing{:else}{changes.length} of {items.length} names change{/if}
      </span>
      <span class="spacer"></span>
      <button class="btn" onclick={onclose}>Cancel</button>
      <button class="btn primary" onclick={rename} disabled={!!error || problems > 0 || changes.length === 0}>Rename</button>
    </footer>
  </div>
</div>

{#snippet scope(rule: { scope: "name" | "extension" | "whole" })}
  <select bind:value={rule.scope} aria-label="Applies to">
    <option value="name">in the name</option>
    <option value="extension">in the extension</option>
    <option value="whole">in name and extension</option>
  </select>
{/snippet}

{#snippet at(rule: { at: "start" | "end" })}
  <select bind:value={rule.at} aria-label="Position">
    <option value="start">at the start</option>
    <option value="end">at the end</option>
  </select>
{/snippet}

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
    width: min(980px, calc(100vw - 48px));
    height: min(640px, calc(100vh - 48px));
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  header,
  footer {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    font-size: 12px;
  }
  header {
    border-bottom: 1px solid var(--border);
  }
  footer {
    border-top: 1px solid var(--border);
    background: var(--chrome);
  }
  h2 {
    margin: 0;
    font-size: 15px;
    white-space: nowrap;
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--muted);
  }
  .save {
    display: flex;
    gap: 6px;
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
  .body {
    display: grid;
    grid-template-columns: 360px minmax(0, 1fr);
    min-height: 0;
  }
  .rules {
    overflow-y: auto;
    padding: 10px 12px;
    border-right: 1px solid var(--border);
    background: var(--sidebar);
  }
  .rule {
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    margin-bottom: 8px;
    font-size: 12px;
  }
  .rule-head {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-bottom: 6px;
  }
  .rule-head button {
    color: var(--muted);
    padding: 0 5px;
  }
  .fields {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-bottom: 6px;
  }
  .fields label {
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--muted);
    flex: 1;
    min-width: 90px;
  }
  .fields label.check {
    flex-direction: row;
    align-items: center;
    flex: 0 0 auto;
    min-width: 0;
  }
  .fields label.wide {
    flex-basis: 100%;
  }
  input,
  select {
    font: inherit;
    color: var(--text);
    background: var(--control);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 6px;
    min-width: 0;
  }
  input[type="number"] {
    width: 64px;
  }
  input.short {
    width: 60px;
  }
  .check input {
    padding: 0;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 11px;
  }
  .add select {
    width: 100%;
  }
  .names {
    overflow: auto;
    font-size: 12px;
  }
  .prow {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    padding: 4px 12px;
    border-bottom: 1px solid var(--stripe);
  }
  .prow.head {
    position: sticky;
    top: 0;
    background: var(--chrome);
    font-weight: 600;
    color: var(--muted);
    font-size: 11px;
  }
  .prow span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prow .new {
    font-weight: 500;
  }
  .prow.same .new {
    color: var(--muted);
    font-weight: normal;
  }
  .prow.bad .new {
    color: var(--danger);
  }
  .prow em {
    font-style: normal;
    font-size: 11px;
    margin-left: 6px;
  }
  .btn {
    padding: 4px 12px;
    border-radius: 6px;
    background: var(--control);
    font-weight: 500;
  }
  .btn.primary {
    background: var(--accent);
    color: white;
  }
  .link {
    color: var(--accent);
  }
  .msg {
    color: var(--muted);
  }
  .msg.err {
    color: var(--danger);
  }
</style>

<script lang="ts" module>
  /** A command that can have a shortcut, as the Keyboard section lists it. */
  export interface KeyCommand {
    id: string;
    title: string;
    group: string;
  }

  export type SettingsSection = "appearance" | "panes" | "apps" | "keyboard";
</script>

<script lang="ts">
  import { errorMessage, type DiffTool, type PaneStart, type Settings, type StartMode } from "./api";
  import { prefs } from "./prefs.svelte";
  import { updates } from "./updates.svelte";
  import {
    DEFAULT_KEYS,
    assignCombo,
    comboFromEvent,
    conflicts,
    formatCombo,
    isMacPlatform,
    normalizeCombo,
    resetBinding,
    resetConflicts,
    unbindable,
    withBinding,
  } from "./keymap";

  let {
    commands,
    diffTools = [],
    mac = isMacPlatform(),
    start = "appearance",
    onclose,
  }: {
    commands: KeyCommand[];
    diffTools?: DiffTool[];
    mac?: boolean;
    start?: SettingsSection;
    onclose: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let section = $state<SettingsSection>(start);
  let error = $state<string | null>(null);

  const sections: { id: SettingsSection; label: string }[] = [
    { id: "appearance", label: "Appearance" },
    { id: "panes", label: "Panes" },
    { id: "apps", label: "Apps" },
    { id: "keyboard", label: "Keyboard" },
  ];

  const s = $derived(prefs.settings);

  async function change(patch: Partial<Settings>) {
    error = null;
    try {
      await prefs.update(patch);
    } catch (e) {
      error = `Could not save the setting: ${errorMessage(e)}`;
    }
  }

  // ---- panes ----

  const COLUMNS = [
    { id: "size", label: "Size" },
    { id: "modified", label: "Date Modified" },
  ];

  function toggleColumn(id: string, shown: boolean) {
    const hidden = new Set(s.hiddenColumns ?? []);
    if (shown) hidden.delete(id);
    else hidden.add(id);
    change({ hiddenColumns: [...hidden] });
  }

  const paneStart = (i: number): PaneStart => s.paneStart?.[i] ?? { mode: "home", folder: null };

  function setPaneStart(i: number, patch: Partial<PaneStart>) {
    const both = [paneStart(0), paneStart(1)];
    both[i] = { ...both[i], ...patch };
    change({ paneStart: both });
  }

  // ---- apps ----

  async function setAutoCheck(e: Event) {
    const box = e.currentTarget as HTMLInputElement;
    error = null;
    try {
      await updates.setAutoCheck(box.checked);
    } catch (err) {
      box.checked = updates.autoCheck;
      error = `Could not save the setting: ${errorMessage(err)}`;
    }
  }

  // ---- keyboard ----

  let query = $state("");
  /** The command whose new shortcut is being recorded, and the key it replaces (null adds one). */
  let recording = $state<{ id: string; replacing: string | null } | null>(null);
  /** A recorded shortcut, or the default keys of a command being reset,
   *  that another command has, waiting for Replace or Cancel. */
  let pending = $state<{ id: string; combos: string[]; replacing: string | null; others: string[]; reset?: boolean } | null>(null);
  let keyNote = $state<string | null>(null);

  const titleOf = (id: string) => commands.find((c) => c.id === id)?.title ?? id;
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter((c) => {
      const keys = (prefs.bindings[c.id] ?? []).map((k) => formatCombo(k, mac)).join(" ");
      return `${c.title} ${c.group} ${keys}`.toLowerCase().includes(q);
    });
  });
  const changed = (id: string) => {
    const now = (prefs.bindings[id] ?? []).map((k) => normalizeCombo(k, mac));
    const def = (DEFAULT_KEYS[id] ?? []).map((k) => normalizeCombo(k, mac));
    return now.join(" ") !== def.join(" ");
  };
  const anyChanged = $derived(Object.keys(s.keyBindings ?? {}).length > 0);

  function record(id: string, replacing: string | null) {
    pending = null;
    keyNote = null;
    recording = { id, replacing };
  }

  function saveBindings(next: Record<string, string[]>) {
    change({ keyBindings: next });
  }

  function recorded(combo: string) {
    if (!recording) return;
    const { id, replacing } = recording;
    recording = null;
    const why = unbindable(combo);
    if (why) {
      keyNote = why;
      return;
    }
    const others = conflicts(prefs.bindings, combo, id, mac);
    if (others.length) {
      pending = { id, combos: [combo], replacing, others };
      return;
    }
    saveBindings(assignCombo(s.keyBindings, id, combo, replacing, mac));
  }

  function replaceConflict() {
    if (!pending) return;
    const { id, combos, replacing, reset } = pending;
    pending = null;
    saveBindings(reset ? resetBinding(s.keyBindings, id, mac) : assignCombo(s.keyBindings, id, combos[0], replacing, mac));
  }

  /** Back to the default keys; asks first when another command has one now. */
  function reset(id: string) {
    keyNote = null;
    const { combos, others } = resetConflicts(prefs.bindings, id, mac);
    if (others.length) {
      pending = { id, combos, replacing: null, others, reset: true };
      return;
    }
    pending = null;
    saveBindings(resetBinding(s.keyBindings, id, mac));
  }

  function removeKey(id: string, combo: string) {
    const keys = (prefs.bindings[id] ?? []).filter((k) => k !== combo);
    saveBindings(withBinding(s.keyBindings, id, keys, mac));
  }

  function keydown(e: KeyboardEvent) {
    if (recording) {
      e.preventDefault();
      e.stopPropagation();
      // Esc on its own stops recording.
      if (e.key === "Escape" && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
        recording = null;
        return;
      }
      const combo = comboFromEvent(e, mac);
      if (combo) recorded(combo);
      return;
    }
    if (e.key === "Escape") {
      e.stopPropagation();
      if (pending) pending = null;
      else onclose();
    }
  }

  const START_MODES: { id: StartMode; label: string }[] = [
    { id: "home", label: "Home folder" },
    { id: "last", label: "Last folder" },
    { id: "folder", label: "This folder:" },
  ];
</script>

<svelte:window onkeydowncapture={keydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="sheet" role="dialog" aria-modal="true" aria-label="Settings" data-testid="settings-dialog">
    <div class="sections" role="tablist" aria-label="Settings sections" aria-orientation="vertical">
      {#each sections as sec (sec.id)}
        <button
          role="tab"
          aria-selected={section === sec.id}
          class:current={section === sec.id}
          data-testid={`settings-tab-${sec.id}`}
          onclick={() => (section = sec.id)}>{sec.label}</button
        >
      {/each}
    </div>

    <div class="content" role="tabpanel" aria-label={sections.find((x) => x.id === section)?.label}>
      <header>
        <h2>{sections.find((x) => x.id === section)?.label}</h2>
        <!-- svelte-ignore a11y_autofocus -->
        <button class="close" onclick={onclose} aria-label="Close" autofocus>×</button>
      </header>

      {#if section === "appearance"}
        <fieldset>
          <legend>Theme</legend>
          <div class="choices">
            {#each [["system", "System"], ["light", "Light"], ["dark", "Dark"]] as [id, label] (id)}
              <label
                ><input type="radio" name="theme" value={id} checked={(s.theme ?? "system") === id} onchange={() => change({ theme: id as Settings["theme"] })} />
                {label}</label
              >
            {/each}
          </div>
        </fieldset>
        <fieldset>
          <legend>Text size</legend>
          <div class="choices">
            {#each [["small", "Small"], ["normal", "Normal"], ["large", "Large"]] as [id, label] (id)}
              <label
                ><input type="radio" name="font-size" value={id} checked={(s.fontSize ?? "normal") === id} onchange={() => change({ fontSize: id as Settings["fontSize"] })} />
                {label}</label
              >
            {/each}
          </div>
        </fieldset>
        <fieldset>
          <legend>Rows in file lists</legend>
          <div class="choices">
            {#each [["normal", "Normal"], ["compact", "Compact"]] as [id, label] (id)}
              <label
                ><input type="radio" name="density" value={id} checked={(s.density ?? "normal") === id} onchange={() => change({ density: id as Settings["density"] })} />
                {label}</label
              >
            {/each}
          </div>
        </fieldset>
      {:else if section === "panes"}
        <label class="check"
          ><input type="checkbox" checked={s.showHidden ?? false} onchange={(e) => change({ showHidden: e.currentTarget.checked })} data-testid="settings-show-hidden" />
          Show hidden files</label
        >
        <fieldset>
          <legend>Columns in list view</legend>
          <div class="choices">
            <label><input type="checkbox" checked disabled /> Name</label>
            {#each COLUMNS as col (col.id)}
              <label
                ><input type="checkbox" checked={!(s.hiddenColumns ?? []).includes(col.id)} onchange={(e) => toggleColumn(col.id, e.currentTarget.checked)} />
                {col.label}</label
              >
            {/each}
          </div>
        </fieldset>
        {#each ["Left", "Right"] as side, i (side)}
          <fieldset>
            <legend>{side} pane opens at start</legend>
            <div class="choices">
              {#each START_MODES as m (m.id)}
                <label
                  ><input type="radio" name={`start-${i}`} checked={paneStart(i).mode === m.id} onchange={() => setPaneStart(i, { mode: m.id })} />
                  {m.label}</label
                >
              {/each}
            </div>
            {#if paneStart(i).mode === "folder"}
              <input
                type="text"
                class="wide"
                aria-label={`${side} pane folder`}
                placeholder="/path/to/folder"
                value={paneStart(i).folder ?? ""}
                onchange={(e) => setPaneStart(i, { folder: e.currentTarget.value.trim() || null })}
              />
            {/if}
          </fieldset>
        {/each}
        <p class="hint">Folders on this computer only; a pane starts in the home folder when its folder is missing.</p>
      {:else if section === "apps"}
        <label class="field">
          <span>Editor</span>
          <input
            type="text"
            value={s.editor ?? ""}
            placeholder="Each file's default app"
            onchange={(e) => change({ editor: e.currentTarget.value.trim() || null })}
            data-testid="settings-editor"
          />
        </label>
        <p class="hint">
          {mac
            ? "App used by Edit (F4), e.g. Visual Studio Code, BBEdit or /Applications/Sublime Text.app."
            : "Program used by Edit (F4), e.g. C:\\Program Files\\Notepad++\\notepad++.exe or code."}
        </p>
        <label class="field">
          <span>Compare app</span>
          <input
            type="text"
            list="settings-diff-tools"
            value={s.diffTool ?? ""}
            placeholder={diffTools[0] ? `${diffTools[0].name} (found)` : "e.g. meld {left} {right}"}
            onchange={(e) => change({ diffTool: e.currentTarget.value.trim() || null })}
            data-testid="settings-diff-tool"
          />
          <datalist id="settings-diff-tools">
            {#each diffTools as t (t.name)}<option value={t.command}>{t.name}</option>{/each}
          </datalist>
        </label>
        <p class="hint">
          The command uses {"{left}"} and {"{right}"} for the two files.{diffTools.length
            ? ` Found on this computer: ${diffTools.map((t) => t.name).join(", ")}.`
            : ""}
        </p>
        <label class="check"
          ><input type="checkbox" checked={updates.autoCheck} onchange={setAutoCheck} data-testid="settings-auto-update" />
          Check for updates automatically</label
        >
      {:else}
        <div class="key-tools">
          <input type="search" placeholder="Find a command or key" aria-label="Find a command or key" bind:value={query} />
          <button class="text" disabled={!anyChanged} onclick={() => saveBindings({})} data-testid="keys-reset-all">Reset All</button>
        </div>
        {#if keyNote}<p class="note" role="status">{keyNote}</p>{/if}
        {#if pending}
          <div class="note conflict" role="alert" data-testid="key-conflict">
            <span
              >{pending.combos.map((k) => formatCombo(k, mac)).join(", ")} {pending.combos.length > 1 ? "are" : "is"} used by {pending.others.map((o) => `“${titleOf(o)}”`).join(", ")}. Use it for “{titleOf(pending.id)}”
              instead?</span
            >
            <button class="primary" onclick={replaceConflict}>Replace</button>
            <button class="text" onclick={() => (pending = null)}>Cancel</button>
          </div>
        {/if}
        <ul class="keys" aria-label="Shortcuts">
          {#each shown as c (c.id)}
            {@const keys = prefs.bindings[c.id] ?? []}
            <li data-testid="key-row" data-command={c.id}>
              <span class="title">{c.title}<span class="group">{c.group}</span></span>
              <span class="combos">
                {#each keys as k (k)}
                  {#if recording?.id === c.id && recording.replacing === k}
                    <span class="combo recording">Press keys…</span>
                  {:else}
                    <span class="combo">
                      <button class="key" title="Click, then press the new keys" onclick={() => record(c.id, k)}>{formatCombo(k, mac)}</button>
                      <button class="remove" aria-label={`Remove ${formatCombo(k, mac)}`} onclick={() => removeKey(c.id, k)}>×</button>
                    </span>
                  {/if}
                {/each}
                {#if recording?.id === c.id && recording.replacing === null}
                  <span class="combo recording">Press keys…</span>
                {:else}
                  <button class="add" aria-label={`Add a shortcut for ${c.title}`} title="Add a shortcut" onclick={() => record(c.id, null)}>+</button>
                {/if}
                {#if changed(c.id)}
                  <button class="text" onclick={() => reset(c.id)}>Reset</button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
        <p class="hint">Click a shortcut, or +, then press the keys. Esc stops recording.</p>
      {/if}

      {#if error}<p class="err" role="alert">{error}</p>{/if}
    </div>
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
    display: flex;
    width: min(680px, calc(100vw - 48px));
    height: min(560px, calc(100vh - 48px));
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  .sections {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 150px;
    flex-shrink: 0;
    padding: 14px 8px;
    background: var(--sidebar);
    border-right: 1px solid var(--border);
  }
  .sections button {
    text-align: left;
    padding: 6px 10px;
    border-radius: 6px;
  }
  .sections button:hover {
    background: var(--control);
  }
  .sections button.current {
    background: var(--selection);
    color: var(--selection-text);
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 12px 20px 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  h2 {
    margin: 0;
    font-size: 16px;
  }
  .close {
    font-size: 16px;
    padding: 2px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  fieldset {
    border: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  legend {
    font-weight: 600;
    padding: 0;
    margin-bottom: 6px;
  }
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
  }
  label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .field {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 4px;
    font-weight: 600;
  }
  input[type="text"],
  input[type="search"] {
    font: inherit;
    font-weight: normal;
    color: var(--text);
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .wide {
    max-width: 420px;
  }
  .hint {
    margin: -6px 0 0;
    font-size: 12px;
    color: var(--muted);
  }
  .err {
    margin: 0;
    color: var(--danger);
  }
  .key-tools {
    display: flex;
    gap: 8px;
  }
  .key-tools input {
    flex: 1;
  }
  .text {
    color: var(--accent);
    padding: 2px 6px;
    border-radius: 5px;
  }
  .text:hover:not(:disabled) {
    background: var(--control);
  }
  .note {
    margin: 0;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--stripe);
  }
  .conflict {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    border: 1px solid var(--accent);
  }
  .conflict span {
    flex: 1;
    min-width: 200px;
  }
  .primary {
    padding: 3px 12px;
    border-radius: 6px;
    background: var(--accent);
    color: #fff;
  }
  .keys {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .keys li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
    padding: 2px 6px;
    border-radius: 5px;
  }
  .keys li:nth-child(even) {
    background: var(--stripe);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group {
    margin-left: 8px;
    font-size: 11px;
    color: var(--muted);
  }
  .combos {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 4px;
  }
  .combo {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--control);
    font-size: 12px;
  }
  .combo .key {
    padding: 1px 6px;
    font-variant-numeric: tabular-nums;
  }
  .combo .remove {
    padding: 1px 5px;
    color: var(--muted);
    border-left: 1px solid var(--border);
  }
  .combo .key:hover,
  .combo .remove:hover,
  .add:hover {
    background: var(--control-hover);
  }
  .combo.recording {
    padding: 1px 8px;
    border-color: var(--accent);
    color: var(--accent);
  }
  .add {
    padding: 0 7px;
    border-radius: 5px;
    color: var(--muted);
  }
</style>

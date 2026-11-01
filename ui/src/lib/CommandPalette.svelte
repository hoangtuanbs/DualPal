<script lang="ts" module>
  export interface Command {
    id: string;
    title: string;
    /** Group shown on the right, e.g. "File" or "Go". */
    group: string;
    shortcut?: string;
    run: () => void;
  }

  /** Score how well `query` matches `text` as a subsequence; null if not at all.
   *  Matches at word starts and runs of consecutive letters score higher. */
  export function fuzzyScore(query: string, text: string): number | null {
    const q = query.toLowerCase().replace(/\s+/g, "");
    if (!q) return 0;
    const t = text.toLowerCase();
    let score = 0;
    let ti = 0;
    let run = 0;
    for (const c of q) {
      const found = t.indexOf(c, ti);
      if (found < 0) return null;
      const wordStart = found === 0 || /[\s\-_/:.(]/.test(t[found - 1]);
      run = found === ti ? run + 1 : 0;
      score += 1 + (wordStart ? 6 : 0) + run * 3 - Math.min(found - ti, 5) * 0.5;
      ti = found + 1;
    }
    return score - t.length * 0.01;
  }
</script>

<script lang="ts">
  /** Find and run any command by typing part of its name (Ctrl/⌘ Shift P). */
  let { commands, onclose }: { commands: Command[]; onclose: () => void } = $props();

  let query = $state("");
  let index = $state(0);
  let field: HTMLInputElement | undefined = $state();
  let list: HTMLElement | undefined = $state();

  const matches = $derived.by(() => {
    if (!query.trim()) return commands;
    return commands
      .map((c) => ({ c, s: fuzzyScore(query, `${c.title} ${c.group}`) }))
      .filter((m) => m.s !== null)
      .sort((a, b) => b.s! - a.s!)
      .map((m) => m.c);
  });

  $effect(() => {
    field?.focus();
  });

  function run(c: Command | undefined) {
    if (!c) return;
    onclose();
    c.run();
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = matches.length;
      if (!n) return;
      index = (index + (e.key === "ArrowDown" ? 1 : -1) + n) % n;
      list?.children[index]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(matches[index]);
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="palette" role="dialog" tabindex="-1" aria-modal="true" aria-label="Command palette" onclick={(e) => e.stopPropagation()}>
    <input
      bind:this={field}
      bind:value={query}
      oninput={() => (index = 0)}
      onkeydown={keydown}
      placeholder="Type a command, a server or a favorite…"
      spellcheck="false"
      autocomplete="off"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-list"
      aria-activedescendant="cmd-{index}"
    />
    <ul id="palette-list" bind:this={list} role="listbox">
      {#each matches as c, i (c.id)}
        <li
          id="cmd-{i}"
          role="option"
          aria-selected={i === index}
          class:active={i === index}
          onmousemove={() => (index = i)}
          onclick={() => run(c)}
        >
          <span class="title">{c.title}</span>
          <span class="group">{c.group}</span>
          {#if c.shortcut}<kbd>{c.shortcut}</kbd>{/if}
        </li>
      {/each}
      {#if matches.length === 0}
        <li class="empty">No matching commands</li>
      {/if}
    </ul>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.2);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 11;
  }
  .palette {
    width: min(560px, calc(100vw - 32px));
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 60px rgb(0 0 0 / 0.35);
    overflow: hidden;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    border: none;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 15px;
    padding: 12px 14px;
    outline: none;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 50vh;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-radius: 7px;
  }
  li.active {
    background: var(--selection);
    color: var(--selection-text);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group {
    font-size: 11px;
    opacity: 0.65;
  }
  kbd {
    font: inherit;
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--control);
    color: var(--text);
  }
  li.active kbd {
    background: rgb(255 255 255 / 0.2);
    color: inherit;
  }
  .empty {
    color: var(--muted);
  }
</style>

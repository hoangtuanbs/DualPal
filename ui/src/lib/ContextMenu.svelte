<script lang="ts" module>
  export type MenuItem =
    | {
        label: string;
        icon?: string;
        shortcut?: string;
        danger?: boolean;
        disabled?: boolean;
        run: () => void;
      }
    | "separator";
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";

  /** A right-click menu at (x, y), in window coordinates. */
  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  let menu: HTMLDivElement | undefined = $state();
  let left = $state(0);
  let top = $state(0);
  let focused = $state(-1);

  const actions = $derived(items.flatMap((it, i) => (it !== "separator" && !it.disabled ? [i] : [])));

  onMount(async () => {
    left = x;
    top = y;
    await tick();
    // Keep the whole menu on screen.
    const r = menu!.getBoundingClientRect();
    if (left + r.width > innerWidth - 4) left = Math.max(4, innerWidth - r.width - 4);
    if (top + r.height > innerHeight - 4) top = Math.max(4, top - r.height);
    menu!.focus();
  });

  function choose(i: number) {
    const it = items[i];
    if (it === "separator" || it.disabled) return;
    onclose();
    it.run();
  }

  function keydown(e: KeyboardEvent) {
    e.stopPropagation();
    const at = actions.indexOf(focused);
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      onclose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      focused = actions[(at + 1) % actions.length];
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      focused = actions[(at <= 0 ? actions.length : at) - 1];
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      focused = e.key === "Home" ? actions[0] : actions[actions.length - 1];
    } else if ((e.key === "Enter" || e.key === " ") && focused >= 0) {
      e.preventDefault();
      choose(focused);
    }
  }
</script>

<svelte:window onblur={onclose} onresize={onclose} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="catcher"
  onpointerdown={(e) => e.target === e.currentTarget && onclose()}
  oncontextmenu={(e) => {
    e.preventDefault();
    onclose();
  }}
>
  <div class="menu" role="menu" tabindex="-1" bind:this={menu} style:left="{left}px" style:top="{top}px" onkeydown={keydown}>
    {#each items as it, i}
      {#if it === "separator"}
        <div class="sep" role="separator"></div>
      {:else}
        <button
          role="menuitem"
          class:danger={it.danger}
          class:focused={focused === i}
          disabled={it.disabled}
          tabindex="-1"
          onpointerenter={() => (focused = it.disabled ? -1 : i)}
          onclick={() => choose(i)}
        >
          <span class="icon">{#if it.icon}<Icon name={it.icon} size={15} />{/if}</span>
          <span class="label">{it.label}</span>
          {#if it.shortcut}<span class="shortcut">{it.shortcut}</span>{/if}
        </button>
      {/if}
    {/each}
  </div>
</div>

<style>
  .catcher {
    position: fixed;
    inset: 0;
    z-index: 20;
  }
  .menu {
    position: fixed;
    min-width: 220px;
    max-width: 360px;
    padding: 4px;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.25);
    outline: none;
    font-size: 13px;
  }
  button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 5px 8px;
    border-radius: 5px;
    text-align: left;
    color: inherit;
  }
  button.focused {
    background: var(--accent);
    color: white;
  }
  button:disabled {
    opacity: 0.45;
  }
  button.danger:not(.focused) {
    color: var(--danger);
  }
  .icon {
    display: inline-flex;
    width: 16px;
    justify-content: center;
  }
  .label {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .shortcut {
    opacity: 0.6;
    font-size: 12px;
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
</style>

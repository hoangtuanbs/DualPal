<script lang="ts">
  import { untrack } from "svelte";
  /** A small in-window modal for confirmations and name prompts. */
  let {
    title,
    message = "",
    input = null,
    confirmLabel = "OK",
    danger = false,
    onconfirm,
    oncancel,
  }: {
    title: string;
    message?: string;
    /** Initial text; null means no text field. */
    input?: string | null;
    confirmLabel?: string;
    danger?: boolean;
    onconfirm: (value: string) => void;
    oncancel: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  let value = $state(input ?? "");
  let field: HTMLInputElement | undefined = $state();
  let okButton: HTMLButtonElement | undefined = $state();

  // Focus once on open. `untrack` keeps this from re-running (and
  // re-selecting the text) on every keystroke.
  $effect(() => {
    const f = field;
    const ok = okButton;
    untrack(() => {
      if (f) {
        f.focus();
        // Select the name without its extension, like Finder and Explorer.
        const dot = value.lastIndexOf(".");
        f.setSelectionRange(0, dot > 0 ? dot : value.length);
      } else {
        ok?.focus();
      }
    });
  });

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (input !== null && !value.trim()) return;
    onconfirm(value.trim());
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      oncancel();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" onkeydown={keydown}>
  <div class="dialog" role="dialog" aria-modal="true" aria-label={title}>
  <form onsubmit={submit}>
    <h2>{title}</h2>
    {#if message}<p>{message}</p>{/if}
    {#if input !== null}
      <input bind:this={field} bind:value spellcheck="false" autocomplete="off" />
    {/if}
    <div class="buttons">
      <button type="button" class="secondary" onclick={oncancel}>Cancel</button>
      <button type="submit" class="primary" class:danger bind:this={okButton}>{confirmLabel}</button>
    </div>
  </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.35);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    width: min(420px, calc(100vw - 32px));
    padding: 18px;
    border-radius: 12px;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
  }
  h2 {
    margin: 0 0 8px;
    font-size: 15px;
  }
  p {
    margin: 0 0 12px;
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  input {
    width: 100%;
    box-sizing: border-box;
    padding: 6px 8px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--control);
    color: var(--text);
    font: inherit;
    margin-bottom: 14px;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .buttons button {
    padding: 5px 14px;
    border-radius: 6px;
    font-weight: 500;
  }
  .secondary {
    background: var(--control);
  }
  .primary {
    background: var(--accent);
    color: white;
  }
  .primary.danger {
    background: var(--danger);
  }
</style>

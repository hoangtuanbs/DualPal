<script lang="ts">
  import { api, errorMessage } from "./api";
  import { releaseNotesUrl } from "./updates";
  import { updates } from "./updates.svelte";

  let notesError = $state<string | null>(null);

  const update = $derived(updates.update);
  const visible = $derived(!updates.dismissed && updates.phase !== "idle");

  async function openNotes() {
    if (!update) return;
    try {
      notesError = null;
      await api.openUrl(releaseNotesUrl(update.version));
    } catch (e) {
      notesError = errorMessage(e);
    }
  }
</script>

{#if visible}
  <div class="bar" role="status" aria-live="polite" data-testid="update-bar">
    {#if updates.phase === "checking"}
      <span>Checking for updates…</span>
    {:else if updates.phase === "upToDate"}
      <span>You're up to date.</span>
      <button class="ghost" onclick={() => updates.dismiss()}>Close</button>
    {:else if updates.phase === "error"}
      <span class="err">{updates.message}</span>
      {#if update}
        <button onclick={() => updates.install()}>Try again</button>
      {/if}
      <button class="ghost" onclick={() => updates.dismiss()}>Close</button>
    {:else if updates.waitingForOperations}
      <span>File operations are still running. Wait for them to finish, or install now?</span>
      <button class="primary" onclick={() => updates.install(true)}>Install anyway</button>
      <button class="ghost" onclick={() => updates.keepWaiting()}>Wait</button>
    {:else if updates.phase === "downloading"}
      <span>Downloading Dual Pal {update?.version}… {updates.progress === null ? "" : `${updates.progress}%`}</span>
    {:else if updates.phase === "installing"}
      <span>Installing Dual Pal {update?.version}…</span>
    {:else if update}
      <span>Dual Pal {update.version} is available.</span>
      <button class="ghost" onclick={openNotes}>Release notes</button>
      <button class="primary" onclick={() => updates.install()}>Install and restart</button>
      <button class="ghost" onclick={() => updates.dismiss()}>Later</button>
    {/if}
    {#if notesError}
      <span class="err">{notesError}</span>
    {/if}
  </div>
{/if}

<style>
  .bar {
    position: fixed;
    right: 16px;
    bottom: 16px;
    /* Above the About sheet's backdrop, so it can still be used from there. */
    z-index: 10;
    max-width: calc(100vw - 32px);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    font-size: 13px;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.25);
  }
  .bar button {
    padding: 5px 10px;
    border-radius: 7px;
    background: var(--control);
  }
  .bar button:hover {
    background: var(--control-hover);
  }
  .bar button.primary {
    background: var(--accent);
    color: white;
  }
  .bar button.ghost {
    background: transparent;
    color: var(--muted);
  }
  .err {
    color: var(--danger);
  }
</style>

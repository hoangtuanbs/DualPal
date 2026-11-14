<script lang="ts">
  import { errorMessage } from "./api";
  import { updates } from "./updates.svelte";

  let notesError = $state<string | null>(null);

  const visible = $derived(!updates.dismissed && updates.phase !== "idle");
  const installed = $derived(updates.currentVersion ? ` You have ${updates.currentVersion}.` : "");

  async function openReleasePage() {
    try {
      notesError = null;
      await updates.openReleasePage();
    } catch (e) {
      notesError = errorMessage(e);
    }
  }
</script>

{#if visible}
  <!-- Keys pressed on the bar's buttons are theirs, not file pane shortcuts. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="bar" role="status" aria-live="polite" data-testid="update-bar" onkeydown={(e) => e.stopPropagation()}>
    {#if updates.phase === "checking"}
      <span>Checking for updates…</span>
    {:else if updates.phase === "upToDate"}
      <span>You're up to date.</span>
      <button class="ghost" onclick={() => updates.dismiss()}>Close</button>
    {:else if updates.phase === "error"}
      <span class="err">{updates.message}</span>
      {#if updates.installable}
        <button onclick={() => updates.install()}>Try again</button>
      {/if}
      {#if updates.version}
        <button class="ghost" onclick={openReleasePage}>Release page</button>
      {/if}
      <button class="ghost" onclick={() => updates.dismiss()}>Close</button>
    {:else if updates.waitingForOperations}
      <span>File operations are still running. Wait for them to finish, or install now?</span>
      <button class="primary" onclick={() => updates.install(true)}>Install anyway</button>
      <button class="ghost" onclick={() => updates.keepWaiting()}>Wait</button>
    {:else if updates.phase === "downloading"}
      <span>Downloading Dual Pal {updates.version}… {updates.progress === null ? "" : `${updates.progress}%`}</span>
    {:else if updates.phase === "installing"}
      <span>Installing Dual Pal {updates.version}…</span>
    {:else if updates.phase === "handedOff"}
      <span>The Dual Pal {updates.version} installer is open. Follow its steps to finish the upgrade.</span>
      <button class="ghost" onclick={() => updates.dismiss()}>Close</button>
    {:else if updates.installable}
      <span>Dual Pal {updates.version} is available.{installed}</span>
      <button class="ghost" onclick={openReleasePage}>Release notes</button>
      <button class="primary" onclick={() => updates.install()}>Install and restart</button>
      <button class="ghost" onclick={() => updates.skip()}>Skip this version</button>
      <button class="ghost" onclick={() => updates.dismiss()}>Later</button>
    {:else if updates.version}
      <!-- No in-app install for this release: send the person to its download page. -->
      <span>Dual Pal {updates.version} is available.{installed} Download it from the release page to upgrade.</span>
      <button class="primary" onclick={openReleasePage}>Download…</button>
      <button class="ghost" onclick={() => updates.skip()}>Skip this version</button>
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

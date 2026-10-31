<script lang="ts">
  import type { EditSession } from "./api";
  import { formatTime } from "./format";

  /** Remote files open in an editor; each saved change is uploaded again. */
  let { sessions, onstop }: { sessions: EditSession[]; onstop: (id: string) => void } = $props();

  function name(s: EditSession) {
    return s.remote.path.split("/").filter(Boolean).pop() ?? s.remote.path;
  }

  function status(s: EditSession) {
    switch (s.status) {
      case "uploading":
        return "Uploading…";
      case "uploaded":
        return `Uploaded ${s.lastUploadMs ? formatTime(s.lastUploadMs) : ""}`;
      case "failed":
        return `Upload failed: ${s.error ?? ""}`;
      default:
        return "Uploads when you save";
    }
  }
</script>

{#if sessions.length > 0}
  <div class="edits" aria-label="Files being edited">
    <span class="label">Editing</span>
    {#each sessions as s (s.id)}
      <span class="chip" class:failed={s.status === "failed"} title="{s.remote.path}\nLocal copy: {s.localPath}">
        <span class="file">{name(s)}</span>
        <span class="status">{status(s)}</span>
        <button title="Stop uploading changes and delete the local copy" aria-label="Stop editing {name(s)}" onclick={() => onstop(s.id)}
          >×</button
        >
      </span>
    {/each}
  </div>
{/if}

<style>
  .edits {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 12px;
    background: var(--chrome);
    border-top: 1px solid var(--border);
    overflow-x: auto;
    font-size: 12px;
  }
  .label {
    color: var(--muted);
    flex-shrink: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px 2px 9px;
    border-radius: 999px;
    background: var(--control);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .chip.failed {
    color: var(--danger);
  }
  .file {
    font-weight: 500;
  }
  .status {
    color: var(--muted);
  }
  .chip.failed .status {
    color: inherit;
  }
  button {
    color: var(--muted);
    padding: 0 4px;
    font-size: 13px;
    line-height: 1;
  }
  button:hover {
    color: var(--text);
  }
</style>

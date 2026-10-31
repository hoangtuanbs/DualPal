<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, errorMessage, type Connection, type SshConfigHost } from "./api";

  /** Add or edit an SFTP or S3 connection. */
  let {
    connection,
    onsave,
    oncancel,
  }: {
    /** The connection to edit, or null to add a new one. */
    connection: Connection | null;
    /** Saved; `connect` is true for "Connect". */
    onsave: (saved: Connection, connect: boolean) => void;
    oncancel: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  const start = connection;
  const sftp0 = start?.endpoint.type === "sftp" ? start.endpoint : null;
  const s30 = start?.endpoint.type === "s3" ? start.endpoint : null;

  let type = $state<"sftp" | "s3">(start?.endpoint.type ?? "sftp");
  let name = $state(start?.name ?? "");
  let initialPath = $state(start?.initialPath ?? "");
  // SFTP
  let host = $state(sftp0?.host ?? "");
  let port = $state(sftp0?.port ?? 22);
  let user = $state(sftp0?.user ?? "");
  let method = $state<"password" | "keyFile" | "agent">(sftp0?.auth.method ?? "password");
  let keyPath = $state(sftp0?.auth.method === "keyFile" ? sftp0.auth.path : "~/.ssh/id_ed25519");
  // S3
  let bucket = $state(s30?.bucket ?? "");
  let region = $state(s30?.region ?? "us-east-1");
  let endpoint = $state(s30?.endpoint ?? "");
  let accessKeyId = $state(s30?.accessKeyId ?? "");
  let pathStyle = $state(s30?.pathStyle ?? false);
  /** Typed secret; empty while editing means "keep the saved one". */
  let secret = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);
  let sshHosts = $state<SshConfigHost[]>([]);
  let first: HTMLInputElement | undefined = $state();

  onMount(() => {
    api.sshConfigHosts().then((h) => (sshHosts = h), () => {});
  });

  $effect(() => {
    const f = first;
    untrack(() => f?.focus());
  });

  const secretLabel = $derived(
    type === "s3" ? "Secret access key" : method === "keyFile" ? "Key passphrase" : method === "password" ? "Password" : null,
  );

  function importHost(alias: string) {
    const h = sshHosts.find((x) => x.alias === alias);
    if (!h) return;
    type = "sftp";
    name = name || h.alias;
    host = h.hostName;
    if (h.user) user = h.user;
    port = h.port ?? 22;
    if (h.identityFile) {
      method = "keyFile";
      keyPath = h.identityFile;
    } else {
      method = "agent";
    }
  }

  function build(): Connection {
    return {
      id: start?.id ?? "",
      name: name.trim() || (type === "sftp" ? host.trim() : bucket.trim()),
      initialPath: initialPath.trim() || null,
      favorites: start?.favorites ?? [],
      endpoint:
        type === "sftp"
          ? {
              type: "sftp",
              host: host.trim(),
              port: Number(port) || 22,
              user: user.trim(),
              auth: method === "keyFile" ? { method, path: keyPath.trim() } : { method },
            }
          : {
              type: "s3",
              bucket: bucket.trim(),
              region: region.trim(),
              endpoint: endpoint.trim() || null,
              accessKeyId: accessKeyId.trim(),
              pathStyle,
            },
    };
  }

  async function save(connect: boolean) {
    busy = true;
    error = null;
    try {
      // An agent sign-in has no secret; clear any old one.
      const s = secretLabel === null ? "" : secret ? secret : null;
      const saved = await api.saveConnection(build(), s);
      onsave(saved, connect);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
    }
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
  <form
    class="dialog"
    onsubmit={(e) => {
      e.preventDefault();
      save(true);
    }}
  >
    <h2>{start ? "Edit server" : "Add server"}</h2>

    <div class="kind" role="radiogroup" aria-label="Kind">
      <button type="button" role="radio" aria-checked={type === "sftp"} onclick={() => (type = "sftp")}>SFTP / SSH</button>
      <button type="button" role="radio" aria-checked={type === "s3"} onclick={() => (type = "s3")}>Amazon S3 / compatible</button>
    </div>

    {#if type === "sftp" && sshHosts.length > 0 && !start}
      <label class="row">
        <span>From ~/.ssh/config</span>
        <select onchange={(e) => importHost(e.currentTarget.value)}>
          <option value="">Choose a host…</option>
          {#each sshHosts as h (h.alias)}
            <option value={h.alias}>{h.alias}{h.hostName !== h.alias ? ` (${h.hostName})` : ""}</option>
          {/each}
        </select>
      </label>
    {/if}

    <label class="row">
      <span>Name</span>
      <input bind:this={first} bind:value={name} placeholder={type === "sftp" ? "My server" : "My bucket"} />
    </label>

    {#if type === "sftp"}
      <div class="row">
        <span>Server</span>
        <div class="pair">
          <input bind:value={host} placeholder="example.com" required autocapitalize="off" spellcheck="false" />
          <input class="port" type="number" min="1" max="65535" bind:value={port} aria-label="Port" />
        </div>
      </div>
      <label class="row">
        <span>User</span>
        <input bind:value={user} required autocapitalize="off" spellcheck="false" />
      </label>
      <label class="row">
        <span>Sign in with</span>
        <select bind:value={method}>
          <option value="password">Password</option>
          <option value="keyFile">Private key file</option>
          <option value="agent">SSH agent and default keys</option>
        </select>
      </label>
      {#if method === "keyFile"}
        <label class="row">
          <span>Key file</span>
          <input bind:value={keyPath} required spellcheck="false" />
        </label>
      {/if}
    {:else}
      <label class="row">
        <span>Bucket</span>
        <input bind:value={bucket} required autocapitalize="off" spellcheck="false" />
      </label>
      <label class="row">
        <span>Region</span>
        <input bind:value={region} placeholder="us-east-1" spellcheck="false" />
      </label>
      <label class="row">
        <span>Endpoint</span>
        <input bind:value={endpoint} placeholder="Leave empty for Amazon S3" spellcheck="false" />
      </label>
      <label class="row">
        <span>Access key ID</span>
        <input bind:value={accessKeyId} required autocapitalize="off" spellcheck="false" />
      </label>
    {/if}

    {#if secretLabel}
      <label class="row">
        <span>{secretLabel}</span>
        <input
          type="password"
          bind:value={secret}
          placeholder={start ? "Unchanged" : method === "keyFile" ? "If the key has one" : ""}
          autocomplete="off"
        />
      </label>
    {/if}

    {#if type === "s3"}
      <label class="check">
        <input type="checkbox" bind:checked={pathStyle} />
        Path-style addresses (MinIO and most self-hosted services)
      </label>
    {/if}

    <label class="row">
      <span>Open folder</span>
      <input bind:value={initialPath} placeholder={type === "sftp" ? "Home folder" : "/"} spellcheck="false" />
    </label>

    <p class="hint">
      {#if type === "sftp"}
        Passwords and passphrases are kept in the {navigator.userAgent.includes("Windows")
          ? "Windows Credential Manager"
          : "macOS Keychain"}, never in Dual Pal's settings.
      {:else}
        The secret key is kept in the {navigator.userAgent.includes("Windows")
          ? "Windows Credential Manager"
          : "macOS Keychain"}.
      {/if}
    </p>

    {#if error}<p class="error">{error}</p>{/if}

    <div class="buttons">
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="button" disabled={busy} onclick={() => save(false)}>Save</button>
      <button type="submit" class="primary" disabled={busy}>Connect</button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 0.3);
    display: grid;
    place-items: center;
    z-index: 10;
  }
  .dialog {
    width: min(480px, calc(100vw - 32px));
    max-height: calc(100vh - 40px);
    overflow-y: auto;
    box-sizing: border-box;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 18px 20px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.3);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .kind {
    display: flex;
    gap: 1px;
    background: var(--border);
    border-radius: 7px;
    overflow: hidden;
    align-self: flex-start;
  }
  .kind button {
    padding: 5px 12px;
    background: var(--control);
  }
  .kind button[aria-checked="true"] {
    background: var(--accent);
    color: white;
  }
  .row {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr);
    align-items: center;
    gap: 10px;
  }
  .row > span {
    color: var(--muted);
    text-align: right;
    font-size: 12px;
  }
  .pair {
    display: flex;
    gap: 6px;
  }
  .pair input:first-child {
    flex: 1;
    min-width: 0;
  }
  .port {
    width: 72px;
  }
  input,
  select {
    font: inherit;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--control);
    color: inherit;
    min-width: 0;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: 130px;
    font-size: 12px;
  }
  .hint {
    margin: 0 0 0 130px;
    font-size: 12px;
    color: var(--muted);
  }
  .error {
    margin: 0;
    color: var(--danger);
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  .buttons button {
    padding: 5px 14px;
    border-radius: 6px;
    background: var(--control);
    border: 1px solid var(--border);
  }
  .buttons .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }
  @media (max-width: 520px) {
    .row {
      grid-template-columns: 1fr;
      gap: 4px;
    }
    .row > span {
      text-align: left;
    }
    .check,
    .hint {
      margin-left: 0;
    }
  }
</style>

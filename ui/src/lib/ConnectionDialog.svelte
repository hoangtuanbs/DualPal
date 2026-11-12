<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, errorMessage, type Connection, type FtpSecurity, type SshConfigHost } from "./api";

  /** Add or edit an SFTP, FTP/FTPS, WebDAV or S3 connection. */
  let {
    connection,
    onsave,
    oncancel,
    onimport,
  }: {
    /** The connection to edit, or null to add a new one. */
    connection: Connection | null;
    /** Saved; `connect` is true for "Connect". */
    onsave: (saved: Connection, connect: boolean) => void;
    oncancel: () => void;
    /** Open the import dialog instead (add mode only). */
    onimport?: () => void;
  } = $props();

  // svelte-ignore state_referenced_locally
  const start = connection;
  const sftp0 = start?.endpoint.type === "sftp" ? start.endpoint : null;
  const s30 = start?.endpoint.type === "s3" ? start.endpoint : null;
  const ftp0 = start?.endpoint.type === "ftp" ? start.endpoint : null;
  const dav0 = start?.endpoint.type === "webDav" ? start.endpoint : null;

  type Kind = "sftp" | "ftp" | "webDav" | "s3";
  let type = $state<Kind>(start?.endpoint.type ?? "sftp");
  let name = $state(start?.name ?? "");
  let initialPath = $state(start?.initialPath ?? "");
  // SFTP
  // SFTP and FTP
  let host = $state(sftp0?.host ?? ftp0?.host ?? "");
  let port = $state(sftp0?.port ?? ftp0?.port ?? 22);
  let user = $state(sftp0?.user ?? ftp0?.user ?? dav0?.user ?? "");
  // FTP
  let security = $state<FtpSecurity>(ftp0?.security ?? "explicit");
  // WebDAV
  let url = $state(dav0?.url ?? "");

  const DEFAULT_PORTS = [22, 21, 990];
  /** The usual port for the kind of server, unless the user typed another. */
  function defaultPort() {
    const want = type === "sftp" ? 22 : security === "implicit" ? 990 : 21;
    if (DEFAULT_PORTS.includes(Number(port))) port = want;
  }
  function choose(k: Kind) {
    type = k;
    defaultPort();
  }

  /** "root@10.0.0.5" typed as the host: move the user to its own field. */
  function splitUserHost() {
    const at = host.lastIndexOf("@");
    if (at < 0) return;
    const u = host.slice(0, at).trim();
    host = host.slice(at + 1).trim();
    if (u && !user.trim()) user = u;
  }
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
    type === "s3"
      ? "Secret access key"
      : type === "ftp" || type === "webDav"
        ? "Password"
        : method === "keyFile"
          ? "Key passphrase"
          : method === "password"
            ? "Password"
            : null,
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
    if (type === "sftp" || type === "ftp") splitUserHost();
    const fallbackName =
      type === "s3" ? bucket.trim() : type === "webDav" ? url.trim().replace(/^https?:\/\//i, "").split("/")[0] : host.trim();
    return {
      id: start?.id ?? "",
      name: name.trim() || fallbackName,
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
          : type === "ftp"
            ? {
                type: "ftp",
                host: host.trim(),
                port: Number(port) || (security === "implicit" ? 990 : 21),
                user: user.trim(),
                security,
              }
          : type === "webDav"
            ? { type: "webDav", url: url.trim(), user: user.trim() }
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
    data-testid="connection-dialog"
    onsubmit={(e) => {
      e.preventDefault();
      save(true);
    }}
  >
    <h2>{start ? "Edit server" : "Add server"}</h2>

    <div class="kind" role="radiogroup" aria-label="Kind">
      <button type="button" role="radio" aria-checked={type === "sftp"} onclick={() => choose("sftp")}>SFTP / SSH</button>
      <button type="button" role="radio" aria-checked={type === "ftp"} onclick={() => choose("ftp")}>FTP / FTPS</button>
      <button type="button" role="radio" aria-checked={type === "webDav"} onclick={() => choose("webDav")}>WebDAV</button>
      <button type="button" role="radio" aria-checked={type === "s3"} onclick={() => choose("s3")}>S3</button>
    </div>

    {#if !start && onimport}
      <button type="button" class="link" onclick={onimport}>Import from WinSCP, FileZilla or Cyberduck…</button>
    {/if}

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
      <input bind:this={first} bind:value={name} placeholder={type === "s3" ? "My bucket" : "My server"} />
    </label>

    {#if type === "sftp" || type === "ftp"}
      <div class="row">
        <span>Server</span>
        <div class="pair">
          <input
            bind:value={host}
            onblur={splitUserHost}
            placeholder="example.com"
            required
            autocapitalize="off"
            spellcheck="false"
          />
          <input class="port" type="number" min="1" max="65535" bind:value={port} aria-label="Port" />
        </div>
      </div>
      <label class="row">
        <span>User</span>
        <input
          bind:value={user}
          required={type === "sftp" && !host.includes("@")}
          placeholder={type === "ftp" ? "Empty for anonymous" : ""}
          autocapitalize="off"
          spellcheck="false"
        />
      </label>
    {/if}
    {#if type === "ftp"}
      <label class="row">
        <span>Encryption</span>
        <select bind:value={security} onchange={defaultPort}>
          <option value="explicit">FTPS: TLS after connecting (usually port 21)</option>
          <option value="implicit">FTPS: TLS from the start (usually port 990)</option>
          <option value="none">None: plain FTP (not encrypted)</option>
        </select>
      </label>
    {:else if type === "webDav"}
      <label class="row">
        <span>Address</span>
        <input
          bind:value={url}
          required
          type="url"
          placeholder="https://cloud.example.com/remote.php/dav/files/me"
          autocapitalize="off"
          spellcheck="false"
        />
      </label>
      <label class="row">
        <span>User</span>
        <input bind:value={user} autocapitalize="off" spellcheck="false" />
      </label>
    {:else if type === "sftp"}
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
    {:else if type === "s3"}
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
      <input
        bind:value={initialPath}
        placeholder={type === "sftp" || type === "ftp" ? "Home folder" : "/"}
        spellcheck="false"
      />
    </label>

    <p class="hint">
      {#if type === "ftp" && security === "none"}
        Plain FTP sends the password and files unencrypted. Use it only on a network you trust.
      {:else if type !== "s3"}
        Passwords and passphrases are kept in the {navigator.userAgent.includes("Windows")
          ? "Windows Credential Manager"
          : navigator.userAgent.includes("Mac")
            ? "macOS Keychain"
            : "system keyring"}, never in Dual Pal's settings.
      {:else}
        The secret key is kept in the {navigator.userAgent.includes("Windows")
          ? "Windows Credential Manager"
          : navigator.userAgent.includes("Mac")
            ? "macOS Keychain"
            : "system keyring"}.
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
    flex-wrap: wrap;
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
  .link {
    align-self: flex-start;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
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

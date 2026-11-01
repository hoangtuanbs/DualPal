<script lang="ts">
  import { api, errorMessage, type AboutInfo } from "./api";
  import { ABOUT, copyText, osName, supportMailUrl } from "./about";
  import Icon from "./Icon.svelte";
  import appIcon from "../assets/app-icon.png";
  import donateQr from "../assets/donate-qr.svg";

  /** The About window, and the Donate page it links to. */
  let { start = "about", onclose }: { start?: "about" | "donate"; onclose: () => void } = $props();

  // svelte-ignore state_referenced_locally
  let page = $state(start);

  let info = $state<AboutInfo | null>(null);
  let copied = $state<"address" | "email" | null>(null);
  let error = $state<string | null>(null);

  api.aboutInfo().then((i) => (info = i));

  async function open(url: string) {
    try {
      error = null;
      await api.openUrl(url);
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function copy(what: "address" | "email") {
    const ok = await copyText(what === "address" ? ABOUT.ethereumAddress : ABOUT.supportEmail);
    if (!ok) {
      error = "Could not copy. Select the text and copy it instead.";
      return;
    }
    copied = what;
    setTimeout(() => (copied = null), 1500);
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={keydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="sheet" role="dialog" aria-modal="true" aria-label={page === "about" ? "About Dual Pal" : "Donate"}>
    <!-- svelte-ignore a11y_autofocus -->
    <button class="close" onclick={onclose} aria-label="Close" autofocus>×</button>

    {#if page === "about"}
      <img class="app-icon" src={appIcon} alt="" width="96" height="96" />
      <h2>Dual Pal</h2>
      <p class="muted">Version {info?.version ?? "…"}{info ? ` · ${osName(info)}` : ""}</p>
      <p class="tagline">A fast dual-pane file manager for your disks, SFTP servers and S3 buckets.</p>

      <div class="credits">
        <strong>Made by {ABOUT.author}</strong>
        {#each ABOUT.companies as company (company)}
          <span>{company}</span>
        {/each}
      </div>

      <div class="links">
        <button onclick={() => open(ABOUT.projectUrl)}><Icon name="github" size={15} /> Dual Pal on GitHub</button>
        <button onclick={() => info && open(supportMailUrl(info))}><Icon name="mail" size={15} /> Email Us…</button>
        <button class="donate" onclick={() => (page = "donate")}><Icon name="heart" size={15} filled /> Donate…</button>
      </div>
      <p class="small">
        Help: <span class="select">{ABOUT.supportEmail}</span>
        <button class="link" onclick={() => copy("email")}>{copied === "email" ? "Copied" : "Copy"}</button>
      </p>
    {:else}
      <span class="heart"><Icon name="heart" size={32} filled /></span>
      <h2>Support Dual Pal</h2>
      <p class="muted center">
        Dual Pal is free. If it saves you time, you can say thanks with a donation in Ethereum.
      </p>
      <img class="qr" src={donateQr} alt="QR code for the Ethereum address" width="220" height="220" />
      <p class="address select">{ABOUT.ethereumAddress}</p>
      <button class="copy" onclick={() => copy("address")}>
        <Icon name={copied === "address" ? "check" : "copy"} size={15} />
        {copied === "address" ? "Copied" : "Copy Address"}
      </button>
      <p class="small center">Ethereum (ETH) address. Please double-check the address in your wallet before sending.</p>
      <button class="link" onclick={() => (page = "about")}>About Dual Pal</button>
    {/if}

    {#if error}
      <p class="small err">{error}</p>
    {/if}
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
    position: relative;
    width: min(420px, calc(100vw - 48px));
    max-height: calc(100vh - 48px);
    overflow: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 28px 26px 22px;
    background: var(--pane-bg);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 50px rgb(0 0 0 / 0.35);
  }
  .close {
    position: absolute;
    top: 8px;
    right: 10px;
    font-size: 16px;
    padding: 2px 8px;
    border-radius: 6px;
    color: var(--muted);
  }
  .app-icon {
    width: 96px;
    height: 96px;
  }
  h2 {
    margin: 0;
    font-size: 20px;
  }
  p {
    margin: 0;
  }
  .muted {
    color: var(--muted);
  }
  .center,
  .tagline {
    text-align: center;
  }
  .credits {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    margin: 6px 0;
    font-size: 12px;
  }
  .links {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
  }
  .links button,
  .copy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    border-radius: 7px;
    background: var(--control);
  }
  .links button:hover,
  .copy:hover {
    background: var(--control-hover);
  }
  .donate :global(.icon),
  .heart {
    color: #e0245e;
  }
  .heart {
    margin-top: 4px;
  }
  .qr {
    width: 220px;
    height: 220px;
    padding: 12px;
    background: white;
    border: 1px solid var(--border);
    border-radius: 12px;
    image-rendering: pixelated;
  }
  .address {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12.5px;
    text-align: center;
    word-break: break-all;
  }
  .copy {
    min-width: 140px;
    justify-content: center;
  }
  .small {
    font-size: 11.5px;
    color: var(--muted);
  }
  .select {
    user-select: text;
    -webkit-user-select: text;
  }
  .link {
    color: var(--accent);
    font-size: 12px;
    padding: 0 2px;
  }
  .link:hover {
    text-decoration: underline;
  }
  .err {
    color: var(--danger);
  }
</style>

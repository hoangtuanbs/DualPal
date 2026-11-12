import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import ConnectionDialog from "./ConnectionDialog.svelte";
import type { Connection, SshConfigHost } from "./api";

/** Record backend calls; `save_connection` echoes the connection back. */
function backend(hosts: SshConfigHost[] = [], fail?: { kind: string; message: string }) {
  const calls: { cmd: string; args: any }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "ssh_config_hosts") return hosts;
    if (cmd === "save_connection") {
      if (fail) throw fail;
      const a = args as any;
      return { ...a.connection, id: a.connection.id || "new-id" };
    }
    return null;
  });
  return { saves: () => calls.filter((c) => c.cmd === "save_connection").map((c) => c.args) };
}

function setup(connection: Connection | null = null) {
  const props = { connection, onsave: vi.fn(), oncancel: vi.fn() };
  render(ConnectionDialog, { props });
  return props;
}

const kind = (name: string) => screen.getByRole("radio", { name });

describe("UI-CMP-004 ConnectionDialog", () => {
  it("SFTP: fields, auth methods, secret as a password field, and the saved shape", async () => {
    const user = userEvent.setup();
    const be = backend();
    const p = setup();
    expect(kind("SFTP / SSH")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByLabelText("Port")).toHaveValue(22);
    // "user@host" typed as the server is split into its fields.
    await user.type(screen.getByPlaceholderText("example.com"), "deploy@build.example");
    await user.type(screen.getByLabelText("Password"), "s3cret");
    expect(screen.getByLabelText("Password")).toHaveAttribute("type", "password");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.onsave).toHaveBeenCalled());
    expect(be.saves()[0]).toEqual({
      connection: {
        id: "",
        name: "build.example",
        initialPath: null,
        favorites: [],
        endpoint: { type: "sftp", host: "build.example", port: 22, user: "deploy", auth: { method: "password" } },
      },
      secret: "s3cret",
    });
    expect(p.onsave.mock.calls[0][1]).toBe(false); // Save, not Connect

    // A key file asks for a passphrase; the agent needs no secret at all.
    await user.selectOptions(screen.getByLabelText("Sign in with"), "keyFile");
    expect(screen.getByLabelText("Key file")).toHaveValue("~/.ssh/id_ed25519");
    expect(screen.getByLabelText("Key passphrase")).toHaveAttribute("type", "password");
    await user.selectOptions(screen.getByLabelText("Sign in with"), "agent");
    expect(screen.queryByLabelText("Key passphrase")).toBeNull();
    expect(screen.queryByLabelText("Password")).toBeNull();
  });

  it("FTP: the encryption choice moves the default port, plain FTP warns", async () => {
    const user = userEvent.setup();
    backend();
    setup();
    await user.click(kind("FTP / FTPS"));
    expect(screen.getByLabelText("Port")).toHaveValue(21);
    await user.selectOptions(screen.getByLabelText("Encryption"), "implicit");
    expect(screen.getByLabelText("Port")).toHaveValue(990);
    await user.selectOptions(screen.getByLabelText("Encryption"), "none");
    expect(screen.getByLabelText("Port")).toHaveValue(21);
    expect(screen.getByText(/not encrypted/i)).toBeInTheDocument();
  });

  it("WebDAV and S3 build their own endpoints", async () => {
    const user = userEvent.setup();
    const be = backend();
    const p = setup();
    await user.click(kind("WebDAV"));
    await user.type(screen.getByLabelText("Address"), "https://dav.example/files/");
    await user.type(screen.getByLabelText("User"), "me");
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(p.onsave).toHaveBeenCalledTimes(1));
    expect(be.saves()[0].connection.endpoint).toEqual({ type: "webDav", url: "https://dav.example/files/", user: "me" });
    expect(be.saves()[0].connection.name).toBe("dav.example");
    expect(p.onsave.mock.calls[0][1]).toBe(true); // Connect

    await user.click(kind("S3"));
    await user.type(screen.getByLabelText("Bucket"), "photos");
    await user.clear(screen.getByLabelText("Region"));
    await user.type(screen.getByLabelText("Region"), "eu-north-1");
    await user.type(screen.getByLabelText("Endpoint"), "http://127.0.0.1:9000");
    await user.type(screen.getByLabelText("Access key ID"), "AKIA1");
    await user.type(screen.getByLabelText("Secret access key"), "sk");
    await user.click(screen.getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(p.onsave).toHaveBeenCalledTimes(2));
    expect(be.saves()[1].connection.endpoint).toEqual({
      type: "s3",
      bucket: "photos",
      region: "eu-north-1",
      endpoint: "http://127.0.0.1:9000",
      accessKeyId: "AKIA1",
      pathStyle: true,
    });
    expect(be.saves()[1].secret).toBe("sk");
  });

  it("shows a save error in the dialog and stays open", async () => {
    const user = userEvent.setup();
    backend([], { kind: "invalid", message: "the server address is missing" });
    const p = setup();
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("the server address is missing")).toBeInTheDocument();
    expect(p.onsave).not.toHaveBeenCalled();
    expect(screen.getByTestId("connection-dialog")).toBeInTheDocument();
  });

  it("editing keeps the saved secret when the field is left empty", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup({
      id: "c1",
      name: "Build box",
      initialPath: "/srv",
      favorites: [{ name: "www", path: "/var/www" }],
      endpoint: { type: "sftp", host: "build.example", port: 2222, user: "deploy", auth: { method: "password" } },
    });
    expect(screen.getByRole("heading", { name: "Edit server" })).toBeInTheDocument();
    expect(screen.getByLabelText("Port")).toHaveValue(2222);
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(be.saves()).toHaveLength(1));
    expect(be.saves()[0].secret).toBeNull();
    expect(be.saves()[0].connection.id).toBe("c1");
    expect(be.saves()[0].connection.favorites).toEqual([{ name: "www", path: "/var/www" }]);
  });

  it("Cancel and Escape close without saving", async () => {
    const user = userEvent.setup();
    const be = backend();
    const p = setup();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await user.type(screen.getByLabelText("Name"), "{Escape}");
    expect(p.oncancel).toHaveBeenCalledTimes(2);
    expect(be.saves()).toEqual([]);
  });
});

describe("UI-CMP-005 ConnectionDialog ssh config", () => {
  it("picking a host from ~/.ssh/config fills the fields", async () => {
    const user = userEvent.setup();
    backend([
      { alias: "build", hostName: "10.0.0.5", user: "deploy", port: 2222, identityFile: "~/.ssh/build_key" },
      { alias: "nas", hostName: "nas.local", user: null, port: null, identityFile: null },
    ]);
    setup();
    const pick = await screen.findByLabelText("From ~/.ssh/config");
    await user.selectOptions(pick, "build");
    expect(screen.getByLabelText("Name")).toHaveValue("build");
    expect(screen.getByPlaceholderText("example.com")).toHaveValue("10.0.0.5");
    expect(screen.getByLabelText("User")).toHaveValue("deploy");
    expect(screen.getByLabelText("Port")).toHaveValue(2222);
    expect(screen.getByLabelText("Sign in with")).toHaveValue("keyFile");
    expect(screen.getByLabelText("Key file")).toHaveValue("~/.ssh/build_key");

    // A host without a key file signs in with the agent.
    await user.selectOptions(pick, "nas");
    expect(screen.getByPlaceholderText("example.com")).toHaveValue("nas.local");
    expect(screen.getByLabelText("Port")).toHaveValue(22);
    expect(screen.getByLabelText("Sign in with")).toHaveValue("agent");
  });

  it("is not offered when editing an existing server", async () => {
    backend([{ alias: "build", hostName: "10.0.0.5", user: null, port: null, identityFile: null }]);
    setup({
      id: "c1",
      name: "x",
      initialPath: null,
      favorites: [],
      endpoint: { type: "sftp", host: "x", port: 22, user: "u", auth: { method: "agent" } },
    });
    await new Promise((r) => setTimeout(r, 20)); // let the host list arrive
    expect(screen.queryByLabelText("From ~/.ssh/config")).toBeNull();
  });
});

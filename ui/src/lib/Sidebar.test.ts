import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import Sidebar from "./Sidebar.svelte";
import type { ConnectionView, Favorite, SyncFavorite, Volume } from "./api";

const volume: Volume = {
  name: "System",
  location: { fs: "local", path: "/" },
  totalBytes: 1e9,
  availableBytes: 5e8,
  removable: false,
  wsl: false,
};
const favorite: Favorite = { name: "Projects", location: { fs: "local", path: "/home/u/Projects" }, icon: "folder" };

function server(id: string, connected: boolean, favorites: { name: string; path: string }[] = []): ConnectionView {
  return {
    id,
    name: `Server ${id}`,
    endpoint: { type: "sftp", host: `${id}.example`, port: 22, user: "me", auth: { method: "password" } },
    initialPath: null,
    favorites,
    connected,
    fsId: `conn:${id}`,
  };
}

const sync = {
  id: "s1",
  name: "Photos backup",
  left: { fs: "local", path: "/p" },
  right: { fs: "conn:a", path: "/backup" },
  mode: "mirror",
  options: {},
} as unknown as SyncFavorite;

function setup(connections: ConnectionView[], connecting: Record<string, boolean> = {}) {
  const props = {
    volumes: [volume],
    favorites: [favorite],
    current: null,
    onopen: vi.fn(),
    onremovefavorite: vi.fn(),
    onreorder: vi.fn(),
    connections,
    connecting,
    onconnect: vi.fn(),
    onaddserver: vi.fn(),
    oneditserver: vi.fn(),
    ondisconnect: vi.fn(),
    onremoveserver: vi.fn(),
    onremoveremotefavorite: vi.fn(),
    syncs: [sync],
    onopensync: vi.fn(),
    onremovesync: vi.fn(),
  };
  render(Sidebar, { props });
  return props;
}

describe("UI-CMP-010 Sidebar", () => {
  it("shows its sections with their items", () => {
    setup([server("a", false)]);
    for (const h of ["Devices", "Favorites", "Syncs"]) {
      expect(screen.getByRole("heading", { name: h })).toBeInTheDocument();
    }
    expect(screen.getByText("System")).toBeInTheDocument();
    expect(screen.getByText("Projects")).toBeInTheDocument();
    expect(screen.getByText("Server a")).toBeInTheDocument();
    expect(screen.getByText("Photos backup")).toBeInTheDocument();
  });

  it("marks connected servers, with Disconnect for them and Remove for the others", async () => {
    const user = userEvent.setup();
    const p = setup([server("a", true), server("b", false)]);
    expect(screen.getAllByLabelText("Connected")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Remove Server a" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Disconnect Server a" }));
    expect(p.ondisconnect).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }));
    expect(screen.queryByRole("button", { name: "Disconnect Server b" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Remove Server b" }));
    expect(p.onremoveserver).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }));
    // Neither button also connects to the server.
    expect(p.onconnect).not.toHaveBeenCalled();
  });

  it("collapses and expands a server's favorites, which open at their path", async () => {
    const user = userEvent.setup();
    const p = setup([server("a", true, [{ name: "www", path: "/var/www" }])]);
    await user.click(screen.getByText("www"));
    expect(p.onconnect).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), "/var/www");
    await user.click(screen.getByRole("button", { name: "Hide favorites" }));
    expect(screen.queryByText("www")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show favorites" }));
    expect(screen.getByText("www")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove www" }));
    expect(p.onremoveremotefavorite).toHaveBeenCalledWith(expect.objectContaining({ id: "a" }), "/var/www");
  });

  it("removes a local favorite or a sync through the callbacks, without opening it", async () => {
    const user = userEvent.setup();
    const p = setup([]);
    await user.click(screen.getByRole("button", { name: "Remove Projects from Favorites" }));
    expect(p.onremovefavorite).toHaveBeenCalledWith(favorite.location);
    expect(p.onopen).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Remove Photos backup" }));
    expect(p.onremovesync).toHaveBeenCalledWith(sync);
    expect(p.onopensync).not.toHaveBeenCalled();
    await user.click(screen.getByText("Photos backup"));
    expect(p.onopensync).toHaveBeenCalledWith(sync);
  });

  it("shows a server that is connecting as busy, instead of the connected dot", async () => {
    const user = userEvent.setup();
    const p = setup([server("a", true), server("b", false)], { a: true });
    expect(screen.getByRole("status", { name: "Connecting" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Connected")).toBeNull();
    const row = screen.getByText("Server a").closest(".item")!;
    expect(row).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("Server b").closest(".item")).not.toHaveAttribute("aria-busy");
    await user.dblClick(screen.getByText("Server b"));
    expect(p.onconnect).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }));
  });

  it("connects a server on double-click or Enter; a single click only selects it", async () => {
    const user = userEvent.setup();
    const p = setup([server("a", false)]);
    const item = screen.getByText("Server a").closest(".item")!;
    await user.click(item);
    expect(item).toHaveClass("selected");
    expect(p.onconnect).not.toHaveBeenCalled();
    await user.dblClick(item);
    expect(p.onconnect).toHaveBeenCalledTimes(1);
    await user.keyboard("{Enter}");
    expect(p.onconnect).toHaveBeenCalledTimes(2);
  });

  it("does not connect when Edit or Remove is double-clicked", async () => {
    const user = userEvent.setup();
    const p = setup([server("a", false)]);
    await user.dblClick(screen.getByRole("button", { name: "Edit Server a" }));
    await user.dblClick(screen.getByRole("button", { name: "Remove Server a" }));
    expect(p.onconnect).not.toHaveBeenCalled();
  });
});

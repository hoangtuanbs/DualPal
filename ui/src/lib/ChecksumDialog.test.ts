import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import ChecksumDialog from "./ChecksumDialog.svelte";

const MD5_ABC = "900150983cd24fb0d6963f7d28e17f72";

/** A backend that answers checksum_calculate at once, or never when `hold` is set. */
function backend({ hold = false }: { hold?: boolean } = {}) {
  const calls: { cmd: string; args: any }[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "checksum_calculate") {
        if (hold) return new Promise(() => {});
        const algorithm = (args as any).algorithm;
        return [{ path: "a.txt", hash: algorithm === "md5" ? MD5_ABC : "f".repeat(64), error: null }, { path: "bad", hash: null, error: "denied" }];
      }
      if (cmd === "checksum_save") return { fs: "local", path: "/home/u/u.sha256" };
      return null;
    },
    { shouldMockEvents: true },
  );
  return {
    calls: (c: string) => calls.filter((x) => x.cmd === c).map((x) => x.args),
  };
}

const setup = () => {
  const props = { folder: { fs: "local", path: "/home/u" }, names: ["a.txt", "bad"], onclose: vi.fn() };
  render(ChecksumDialog, { props });
  return props;
};

describe("ChecksumDialog", () => {
  it("lists each file with its hash, and saves a checksum file with the chosen algorithm", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await waitFor(() => expect(screen.getByText("a.txt")).toBeInTheDocument());
    expect(be.calls("checksum_calculate")[0]).toEqual({
      folder: { fs: "local", path: "/home/u" },
      names: ["a.txt", "bad"],
      algorithm: "sha256",
      request: expect.any(Number),
    });
    expect(screen.getByText("denied")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("1 file hashed, 1 could not be read");

    // The default name comes from the folder; the extension follows the algorithm.
    const name = screen.getByRole("textbox", { name: "Checksum file name" });
    expect(name).toHaveValue("u.sha256");
    await user.click(screen.getByRole("button", { name: "Save as checksum file" }));
    const saved = be.calls("checksum_save")[0];
    expect(saved).toMatchObject({ folder: { fs: "local", path: "/home/u" }, fileName: "u.sha256", algorithm: "sha256", overwrite: false });
    expect(saved.entries).toEqual([{ path: "a.txt", hash: "f".repeat(64), error: null }, { path: "bad", hash: null, error: "denied" }]);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Saved u.sha256"));
  });

  it("runs again with the new algorithm and renames the save file to match", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await waitFor(() => expect(screen.getByText("a.txt")).toBeInTheDocument());
    await user.selectOptions(screen.getByRole("combobox", { name: "Algorithm" }), "md5");
    await waitFor(() => expect(be.calls("checksum_calculate")).toHaveLength(2));
    expect(be.calls("checksum_calculate")[1]).toMatchObject({ algorithm: "md5" });
    expect(screen.getByRole("textbox", { name: "Checksum file name" })).toHaveValue("u.md5");
    await waitFor(() => expect(screen.getByText(MD5_ABC)).toBeInTheDocument());
  });

  it("shows progress from events and Cancel stops the run", async () => {
    const user = userEvent.setup();
    const be = backend({ hold: true });
    setup();
    await waitFor(() => expect(be.calls("checksum_calculate")).toHaveLength(1));
    const { request } = be.calls("checksum_calculate")[0];
    await emit("checksum-progress", { request, filesDone: 1, filesTotal: 3, bytesDone: 1024, bytesTotal: 4096, current: "big.iso" });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 of 3 files"));
    expect(screen.getByText("big.iso")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(be.calls("cancel_checksum")).toEqual([{ request }]);
    expect(screen.getByRole("status")).toHaveTextContent("Stopped.");
    expect(screen.getByRole("button", { name: "Save as checksum file" })).toBeDisabled();
  });
});

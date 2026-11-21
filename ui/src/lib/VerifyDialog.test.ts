import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import VerifyDialog from "./VerifyDialog.svelte";

const report = {
  entries: [
    { path: "a.txt", status: "ok", expected: "aa", actual: "aa", error: null },
    { path: "gone.txt", status: "missing", expected: "bb", actual: null, error: null },
    { path: "b.txt", status: "failed", expected: "cc", actual: "dd", error: null },
  ],
  ok: 1,
  failed: 1,
  missing: 1,
};

function backend(answer: unknown = report) {
  const calls: { cmd: string; args: any }[] = [];
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      return cmd === "checksum_verify" ? answer : null;
    },
    { shouldMockEvents: true },
  );
  return { calls: (c: string) => calls.filter((x) => x.cmd === c).map((x) => x.args) };
}

describe("VerifyDialog", () => {
  it("shows the counts, with failures first", async () => {
    const be = backend();
    render(VerifyDialog, { props: { file: { fs: "local", path: "/home/u/SUMS.md5" }, onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 OK · 1 failed · 1 missing"));
    expect(be.calls("checksum_verify")[0]).toMatchObject({ file: { fs: "local", path: "/home/u/SUMS.md5" }, algorithm: "md5" });

    const rows = within(screen.getByRole("list", { name: "Results" })).getAllByRole("listitem");
    expect(rows.map((r) => r.textContent)).toEqual([
      expect.stringMatching(/^FAILED\s*b\.txt/),
      expect.stringMatching(/^MISSING\s*gone\.txt/),
      expect.stringMatching(/^OK\s*a\.txt/),
    ]);
    expect(rows[0]).toHaveTextContent("is dd");
  });

  it("refuses a file that is not a checksum file", async () => {
    const be = backend();
    render(VerifyDialog, { props: { file: { fs: "local", path: "/home/u/notes.txt" }, onclose: vi.fn() } });
    expect(screen.getByRole("status")).toHaveTextContent("Not a checksum file");
    expect(be.calls("checksum_verify")).toEqual([]);
  });

  it("lists every line, even when a file is listed twice", async () => {
    const twice = {
      entries: [
        { path: "a.txt", status: "ok", expected: "aa", actual: "aa", error: null },
        { path: "a.txt", status: "failed", expected: "bb", actual: "aa", error: null },
      ],
      ok: 1,
      failed: 1,
      missing: 0,
    };
    backend(twice);
    render(VerifyDialog, { props: { file: { fs: "local", path: "/home/u/SUMS.md5" }, onclose: vi.fn() } });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 OK · 1 failed · 0 missing"));
    const rows = within(screen.getByRole("list", { name: "Results" })).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
  });

  it("takes the focus when it opens, so Escape reaches it", async () => {
    backend();
    const onclose = vi.fn();
    render(VerifyDialog, { props: { file: { fs: "local", path: "/home/u/SUMS.md5" }, onclose } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toHaveFocus());
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 OK"));
    await userEvent.setup().keyboard("{Escape}");
    expect(onclose).toHaveBeenCalled();
  });
});

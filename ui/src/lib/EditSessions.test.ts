import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import EditSessions from "./EditSessions.svelte";
import type { EditSession } from "./api";

function session(status: EditSession["status"], error: string | null = null): EditSession {
  return {
    id: "e1",
    remote: { fs: "conn:a", path: "/srv/notes.md" },
    localPath: "/tmp/e1/notes.md",
    status,
    uploads: 0,
    lastUploadMs: null,
    error,
  };
}

function setup(s: EditSession) {
  const props = { sessions: [s], onstop: vi.fn(), onupload: vi.fn(), onresolve: vi.fn() };
  render(EditSessions, { props });
  return props;
}

describe("EditSessions", () => {
  it("EDIT-007 a conflict offers to overwrite the server copy or skip the save", async () => {
    const props = setup(session("conflict", "the file changed on the server since you opened it"));
    expect(screen.getByTestId("edit-sessions").textContent).toContain("Changed on the server");
    expect(screen.queryByRole("button", { name: "Upload" })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Overwrite server copy" }));
    expect(props.onresolve).toHaveBeenLastCalledWith("e1", true);
    await userEvent.click(screen.getByRole("button", { name: "Skip this save" }));
    expect(props.onresolve).toHaveBeenLastCalledWith("e1", false);
    expect(props.onupload).not.toHaveBeenCalled();
  });

  it("EDIT-007 a failed upload offers Upload, not the conflict choices", async () => {
    const props = setup(session("failed", "connection lost"));
    expect(screen.queryByRole("button", { name: "Overwrite server copy" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Upload" }));
    expect(props.onupload).toHaveBeenCalledWith("e1");
  });
});

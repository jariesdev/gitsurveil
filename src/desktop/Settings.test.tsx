/**
 * Smoke tests for the Settings pane: it must render the daemon's registered
 * apps and drive add/remove through the IPC client.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { appsAdd, appsList, appsRemove } from "../ipc";
import { Settings } from "./Settings";

vi.mock("../ipc", () => ({
  appsAdd: vi.fn(),
  appsList: vi.fn(),
  appsRemove: vi.fn(),
}));

// Each test sets its own app registry; reset so a `mockResolvedValueOnce`
// left unconsumed by an earlier assertion never leaks into the next test.
beforeEach(() => {
  vi.resetAllMocks();
});

describe("Settings", () => {
  it("loads the registered applications on mount", async () => {
    vi.mocked(appsList).mockResolvedValue([
      { name: "VS Code", command: "code" },
      { name: "Sublime Merge", command: "smerge" },
    ]);
    render(<Settings />);
    expect(await screen.findByText("VS Code")).toBeTruthy();
    expect(screen.getByText("code")).toBeTruthy();
    expect(screen.getByText("Sublime Merge")).toBeTruthy();
    expect(screen.getByText("smerge")).toBeTruthy();
  });

  it("adds an application and reloads the list", async () => {
    vi.mocked(appsList)
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([{ name: "VS Code", command: "code" }]);
    vi.mocked(appsAdd).mockResolvedValue({ name: "VS Code", command: "code" });
    render(<Settings />);
    expect(screen.getByText(/No applications yet/)).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "VS Code" },
    });
    fireEvent.change(screen.getByLabelText("Command"), {
      target: { value: "code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add application" }));

    await waitFor(() => expect(appsAdd).toHaveBeenCalledWith("VS Code", "code"));
    // The reload brings the new row in; the empty state is gone.
    expect(await screen.findByText("VS Code")).toBeTruthy();
    expect(screen.queryByText(/No applications yet/)).toBeNull();
  });

  it("removes an application", async () => {
    vi.mocked(appsList).mockResolvedValue([
      { name: "Sublime Merge", command: "smerge" },
    ]);
    vi.mocked(appsRemove).mockResolvedValue(undefined);
    render(<Settings />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() => expect(appsRemove).toHaveBeenCalledWith("smerge"));
  });

  it("surfaces the daemon's error when adding a duplicate", async () => {
    vi.mocked(appsList).mockResolvedValue([]);
    vi.mocked(appsAdd).mockRejectedValue(new Error("code is already registered"));
    render(<Settings />);

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "VS Code" },
    });
    fireEvent.change(screen.getByLabelText("Command"), {
      target: { value: "code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add application" }));

    expect(
      await screen.findByText(/code is already registered/),
    ).toBeTruthy();
  });
});

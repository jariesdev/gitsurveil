/**
 * Smoke tests for the Settings pane: it must render the daemon's registered
 * apps and drive add/remove through the IPC client.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { appsAdd, appsList, appsRemove, notificationsPrefs, notificationsSetPref } from "../ipc";
import { Settings } from "./Settings";

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }));

vi.mock("../ipc", () => ({
  appsAdd: vi.fn(),
  appsList: vi.fn(),
  appsRemove: vi.fn(),
  notificationsPrefs: vi.fn(),
  notificationsSetPref: vi.fn(),
}));

// Each test sets its own app registry; reset so a `mockResolvedValueOnce`
// left unconsumed by an earlier assertion never leaks into the next test.
// notificationsPrefs defaults to an empty list so tests that don't care about
// the notifications section don't have to stub it themselves.
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(notificationsPrefs).mockResolvedValue([]);
});

describe("Settings", () => {
  it("loads the registered applications on mount", async () => {
    vi.mocked(appsList).mockResolvedValue([
      { name: "VS Code", command: "code" },
      { name: "Sublime Merge", command: "smerge" },
    ]);
    render(<Settings />);
    expect(await screen.findByText("VS Code")).toBeTruthy();
    // The command is matched on the row's `.font-mono` cell, not the inline
    // `<code>code</code>` example in the hint paragraph.
    expect(screen.getByText("code", { selector: ".font-mono" })).toBeTruthy();
    expect(screen.getByText("Sublime Merge")).toBeTruthy();
    expect(screen.getByText("smerge", { selector: ".font-mono" })).toBeTruthy();
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
    fireEvent.change(screen.getByLabelText("Application or Command"), {
      target: { value: "code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add application" }));

    await waitFor(() => expect(appsAdd).toHaveBeenCalledWith("VS Code", "code"));
    // The reload brings the new row in; the empty state is gone.
    expect(await screen.findByText("VS Code")).toBeTruthy();
    expect(screen.queryByText(/No applications yet/)).toBeNull();
  });

  it("fills the command field from the executable file picker", async () => {
    vi.mocked(appsList).mockResolvedValue([]);
    dialog.open.mockResolvedValueOnce("/usr/local/bin/code");
    render(<Settings />);

    fireEvent.click(screen.getByRole("button", { name: "Browse…" }));
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Application or Command") as HTMLInputElement).value,
      ).toBe("/usr/local/bin/code"),
    );
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

  it("loads and toggles notification preferences", async () => {
    vi.mocked(appsList).mockResolvedValue([]);
    vi.mocked(notificationsPrefs).mockResolvedValue([
      { kind: "ci_failed", enabled: true },
      { kind: "authored", enabled: false },
    ]);
    vi.mocked(notificationsSetPref).mockResolvedValue(undefined);
    render(<Settings />);

    const ciFailed = (await screen.findByLabelText("CI failed")) as HTMLInputElement;
    const authored = screen.getByLabelText("Your PR") as HTMLInputElement;
    expect(ciFailed.checked).toBe(true);
    expect(authored.checked).toBe(false);

    fireEvent.click(authored);
    expect(notificationsSetPref).toHaveBeenCalledWith("authored", true);
    await waitFor(() => expect(authored.checked).toBe(true));
  });

  it("rolls a preference back if the daemon rejects it", async () => {
    vi.mocked(appsList).mockResolvedValue([]);
    vi.mocked(notificationsPrefs).mockResolvedValue([{ kind: "ci_failed", enabled: true }]);
    vi.mocked(notificationsSetPref).mockRejectedValue(new Error("daemon unreachable"));
    render(<Settings />);

    const ciFailed = (await screen.findByLabelText("CI failed")) as HTMLInputElement;
    fireEvent.click(ciFailed);
    expect(await screen.findByText(/daemon unreachable/)).toBeTruthy();
    // Reverted to its pre-click state — the daemon never persisted the toggle.
    expect(ciFailed.checked).toBe(true);
  });

  it("surfaces the daemon's error when adding a duplicate", async () => {
    vi.mocked(appsList).mockResolvedValue([]);
    vi.mocked(appsAdd).mockRejectedValue(new Error("code is already registered"));
    render(<Settings />);

    fireEvent.change(screen.getByLabelText("Name"), {
      target: { value: "VS Code" },
    });
    fireEvent.change(screen.getByLabelText("Application or Command"), {
      target: { value: "code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add application" }));

    expect(
      await screen.findByText(/code is already registered/),
    ).toBeTruthy();
  });
});

/**
 * Tests for the popover's three states. The IPC layer is mocked, so these
 * exercise exactly what the component does with whatever the daemon returns —
 * including the "daemon isn't running" path, which is the state users are most
 * likely to hit first and the easiest one to get wrong.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { Popover } from "./Popover";
import { listen } from "@tauri-apps/api/event";
import type { ScoredItem, StatusResult } from "./types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("./ipc", () => ({
  listItems: vi.fn(),
  daemonStatus: vi.fn(),
  openUrl: vi.fn(),
  dismissItem: vi.fn(),
  closePopover: vi.fn(),
  openMainWindow: vi.fn(),
  browsersList: vi.fn().mockResolvedValue([]),
  openUrlWithBrowser: vi.fn(),
}));

const { listItems, daemonStatus, dismissItem } = await import("./ipc");

const status: StatusResult = {
  version: "0.1.0",
  uptime_secs: 42,
  account_count: 1,
  open_item_count: 1,
  top_severity: "high",
};

function item(overrides: Partial<ScoredItem> = {}): ScoredItem {
  return {
    id: "item-1",
    account_id: "acc-1",
    kind: "review_requested",
    state: "open",
    repo: "acme/api",
    number: 482,
    title: "Fix the thing",
    url: "https://github.com/acme/api/pull/482",
    author: "someone",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    first_seen_at: new Date().toISOString(),
    last_seen_at: new Date().toISOString(),
    ci_status: "none",
    raw_kind: "review_requested",
    score: 80,
    severity: "high",
    muted: false,
    ...overrides,
  };
}

describe("Popover", () => {
  beforeEach(() => {
    vi.mocked(listItems).mockReset();
    vi.mocked(daemonStatus).mockReset();
    vi.mocked(dismissItem).mockReset();
  });

  it("renders items returned by the daemon", async () => {
    vi.mocked(listItems).mockResolvedValue([item()]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    expect(await screen.findByText("Fix the thing")).toBeInTheDocument();
    expect(screen.getByText("Review requested")).toBeInTheDocument();
    expect(screen.getByText("acme/api#482")).toBeInTheDocument();
    expect(screen.getByText("1 item")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Open GitSurveil" }),
    ).toBeInTheDocument();
  });

  it("shows an all-clear state when there is nothing to do", async () => {
    vi.mocked(listItems).mockResolvedValue([]);
    vi.mocked(daemonStatus).mockResolvedValue({ ...status, open_item_count: 0 });

    render(<Popover />);

    expect(await screen.findByText("All clear")).toBeInTheDocument();
    expect(
      screen.getByText("Nothing needs your attention."),
    ).toBeInTheDocument();
  });

  it("tells the user how to start the service when it is unreachable", async () => {
    // The Rust command surfaces connection failures as a rejected promise;
    // the popover must render a recovery hint, not a blank list.
    vi.mocked(listItems).mockRejectedValue(new Error("cannot reach service"));
    vi.mocked(daemonStatus).mockRejectedValue(new Error("cannot reach service"));

    render(<Popover />);

    await waitFor(() => {
      expect(
        screen.getByText("The GitSurveil service isn’t running"),
      ).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("labels each row with its priority band", async () => {
    vi.mocked(listItems).mockResolvedValue([item({ severity: "critical" })]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    expect(await screen.findByLabelText("critical priority")).toBeInTheDocument();
  });

  it("marks muted items as muted while still listing them", async () => {
    // Muting silences notifications; the item must stay visible and stay
    // ranked, or the user loses track of it entirely.
    vi.mocked(listItems).mockResolvedValue([
      item({ severity: "info", muted: true, title: "Muted thread" }),
    ]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    expect(await screen.findByText("Muted thread")).toBeInTheDocument();
    expect(screen.getByLabelText("info priority, muted")).toBeInTheDocument();
  });

  it("renders items in the order the daemon returned them", async () => {
    // The daemon sorts by score; the popover must not re-sort or reverse it.
    vi.mocked(listItems).mockResolvedValue([
      item({ id: "a", title: "First", severity: "critical", score: 100 }),
      item({ id: "b", title: "Second", severity: "normal", score: 40 }),
    ]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    await screen.findByText("First");
    // Scoped to the list, so the header's "Open GitSurveil" button doesn't
    // count as the first row.
    const rows = screen.getAllByRole("listitem").map((li) => li.textContent);
    expect(rows[0]).toContain("First");
    expect(rows[1]).toContain("Second");
  });

  it("marks failing CI with an accessible indicator", async () => {
    vi.mocked(listItems).mockResolvedValue([item({ ci_status: "failing" })]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    expect(await screen.findByLabelText("CI failing")).toBeInTheDocument();
  });

  it("copies the item URL from a row's context menu", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    vi.mocked(listItems).mockResolvedValue([item()]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    const row = await screen.findByText("Fix the thing");
    fireEvent.contextMenu(row, { clientX: 40, clientY: 30 });

    const menuItem = await screen.findByRole("menuitem", { name: "Copy URL" });
    fireEvent.click(menuItem);

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("https://github.com/acme/api/pull/482");
    });
  });

  it("dismisses an item from its row and the list refreshes", async () => {
    // The daemon drops the item after dismissal, so the second fetch returns
    // nothing — the popover must reflect that instead of keeping the row. The
    // refresh is driven by the `items-changed` event (the popover no longer
    // reloads itself), so the test fires the listener the way the Rust shell
    // would after the dismiss command succeeds.
    vi.mocked(listItems)
      .mockResolvedValueOnce([item()])
      .mockResolvedValue([]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);

    const dismissButton = await screen.findByRole("button", {
      name: "Dismiss Fix the thing",
    });
    fireEvent.click(dismissButton);

    await waitFor(() => {
      expect(dismissItem).toHaveBeenCalledWith("item-1");
    });

    const handler = vi.mocked(listen).mock.calls.at(-1)![1];
    await act(async () => {
      handler({ event: "items-changed", id: 0, payload: undefined });
    });

    expect(await screen.findByText("All clear")).toBeInTheDocument();
  });

  it("refetches when an item's state changes in the desktop UI", async () => {
    // Restoring a dismissed item in History (or dismissing in the Dashboard)
    // emits `items-changed`; the popover must show the restored item without
    // waiting for its own action.
    vi.mocked(listItems).mockResolvedValue([]);
    vi.mocked(daemonStatus).mockResolvedValue(status);

    render(<Popover />);
    await waitFor(() => expect(vi.mocked(listItems)).toHaveBeenCalled());

    const before = vi.mocked(listItems).mock.calls.length;
    const handler = vi.mocked(listen).mock.calls.at(-1)![1];
    await act(async () => {
      handler({ event: "items-changed", id: 0, payload: undefined });
    });

    await waitFor(() => {
      expect(vi.mocked(listItems).mock.calls.length).toBeGreaterThan(before);
    });
  });
});

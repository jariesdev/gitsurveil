/**
 * Integration smoke test for the main window: opening it and navigating to the
 * Repositories view must not take the whole window down (white screen = an
 * uncaught React render error). Reproduces the exact reported bug.
 */

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { listen } from "@tauri-apps/api/event";
import type { RepoCatalog, ScoredItem } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

const mockIpc = vi.hoisted(() => {
  const catalog: RepoCatalog = {
    orgs: [
      { account_id: "acc-1", host: "github.com", name: "ariesragingriverict" },
      { account_id: "acc-1", host: "github.com", name: "wthvillas" },
    ],
    repos: [
      {
        account_id: "acc-1",
        host: "github.com",
        owner: "ariesragingriverict",
        name: "paypal",
        full_name: "ariesragingriverict/paypal",
        url: "https://github.com/ariesragingriverict/paypal",
        description: "PayPal gateway integration.",
        private: false,
        default_branch: "main",
        clone_url: "https://github.com/ariesragingriverict/paypal.git",
        clone_path: null,
        tracked: false,
        notify_enabled: true,
        first_seen_at: "2026-08-14T15:22:26.237158+00:00",
        notified_at: "2026-08-14T15:22:26.237158+00:00",
        last_refreshed_at: "2026-08-14T15:22:26.237158+00:00",
      },
      {
        account_id: "acc-1",
        host: "github.com",
        owner: "wthvillas",
        name: "villasplatform",
        full_name: "wthvillas/villasplatform",
        url: "https://github.com/wthvillas/villasplatform",
        description: null,
        private: true,
        default_branch: "main",
        clone_url: "https://github.com/wthvillas/villasplatform.git",
        clone_path: null,
        tracked: false,
        notify_enabled: true,
        first_seen_at: "2026-08-14T15:22:26.237158+00:00",
        notified_at: "2026-08-14T15:22:26.237158+00:00",
        last_refreshed_at: "2026-08-14T15:22:26.237158+00:00",
      },
    ],
  };
  const m = {
    daemonStatus: vi.fn(),
    listAccounts: vi.fn(),
    listHistory: vi.fn(),
    listItems: vi.fn(),
    listRules: vi.fn(),
    openUrl: vi.fn(),
    reposAckNew: vi.fn(),
    reposList: vi.fn(),
    reposNew: vi.fn(),
    appsList: vi.fn(),
    notificationsPrefs: vi.fn(),
    undismissItem: vi.fn(),
    clearHistory: vi.fn(),
    listPullRequests: vi.fn(),
    prDetail: vi.fn(),
    pollNow: vi.fn(),
    dismissItem: vi.fn(),
  };
  m.daemonStatus.mockResolvedValue({
    version: "0.1.0",
    next_poll_at: "2026-08-14T16:00:00Z",
    online: true,
    accounts: 1,
  });
  m.listAccounts.mockResolvedValue([
    {
      id: "acc-1",
      host: "github.com",
      api_base: "https://api.github.com",
      login: "ariesragingriverict",
      auth_kind: "pat",
    },
  ]);
  m.listHistory.mockResolvedValue([]);
  m.listItems.mockResolvedValue([]);
  m.listRules.mockResolvedValue([]);
  m.reposList.mockResolvedValue(catalog);
  m.reposNew.mockResolvedValue([]);
  m.appsList.mockResolvedValue([]);
  m.notificationsPrefs.mockResolvedValue([]);
  m.listPullRequests.mockResolvedValue([]);
  m.prDetail.mockRejectedValue(new Error("not opened"));
  m.pollNow.mockResolvedValue(undefined);
  m.undismissItem.mockResolvedValue(undefined);
  m.clearHistory.mockResolvedValue(undefined);
  m.dismissItem.mockResolvedValue(undefined);
  m.openUrl.mockResolvedValue(undefined);
  m.reposAckNew.mockResolvedValue(1);
  return m;
});

vi.mock("../ipc", () => mockIpc);

let confirmSpy: { mockRestore: () => void } | undefined;

beforeEach(() => {
  Object.values(mockIpc).forEach((fn) => fn.mockClear());
});

afterEach(() => {
  confirmSpy?.mockRestore();
  confirmSpy = undefined;
});

describe("App navigation", () => {
  it("renders the Repositories view when its nav item is clicked", async () => {
    const user = userEvent.setup();
    render(<App />);

    const reposNav = await screen.findByRole("button", { name: /Repositories/ });
    await user.click(reposNav);

    expect(await screen.findByText("ariesragingriverict/paypal")).toBeTruthy();
    expect(screen.getByText("wthvillas/villasplatform")).toBeTruthy();
    expect(
      screen.getByRole("heading", { name: "Repository and Worktrees" }),
    ).toBeTruthy();
  });

  it("renders the Settings view when its nav item is clicked", async () => {
    const user = userEvent.setup();
    render(<App />);

    const settingsNav = await screen.findByRole("button", { name: "Settings" });
    await user.click(settingsNav);

    expect(
      await screen.findByText("Open with… applications"),
    ).toBeTruthy();
  });

  it("clears all history after confirmation", async () => {
    const user = userEvent.setup();
    const resolved: ScoredItem = {
      id: "done-1",
      account_id: "acc-1",
      kind: "review_requested",
      state: "done",
      repo: "acme/api",
      number: 482,
      title: "Fix the thing",
      url: "https://github.com/acme/api/pull/482",
      author: "someone",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
      first_seen_at: "2026-08-01T00:00:00Z",
      last_seen_at: "2026-08-01T00:00:00Z",
      ci_status: "passing",
      raw_kind: "review_requested",
      score: 0,
      severity: "info",
      muted: false,
    };
    mockIpc.listHistory.mockResolvedValue([resolved]);
    confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<App />);

    const historyNav = await screen.findByRole("button", { name: "History" });
    await user.click(historyNav);

    const clearButton = await screen.findByRole("button", {
      name: "Clear all history",
    });
    await user.click(clearButton);

    expect(mockIpc.clearHistory).toHaveBeenCalledTimes(1);
    expect(window.confirm).toHaveBeenCalledWith(
      expect.stringContaining("can’t be undone"),
    );
  });

  it("does not clear history when the confirmation is declined", async () => {
    const user = userEvent.setup();
    const resolved: ScoredItem = {
      id: "done-1",
      account_id: "acc-1",
      kind: "review_requested",
      state: "done",
      repo: "acme/api",
      number: 482,
      title: "Fix the thing",
      url: "https://github.com/acme/api/pull/482",
      author: "someone",
      created_at: "2026-08-01T00:00:00Z",
      updated_at: "2026-08-01T00:00:00Z",
      first_seen_at: "2026-08-01T00:00:00Z",
      last_seen_at: "2026-08-01T00:00:00Z",
      ci_status: "passing",
      raw_kind: "review_requested",
      score: 0,
      severity: "info",
      muted: false,
    };
    mockIpc.listHistory.mockResolvedValue([resolved]);
    confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<App />);

    const historyNav = await screen.findByRole("button", { name: "History" });
    await user.click(historyNav);

    const clearButton = await screen.findByRole("button", {
      name: "Clear all history",
    });
    await user.click(clearButton);

    expect(mockIpc.clearHistory).not.toHaveBeenCalled();
  });

  it("refreshes when the shell reports items changed elsewhere", async () => {
    // The popover's dismiss runs the same daemon command; the Rust shell then
    // emits `items-changed`, and the app must refetch so an open Dashboard
    // drops the item without waiting for its own action.
    mockIpc.listItems.mockResolvedValue([]);
    render(<App />);
    await waitFor(() => expect(mockIpc.listItems).toHaveBeenCalled());

    const before = mockIpc.listItems.mock.calls.length;
    const handler = vi.mocked(listen).mock.calls.at(-1)![1];
    await act(async () => {
      handler({ event: "items-changed", id: 0, payload: undefined });
    });

    await waitFor(() =>
      expect(mockIpc.listItems.mock.calls.length).toBeGreaterThan(before),
    );
  });
});

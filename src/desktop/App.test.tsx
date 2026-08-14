/**
 * Integration smoke test for the main window: opening it and navigating to the
 * Repositories view must not take the whole window down (white screen = an
 * uncaught React render error). Reproduces the exact reported bug.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { RepoCatalog } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

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
    undismissItem: vi.fn(),
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
  m.listPullRequests.mockResolvedValue([]);
  m.prDetail.mockRejectedValue(new Error("not opened"));
  m.pollNow.mockResolvedValue(undefined);
  m.undismissItem.mockResolvedValue(undefined);
  m.dismissItem.mockResolvedValue(undefined);
  m.openUrl.mockResolvedValue(undefined);
  m.reposAckNew.mockResolvedValue(1);
  return m;
});

vi.mock("../ipc", () => mockIpc);

beforeEach(() => {
  Object.values(mockIpc).forEach((fn) => fn.mockClear());
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
});

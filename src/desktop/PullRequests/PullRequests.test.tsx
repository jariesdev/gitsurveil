/**
 * Tests for the Pull Requests view's row context menu: right-clicking a row
 * must offer **Open in GitHub** (the provider for that PR's account) and open
 * the PR's URL through the same `openUrl` path the rest of the app uses.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PullRequests } from "./PullRequests";
import type { AccountRef, PullRequestSummary } from "../../types";

vi.mock("../../ipc", () => ({
  listPullRequests: vi.fn(),
  listRepos: vi.fn(),
  openUrl: vi.fn(),
}));

const { listPullRequests, listRepos, openUrl } = await import("../../ipc");

const account: AccountRef = {
  id: "acc-1",
  login: "alice",
  host: "github.com",
  api_base: "https://api.github.com",
  auth_kind: "pat",
};

function pr(overrides: Partial<PullRequestSummary> = {}): PullRequestSummary {
  return {
    account_id: account.id,
    repo: "acme/api",
    number: 482,
    title: "Add rate limiting",
    url: "https://github.com/acme/api/pull/482",
    author: "carol",
    roles: ["review_requested"],
    state: "open",
    draft: false,
    ci_status: "passing",
    review_decision: "none",
    unresolved_threads: 0,
    mergeability: "clean",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  };
}

describe("PullRequests row context menu", () => {
  beforeEach(() => {
    vi.mocked(listPullRequests).mockReset();
    vi.mocked(listRepos).mockReset();
    vi.mocked(openUrl).mockReset();
    vi.mocked(listPullRequests).mockResolvedValue([pr()]);
    vi.mocked(listRepos).mockResolvedValue([]);
  });

  it("opens the PR in the provider from a row's context menu", async () => {
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const row = await screen.findByText("Add rate limiting");
    fireEvent.contextMenu(row, { clientX: 120, clientY: 80 });

    const item = await screen.findByRole("menuitem", { name: "Open in GitHub" });
    fireEvent.click(item);

    await waitFor(() => {
      expect(openUrl).toHaveBeenCalledWith("https://github.com/acme/api/pull/482");
    });
  });

  it("dismisses the menu without opening anything when clicking away", async () => {
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const row = await screen.findByText("Add rate limiting");
    fireEvent.contextMenu(row, { clientX: 120, clientY: 80 });
    await screen.findByRole("menuitem", { name: "Open in GitHub" });

    fireEvent.pointerDown(document.body);

    await waitFor(() => {
      expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    });
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("shows an unresolved-review badge with the thread count", async () => {
    vi.mocked(listPullRequests).mockResolvedValue([pr({ unresolved_threads: 3 })]);
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const badge = await screen.findByTitle("3 unresolved review threads");
    expect(badge).toBeInTheDocument();
    expect(badge.textContent).toContain("3");
  });

  it("hides the badge when there are no unresolved threads", async () => {
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    await screen.findByText("Add rate limiting");
    expect(
      screen.queryByTitle("unresolved review thread"),
    ).not.toBeInTheDocument();
  });
});

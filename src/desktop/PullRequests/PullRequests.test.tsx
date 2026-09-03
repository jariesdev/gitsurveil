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
  reposList: vi.fn(),
  openUrl: vi.fn(),
  prDetail: vi.fn(),
  prComments: vi.fn(),
  prBranches: vi.fn(),
  prLabels: vi.fn(),
}));

const { listPullRequests, reposList, openUrl, prDetail, prComments, prBranches, prLabels } =
  await import("../../ipc");

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
    mergeable: "clean",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  };
}

describe("PullRequests row context menu", () => {
  beforeEach(() => {
    vi.mocked(listPullRequests).mockReset();
    vi.mocked(reposList).mockReset();
    vi.mocked(openUrl).mockReset();
    vi.mocked(listPullRequests).mockResolvedValue([pr()]);
    vi.mocked(reposList).mockResolvedValue({ orgs: [], repos: [] });
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

  it("copies the PR URL from a row's context menu", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const row = await screen.findByText("Add rate limiting");
    fireEvent.contextMenu(row, { clientX: 120, clientY: 80 });

    const item = await screen.findByRole("menuitem", { name: "Copy URL" });
    fireEvent.click(item);

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("https://github.com/acme/api/pull/482");
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

describe("PullRequests row selection", () => {
  beforeEach(() => {
    vi.mocked(listPullRequests).mockReset();
    vi.mocked(reposList).mockReset();
    vi.mocked(prDetail).mockReset();
    vi.mocked(prComments).mockReset();
    vi.mocked(prBranches).mockReset();
    vi.mocked(prLabels).mockReset();
    vi.mocked(listPullRequests).mockResolvedValue([pr()]);
    vi.mocked(reposList).mockResolvedValue({ orgs: [], repos: [] });
    vi.mocked(prDetail).mockResolvedValue({
      repo: "acme/api",
      number: 482,
      title: "Add rate limiting",
      body: "Some description",
      state: "open",
      draft: false,
      base: "main",
      head: "feature/limits",
      author: "carol",
      labels: [],
      reviewers: [{ login: "dave", state: "pending", rounds: 0 }],
      checks: [],
      mergeability: "clean",
      url: "https://github.com/acme/api/pull/482",
      head_sha: "abc123",
    });
    vi.mocked(prComments).mockResolvedValue({ issue_comments: [], review_threads: [] });
    vi.mocked(prBranches).mockResolvedValue([]);
    vi.mocked(prLabels).mockResolvedValue([]);
  });

  it("highlights the row whose detail pane is open", async () => {
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const row = await screen.findByText("Add rate limiting");
    expect(row.closest("[aria-current='true']")).toBeNull();

    fireEvent.click(row);
    await screen.findByRole("complementary", { name: "Pull request detail" });

    expect(row.closest("[aria-current='true']")).not.toBeNull();
  });

  it("drops the highlight when the detail pane closes", async () => {
    render(<PullRequests accounts={[account]} onOpenRepos={() => {}} />);

    const row = await screen.findByText("Add rate limiting");
    fireEvent.click(row);
    await screen.findByRole("complementary", { name: "Pull request detail" });
    expect(row.closest("[aria-current='true']")).not.toBeNull();

    fireEvent.click(
      screen.getByRole("button", { name: "Close detail" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("complementary", { name: "Pull request detail" })).not.toBeInTheDocument();
    });
    expect(row.closest("[aria-current='true']")).toBeNull();
  });
});

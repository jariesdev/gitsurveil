/**
 * Tests for the PR detail pane. The focus is the mutating actions: this is
 * the only part of the app that writes to GitHub, so what it does — and
 * refuses to do — matters more than how it looks.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PrDetail } from "./PrDetail";
import type { PullRequestDetail } from "../types";

vi.mock("../ipc", () => ({
  prDetail: vi.fn(),
  prComments: vi.fn(),
  prComment: vi.fn(),
  prUpdate: vi.fn(),
  prClose: vi.fn(),
  prMerge: vi.fn(),
  openUrl: vi.fn(),
}));

const ipc = await import("../ipc");

function pr(overrides: Partial<PullRequestDetail> = {}): PullRequestDetail {
  return {
    repo: "acme/api",
    number: 482,
    title: "Add rate limiting",
    body: "Some description",
    state: "open",
    draft: false,
    base: "main",
    head: "feature/limits",
    author: "carol",
    labels: ["enhancement"],
    reviewers: [{ login: "dave", state: "pending" }],
    checks: [{ name: "build", conclusion: "success", url: null }],
    mergeability: "clean",
    url: "https://github.com/acme/api/pull/482",
    head_sha: "abc123",
    ...overrides,
  };
}

describe("PrDetail", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.prComments).mockResolvedValue([]);
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  it("renders the pull request and its metadata", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    expect(await screen.findByText("Add rate limiting")).toBeInTheDocument();
    expect(screen.getByText("acme/api#482")).toBeInTheDocument();
    expect(screen.getByText("feature/limits → main")).toBeInTheDocument();
    expect(screen.getByText("Ready to merge")).toBeInTheDocument();
  });

  it("passes head_sha when merging, so a moved PR is rejected by GitHub", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr({ head_sha: "deadbeef" }));
    vi.mocked(ipc.prMerge).mockResolvedValue(undefined);

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));

    await waitFor(() => {
      expect(ipc.prMerge).toHaveBeenCalledWith(
        "acme/api",
        482,
        "merge",
        "deadbeef",
      );
    });
  });

  it("does not merge when the confirmation is declined", async () => {
    // Merging can't be undone from here, so a mis-click must not be enough.
    vi.spyOn(window, "confirm").mockReturnValue(false);
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));

    expect(ipc.prMerge).not.toHaveBeenCalled();
  });

  it("disables merging when the branch has conflicts", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr({ mergeability: "conflicted" }));

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    expect(await screen.findByText("Conflicts with base branch")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Merge" })).toBeDisabled();
  });

  it("offers the conflict resolver entry only for conflicted PRs", async () => {
    const onResolve = vi.fn();
    vi.mocked(ipc.prDetail).mockResolvedValue(pr({ mergeability: "conflicted" }));

    const { unmount } = render(
      <PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={onResolve} />,
    );
    await screen.findByText("Add rate limiting");

    const entry = screen.getByRole("button", { name: "Resolve conflicts" });
    await userEvent.click(entry);
    expect(onResolve).toHaveBeenCalledOnce();

    unmount();
    vi.mocked(ipc.prDetail).mockResolvedValue(pr({ mergeability: "clean" }));
    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={onResolve} />);
    await screen.findByText("Add rate limiting");
    expect(
      screen.queryByRole("button", { name: "Resolve conflicts" }),
    ).not.toBeInTheDocument();
  });

  it("offers no write actions on a merged pull request", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr({ state: "merged" }));

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");

    expect(screen.queryByRole("button", { name: "Merge" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
  });

  it("sends only the edited fields on save", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());
    vi.mocked(ipc.prUpdate).mockResolvedValue(pr({ title: "New title" }));

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));

    const title = screen.getByLabelText("Title");
    await userEvent.clear(title);
    await userEvent.type(title, "New title");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(ipc.prUpdate).toHaveBeenCalledWith("acme/api", 482, {
        title: "New title",
        body: "Some description",
      });
    });
  });

  it("surfaces a rejected mutation instead of failing silently", async () => {
    // GitHub's message ("Validation Failed", a missing scope) is the useful
    // part; swallowing it would leave the user with a button that did nothing.
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());
    vi.mocked(ipc.prMerge).mockRejectedValue(
      new Error("GitHub 405: Pull Request is not mergeable"),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Pull Request is not mergeable",
    );
  });
});

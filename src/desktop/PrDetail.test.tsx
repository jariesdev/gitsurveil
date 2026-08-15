/**
 * Tests for the PR detail pane. The focus is the mutating actions: this is
 * the only part of the app that writes to GitHub, so what it does — and
 * refuses to do — matters more than how it looks.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PrDetail } from "./PrDetail";
import type { Conversation, PullRequestDetail } from "../types";

vi.mock("../ipc", () => ({
  prDetail: vi.fn(),
  prComments: vi.fn(),
  prComment: vi.fn(),
  prCommentReply: vi.fn(),
  prResolve: vi.fn(),
  prUpdate: vi.fn(),
  prBranches: vi.fn(),
  prLabels: vi.fn(),
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
    reviewers: [{ login: "dave", state: "pending", rounds: 0 }],
    checks: [{ name: "build", conclusion: "success", url: null }],
    mergeability: "clean",
    url: "https://github.com/acme/api/pull/482",
    head_sha: "abc123",
    ...overrides,
  };
}

function conversation(overrides: Partial<Conversation> = {}): Conversation {
  return {
    issue_comments: [],
    review_threads: [],
    ...overrides,
  };
}

describe("PrDetail", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ipc.prComments).mockResolvedValue(conversation());
    vi.mocked(ipc.prBranches).mockResolvedValue(["main", "develop"]);
    vi.mocked(ipc.prLabels).mockResolvedValue(["bug", "enhancement", "urgent"]);
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

  it("shows each reviewer's review-round count next to their name", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(
      pr({
        reviewers: [
          { login: "dave", state: "approved", rounds: 3 },
          { login: "erin", state: "pending", rounds: 0 },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");

    expect(screen.getByText(/^dave/)).toBeInTheDocument();
    expect(screen.getByLabelText("3 review rounds")).toBeInTheDocument();
    expect(screen.getByLabelText("0 review rounds")).toBeInTheDocument();
    expect(screen.getByText("approved")).toBeInTheDocument();
    expect(screen.getByText("pending")).toBeInTheDocument();
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

  it("closes the pull request via the Close PR button", async () => {
    // The footer action reads "Close PR" to distinguish it from the button
    // that closes the detail pane itself.
    vi.spyOn(window, "confirm").mockReturnValue(true);
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());
    vi.mocked(ipc.prClose).mockResolvedValue(undefined);

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Close PR" }));

    expect(ipc.prClose).toHaveBeenCalledWith("acme/api", 482);
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

    // The description and metadata were untouched, so only the title goes out.
    await waitFor(() => {
      expect(ipc.prUpdate).toHaveBeenCalledWith("acme/api", 482, {
        title: "New title",
      });
    });
  });

  it("edits the base branch, labels, and draft flag inline", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(pr());
    vi.mocked(ipc.prUpdate).mockResolvedValue(
      pr({ base: "develop", draft: true, labels: ["bug", "hotfix"] }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Add rate limiting");
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));

    // Opening edit loads branches for the target-branch picker and the repo
    // labels for the tag picker.
    await waitFor(() => {
      expect(ipc.prBranches).toHaveBeenCalledWith("acme/api");
      expect(ipc.prLabels).toHaveBeenCalledWith("acme/api");
    });

    const base = screen.getByLabelText("Base branch");
    await userEvent.clear(base);
    await userEvent.type(base, "develop");

    // "enhancement" is on the PR and starts selected; "bug" is not.
    expect(screen.getByRole("button", { name: "enhancement" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await userEvent.click(screen.getByRole("button", { name: "bug" }));
    await userEvent.click(screen.getByRole("button", { name: "enhancement" }));

    // A brand-new label can be typed in; GitHub creates it on assignment.
    await userEvent.type(screen.getByLabelText("New label"), "hotfix{Enter}");

    await userEvent.click(screen.getByLabelText("Draft"));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(ipc.prUpdate).toHaveBeenCalledWith("acme/api", 482, {
        base: "develop",
        labels: ["bug", "hotfix"],
        draft: true,
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

  it("renders issue comments and review threads as a threaded conversation", async () => {
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        issue_comments: [
          { id: 1, author: "bob", body: "Nice work", created_at: "2026-08-13T12:00:00Z", path: null },
        ],
        review_threads: [
          {
            id: "thread-1",
            path: "src/api.rs",
            resolved: false,
            comments: [
              { id: 10, author: "carol", body: "Nits on line 5", created_at: "2026-08-13T12:00:00Z", path: null },
              { id: 11, author: "dave", body: "Fixed", created_at: "2026-08-13T12:00:00Z", path: null },
            ],
          },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    expect(await screen.findByText("Nice work")).toBeInTheDocument();
    expect(screen.getByText("src/api.rs")).toBeInTheDocument();
    expect(screen.getByText("Nits on line 5")).toBeInTheDocument();
    expect(screen.getByText("Fixed")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Conversation (3)" })).toBeInTheDocument();
  });

  it("renders comment and description markdown, sanitized", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(
      pr({ body: "**Bold** description with `code` and [link](https://x.test) <script>alert(1)</script>" }),
    );
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        issue_comments: [
          { id: 1, author: "bob", body: "Inline **bold** comment", created_at: "2026-08-13T12:00:00Z", path: null },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    await waitFor(() => {
      expect(document.querySelectorAll(".markdown strong").length).toBe(2);
    });
    // Markdown came through; the inline `<script>` did not.
    expect(document.querySelector(".markdown strong")?.textContent).toBe("Bold");
    expect(document.querySelector(".markdown a")?.getAttribute("href")).toBe("https://x.test");
    expect(document.querySelector("script")).toBeNull();
    expect(screen.queryByText("alert(1)")).not.toBeInTheDocument();
  });

  it("marks resolved threads and offers an unresolve action", async () => {
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        review_threads: [
          {
            id: "thread-1",
            path: null,
            resolved: true,
            comments: [
              { id: 10, author: "carol", body: "Done", created_at: "2026-08-13T12:00:00Z", path: null },
            ],
          },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    expect(await screen.findByText("Done")).toBeInTheDocument();
    expect(screen.getByText("Resolved")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Unresolve" }));
    await waitFor(() => {
      expect(ipc.prResolve).toHaveBeenCalledWith("acme/api", "thread-1", false);
    });
  });

  it("replies inside a thread using the last comment's id", async () => {
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        review_threads: [
          {
            id: "thread-1",
            path: "src/api.rs",
            resolved: false,
            comments: [
              { id: 10, author: "carol", body: "Nits", created_at: "2026-08-13T12:00:00Z", path: null },
              { id: 11, author: "dave", body: "Fixed", created_at: "2026-08-13T12:00:00Z", path: null },
            ],
          },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Nits");

    await userEvent.click(screen.getByRole("button", { name: "Reply in thread" }));
    const reply = screen.getByLabelText("Reply");
    await userEvent.type(reply, "Thanks!");
    await userEvent.click(screen.getByRole("button", { name: "Post reply" }));

    await waitFor(() => {
      expect(ipc.prCommentReply).toHaveBeenCalledWith("acme/api", 482, 11, "Thanks!");
    });
  });

  it("focuses the reply box when opened, and Esc cancels the draft", async () => {
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        review_threads: [
          {
            id: "thread-1",
            path: null,
            resolved: false,
            comments: [
              { id: 10, author: "carol", body: "Nits", created_at: "2026-08-13T12:00:00Z", path: null },
            ],
          },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Nits");

    await userEvent.click(screen.getByRole("button", { name: "Reply in thread" }));
    const reply = screen.getByLabelText("Reply");
    // Opened ready to type, no extra click needed.
    expect(reply).toHaveFocus();

    await userEvent.type(reply, "Draft");
    await userEvent.keyboard("{Escape}");
    // Esc discards the draft and closes the box.
    expect(screen.queryByLabelText("Reply")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Reply in thread" })).toBeInTheDocument();
    expect(ipc.prCommentReply).not.toHaveBeenCalled();
  });

  it("posts the reply on Shift+Enter and keeps bare Enter a newline", async () => {
    vi.mocked(ipc.prComments).mockResolvedValue(
      conversation({
        review_threads: [
          {
            id: "thread-1",
            path: null,
            resolved: false,
            comments: [
              { id: 10, author: "carol", body: "Nits", created_at: "2026-08-13T12:00:00Z", path: null },
            ],
          },
        ],
      }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);
    await screen.findByText("Nits");

    await userEvent.click(screen.getByRole("button", { name: "Reply in thread" }));
    const reply = screen.getByLabelText("Reply");

    // A bare Enter is a newline, not a send.
    await userEvent.type(reply, "line one{Enter}line two");
    expect((reply as HTMLTextAreaElement).value).toBe("line one\nline two");
    expect(ipc.prCommentReply).not.toHaveBeenCalled();

    await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
    await waitFor(() => {
      expect(ipc.prCommentReply).toHaveBeenCalledWith(
        "acme/api",
        482,
        10,
        "line one\nline two",
      );
    });
  });

  it("opens markdown links in the system browser", async () => {
    vi.mocked(ipc.prDetail).mockResolvedValue(
      pr({ body: "See [docs](https://docs.example.com/guide) for details" }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    const link = await screen.findByRole("link", { name: "docs" });
    await userEvent.click(link);

    await waitFor(() => {
      expect(ipc.openUrl).toHaveBeenCalledWith("https://docs.example.com/guide");
    });
  });

  it("copies a markdown link from its context menu", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    vi.mocked(ipc.prDetail).mockResolvedValue(
      pr({ body: "See [docs](https://docs.example.com/guide) for details" }),
    );

    render(<PrDetail repo="acme/api" number={482} onClose={vi.fn()} onChanged={vi.fn()} onResolve={vi.fn()} />);

    const link = await screen.findByRole("link", { name: "docs" });
    fireEvent.contextMenu(link);

    const copy = await screen.findByRole("menuitem", { name: "Copy link" });
    await userEvent.click(copy);

    expect(writeText).toHaveBeenCalledWith("https://docs.example.com/guide");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
});

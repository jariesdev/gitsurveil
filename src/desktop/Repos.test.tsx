/**
 * Smoke test for the Repositories pane: it must render (and not blow up the
 * whole window) for the shapes the daemon can actually hand back.
 */

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Repos } from "./Repos";
import {
  appsList,
  appsOpen,
  openUrl,
  reposClone,
  reposCloneStatus,
  reposSet,
  reposWorktreeAdd,
  reposWorktreeRemove,
  reposWorktrees,
} from "../ipc";import type {
  AccountRef,
  CloneStatus,
  RepoCatalog,
  Repository,
  WorktreesResult,
} from "../types";

vi.mock("../ipc", () => ({
  appsList: vi.fn().mockResolvedValue([]),
  appsOpen: vi.fn(),
  openUrl: vi.fn(),
  reposClone: vi.fn(),
  reposCloneStatus: vi.fn(),
  reposRefresh: vi.fn(),
  reposRemove: vi.fn(),
  reposSet: vi.fn(),
  reposWorktreeAdd: vi.fn(),
  reposWorktreeRemove: vi.fn(),
  reposWorktrees: vi.fn(),
}));

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }));

// Clear call histories (not implementations) so a lazy-load assertion like
// `reposWorktrees not called` isn't polluted by an earlier test's expand.
beforeEach(() => {
    vi.clearAllMocks();
});

function repository(overrides: Partial<Repository> = {}): Repository {
  return {
    account_id: "acc-1",
    host: "github.com",
    owner: "acme",
    name: "api",
    full_name: "acme/api",
    url: "https://github.com/acme/api",
    description: null,
    private: false,
    default_branch: "main",
    clone_url: "https://github.com/acme/api.git",
    clone_path: null,
    tracked: false,
    notify_enabled: true,
    first_seen_at: "2026-08-13T12:00:00Z",
    notified_at: null,
    last_refreshed_at: "2026-08-13T12:00:00Z",
    ...overrides,
  };
}

const account: AccountRef = {
  id: "acc-1",
  host: "github.com",
  api_base: "https://api.github.com",
  login: "alice",
  auth_kind: "pat",
};

const catalog: RepoCatalog = {
  orgs: [{ account_id: "acc-1", host: "github.com", name: "acme" }],
  repos: [
    repository(),
    repository({
      owner: "acme",
      name: "web",
      full_name: "acme/web",
      url: "https://github.com/acme/web",
      tracked: true,
      clone_path: "/tmp/acme/web",
    }),
  ],
};

describe("Repos", () => {
  it("renders the catalog rows", () => {
    render(
      <Repos catalog={catalog} accounts={[account]} onChange={() => {}} />,
    );
    expect(screen.getByText("acme/api")).toBeTruthy();
    expect(screen.getByText("acme/web")).toBeTruthy();
  });

  it("toggles worktrees on a single click of an expandable row", async () => {
    vi.mocked(reposWorktrees).mockResolvedValue({ worktrees: [], branches: ["main"] });
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);
    expect(reposWorktrees).not.toHaveBeenCalled();
    // acme/web is tracked with a clone path, so a single click expands it.
    fireEvent.click(screen.getByText("acme/web"));
    await waitFor(() => expect(reposWorktrees).toHaveBeenCalledWith("acme/web"));
  });

  it("opens a repo in the browser on a double click of the row", () => {
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);
    fireEvent.doubleClick(screen.getByText("acme/api"));
    expect(openUrl).toHaveBeenCalledWith("https://github.com/acme/api");
  });

  it("lets an untracked repo map an existing local clone read-only", async () => {
    dialog.open.mockResolvedValueOnce("/Users/alice/work/acme/api");
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Actions for acme/api" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Pick existing clone…" }));

    await waitFor(() =>
      expect(reposSet).toHaveBeenCalledWith("acme/api", "/Users/alice/work/acme/api"),
    );
  });

  it("polls a freshly started clone to completion and reloads the catalog", async () => {
    // Regression: a just-started job has `status === null`, so the poll loop
    // must treat it as running — otherwise no interval ever starts and the
    // clone sits in a stuck indeterminate bar until a manual refresh.
    vi.useFakeTimers();
    try {
      dialog.open.mockResolvedValueOnce("/Users/alice/work/new");
      vi.mocked(reposClone).mockResolvedValueOnce("job-1");
      const done: CloneStatus = {
        job_id: "job-1",
        status: "done",
        received: 0,
        total: 0,
        repo: null,
        error: null,
      };
      vi.mocked(reposCloneStatus).mockResolvedValueOnce(done);
      const onChange = vi.fn();
      render(<Repos catalog={catalog} accounts={[account]} onChange={onChange} />);

      fireEvent.click(screen.getByRole("button", { name: "Actions for acme/api" }));
      fireEvent.click(screen.getByRole("menuitem", { name: "Clone to…" }));
      // Flush the picker/IPC microtasks so the job is registered before we poll.
      await act(async () => {});
      expect(reposClone).toHaveBeenCalledWith("acme/api", "/Users/alice/work/new");

      await act(async () => {
        vi.advanceTimersByTime(1000);
      });
      await act(async () => {});
      expect(reposCloneStatus).toHaveBeenCalledWith("job-1");
      expect(onChange).toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  /** AC: a worktree whose branch belongs to a merged PR is marked, so the
   *  user can decide whether to keep it. The chip is informational — it must
   *  never remove anything by itself. */
  it("marks a worktree whose branch has a merged pull request", async () => {
    vi.mocked(reposWorktrees).mockResolvedValue({
      worktrees: [
        {
          name: "wt-acme-web-feature",
          path: "/tmp/acme/web/wt-feature",
          branch: "feature",
          head: "abc1234",
          merged_pr: {
            number: 482,
            title: "Add rate limits",
            url: "https://github.com/acme/web/pull/482",
          },
        },
        {
          name: "wt-acme-web-wip",
          path: "/tmp/acme/web/wt-wip",
          branch: "wip",
          head: "def5678",
        },
      ],
      branches: ["main", "feature", "wip"],
    });
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));

    const chip = await screen.findByTitle("Merged in #482: Add rate limits");
    expect(chip.textContent).toContain("Merged #482");
    // Exactly one chip: the unmerged worktree must not get one.
    expect(screen.getAllByText(/^Merged #/)).toHaveLength(1);

    fireEvent.click(chip);
    expect(openUrl).toHaveBeenCalledWith("https://github.com/acme/web/pull/482");
    // Nothing was removed — the chip only opens the PR.
    expect(reposWorktreeRemove).not.toHaveBeenCalled();
  });

  it("shows a Force delete button when worktree delete fails with dirty error", async () => {
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-web-feature", path: "/tmp/acme/web/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    vi.mocked(reposWorktrees).mockResolvedValue(worktrees);
    vi.mocked(reposWorktreeRemove).mockRejectedValue(
      "the worktree at /tmp/acme/web/wt-feature has uncommitted changes or untracked files — commit or stash them before deleting",
    );
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("feature");

    fireEvent.contextMenu(screen.getByText(/wt-feature/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete worktree" }));

    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(screen.getByText(/uncommitted changes/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Force delete" })).toBeTruthy();
  });

  it("opens confirm dialog on Force delete click and calls force delete on confirm", async () => {
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-web-feature", path: "/tmp/acme/web/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    vi.mocked(reposWorktrees).mockResolvedValue(worktrees);
    vi.mocked(reposWorktreeRemove)
      .mockRejectedValueOnce(
        "the worktree at /tmp/acme/web/wt-feature has uncommitted changes or untracked files — commit or stash them before deleting",
      )
      .mockResolvedValueOnce(undefined);
    const onChange = vi.fn();
    render(<Repos catalog={catalog} accounts={[account]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("feature");

    // Trigger dirty error.
    fireEvent.contextMenu(screen.getByText(/wt-feature/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete worktree" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Force delete" })).toBeTruthy());

    // Open confirm dialog.
    fireEvent.click(screen.getByRole("button", { name: "Force delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Force delete worktree?" });
    expect(dialog.textContent).toContain("permanently lost");

    // Confirm — scope to the dialog so we don't match the error bar button.
    fireEvent.click(within(dialog).getByRole("button", { name: "Force delete" }));
    await waitFor(() =>
      expect(reposWorktreeRemove).toHaveBeenCalledWith(
        "acme/web",
        "wt-acme-web-feature",
        true,
      ),
    );
    expect(onChange).toHaveBeenCalled();
  });

  it("cancels force delete when confirm dialog is dismissed", async () => {
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-web-feature", path: "/tmp/acme/web/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    vi.mocked(reposWorktrees).mockResolvedValue(worktrees);
    vi.mocked(reposWorktreeRemove).mockRejectedValueOnce(
      "the worktree at /tmp/acme/web/wt-feature has uncommitted changes or untracked files — commit or stash them before deleting",
    );
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("feature");

    fireEvent.contextMenu(screen.getByText(/wt-feature/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete worktree" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Force delete" })).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Force delete" }));
    await screen.findByRole("dialog", { name: "Force delete worktree?" });

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Force delete worktree?" })).toBeNull(),
    );
    // Only the initial (failed) call — no force call.
    expect(reposWorktreeRemove).toHaveBeenCalledTimes(1);
  });

  it("renders the daemon's real catalog data without crashing", () => {
    // Snapshot of the user's actual `repositories`/`orgs` rows (minus the
    // SQLite 0/1 ints, which serde emits as real booleans). Guards against a
    // regression where real-world data takes the pane down with it.
    const repos = [
      repository({
        owner: "ariesragingriverict",
        name: "ariesragingriverict",
        full_name: "ariesragingriverict/ariesragingriverict",
        url: "https://github.com/ariesragingriverict/ariesragingriverict",
        description: "Config files for my GitHub profile.",
        notified_at: "2026-08-14T15:22:26.237158+00:00",
      }),
      repository({
        owner: "ariesragingriverict",
        name: "omnipay-paypal",
        full_name: "ariesragingriverict/omnipay-paypal",
        url: "https://github.com/ariesragingriverict/omnipay-paypal",
        notified_at: "2026-08-14T15:22:26.237158+00:00",
      }),
      repository({
        owner: "wthvillas",
        name: "villasplatform",
        full_name: "wthvillas/villasplatform",
        url: "https://github.com/wthvillas/villasplatform",
        notified_at: "2026-08-14T15:22:26.237158+00:00",
      }),
    ];
    const realCatalog: RepoCatalog = {
      orgs: [
        { account_id: "acc-1", host: "github.com", name: "ariesragingriverict" },
        { account_id: "acc-1", host: "github.com", name: "wthvillas" },
      ],
      repos,
    };
    render(<Repos catalog={realCatalog} accounts={[account]} onChange={() => {}} />);
    expect(screen.getAllByText(/villasplatform/)).toHaveLength(1);
  });

  it("expands a tracked repo to lazy-load and list its worktrees", async () => {
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-api-feature", path: "/tmp/acme/api/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    vi.mocked(reposWorktrees).mockResolvedValue(worktrees);
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);

    expect(reposWorktrees).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));

    await waitFor(() => expect(reposWorktrees).toHaveBeenCalledWith("acme/web"));
    expect(await screen.findByText("feature")).toBeTruthy();
    expect(screen.getByText(/wt-feature/)).toBeTruthy();
    // An untracked repo has no clone and therefore no expand affordance.
    expect(
      screen.queryByRole("button", { name: "Worktrees for acme/api" }),
    ).toBeNull();
  });

  it("adds a worktree with the branch/relative path and reloads the catalog", async () => {
    vi.mocked(reposWorktrees).mockResolvedValue({ worktrees: [], branches: ["main", "feature"] });
    const onChange = vi.fn();
    render(<Repos catalog={catalog} accounts={[account]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("No worktrees yet.");

    fireEvent.change(screen.getByLabelText("Branch for new acme/web worktree"), {
      target: { value: "feature" },
    });
    // The path auto-derives as `wt-{owner}-{name}-{branch}` next to the clone.
    expect(
      (screen.getByLabelText("Path for new acme/web worktree") as HTMLInputElement).value,
    ).toBe("/tmp/acme/wt-acme-web-feature");
    fireEvent.submit(screen.getByRole("button", { name: "Add" }));

    await waitFor(() =>
      expect(reposWorktreeAdd).toHaveBeenCalledWith(
        "acme/web",
        "feature",
        "/tmp/acme/wt-acme-web-feature",
      ),
    );
    expect(onChange).toHaveBeenCalled();
    // The branch field resets for the next worktree; the path returns to the
    // default for an empty branch.
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Branch for new acme/web worktree") as HTMLInputElement).value,
      ).toBe(""),
    );
    expect(
      (screen.getByLabelText("Path for new acme/web worktree") as HTMLInputElement).value,
    ).toBe("/tmp/acme/wt-acme-web-work");
  });

  it("deletes a worktree from its row's context menu and reloads the catalog", async () => {
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-web-feature", path: "/tmp/acme/web/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    // First call renders the list; the delete triggers a reload that returns
    // the list without the removed worktree.
    vi.mocked(reposWorktrees)
      .mockResolvedValueOnce(worktrees)
      .mockResolvedValueOnce({ worktrees: [], branches: ["main", "feature"] });
    const onChange = vi.fn();
    render(<Repos catalog={catalog} accounts={[account]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("feature");

    vi.mocked(reposWorktreeRemove).mockResolvedValue(undefined);
    fireEvent.contextMenu(screen.getByText(/wt-feature/));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete worktree" }));

    await waitFor(() => {
      expect(reposWorktreeRemove).toHaveBeenCalledWith(
        "acme/web",
        "wt-acme-web-feature",
        false,
      );
      expect(onChange).toHaveBeenCalled();
    });
    // The deleted row is removed from the list in place.
    await waitFor(() =>
      expect(screen.queryByText(/wt-feature/)).toBeNull(),
    );
    expect(screen.getByText("No worktrees yet.")).toBeTruthy();
  });

  it("opens a worktree with a registered app via the submenu", async () => {
    vi.mocked(appsList).mockResolvedValue([
      { name: "VS Code", command: "code" },
    ]);
    const worktrees: WorktreesResult = {
      worktrees: [
        { name: "wt-acme-web-feature", path: "/tmp/acme/web/wt-feature", branch: "feature", head: "abc1234" },
      ],
      branches: ["main", "feature"],
    };
    vi.mocked(reposWorktrees).mockResolvedValue(worktrees);
    render(<Repos catalog={catalog} accounts={[account]} onChange={() => {}} />);

    fireEvent.click(screen.getByRole("button", { name: "Worktrees for acme/web" }));
    await screen.findByText("feature");

    fireEvent.contextMenu(screen.getByText(/wt-feature/));
    const openWith = screen.getByRole("menuitem", { name: "Open with" });
    // The submenu is closed until hovered.
    expect(screen.queryByRole("menuitem", { name: "VS Code" })).toBeNull();
    fireEvent.mouseOver(openWith);
    fireEvent.click(screen.getByRole("menuitem", { name: "VS Code" }));

    await waitFor(() =>
      expect(appsOpen).toHaveBeenCalledWith("code", "/tmp/acme/web/wt-feature"),
    );
  });
});

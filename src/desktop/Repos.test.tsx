/**
 * Smoke test for the Repositories pane: it must render (and not blow up the
 * whole window) for the shapes the daemon can actually hand back.
 */

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Repos } from "./Repos";
import { reposClone, reposCloneStatus, reposSet } from "../ipc";
import type { AccountRef, CloneStatus, RepoCatalog, Repository } from "../types";

vi.mock("../ipc", () => ({
  openUrl: vi.fn(),
  reposClone: vi.fn(),
  reposCloneStatus: vi.fn(),
  reposRefresh: vi.fn(),
  reposRemove: vi.fn(),
  reposSet: vi.fn(),
}));

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }));

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
});

/**
 * Regression test: a single PR can produce more than one `ActionItem` (e.g.
 * Assigned + Authored + ReadyToMerge all on the same `repo`#`number`), so the
 * active-row highlight and the detail-pane item lookup must key off `id`,
 * not `repo`/`number` — otherwise every row for that PR lights up together.
 */

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Dashboard } from "./Dashboard";
import type { ScoredItem } from "../types";

vi.mock("../ipc", () => ({
  dismissItem: vi.fn(),
  openUrl: vi.fn(),
  pollNow: vi.fn(),
  // Left pending so PrDetail stays in its loading state — this test only
  // cares about row highlighting, not detail-pane content.
  prDetail: vi.fn(() => new Promise(() => {})),
  prComments: vi.fn(() => new Promise(() => {})),
}));

function item(overrides: Partial<ScoredItem>): ScoredItem {
  return {
    id: "id-1",
    account_id: "acc-1",
    kind: "assigned",
    state: "open",
    repo: "wthvillas/villasplatform",
    number: 545,
    title: "From Pricing in SERP",
    url: "https://github.com/wthvillas/villasplatform/pull/545",
    author: "someone",
    created_at: "2026-08-10T00:00:00Z",
    updated_at: "2026-08-18T00:00:00Z",
    first_seen_at: "2026-08-13T00:00:00Z",
    last_seen_at: "2026-08-18T00:00:00Z",
    ci_status: "passing",
    raw_kind: "Assigned",
    dismissed_updated_at: null,
    dismissed_at: null,
    dismissed_ci_status: null,
    score: 70,
    severity: "high",
    muted: false,
    ...overrides,
  };
}

describe("Dashboard active-row highlight", () => {
  it("highlights only the clicked row when several items share the same repo#number", async () => {
    const user = userEvent.setup();
    const items = [
      item({ id: "id-assigned", kind: "assigned" }),
      item({ id: "id-authored", kind: "authored" }),
      item({ id: "id-ready", kind: "ready_to_merge" }),
    ];

    render(
      <Dashboard items={items} accounts={[]} onRefresh={vi.fn()} onOpenAccounts={vi.fn()} />,
    );

    const rowFor = (kindLabel: string): HTMLElement =>
      screen
        .getAllByText(kindLabel)
        .map((el) => el.closest<HTMLElement>("div.group"))
        .find((row): row is HTMLElement => row !== null)!;
    const assignedRow = rowFor("Assigned");
    const authoredRow = rowFor("Your PR");
    const readyRow = rowFor("Ready to merge");

    await user.click(within(assignedRow).getByTitle("From Pricing in SERP"));

    expect(assignedRow).toHaveAttribute("aria-current", "true");
    expect(authoredRow).not.toHaveAttribute("aria-current");
    expect(readyRow).not.toHaveAttribute("aria-current");
  });
});

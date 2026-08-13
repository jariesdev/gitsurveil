/**
 * Tests for the Pull Requests view filters. These decide which PRs a user
 * sees, so they're tested directly rather than through the component.
 */

import { describe, expect, it } from "vitest";
import {
  applyPrFilters,
  matchesAttention,
  NO_PR_FILTERS,
  sortByRecent,
} from "./filters";
import type { PullRequestSummary } from "../../types";

function pr(overrides: Partial<PullRequestSummary> = {}): PullRequestSummary {
  return {
    account_id: "acc-1",
    repo: "acme/api",
    number: 1,
    title: "Fix the thing",
    url: "u",
    author: "someone",
    roles: ["authored"],
    state: "open",
    draft: false,
    ci_status: "none",
    review_decision: "none",
    mergeability: "clean",
    created_at: "2026-08-13T12:00:00Z",
    updated_at: "2026-08-13T12:00:00Z",
    ...overrides,
  };
}

describe("applyPrFilters", () => {
  it("returns everything when no filter is set (AC-3.4)", () => {
    const list = [pr({ number: 1 }), pr({ number: 2 })];
    expect(applyPrFilters(list, NO_PR_FILTERS)).toHaveLength(2);
  });

  it("filters by account in isolation (AC-3.1)", () => {
    const list = [pr({ number: 1 }), pr({ number: 2, account_id: "acc-2" })];
    const found = applyPrFilters(list, { ...NO_PR_FILTERS, accountId: "acc-2" });
    expect(found.map((p) => p.number)).toEqual([2]);
  });

  it("filters by repository in isolation (AC-3.1)", () => {
    const list = [pr({ number: 1 }), pr({ number: 2, repo: "acme/web" })];
    const found = applyPrFilters(list, { ...NO_PR_FILTERS, repo: "acme/web" });
    expect(found.map((p) => p.number)).toEqual([2]);
  });

  it("filters by role, matching a PR that carries it among several (AC-3.1)", () => {
    const list = [
      pr({ number: 1, roles: ["authored", "assigned"] }),
      pr({ number: 2, roles: ["review_requested"] }),
    ];
    const found = applyPrFilters(list, { ...NO_PR_FILTERS, role: "assigned" });
    expect(found.map((p) => p.number)).toEqual([1]);
  });

  it.each([
    ["draft", pr({ draft: true })],
    ["ci_failing", pr({ ci_status: "failing" })],
    ["approved", pr({ review_decision: "approved" })],
  ] as const)("filters by attention=%s in isolation (AC-3.1)", (attention, keep) => {
    const list = [keep, pr({ number: 2 })];
    const found = applyPrFilters(list, { ...NO_PR_FILTERS, attention });
    expect(found).toEqual([keep]);
  });

  it("search matches both title and repository, case-insensitively (AC-3.3)", () => {
    const list = [
      pr({ number: 1, title: "Fix login" }),
      pr({ number: 2, title: "Other", repo: "acme/LOGIN-service" }),
      pr({ number: 3, title: "Unrelated", repo: "acme/web" }),
    ];
    const found = applyPrFilters(list, { ...NO_PR_FILTERS, search: "LOGIN" });
    expect(found.map((p) => p.number)).toEqual([1, 2]);
  });

  it("combines dimensions as AND (AC-3.2)", () => {
    const list = [
      pr({ number: 1, account_id: "acc-2", repo: "acme/api", draft: true }),
      pr({ number: 2, account_id: "acc-1", repo: "acme/api", draft: true }),
      pr({ number: 3, account_id: "acc-1", repo: "acme/web", draft: true }),
    ];
    const found = applyPrFilters(list, {
      ...NO_PR_FILTERS,
      accountId: "acc-1",
      repo: "acme/api",
      attention: "draft",
    });
    expect(found.map((p) => p.number)).toEqual([2]);
  });
});

describe("matchesAttention", () => {
  it("flags only an explicit conflict, never unknown (AC-4.5)", () => {
    expect(matchesAttention(pr({ mergeability: "conflicted" }), "conflicted")).toBe(true);
    expect(matchesAttention(pr({ mergeability: "unknown" }), "conflicted")).toBe(false);
    expect(matchesAttention(pr({ mergeability: "clean" }), "conflicted")).toBe(false);
    expect(matchesAttention(pr({ mergeability: "blocked" }), "conflicted")).toBe(false);
  });

  it("treats pending CI as not failing", () => {
    expect(matchesAttention(pr({ ci_status: "pending" }), "ci_failing")).toBe(false);
  });
});

describe("sortByRecent", () => {
  it("orders most-recently-updated first and does not mutate the input", () => {
    const list = [
      pr({ number: 1, updated_at: "2026-08-13T09:00:00Z" }),
      pr({ number: 2, updated_at: "2026-08-13T18:00:00Z" }),
      pr({ number: 3, updated_at: "2026-08-13T12:00:00Z" }),
    ];
    const sorted = sortByRecent(list);
    expect(sorted.map((p) => p.number)).toEqual([2, 3, 1]);
    expect(list.map((p) => p.number)).toEqual([1, 2, 3]);
  });
});

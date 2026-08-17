/**
 * Tests for dashboard grouping and filtering. These decide what a user
 * actually sees, so they're tested directly rather than through the component.
 */

import { describe, expect, it } from "vitest";
import { applyFilters, groupItems, NO_FILTERS } from "./grouping";
import type { ScoredItem } from "../types";

function item(overrides: Partial<ScoredItem> = {}): ScoredItem {
  return {
    id: "i",
    account_id: "acc-1",
    kind: "assigned",
    state: "open",
    repo: "acme/api",
    number: 1,
    title: "Fix the thing",
    url: "u",
    author: "someone",
    created_at: "2026-08-13T12:00:00Z",
    updated_at: "2026-08-13T12:00:00Z",
    first_seen_at: "2026-08-13T12:00:00Z",
    last_seen_at: "2026-08-13T12:00:00Z",
    ci_status: "none",
    raw_kind: "assign",
    score: 40,
    severity: "normal",
    muted: false,
    ...overrides,
  };
}

describe("applyFilters", () => {
  it("returns everything when no filter is set", () => {
    const items = [item({ id: "a" }), item({ id: "b" })];
    expect(applyFilters(items, NO_FILTERS)).toHaveLength(2);
  });

  it("matches search against both title and repository, case-insensitively", () => {
    const items = [
      item({ id: "a", title: "Fix login" }),
      item({ id: "b", title: "Other", repo: "acme/login-service" }),
      item({ id: "c", title: "Unrelated", repo: "acme/web" }),
    ];
    const found = applyFilters(items, { ...NO_FILTERS, search: "LOGIN" });
    expect(found.map((i) => i.id)).toEqual(["a", "b"]);
  });

  it("combines filters as AND", () => {
    const items = [
      item({ id: "a", kind: "ci_failed", severity: "critical" }),
      item({ id: "b", kind: "ci_failed", severity: "normal" }),
      item({ id: "c", kind: "assigned", severity: "critical" }),
    ];
    const found = applyFilters(items, {
      ...NO_FILTERS,
      kind: "ci_failed",
      severity: "critical",
    });
    expect(found.map((i) => i.id)).toEqual(["a"]);
  });

  it("filters by account", () => {
    const items = [
      item({ id: "a", account_id: "acc-1" }),
      item({ id: "b", account_id: "acc-2" }),
    ];
    const found = applyFilters(items, { ...NO_FILTERS, accountId: "acc-2" });
    expect(found.map((i) => i.id)).toEqual(["b"]);
  });

  it("filters by repos when repos array is non-empty", () => {
    const items = [
      item({ id: "a", repo: "acme/api" }),
      item({ id: "b", repo: "acme/web" }),
      item({ id: "c", repo: "acme/api" }),
    ];
    const found = applyFilters(items, { ...NO_FILTERS, repos: ["acme/api"] });
    expect(found.map((i) => i.id)).toEqual(["a", "c"]);
  });

  it("passes all items when repos array is empty", () => {
    const items = [
      item({ id: "a", repo: "acme/api" }),
      item({ id: "b", repo: "acme/web" }),
    ];
    const found = applyFilters(items, { ...NO_FILTERS, repos: [] });
    expect(found.map((i) => i.id)).toEqual(["a", "b"]);
  });

  it("combines repo filter with search — repo narrows first, search within", () => {
    const items = [
      item({ id: "a", repo: "acme/api", title: "Fix login" }),
      item({ id: "b", repo: "acme/api", title: "Add tests" }),
      item({ id: "c", repo: "acme/web", title: "Fix login" }),
    ];
    const found = applyFilters(items, {
      ...NO_FILTERS,
      repos: ["acme/api"],
      search: "login",
    });
    expect(found.map((i) => i.id)).toEqual(["a"]);
  });

  it("combines repos with multiple selections as OR within repos", () => {
    const items = [
      item({ id: "a", repo: "acme/api" }),
      item({ id: "b", repo: "acme/web" }),
      item({ id: "c", repo: "acme/mobile" }),
    ];
    const found = applyFilters(items, {
      ...NO_FILTERS,
      repos: ["acme/api", "acme/web"],
    });
    expect(found.map((i) => i.id)).toEqual(["a", "b"]);
  });
});

describe("groupItems", () => {
  it("groups by severity in descending urgency and drops empty groups", () => {
    const items = [
      item({ id: "crit", severity: "critical" }),
      item({ id: "norm", severity: "normal" }),
    ];
    const groups = groupItems(items, "priority");
    expect(groups.map((g) => g.key)).toEqual(["critical", "normal"]);
    expect(groups.every((g) => g.items.length > 0)).toBe(true);
  });

  it("orders type groups by their most urgent member", () => {
    // A category is only as urgent as its top item; sorting by that keeps the
    // section you must act on first at the top.
    const items = [
      item({ id: "a", kind: "assigned", score: 40 }),
      item({ id: "b", kind: "ci_failed", score: 100 }),
      item({ id: "c", kind: "assigned", score: 35 }),
    ];
    const groups = groupItems(items, "type");
    expect(groups.map((g) => g.key)).toEqual(["ci_failed", "assigned"]);
  });

  it("preserves the daemon's ordering inside a group", () => {
    const items = [
      item({ id: "first", severity: "normal", score: 50 }),
      item({ id: "second", severity: "normal", score: 40 }),
    ];
    const [group] = groupItems(items, "priority");
    expect(group.items.map((i) => i.id)).toEqual(["first", "second"]);
  });

  it("returns no groups for an empty list", () => {
    expect(groupItems([], "priority")).toEqual([]);
    expect(groupItems([], "type")).toEqual([]);
  });
});

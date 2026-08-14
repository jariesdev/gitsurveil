/**
 * Tests for the Repositories pane filters. These decide which catalog rows a
 * user sees, so they're tested directly rather than through the component —
 * mirroring `src/desktop/PullRequests/filters.test.ts`.
 */

import { describe, expect, it } from "vitest";
import {
  applyRepoFilters,
  hasActiveRepoFilters,
  NO_REPO_FILTERS,
  orgOptions,
  reviveRepoFilters,
} from "./repoFilters";
import type { OrgRef, Repository } from "../types";

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

const org = (account_id: string, name: string): OrgRef => ({ account_id, host: "github.com", name });

describe("applyRepoFilters", () => {
  it("returns everything, sorted by full_name, when no filter is set", () => {
    const list = [
      repository({ full_name: "zebra/zoo", owner: "zebra", name: "zoo" }),
      repository({ full_name: "acme/api", owner: "acme", name: "api" }),
    ];
    expect(applyRepoFilters(list, NO_REPO_FILTERS).map((r) => r.full_name)).toEqual([
      "acme/api",
      "zebra/zoo",
    ]);
  });

  it("filters by account in isolation", () => {
    const list = [repository(), repository({ account_id: "acc-2" })];
    const found = applyRepoFilters(list, { ...NO_REPO_FILTERS, accountId: "acc-2" });
    expect(found).toHaveLength(1);
    expect(found[0].account_id).toBe("acc-2");
  });

  it("filters by organization in isolation", () => {
    const list = [repository(), repository({ owner: "other", full_name: "other/lib" })];
    const found = applyRepoFilters(list, { ...NO_REPO_FILTERS, org: "acme" });
    expect(found.map((r) => r.full_name)).toEqual(["acme/api"]);
  });

  it("combines account and organization as AND", () => {
    const list = [
      repository(), // acc-1 / acme
      repository({ account_id: "acc-2" }), // acc-2 / acme
      repository({ owner: "acme", full_name: "acme/tools" }), // acc-1 / acme
    ];
    const found = applyRepoFilters(list, { accountId: "acc-2", org: "acme" });
    expect(found.map((r) => r.full_name)).toEqual(["acme/api"]);
  });

  it("cannot match legacy rows that have no account", () => {
    const list = [repository({ account_id: null })];
    const found = applyRepoFilters(list, { ...NO_REPO_FILTERS, accountId: "acc-1" });
    expect(found).toHaveLength(0);
  });
});

describe("orgOptions", () => {
  const orgs = [org("acc-1", "acme"), org("acc-1", "zebra"), org("acc-2", "acme")];
  const repos = [
    repository({ owner: "acme", full_name: "acme/api" }),
    repository({ owner: "acme", full_name: "acme/tools" }),
    repository({ owner: "zebra", full_name: "zebra/zoo" }),
  ];

  it("lists only the selected account's orgs, with repo counts, sorted", () => {
    expect(orgOptions(orgs, repos, "acc-1")).toEqual([
      { name: "acme", count: 2 },
      { name: "zebra", count: 1 },
    ]);
  });

  it("keeps an org in the list even when every repo under it is filtered out", () => {
    expect(orgOptions(orgs, [], "acc-1")).toEqual([
      { name: "acme", count: 0 },
      { name: "zebra", count: 0 },
    ]);
  });
});

describe("reviveRepoFilters", () => {
  const known = ["acc-1", "acc-2"];
  const orgNames = (accountId: string) =>
    accountId === "acc-1" ? ["acme"] : [];

  it("falls back when the stored value is not an object", () => {
    expect(reviveRepoFilters(null, known, orgNames)).toEqual(NO_REPO_FILTERS);
    expect(reviveRepoFilters("nope", known, orgNames)).toEqual(NO_REPO_FILTERS);
  });

  it("drops an account id that no longer exists", () => {
    const revived = reviveRepoFilters({ accountId: "gone", org: "acme" }, known, orgNames);
    expect(revived).toEqual(NO_REPO_FILTERS);
  });

  it("drops an org that does not exist under the account", () => {
    const revived = reviveRepoFilters({ accountId: "acc-1", org: "ghost" }, known, orgNames);
    expect(revived).toEqual({ accountId: "acc-1", org: "" });
  });

  it("keeps a valid account and org", () => {
    const revived = reviveRepoFilters({ accountId: "acc-1", org: "acme" }, known, orgNames);
    expect(revived).toEqual({ accountId: "acc-1", org: "acme" });
  });
});

describe("hasActiveRepoFilters", () => {
  it("is false when nothing constrains the list", () => {
    expect(hasActiveRepoFilters(NO_REPO_FILTERS)).toBe(false);
  });

  it("is true when either dimension is set", () => {
    expect(hasActiveRepoFilters({ accountId: "acc-1", org: "" })).toBe(true);
    expect(hasActiveRepoFilters({ accountId: "", org: "acme" })).toBe(true);
  });
});

/**
 * Tests for filter persistence.
 *
 * The point of the hook is that a value survives unmounting, so the tests
 * unmount and remount rather than asserting on `localStorage` contents — that
 * is the behaviour the user notices when switching sidebar views or closing
 * the window.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { usePersistentState } from "./usePersistentState";
import { revivePrFilters, NO_PR_FILTERS } from "./PullRequests/filters";

function Counter({ storageKey = "test.value" }: { storageKey?: string }) {
  const [value, setValue] = usePersistentState(storageKey, "initial");
  return (
    <button type="button" onClick={() => setValue("changed")}>
      {value}
    </button>
  );
}

describe("usePersistentState", () => {
  beforeEach(() => localStorage.clear());

  it("restores the value after the component unmounts", async () => {
    const first = render(<Counter />);
    await userEvent.click(screen.getByRole("button"));
    expect(screen.getByRole("button")).toHaveTextContent("changed");

    // Exactly what leaving and returning to the view does.
    first.unmount();
    render(<Counter />);
    expect(screen.getByRole("button")).toHaveTextContent("changed");
  });

  it("falls back when nothing is stored", () => {
    render(<Counter />);
    expect(screen.getByRole("button")).toHaveTextContent("initial");
  });

  it("falls back rather than throwing on corrupt storage", () => {
    // A blank pane is a far worse outcome than a forgotten filter, so
    // unparseable storage must not propagate.
    localStorage.setItem("gitsurveil.test.value", "{not json");
    render(<Counter />);
    expect(screen.getByRole("button")).toHaveTextContent("initial");
  });

  it("keeps separate keys independent", async () => {
    render(<Counter storageKey="a" />);
    await userEvent.click(screen.getByRole("button"));
    render(<Counter storageKey="b" />);
    expect(screen.getAllByRole("button")[1]).toHaveTextContent("initial");
  });
});

describe("revivePrFilters", () => {
  it("restores a filter set that is still valid", () => {
    const stored = {
      search: "login",
      accountId: "acc-1",
      repo: "acme/api",
      role: "authored",
      attention: "conflicted",
    };
    expect(revivePrFilters(stored, ["acc-1"])).toEqual(stored);
  });

  it("clears an account filter naming an account that is gone", () => {
    // Otherwise the list is empty and nothing on screen explains why: the
    // dropdown would show a value that no longer has an entry.
    const revived = revivePrFilters(
      { ...NO_PR_FILTERS, accountId: "deleted-account" },
      ["acc-1"],
    );
    expect(revived.accountId).toBe("");
  });

  it("drops values outside the known set", () => {
    const revived = revivePrFilters(
      { role: "not-a-role", attention: "nonsense", search: 42 },
      [],
    );
    expect(revived).toEqual(NO_PR_FILTERS);
  });

  it("survives junk without throwing", () => {
    expect(revivePrFilters(null, [])).toEqual(NO_PR_FILTERS);
    expect(revivePrFilters("a string", [])).toEqual(NO_PR_FILTERS);
    expect(revivePrFilters(7, [])).toEqual(NO_PR_FILTERS);
  });
});

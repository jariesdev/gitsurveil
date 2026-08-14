/**
 * Tests for the item row's context menu: right-clicking a row must offer
 * **Copy URL**, which copies the item's URL without triggering `onOpen`.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ItemRow } from "./ItemRow";
import type { ScoredItem } from "../types";

function item(overrides: Partial<ScoredItem> = {}): ScoredItem {
  return {
    id: "item-1",
    account_id: "acc-1",
    kind: "review_requested",
    state: "open",
    repo: "acme/api",
    number: 482,
    title: "Add rate limiting",
    url: "https://github.com/acme/api/pull/482",
    author: "carol",
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    first_seen_at: new Date().toISOString(),
    last_seen_at: new Date().toISOString(),
    ci_status: "passing",
    raw_kind: "review_requested",
    score: 80,
    severity: "high",
    muted: false,
    ...overrides,
  };
}

describe("ItemRow context menu", () => {
  it("copies the item URL from the row's context menu", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const onOpen = vi.fn();
    render(<ItemRow item={item()} onOpen={onOpen} />);

    const row = screen.getByText("Add rate limiting").closest("div.group");
    fireEvent.contextMenu(row!, { clientX: 100, clientY: 60 });

    const menuItem = await screen.findByRole("menuitem", { name: "Copy URL" });
    fireEvent.click(menuItem);

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("https://github.com/acme/api/pull/482");
    });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("dismisses the menu without copying anything when clicking away", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    render(<ItemRow item={item()} onOpen={vi.fn()} />);

    const row = screen.getByText("Add rate limiting").closest("div.group");
    fireEvent.contextMenu(row!, { clientX: 100, clientY: 60 });
    await screen.findByRole("menuitem", { name: "Copy URL" });

    fireEvent.pointerDown(document.body);

    await waitFor(() => {
      expect(screen.queryByRole("menuitem")).not.toBeInTheDocument();
    });
    expect(writeText).not.toHaveBeenCalled();
  });
});

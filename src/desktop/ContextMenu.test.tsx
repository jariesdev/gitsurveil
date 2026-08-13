/**
 * Tests for the context menu shown on PR rows.
 *
 * The component is pure UI: it renders items at the cursor, dismisses on the
 * standard gestures, and lets the parent own what each item does. Focus here
 * is those dismissal behaviors — the menu must never get stuck open.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ContextMenu } from "./ContextMenu";

function renderMenu(overrides: { onClose?: () => void } = {}) {
  const onClose = overrides.onClose ?? vi.fn();
  const onSelect = vi.fn();
  render(
    <ContextMenu
      x={100}
      y={100}
      onClose={onClose}
      items={[{ label: "Open in browser", onSelect }]}
    />,
  );
  return { onClose, onSelect };
}

describe("ContextMenu", () => {
  it("renders its items as menu items", () => {
    renderMenu();
    expect(
      screen.getByRole("menuitem", { name: "Open in browser" }),
    ).toBeInTheDocument();
  });

  it("runs the item's action when clicked", () => {
    const { onSelect } = renderMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Open in browser" }));
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", () => {
    const { onClose } = renderMenu();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on a pointer down outside the menu", () => {
    const { onClose } = renderMenu();
    fireEvent.pointerDown(document.body);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("stays open on a pointer down inside the menu", () => {
    const { onClose } = renderMenu();
    fireEvent.pointerDown(screen.getByRole("menuitem", { name: "Open in browser" }));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes when the viewport scrolls", () => {
    const { onClose } = renderMenu();
    fireEvent.scroll(window);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("clamps its position to the viewport", () => {
    render(
      <ContextMenu
        x={5000}
        y={5000}
        onClose={vi.fn()}
        items={[{ label: "x", onSelect: vi.fn() }]}
      />,
    );
    const menu = screen.getByRole("menu");
    expect(parseInt(menu.style.left, 10)).toBeLessThan(window.innerWidth);
    expect(parseInt(menu.style.top, 10)).toBeLessThan(window.innerHeight);
    // And actually moved off the requested position, not just "some number".
    expect(parseInt(menu.style.left, 10)).toBeLessThan(5000);
  });
});

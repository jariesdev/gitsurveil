/**
 * Regression test for the white-screen bug: a view that throws during render
 * must show the boundary's fallback, never unmount the whole window.
 */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ViewErrorBoundary } from "./ErrorBoundary";

function ExplodingView(): never {
  throw new TypeError("payload shape mismatch");
}

describe("ViewErrorBoundary", () => {
  it("renders the fallback instead of unmounting the tree", () => {
    render(
      <ViewErrorBoundary onReset={() => {}}>
        <ExplodingView />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("This view failed to render.")).toBeTruthy();
    expect(screen.getByText(/payload shape mismatch/)).toBeTruthy();
  });

  it("clears the error and re-renders children via the reset button", async () => {
    const user = userEvent.setup();
    const onReset = vi.fn();
    const { rerender } = render(
      <ViewErrorBoundary onReset={onReset}>
        <ExplodingView />
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("This view failed to render.")).toBeTruthy();

    rerender(
      <ViewErrorBoundary onReset={onReset}>
        <div>healthy view</div>
      </ViewErrorBoundary>,
    );
    expect(screen.getByText("This view failed to render.")).toBeTruthy();

    await user.click(screen.getByText("Back to dashboard"));
    expect(onReset).toHaveBeenCalledOnce();
    expect(screen.getByText("healthy view")).toBeTruthy();
    expect(screen.queryByText("This view failed to render.")).toBeNull();
  });
});

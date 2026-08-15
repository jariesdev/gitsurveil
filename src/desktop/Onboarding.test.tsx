/**
 * First-run onboarding screen (`specs/desktop-ui.md` § Onboarding): renders
 * the pitch and the shared add-account form, links to the GitHub token page,
 * and exposes skip for the App to hide it for the session.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Onboarding } from "./Onboarding";
import { addAccount, openUrl } from "../ipc";

vi.mock("../ipc", () => ({
  addAccount: vi.fn(),
  openUrl: vi.fn(),
}));

describe("Onboarding", () => {
  it("renders the pitch and the add-account form", () => {
    render(<Onboarding onAdded={() => {}} onSkip={() => {}} />);

    expect(
      screen.getByRole("heading", { name: "Welcome to GitSurveil" }),
    ).toBeTruthy();
    expect(screen.getByLabelText("Provider")).toBeTruthy();
    expect(screen.getByLabelText(/Personal access token/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add account" })).toBeTruthy();
  });

  it("links to the GitHub token page from the helper", () => {
    render(<Onboarding onAdded={() => {}} onSkip={() => {}} />);

    fireEvent.click(screen.getByText("Where do I get a token?"));
    fireEvent.click(
      screen.getByRole("button", { name: "Create a token on GitHub" }),
    );

    expect(openUrl).toHaveBeenCalledWith("https://github.com/settings/tokens");
  });

  it("adds an account and reports it to the App", async () => {
    vi.mocked(addAccount).mockResolvedValue({
      id: "acc-1",
      host: "github.com",
      api_base: "https://api.github.com",
      login: "alice",
      auth_kind: "pat",
    });
    const onAdded = vi.fn();
    render(<Onboarding onAdded={onAdded} onSkip={() => {}} />);

    fireEvent.change(screen.getByLabelText(/Personal access token/), {
      target: { value: "ghp_123" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    await vi.waitFor(() =>
      expect(addAccount).toHaveBeenCalledWith("github.com", "ghp_123", undefined),
    );
    expect(onAdded).toHaveBeenCalled();
  });

  it("surfaces the daemon's validation error", async () => {
    vi.mocked(addAccount).mockRejectedValue(new Error("bad credentials"));
    render(<Onboarding onAdded={() => {}} onSkip={() => {}} />);

    fireEvent.change(screen.getByLabelText(/Personal access token/), {
      target: { value: "ghp_123" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "bad credentials",
    );
  });

  it("hides via Skip for now", () => {
    const onSkip = vi.fn();
    render(<Onboarding onAdded={() => {}} onSkip={onSkip} />);

    fireEvent.click(screen.getByRole("button", { name: "Skip for now" }));

    expect(onSkip).toHaveBeenCalled();
  });
});

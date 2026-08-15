/**
 * Tests the per-account repo checklist: toggling a checkbox calls
 * `reposSetNotify` and refreshes, without touching clone tracking.
 */

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Accounts } from "./Accounts";
import { addAccount, openUrl, reposSetNotify } from "../ipc";
import type { AccountRef, RepoCatalog, Repository } from "../types";

vi.mock("../ipc", () => ({
  addAccount: vi.fn(),
  removeAccount: vi.fn(),
  reposSetNotify: vi.fn().mockResolvedValue({}),
  openUrl: vi.fn(),
}));

const account: AccountRef = {
  id: "acc-1",
  host: "github.com",
  api_base: "https://api.github.com",
  login: "alice",
  auth_kind: "pat",
};

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

const catalog: RepoCatalog = {
  orgs: [{ account_id: "acc-1", host: "github.com", name: "acme" }],
  repos: [repository(), repository({ name: "web", full_name: "acme/web", notify_enabled: false })],
};

describe("Accounts notify checklist", () => {
  it("shows the account's repos with their current notify state", () => {
    render(<Accounts accounts={[account]} catalog={catalog} onChange={() => {}} />);
    fireEvent.click(screen.getByText(/Notify me about/));

    const enabled = screen.getByLabelText("acme/api") as HTMLInputElement;
    const disabled = screen.getByLabelText("acme/web") as HTMLInputElement;
    expect(enabled.checked).toBe(true);
    expect(disabled.checked).toBe(false);
  });

  it("toggling a checkbox calls reposSetNotify and refreshes", async () => {
    const onChange = vi.fn();
    render(<Accounts accounts={[account]} catalog={catalog} onChange={onChange} />);
    fireEvent.click(screen.getByText(/Notify me about/));

    fireEvent.click(screen.getByLabelText("acme/web"));

    expect(reposSetNotify).toHaveBeenCalledWith("acc-1", "acme/web", true);
    await vi.waitFor(() => expect(onChange).toHaveBeenCalled());
  });

  it("omits the checklist for an account with no discovered repos", () => {
    const other: AccountRef = { ...account, id: "acc-2", login: "bob" };
    render(<Accounts accounts={[other]} catalog={catalog} onChange={() => {}} />);
    expect(screen.queryByText(/Notify me about/)).toBeNull();
  });
});

describe("Accounts add form", () => {
  it("submits github.com by default with the GitHub provider", async () => {
    const onChange = vi.fn();
    render(<Accounts accounts={[]} catalog={catalog} onChange={onChange} />);

    // The Enterprise-only fields are hidden for the GitHub provider.
    expect(screen.queryByLabelText("Enterprise host")).toBeNull();
    expect(screen.queryByLabelText("API base URL")).toBeNull();

    fireEvent.change(screen.getByLabelText(/Personal access token/), {
      target: { value: "ghp_123" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    await vi.waitFor(() =>
      expect(addAccount).toHaveBeenCalledWith("github.com", "ghp_123", undefined),
    );
    expect(onChange).toHaveBeenCalled();
  });

  it("reveals Enterprise host and API base fields when Enterprise is selected", () => {
    render(<Accounts accounts={[]} catalog={catalog} onChange={() => {}} />);

    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "enterprise" },
    });

    expect(screen.getByLabelText("Enterprise host")).toBeTruthy();
    expect(screen.getByLabelText("API base URL")).toBeTruthy();
  });

  it("submits the Enterprise host and API base when Enterprise is selected", async () => {
    const onChange = vi.fn();
    render(<Accounts accounts={[]} catalog={catalog} onChange={onChange} />);

    fireEvent.change(screen.getByLabelText("Provider"), {
      target: { value: "enterprise" },
    });
    fireEvent.change(screen.getByLabelText("Enterprise host"), {
      target: { value: "github.example.com" },
    });
    fireEvent.change(screen.getByLabelText("API base URL"), {
      target: { value: "https://github.example.com/api/v3" },
    });
    fireEvent.change(screen.getByLabelText(/Personal access token/), {
      target: { value: "ghp_123" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add account" }));

    await vi.waitFor(() =>
      expect(addAccount).toHaveBeenCalledWith(
        "github.example.com",
        "ghp_123",
        "https://github.example.com/api/v3",
      ),
    );
    expect(onChange).toHaveBeenCalled();
  });
});

describe("Accounts token helper", () => {
  it("reveals scopes and the keychain note when expanded", () => {
    render(<Accounts accounts={[]} catalog={catalog} onChange={() => {}} />);

    fireEvent.click(screen.getByText("Where do I get a token?"));

    expect(screen.getAllByText(/notifications/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/repo/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/OS keychain/).length).toBeGreaterThan(0);
  });

  it("opens the GitHub token page from the helper", () => {
    render(<Accounts accounts={[]} catalog={catalog} onChange={() => {}} />);

    fireEvent.click(screen.getByText("Where do I get a token?"));
    fireEvent.click(screen.getByRole("button", { name: "Create a token on GitHub" }));

    expect(openUrl).toHaveBeenCalledWith("https://github.com/settings/tokens");
  });
});

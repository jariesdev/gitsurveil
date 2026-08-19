import { describe, expect, it } from "vitest";
import { returnedChanges } from "./returnedChanges";
import type { ActionItem, Comment, Conversation } from "../types";

function item(overrides: Partial<ActionItem> = {}): Pick<
  ActionItem,
  "dismissed_updated_at" | "dismissed_at" | "dismissed_ci_status" | "ci_status"
> {
  return {
    dismissed_updated_at: "2026-08-01T12:00:00Z",
    dismissed_at: "2026-08-01T15:00:00Z",
    dismissed_ci_status: "passing",
    ci_status: "passing",
    ...overrides,
  };
}

function comment(overrides: Partial<Comment> = {}): Comment {
  return {
    id: 1,
    author: "ana",
    body: "looks good",
    created_at: "2026-08-01T00:00:00Z",
    path: null,
    ...overrides,
  };
}

function conversation(overrides: Partial<Conversation> = {}): Conversation {
  return { issue_comments: [], review_threads: [], ...overrides };
}

describe("returnedChanges", () => {
  it("is null when the item was never dismissed", () => {
    expect(
      returnedChanges(item({ dismissed_updated_at: null, dismissed_at: null }), conversation()),
    ).toBeNull();
  });

  it("splits issue comments by the dismissal watermark", () => {
    const old = comment({ id: 1, created_at: "2026-08-01T00:00:00Z" });
    const fresh = comment({ id: 2, created_at: "2026-08-01T13:00:00Z" });
    const result = returnedChanges(item(), conversation({ issue_comments: [old, fresh] }));
    expect(result?.newIssueComments).toEqual([fresh]);
  });

  it("flags a thread whose newest reply arrived after the watermark", () => {
    const result = returnedChanges(
      item(),
      conversation({
        review_threads: [
          {
            id: "t1",
            path: "src/main.rs",
            resolved: false,
            comments: [
              comment({ id: 1, created_at: "2026-08-01T00:00:00Z" }),
              comment({ id: 2, created_at: "2026-08-01T13:00:00Z" }),
            ],
          },
          {
            id: "t2",
            path: "src/lib.rs",
            resolved: true,
            comments: [comment({ id: 3, created_at: "2026-08-01T00:00:00Z" })],
          },
        ],
      }),
    );
    expect(result?.threadsWithNewReplies.map((t) => t.id)).toEqual(["t1"]);
  });

  it("flags a CI pass-to-fail transition", () => {
    const result = returnedChanges(
      item({ dismissed_ci_status: "passing", ci_status: "failing" }),
      conversation(),
    );
    expect(result?.ciFlippedToFailing).toBe(true);
  });

  it("does not flag CI that was already failing at dismissal", () => {
    const result = returnedChanges(
      item({ dismissed_ci_status: "failing", ci_status: "failing" }),
      conversation(),
    );
    expect(result?.ciFlippedToFailing).toBe(false);
  });

  it("reports nothingNameable when no change is nameable", () => {
    const result = returnedChanges(item(), conversation());
    expect(result?.nothingNameable).toBe(true);
  });

  it("reports nothingNameable false when any change is nameable", () => {
    const result = returnedChanges(
      item({ dismissed_ci_status: "passing", ci_status: "failing" }),
      conversation(),
    );
    expect(result?.nothingNameable).toBe(false);
  });
});

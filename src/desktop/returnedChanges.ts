/**
 * "What changed since you dismissed this" (`specs/desktop-ui.md`,
 * `specs/github-integration.md` § Dismissal watermark).
 *
 * A dismissed item resurfaces the moment GitHub reports any `updated_at`
 * advance, which can be as trivial as a label edit — the returned item alone
 * gives no hint why. `ActionItem.dismissed_updated_at` is the item's own
 * `updated_at` at dismissal time, a GitHub timestamp, so every comment or
 * thread reply whose `created_at` is newer is provably something the user has
 * never seen. `dismissed_ci_status` is stored separately because `Check`
 * carries no timestamp of its own — the current status alone can't reveal a
 * pass→fail transition.
 *
 * Deliberately out of scope: commits (never fetched into `Conversation`) and
 * reviewer state flips (the daemon holds no reviewer snapshot at dismissal
 * time — `items.dismiss` only ever sees the item id).
 */

import type { ActionItem, CiStatus, Comment, Conversation, ReviewThread } from "../types";

export interface ReturnedChanges {
  /** Local dismissal time, for display only ("dismissed 3h ago"). */
  dismissedAt: string;
  newIssueComments: Comment[];
  threadsWithNewReplies: ReviewThread[];
  ciFlippedToFailing: boolean;
  /** True when every field above is empty/false — drives the honest fallback
   * banner rather than a block of empty bullet points. */
  nothingNameable: boolean;
}

/**
 * `null` when the item was never dismissed (or was restored via History,
 * which clears the watermark) — there is nothing to explain in that case.
 */
export function returnedChanges(
  item: Pick<ActionItem, "dismissed_updated_at" | "dismissed_at" | "dismissed_ci_status" | "ci_status">,
  conversation: Conversation,
): ReturnedChanges | null {
  const watermark = item.dismissed_updated_at;
  if (watermark === null || item.dismissed_at === null) return null;

  const isNew = (comment: Comment) => comment.created_at > watermark;

  const newIssueComments = conversation.issue_comments.filter(isNew);
  const threadsWithNewReplies = conversation.review_threads.filter((thread) =>
    thread.comments.some(isNew),
  );
  const ciFlippedToFailing = wasNotFailing(item.dismissed_ci_status) && item.ci_status === "failing";

  return {
    dismissedAt: item.dismissed_at,
    newIssueComments,
    threadsWithNewReplies,
    ciFlippedToFailing,
    nothingNameable:
      newIssueComments.length === 0 && threadsWithNewReplies.length === 0 && !ciFlippedToFailing,
  };
}

function wasNotFailing(status: CiStatus | null): boolean {
  return status !== null && status !== "failing";
}

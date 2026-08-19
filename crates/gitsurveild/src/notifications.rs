//! Desktop notification dispatch (`specs/notifications.md`). Fired by the
//! daemon itself via `notify-rust` so alerts work with zero UI processes
//! running.
//!
//! *Whether* to fire is decided by the priority engine's gate
//! ([`crate::priority::should_notify`]); this module owns only the "what does
//! the user actually see" half — content, and collapsing a burst into one
//! message.

use gitsurveil_proto::ScoredItem;
use notify_rust::Notification;

/// Above this many notifications in one poll, send a single summary instead.
/// A catch-up poll after time offline can otherwise fire a dozen at once,
/// which is worse than useless — the user dismisses the stack unread.
const BURST_COLLAPSE_THRESHOLD: usize = 3;

/// Sends notifications for `items`, which the caller has already filtered
/// through the gate and sorted most-urgent first.
///
/// Failures are logged, not propagated: a broken notification backend must
/// never take down the poll loop that's trying to report other problems.
pub fn dispatch_batch(items: &[ScoredItem]) {
    let Some(top) = items.first() else {
        return;
    };

    if items.len() > BURST_COLLAPSE_THRESHOLD {
        send(
            &format!("{} new items", items.len()),
            &format!("Highest priority: {}", describe(top)),
        );
        return;
    }

    for scored in items {
        send(&describe(scored), &scored.item.title);
    }
}

/// One-line identification of an item: what kind it is and where it lives.
fn describe(scored: &ScoredItem) -> String {
    let location = match scored.item.number {
        Some(number) => format!("{}#{}", scored.item.repo, number),
        None => scored.item.repo.clone(),
    };
    format!("{} · {}", kind_label(scored), location)
}

fn kind_label(scored: &ScoredItem) -> &'static str {
    use gitsurveil_proto::ItemKind::*;
    match scored.item.kind {
        ReviewRequested => "Review requested",
        Assigned => "Assigned",
        Mentioned => "Mentioned",
        Participating => "Participating",
        CiFailed => "CI failed",
        ReviewStateChanged => "Changes requested",
        ReadyToMerge => "Ready to merge",
        Authored => "Your PR",
        ReviewedByMe => "PR you reviewed",
    }
}

fn send(summary: &str, body: &str) {
    if let Err(e) = Notification::new().summary(summary).body(body).show() {
        tracing::warn!("failed to send desktop notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gitsurveil_proto::{ActionItem, CiStatus, ItemKind, ItemState};

    fn scored(kind: ItemKind, repo: &str, number: Option<u64>) -> ScoredItem {
        let item = ActionItem {
            id: "a".into(),
            account_id: "acc".into(),
            kind,
            state: ItemState::Open,
            repo: repo.into(),
            number,
            title: "Fix the thing".into(),
            url: "u".into(),
            author: "someone".into(),
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            first_seen_at: "2026-08-13T12:00:00Z".into(),
            last_seen_at: "2026-08-13T12:00:00Z".into(),
            ci_status: CiStatus::None,
            raw_kind: "x".into(),
            dismissed_updated_at: None,
            dismissed_at: None,
            dismissed_ci_status: None,
            activity: None,
            archived: false,
        };
        crate::priority::score_item(&item, &[], Utc::now())
    }

    #[test]
    fn describes_an_item_by_kind_and_location() {
        assert_eq!(
            describe(&scored(ItemKind::ReviewRequested, "acme/api", Some(482))),
            "Review requested · acme/api#482"
        );
    }

    #[test]
    fn omits_the_number_when_an_item_has_none() {
        // Some notification threads carry no PR/issue number; the label must
        // not read "acme/api#" with a dangling separator.
        assert_eq!(
            describe(&scored(ItemKind::Mentioned, "acme/api", None)),
            "Mentioned · acme/api"
        );
    }
}

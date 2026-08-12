//! Desktop notification dispatch (`specs/notifications.md`). Fired by the
//! daemon itself via `notify-rust` so alerts work with zero UI processes
//! running — the whole point of Phase 2.
//!
//! The fire/no-fire decision here is deliberately simpler than the final
//! design: `specs/notifications.md`'s full trigger table depends on the
//! priority engine's outrank gate, which doesn't exist until Phase 4. Until
//! then, [`should_notify`] implements the two trigger rows that don't need
//! it — a brand new item, and a CI pass→fail transition on an item we
//! already knew about — and treats every carried-over item as silent,
//! which is the one invariant that must hold from day one (a carried item
//! must never re-notify).

use gitsurveil_proto::{ActionItem, CiStatus};
use notify_rust::Notification;

use crate::github::diff::ChangeKind;

/// Decides whether `item` (compared against `prev`, its previous stored
/// state, when one exists) should produce a desktop notification this poll
/// cycle. Pure and exhaustively tested — the transport (`dispatch_batch`)
/// trusts this completely and does no filtering of its own.
pub fn should_notify(change: ChangeKind, prev: Option<&ActionItem>, item: &ActionItem) -> bool {
    match change {
        ChangeKind::New => true,
        ChangeKind::Carried => false,
        ChangeKind::Updated => {
            // The one "Updated" trigger that doesn't need the priority
            // engine's gate: CI flipping to failing is unambiguously worth
            // an interruption regardless of severity rules.
            item.ci_status == CiStatus::Failing
                && prev.map(|p| p.ci_status) != Some(CiStatus::Failing)
        }
    }
}

/// Sends one native notification per item in `items`, or — when there are
/// more than three — a single collapsed summary naming the count and the
/// first item, per `specs/notifications.md`'s "burst collapse" rule (a poll
/// after a period offline can otherwise fire a dozen notifications at once).
///
/// Failures are logged, not propagated: a broken notification backend must
/// never take down the poll loop that's trying to report *other* problems.
pub fn dispatch_batch(items: &[ActionItem]) {
    if items.is_empty() {
        return;
    }
    if items.len() > 3 {
        send(
            &format!("{} new items", items.len()),
            &format!(
                "Highest priority: {} {}#{}",
                describe_kind(&items[0]),
                items[0].repo,
                items[0].number.map(|n| n.to_string()).unwrap_or_default()
            ),
        );
        return;
    }
    for item in items {
        send(
            &format!("{} · {}#{}", describe_kind(item), item.repo, item.number.map(|n| n.to_string()).unwrap_or_default()),
            &item.title,
        );
    }
}

fn describe_kind(item: &ActionItem) -> &'static str {
    use gitsurveil_proto::ItemKind::*;
    match item.kind {
        ReviewRequested => "Review requested",
        Assigned => "Assigned",
        Mentioned => "Mentioned",
        Participating => "Participating",
        CiFailed => "CI failed",
        ReviewStateChanged => "Changes requested",
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
    use gitsurveil_proto::{ItemKind, ItemState};

    fn item(ci_status: CiStatus) -> ActionItem {
        ActionItem {
            id: "a".into(),
            account_id: "acc-1".into(),
            kind: ItemKind::ReviewRequested,
            state: ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "https://github.com/acme/api/pull/1".into(),
            author: "a".into(),
            created_at: "t1".into(),
            updated_at: "t1".into(),
            first_seen_at: "t1".into(),
            last_seen_at: "t1".into(),
            ci_status,
            raw_kind: "review_requested".into(),
        }
    }

    #[test]
    fn new_item_always_notifies() {
        assert!(should_notify(ChangeKind::New, None, &item(CiStatus::None)));
    }

    #[test]
    fn carried_item_never_notifies_regardless_of_ci_status() {
        let prev = item(CiStatus::Passing);
        let now = item(CiStatus::Failing);
        // Carried means the diff already decided nothing meaningful changed;
        // this must hold even if ci_status somehow differs, since a carried
        // classification is a promise the notification gate depends on.
        assert!(!should_notify(ChangeKind::Carried, Some(&prev), &now));
    }

    #[test]
    fn updated_item_notifies_on_ci_pass_to_fail_transition() {
        let prev = item(CiStatus::Passing);
        let now = item(CiStatus::Failing);
        assert!(should_notify(ChangeKind::Updated, Some(&prev), &now));
    }

    #[test]
    fn updated_item_does_not_notify_when_still_failing() {
        let prev = item(CiStatus::Failing);
        let now = item(CiStatus::Failing);
        assert!(!should_notify(ChangeKind::Updated, Some(&prev), &now));
    }

    #[test]
    fn updated_item_does_not_notify_on_unrelated_change() {
        let prev = item(CiStatus::None);
        let now = item(CiStatus::None);
        assert!(!should_notify(ChangeKind::Updated, Some(&prev), &now));
    }

    #[test]
    fn updated_item_with_no_prior_record_does_not_crash_on_ci_check() {
        // Defensive: Updated should never occur with prev=None in practice
        // (diff only marks Updated when a previous record exists), but the
        // function must not panic if it ever does.
        assert!(!should_notify(ChangeKind::Updated, None, &item(CiStatus::Passing)));
    }

    /// Not run by default (`cargo test` shouldn't spam the desktop with real
    /// notifications). Run explicitly with
    /// `cargo test -p gitsurveild live_notification -- --ignored --nocapture`
    /// to confirm this unbundled binary can actually post to the OS —
    /// exactly the open question `specs/notifications.md` flags for macOS.
    #[test]
    #[ignore]
    fn live_notification_smoke_test() {
        dispatch_batch(&[item(CiStatus::Failing)]);
    }
}

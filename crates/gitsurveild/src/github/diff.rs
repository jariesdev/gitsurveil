//! Pure diff/dedup logic between a stored snapshot and a freshly fetched one
//! (`specs/github-integration.md`, "Diff semantics"). No I/O — this is the
//! easiest part of the daemon to get exhaustively right with tests, so it
//! stays a plain function over plain data.

use std::collections::HashMap;

use gitsurveil_proto::ActionItem;

/// How a fetched item compares to what was previously stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Not seen in a previous poll — a candidate for notification (the
    /// actual notify/don't-notify decision is the priority engine's gate,
    /// Phase 4).
    New,
    /// Seen before, but GitHub's `updated_at` advanced (new commits,
    /// comments, or a CI status flip).
    Updated,
    /// Seen before, unchanged. Must never re-trigger a notification.
    Carried,
}

/// The result of comparing a previous snapshot to a freshly fetched one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diff {
    /// Items present in the fetched snapshot, each tagged with how they
    /// compare to what was stored. Callers upsert all of these.
    pub changes: Vec<(ChangeKind, ActionItem)>,
    /// Ids present in the previous snapshot but absent from the fetched one
    /// — resolved upstream (closed/merged). Callers mark these `Done`.
    pub resolved_ids: Vec<String>,
}

/// Compares `previous` (what's stored for this account) against `fetched`
/// (this poll's normalized results) and classifies every item.
///
/// Dismissed items are intentionally not treated specially here: if GitHub
/// still returns a dismissed item with an unchanged `updated_at`, it's
/// `Carried` and stays dismissed; if `updated_at` has advanced, it's
/// `Updated`, which the caller uses to resurrect it (`specs/github-integration.md`,
/// "Dismissed items stay dismissed... unless activity resurrects them").
pub fn diff(previous: &[ActionItem], fetched: &[ActionItem]) -> Diff {
    let previous_by_id: HashMap<&str, &ActionItem> =
        previous.iter().map(|i| (i.id.as_str(), i)).collect();

    let mut changes = Vec::with_capacity(fetched.len());
    for item in fetched {
        let kind = match previous_by_id.get(item.id.as_str()) {
            None => ChangeKind::New,
            Some(prev) if prev.updated_at != item.updated_at => ChangeKind::Updated,
            Some(_) => ChangeKind::Carried,
        };
        changes.push((kind, item.clone()));
    }

    let fetched_ids: std::collections::HashSet<&str> =
        fetched.iter().map(|i| i.id.as_str()).collect();
    let resolved_ids = previous
        .iter()
        .filter(|i| !fetched_ids.contains(i.id.as_str()))
        .map(|i| i.id.clone())
        .collect();

    Diff {
        changes,
        resolved_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitsurveil_proto::{CiStatus, ItemKind, ItemState};

    fn item(id: &str, updated_at: &str) -> ActionItem {
        ActionItem {
            id: id.into(),
            account_id: "acc-1".into(),
            kind: ItemKind::ReviewRequested,
            state: ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "https://github.com/acme/api/pull/1".into(),
            author: "a".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: updated_at.into(),
            first_seen_at: "2026-08-01T00:00:00Z".into(),
            last_seen_at: "2026-08-01T00:00:00Z".into(),
            ci_status: CiStatus::None,
            raw_kind: "review_requested".into(),
            activity: None,
            archived: false,
        }
    }

    #[test]
    fn empty_previous_marks_everything_new() {
        let d = diff(&[], &[item("a", "t1"), item("b", "t1")]);
        assert_eq!(d.changes.len(), 2);
        assert!(d.changes.iter().all(|(k, _)| *k == ChangeKind::New));
        assert!(d.resolved_ids.is_empty());
    }

    #[test]
    fn unchanged_item_is_carried() {
        let prev = vec![item("a", "t1")];
        let fetched = vec![item("a", "t1")];
        let d = diff(&prev, &fetched);
        assert_eq!(d.changes, vec![(ChangeKind::Carried, item("a", "t1"))]);
    }

    #[test]
    fn advanced_updated_at_is_updated_not_new() {
        let prev = vec![item("a", "t1")];
        let fetched = vec![item("a", "t2")];
        let d = diff(&prev, &fetched);
        assert_eq!(d.changes, vec![(ChangeKind::Updated, item("a", "t2"))]);
    }

    #[test]
    fn missing_from_fetched_is_resolved() {
        let prev = vec![item("a", "t1"), item("b", "t1")];
        let fetched = vec![item("a", "t1")];
        let d = diff(&prev, &fetched);
        assert_eq!(d.resolved_ids, vec!["b".to_string()]);
    }

    #[test]
    fn a_carried_item_never_reappears_as_resolved_or_new() {
        // Guards the exact behavior the notification gate depends on:
        // polling the same unchanged state twice must be a total no-op.
        let prev = vec![item("a", "t1")];
        let d1 = diff(&prev, &prev.clone());
        assert_eq!(d1.changes, vec![(ChangeKind::Carried, item("a", "t1"))]);
        assert!(d1.resolved_ids.is_empty());
    }
}

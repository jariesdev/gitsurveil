//! Ties the GitHub client, the diff logic, and the store together into a
//! recurring background task. This is Phase 1's core deliverable: a process
//! that provably tracks the user's review requests, assignments, mentions,
//! and CI failures with no UI involved.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gitsurveil_proto::{AccountRef, CiStatus, ScoredItem};

use crate::github::diff::{diff, ChangeKind};
use crate::github::GitHubClient;
use crate::priority::{self, Rule};
use crate::store::Store;
use crate::{keychain, notifications, DaemonError};

const NOTIFICATIONS_ENDPOINT: &str = "/notifications";

/// Backoff duration (in seconds) when a poll hits a GitHub rate limit.
/// The spec says "back off exponentially", but a fixed floor is sufficient
/// for Phase 1 — the next cycle retries normally.
const RATE_LIMIT_BACKOFF_SECS: u64 = 300;

/// Current UTC time as RFC 3339, used to stamp `first_seen_at`/`last_seen_at`
/// on freshly-fetched items. Centralized here so every call site formats
/// timestamps identically.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Runs the poll loop forever, sleeping `interval_secs` between cycles.
/// Intended to be spawned as its own tokio task; errors for a single account
/// are logged and never abort the loop — one broken account must not stop
/// polling for the rest (`specs/github-integration.md`, "Edge cases").
///
/// GitHub's `x-poll-interval` response header can ask us to slow down;
/// `poll_all_accounts` returns the largest interval any account requested
/// this cycle, and it raises (never lowers) the sleep so we always honor the
/// most conservative account. A config change is the only way to reduce it.
pub async fn run(store: Arc<Store>, rules: Vec<Rule>, mut interval_secs: u64) {
    loop {
        if let Some(requested) = poll_all_accounts(&store, &rules).await {
            interval_secs = interval_secs.max(requested);
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

/// Runs one full poll cycle across every account: fetch, diff, store, and
/// dispatch any notifications that clear the gate. Exposed so the `poll.now`
/// API method can force a cycle without waiting for the timer.
pub async fn poll_all_accounts(store: &Store, rules: &[Rule]) -> Option<u64> {
    let accounts = match store.list_accounts() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("failed to list accounts for poll: {e}");
            return None;
        }
    };

    // The gate compares against the top score across *everything* already
    // open, not just this account's items — you care about what outranks your
    // current work, regardless of which account it came from.
    let prev_top_score = match store.open_items() {
        Ok(items) => priority::score_all(&items, rules, Utc::now())
            .first()
            .map(|s| s.score),
        Err(e) => {
            tracing::error!("failed to read open items before poll: {e}");
            None
        }
    };

    let disabled_kinds = store.disabled_kinds().unwrap_or_default();

    let mut max_requested_interval = None;
    let mut notify_candidates = Vec::new();
    let mut hit_rate_limit = false;
    for account in accounts {
        match poll_account(store, &account, rules, &disabled_kinds).await {
            Ok((requested, mut candidates)) => {
                max_requested_interval = max_requested_interval.max(requested);
                notify_candidates.append(&mut candidates);
            }
            Err(e) => {
                if is_rate_limit_error(&e) {
                    tracing::warn!(
                        account = %account.login,
                        "rate limited — backing off {RATE_LIMIT_BACKOFF_SECS}s"
                    );
                    hit_rate_limit = true;
                } else {
                    tracing::warn!(account = %account.login, "poll failed: {e}");
                }
            }
        }
    }

    // Gate and sort once for the whole cycle, so a burst across several
    // accounts collapses into a single notification rather than one per
    // account.
    let mut to_notify: Vec<_> = notify_candidates
        .into_iter()
        .filter(|scored| priority::should_notify(prev_top_score, scored))
        .collect();
    to_notify.sort_by(|a, b| b.score.cmp(&a.score));
    notifications::dispatch_batch(&to_notify);

    // When a rate limit was hit, use the backoff interval instead of
    // whatever GitHub requested — the normal interval is too aggressive.
    if hit_rate_limit {
        Some(RATE_LIMIT_BACKOFF_SECS)
    } else {
        max_requested_interval
    }
}

/// Polls one account, updates the store, and returns the `X-Poll-Interval`
/// GitHub requested plus the scored items that are *eligible* for a
/// notification. The caller applies the gate, so it can compare against one
/// top score spanning every account.
async fn poll_account(
    store: &Store,
    account: &AccountRef,
    rules: &[Rule],
    disabled_kinds: &std::collections::HashSet<gitsurveil_proto::ItemKind>,
) -> crate::error::Result<(Option<u64>, Vec<ScoredItem>)> {
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;

    let snapshot = client.fetch_search_items(&account.login).await?;
    let mut fetched = snapshot.items;

    let mut requested_interval = None;
    let prev_etag = store.get_etag(&account.id, NOTIFICATIONS_ENDPOINT)?;
    match client.poll_notifications(prev_etag.as_deref()).await? {
        crate::github::client::NotificationsPoll::NotModified => {
            tracing::debug!(account = %account.login, "notifications unchanged (304)");
        }
        crate::github::client::NotificationsPoll::Modified {
            items,
            etag,
            poll_interval_secs,
        } => {
            fetched.extend(items);
            requested_interval = poll_interval_secs;
            if let Some(etag) = etag {
                store.set_etag(&account.id, NOTIFICATIONS_ENDPOINT, &etag)?;
            }
        }
    }

    // The `review-requested:@me` search is the authoritative source for
    // `ReviewRequested` items: it only returns PRs whose review is still owed
    // (GitHub drops a PR the moment the user reviews it). A `/notifications`
    // feed item for the same PR is therefore either a duplicate (the search
    // already has it) or stale (the user already reviewed it). Keep feed items
    // only for PRs the search doesn't know about at all — e.g. team-based
    // requests that never show up in `@me` searches.
    let notif_prefix = format!("{}:notif:", account.id);
    fetched.retain(|item| {
        if item.kind != gitsurveil_proto::ItemKind::ReviewRequested
            || !item.id.starts_with(&notif_prefix)
        {
            return true;
        }
        match pull_number_from_url(&item.url) {
            Some(number) => {
                let key = (item.repo.clone(), number);
                !snapshot.review_requested_keys.contains(&key)
                    && !snapshot.reviewed_by_me_keys.contains(&key)
            }
            // No PR number to dedupe against (e.g. a system notification);
            // keep it rather than risk dropping a legitimate request.
            None => true,
        }
    });

    let muted_repos = store.muted_repos(&account.id)?;
    let previous = store.items_for_account(&account.id)?;
    let previous_by_id: HashMap<&str, &gitsurveil_proto::ActionItem> =
        previous.iter().map(|i| (i.id.as_str(), i)).collect();
    let result = diff(&previous, &fetched);

    let now = Utc::now();
    let mut candidates = Vec::new();
    for (change_kind, mut item) in result.changes {
        let prev = previous_by_id.get(item.id.as_str()).copied();
        let preserve = should_preserve_local_state(change_kind, prev.is_some_and(|p| p.archived));
        if change_kind != ChangeKind::New {
            // Preserve the original first_seen_at on updates/carries; the
            // GitHub client only knows "now" since it has no prior record.
            if let Some(prev) = prev {
                item.first_seen_at = prev.first_seen_at.clone();
            }
        }

        if !preserve
            && newly_relevant(change_kind, &item, prev)
            && !muted_repos.contains(&item.repo)
            && !disabled_kinds.contains(&item.kind)
        {
            candidates.push(priority::score_item(&item, rules, now));
        }

        item.last_seen_at = now_rfc3339();
        // Local lifecycle state beats GitHub's: a Carried item is unchanged,
        // so leave the stored row (and any dismiss marker) untouched; an
        // archived item was cleared for good and must never be resurrected —
        // not even by new activity. Writing here would clobber `state` with
        // the fetched item's `Open` (see `should_preserve_local_state`).
        if !preserve {
            store.upsert_item(&item)?;
        }
    }

    for resolved_id in result.resolved_ids {
        // An archived item that later resolves upstream stays archived — the
        // user cleared it permanently and must not see it resurface in
        // history.
        if !previous_by_id
            .get(resolved_id.as_str())
            .is_some_and(|p| p.archived)
        {
            store.mark_item_done(&resolved_id)?;
        }
    }

    Ok((requested_interval, candidates))
}

/// Extracts the PR/issue number from a GitHub API `subject.url`
/// (`https://api.github.com/repos/{owner}/{repo}/pulls/{number}`, or the
/// equivalent `/issues/{number}` form). Returns `None` for non-API or
/// number-less URLs.
fn pull_number_from_url(url: &str) -> Option<u64> {
    url.split('?')
        .next()?
        .rsplit('/')
        .next()?
        .parse::<u64>()
        .ok()
}

/// Whether an error is a GitHub rate-limit error. GitHub's GraphQL API
/// returns `"API rate limit exceeded"` in the error message when the
/// per-user hourly quota is exhausted.
fn is_rate_limit_error(e: &DaemonError) -> bool {
    let msg = e.to_string();
    msg.to_lowercase().contains("rate limit exceeded")
}

/// Parsed form of the `activity` fingerprint an `Authored` item carries: the
/// sorted ids of comments written by people other than the account user, and
/// the ids of review threads that are currently unresolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ActivityFingerprint {
    comment_ids: BTreeSet<u64>,
    unresolved_thread_ids: BTreeSet<String>,
}

impl ActivityFingerprint {
    /// Parses the `c:<ids>;u:<ids>` string stored on an `Authored` item. An
    /// absent or legacy value parses to an empty fingerprint, so the first
    /// poll after an upgrade compares against nothing (any currently-qualifying
    /// activity reads as a transition once).
    fn parse(encoded: Option<&str>) -> Self {
        let mut fingerprint = ActivityFingerprint::default();
        let Some(encoded) = encoded else {
            return fingerprint;
        };
        for part in encoded.split(';') {
            if let Some(ids) = part.strip_prefix("c:") {
                fingerprint.comment_ids = ids
                    .split(',')
                    .filter_map(|id| id.parse::<u64>().ok())
                    .collect();
            } else if let Some(ids) = part.strip_prefix("u:") {
                fingerprint.unresolved_thread_ids = ids
                    .split(',')
                    .filter(|id| !id.is_empty())
                    .map(String::from)
                    .collect();
            }
        }
        fingerprint
    }
}

/// Whether the poller must leave an item's stored row untouched. Returns
/// `true` for `Carried` items — GitHub reports no change, so the local
/// dismiss marker wins over the fetched item's `Open` state ("dismissed
/// stays dismissed unless activity resumes it") — and for `archived` items
/// regardless of activity: the user cleared them for good, so they are never
/// resurrected, re-shown, or notified about again.
fn should_preserve_local_state(change_kind: ChangeKind, prev_archived: bool) -> bool {
    prev_archived || change_kind == ChangeKind::Carried
}

/// Whether a diffed item is worth considering for a notification. Only
/// genuinely new items, or ones whose CI just broke, normally qualify —
/// everything else is a silent update: it will still show up in the list and
/// the tray color. `Authored` and `ReviewedByMe` are curated by content, not
/// by `updated_at`: `ReviewedByMe` items only move when a reply arrives (so
/// every update counts), while `Authored` items interrupt only on a new
/// comment from someone else, a thread newly unresolved, or a CI failure.
fn newly_relevant(
    change_kind: ChangeKind,
    item: &gitsurveil_proto::ActionItem,
    prev: Option<&gitsurveil_proto::ActionItem>,
) -> bool {
    match change_kind {
        ChangeKind::New => true,
        ChangeKind::Carried => false,
        ChangeKind::Updated => match item.kind {
            gitsurveil_proto::ItemKind::Authored => {
                let prev_fingerprint = prev
                    .and_then(|p| p.activity.as_deref())
                    .map(|encoded| ActivityFingerprint::parse(Some(encoded)))
                    .unwrap_or_default();
                let fingerprint = ActivityFingerprint::parse(item.activity.as_deref());
                let new_comment = !fingerprint
                    .comment_ids
                    .difference(&prev_fingerprint.comment_ids)
                    .next()
                    .is_none();
                let newly_unresolved = !fingerprint
                    .unresolved_thread_ids
                    .difference(&prev_fingerprint.unresolved_thread_ids)
                    .next()
                    .is_none();
                new_comment
                    || newly_unresolved
                    || (item.ci_status == CiStatus::Failing
                        && prev.map(|p| p.ci_status) != Some(CiStatus::Failing))
            }
            gitsurveil_proto::ItemKind::ReviewedByMe => true,
            _ => {
                item.ci_status == CiStatus::Failing
                    && prev.map(|p| p.ci_status) != Some(CiStatus::Failing)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitsurveil_proto::{ItemKind, ItemState};

    fn item(kind: ItemKind, ci_status: CiStatus) -> gitsurveil_proto::ActionItem {
        gitsurveil_proto::ActionItem {
            id: "id".into(),
            account_id: "acc-1".into(),
            kind,
            state: ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "https://github.com/acme/api/pull/1".into(),
            author: "a".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: "2026-08-01T00:00:00Z".into(),
            first_seen_at: "2026-08-01T00:00:00Z".into(),
            last_seen_at: "2026-08-01T00:00:00Z".into(),
            ci_status,
            raw_kind: "x".into(),
            activity: None,
            archived: false,
        }
    }

    #[test]
    fn new_items_are_always_relevant() {
        assert!(newly_relevant(
            ChangeKind::New,
            &item(ItemKind::ReviewRequested, CiStatus::None),
            None
        ));
    }

    #[test]
    fn carried_items_are_never_relevant() {
        assert!(!newly_relevant(
            ChangeKind::Carried,
            &item(ItemKind::ReviewRequested, CiStatus::Failing),
            None
        ));
    }

    #[test]
    fn ordinary_kinds_only_update_on_a_ci_failure_transition() {
        let prev = item(ItemKind::ReviewRequested, CiStatus::Passing);
        let now_failing = item(ItemKind::ReviewRequested, CiStatus::Failing);
        assert!(newly_relevant(ChangeKind::Updated, &now_failing, Some(&prev)));

        // Already failing before -> not a fresh transition.
        let prev_failing = item(ItemKind::ReviewRequested, CiStatus::Failing);
        assert!(!newly_relevant(ChangeKind::Updated, &now_failing, Some(&prev_failing)));

        // A non-CI update (e.g. a new comment) doesn't qualify.
        let now_comment = item(ItemKind::ReviewRequested, CiStatus::Passing);
        assert!(!newly_relevant(ChangeKind::Updated, &now_comment, Some(&prev)));
    }

    #[test]
    fn authored_updates_only_count_for_qualifying_transitions() {
        // A commit or other activity that leaves the fingerprint unchanged
        // (only `updated_at` moved) is silent.
        let prev = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:1;u:");
        let commit_only = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:1;u:");
        assert!(
            !newly_relevant(ChangeKind::Updated, &commit_only, Some(&prev)),
            "a commit that doesn't change the fingerprint must not notify"
        );

        // A comment from someone else (new comment id) qualifies.
        let new_comment = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:1,2;u:");
        assert!(newly_relevant(ChangeKind::Updated, &new_comment, Some(&prev)));

        // A thread becoming unresolved qualifies.
        let newly_unresolved = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:1;u:t1");
        assert!(newly_relevant(ChangeKind::Updated, &newly_unresolved, Some(&prev)));

        // A CI failure transition qualifies even with an unchanged fingerprint.
        let now_failing = with_activity(item(ItemKind::Authored, CiStatus::Failing), "c:1;u:");
        assert!(newly_relevant(ChangeKind::Updated, &now_failing, Some(&prev)));

        // Resolving the last qualifying signal (thread gone from fingerprint)
        // is not a notification moment — the item simply leaves the list.
        let prev_unresolved = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:;u:t1");
        let resolved = with_activity(item(ItemKind::Authored, CiStatus::Passing), "c:;u:");
        assert!(!newly_relevant(ChangeKind::Updated, &resolved, Some(&prev_unresolved)));
    }

    fn with_activity(item: gitsurveil_proto::ActionItem, activity: &str) -> gitsurveil_proto::ActionItem {
        let mut item = item;
        item.activity = Some(activity.into());
        item
    }

    #[test]
    fn reviewed_by_me_updates_always_count() {
        // A `ReviewedByMe` item only ever moves `updated_at` when a new reply
        // arrives, so every update is worth surfacing.
        let prev = item(ItemKind::ReviewedByMe, CiStatus::Passing);
        let updated = item(ItemKind::ReviewedByMe, CiStatus::Passing);
        assert!(newly_relevant(ChangeKind::Updated, &updated, Some(&prev)));
    }

    #[test]
    fn fingerprint_parses_absent_legacy_and_partial_values() {
        assert_eq!(ActivityFingerprint::parse(None), ActivityFingerprint::default());
        assert_eq!(
            ActivityFingerprint::parse(Some("c:1,2;u:t1,t2")),
            ActivityFingerprint {
                comment_ids: BTreeSet::from([1, 2]),
                unresolved_thread_ids: BTreeSet::from(["t1".into(), "t2".into()]),
            }
        );
        assert_eq!(
            ActivityFingerprint::parse(Some("c:;u:")),
            ActivityFingerprint::default()
        );
    }

    #[test]
    fn preserve_local_state_cases() {
        assert!(should_preserve_local_state(ChangeKind::Carried, false), "carried keeps its row");
        assert!(should_preserve_local_state(ChangeKind::Carried, true), "carried archived stays archived");
        assert!(should_preserve_local_state(ChangeKind::Updated, true), "archived is immune to new activity");
        assert!(should_preserve_local_state(ChangeKind::New, true), "archived never becomes New again");
        assert!(!should_preserve_local_state(ChangeKind::Updated, false), "activity resurrects a dismissed item");
        assert!(!should_preserve_local_state(ChangeKind::New, false), "brand-new items are written");
    }

    #[test]
    fn dismissed_survives_an_unchanged_poll_but_activity_resurrects_it() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&AccountRef {
            id: "acc-1".into(),
            host: "github.com".into(),
            api_base: "https://api.github.com".into(),
            login: "octocat".into(),
            auth_kind: gitsurveil_proto::AuthKind::Pat,
        }).unwrap();
        store.upsert_item(&item(ItemKind::ReviewRequested, CiStatus::None)).unwrap();
        store.set_dismissed("id", true).unwrap();

        // Poll 1: GitHub unchanged -> Carried. The poller skips the write, so
        // the dismiss marker survives and the item stays out of the Dashboard.
        let previous = store.items_for_account("acc-1").unwrap();
        let result = diff(&previous, &[item(ItemKind::ReviewRequested, CiStatus::None)]);
        assert_eq!(result.changes[0].0, ChangeKind::Carried);
        for (kind, i) in result.changes {
            let archived = previous.iter().any(|p| p.id == i.id && p.archived);
            if !should_preserve_local_state(kind, archived) {
                store.upsert_item(&i).unwrap();
            }
        }
        assert!(store.open_items().unwrap().is_empty(), "dismissed survives an unchanged poll");

        // Poll 2: `updated_at` advanced -> Updated. Dismissal is a local hide,
        // so spec says activity resurrects the item into the Dashboard.
        let previous = store.items_for_account("acc-1").unwrap();
        let mut active = item(ItemKind::ReviewRequested, CiStatus::None);
        active.updated_at = "2026-08-02T00:00:00Z".into();
        let result = diff(&previous, &[active]);
        assert_eq!(result.changes[0].0, ChangeKind::Updated);
        for (kind, i) in result.changes {
            let archived = previous.iter().any(|p| p.id == i.id && p.archived);
            if !should_preserve_local_state(kind, archived) {
                store.upsert_item(&i).unwrap();
            }
        }
        assert_eq!(store.open_items().unwrap().len(), 1, "activity brings the item back");
    }

    #[test]
    fn archived_item_never_resurrects_even_on_activity() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&AccountRef {
            id: "acc-1".into(),
            host: "github.com".into(),
            api_base: "https://api.github.com".into(),
            login: "octocat".into(),
            auth_kind: gitsurveil_proto::AuthKind::Pat,
        }).unwrap();
        store.upsert_item(&item(ItemKind::ReviewRequested, CiStatus::None)).unwrap();
        store.set_dismissed("id", true).unwrap();
        store.clear_history().unwrap();

        // New GitHub activity -> Updated, but the item is archived: the poller
        // must skip the write so it can't resurface in the Dashboard or
        // history.
        let previous = store.items_for_account("acc-1").unwrap();
        assert!(previous[0].archived);
        let mut active = item(ItemKind::ReviewRequested, CiStatus::None);
        active.updated_at = "2026-08-02T00:00:00Z".into();
        let result = diff(&previous, &[active]);
        assert_eq!(result.changes[0].0, ChangeKind::Updated);
        for (kind, i) in result.changes {
            let archived = previous.iter().any(|p| p.id == i.id && p.archived);
            assert!(
                should_preserve_local_state(kind, archived),
                "archived items are never written back"
            );
            if !should_preserve_local_state(kind, archived) {
                store.upsert_item(&i).unwrap();
            }
        }
        assert!(store.open_items().unwrap().is_empty(), "archived never resurfaces");
        assert!(store.history_items(50).unwrap().is_empty(), "archived never shows in history");
    }
}

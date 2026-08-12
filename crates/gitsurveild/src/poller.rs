//! Ties the GitHub client, the diff logic, and the store together into a
//! recurring background task. This is Phase 1's core deliverable: a process
//! that provably tracks the user's review requests, assignments, mentions,
//! and CI failures with no UI involved.

use std::sync::Arc;
use std::time::Duration;

use gitsurveil_proto::AccountRef;

use crate::github::diff::{diff, ChangeKind};
use crate::github::GitHubClient;
use crate::store::Store;
use crate::{keychain, DaemonError};

const NOTIFICATIONS_ENDPOINT: &str = "/notifications";

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
pub async fn run(store: Arc<Store>, mut interval_secs: u64) {
    loop {
        if let Some(requested) = poll_all_accounts(&store).await {
            interval_secs = interval_secs.max(requested);
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

async fn poll_all_accounts(store: &Store) -> Option<u64> {
    let accounts = match store.list_accounts() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("failed to list accounts for poll: {e}");
            return None;
        }
    };
    let mut max_requested_interval = None;
    for account in accounts {
        match poll_account(store, &account).await {
            Ok(requested) => {
                max_requested_interval = max_requested_interval.max(requested);
            }
            Err(e) => tracing::warn!(account = %account.login, "poll failed: {e}"),
        }
    }
    max_requested_interval
}

/// Polls one account and returns the `X-Poll-Interval` GitHub requested for
/// it this cycle, if any.
async fn poll_account(store: &Store, account: &AccountRef) -> crate::error::Result<Option<u64>> {
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;

    let mut fetched = client.fetch_search_items().await?;

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

    let previous = store.items_for_account(&account.id)?;
    let result = diff(&previous, &fetched);

    for (change_kind, mut item) in result.changes {
        if change_kind != ChangeKind::New {
            // Preserve the original first_seen_at on updates/carries; the
            // GitHub client only knows "now" since it has no prior record.
            if let Some(prev) = previous.iter().find(|p| p.id == item.id) {
                item.first_seen_at = prev.first_seen_at.clone();
            }
        }
        item.last_seen_at = now_rfc3339();
        store.upsert_item(&item)?;
    }
    for resolved_id in result.resolved_ids {
        store.mark_item_done(&resolved_id)?;
    }

    Ok(requested_interval)
}

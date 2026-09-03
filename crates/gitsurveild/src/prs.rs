//! Pull-request sync: keeps the `pull_requests` table in step with GitHub
//! (`specs/desktop-ui.md`, Pull Requests; `specs/daemon.md`, storage).
//!
//! The Pull Requests view used to issue a live GraphQL query on every open.
//! That is fine for a view the user opens deliberately, but it cannot answer
//! "has this worktree's branch been merged?" — the Repositories pane needs
//! that on every panel expand, offline, with no account round trip. So PRs
//! are synced into SQLite here and both surfaces read the table.
//!
//! Two rules shape this module:
//!
//! - **No polling task of its own.** The sync rides the existing poller cycle
//!   (`CLAUDE.md`'s rate-limit rule); [`sync_due`] throttles it well below the
//!   poll interval.
//! - **Failures never propagate.** A rate limit, an expired token, or an
//!   offline machine leaves the last good table in place; the poll cycle
//!   carries on.

use gitsurveil_proto::{AccountRef, PrState, PullRequestSummary};

use crate::error::Result;
use crate::github::GitHubClient;
use crate::keychain;
use crate::store::Store;

/// `meta` key holding the RFC 3339 timestamp of the last completed sync.
const SYNCED_AT_KEY: &str = "prs_synced_at";

/// Minimum gap between syncs, in seconds.
///
/// Deliberately far slower than the 60s notification cycle: PR state moves in
/// minutes, not seconds, and each sync costs three GraphQL searches per
/// account (open, merged, closed) with review threads attached to every node.
const SYNC_INTERVAL_SECS: i64 = 900;

/// How long a settled (merged or closed) PR is kept before pruning. Long
/// enough that a worktree left lying around for weeks still shows its
/// "Merged" marker, short enough that the table doesn't grow without bound.
const RETENTION_DAYS: i64 = 90;

/// Whether enough time has passed since the last sync to run another.
///
/// A missing or unparseable watermark means "never synced", so the first
/// poll cycle after an upgrade fills the table.
pub fn sync_due(store: &Store, now: chrono::DateTime<chrono::Utc>) -> bool {
    let last = match store.get_meta(SYNCED_AT_KEY) {
        Ok(Some(v)) => v,
        Ok(None) => return true,
        Err(e) => {
            tracing::warn!("could not read the pull-request sync watermark: {e}");
            return false;
        }
    };
    match chrono::DateTime::parse_from_rfc3339(&last) {
        Ok(t) => (now - t.with_timezone(&chrono::Utc)).num_seconds() >= SYNC_INTERVAL_SECS,
        Err(_) => true,
    }
}

/// Syncs every configured account, then prunes settled rows and records the
/// watermark.
///
/// One account failing does not stop the others — the same rule the poll loop
/// follows, for the same reason: one broken token must not blank the view for
/// every other account.
pub async fn sync_all(store: &Store) -> Result<()> {
    let now = chrono::Utc::now();
    let synced_at = now.to_rfc3339();
    for account in store.list_accounts()? {
        if let Err(e) = sync_account(store, &account, &synced_at).await {
            tracing::warn!(account = %account.login, "pull-request sync failed: {e}");
        }
    }
    let cutoff = (now - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
    match store.prune_pull_requests(&cutoff) {
        Ok(n) if n > 0 => tracing::debug!("pruned {n} settled pull requests"),
        Ok(_) => {}
        Err(e) => tracing::warn!("pruning pull requests failed: {e}"),
    }
    store.set_meta(SYNCED_AT_KEY, &synced_at)?;
    Ok(())
}

/// Fetches one account's pull requests in all three states and reconciles the
/// stored rows against them.
///
/// Merged and closed are fetched explicitly because a merged PR is precisely
/// what *stops* appearing in an open search — polling the open set alone can
/// never observe the transition that the "Merged" marker depends on.
async fn sync_account(store: &Store, account: &AccountRef, synced_at: &str) -> Result<()> {
    let token = match keychain::get_token(&account.id)? {
        Some(t) => t,
        // An account with no stored token isn't an error here; it just has
        // nothing to sync until the user re-authenticates.
        None => return Ok(()),
    };
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;

    let mut all: Vec<PullRequestSummary> = Vec::new();
    for state in [PrState::Open, PrState::Merged, PrState::Closed] {
        all.extend(client.list_pull_requests(Some(state)).await?);
    }
    // ponytail: search returns at most 100 nodes per role, and only PRs the
    // user authored, reviewed, or was assigned. A branch whose PR someone
    // else opened, or one beyond that cap, will not be marked merged.
    // Upgrade path: per-repo `repository { pullRequests(headRefName:) }`
    // lookups driven by the worktree branches actually on disk.
    store.upsert_pull_requests(&all, synced_at)?;

    // Anything still marked open that this sync didn't see has left the open
    // set. If it was merged or closed, the passes above already corrected it;
    // otherwise the row is stale and must go rather than linger as open.
    let dropped = store.drop_stale_open_prs(&account.id, synced_at)?;
    tracing::debug!(
        account = %account.login,
        fetched = all.len(),
        dropped,
        "pull requests synced"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    /// A never-synced store must sync on the first cycle, and a fresh
    /// watermark must hold the next one off — this is the whole rate-limit
    /// guarantee, so it is worth pinning.
    #[test]
    fn sync_is_due_only_after_the_interval_elapses() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        assert!(sync_due(&store, now), "a never-synced store must sync");

        store.set_meta(SYNCED_AT_KEY, &now.to_rfc3339()).unwrap();
        assert!(!sync_due(&store, now));
        assert!(!sync_due(&store, now + Duration::seconds(SYNC_INTERVAL_SECS - 1)));
        assert!(sync_due(&store, now + Duration::seconds(SYNC_INTERVAL_SECS)));
    }

    /// A watermark written by a future version (or corrupted on disk) must
    /// not wedge the sync off forever.
    #[test]
    fn an_unparseable_watermark_forces_a_sync() {
        let store = Store::open_in_memory().unwrap();
        store.set_meta(SYNCED_AT_KEY, "not-a-timestamp").unwrap();
        assert!(sync_due(&store, Utc::now()));
    }
}

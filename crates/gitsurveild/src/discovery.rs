//! Background repository discovery (`specs/desktop-ui.md`).
//!
//! The catalog is a slow-moving feed, so unlike the minute-level poller it
//! refreshes every six hours and at startup. Discovery is rate-limit-aware:
//! before a cycle it checks the account's remaining core quota and skips when
//! a full pass would risk crowding out the poller, and any failed pass just
//! leaves the stale cache served (never empties it). `repos.refresh` forces a
//! cycle on demand.

use std::sync::Arc;
use std::time::Duration;

use gitsurveil_proto::AccountRef;

use crate::error::Result;
use crate::github::GitHubClient;
use crate::poller::now_rfc3339;
use crate::store::Store;
use crate::{keychain, DaemonError};

/// How often discovery refreshes the catalog. Six hours keeps the fast path
/// nearly free: `GET /user/repos` costs a few requests per cycle, and between
/// cycles the pane reads SQLite.
const DISCOVERY_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// Discovery bails for an account when fewer than this many core requests
/// remain. The poller's own work must never be starved by the background
/// catalog pass.
const MIN_CORE_REMAINING: u64 = 200;

/// Runs the discovery loop forever: one pass across every account, then sleep.
/// Errors never abort the loop — a broken account must not stop discovery for
/// the rest (`specs/github-integration.md`, "Edge cases").
pub async fn run(store: Arc<Store>) {
    loop {
        discover_all_accounts(&store).await;
        tokio::time::sleep(Duration::from_secs(DISCOVERY_INTERVAL_SECS)).await;
    }
}

/// Runs one discovery pass across every account, merging any fresh catalog
/// into the store. Exposed so the `repos.refresh` API method can force a
/// cycle without waiting for the timer.
pub async fn discover_all_accounts(store: &Store) {
    let accounts = match store.list_accounts() {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("failed to list accounts for discovery: {e}");
            return;
        }
    };
    for account in accounts {
        if let Err(e) = discover_account(store, &account).await {
            tracing::warn!(account = %account.login, "discovery failed: {e}");
        }
    }
}

/// Fetches one account's catalog and merges it into the store.
async fn discover_account(store: &Store, account: &AccountRef) -> Result<()> {
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;

    // Skip the cycle when the account is close to its quota: the poller owns
    // the rate limit, and a stale catalog is fine for a pane. A 429 elsewhere
    // surfaces as an error here, leaves the cache stale, and retries next
    // cycle — the backoff is simply the six-hour cadence.
    let remaining = client.core_remaining().await?;
    if remaining < MIN_CORE_REMAINING {
        tracing::info!(account = %account.login, remaining, "skipping discovery: core quota low");
        return Ok(());
    }

    let repos = client.list_repos().await?;
    store.upsert_catalog(&account.id, &account.host, &repos, &now_rfc3339())?;
    tracing::info!(account = %account.login, count = repos.len(), "discovery refreshed catalog");
    Ok(())
}

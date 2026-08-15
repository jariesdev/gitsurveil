//! SQLite state store (`specs/daemon.md`). Owns `accounts`, `items`, the
//! per-endpoint `etags` cache used to make no-change polls nearly free
//! (`specs/github-integration.md`), the `repositories`/`clone_jobs` tables
//! behind the Repositories pane and new-repo detection
//! (`specs/desktop-ui.md`), and the `apps` table behind the "Open with"
//! worktree menu (`specs/desktop-ui.md`). `history`/`ai_reports` tables are
//! added in the phases that use them rather than declared here unused —
//! schema grows with the feature that needs it.
//!
//! A single [`Store`] wraps one `rusqlite::Connection` behind a `Mutex`
//! (SQLite serializes writers anyway; this avoids a connection pool for a
//! workload that's a handful of queries per minute).

use std::path::Path;
use std::sync::Mutex;

use gitsurveil_proto::{
    AccountRef, ActionItem, AuthKind, CiStatus, CloneStatus, ItemKind, ItemState, OrgRef,
    RegisteredApp, RepoCatalog, Repository,
};
use rusqlite::{params, Connection};

use crate::error::{DaemonError, Result};
use crate::github::client::DiscoveredRepo;

const SCHEMA_VERSION: i64 = 5;

/// The daemon's persistent state store.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Opens (creating if necessary) the SQLite database at `path` and
    /// applies the schema.
    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// In-memory store, used by tests so they never touch disk.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS accounts (
                id         TEXT PRIMARY KEY,
                host       TEXT NOT NULL,
                api_base   TEXT NOT NULL,
                login      TEXT NOT NULL,
                auth_kind  TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS items (
                id             TEXT PRIMARY KEY,
                account_id     TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                kind           TEXT NOT NULL,
                state          TEXT NOT NULL,
                repo           TEXT NOT NULL,
                number         INTEGER,
                title          TEXT NOT NULL,
                url            TEXT NOT NULL,
                author         TEXT NOT NULL,
                created_at     TEXT NOT NULL,
                updated_at     TEXT NOT NULL,
                first_seen_at  TEXT NOT NULL,
                last_seen_at   TEXT NOT NULL,
                ci_status      TEXT NOT NULL,
                raw_kind       TEXT NOT NULL,
                activity       TEXT,
                archived       INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_items_account ON items(account_id);
            CREATE INDEX IF NOT EXISTS idx_items_state ON items(state);
            CREATE TABLE IF NOT EXISTS etags (
                account_id TEXT NOT NULL,
                endpoint   TEXT NOT NULL,
                etag       TEXT NOT NULL,
                PRIMARY KEY (account_id, endpoint)
            );
            CREATE TABLE IF NOT EXISTS repositories (
                account_id       TEXT REFERENCES accounts(id) ON DELETE CASCADE,
                host             TEXT NOT NULL,
                owner            TEXT NOT NULL,
                name             TEXT NOT NULL,
                full_name        TEXT NOT NULL,
                url              TEXT NOT NULL,
                description      TEXT,
                private          INTEGER NOT NULL DEFAULT 0,
                default_branch   TEXT NOT NULL,
                clone_url        TEXT NOT NULL,
                clone_path       TEXT,
                tracked          INTEGER NOT NULL DEFAULT 0,
                notify_enabled   INTEGER NOT NULL DEFAULT 1,
                first_seen_at    TEXT NOT NULL,
                notified_at      TEXT,
                last_refreshed_at TEXT NOT NULL,
                UNIQUE(account_id, full_name)
            );
            CREATE INDEX IF NOT EXISTS idx_repositories_tracked ON repositories(tracked);
            CREATE INDEX IF NOT EXISTS idx_repositories_notified ON repositories(notified_at);
            CREATE TABLE IF NOT EXISTS clone_jobs (
                id          TEXT PRIMARY KEY,
                account_id  TEXT NOT NULL,
                full_name   TEXT NOT NULL,
                target_path TEXT NOT NULL,
                target_owned INTEGER NOT NULL DEFAULT 0,
                status      TEXT NOT NULL,
                received    INTEGER NOT NULL DEFAULT 0,
                total       INTEGER NOT NULL DEFAULT 0,
                error       TEXT,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS apps (
                name       TEXT NOT NULL,
                command    TEXT PRIMARY KEY,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS notification_prefs (
                kind    TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL DEFAULT 1
            );
            ",
        )?;
        // v3: `clone_jobs.target_owned` records whether the daemon created the
        // clone target. Startup and failure cleanup may only remove targets it
        // created; a pre-existing path is never deleted, no matter what.
        let has_target_owned = conn
            .prepare("SELECT 1 FROM pragma_table_info('clone_jobs') WHERE name = 'target_owned'")?
            .query_row([], |_| Ok(()))
            .is_ok();
        if !has_target_owned {
            conn.execute_batch(
                "ALTER TABLE clone_jobs ADD COLUMN target_owned INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        // v4: `repositories.notify_enabled` gates whether a repo's items feed
        // notifications and the Pull Requests view. Independent of `tracked`,
        // which gates conflict-resolution eligibility (has a local clone) —
        // conflating the two would silently repurpose already-tracked repos.
        let has_notify_enabled = conn
            .prepare("SELECT 1 FROM pragma_table_info('repositories') WHERE name = 'notify_enabled'")?
            .query_row([], |_| Ok(()))
            .is_ok();
        if !has_notify_enabled {
            conn.execute_batch(
                "ALTER TABLE repositories ADD COLUMN notify_enabled INTEGER NOT NULL DEFAULT 1",
            )?;
        }
        // v5: `items.activity` is the daemon-internal fingerprint of the
        // activity that makes an item qualify (e.g. the comment ids and
        // unresolved thread ids behind an `Authored` item). The poller compares
        // it across polls to detect qualifying transitions without relying on
        // `updated_at`, which also advances on irrelevant events like commits.
        let has_activity = conn
            .prepare("SELECT 1 FROM pragma_table_info('items') WHERE name = 'activity'")?
            .query_row([], |_| Ok(()))
            .is_ok();
        if !has_activity {
            conn.execute_batch("ALTER TABLE items ADD COLUMN activity TEXT")?;
        }
        // v6: `items.archived` is the permanent tombstone "Clear all history"
        // writes. Archived items are excluded from the Dashboard and history,
        // and the poller never resurrects them, so a cleared item that is
        // still open on GitHub can't come back on the next poll.
        let has_archived = conn
            .prepare("SELECT 1 FROM pragma_table_info('items') WHERE name = 'archived'")?
            .query_row([], |_| Ok(()))
            .is_ok();
        if !has_archived {
            conn.execute_batch(
                "ALTER TABLE items ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    // ---- accounts ----------------------------------------------------

    /// Inserts or replaces an account row (tokens are never part of this —
    /// see `crate::keychain`).
    pub fn upsert_account(&self, account: &AccountRef) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO accounts (id, host, api_base, login, auth_kind)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                host = excluded.host,
                api_base = excluded.api_base,
                login = excluded.login,
                auth_kind = excluded.auth_kind",
            params![
                account.id,
                account.host,
                account.api_base,
                account.login,
                auth_kind_to_str(account.auth_kind),
            ],
        )?;
        Ok(())
    }

    /// Removes an account and (via `ON DELETE CASCADE`) all of its items.
    /// Does not touch the keychain — callers must also call
    /// [`crate::keychain::delete_token`].
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM accounts WHERE id = ?1", params![account_id])?;
        Ok(())
    }

    /// Lists all configured accounts.
    pub fn list_accounts(&self) -> Result<Vec<AccountRef>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, host, api_base, login, auth_kind FROM accounts")?;
        let rows = stmt.query_map([], |row| {
            let auth_kind_str: String = row.get(4)?;
            Ok(AccountRef {
                id: row.get(0)?,
                host: row.get(1)?,
                api_base: row.get(2)?,
                login: row.get(3)?,
                auth_kind: auth_kind_from_str(&auth_kind_str),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// The account row for `id`, or [`None`] when it isn't configured. Used
    /// by repo operations that need the account behind a catalog row (its
    /// login and token for cloning).
    pub fn find_account(&self, id: &str) -> Result<Option<AccountRef>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, host, api_base, login, auth_kind FROM accounts WHERE id = ?1",
            params![id],
            |row| {
                let auth_kind_str: String = row.get(4)?;
                Ok(AccountRef {
                    id: row.get(0)?,
                    host: row.get(1)?,
                    api_base: row.get(2)?,
                    login: row.get(3)?,
                    auth_kind: auth_kind_from_str(&auth_kind_str),
                })
            },
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
    }

    // ---- items ---------------------------------------------------------

    /// Replaces the stored row for `item.id` (insert or full overwrite).
    /// Used by the poller after computing a diff — the diff itself is pure
    /// and doesn't touch storage (see `crate::github::diff`).
    ///
    /// `archived` is deliberately *not* part of the conflict update: it is a
    /// permanent tombstone owned by the user's "Clear all history" action,
    /// so a poll can never un-archive an item. The poller additionally skips
    /// archived rows entirely (`should_preserve_local_state`), so this only
    /// matters as a defensive guarantee.
    pub fn upsert_item(&self, item: &ActionItem) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO items (
                id, account_id, kind, state, repo, number, title, url, author,
                created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                raw_kind, activity, archived
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                title = excluded.title,
                url = excluded.url,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at,
                ci_status = excluded.ci_status,
                activity = excluded.activity",
            params![
                item.id,
                item.account_id,
                kind_to_str(item.kind),
                state_to_str(item.state),
                item.repo,
                item.number.map(|n| n as i64),
                item.title,
                item.url,
                item.author,
                item.created_at,
                item.updated_at,
                item.first_seen_at,
                item.last_seen_at,
                ci_status_to_str(item.ci_status),
                item.raw_kind,
                item.activity,
                item.archived as i64,
            ],
        )?;
        Ok(())
    }

    /// Marks an item `Done` (resolved upstream) rather than deleting it —
    /// Phase 5's history view reads done items before they age out.
    pub fn mark_item_done(&self, item_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE items SET state = 'done' WHERE id = ?1",
            params![item_id],
        )?;
        Ok(())
    }

    /// Sets an item's local dismissed state (`items.dismiss`/`items.undismiss`).
    pub fn set_dismissed(&self, item_id: &str, dismissed: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let state = if dismissed { "dismissed" } else { "open" };
        conn.execute(
            "UPDATE items SET state = ?1 WHERE id = ?2",
            params![state, item_id],
        )?;
        Ok(())
    }

    /// All items currently stored for `account_id`, regardless of state or
    /// archive status — the poller diffs against this full set
    /// (`specs/github-integration.md`).
    pub fn items_for_account(&self, account_id: &str) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                    raw_kind, activity, archived
             FROM items WHERE account_id = ?1",
        )?;
        let rows = stmt.query_map(params![account_id], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Open, non-archived items across all accounts, for the `items.list` API
    /// method and the `status` open-item count. Done, dismissed, and archived
    /// items are excluded — the Dashboard only shows what needs action.
    pub fn open_items(&self) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                    raw_kind, activity, archived
             FROM items i WHERE state = 'open' AND archived = 0 AND NOT EXISTS (
                SELECT 1 FROM repositories r
                WHERE r.account_id = i.account_id AND r.full_name = i.repo
                  AND r.notify_enabled = 0
             )",
        )?;
        let rows = stmt.query_map([], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Resolved and dismissed items, newest activity first — the desktop UI's
    /// history view (`specs/desktop-ui.md`). Archived items are excluded: the
    /// user cleared them for good, so they must not resurface here.
    pub fn history_items(&self, limit: usize) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                    raw_kind, activity, archived
             FROM items i WHERE state IN ('done', 'dismissed') AND archived = 0
               AND NOT EXISTS (
                SELECT 1 FROM repositories r
                WHERE r.account_id = i.account_id AND r.full_name = i.repo
                  AND r.notify_enabled = 0
             )
             ORDER BY last_seen_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Empties the History view (`items.clear_history`). Every resolved and
    /// dismissed item is archived rather than deleted: archived items are
    /// excluded from the Dashboard and history, and the poller never
    /// resurrects them, so a dismissed item that is still open on GitHub can't
    /// come back on the next poll (deleting the row would re-add it as `New`).
    /// Callers confirm with the user first — archiving is permanent.
    pub fn clear_history(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE items SET archived = 1 WHERE state IN ('done', 'dismissed')",
            [],
        )?;
        Ok(())
    }

    // ---- etags -----------------------------------------------------------

    /// The cached ETag for `(account_id, endpoint)`, if any. A `304` response
    /// using this costs zero GitHub rate-limit quota.
    pub fn get_etag(&self, account_id: &str, endpoint: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT etag FROM etags WHERE account_id = ?1 AND endpoint = ?2",
            params![account_id, endpoint],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
    }

    /// Stores the ETag returned for `(account_id, endpoint)`.
    pub fn set_etag(&self, account_id: &str, endpoint: &str, etag: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO etags (account_id, endpoint, etag) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, endpoint) DO UPDATE SET etag = excluded.etag",
            params![account_id, endpoint, etag],
        )?;
        Ok(())
    }

    // ---- repositories ------------------------------------------------

    /// Merges one discovery pass for an account into the catalog: new repos
    /// are inserted untracked, known ones have their GitHub-provided fields
    /// refreshed. `tracked`, `clone_path`, `first_seen_at`, and `notified_at`
    /// are never overwritten by discovery — they record user intent and the
    /// *first* time a repo was seen, both of which a refresh must preserve.
    ///
    /// The account's very first discovery pass is a baseline: every repo found
    /// is pre-acknowledged (`notified_at` set), so a fresh install doesn't
    /// flood the user with "new repository" prompts for repos they already
    /// had. Subsequent passes leave `notified_at` null so genuinely new repos
    /// surface via `repos.new`.
    pub fn upsert_catalog(
        &self,
        account_id: &str,
        host: &str,
        discovered: &[DiscoveredRepo],
        now: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let is_baseline: bool = {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM repositories WHERE account_id = ?1",
                    params![account_id], |row| row.get(0))?;
            count == 0
        };
        let notified = if is_baseline { Some(now) } else { None };
        for repo in discovered {
            conn.execute(
                "INSERT INTO repositories (
                    account_id, host, owner, name, full_name, url, description, private,
                    default_branch, clone_url, clone_path, tracked,
                    first_seen_at, notified_at, last_refreshed_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL,0,?11,?12,?11)
                 ON CONFLICT(account_id, full_name) DO UPDATE SET
                    host = excluded.host,
                    url = excluded.url,
                    description = excluded.description,
                    private = excluded.private,
                    default_branch = excluded.default_branch,
                    clone_url = excluded.clone_url,
                    last_refreshed_at = excluded.last_refreshed_at",
                params![
                    account_id,
                    host,
                    repo.owner,
                    repo.name,
                    repo.full_name(),
                    repo.url,
                    repo.description,
                    repo.private as i64,
                    repo.default_branch,
                    repo.clone_url,
                    now,
                    notified,
                ],
            )?;
        }
        Ok(())
    }

    /// Every repo and the orgs to group them by, for the Repositories pane
    /// (`repos.list`). Orgs are derived from the catalog itself (distinct
    /// `owner` per account) rather than stored separately — an org with no
    /// discovered repos is an empty filter anyway.
    pub fn list_catalog(&self) -> Result<RepoCatalog> {
        let conn = self.conn.lock().unwrap();
        let mut repos_stmt = conn.prepare(
            "SELECT account_id, host, owner, name, full_name, url, description, private,
                    default_branch, clone_url, clone_path, tracked,
                    first_seen_at, notified_at, last_refreshed_at, notify_enabled
             FROM repositories ORDER BY full_name",
        )?;
        let repos = repos_stmt
            .query_map([], row_to_repository)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Legacy rows without an account aren't attributable to any account,
        // so they're excluded from the org list but still shown in the repo
        // list under "All accounts".
        let mut orgs_stmt = conn.prepare(
            "SELECT account_id, host, owner FROM repositories
             WHERE account_id IS NOT NULL
             GROUP BY account_id, host, owner
             ORDER BY owner",
        )?;
        let orgs = orgs_stmt
            .query_map([], |row| {
                Ok(OrgRef {
                    account_id: row.get(0)?,
                    host: row.get(1)?,
                    name: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(RepoCatalog { orgs, repos })
    }

    /// Untracked repos the user hasn't acknowledged yet — `repos.new`.
    /// A repo leaves this set when its path is registered (`repos.set`, a
    /// finished `repos.clone`) or via `ack_new_repos` (dismiss-all).
    pub fn list_new_repos(&self) -> Result<Vec<Repository>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT account_id, host, owner, name, full_name, url, description, private,
                    default_branch, clone_url, clone_path, tracked,
                    first_seen_at, notified_at, last_refreshed_at, notify_enabled
             FROM repositories WHERE tracked = 0 AND notified_at IS NULL
             ORDER BY first_seen_at DESC",
        )?;
        let rows = stmt
            .query_map([], row_to_repository)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Marks every unacknowledged new repo as acknowledged (dismiss-all) and
    /// returns how many rows that covered.
    pub fn ack_new_repos(&self, now: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE repositories SET notified_at = ?1 WHERE tracked = 0 AND notified_at IS NULL",
            params![now],
        )?;
        Ok(changed)
    }

    /// The row for `(account_id, full_name)`, if the account has it in its
    /// catalog.
    pub fn find_repo(&self, account_id: &str, full_name: &str) -> Result<Option<Repository>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT account_id, host, owner, name, full_name, url, description, private,
                    default_branch, clone_url, clone_path, tracked,
                    first_seen_at, notified_at, last_refreshed_at, notify_enabled
             FROM repositories WHERE account_id = ?1 AND full_name = ?2",
            params![account_id, full_name],
            row_to_repository,
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
    }

    /// Distinct account ids holding a row for `full_name` — how a `repo`-only
    /// call (no `account_id`) resolves which account it means when the same
    /// `owner/name` exists under several.
    pub fn accounts_for_repo(&self, full_name: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT account_id FROM repositories
             WHERE full_name = ?1 AND account_id IS NOT NULL",
        )?;
        let rows = stmt
            .query_map(params![full_name], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Registers (or replaces) a repo's local clone path, marks it tracked,
    /// and acknowledges it as seen. Returns the updated row, or [`None`] when
    /// the repo isn't in the catalog. Backs both `repos.set` and the tail of a
    /// successful `repos.clone`.
    pub fn set_repo_path(
        &self,
        account_id: &str,
        full_name: &str,
        path: &str,
        now: &str,
    ) -> Result<Option<Repository>> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE repositories SET clone_path = ?3, tracked = 1, notified_at = ?4
             WHERE account_id = ?1 AND full_name = ?2",
            params![account_id, full_name, path, now],
        )?;
        drop(conn);
        if changed == 0 {
            return Ok(None);
        }
        self.find_repo(account_id, full_name)
    }

    /// Full names of the account's repos with notifications muted
    /// (`notify_enabled = 0`). Checked by the poller before turning an item
    /// into a notification candidate, and by the Pull Requests list — the
    /// one definition of "enabled" both share.
    pub fn muted_repos(&self, account_id: &str) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT full_name FROM repositories WHERE account_id = ?1 AND notify_enabled = 0",
        )?;
        let rows = stmt
            .query_map(params![account_id], |row| row.get(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<String>>>()?;
        Ok(rows)
    }

    /// Sets whether a repo's items feed notifications and the Pull Requests
    /// view. Returns the updated row, or [`None`] when the repo isn't in the
    /// catalog. Backs `repos.set_notify`.
    pub fn set_notify_enabled(
        &self,
        account_id: &str,
        full_name: &str,
        enabled: bool,
    ) -> Result<Option<Repository>> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE repositories SET notify_enabled = ?3 WHERE account_id = ?1 AND full_name = ?2",
            params![account_id, full_name, enabled as i64],
        )?;
        drop(conn);
        if changed == 0 {
            return Ok(None);
        }
        self.find_repo(account_id, full_name)
    }

    /// Clears a repo's local clone path and untracks it (`repos.remove`).
    /// Idempotent; the repo stays in the catalog so it can be re-registered
    /// or cloned later.
    pub fn untrack_repo(&self, account_id: &str, full_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE repositories SET clone_path = NULL, tracked = 0
             WHERE account_id = ?1 AND full_name = ?2",
            params![account_id, full_name],
        )?;
        Ok(())
    }

    /// Imports repos from a pre-catalog config file (the old `repos` block in
    /// `config.toml`) into the catalog, tracked with their existing clone
    /// paths. Runs at most once: it's a no-op when the catalog already has
    /// rows. Rows get `account_id` only when exactly one account is
    /// configured, otherwise [`None`] — with several accounts there's no way
    /// to attribute the old list.
    pub fn import_legacy_repos(&self, legacy: &[(String, String)], now: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let existing: i64 =
            conn.query_row("SELECT COUNT(*) FROM repositories", [], |row| row.get(0))?;
        if existing > 0 {
            return Ok(0);
        }
        let account_id: Option<String> = {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))?;
            if count == 1 {
                conn.query_row("SELECT id FROM accounts", [], |row| row.get(0))
                    .ok()
            } else {
                None
            }
        };
        let mut imported = 0;
        for (full_name, path) in legacy {
            let (owner, name) = full_name.split_once('/').unwrap_or((full_name.as_str(), ""));
            let changed = conn.execute(
                "INSERT OR IGNORE INTO repositories (
                    account_id, host, owner, name, full_name, url, description, private,
                    default_branch, clone_url, clone_path, tracked,
                    first_seen_at, notified_at, last_refreshed_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,NULL,0,'main',?7,?8,1,?9,?9,?9)",
                params![
                    account_id,
                    "github.com",
                    owner,
                    name,
                    full_name,
                    // Placeholders only — the next discovery refresh fills in
                    // the real url/branch/clone_url from GitHub.
                    format!("https://github.com/{full_name}"),
                    format!("https://github.com/{full_name}.git"),
                    path,
                    now,
                ],
            )?;
            imported += changed as usize;
        }
        Ok(imported)
    }

    // ---- clone jobs ---------------------------------------------------

    /// Creates a `running` clone job and returns its id. The row survives a
    /// daemon restart so startup can find and clean up the partial clone.
    ///
    /// `target_owned` is true only when the target did not exist when the job
    /// started — i.e. the daemon is about to create it. Cleanup on failure or
    /// at startup may only remove directories the daemon owns.
    pub fn create_clone_job(
        &self,
        account_id: &str,
        full_name: &str,
        target_path: &str,
        target_owned: bool,
        now: &str,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO clone_jobs (id, account_id, full_name, target_path, target_owned, status, received, total, error, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,'running',0,0,NULL,?6,?6)",
            params![id, account_id, full_name, target_path, target_owned as i64, now],
        )?;
        Ok(id)
    }

    /// The current wire status of a clone job, or [`None`] for an unknown id.
    /// A finished job carries the updated repository row so the UI can mark
    /// the repo tracked without refetching.
    pub fn clone_status(&self, job_id: &str) -> Result<Option<CloneStatus>> {
        let conn = self.conn.lock().unwrap();
        let job = conn.query_row(
            "SELECT status, received, total, error, account_id, full_name
             FROM clone_jobs WHERE id = ?1",
            params![job_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        );
        let (status, received, total, error, account_id, full_name) = match job {
            Ok(job) => job,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        drop(conn);
        // A done job must show the resulting repo row; the catalog lookup is
        // a separate query so the status row's column order never needs to
        // line up with the repository row's.
        let repo = if status == "done" {
            let account_id = account_id.ok_or_else(|| {
                DaemonError::Config("a finished clone job must have an account".into())
            })?;
            self.find_repo(&account_id, &full_name)?
        } else {
            None
        };
        Ok(Some(CloneStatus {
            job_id: job_id.to_string(),
            status: match status.as_str() {
                "done" => gitsurveil_proto::CloneState::Done,
                "failed" => gitsurveil_proto::CloneState::Failed,
                _ => gitsurveil_proto::CloneState::Running,
            },
            received: received as u64,
            total: total as u64,
            repo,
            error,
        }))
    }

    /// Records how many bytes git has fetched. Progress rows are what the
    /// Repositories pane's progress bar polls (`repos.clone_status`).
    pub fn update_clone_progress(&self, job_id: &str, received: u64, total: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE clone_jobs SET received = ?2, total = ?3 WHERE id = ?1",
            params![job_id, received as i64, total as i64],
        )?;
        Ok(())
    }

    /// Marks a clone job done and the repo tracked in one step — both halves
    /// must land together or a crash between them would leave an inconsistent
    /// "done but not tracked" repo. Returns the updated repo row.
    pub fn finish_clone_job(&self, job_id: &str, now: &str) -> Result<Option<Repository>> {
        let conn = self.conn.lock().unwrap();
        let (account_id, full_name, target_path): (String, String, String) = conn.query_row(
            "SELECT account_id, full_name, target_path FROM clone_jobs WHERE id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        conn.execute(
            "UPDATE clone_jobs SET status = 'done', updated_at = ?2 WHERE id = ?1",
            params![job_id, now],
        )?;
        drop(conn);
        self.set_repo_path(&account_id, &full_name, &target_path, now)
    }

    /// Marks a clone job failed with `error` as the reason shown in the UI.
    pub fn fail_clone_job(&self, job_id: &str, error: &str, now: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE clone_jobs SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1",
            params![job_id, error, now],
        )?;
        Ok(())
    }

    /// Jobs left `running` by a previous daemon run, as
    /// `(job_id, target_path, target_owned)`. Their partial targets must be
    /// removed and the rows deleted at startup (or the next clone into the
    /// same path would collide with a half-finished checkout) — but only when
    /// `target_owned` says the daemon created the target.
    pub fn stale_running_jobs(&self) -> Result<Vec<(String, String, bool)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, target_path, target_owned FROM clone_jobs WHERE status = 'running'")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Removes a clone job row (used after cleaning up a stale running job).
    pub fn delete_clone_job(&self, job_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM clone_jobs WHERE id = ?1", params![job_id])?;
        Ok(())
    }

    // ---- apps ------------------------------------------------------------

    /// Every registered "Open with" application, by display name — `apps.list`.
    pub fn list_apps(&self) -> Result<Vec<RegisteredApp>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT name, command FROM apps ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| {
            Ok(RegisteredApp {
                name: row.get(0)?,
                command: row.get(1)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Whether `command` is registered — the gate `apps.open` checks before
    /// launching anything.
    pub fn app_registered(&self, command: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM apps WHERE command = ?1", params![command],
                |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Registers an app. Returns `false` when the command is already
    /// registered (the caller reports that as a conflict). The daemon only
    /// ever runs `command` from an `apps` row, so the stored command is what
    /// gets launched; a duplicate would be two display names for the same
    /// executable, which is never useful.
    pub fn add_app(&self, app: &RegisteredApp, now: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO apps (name, command, created_at) VALUES (?1, ?2, ?3)",
            params![app.name, app.command, now],
        )?;
        Ok(inserted == 1)
    }

    /// Removes a registered app. Idempotent; `false` when it wasn't there.
    pub fn remove_app(&self, command: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute("DELETE FROM apps WHERE command = ?1", params![command])?;
        Ok(removed == 1)
    }

    // ---- notification preferences -----------------------------------------

    /// Item kinds the user has turned off notifications for
    /// (`specs/notifications.md` § Preferences). A kind with no row is
    /// enabled by default, so a fresh install notifies on everything without
    /// requiring an opt-in step.
    pub fn disabled_kinds(&self) -> Result<std::collections::HashSet<ItemKind>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT kind FROM notification_prefs WHERE enabled = 0")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.iter().map(|s| kind_from_str(s)).collect())
    }

    /// Sets whether `kind` should produce a notification. Does not affect
    /// whether items of that kind still appear in the Dashboard or history —
    /// this only gates the OS notification/tray interruption.
    pub fn set_notification_pref(&self, kind: ItemKind, enabled: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notification_prefs (kind, enabled) VALUES (?1, ?2)
             ON CONFLICT(kind) DO UPDATE SET enabled = excluded.enabled",
            params![kind_to_str(kind), enabled as i64],
        )?;
        Ok(())
    }
}

fn row_to_repository(row: &rusqlite::Row) -> rusqlite::Result<Repository> {
    let description: Option<String> = row.get(6)?;
    let private: i64 = row.get(7)?;
    let tracked: i64 = row.get(11)?;
    let notify_enabled: i64 = row.get(15)?;
    Ok(Repository {
        account_id: row.get(0)?,
        host: row.get(1)?,
        owner: row.get(2)?,
        name: row.get(3)?,
        full_name: row.get(4)?,
        url: row.get(5)?,
        description,
        private: private != 0,
        default_branch: row.get(8)?,
        clone_url: row.get(9)?,
        clone_path: row.get(10)?,
        tracked: tracked != 0,
        first_seen_at: row.get(12)?,
        notified_at: row.get(13)?,
        last_refreshed_at: row.get(14)?,
        notify_enabled: notify_enabled != 0,
    })
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<ActionItem> {
    let kind_str: String = row.get(2)?;
    let state_str: String = row.get(3)?;
    let ci_status_str: String = row.get(13)?;
    let number: Option<i64> = row.get(5)?;
    Ok(ActionItem {
        id: row.get(0)?,
        account_id: row.get(1)?,
        kind: kind_from_str(&kind_str),
        state: state_from_str(&state_str),
        repo: row.get(4)?,
        number: number.map(|n| n as u64),
        title: row.get(6)?,
        url: row.get(7)?,
        author: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        first_seen_at: row.get(11)?,
        last_seen_at: row.get(12)?,
        ci_status: ci_status_from_str(&ci_status_str),
        raw_kind: row.get(14)?,
        activity: row.get(15)?,
        archived: row.get(16)?,
    })
}

// serde already gives these enums snake_case JSON representations; reusing
// that here (rather than deriving a second string mapping) would require
// pulling serde_json into every query, so these small mappings are kept
// hand-written and exhaustively matched — the compiler catches drift if a
// variant is ever added to gitsurveil-proto without updating these.

fn kind_to_str(k: ItemKind) -> &'static str {
    match k {
        ItemKind::ReviewRequested => "review_requested",
        ItemKind::Assigned => "assigned",
        ItemKind::Mentioned => "mentioned",
        ItemKind::Participating => "participating",
        ItemKind::CiFailed => "ci_failed",
        ItemKind::ReviewStateChanged => "review_state_changed",
        ItemKind::ReadyToMerge => "ready_to_merge",
        ItemKind::Authored => "authored",
        ItemKind::ReviewedByMe => "reviewed_by_me",
    }
}

fn kind_from_str(s: &str) -> ItemKind {
    match s {
        "review_requested" => ItemKind::ReviewRequested,
        "assigned" => ItemKind::Assigned,
        "mentioned" => ItemKind::Mentioned,
        "ci_failed" => ItemKind::CiFailed,
        "review_state_changed" => ItemKind::ReviewStateChanged,
        "ready_to_merge" => ItemKind::ReadyToMerge,
        "authored" => ItemKind::Authored,
        "reviewed_by_me" => ItemKind::ReviewedByMe,
        _ => ItemKind::Participating,
    }
}

fn state_to_str(s: ItemState) -> &'static str {
    match s {
        ItemState::Open => "open",
        ItemState::Done => "done",
        ItemState::Dismissed => "dismissed",
    }
}

fn state_from_str(s: &str) -> ItemState {
    match s {
        "done" => ItemState::Done,
        "dismissed" => ItemState::Dismissed,
        _ => ItemState::Open,
    }
}

fn ci_status_to_str(s: CiStatus) -> &'static str {
    match s {
        CiStatus::None => "none",
        CiStatus::Pending => "pending",
        CiStatus::Passing => "passing",
        CiStatus::Failing => "failing",
    }
}

fn ci_status_from_str(s: &str) -> CiStatus {
    match s {
        "pending" => CiStatus::Pending,
        "passing" => CiStatus::Passing,
        "failing" => CiStatus::Failing,
        _ => CiStatus::None,
    }
}

fn auth_kind_to_str(k: AuthKind) -> &'static str {
    match k {
        AuthKind::Pat => "pat",
        AuthKind::OauthDevice => "oauth_device",
    }
}

fn auth_kind_from_str(s: &str) -> AuthKind {
    match s {
        "oauth_device" => AuthKind::OauthDevice,
        _ => AuthKind::Pat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_account() -> AccountRef {
        AccountRef {
            id: "acc-1".into(),
            host: "github.com".into(),
            api_base: "https://api.github.com".into(),
            login: "octocat".into(),
            auth_kind: AuthKind::Pat,
        }
    }

    fn sample_item(id: &str) -> ActionItem {
        ActionItem {
            id: id.into(),
            account_id: "acc-1".into(),
            kind: ItemKind::ReviewRequested,
            state: ItemState::Open,
            repo: "acme/api".into(),
            number: Some(482),
            title: "Fix the thing".into(),
            url: "https://github.com/acme/api/pull/482".into(),
            author: "someone".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            updated_at: "2026-08-01T00:00:00Z".into(),
            first_seen_at: "2026-08-01T00:00:00Z".into(),
            last_seen_at: "2026-08-01T00:00:00Z".into(),
            ci_status: CiStatus::Passing,
            raw_kind: "review_requested".into(),
            activity: None,
            archived: false,
        }
    }

    #[test]
    fn round_trips_account_and_item() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        assert_eq!(store.list_accounts().unwrap(), vec![sample_account()]);

        let item = sample_item("item-1");
        store.upsert_item(&item).unwrap();
        assert_eq!(store.open_items().unwrap(), vec![item]);
    }

    #[test]
    fn activity_fingerprint_survives_upsert_and_readback() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        let item = ActionItem {
            activity: Some("c:1,2,3;u:t1".into()),
            ..sample_item("item-1")
        };
        store.upsert_item(&item).unwrap();
        // `items_for_account` is what the poller diffs against, so the
        // fingerprint must be present there (not just in `open_items`).
        assert_eq!(store.items_for_account("acc-1").unwrap(), vec![item.clone()]);
        assert_eq!(store.open_items().unwrap(), vec![item]);
    }

    #[test]
    fn dismiss_removes_from_open_items() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();

        store.set_dismissed("item-1", true).unwrap();
        assert!(store.open_items().unwrap().is_empty());

        store.set_dismissed("item-1", false).unwrap();
        assert_eq!(store.open_items().unwrap().len(), 1);
    }

    #[test]
    fn notification_prefs_default_enabled_and_toggle_persists() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.disabled_kinds().unwrap().is_empty(), "nothing muted by default");

        store
            .set_notification_pref(ItemKind::Authored, false)
            .unwrap();
        assert_eq!(
            store.disabled_kinds().unwrap(),
            std::collections::HashSet::from([ItemKind::Authored])
        );

        // Every other kind is still enabled by default.
        assert!(!store.disabled_kinds().unwrap().contains(&ItemKind::CiFailed));

        store
            .set_notification_pref(ItemKind::Authored, true)
            .unwrap();
        assert!(store.disabled_kinds().unwrap().is_empty(), "re-enabling clears it");
    }

    #[test]
    fn mark_done_removes_from_open_items() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();

        store.mark_item_done("item-1").unwrap();
        assert!(store.open_items().unwrap().is_empty());
    }

    #[test]
    fn etag_round_trips_and_updates() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.get_etag("acc-1", "/notifications").unwrap(), None);

        store.set_etag("acc-1", "/notifications", "abc").unwrap();
        assert_eq!(
            store.get_etag("acc-1", "/notifications").unwrap(),
            Some("abc".into())
        );

        store.set_etag("acc-1", "/notifications", "def").unwrap();
        assert_eq!(
            store.get_etag("acc-1", "/notifications").unwrap(),
            Some("def".into())
        );
    }

    #[test]
    fn removing_account_cascades_items() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();

        store.remove_account("acc-1").unwrap();
        assert!(store.items_for_account("acc-1").unwrap().is_empty());
    }

    fn sample_discovered(name: &str) -> DiscoveredRepo {
        DiscoveredRepo {
            owner: "acme".into(),
            name: name.into(),
            url: format!("https://github.com/acme/{name}"),
            description: None,
            private: false,
            default_branch: "main".into(),
            clone_url: format!("https://github.com/acme/{name}.git"),
        }
    }

    #[test]
    fn catalog_baseline_acks_first_pass_only() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        let t0 = "2026-08-01T00:00:00Z";
        let t1 = "2026-08-02T00:00:00Z";

        // First discovery is a baseline: the repo is recorded but acked so it
        // never floods the new-repo modal.
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], t0)
            .unwrap();
        assert!(store.list_new_repos().unwrap().is_empty());

        // A repo appearing in a later pass is genuinely new.
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("newlib")], t1)
            .unwrap();
        let new = store.list_new_repos().unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].full_name, "acme/newlib");

        // Dismiss-all acks just the pending row, and only once.
        assert_eq!(store.ack_new_repos(t1).unwrap(), 1);
        assert_eq!(store.ack_new_repos(t1).unwrap(), 0);
        assert!(store.list_new_repos().unwrap().is_empty());
    }

    #[test]
    fn set_repo_path_tracks_and_acks() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], "t0")
            .unwrap();

        let row = store
            .set_repo_path("acc-1", "acme/api", "/tmp/acme-api", "t1")
            .unwrap()
            .expect("repo is in the catalog");
        assert!(row.tracked);
        assert_eq!(row.clone_path.as_deref(), Some("/tmp/acme-api"));
        assert_eq!(row.notified_at.as_deref(), Some("t1"));
        assert!(store.list_new_repos().unwrap().is_empty());

        store.untrack_repo("acc-1", "acme/api").unwrap();
        let row = store.find_repo("acc-1", "acme/api").unwrap().unwrap();
        assert!(!row.tracked);
        assert!(row.clone_path.is_none());
    }

    #[test]
    fn notify_enabled_defaults_true_and_is_independent_of_tracked() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], "t0")
            .unwrap();

        let row = store.find_repo("acc-1", "acme/api").unwrap().unwrap();
        assert!(row.notify_enabled, "new repos default to notifying");
        assert!(!row.tracked, "notify_enabled must not imply clone tracking");

        // Registering a clone (tracked = true) must not touch notify_enabled.
        store
            .set_repo_path("acc-1", "acme/api", "/tmp/acme-api", "t1")
            .unwrap();
        let row = store.find_repo("acc-1", "acme/api").unwrap().unwrap();
        assert!(row.tracked);
        assert!(row.notify_enabled);

        let row = store
            .set_notify_enabled("acc-1", "acme/api", false)
            .unwrap()
            .expect("repo is in the catalog");
        assert!(!row.notify_enabled);
        assert!(row.tracked, "muting notifications must not untrack the clone");

        assert_eq!(
            store.muted_repos("acc-1").unwrap(),
            std::collections::HashSet::from(["acme/api".to_string()])
        );

        assert!(store
            .set_notify_enabled("acc-1", "no/such-repo", true)
            .unwrap()
            .is_none());
    }

    #[test]
    fn muted_repo_items_are_excluded_from_open_and_history() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], "t0")
            .unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();
        assert_eq!(store.open_items().unwrap().len(), 1);

        store
            .set_notify_enabled("acc-1", "acme/api", false)
            .unwrap();
        assert!(
            store.open_items().unwrap().is_empty(),
            "muting a repo hides its open items immediately"
        );

        store.mark_item_done("item-1").unwrap();
        assert!(
            store.history_items(50).unwrap().is_empty(),
            "muting a repo hides its history too"
        );

        store
            .set_notify_enabled("acc-1", "acme/api", true)
            .unwrap();
        assert_eq!(
            store.history_items(50).unwrap().len(),
            1,
            "unmuting restores it with no data loss"
        );
    }

    #[test]
    fn clear_history_archives_everything_and_keeps_open() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("open-1")).unwrap();
        store.upsert_item(&sample_item("done-1")).unwrap();
        store.upsert_item(&sample_item("dismissed-1")).unwrap();
        store.mark_item_done("done-1").unwrap();
        store.set_dismissed("dismissed-1", true).unwrap();

        store.clear_history().unwrap();

        assert_eq!(store.history_items(50).unwrap().len(), 0, "history is gone");
        let open = store.open_items().unwrap();
        assert_eq!(open.len(), 1, "open items are untouched");
        assert_eq!(open[0].id, "open-1");
        // History rows are archived, not deleted: the dismissed item is still
        // open on GitHub, and keeping the row as an archive is what stops the
        // next poll from re-adding it to the Dashboard.
        let all = store.items_for_account("acc-1").unwrap();
        let archived_ids: Vec<&str> =
            all.iter().filter(|i| i.archived).map(|i| i.id.as_str()).collect();
        assert_eq!(archived_ids, vec!["done-1", "dismissed-1"]);
    }

    #[test]
    fn history_items_exclude_archived_but_keep_dismissed() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("dismissed-keep")).unwrap();
        store.set_dismissed("dismissed-keep", true).unwrap();
        let mut archived = sample_item("dismissed-archived");
        archived.state = gitsurveil_proto::ItemState::Dismissed;
        archived.archived = true;
        store.upsert_item(&archived).unwrap();

        let history = store.history_items(50).unwrap();
        assert_eq!(history.len(), 1, "only the non-archived dismissed item shows");
        assert_eq!(history[0].id, "dismissed-keep");
    }

    #[test]
    fn clone_job_runs_and_finishes_tracked() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], "t0")
            .unwrap();

        let job = store
            .create_clone_job("acc-1", "acme/api", "/tmp/acme-api", true, "t1")
            .unwrap();
        let status = store.clone_status(&job).unwrap().unwrap();
        assert_eq!(status.status, gitsurveil_proto::CloneState::Running);
        assert_eq!(status.repo, None);

        store.update_clone_progress(&job, 4096, 0).unwrap();
        assert_eq!(store.clone_status(&job).unwrap().unwrap().received, 4096);

        let repo = store
            .finish_clone_job(&job, "t2")
            .unwrap()
            .expect("finished clone carries the repo row");
        assert!(repo.tracked);
        assert_eq!(repo.clone_path.as_deref(), Some("/tmp/acme-api"));
        let status = store.clone_status(&job).unwrap().unwrap();
        assert_eq!(status.status, gitsurveil_proto::CloneState::Done);
        assert_eq!(status.repo, Some(repo));
    }

    #[test]
    fn failed_and_stale_clone_jobs() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
            .upsert_catalog("acc-1", "github.com", &[sample_discovered("api")], "t0")
            .unwrap();

        let failed = store
            .create_clone_job("acc-1", "acme/api", "/tmp/a", false, "t1")
            .unwrap();
        store.fail_clone_job(&failed, "network error", "t2").unwrap();
        let status = store.clone_status(&failed).unwrap().unwrap();
        assert_eq!(status.status, gitsurveil_proto::CloneState::Failed);
        assert_eq!(status.error.as_deref(), Some("network error"));
        // A failed job is never "stale running" at startup.
        assert!(store.stale_running_jobs().unwrap().is_empty());

        // A running job left behind by a crash shows up for cleanup.
        let stale = store
            .create_clone_job("acc-1", "acme/api", "/tmp/b", true, "t3")
            .unwrap();
        let pending = store.stale_running_jobs().unwrap();
        assert_eq!(pending, vec![(stale.clone(), "/tmp/b".into(), true)]);

        // A job whose target pre-existed is reported so startup leaves it be.
        let preexisting = store
            .create_clone_job("acc-1", "acme/api", "/tmp/c", false, "t4")
            .unwrap();
        let pending = store.stale_running_jobs().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&(preexisting.clone(), "/tmp/c".into(), false)));

        store.delete_clone_job(&stale).unwrap();
        store.delete_clone_job(&preexisting).unwrap();
        assert!(store.stale_running_jobs().unwrap().is_empty());
    }

    #[test]
    fn legacy_repos_import_once_with_sole_account() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();

        let imported = store
            .import_legacy_repos(&[("acme/api".into(), "/tmp/acme-api".into())], "t0")
            .unwrap();
        assert_eq!(imported, 1);
        let row = store.find_repo("acc-1", "acme/api").unwrap().unwrap();
        assert!(row.tracked);
        assert_eq!(row.clone_path.as_deref(), Some("/tmp/acme-api"));

        // A second import is a no-op once the catalog has rows.
        assert_eq!(
            store
                .import_legacy_repos(&[("acme/other".into(), "/tmp/x".into())], "t1")
                .unwrap(),
            0
        );
        assert!(store.find_repo("acc-1", "acme/other").unwrap().is_none());
    }

    #[test]
    fn apps_round_trip_and_duplicate_rejected() {
        let store = Store::open_in_memory().unwrap();
        let code = RegisteredApp {
            name: "VS Code".into(),
            command: "code".into(),
        };
        assert!(store.add_app(&code, "t0").unwrap());
        // Same command under another name is rejected — the command is the key.
        assert!(!store
            .add_app(&RegisteredApp { name: "Code".into(), command: "code".into() }, "t1")
            .unwrap());

        store
            .add_app(&RegisteredApp { name: "Sublime Merge".into(), command: "smerge".into() }, "t2")
            .unwrap();
        assert_eq!(
            store.list_apps().unwrap(),
            vec![
                RegisteredApp { name: "Sublime Merge".into(), command: "smerge".into() },
                RegisteredApp { name: "VS Code".into(), command: "code".into() },
            ]
        );
        assert!(store.app_registered("code").unwrap());
        assert!(!store.app_registered("vim").unwrap());

        assert!(store.remove_app("code").unwrap());
        assert!(!store.remove_app("code").unwrap()); // idempotent
        assert_eq!(store.list_apps().unwrap().len(), 1);
    }
}

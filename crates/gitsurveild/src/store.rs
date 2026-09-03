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
    AccountRef, ActionItem, AuthKind, CiStatus, CloneStatus, ItemKind, ItemState, MergedPrRef,
    Mergeability, OrgRef, PrRole, PrState, PullRequestSummary, RegisteredApp, RepoCatalog,
    Repository, ReviewDecision,
};
use rusqlite::{params, Connection};

use crate::error::{DaemonError, Result};
use crate::github::client::DiscoveredRepo;

const SCHEMA_VERSION: i64 = 8;

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
    /// Diagnostic-only constructor: wraps an already-open connection without
    /// running `migrate()`, so a test can seed a pre-migration schema first.
    #[cfg(test)]
    fn from_connection(conn: Connection) -> Store {
        Store {
            conn: Mutex::new(conn),
        }
    }

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
                archived       INTEGER NOT NULL DEFAULT 0,
                dismissed_updated_at TEXT,
                dismissed_at         TEXT,
                dismissed_ci_status  TEXT
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
            -- v8: the Pull Requests view used to be a live GraphQL query with
            -- no storage. Persisting it is what makes the question -- has this
            -- worktree's branch been merged? -- answerable without a network round trip on every
            -- panel expand (`specs/desktop-ui.md`, Worktrees). Enum columns
            -- hold the serde `snake_case` spelling so the proto types
            -- round-trip through serde with no second mapping to keep in sync.
            CREATE TABLE IF NOT EXISTS pull_requests (
                account_id         TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                repo               TEXT NOT NULL,
                number             INTEGER NOT NULL,
                title              TEXT NOT NULL,
                url                TEXT NOT NULL,
                author             TEXT NOT NULL,
                roles              TEXT NOT NULL DEFAULT '[]',
                state              TEXT NOT NULL,
                draft              INTEGER NOT NULL DEFAULT 0,
                ci_status          TEXT NOT NULL,
                review_decision    TEXT NOT NULL,
                unresolved_threads INTEGER NOT NULL DEFAULT 0,
                mergeable          TEXT NOT NULL,
                created_at         TEXT NOT NULL,
                updated_at         TEXT NOT NULL,
                head_ref           TEXT,
                synced_at          TEXT NOT NULL,
                PRIMARY KEY (account_id, repo, number)
            );
            CREATE INDEX IF NOT EXISTS idx_prs_state ON pull_requests(state);
            -- The index the worktree join reads; without it every panel expand
            -- scans the whole table.
            CREATE INDEX IF NOT EXISTS idx_prs_head ON pull_requests(repo, head_ref);
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
        // v7: `items.dismissed_updated_at`/`dismissed_at`/`dismissed_ci_status`
        // snapshot the item at the moment of dismissal, so a resurrected item's
        // detail pane can show what changed since then. `dismissed_updated_at`
        // is GitHub's `updated_at` at dismissal time — the skew-free watermark
        // used to split later comments into "already seen" and "arrived while
        // dismissed" (`specs/github-integration.md` § Clock skew: comparisons
        // never mix local and GitHub time). `dismissed_at` is local time, for
        // display only. `dismissed_ci_status` is stored because `Check` carries
        // no timestamp, so a pass→fail transition is otherwise unrecoverable.
        let has_dismissed_watermark = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('items') WHERE name = 'dismissed_updated_at'",
            )?
            .query_row([], |_| Ok(()))
            .is_ok();
        if !has_dismissed_watermark {
            conn.execute_batch(
                "ALTER TABLE items ADD COLUMN dismissed_updated_at TEXT;
                 ALTER TABLE items ADD COLUMN dismissed_at TEXT;
                 ALTER TABLE items ADD COLUMN dismissed_ci_status TEXT;",
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
    /// Upserts a freshly fetched item. On conflict, `dismissed_updated_at`,
    /// `dismissed_at`, and `dismissed_ci_status` are deliberately absent from
    /// `SET` — a fetched item never carries dismissal data (it's always
    /// `None`), so including them would clobber the snapshot `set_dismissed`
    /// wrote. This is what lets a resurrected item's detail pane still show
    /// what changed since it was dismissed.
    pub fn upsert_item(&self, item: &ActionItem) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO items (
                id, account_id, kind, state, repo, number, title, url, author,
                created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                raw_kind, activity, archived,
                dismissed_updated_at, dismissed_at, dismissed_ci_status
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
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
                item.dismissed_updated_at,
                item.dismissed_at,
                item.dismissed_ci_status.map(ci_status_to_str),
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
    /// Dismissing snapshots the item's own `updated_at`/`ci_status` into the
    /// `dismissed_*` columns in the same statement — the watermark a
    /// resurrected item's detail pane later diffs against
    /// (`specs/github-integration.md` § Dismissal watermark). Undismissing
    /// (manual restore from History) clears the snapshot: the user brought it
    /// back deliberately, so there's nothing left to explain.
    pub fn set_dismissed(&self, item_id: &str, dismissed: bool, now: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if dismissed {
            conn.execute(
                "UPDATE items SET state = 'dismissed',
                    dismissed_updated_at = updated_at,
                    dismissed_ci_status = ci_status,
                    dismissed_at = ?1
                 WHERE id = ?2",
                params![now, item_id],
            )?;
        } else {
            conn.execute(
                "UPDATE items SET state = 'open',
                    dismissed_updated_at = NULL,
                    dismissed_ci_status = NULL,
                    dismissed_at = NULL
                 WHERE id = ?1",
                params![item_id],
            )?;
        }
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
                    raw_kind, activity, archived,
                    dismissed_updated_at, dismissed_at, dismissed_ci_status
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
                    raw_kind, activity, archived,
                    dismissed_updated_at, dismissed_at, dismissed_ci_status
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
                    raw_kind, activity, archived,
                    dismissed_updated_at, dismissed_at, dismissed_ci_status
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

    // ---- meta ----------------------------------------------------------

    /// Reads a `meta` row, or [`None`] when the key was never written. The
    /// table has held only `schema_version` until now; the PR sync uses it
    /// for its watermark rather than adding a one-row table of its own.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e.into()),
        })
    }

    /// Writes a `meta` row, replacing any previous value.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- pull requests ---------------------------------------------------

    /// Inserts or replaces PR rows, stamping every one with `synced_at`.
    ///
    /// One transaction, so a partial sync never leaves the table half-updated.
    /// `synced_at` is the caller's cycle timestamp rather than "now" per row:
    /// [`drop_stale_open_prs`](Self::drop_stale_open_prs) uses it to tell rows
    /// this sync saw from rows it didn't.
    pub fn upsert_pull_requests(&self, prs: &[PullRequestSummary], synced_at: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO pull_requests (
                    account_id, repo, number, title, url, author, roles, state, draft,
                    ci_status, review_decision, unresolved_threads, mergeable,
                    created_at, updated_at, head_ref, synced_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            for pr in prs {
                stmt.execute(params![
                    pr.account_id,
                    pr.repo,
                    pr.number as i64,
                    pr.title,
                    pr.url,
                    pr.author,
                    serde_json::to_string(&pr.roles).unwrap_or_else(|_| "[]".into()),
                    enum_to_str(&pr.state),
                    pr.draft as i64,
                    enum_to_str(&pr.ci_status),
                    enum_to_str(&pr.review_decision),
                    pr.unresolved_threads as i64,
                    enum_to_str(&pr.mergeable),
                    pr.created_at,
                    pr.updated_at,
                    pr.head_ref,
                    synced_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The stored PRs for the Pull Requests view, most recently updated first.
    /// `account_id`/`state` are both optional narrowings, matching `prs.list`.
    pub fn list_pull_requests(
        &self,
        account_id: Option<&str>,
        state: Option<PrState>,
    ) -> Result<Vec<PullRequestSummary>> {
        let conn = self.conn.lock().unwrap();
        let state = state.map(|s| enum_to_str(&s));
        // Both filters are optional, so bind them as "NULL means no filter"
        // rather than building the SQL by concatenation.
        let mut stmt = conn.prepare(
            "SELECT account_id, repo, number, title, url, author, roles, state, draft,
                    ci_status, review_decision, unresolved_threads, mergeable,
                    created_at, updated_at, head_ref
             FROM pull_requests
             WHERE (?1 IS NULL OR account_id = ?1)
               AND (?2 IS NULL OR state = ?2)
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![account_id, state], row_to_pull_request)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Merged PRs for `repo`, keyed by their head branch — the join behind the
    /// Repositories pane's "Merged" chip.
    ///
    /// Keyed across every account, because a worktree belongs to a clone, not
    /// to an account. When two merged PRs share a head branch (a branch reused
    /// after an earlier merge) the highest number wins, since that is the most
    /// recent merge.
    pub fn merged_prs_by_head(
        &self,
        repo: &str,
    ) -> Result<std::collections::HashMap<String, MergedPrRef>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT head_ref, number, title, url
             FROM pull_requests
             WHERE repo = ?1 AND state = 'merged' AND head_ref IS NOT NULL
             ORDER BY number ASC",
        )?;
        let rows = stmt.query_map(params![repo], |row| {
            let head: String = row.get(0)?;
            let number: i64 = row.get(1)?;
            Ok((
                head,
                MergedPrRef {
                    number: number as u64,
                    title: row.get(2)?,
                    url: row.get(3)?,
                },
            ))
        })?;
        // Ascending order means the highest number is inserted last and wins.
        rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
            .map_err(Into::into)
    }

    /// Deletes rows still marked `open` that the sync at `synced_at` did not
    /// see, and returns how many went.
    ///
    /// A PR that leaves the open set has either been merged or closed — in
    /// which case the same sync's merged/closed pass already corrected it —
    /// or has fallen outside what search returns for this user. Either way the
    /// stale row must go, so the view never shows a PR as open when it isn't.
    pub fn drop_stale_open_prs(&self, account_id: &str, synced_at: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM pull_requests
             WHERE account_id = ?1 AND state = 'open' AND synced_at < ?2",
            params![account_id, synced_at],
        )?;
        Ok(n)
    }

    /// Drops settled (merged/closed) PRs last updated before `cutoff`, and
    /// returns how many went. Open rows are never pruned by age — they leave
    /// only via [`drop_stale_open_prs`](Self::drop_stale_open_prs) — so an
    /// abandoned long-lived PR can't vanish from the view while still open.
    pub fn prune_pull_requests(&self, cutoff: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM pull_requests WHERE state != 'open' AND updated_at < ?1",
            params![cutoff],
        )?;
        Ok(n)
    }
}

/// Serializes a `#[serde(rename_all = "snake_case")]` proto enum to the string
/// stored in SQLite. Going through serde keeps the column spelling and the
/// wire spelling the same value by construction, so no second mapping can
/// drift out of sync with the proto.
fn enum_to_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Inverse of [`enum_to_str`], falling back to `fallback` for a value written
/// by a future version (or corrupted). A stored row must never fail a whole
/// listing over one unrecognized enum.
fn enum_from_str<T: serde::de::DeserializeOwned>(s: &str, fallback: T) -> T {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(fallback)
}

fn row_to_pull_request(row: &rusqlite::Row) -> rusqlite::Result<PullRequestSummary> {
    let roles: String = row.get(6)?;
    let state: String = row.get(7)?;
    let ci_status: String = row.get(9)?;
    let review_decision: String = row.get(10)?;
    let mergeable: String = row.get(12)?;
    let number: i64 = row.get(2)?;
    let draft: i64 = row.get(8)?;
    let unresolved_threads: i64 = row.get(11)?;
    Ok(PullRequestSummary {
        account_id: row.get(0)?,
        repo: row.get(1)?,
        number: number as u64,
        title: row.get(3)?,
        url: row.get(4)?,
        author: row.get(5)?,
        roles: serde_json::from_str::<Vec<PrRole>>(&roles).unwrap_or_default(),
        state: enum_from_str(&state, PrState::Open),
        draft: draft != 0,
        ci_status: enum_from_str(&ci_status, CiStatus::None),
        review_decision: enum_from_str(&review_decision, ReviewDecision::None),
        unresolved_threads: unresolved_threads as u64,
        mergeable: enum_from_str(&mergeable, Mergeability::Unknown),
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        head_ref: row.get(15)?,
    })
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
    let dismissed_ci_status_str: Option<String> = row.get(19)?;
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
        dismissed_updated_at: row.get(17)?,
        dismissed_at: row.get(18)?,
        dismissed_ci_status: dismissed_ci_status_str.as_deref().map(ci_status_from_str),
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
            dismissed_updated_at: None,
            dismissed_at: None,
            dismissed_ci_status: None,
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

        store.set_dismissed("item-1", true, "2024-01-01T00:00:00Z").unwrap();
        assert!(store.open_items().unwrap().is_empty());

        store.set_dismissed("item-1", false, "2024-01-01T00:00:00Z").unwrap();
        assert_eq!(store.open_items().unwrap().len(), 1);
    }

    #[test]
    fn upgrading_a_pre_dismissal_watermark_db_does_not_duplicate_items() {
        // Simulates a real installed database from before the
        // `dismissed_updated_at`/`dismissed_at`/`dismissed_ci_status` columns
        // existed — the in-memory `Store::open_in_memory()` helper used by
        // every other test always starts from `CREATE TABLE IF NOT EXISTS`
        // with the new columns already baked in, so it never actually
        // exercises the `ALTER TABLE` upgrade path a real user hits.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (
                id TEXT PRIMARY KEY, host TEXT NOT NULL, api_base TEXT NOT NULL,
                login TEXT NOT NULL, auth_kind TEXT NOT NULL
             );
             CREATE TABLE items (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                kind TEXT NOT NULL, state TEXT NOT NULL, repo TEXT NOT NULL,
                number INTEGER, title TEXT NOT NULL, url TEXT NOT NULL, author TEXT NOT NULL,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                first_seen_at TEXT NOT NULL, last_seen_at TEXT NOT NULL,
                ci_status TEXT NOT NULL, raw_kind TEXT NOT NULL,
                activity TEXT, archived INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO accounts VALUES
                ('acc-1', 'github.com', 'https://api.github.com', 'octocat', 'pat');
             INSERT INTO items (
                id, account_id, kind, state, repo, number, title, url, author,
                created_at, updated_at, first_seen_at, last_seen_at, ci_status,
                raw_kind, activity, archived
             ) VALUES (
                'item-1', 'acc-1', 'review_requested', 'open', 'acme/api', 482,
                'Fix the thing', 'https://github.com/acme/api/pull/482', 'someone',
                't0', 't0', 't0', 't0', 'passing', 'review_requested', NULL, 0
             );",
        )
        .unwrap();

        let store = Store::from_connection(conn);
        store.migrate().unwrap();
        // A second run (e.g. a daemon restart) must be idempotent too.
        store.migrate().unwrap();

        let items = store.items_for_account("acc-1").unwrap();
        assert_eq!(items.len(), 1, "upgrading the schema must not duplicate existing rows");
        assert_eq!(items[0].dismissed_updated_at, None);

        // The upgraded row must still upsert in place, not insert a sibling.
        let mut refetched = sample_item("item-1");
        refetched.updated_at = "t1".into();
        store.upsert_item(&refetched).unwrap();
        assert_eq!(store.items_for_account("acc-1").unwrap().len(), 1);
    }

    #[test]
    fn dismiss_snapshots_updated_at_and_ci_status_as_the_watermark() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();

        store
            .set_dismissed("item-1", true, "2026-08-02T00:00:00Z")
            .unwrap();

        let item = store
            .items_for_account("acc-1")
            .unwrap()
            .into_iter()
            .find(|i| i.id == "item-1")
            .unwrap();
        assert_eq!(item.dismissed_updated_at.as_deref(), Some("2026-08-01T00:00:00Z"));
        assert_eq!(item.dismissed_at.as_deref(), Some("2026-08-02T00:00:00Z"));
        assert_eq!(item.dismissed_ci_status, Some(CiStatus::Passing));
    }

    #[test]
    fn undismiss_clears_the_dismissal_watermark() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();
        store
            .set_dismissed("item-1", true, "2026-08-02T00:00:00Z")
            .unwrap();

        store
            .set_dismissed("item-1", false, "2026-08-03T00:00:00Z")
            .unwrap();

        let item = store
            .items_for_account("acc-1")
            .unwrap()
            .into_iter()
            .find(|i| i.id == "item-1")
            .unwrap();
        assert_eq!(item.dismissed_updated_at, None);
        assert_eq!(item.dismissed_at, None);
        assert_eq!(item.dismissed_ci_status, None);
    }

    #[test]
    fn resurrecting_a_dismissed_item_preserves_its_watermark() {
        // The poller's upsert on an `Updated` item is what reopens a
        // dismissed row (`should_preserve_local_state` in poller.rs only
        // skips the write for `Carried` items). That upsert must not wipe the
        // dismissal snapshot the detail pane needs to explain the return.
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store.upsert_item(&sample_item("item-1")).unwrap();
        store
            .set_dismissed("item-1", true, "2026-08-02T00:00:00Z")
            .unwrap();

        let mut updated = sample_item("item-1");
        updated.updated_at = "2026-08-05T00:00:00Z".into();
        updated.ci_status = CiStatus::Failing;
        store.upsert_item(&updated).unwrap();

        let item = store
            .items_for_account("acc-1")
            .unwrap()
            .into_iter()
            .find(|i| i.id == "item-1")
            .unwrap();
        assert_eq!(item.state, ItemState::Open, "the resurrecting upsert reopens the item, as the poller does for an Updated item");
        assert_eq!(
            item.dismissed_updated_at.as_deref(),
            Some("2026-08-01T00:00:00Z"),
            "watermark survives the resurrecting upsert"
        );
        assert_eq!(item.dismissed_ci_status, Some(CiStatus::Passing));
        assert_eq!(item.ci_status, CiStatus::Failing, "current status still reflects the fresh fetch");
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
        store.set_dismissed("dismissed-1", true, "2024-01-01T00:00:00Z").unwrap();

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
        store.set_dismissed("dismissed-keep", true, "2024-01-01T00:00:00Z").unwrap();
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

    // ---- pull requests -------------------------------------------------

    fn sample_pr(number: u64, state: PrState, head_ref: Option<&str>) -> PullRequestSummary {
        PullRequestSummary {
            account_id: "acc-1".into(),
            repo: "acme/api".into(),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/acme/api/pull/{number}"),
            author: "octocat".into(),
            roles: vec![PrRole::Authored],
            state,
            draft: false,
            ci_status: CiStatus::Passing,
            review_decision: ReviewDecision::Approved,
            unresolved_threads: 2,
            mergeable: Mergeability::Clean,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: format!("2026-01-0{}T00:00:00Z", number.min(9)),
            head_ref: head_ref.map(str::to_owned),
        }
    }

    fn store_with_account() -> Store {
        let store = Store::open_in_memory().unwrap();
        store.upsert_account(&sample_account()).unwrap();
        store
    }

    /// Every field must survive the SQLite round-trip — the enum and role
    /// columns go through serde, so a rename in the proto would silently
    /// change what is stored if this stopped checking equality.
    #[test]
    fn pull_requests_round_trip_through_the_store() {
        let store = store_with_account();
        let pr = sample_pr(1, PrState::Open, Some("feat/login"));
        store.upsert_pull_requests(&[pr.clone()], "t1").unwrap();

        let listed = store.list_pull_requests(None, None).unwrap();
        assert_eq!(listed, vec![pr.clone()]);

        // Re-syncing the same PR updates it in place rather than duplicating.
        let mut updated = pr.clone();
        updated.title = "renamed".into();
        store.upsert_pull_requests(&[updated.clone()], "t2").unwrap();
        assert_eq!(store.list_pull_requests(None, None).unwrap(), vec![updated]);
    }

    #[test]
    fn listing_pull_requests_filters_by_account_and_state() {
        let store = store_with_account();
        store
            .upsert_pull_requests(
                &[
                    sample_pr(1, PrState::Open, Some("feat/a")),
                    sample_pr(2, PrState::Merged, Some("feat/b")),
                ],
                "t1",
            )
            .unwrap();

        let open = store.list_pull_requests(None, Some(PrState::Open)).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].number, 1);

        assert_eq!(store.list_pull_requests(Some("acc-1"), None).unwrap().len(), 2);
        assert!(store.list_pull_requests(Some("acc-2"), None).unwrap().is_empty());
    }

    /// The worktree join: keyed by head branch, merged only, most recent
    /// merge winning when a branch was reused.
    #[test]
    fn merged_prs_are_keyed_by_head_branch() {
        let store = store_with_account();
        store
            .upsert_pull_requests(
                &[
                    sample_pr(1, PrState::Merged, Some("feat/login")),
                    sample_pr(2, PrState::Open, Some("feat/open")),
                    sample_pr(3, PrState::Closed, Some("feat/abandoned")),
                    // Same branch merged twice; the later PR wins.
                    sample_pr(4, PrState::Merged, Some("feat/login")),
                    // No head branch at all — must not panic or key on NULL.
                    sample_pr(5, PrState::Merged, None),
                ],
                "t1",
            )
            .unwrap();

        let merged = store.merged_prs_by_head("acme/api").unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged["feat/login"].number, 4);
        assert!(!merged.contains_key("feat/open"));
        assert!(!merged.contains_key("feat/abandoned"));

        assert!(store.merged_prs_by_head("acme/other").unwrap().is_empty());
    }

    /// A PR that left the open set between syncs must not linger as open.
    #[test]
    fn stale_open_prs_are_dropped_but_settled_ones_are_kept() {
        let store = store_with_account();
        store
            .upsert_pull_requests(
                &[
                    sample_pr(1, PrState::Open, Some("feat/a")),
                    sample_pr(2, PrState::Open, Some("feat/b")),
                    sample_pr(3, PrState::Merged, Some("feat/c")),
                ],
                "t1",
            )
            .unwrap();

        // Second sync sees only #1; #2 vanished, #3 is settled so untouched.
        store
            .upsert_pull_requests(&[sample_pr(1, PrState::Open, Some("feat/a"))], "t2")
            .unwrap();
        assert_eq!(store.drop_stale_open_prs("acc-1", "t2").unwrap(), 1);

        let numbers: Vec<u64> = store
            .list_pull_requests(None, None)
            .unwrap()
            .iter()
            .map(|p| p.number)
            .collect();
        assert_eq!(numbers.len(), 2);
        assert!(numbers.contains(&1) && numbers.contains(&3));
    }

    #[test]
    fn pruning_removes_old_settled_prs_only() {
        let store = store_with_account();
        let mut old_open = sample_pr(1, PrState::Open, None);
        old_open.updated_at = "2020-01-01T00:00:00Z".into();
        let mut old_merged = sample_pr(2, PrState::Merged, None);
        old_merged.updated_at = "2020-01-01T00:00:00Z".into();
        store
            .upsert_pull_requests(&[old_open, old_merged, sample_pr(3, PrState::Merged, None)], "t1")
            .unwrap();

        assert_eq!(store.prune_pull_requests("2025-01-01T00:00:00Z").unwrap(), 1);
        let numbers: Vec<u64> = store
            .list_pull_requests(None, None)
            .unwrap()
            .iter()
            .map(|p| p.number)
            .collect();
        assert!(numbers.contains(&1), "an open PR must survive pruning by age");
        assert!(numbers.contains(&3));
    }

    #[test]
    fn meta_values_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.get_meta("prs_synced_at").unwrap().is_none());
        store.set_meta("prs_synced_at", "t1").unwrap();
        store.set_meta("prs_synced_at", "t2").unwrap();
        assert_eq!(store.get_meta("prs_synced_at").unwrap().as_deref(), Some("t2"));
    }
}

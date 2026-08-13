//! SQLite state store (`specs/daemon.md`). Owns `accounts`, `items`, and the
//! per-endpoint `etags` cache used to make no-change polls nearly free
//! (`specs/github-integration.md`). `history`/`ai_reports` tables are added
//! in the phases that use them (Phase 5, Phase 8) rather than declared here
//! unused — schema grows with the feature that needs it.
//!
//! A single [`Store`] wraps one `rusqlite::Connection` behind a `Mutex`
//! (SQLite serializes writers anyway; this avoids a connection pool for a
//! workload that's a handful of queries per minute).

use std::path::Path;
use std::sync::Mutex;

use gitsurveil_proto::{AccountRef, ActionItem, AuthKind, CiStatus, ItemKind, ItemState};
use rusqlite::{params, Connection};

use crate::error::Result;

const SCHEMA_VERSION: i64 = 1;

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
                raw_kind       TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_items_account ON items(account_id);
            CREATE INDEX IF NOT EXISTS idx_items_state ON items(state);
            CREATE TABLE IF NOT EXISTS etags (
                account_id TEXT NOT NULL,
                endpoint   TEXT NOT NULL,
                etag       TEXT NOT NULL,
                PRIMARY KEY (account_id, endpoint)
            );
            ",
        )?;
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

    // ---- items ---------------------------------------------------------

    /// Replaces the stored row for `item.id` (insert or full overwrite).
    /// Used by the poller after computing a diff — the diff itself is pure
    /// and doesn't touch storage (see `crate::github::diff`).
    pub fn upsert_item(&self, item: &ActionItem) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO items (
                id, account_id, kind, state, repo, number, title, url, author,
                created_at, updated_at, first_seen_at, last_seen_at, ci_status, raw_kind
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                title = excluded.title,
                url = excluded.url,
                updated_at = excluded.updated_at,
                last_seen_at = excluded.last_seen_at,
                ci_status = excluded.ci_status",
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

    /// All items currently stored for `account_id`, regardless of state —
    /// the poller diffs against this full set (`specs/github-integration.md`).
    pub fn items_for_account(&self, account_id: &str) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status, raw_kind
             FROM items WHERE account_id = ?1",
        )?;
        let rows = stmt.query_map(params![account_id], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Open (non-done, non-dismissed) items across all accounts, for the
    /// `items.list` API method and the `status` open-item count.
    pub fn open_items(&self) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status, raw_kind
             FROM items WHERE state = 'open'",
        )?;
        let rows = stmt.query_map([], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Resolved and dismissed items, newest activity first — the desktop UI's
    /// history view (`specs/desktop-ui.md`).
    pub fn history_items(&self, limit: usize) -> Result<Vec<ActionItem>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, account_id, kind, state, repo, number, title, url, author,
                    created_at, updated_at, first_seen_at, last_seen_at, ci_status, raw_kind
             FROM items WHERE state != 'open'
             ORDER BY last_seen_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_item)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
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
}

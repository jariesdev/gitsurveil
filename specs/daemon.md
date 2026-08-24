# Daemon (`gitsurveild`)

## Overview

The background service that owns all state and side effects. Single Rust
binary, no framework — tokio runtime plus libraries. UIs are thin clients
over its local API.

## Goals

- Runs at login with no UI; survives app quit; restarts on crash.
- Idle RSS < ~15 MB. No-change poll cycles cost near-zero CPU and quota.
- Every capability reachable over the local API — the daemon is fully usable
  headless (curl-able in dev).

## Non-goals

- Any network listener. Local socket/pipe only.
- Multi-user daemons; one instance per OS user.

## Lifecycle

| OS | Registration | Restart policy |
|---|---|---|
| macOS | launchd user agent `io.gitsurveil.daemon.plist` | `KeepAlive: true` |
| Linux | systemd user unit `gitsurveild.service` | `Restart=on-failure` |
| Windows | `Run` registry key (user) | app spawns the daemon directly on launch |

- `gitsurveild --foreground` — run attached to a terminal (dev, headless).
- `gitsurveild install|uninstall|status` — manage registration (also invoked
  by the app on every launch; idempotent).
- Single-instance enforced by an exclusive lock on the socket path.
- Graceful shutdown on SIGTERM: finish writing state, close socket.
- Crash recovery: all durable state in SQLite; restart re-polls and converges.

## Storage

SQLite (`rusqlite`, WAL mode) at the platform data dir
(`~/Library/Application Support/gitsurveil/`, `~/.local/share/gitsurveil/`,
`%APPDATA%\gitsurveil\`):

| Table | Contents |
|---|---|
| `accounts` | account rows (NO tokens — keychain only) |
| `items` | current `ActionItem`s + local state (dismissed, first_seen) + the `activity` fingerprint and `archived` tombstone columns (daemon-internal; never cross IPC) + the `dismissed_updated_at`/`dismissed_at`/`dismissed_ci_status` dismissal-watermark columns (cross IPC — the desktop detail pane reads them, `github-integration.md` § Dismissal watermark). Resolved/dismissed items are history — rows with `state IN ('done','dismissed')` and `archived = 0`, read via `items.history`, archived via `items.clear_history` (no ring-buffer pruning yet) |
| `repositories` | the discovered repo catalog per account (tracked, new-ack) |
| `orgs` | distinct organizations per account, for catalog filtering |
| `clone_jobs` | background `repos.clone` jobs (target, progress, result) |
| `apps` | registered "Open with…" applications (`name`, `command` keyed unique) |
| `etags` | conditional-request cache per endpoint |
| `ai_reports` | AI review summaries keyed by item id |
| `meta` | schema version for migrations |

Settings and priority rules live in a TOML config file next to the DB
(human-editable, hot-reloaded); the API can write it.

## Local API

Transport: unix domain socket / Windows named pipe, newline-delimited JSON.
Every message: `{ "id": n, "method": "...", "params": {...} }` →
`{ "id": n, "result": ... }` or `{ "id": n, "error": { code, message } }`.

### Methods

| Method | Params | Result |
|---|---|---|
| `status` | – | version, uptime, per-account poll state, rate-limit remaining |
| `items.list` | filter (kind/repo/severity/state), group_by | items with scores + severity |
| `items.history` | limit | resolved + dismissed items, newest first (archived items excluded) |
| `items.clear_history` | – | ok — archives every resolved/dismissed item (sets `archived = 1`); open items are untouched. Archived items are invisible in the Dashboard *and* history, and the poller never resurrects them — so a dismissed item that is still open on GitHub cannot come back. There is no undo (UI confirms first) |
| `items.dismiss` / `items.undismiss` | item id | ok |
| `items.mark_read` | item id | ok (propagates to GitHub notifications API) |
| `accounts.list` / `accounts.add` / `accounts.remove` / `accounts.update_token` | … | account rows / validation result |
| `rules.list` / `rules.set` | rules | ok (writes config) |
| `notifications.prefs` | – | `KindPref[]` — every `ItemKind`'s notification preference, enabled by default (`notifications.md` § Preferences) |
| `notifications.set_pref` | kind, enabled | ok — gates only the OS notification/tray interruption for that kind, not Dashboard/history visibility |
| `settings.get` / `settings.set` | key/values | ok |
| `poll.now` | account id? | triggers immediate poll |
| `pr.create` / `pr.update` / `pr.close` / `pr.merge` | see `pr-management.md` | result |
| `pr.comments` / `pr.comment_reply` / `pr.resolve` | pr ref / thread id / body | `Conversation` / comment / { resolved } — see `pr-management.md` |
| `pr.branches` / `pr.labels` | repo | `Vec<String>` for the create/edit pickers |
| `prs.list` | account_id?, state (open/closed/merged) | `PullRequestSummary[]` — live query, one GraphQL request per account, concatenated; not stored, never polled on a timer. Fetched on view open/refilter only. |
| `repos.list` | – | `RepoCatalog` — every discovered repo + orgs per account, with tracked state and clone path. See `github-integration.md` for how discovery fills it. |
| `repos.set` | repo (full_name), path | the updated `Repository` — registers an existing local clone; validates it's a git repo whose remote is that repo |
| `repos.set_notify` | account_id, repo (full_name), enabled | the updated `Repository` — sets `notify_enabled`, gating whether the repo's items feed notifications and `prs.list`. Independent of clone tracking; see `github-integration.md` § Notification scope |
| `repos.remove` | repo (full_name) | ok — forgets the registered clone path (does **not** delete files) |
| `repos.new` | – | `Repository[]` the user hasn't acknowledged yet (tracked=0 AND notified_at IS NULL) |
| `repos.ack_new` | first_seen_at (dismissal watermark) | `u64` count acked — marks every new repo at or before the watermark as acknowledged |
| `repos.refresh` | – | `RepoCatalog` — triggers an immediate discovery pass, skipping when the account's core quota is below `MIN_CORE_REMAINING` (200) |
| `repos.clone` | repo (full_name), path | `job_id` — starts a background HTTPS clone into `path`; returns immediately, progress via `repos.clone_status`. The daemon never deletes pre-existing files: it refuses a non-empty target, and on failure removes only a target it created |
| `repos.clone_status` | job_id | `CloneStatus` (running/done/failed + bytes received; `total` stays 0 — git2 can't predict pack size, the UI shows an indeterminate bar) or `null` when the job is unknown |
| `repos.worktrees` | repo (full_name) | `WorktreesResult` — the clone's user-created worktrees (name, path, checked-out branch, short head id) plus the branches a new one can be created from (local names; remote names deduped to their short form). Derived from the clone's git metadata on every call, so worktrees made or removed outside GitSurveil show up too. `gitsurveil-*` conflict-resolver sessions are excluded. Requires a registered clone path (`config_error` otherwise) |
| `repos.worktree_add` | repo (full_name), branch, path | the new `WorktreeInfo` — creates a worktree for `branch` at `path` (absolute, or relative to the clone's parent). `branch` may be an existing local branch, an `origin/` remote branch (a local tracking branch is created), or a brand-new name (created at the clone's HEAD). Refuses a non-empty existing target, the clone's own path, and a branch already checked out elsewhere — nothing pre-existing is ever touched. On checkout failure the worktree and any newly created branch are rolled back |
| `repos.worktree_remove` | repo (full_name), name | ok — unregisters the worktree and removes its working directory, **keeping the branch**. Refuses dirty worktrees (uncommitted changes or untracked files) and `gitsurveil-*` conflict-session worktrees |
| `apps.list` | – | `RegisteredApp[]` — the registered "Open with…" applications (`name`, `command`), sorted by name, case-insensitive |
| `apps.add` | name, command | the new `RegisteredApp` — registers an application for the worktree "Open with…" menu. `command` is a single whitespace-free token: an executable name resolved on `PATH` or an absolute path to one (no flags, args, or NUL); rejects a command already registered |
| `apps.remove` | command | ok — forgets a registered application. Idempotent (unknown command is not an error) |
| `apps.open` | command, path | ok — the daemon spawns `command <path>` (no shell; `cmd /C` on Windows). Refuses an unregistered command and a NUL in `path`. Spawn failure is a `config_error` telling the user to make sure the executable is on `PATH`. This is the daemon-owned side effect behind the worktree "Open with…" submenu |
| `conflicts.prepare` | repo, number, account_id? | session + conflicted file list |
| `conflicts.file` | session_id, path | conflict segments |
| `conflicts.save` | session_id, path, content? \| pick? | ok |
| `conflicts.commit` | session_id, message | ok |
| `conflicts.push` | session_id | ok (tears session down) |
| `conflicts.abort` | session_id | ok (idempotent) |
| `ai.review` / `ai.report` / `ai.summary` | item id / type | report / digest |
| `automation.features` | – | `Vec<AutomationFeature>` — available automations with status |
| `automation.settings` | – | `AutomationSettings` — global force push, concurrency, master toggle |
| `automation.set_settings` | force_push_enabled?, max_concurrent_rebases?, global_enabled? | `AutomationSettings` |
| `automation.rebase.configs` | – | `Vec<AutoRebaseConfig>` — per-repo auto-rebase settings |
| `automation.rebase.set_config` | repo, enabled?, mode?, base_branch?, head_branches?, branch_overrides?, authored_only? | `AutoRebaseConfig` |
| `automation.rebase.state` | – | `AutomationState` — configs + active + recent + failed |
| `automation.rebase.dismiss_failure` | id | ok |
| `automation.rebase.cancel` | id | ok |
| `automation.rebase.clear_log` | – | ok |
| `automation.rebase.trigger` | – | `{ log: Vec<AutoRebaseEntry> }` — runs detection immediately |

### Event stream

A client sends `{ "method": "subscribe" }` and then receives pushed events on
the same connection:

| Event | Payload |
|---|---|
| `items.changed` | added / updated / resolved item ids (post-diff) |
| `severity.changed` | new top severity (drives tray color) |
| `poll.status` | polling / ok / throttled / auth_error per account |
| `ai.report_ready` | item id |
| `automation.rebase.started` | id, repo, head_branch, base_branch, attempt |
| `automation.rebase.completed` | id, repo, head_branch, base_branch, attempt |
| `automation.rebase.failed` | id, repo, head_branch, base_branch, error, attempt |
| `automation.rebase.progress` | id, repo, head_branch, phase (fetching/rebasing/pushing) |
| `automation.rebase.retrying` | id, repo, head_branch, attempt, max_attempts, reason |
| `automation.rebase.cancelled` | id, repo, head_branch |

Protocol types are defined once in a shared Rust crate (`gitsurveil-proto`)
used by both daemon and Tauri shell; TypeScript types generated from it so
UI and daemon can't drift.

## Notifications dispatch

The daemon fires native desktop notifications itself (`notify-rust`) per the
gate in `priority-engine.md` — see `notifications.md` for content/UX. This is
why alerts work with zero UI processes.

## Error handling principles

- Network/API failures degrade to stale data + a `poll.status` event; the app
  shows staleness, the daemon never crashes on a bad response.
- All errors logged (rotating file log, `tracing`); log level configurable.
- Keychain unavailable (locked session): retry with backoff, surface status.

## Open questions

- Named pipe ACL details on Windows — verify default DACL restricts to the
  current user (expected) during Phase 1.

# Desktop UI (main window)

## Overview

The full application window: dashboard, PR management, conflict resolver,
history, rule editor, accounts, settings. A thin client — every action goes
through the daemon API; the window can be closed at any time without affecting
monitoring.

## Goals

- Dashboard makes triage fast: grouped, filterable, searchable.
- All PR work (see `pr-management.md`, `conflict-resolver.md`) reachable from
  the item you were notified about — no context switch to the browser unless
  you want one.

## Non-goals

- A general GitHub browser. Scope is the user's action items and their PRs.

## Onboarding

First-run orientation for a fresh install. While `accounts.list` reports no
account at all, the window opens to a full-window welcome screen
(`Onboarding.tsx`) instead of the shell: a short pitch of what GitSurveil does,
and the same add-account form the Accounts view uses (`AccountForm.tsx`),
front and center.

- Detection is pure UI state from the existing `accounts.list` call — the
  daemon and its API are untouched.
- **Skip for now** hides the screen for the current window session only. It is
  not persisted and adds no daemon state: with no account configured the next
  window open shows it again, because a user who never added an account still
  needs this orientation.
- Adding an account from the welcome screen reloads the window's data; once an
  account exists the onboarding screen stops rendering and the window lands on
  the Dashboard.

## Layout

Sidebar navigation: **Dashboard · Pull Requests · History · Rules ·
Repositories · Automation · Accounts · Settings**.

### Dashboard (default view)

- Notifications **grouped by priority (default)** — Critical / High / Normal /
  Info sections — with a toggle to **group by type** (Reviews, CI failures,
  Mentions, Assigned, Participating).
- Row: severity dot, kind, repo#number, title, author, age, CI chip, AI badge
  if a report exists. Right-click → context menu with **Copy URL** (same in
  History; the popover's own row menu mirrors it — `menubar-ui.md`).
- Row click → detail panel: description, comment threads, CI checks, AI report
  tab (if any), and actions (open on GitHub, dismiss, and the PR actions from
  `pr-management.md` when the item is a PR). The description and all comment
  bodies render as sanitized markdown (`src/markdown.ts`).
- Detail conversation pane: issue comments, per-file review threads with a
  **Resolved** badge and a Resolve/Unresolve toggle, reply-in-thread, and a
  top-level comment box (`pr-management.md`). The reply box opens already
  focused; Shift+Enter posts, Esc cancels, and a bare Enter is a newline.
- The PR detail pane's **Edit** form edits title, description, target branch
  (with repo-branch suggestions), labels, and the draft flag inline; only
  changed fields are written back (`pr-management.md`).
- **Returned-item banner**: when the opened item was previously dismissed
  (`dismissed_updated_at` is set — `github-integration.md` § Dismissal
  watermark), a banner sits above the description naming what changed since
  then: new issue comments (with authors), review threads with new replies,
  and a CI passing→failing transition. When none of those apply — e.g. the
  item returned over a label edit, which the daemon doesn't track — the
  banner says so plainly ("updated on GitHub … no new comments") rather than
  showing nothing or implying a change that isn't there. Each comment posted
  after the watermark also carries an inline **New** badge and accent border
  in the conversation section, so the user can scan straight to it. The
  banner clears when the item is re-dismissed or restored from History (both
  reset the watermark) — there is no separate acknowledge action.
- Filter bar: text search, account, **repo (multi-select dropdown with checkboxes, repos derived from visible items after account/type/severity filters)**, kind, severity, show-dismissed.
- With no account configured at all (onboarding skipped for the session), the
  empty state reads "No account yet — add one to start monitoring." with an
  **Add account** button that jumps to the Accounts view. The "Nothing needs
  your attention." / "No items match these filters." states are unchanged.
- Every window stays in sync: the Rust shell emits `items-changed` after any
  `items.dismiss`/`items.undismiss` command, and **all** open windows (this one
  and the popover) refetch from that single event — no window refreshes on its
  own action. Dismissing in the popover drops the item from an open Dashboard
  immediately; restoring a dismissed item in History brings it back in the
  Dashboard and the popover at the same time.
- When AI is enabled: per-type digest cards at the top (same summaries as the
  popover, see `ai-review.md`).

### Pull Requests

The user's PRs across every configured account — **standing state**, not an
event inbox. Where the dashboard shows what pings you, this shows what's in
flight: draft, review decision, CI, and mergeability as they are *right now*.

- Data is a live query (`prs.list`, `github-integration.md`): one GraphQL
  request per account, made when the view opens or the Status filter changes.
  Nothing here is stored, and there is **no polling while the view is open**.
- Results are filtered daemon-side to repos with `notify_enabled = true`
  (`github-integration.md` § Notification scope), so a repo unchecked in the
  Accounts checklist never appears here, regardless of clone-tracking state.
- Rows: title · `repo#number` · author · account (when several) · role badges ·
  draft badge · CI chip · review decision · conflict warning · state badge ·
  unresolved-review badge · age. Sorted by most recently updated.
- Filters — Status is the only daemon-side one (it changes the GraphQL search
  qualifier); the rest filter in-memory in the webview:
  - **Status** — Open (default) / Closed / Merged / All. Re-queries the daemon.
  - **Account** — all / each configured account.
  - **Repository** — all / each repo present in the results.
  - **Role** — all / authored / review requested / assigned.
  - **Attention** — all / draft / conflicted / CI failing / approved.
  - **Search** — title + repository, case-insensitive.
- Row click opens the same `PrDetail` pane as the dashboard; conflicted rows
  carry an inline **Resolve conflicts** action opening the same
  `ConflictResolver`. Both routes share one handler — the view never
  reimplements PR management or resolution. The row whose detail pane is open
  is highlighted (neutral selected background, `aria-current`); closing the
  pane drops the highlight.
- Right-clicking a row opens a context menu with **Open in GitHub** (named
  after the PR's provider — GitHub, GitLab, or the account's host), which
  opens the PR on GitHub in the default browser (the same `openUrl` path the
  dashboard rows and the detail pane's "Open on GitHub" use), and **Copy URL**.
- A chat-bubble badge with a number shows how many **unresolved review
  threads** the PR has (counted daemon-side from the GraphQL
  `reviewThreads` fragment); it is hidden when there are none.
- **Resolve conflicts** needs a registered local clone (a tracked row in the
  `repos.list` catalog). Without one, the row still shows the action but
  explains why it can't run and offers a jump to the Repositories tab — never
  a silent miss or an obscure error.
- `Mergeability::Unknown` means GitHub is still computing mergeability and is
  **never** treated as conflicted.

### Repository and Worktrees

The catalog of every repository discovered across the user's accounts —
standing state owned by the daemon (`github-integration.md`), rendered from
`repos.list`.

- **New-repositories modal** (`NewReposModal`): on main-window open the app
  asks `repos.new` **once** (never on every refresh) and shows a dismiss-all
  modal when anything is unacked. The single action acks the whole batch
  (`repos.ack_new`); per-repo decisions belong in the pane, so the modal has
  no per-row controls. It appears only on window open — dismissing it means
  "don't interrupt this session again".
- Rows: `owner/name` · **Private** chip · account chip (only with multiple
  accounts) · description · clone path or "No local clone". A **single click
  anywhere on the row** toggles the worktree panel; a **double click** opens
  the repo in the browser. The expand chevron and the `⋯` actions button stop
  propagation, so they don't toggle the panel themselves (the chevron still
  toggles on its own single click as an affordance).
- **Account + Organization filters**, persisted via `usePersistentState` and
  revived defensively — an account or org that no longer exists is dropped
  rather than restoring an empty list; changing account clears the org. A
  footer shows how many rows a filter hides and offers a clear action.
- Right-click menu:
  - **Open in browser** — the repo on GitHub (same `openUrl` path as
    everything else).
  - **Clone to…** — native folder picker (`@tauri-apps/plugin-dialog`), then a
    background clone (`repos.clone`). The daemon only ever creates the chosen
    target when it is absent and, on failure, only removes a target it
    created. If the folder already exists with content, the clone is refused
    and the existing files are never deleted — pick an empty or new folder.
    The row shows an **indeterminate** progress bar plus a running byte count
    while it runs; git2 can't predict the pack size, so there's no fraction.
  - **Pick existing clone…** — native folder picker, then `repos.set`: the
    daemon validates the path (a git repo whose `origin` points at this repo)
    and records the mapping. This is a map-only operation — the daemon never
    writes to or deletes anything in the chosen folder; the user keeps full
    ownership of their local checkout.
  - Tracked rows: **Change clone path…** and **Remove clone path** (the latter
    forgets the path — it never deletes files).
- A failed clone keeps its error on the row with **Retry clone** and **Pick
  existing clone…** actions (same map-only `repos.set` path). Clone progress is polled about once a second via
  `repos.clone_status`; **Refresh** in the header forces a discovery pass
  (`repos.refresh`).
- **Worktrees**: tracked rows with a registered clone path get an expand
  chevron. Expanding loads `repos.worktrees` (lazily, once per expand) and
  shows the repo's **user-created worktrees** — `git worktree list` minus the
  `gitsurveil-*` conflict-resolver sessions. Each row shows the checked-out
  branch (bold), the path (muted), and a 7-char head id; right-click a row for
  **Open with…** (a submenu of the registered apps from Settings — each opens
  the worktree path with `command <path>`, spawned by the daemon via
  `apps.open`) and **Delete worktree** (`repos.worktree_remove`). Deleting unregisters the
  worktree, removes its directory, and **keeps the branch**; the daemon refuses
  dirty worktrees and conflict sessions. Data is read live from the clone on
  every expand, so worktrees created or removed outside GitSurveil show up too.
  The panel ends with an inline add form (`repos.worktree_add`): the branch
  field is a combobox (`<input list>` + `<datalist>` of the branches from
  `repos.worktrees`) that also accepts a brand-new name, which the daemon
  creates at the clone's HEAD; the target path is prefilled with
  `wt-{owner}-{name}-{branch}` next to the clone and keeps tracking the branch
  until the user edits it by hand. Relative paths resolve next to the clone.
  The daemon refuses a non-empty target, the clone's own path, and a branch
  already checked out elsewhere; nothing pre-existing is ever touched. A
  successful add reloads the worktree list and the catalog.
- Empty states distinguish "no accounts yet" (add one first), "no repos
  discovered" (offers Scan now), and "filters match nothing".

### History

Resolved/dismissed items (no ring-buffer pruning yet), same filters, read-only
except for two actions: **Restore** on dismissed rows (`items.undismiss`) and
**Clear all history** in the header (`items.clear_history`), which archives
every resolved/dismissed item after a confirm dialog — open items are
untouched and there is no undo.

"Archives" is literal: cleared rows are not deleted. A dismissed item is still
open on GitHub, so deleting it would let the next poll re-add it to the
Dashboard; archiving keeps it invisible in both the Dashboard and history, and
  the daemon never resurfaces it, even when GitHub reports new activity on it.

### Automation

Lists available background automations (currently just **Auto Rebase**). Each
feature is a clickable row showing name, description, and enabled/disabled
status. Clicking a row drills into its dedicated page; a back arrow returns to
the list. The Automation view uses a `currentView` state to switch between the
feature list and individual feature pages.

#### Auto Rebase Page

Sections top to bottom:

1. **Failed Auto-Rebases** — repo, head→base, timestamp, exclamation icon
   with tooltip (reason), "Resolve manually" link, Dismiss button.
2. **Per-Repo Configuration** — for each tracked repo: repo name, enable
   toggle, mode radio buttons (PR-based / Branch-based), base branch dropdown
   (branch mode), head branches checkbox list (branch mode), branch overrides
   table, git commands preview (expandable).
3. **Active Rebases** — running rows with progress bar, phase label, attempt
   counter ("Attempt 2/3"), cancel button.
4. **Recent History** — last 20 entries, expandable git commands, "Clear log"
   button.
5. **Settings** — global auto-rebase master toggle, force push toggle (off by
   default), max concurrent rebases number input (default 3, range 1–10).

### Rules

Graphical editor for priority rules (`priority-engine.md`): list with enable
toggles, match-condition builder, effect editor, drag ordering, live preview
("this rule currently matches N open items"). Writes via `rules.set`; the
daemon's TOML config remains the source of truth.

### Accounts

Add/remove/update accounts: PAT paste (with scope validation feedback),
token rotation, and OAuth device-flow walkthrough; per-account poll status
and rate-limit remaining.

- **Add form** (`AccountForm.tsx`, shared with the onboarding screen): a
  **Provider** dropdown of the supported forges (GitHub / GitHub Enterprise
  Server). GitHub needs only the token and fixes `host` to `github.com`;
  Enterprise reveals **Enterprise host** and **API base URL** fields instead of
  a free-text host input. Everything still round-trips through `accounts.add`
  unchanged. The token field carries a collapsible **Where do I get a
  token?** helper — classic vs. fine-grained PAT guidance, the `notifications`
  and `repo` scopes, and the keychain note — plus a **Create a token on
  GitHub** button (`openUrl` to `github.com/settings/tokens`).

- **Update token** (`UpdateTokenButton` in `Accounts.tsx`): each account row
  has an **Update token** button that expands an inline form with just a new
  PAT field. The host and API base are unchanged so only the token needs to
  be re-entered. The new token is validated against GitHub (`GET /user`)
  before it replaces the old one in the OS keychain via `accounts.update_token`.
  A login refresh runs in case the new token belongs to a different user.

- **Notify checklist**: each account with discovered repos gets a collapsed
  checklist (`repos.list` catalog, filtered to that account) — one checkbox
  per repo, bound to `notify_enabled` and toggled via `repos.set_notify`
  (`github-integration.md` § Notification scope). Unchecking a repo removes
  it from notifications, the Dashboard, and the Pull Requests view
  immediately; it does not affect a registered local clone. New repos default
  checked, so nothing already notifying goes silent without an explicit
  uncheck.

### Settings

Poll interval, notification preferences and quiet hours (`notifications.md`),
launch-at-login, AI review toggle + provider config (`ai-review.md`),
theme. Local clone paths are managed in the Repositories pane, not here.
The pane scrolls internally when it is taller than the window.

#### Notifications

A checklist, one checkbox per `ItemKind`, checked by default
(`notifications.prefs` / `notifications.set_pref`, `notifications.md` §
Preferences). Unlike the Accounts checklist's `notify_enabled`, unchecking a
kind here does **not** hide its items from the Dashboard or history — it only
stops the OS notification. Toggling is optimistic (checkbox flips
immediately) and rolls back with an inline error if the daemon call fails.

#### Applications

The "Open with…" apps offered on worktree context menus in the Repository and
Worktrees pane. Each row is a **Name** (what the menu shows) and an
**Application or Command** — an executable name on `PATH`, an absolute path to
one, or a path picked with the **Browse…** native file dialog; choosing one in
a menu makes the daemon run
`command <path>` (`apps.open`, no shell). The list self-loads from `apps.list`
on mount; the **Add application** button below the list opens a modal
(`AddAppModal.tsx`) with the name/command form — the daemon's validation
errors (e.g. "already registered") render inline and keep the modal open, and
per-row **Remove** (`apps.remove`) round-trips through the daemon's registry. A
parent "Open with…" menu item only renders when at least one app is registered.

## Data flow

- Same daemon socket client as the popover; subscribes to the event stream
  while open, so the dashboard live-updates on poll.
- Auto-refresh: both the dashboard and popover re-read items from SQLite
  every 5 seconds (`useInterval`). This only reads from the local database
  — no external API calls are made. Event-driven refreshes (`items-changed`)
  share a 500 ms debounce with the interval to coalesce rapid back-to-back
  calls. Initial mount and manual poll actions bypass the debounce.
- Search inputs in the Dashboard and Pull Requests view debounce at 300 ms:
  the input updates instantly for visual feedback, but the filter applied to
  the list updates only after 300 ms of inactivity.
- Window closed ≠ app quit: closing the main window leaves the tray app
  running; the main window's webview is likewise dropped when closed.

## Edge cases

- Daemon down: full-window state with "start service" action.
- Item resolved upstream while its detail panel is open: banner "resolved on
  GitHub", panel becomes read-only.
- View crash: the pane content is wrapped in an error boundary, so a render
  error (e.g. an old daemon answering a newer method's payload shape) shows an
  inline "This view failed to render" box with the error instead of a blank
  white window. The sidebar stays usable; the boundary is keyed by the current
  view, so navigating away and back (or "Back to dashboard") recovers.

## Verification

- Vitest component tests for grouping, filters (PR and repo), the repo filter
  revival, rule editor validation, the PR row context menu, the error
  boundary, the onboarding flow (welcome with no accounts, add-account round
  trip, session-only skip), and a full-window navigation smoke test that opens
  the Repositories view.
- Manual pass against a real account for each dashboard grouping mode.

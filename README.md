# GitSurveil

A GitHub action-item monitor that runs quietly in the background and tells you
only what actually needs your attention.

A local daemon polls GitHub, normalizes everything you're on the hook for —
review requests, assignments, mentions, failing CI — into one prioritized list,
and fires desktop notifications. A Pull Requests view shows your own PRs'
standing state (draft, reviews, CI, mergeability) and notifies you the moment
one is approved, green, and ready to merge. Nothing is hosted: your machine
talks to GitHub directly with your own token, and no data goes anywhere else.

## Status

**Early development.** Phases 1–7 and 9 of 9 are done: the daemon monitors
GitHub, prioritizes what it finds, and notifies you; a menubar app shows what's
pending; a desktop window provides the dashboard, history, rules, accounts,
pull-request management, and a three-pane conflict resolver that lets you merge
a conflicted PR from within the app. A **Repositories** pane discovers every
repository across your accounts, flags the ones you haven't seen yet, and can
clone any of them in the background — the local copies the conflict resolver
works on. The daemon also registers itself as a login service (a launchd agent
on macOS, a systemd user unit on Linux, the per-user `Run` key on Windows), so
monitoring starts at login whether or not the app is ever opened.

| Phase | Feature | Status |
|---|---|---|
| 1 | Core monitoring (poller, storage, local API) | ✅ Done |
| 2 | Desktop notifications | ✅ Done |
| 3 | Menubar app (tray + notifications popover) | ✅ Done |
| 4 | Priority engine (scoring, severity tray, outrank gate) | ✅ Done |
| 5 | Full desktop UI (dashboard, rules, accounts) | ✅ Done |
| 6 | PR management (create/update/close/merge, comments) | ✅ Done |
| 7 | Conflict resolver (3-pane, Sublime Merge-style) | ✅ Done |
| 8 | AI PR review (opt-in; Ollama or Claude) | Not started |
| 9 | Service registration & packaging | ✅ Done |

The packaged app ships the daemon inside it and registers/starts it on every
launch — idempotently, so a lost registration heals itself. `gitsurveild
status` reports whether it is registered and whether it is currently answering;
`gitsurveild install` and `gitsurveild uninstall` manage the registration by
hand.

## What it monitors

- Review requests waiting on you
- Pull requests and issues assigned to you
- Mentions
- Failing CI on your pull requests
- Your pull requests that become ready to merge (approved, green, not a draft)
- Comments from others, unresolved review threads, or failing CI on pull
  requests you opened
- Replies awaiting you in review threads you commented on

The last two are curated: a "your PR has activity" item exists only while a
comment from someone else, an unresolved thread, or a failing check is
actually there, and a "PR you reviewed" item only while someone is waiting on
your follow-up — both drop out of your list the moment that's no longer true.

Multiple accounts are supported, including GitHub Enterprise.

## How it decides what matters

Every item gets a score: a base value for its type, plus any rules you've
written, plus one point for every four hours it stays open (capped at 30, so
age nudges old review requests up without ever drowning out a real emergency).

| Type | Base score |
|---|---|
| Failing CI on your PR | 100 |
| Review requested | 80 |
| Changes requested on your PR | 70 |
| Ready to merge (approved, green, not a draft) | 65 |
| Mentioned | 50 |
| PR you reviewed, waiting on your reply | 45 |
| Assigned | 40 |
| Your PR needs attention (comments/unresolved threads/failing CI) | 30 |
| Participating | 20 |

Scores map to severity bands — critical, high, normal, info — which set the
tray icon color and group the dashboard.

The part that matters: **you're only interrupted when something outranks what
was already at the top of your list.** Everything else lands silently and shows
up in the tray color instead. A failing build is the one exception and always
interrupts, because it usually blocks other people too.

Rules live in `config.toml` in the data directory and can add or subtract
points, pin an item's severity, or mute its notifications. Muting silences an
item without hiding it — it still lists, and still counts toward the tray
color. By default, "participating" threads are muted.

## Architecture

```
gitsurveild (Rust daemon) ── polls GitHub, owns all state,
                             sends notifications, serves a local
                             JSON API over a unix socket
        ▲
GitSurveil (Tauri v2 app) ── tray icon + notifications popover,
                             plus the full desktop window
```

The daemon owns everything stateful; the app only renders and forwards intent.
Quitting the app doesn't stop monitoring. The daemon never listens on a network
port, and tokens live only in the OS keychain — never in the database, config
files, or logs.

Webviews are hidden, not destroyed, when their window is dismissed — the
popover stays warm between tray clicks so it opens instantly. A background task
destroys the popover's webview after it has sat hidden for an idle timeout, so
an abandoned popover eventually costs nothing.

## Install a release build

Download the file for your platform from the
[releases page](../../releases): a `.dmg` for macOS (Apple Silicon or Intel),
`.AppImage`/`.deb` for Linux, `.msi` for Windows.

These builds are not code-signed, so the first launch is blocked:

- **macOS** — right-click the app, choose **Open**, then confirm.
- **Windows** — SmartScreen: **More info → Run anyway**.

The background service ships inside the app. Launching the app registers and
starts it automatically (every launch, idempotently). To manage the
registration by hand — e.g. to have monitoring start at login before you ever
open the app:

```bash
# macOS, after dragging GitSurveil.app to /Applications
/Applications/GitSurveil.app/Contents/MacOS/gitsurveild install
```

`gitsurveild status` reports whether it is registered and whether it is
currently answering; `gitsurveild uninstall` removes the registration.

## Build from source

## Requirements

- **Rust** (stable). Install via [rustup](https://rustup.rs):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js 20+ and pnpm**, to build the app's frontend.
- **macOS or Linux.** The Windows named-pipe transport is written but not yet
  verified on real Windows hardware.
- A **GitHub personal access token** with the `notifications` and `repo` scopes.

## Install

```bash
git clone <repository-url>
cd gitsurveil
pnpm install

# The app must be built through the Tauri CLI, which embeds the frontend
# bundle. A plain `cargo build` produces a binary that still points at the
# dev server, so its popover comes up blank.
pnpm tauri build --config crates/gitsurveil-app/tauri.conf.json --no-bundle

# The daemon builds OpenSSL from source (git2's vendored-openssl feature), so
# no system OpenSSL headers are needed on any platform.
cargo build --release -p gitsurveild
```

To produce an installable bundle instead, stage the daemon as a sidecar first
so it ships inside the app, then bundle:

```bash
./scripts/bundle-daemon.sh
pnpm tauri build --config crates/gitsurveil-app/tauri.conf.json
```

Two binaries land in `target/release/`: `gitsurveild` (the daemon) and
`gitsurveil` (the menubar app). Drop `--no-bundle` to also produce a `.app`
and installer.

## Usage

### 1. Start the daemon

```bash
cargo run -p gitsurveild -- --foreground
```

It stays attached to the terminal and logs what it's doing. Leave it running.
This is the development path; in a packaged install the daemon registers and
starts itself at login (see "Install a release build").

### 1b. Stop the daemon

Foreground mode stops with `Ctrl+C` (or by closing the terminal). If the
daemon was installed as a login service instead (`gitsurveild install`), stop
it with:

```bash
launchctl bootout gui/$(id -u)/io.gitsurveil.daemon
```

or remove the registration entirely with `gitsurveild uninstall`.

### 2. Add your GitHub account

Easiest from the app: run it (step 3), open the window, and use **Accounts** —
pick your provider (GitHub or GitHub Enterprise Server), paste the token, and
add. GitHub needs nothing else; Enterprise additionally asks for its host and
API base URL.

The first time you open the window with no accounts configured, a welcome
screen appears instead of the dashboard: a quick pitch of what GitSurveil does,
with the add-account form right there — paste a token and you're monitoring.
**Skip for now** hides it for that session; with no account configured it comes
back the next time you open the window. The **Accounts** view offers the same
form, with a collapsible "Where do I get a token?" helper and a direct link to
the GitHub token page.

To do it from the terminal instead, the daemon exposes a JSON API over a unix
socket:

```bash
SOCK="$HOME/Library/Application Support/io.gitsurveil.gitsurveil/daemon.sock"   # macOS
# SOCK="$XDG_RUNTIME_DIR/gitsurveil.sock"                                       # Linux

echo '{"id":1,"method":"accounts.add","params":{"host":"github.com","token":"ghp_yourtoken"}}' | nc -U "$SOCK"
```

The token is validated against GitHub, then stored in your OS keychain. For
GitHub Enterprise, add `"api_base":"https://your-host/api/v3"` to the params.

Once an account is added, the daemon polls every 60 seconds (honoring GitHub's
own rate-limit guidance) and sends a desktop notification when something new
needs you.

### 3. Run the app

```bash
./target/release/gitsurveil
```

An icon appears in your menu bar, colored by the most urgent thing waiting on
you. Click it for the popover: everything currently open, most urgent first,
with a dot per priority band. Click a row to open it on GitHub, or hover a row
to dismiss it (it disappears from the Dashboard too). Clicking elsewhere
dismisses the popover.

**Open GitSurveil** — from the popover header or the tray's right-click menu —
opens the full window:

- **Dashboard** — items grouped by priority (or by type), with search and
  filters by account, **repository**, type, and severity. Dismiss anything you
  don't want to see, or force an immediate check with **Check now**.
- **Pull Requests** — your PRs across all accounts, with their state: draft,
  review decision, CI, and mergeability. The daemon keeps these in its local
  database and refreshes them on its poll cycle, so the view opens instantly
  and still works with no network; **Refresh** forces a round trip when you
  can't wait for the next sync. Filter by status (Open is the
  default; Closed, Merged, or All re-query the daemon), account, repository,
  role, and attention (draft / conflicted / CI failing / approved), plus
  title+repo search. A chat-bubble badge shows how many unresolved review
  threads each PR has. Click a row for the full PR detail pane — rendered
  markdown description and comments, per-file review threads you can reply to,
  resolve, or unresolve, and inline editing of the title, description, target
  branch, labels, and draft flag — and a conflicted
  row gets an inline **Resolve conflicts** action (needs a local clone
  configured for that repository on the Repositories tab). Right-click a row
  to open the PR on GitHub (the menu names the provider, e.g. **Open in
  GitHub**) in your browser.
- **History** — resolved and dismissed items, with the option to restore a
  dismissal or clear all history at once. Restoring brings the item straight
  back into the Dashboard and the popover. Clearing archives the items: they
  disappear for good and never come back, even though they may still be open
  on GitHub.
- **Rules** — how scoring works, and what your configured rules do.
- **Repository and Worktrees** — every repository discovered across your
  accounts, with account and organization filters. A single click on a row
  expands or collapses its worktree panel; a **double click** opens the repo
  in your browser. Repos
  the daemon has found but you haven't seen yet are flagged in a "new
  repositories" modal when the window opens (one dismissal acks the whole
  batch). Right-click any row to open it on GitHub, clone it locally into a
  folder you pick (a background job with a progress bar), or point it at an
  existing clone. Tracked repos are what the conflict resolver works on —
  always on a throwaway worktree, never in your working tree. Clones are
  HTTPS-only and are never pushed to without an explicit action. Rows with a
  registered clone expand to show the repo's worktrees (and to add or delete
  them): pick an existing branch or type a new one, and the daemon creates
  `wt-{owner}-{name}-{branch}` next to the clone. Right-click a worktree to
  **Open with…** a registered app (the daemon runs `command <path>`) or to
  delete it — deleting removes its directory but keeps its branch.
- **Settings** — a notification-kind checklist (review requested, assigned,
  mentioned, participating, CI failed, changes requested, ready to merge,
  your PR has activity, a PR you reviewed has activity) checked by default;
  unchecking a kind only stops its desktop notification — it still appears in
  the Dashboard and history. Below that, the **Open with… applications** for
  worktree menus: give each a name and an **Application or Command** (an
  executable on your PATH, an absolute path, or one picked with the
  **Browse…** file dialog). You only get an "Open with…" menu when at least
  one app is registered.
- **Accounts** — add or remove accounts. Each account with discovered repos
  gets a checklist to choose which repos feed notifications, the Dashboard,
  and the Pull Requests view — independent of which repos have a local clone
  registered. New repos default to on.

Clicking a pull request opens a detail pane beside the list: description,
reviewers, checks, and the full conversation, with buttons to edit the title
and body, comment, close, or merge (merge commit, squash, or rebase). Merging
and closing ask for confirmation first, and a merge is rejected by GitHub if
the branch moved since the pane loaded it.

### Resolving merge conflicts

When a PR is conflicted with its base branch, the detail pane shows **Resolve
conflicts**. Clicking it starts a resolution session:

1. The daemon makes a temp worktree from the repo's configured clone, merges
   the base branch into the PR branch there, and lists every conflicted file.
   Your local clone is never touched.
2. For each file you get a three-pane editor: the PR branch's side, the base
   branch's side, and the editable result. Each conflict starts as its raw
   marker block; **Use ours / Use theirs / Use both** replace it, or you hand-edit.
3. Save each file (a file counts as resolved once its text holds no markers),
   then **Commit resolution**. The daemon refuses to commit if any marker is
   left.
4. **Push & finish** pushes the resolution to the PR's branch and tears the
   worktree down. **Abort** throws the session away at any point — again,
   nothing but the worktree is touched.

Binary files and files over 5 MB skip the editor and offer a whole-file
**Keep ours / Keep theirs** instead. One session per repo: starting another
while one is live tells you to finish or abort the first.

Quitting the app leaves the daemon running, so notifications keep arriving.

### 4. Inspect what it's tracking (optional)

```bash
echo '{"id":2,"method":"status","params":null}'        | nc -U "$SOCK"
echo '{"id":3,"method":"items.list","params":null}'    | nc -U "$SOCK"
echo '{"id":4,"method":"accounts.list","params":null}' | nc -U "$SOCK"
```

### Available API methods

| Method | Purpose |
|---|---|
| `status` | Version, uptime, account count, open item count, top severity |
| `items.list` | Open items, scored and ordered by priority |
| `items.history` | Resolved and dismissed items |
| `items.clear_history` | Archive every resolved and dismissed item — hidden from the Dashboard and history permanently (no undo) |
| `items.dismiss` / `items.undismiss` | Hide or restore an item locally |
| `accounts.list` | Configured accounts (never includes tokens) |
| `accounts.add` | Validate a token and register an account |
| `accounts.remove` | Remove an account, its items, and its token |
| `rules.list` | The active priority rules |
| `notifications.prefs` | Every item kind's notification preference, enabled by default |
| `notifications.set_pref` | Set whether a kind may produce a notification (Dashboard/history are unaffected) |
| `poll.now` | Check GitHub immediately |
| `pr.detail` | Full detail for one pull request |
| `pr.create` / `pr.update` | Create a PR, or edit title/body/base/labels/reviewers |
| `pr.close` / `pr.merge` | Close without merging, or merge |
| `pr.comments` / `pr.comment` | Read the conversation, or post a top-level comment |
| `pr.comment_reply` | Reply inside a review thread |
| `pr.resolve` | Resolve or unresolve a review thread |
| `pr.branches` / `pr.labels` | Branch names and repo labels, for the create/edit pickers |
| `prs.list` | Stored list of PRs across accounts (standing state for the Pull Requests view); optional account filter and open/closed/merged state |
| `prs.refresh` | Force a pull-request sync with GitHub now, then return the refreshed list |
| `repos.list` | The full repository catalog (every discovered repo + organizations), with tracked state and clone path |
| `repos.new` | Repositories discovered but not yet acknowledged |
| `repos.ack_new` | Dismiss the whole "new repositories" batch |
| `repos.refresh` | Force a discovery pass right now |
| `repos.clone` | Start a background HTTPS clone; returns a `job_id` |
| `repos.clone_status` | Progress of a clone job (bytes received; the total is unknowable, so it stays 0) |
| `repos.set` | Register an existing local clone as a repo's path (validated) |
| `repos.set_notify` | Set whether a repo's items feed notifications and the Pull Requests view (independent of clone tracking) |
| `repos.remove` | Forget a repo's clone path (does not delete files) |
| `repos.worktrees` | A repo's user-created worktrees plus the branches a new one can use |
| `repos.worktree_add` | Create a worktree (existing branch or a new one); nothing pre-existing is touched |
| `repos.worktree_remove` | Remove a worktree and its directory; the branch survives |
| `apps.list` | The registered "Open with…" applications |
| `apps.add` | Register an application (name + bare command on PATH) |
| `apps.remove` | Forget a registered application |
| `apps.open` | Open a path with a registered app (`command <path>`, spawned by the daemon) |
| `conflicts.prepare` | Start a resolution session (temp worktree) |
| `conflicts.file` | One conflicted file's segments, from the worktree |
| `conflicts.save` | Write resolved content, or pick a whole-file side |
| `conflicts.commit` | Create the resolution merge commit |
| `conflicts.push` | Push the resolution and tear the session down |
| `conflicts.abort` | Abandon the session (idempotent) |

More methods land with each phase — see `specs/daemon.md` for the full planned
surface.

## Notifications

You get a notification when a new action item appears, when CI flips from
passing to failing on something you care about, when a pull request you opened
gains a comment from someone else or an unresolved review thread, when a
thread you commented in gets a reply, or when one of your pull requests
crosses into ready-to-merge (approved, green, not a draft). Your own
commits and comments never notify you. Items you've already seen never notify
twice. If a single poll turns up more than three new items (say, after being
offline), they collapse into one summary notification instead of a burst.

Three current limitations, all temporary:

- **Clicking a notification doesn't open the item.** macOS only supports action
  labels for unbundled binaries. Use the menubar popover to click through to
  GitHub instead.
- **No quiet hours yet.** The outrank gate means most things stay silent
  already, but there's no time-of-day suppression.
- **Rules are read-only in the UI.** You can see what they do, but editing
  means hand-editing `config.toml` and restarting the daemon. A graphical
  editor is still to come.

## A note on sleep

Closing your laptop lid sleeps the OS, and no background process runs during
sleep — that's true of any app, and GitSurveil doesn't fight it with wake locks.
What the daemon does give you: monitoring with no UI open, an immediate catch-up
poll on wake, and continuous monitoring on machines that stay awake (desktops,
clamshell mode on power).

## Development

```bash
cargo test              # daemon + proto tests
pnpm test               # frontend tests (Vitest)
cargo doc --no-deps     # must build warning-free
pnpm tauri dev          # run the app with frontend hot-reload
```

`pnpm tauri dev` expects the daemon to already be running.

### Memory footprint

Measured on macOS with release builds:

| Process | Footprint |
|---|---|
| `gitsurveild` (daemon), idle | ~3–9 MB |
| `gitsurveil`, popover dismissed (warm) | ~24 MB |
| `gitsurveil`, popover open | ~25 MB |

These are `phys_footprint` — the private memory macOS attributes to the
process, and what Activity Monitor shows. Raw RSS reads far higher (~84 MB)
because it counts shared system framework pages that every app maps.

Dismissing the popover hides its webview (so the next tray click is instant);
leaving it hidden past the idle timeout destroys it outright (the WebKit
content process disappears), and repeated open/dismiss cycles hold steady
rather than creeping upward. Figures above were measured with the destroy-on-
close behavior; with the hidden webview the "popover closed" row reflects a
recently-dismissed, still-warm popover until the idle teardown reclaims it.

Specifications live in `/specs` — one document per feature, and the source of
truth for behavior. Read the relevant spec before changing a feature.

To verify desktop notifications actually reach your OS:

```bash
cargo test -p gitsurveild live_notification -- --ignored --nocapture
```

## Privacy

- Tokens are stored in the OS keychain only — never in SQLite, config files, or logs.
- The daemon never opens a network port; the local API is a user-permissioned
  unix socket.
- Nothing is ever posted to GitHub without an explicit action from you.
- AI review (Phase 8) will be off by default, and when enabled can run fully
  locally via Ollama.

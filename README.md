# gitsurveil

A GitHub action-item monitor that runs quietly in the background and tells you
only what actually needs your attention.

A local daemon polls GitHub, normalizes everything you're on the hook for —
review requests, assignments, mentions, failing CI — into one prioritized list,
and fires desktop notifications. A Pull Requests view shows your own PRs'
standing state (draft, reviews, CI, mergeability) and notifies you the moment
one is approved, green, and ready to merge. Nothing is hosted: your machine
talks to GitHub directly with your own token, and no data goes anywhere else.

## Status

**Early development.** Phases 1–7 of 9 are done: the daemon monitors GitHub,
prioritizes what it finds, and notifies you; a menubar app shows what's
pending; a desktop window provides the dashboard, history, rules, accounts,
pull-request management, and a three-pane conflict resolver that lets you merge
a conflicted PR from within the app.

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
| 9 | Service registration & packaging | Not started |

Because Phase 9 hasn't landed, the daemon does **not** yet start at login — you
run it manually, and it stops when you close the terminal.

## What it monitors

- Review requests waiting on you
- Pull requests and issues assigned to you
- Mentions
- Failing CI on your pull requests
- Your pull requests that become ready to merge (approved, green, not a draft)

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
| Assigned | 40 |
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
gitsurveil (Tauri v2 app) ── tray icon + notifications popover,
                             plus the full desktop window
```

The daemon owns everything stateful; the app only renders and forwards intent.
Quitting the app doesn't stop monitoring. The daemon never listens on a network
port, and tokens live only in the OS keychain — never in the database, config
files, or logs.

Each webview is **destroyed** when its window closes, not hidden, and rebuilt
when reopened. That's what keeps an idle menubar app cheap: with everything
closed, no webview process exists at all.

## Install a release build

Download the file for your platform from the
[releases page](../../releases): a `.dmg` for macOS (Apple Silicon or Intel),
`.AppImage`/`.deb` for Linux, `.msi` for Windows.

These builds are not code-signed, so the first launch is blocked:

- **macOS** — right-click the app, choose **Open**, then confirm.
- **Windows** — SmartScreen: **More info → Run anyway**.

The background service ships inside the app. To have it start at login:

```bash
# macOS, after dragging gitsurveil.app to /Applications
/Applications/gitsurveil.app/Contents/MacOS/gitsurveild install
```

`gitsurveild status` reports whether it is registered and whether it is
currently answering; `gitsurveild uninstall` removes the registration.

## Build from source

## Requirements

- **Rust** (stable). Install via [rustup](https://rustup.rs):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js 20+ and pnpm**, to build the app's frontend.
- **macOS or Linux.** The Windows named-pipe transport is written but not yet
  verified; it gets tested in Phase 9.
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
(Login-time autostart is Phase 9; until then this is the only way to run it.)

### 2. Add your GitHub account

Easiest from the app: run it (step 3), open the window, and use **Accounts**.

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
with a dot per priority band. Click a row to open it on GitHub. Clicking
elsewhere dismisses the popover.

**Open gitsurveil** — from the popover header or the tray's right-click menu —
opens the full window:

- **Dashboard** — items grouped by priority (or by type), with search and
  filters by account, type, and severity. Dismiss anything you don't want to
  see, or force an immediate check with **Check now**.
- **Pull Requests** — your PRs across all accounts, with their live state:
  draft, review decision, CI, and mergeability. Filter by status (Open is the
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
  dismissal.
- **Rules** — how scoring works, and what your configured rules do.
- **Repositories** — local clone paths the conflict resolver uses. Resolution
  always happens on a throwaway worktree cloned from this path, never in your
  working tree.
- **Accounts** — add or remove accounts.

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
| `items.dismiss` / `items.undismiss` | Hide or restore an item locally |
| `accounts.list` | Configured accounts (never includes tokens) |
| `accounts.add` | Validate a token and register an account |
| `accounts.remove` | Remove an account, its items, and its token |
| `rules.list` | The active priority rules |
| `poll.now` | Check GitHub immediately |
| `pr.detail` | Full detail for one pull request |
| `pr.create` / `pr.update` | Create a PR, or edit title/body/base/labels/reviewers |
| `pr.close` / `pr.merge` | Close without merging, or merge |
| `pr.comments` / `pr.comment` | Read the conversation, or post a top-level comment |
| `pr.comment_reply` | Reply inside a review thread |
| `pr.resolve` | Resolve or unresolve a review thread |
| `pr.branches` / `pr.labels` | Branch names and repo labels, for the create/edit pickers |
| `prs.list` | Live list of PRs across accounts (standing state for the Pull Requests view); optional account filter and open/closed/merged state |
| `repos.list` / `repos.set` / `repos.remove` | Local clone paths for the conflict resolver |
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
passing to failing on something you care about, or when one of your pull
requests crosses into ready-to-merge (approved, green, not a draft). Items
you've already seen never notify twice. If a single poll turns up more than
three new items (say, after being offline), they collapse into one summary
notification instead of a burst.

Two current limitations, both temporary:

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
sleep — that's true of any app, and gitsurveil doesn't fight it with wake locks.
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
| `gitsurveil`, popover closed | ~24 MB |
| `gitsurveil`, popover open | ~25 MB |

These are `phys_footprint` — the private memory macOS attributes to the
process, and what Activity Monitor shows. Raw RSS reads far higher (~84 MB)
because it counts shared system framework pages that every app maps.

Closing the popover destroys its webview process outright (verified: the
WebKit content process disappears), and repeated open/close cycles hold steady
rather than creeping upward.

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

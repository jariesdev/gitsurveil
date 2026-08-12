# gitsurveil

A GitHub action-item monitor that runs quietly in the background and tells you
only what actually needs your attention.

A local daemon polls GitHub, normalizes everything you're on the hook for —
review requests, assignments, mentions, failing CI — into one prioritized list,
and fires desktop notifications. Nothing is hosted: your machine talks to
GitHub directly with your own token, and no data goes anywhere else.

## Status

**Early development.** Phases 1–2 of 9 are done: the daemon monitors GitHub and
sends notifications. **There is no user interface yet** — the menubar app
arrives in Phase 3, the full desktop UI in Phase 5.

| Phase | Feature | Status |
|---|---|---|
| 1 | Core monitoring (poller, storage, local API) | ✅ Done |
| 2 | Desktop notifications | ✅ Done |
| 3 | Menubar app (tray + notifications popover) | Not started |
| 4 | Priority engine (scoring, severity tray, outrank gate) | Not started |
| 5 | Full desktop UI (dashboard, rules, accounts) | Not started |
| 6 | PR management (create/update/close/merge, comments) | Not started |
| 7 | Conflict resolver (3-pane, Sublime Merge-style) | Not started |
| 8 | AI PR review (opt-in; Ollama or Claude) | Not started |
| 9 | Service registration & packaging | Not started |

Because Phase 9 hasn't landed, the daemon does **not** yet start at login — you
run it manually, and it stops when you close the terminal.

## What it monitors

- Review requests waiting on you
- Pull requests and issues assigned to you
- Mentions
- Failing CI on your pull requests

Multiple accounts are supported, including GitHub Enterprise.

## Architecture

```
gitsurveild (Rust daemon) ── polls GitHub, owns all state,
                             sends notifications, serves a local
                             JSON API over a unix socket
        ▲
        └── thin UI clients (Tauri v2 + React) — not built yet
```

The daemon owns everything stateful; the UIs, once they exist, only render and
forward intent. It never listens on a network port, and tokens live only in the
OS keychain — never in the database, config files, or logs.

## Requirements

- **Rust** (stable). Install via [rustup](https://rustup.rs):
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **macOS or Linux.** The Windows named-pipe transport is written but not yet
  verified; it gets tested in Phase 9.
- A **GitHub personal access token** with the `notifications` and `repo` scopes.

## Install

```bash
git clone <repository-url>
cd gitsurveil
cargo build --release
```

The daemon binary lands at `target/release/gitsurveild`.

## Usage

### 1. Start the daemon

```bash
cargo run -p gitsurveild -- --foreground
```

It stays attached to the terminal and logs what it's doing. Leave it running.
(Login-time autostart is Phase 9; until then this is the only way to run it.)

### 2. Add your GitHub account

The daemon exposes a JSON API over a unix socket. In a second terminal:

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

### 3. Inspect what it's tracking

```bash
echo '{"id":2,"method":"status","params":null}'        | nc -U "$SOCK"
echo '{"id":3,"method":"items.list","params":null}'    | nc -U "$SOCK"
echo '{"id":4,"method":"accounts.list","params":null}' | nc -U "$SOCK"
```

### Available API methods

| Method | Purpose |
|---|---|
| `status` | Version, uptime, account count, open item count |
| `items.list` | All currently open action items |
| `accounts.list` | Configured accounts (never includes tokens) |
| `accounts.add` | Validate a token and register an account |

More methods land with each phase — see `specs/daemon.md` for the full planned
surface.

## Notifications

You get a notification when a new action item appears, or when CI flips from
passing to failing on something you care about. Items you've already seen never
notify twice. If a single poll turns up more than three new items (say, after
being offline), they collapse into one summary notification instead of a burst.

Two current limitations, both temporary:

- **Clicking a notification doesn't open the item.** macOS only supports action
  labels for unbundled binaries, so rather than half-implement it, click-through
  will come via the menubar popover in Phase 3.
- **No quiet hours or priority filtering yet.** Everything new notifies. The
  priority engine in Phase 4 adds severity scoring and the "only interrupt me
  for things that outrank my current work" gate.

## A note on sleep

Closing your laptop lid sleeps the OS, and no background process runs during
sleep — that's true of any app, and gitsurveil doesn't fight it with wake locks.
What the daemon does give you: monitoring with no UI open, an immediate catch-up
poll on wake, and continuous monitoring on machines that stay awake (desktops,
clamshell mode on power).

## Development

```bash
cargo test              # daemon + proto tests
cargo doc --no-deps     # must build warning-free
cargo build --release   # optimized binary
```

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

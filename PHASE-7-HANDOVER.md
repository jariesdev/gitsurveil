# Phase 7 handover: conflict resolver

Implementation brief for the next agent. Read this in full before writing code.

---

## 0. Read these first, in this order

| File | Why |
|---|---|
| `CLAUDE.md` | Hard rules. Non-negotiable. |
| `specs/conflict-resolver.md` | The feature spec. Source of truth for behavior. |
| `.claude/rules/git-commits.md` | Commit message + grouping rules. |
| `.claude/rules/naming.md` | Two product names are banned repo-wide. |
| `crates/gitsurveild/src/socket.rs` | The API pattern you will extend. |
| `crates/gitsurveild/src/github/pr.rs` | The most recent feature; mirror its shape. |

If this document and `specs/conflict-resolver.md` disagree, **the spec wins** —
and update this file to match.

---

## 1. Where the project stands

Phases 1–6 of 9 are complete and committed.

```
gitsurveild (Rust daemon)  ← owns ALL state and side effects
      ▲ unix socket, newline-delimited JSON
gitsurveil (Tauri v2 app)  ← tray + popover + desktop window; renders only
```

**Working today:** GitHub polling with ETag conditional requests, SQLite store,
OS-keychain tokens, priority scoring with an outrank notification gate,
severity-colored tray, notifications popover, desktop window (dashboard /
history / rules / accounts), and full PR management (detail, create, update,
close, merge, comments).

**Test baseline — do not regress:**

```bash
cargo test --workspace     # 40 passing
pnpm test                  # 22 passing
cargo doc --no-deps        # 0 warnings (enforced by #![warn(missing_docs)])
```

**Existing API methods** (all in `socket.rs::dispatch`): `status`,
`items.{list,history,dismiss,undismiss}`, `accounts.{list,add,remove}`,
`rules.list`, `poll.now`, `pr.{detail,create,update,close,merge,comments,comment,branches}`.

### Known gaps carried into Phase 7

1. **No PR operation has ever run against real GitHub.** Every wire shape in
   `github/pr.rs` was written from GitHub's docs, not observed responses. The
   conflict resolver depends on `pr.detail`'s `mergeability` field being
   correct. **Verify that against a real PR before trusting it.**
2. **Rules are read-only** — no `rules.set`, no config hot-reload.
3. **Windows named-pipe transport is written but never executed.**
4. The daemon does not start at login (Phase 9).

---

## 2. Hard rules (from `CLAUDE.md`)

Violating any of these is a defect, not a style choice.

- **Never touch the user's working tree.** Temp worktrees only. This is the
  single most important rule in this phase.
- Tokens live in the OS keychain only — never SQLite, config files, or logs.
- The daemon never opens a network port.
- Nothing is pushed to GitHub without an explicit user action.
- Every module gets `//!` docs; every public item gets `///` docs stating
  purpose, errors, and invariants. `cargo doc` must stay warning-free.
- Inline `//` comments explain **why**, not what, on non-obvious logic.
- No frameworks in the daemon. Libraries only.
- Prefer a pure function; the priority engine and notification gate are pure by
  spec and are the model to follow.

---

## 3. Scope

### In

Resolve a PR's merge conflicts in-app: prepare a temp worktree, present a
three-pane (ours / result / theirs) editor per conflicted file, apply
resolutions, commit, push. Abort cleanly at any point.

### Out (do not build)

- A general git client — no staging UI, log browsing, or rebase UI.
- Auto-cloning. v1 requires the user to configure a local clone path.
- Binary-file merge. Binaries get whole-file pick-ours/pick-theirs only.
- Inline-diff review comments (that is a PR-management gap, not this phase).

---

## 4. Architecture decisions — already made, do not relitigate

### 4.1 Git work happens in `spawn_blocking`

**`git2::Repository` and `git2::Worktree` are `!Send`.** They cannot cross an
`.await` point in the tokio daemon. Every git operation must be wrapped:

```rust
let result = tokio::task::spawn_blocking(move || {
    // all git2 usage confined here; nothing borrowed from outside
}).await.map_err(/* join error */)?;
```

Do not attempt to hold a `Repository` in `ServerState`. Open it per operation
from a `PathBuf`. Opening a repo is cheap relative to a network round trip.

### 4.2 Worktree creation: `git2` first, shell fallback if needed

`git2::Repository::worktree()` exists and should be tried first. If it proves
unreliable (libgit2's worktree support is thinner than the CLI's), fall back to
`std::process::Command` invoking `git worktree add`. **Decide this empirically
with a spike before building on top of it** — a wrong choice here is expensive
to unwind. Record the finding in `specs/conflict-resolver.md`.

If you shell out, note it with a `ponytail:` comment naming the ceiling.

### 4.3 Session state lives in memory, keyed by repo

A resolution session is not durable. Put it in `ServerState` behind a
`Mutex<HashMap<String, Session>>` keyed by `"owner/name"`.

- **One active session per repo.** A second `conflicts.prepare` for a repo with
  a live session must fail with a clear error, not silently clobber it.
- **Daemon restart drops sessions.** Prune orphaned worktrees on startup
  (`git worktree prune` semantics) — the spec requires this.

### 4.4 Conflict parsing is a pure function

Extract hunks from conflicted file content with a pure function over a string.
No I/O. This is the piece that most deserves exhaustive tests, exactly like
`priority.rs::score_item`.

```rust
/// Splits conflicted file content into ordered regions.
pub fn parse_conflicts(content: &str) -> Vec<Region>;

pub enum Region {
    Context(String),
    Conflict { ours: String, theirs: String, base: Option<String> },
}
```

Handle: `<<<<<<<`, `|||||||` (diff3 base, may be absent), `=======`, `>>>>>>>`.
Nested/malformed markers must not panic — return the raw text as context and
let the UI show it rather than crashing the daemon.

### 4.5 Push auth reuses the account token

Use `git2::RemoteCallbacks::credentials` with
`Cred::userpass_plaintext(&login, &token)` over HTTPS. Pull the token from
`crate::keychain::get_token(account_id)`. No SSH key handling in v1.

**The token must never reach a log line or an error message returned to the
UI.** Scrub it if a git error echoes the remote URL.

---

## 5. Implementation order

Each step should build, pass tests, and be committed on its own.

### Step 1 — Per-repository clone paths

Users must tell gitsurveil where their local clone lives.

- `crates/gitsurveild/src/config.rs`: add
  `pub repos: Vec<RepoConfig>` with `{ repo: String, path: PathBuf }`.
  Follow the existing `rules` field pattern (`#[serde(default)]`).
- New API: `repos.list`, `repos.set` (upsert), `repos.remove`.
- **Validate on set**: path exists, is a git repository, and its `origin`
  remote URL contains the `owner/name`. Reject with a specific message —
  "not a git repository" and "remote does not match acme/api" are different
  problems and the user fixes them differently.
- Settings UI: a repo→path table in the desktop window.

Commit: `Add per-repository local clone paths`

### Step 2 — Conflict parsing (pure, no git yet)

- New file `crates/gitsurveild/src/conflicts/parse.rs`.
- Implement `parse_conflicts` per §4.4, plus `render_resolution(regions) -> String`.
- Round-trip property: parsing then rendering an unmodified conflict must
  reproduce the input byte-for-byte.
- Tests: no conflicts; single conflict; multiple conflicts; diff3 with base
  section; CRLF line endings; a file that ends mid-conflict (truncated);
  marker-like text inside a string literal.

Commit: `Add conflict marker parsing`

### Step 3 — Session lifecycle (git operations)

- New file `crates/gitsurveild/src/conflicts/session.rs`.
- `prepare(repo_path, head_branch, base_branch) -> Session`:
  1. Open repo, `fetch` origin (with credentials).
  2. **Verify the user's worktree is clean.** Dirty → abort with "commit or
     stash first". Never proceed past this check.
  3. Create temp worktree under the data dir, not inside the user's repo.
  4. Check out the PR head branch there.
  5. Merge the base branch → conflicted index.
  6. Return session id + conflicted file list with per-file conflict counts.
- `abort(session_id)`: remove the worktree, prune, drop the session. Must be
  idempotent and must leave zero trace.
- Startup hook in `main.rs`: prune orphaned worktrees from previous runs.

Commit: `Add conflict resolution sessions on temp worktrees`

### Step 4 — Daemon API

Add to `socket.rs::dispatch`, following the `PrAction` enum pattern already
there (one handler, one params struct, an action enum):

| Method | Params | Returns |
|---|---|---|
| `conflicts.prepare` | `repo`, `number` | session id, file list |
| `conflicts.file` | `session_id`, `path` | regions (§4.4) |
| `conflicts.save` | `session_id`, `path`, `content` | ok |
| `conflicts.commit` | `session_id`, `message` | ok |
| `conflicts.push` | `session_id` | ok |
| `conflicts.abort` | `session_id` | ok |

**`conflicts.commit` must refuse to commit a file still containing conflict
markers.** That is the last line of defense against pushing `<<<<<<<` to a
shared branch.

Commit: `Add conflict resolver API methods`

### Step 5 — Three-pane UI

- New `src/desktop/conflicts/` — `ConflictResolver.tsx`, `ThreePane.tsx`,
  `FileList.tsx`.
- Entry point: `src/desktop/PrDetail.tsx` already renders
  `mergeability === "conflicted"`. Add a **Resolve conflicts** button there.
- Layout: file list on the left; three panes for the selected file. Left =
  ours (PR branch), right = theirs (base), center = editable result.
- Per hunk: take-left / take-right / take-both / edit center manually.
  Non-conflicting context pre-filled in the center.
- A file is resolved when no markers remain. Show progress (`3 of 5 resolved`).
- Whole-file pick-ours / pick-theirs shortcut; the **only** option for
  binaries and files >5 MB.
- **Abort must be reachable from every state**, and must confirm.

Commit: `Add three-pane conflict resolution UI`

---

## 6. Gotchas

| Risk | Mitigation |
|---|---|
| `git2` types are `!Send` | All git work inside `spawn_blocking`; never hold a `Repository` across `.await`. |
| Dirty user worktree | Hard stop before creating anything. Test this path first — it protects real work. |
| Token leaking into errors | Scrub remote URLs from git error strings before returning them. |
| Orphaned worktrees after a crash | Prune on daemon startup; make `abort` idempotent. |
| Merge markers reaching the branch | Reject at `conflicts.commit`, not just in the UI. |
| CRLF repos | Honor the repo's own git config; do not normalize line endings yourself. |
| Base branch moves mid-resolution | Push head only. If GitHub then reports conflicts, offer a re-run. |
| Large files freezing the webview | Virtualize, or fall back to whole-file pick above 5 MB. |

---

## 7. Verification

### Automated (must pass before committing)

```bash
cargo test --workspace     # ≥40 passing, plus your new tests
pnpm test                  # ≥22 passing, plus your new tests
cargo doc --no-deps        # must stay at 0 warnings
```

Build fixture repos programmatically in tests (`tempfile` + `git2`) — do not
depend on any checkout existing on the machine. Cover: conflict detection,
hunk extraction, resolution application, merge commit, and **abort leaving no
worktree behind**.

### Manual — this phase cannot be signed off without it

Automated tests will not catch a wrong three-pane layout or an unusable editor.

1. Build: `pnpm tauri build --config crates/gitsurveil-app/tauri.conf.json --no-bundle`
   then `cargo build --release -p gitsurveild`.
   **A plain `cargo build` produces an app whose webview points at the dev
   server and renders blank.** This has already cost one debugging session.
2. Start daemon (`RUST_LOG=gitsurveild=debug ./target/release/gitsurveild --foreground`)
   then the app.
3. On a scratch repo, manufacture a real conflict on a PR.
4. Resolve one hunk each way: take-left, take-right, manual edit, whole-file.
5. Commit, push, confirm GitHub reports the PR mergeable.
6. Re-run and **abort** midway; confirm `git worktree list` is clean and the
   user's checkout is untouched.

**Hand the build to the human for a look before declaring the phase done.**
Prior phases shipped three bugs that only a human clicking found: a duplicate
tray icon, a stray title bar, and a dead Quit menu item.

---

## 8. Acceptance criteria

The phase is complete when **every** criterion below passes. Each is written to
be checkable — if you cannot demonstrate it, it has not passed. Do not mark a
criterion done by inspection of your own code; run it.

### AC-1 Safety: the user's working tree is never touched

The highest-priority requirement in this phase. A failure here destroys real work.

| # | Criterion | How to verify | Fail condition |
|---|---|---|---|
| 1.1 | A dirty working tree blocks preparation | In a configured clone, edit a tracked file without committing. Call `conflicts.prepare`. | Anything is created, or the edit is altered/reverted. Must return an error naming the dirty state. |
| 1.2 | Resolution happens only in a temp worktree | Run a full resolution. Before/after, `git -C <clone> status --porcelain` and `git -C <clone> rev-parse HEAD`. | The user's `HEAD`, index, or untracked files changed. |
| 1.3 | Temp worktrees live outside the user's repo | Inspect the worktree path from `conflicts.prepare`. | Path is inside the user's clone directory. |
| 1.4 | The user's checked-out branch is unchanged | `git -C <clone> branch --show-current` before and after a full resolution. | It differs. |

### AC-2 Session lifecycle

| # | Criterion | How to verify | Fail condition |
|---|---|---|---|
| 2.1 | `abort` leaves zero trace | Prepare, then abort. Run `git -C <clone> worktree list` and check the data dir. | Any worktree, temp directory, or session entry survives. |
| 2.2 | `abort` is idempotent | Call `conflicts.abort` twice with the same id. | The second call errors or panics. It must succeed or return a benign "no such session". |
| 2.3 | Abort works from every state | Abort (a) right after prepare, (b) after saving one file, (c) after commit but before push. | Any of the three leaves residue. |
| 2.4 | One session per repo | Call `conflicts.prepare` twice for the same repo without aborting. | The second silently replaces the first. Must return a clear error. |
| 2.5 | Orphans are pruned on startup | Prepare a session, `kill -9` the daemon, restart it. | The orphaned worktree is still registered after startup. |
| 2.6 | A crash never corrupts the user's repo | After 2.5, run `git -C <clone> fsck` and `status`. | Any corruption or unexpected modification. |

### AC-3 Conflict parsing (pure logic)

| # | Criterion | How to verify | Fail condition |
|---|---|---|---|
| 3.1 | Round-trip is byte-exact | Property test: `render_resolution(parse_conflicts(s)) == s` for unmodified conflicted content. | Any byte differs, including trailing newlines. |
| 3.2 | diff3 base section handled | Parse content containing `\|\|\|\|\|\|\|`. | The base section is lost or misattributed to ours/theirs. |
| 3.3 | Malformed markers do not panic | Parse: truncated conflict, nested markers, marker-like text inside a string literal. | A panic, or a `Result::Err` that takes down the request. Must degrade to context text. |
| 3.4 | CRLF preserved | Parse and render a CRLF file. | Line endings are normalized to LF. |
| 3.5 | Multiple conflicts in one file | Parse a file with ≥3 conflicts. | Regions are merged, reordered, or dropped. |

### AC-4 API surface

All six methods reachable over the socket. Verify each with `nc -U`, not just
by reading the dispatch table.

```bash
SOCK="$HOME/Library/Application Support/io.gitsurveil.gitsurveil/daemon.sock"
echo '{"id":1,"method":"conflicts.prepare","params":{...}}' | nc -U "$SOCK"
```

| # | Criterion | Fail condition |
|---|---|---|
| 4.1 | `conflicts.prepare` returns a session id and per-file conflict counts | Returns `unknown_method`, or a file list without counts. |
| 4.2 | `conflicts.file` returns ordered regions | Regions out of order, or context lost. |
| 4.3 | `conflicts.save` persists content into the worktree | A subsequent `conflicts.file` does not reflect the save. |
| 4.4 | `conflicts.commit` **refuses** content with conflict markers | A commit containing `<<<<<<<` succeeds. **This is a release blocker.** |
| 4.5 | `conflicts.push` reports git's error verbatim on rejection | A protected-branch rejection surfaces as a generic message. |
| 4.6 | `conflicts.abort` tears down the session | See AC-2. |
| 4.7 | Errors use correct codes | A git failure reports `config_error` rather than a git/github-specific code. |
| 4.8 | No token appears in any response or log | Grep the daemon log and every response for the token. **Any occurrence is a release blocker.** |

### AC-5 Three-pane UI

Automated tests cannot judge this. A human must look at it.

| # | Criterion | Fail condition |
|---|---|---|
| 5.1 | **Resolve conflicts** appears in `PrDetail` only when `mergeability === "conflicted"` | Shown on a clean PR, or missing on a conflicted one. |
| 5.2 | Panes are ours (left) / result (center, editable) / theirs (right) | Panes are mislabeled or swapped — this silently causes wrong resolutions. |
| 5.3 | Non-conflicting context is pre-filled in the center | The user must retype unconflicted lines. |
| 5.4 | All four hunk actions work: take-left, take-right, take-both, manual edit | Any action produces the wrong text. |
| 5.5 | Whole-file pick-ours / pick-theirs available | Missing, or not the *only* option for binaries and files >5 MB. |
| 5.6 | Progress is visible (e.g. "3 of 5 resolved") | The user cannot tell what remains. |
| 5.7 | Abort reachable from every state, and confirms first | Unreachable anywhere, or destroys work without confirmation. |
| 5.8 | Errors surface in the UI, not just the log | A failed push looks like a no-op button. |

### AC-6 End-to-end (mandatory — the phase cannot ship without this)

On a scratch GitHub repo with a **real** token, using **release builds**:

```bash
pnpm tauri build --config crates/gitsurveil-app/tauri.conf.json --no-bundle
cargo build --release -p gitsurveild
```

| # | Criterion | Fail condition |
|---|---|---|
| 6.1 | Manufacture a real conflict; the PR shows as conflicted in `PrDetail` | Mergeability is wrong — this also invalidates the untested `pr.detail` mapping (see §1, gap 1). |
| 6.2 | Resolve using each of the four actions across ≥2 files | Any produces incorrect merged content. |
| 6.3 | Commit and push succeed | Failure at either step. |
| 6.4 | GitHub reports the PR mergeable afterwards | Still conflicted, or the branch history is wrong. |
| 6.5 | The merged file content on GitHub is exactly what the center pane showed | **Any** discrepancy. This is the whole feature. |
| 6.6 | Re-run and abort midway; user's clone is pristine | See AC-1/AC-2. |

### AC-7 Regressions and hygiene

| # | Criterion | Command | Fail condition |
|---|---|---|---|
| 7.1 | Rust tests pass, count increased | `cargo test --workspace` | Fewer than 40 passing, or any failure. |
| 7.2 | Frontend tests pass, count increased | `pnpm test` | Fewer than 22 passing, or any failure. |
| 7.3 | Docs build clean | `cargo doc --no-deps` | Any warning. `#![warn(missing_docs)]` is enforced. |
| 7.4 | No build warnings | `cargo build --workspace` | Any warning. |
| 7.5 | Existing features still work | Open popover and dashboard; dismiss an item; check tray color. | Any Phase 1–6 behavior broke. |
| 7.6 | Banned names absent from shipped code and docs | `grep -riE "gitify\|catlight" --include="*.rs" --include="*.ts" --include="*.tsx" README.md specs/ crates/ src/` | Any match. (`.claude/rules/` and this handover legitimately name them in order to ban them — that is why they are excluded from the search paths above.) |
| 7.7 | No `Co-Authored-By` in new commits | `git log origin/main..HEAD --format=%B \| grep -i co-authored` | Any match. |

### AC-8 Documentation

| # | Criterion | Fail condition |
|---|---|---|
| 8.1 | `README.md` status table marks Phase 7 done | Still says "Not started". |
| 8.2 | `README.md` describes conflict resolution in the usage section | Feature undocumented. |
| 8.3 | `README.md` lists the six new API methods | Table is stale. |
| 8.4 | `README.md` describes only what ships | Any claim about unimplemented behavior. |
| 8.5 | `specs/conflict-resolver.md` matches shipped behavior | Spec and code disagree with no note explaining why. |
| 8.6 | Deliberate shortcuts carry `ponytail:` comments | An unmarked corner was cut. |

### Sign-off

The phase is done when AC-1 through AC-8 all pass **and a human has run the
app and confirmed it**. Report honestly which criteria you verified yourself
versus which you could not — an unverified criterion is not a passing one.

---

## 9. Conventions

**Commits** — group by type, one coherent change each (`.claude/rules/git-commits.md`):

- No `Co-Authored-By` trailers, ever.
- Don't mix a feature with unrelated docs, config, or dependency churn.
- A dependency added *because* a feature needs it belongs with that feature.
- If the message needs "and" to describe unrelated changes, split it.

**Naming** — two competitor product names are banned repo-wide; see
`.claude/rules/naming.md`. Describe the behavior directly instead.

**Style** — match the surrounding code. The daemon is plain tokio + libraries.
Don't add a trait with one implementation unless the spec names it as an
extension point.

---

## 10. Untracked files

`specs/`, `CLAUDE.md`, and `.claude/` are **deliberately untracked** in git by
the repo owner's decision. They exist on disk and are authoritative — read
them. Do not add them to git without asking.

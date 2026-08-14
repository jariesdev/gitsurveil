# Handover: Pull Requests view + "ready to merge" notification

Implementation brief for the next agent. Read in full before writing code.

---

## 0. ⚠️ The workspace does not currently compile

Two edits were left on disk by a previous session and are **the wrong shape**.
Your first task is to undo them.

```
 M crates/gitsurveil-proto/src/item.rs      ← remove `ItemKind::Authored`
 M crates/gitsurveild/src/github/client.rs  ← remove the plain `authored:` alias,
                                              its envelope field, and its mapping
```

`cargo build --workspace` currently fails with 3 `non-exhaustive patterns`
errors. Reverting both files (`git checkout -- <file>`) restores a green build
at **69 Rust / 23 frontend tests**. Confirm that before starting.

The `authored:` GraphQL alias **does** come back later (Part 2), but with more
fields and completely different semantics. Do not simply keep it.

---

## 1. Read these first

| File | Why |
|---|---|
| `CLAUDE.md` | Hard rules. Non-negotiable. |
| `specs/desktop-ui.md` | Where the new view lives. |
| `specs/pr-management.md` | Phase 6, which this view is the door to. |
| `specs/conflict-resolver.md` | Phase 7, reachable from the new view. |
| `.claude/rules/git-commits.md` | Commit grouping + message rules. |
| `.claude/rules/naming.md` | Two product names banned repo-wide. |
| `src/desktop/Dashboard.tsx` | The filter/`Select` pattern to reuse. |
| `src/desktop/grouping.ts` | The pure-filter-logic pattern to mirror. |
| `crates/gitsurveild/src/github/pr.rs` | The API-client shape to follow. |

Spec beats this document if they disagree — and update this document to match.

---

## 2. The problem

gitsurveil never shows the pull requests **you opened**. The poller runs only
`review-requested:@me` and `assignee:@me`. A healthy PR you authored, waiting
on review, is invisible.

This strands two finished phases: PR management (6) and the conflict resolver
(7) act almost entirely on *your own* PRs, but nothing surfaces them — so those
features are unreachable unless someone else assigns or mentions you. It also
contradicts `specs/desktop-ui.md`, which scopes the app to "the user's action
items **and their PRs**".

**Root cause:** the item model was built around *what pings you* — an inbox
shape where every kind is a notification. Your own work in flight is a standing
state, not an event, so no category existed for it.

---

## 3. The central design decision

**Standing state and events want different models.** Conflating them is exactly
what went wrong the first time. So this splits in two:

| Part | Shape | Why |
|---|---|---|
| **1. Pull Requests view** | Its own live query, its own type | `ActionItem` cannot carry draft state, review decision, or mergeability without distorting a model shared by mentions and CI events. |
| **2. `ItemKind::ReadyToMerge`** | A normal action item | "Your PR became mergeable" genuinely *is* an event, so it fits the existing model exactly. |

Modelling part 2 as `ItemKind::Authored` (the reverted attempt) would notify on
every PR you opened, and produce duplicate rows for self-assigned PRs.
`ReadyToMerge` has neither problem because it fires on a **transition**.

---

## 4. Hard rules (from `CLAUDE.md`)

- Tokens: OS keychain only — never SQLite, config files, logs, or API responses.
- The daemon never opens a network port.
- **Nothing is written to GitHub without an explicit user action.**
- Never touch the user's working tree — temp worktrees only (Phase 7).
- `//!` docs on every module, `///` on every public item. `cargo doc` must stay
  warning-free; `#![warn(missing_docs)]` is enforced.
- Inline `//` comments explain **why**, not what.
- No frameworks in the daemon. Prefer a pure function.

---

## 5. Part 1 — Pull Requests view

### 5.1 Types (`crates/gitsurveil-proto/src/pr.rs`)

Add beside the existing `PullRequestDetail`:

```rust
pub struct PullRequestSummary {
    pub account_id: String,
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub roles: Vec<PrRole>,       // why it is in your list; may be several
    pub state: PrState,
    pub draft: bool,
    pub ci_status: CiStatus,      // reuse existing enum
    pub review_decision: ReviewDecision,
    pub mergeable: Mergeability,  // reuse from Phase 6
    pub created_at: String,
    pub updated_at: String,
}

pub enum PrRole { Authored, ReviewRequested, Assigned }
pub enum PrState { Open, Closed, Merged }
pub enum ReviewDecision { Approved, ChangesRequested, ReviewRequired, None }
```

`roles` is a **set, not a single value.** That is what prevents duplicate rows:
a PR you authored *and* self-assigned is one row with two badges. Getting this
wrong is the most likely way to ship a visible bug.

### 5.2 Daemon

`crates/gitsurveild/src/github/pr.rs` — `list_pull_requests(state)`:

- One GraphQL request per account, three aliases (`authored`,
  `reviewRequested`, `assigned`) over a fragment richer than the poller's:
  adds `isDraft`, `reviewDecision`, `mergeable`, `state`.
- `state` selects the search qualifier: `is:open` (default) / `is:closed` /
  `is:merged` / none for all.
- Merge the three sets by `(repo, number)`, **unioning `roles`**.
- Live query, **not stored**. No schema change, always fresh, and it costs one
  request per account only when the user opens or refilters the view.

`socket.rs` — `prs.list` taking optional `account_id` and `state`. Without an
account, query every configured account and concatenate. Follow the existing
`PrAction` handler pattern in that file.

### 5.3 UI

New `src/desktop/PullRequests.tsx`; add a **Pull Requests** entry to `NAV` in
`src/desktop/App.tsx`, between Dashboard and History.

**Filters** — reuse the `Select` component pattern from `Dashboard.tsx`:

| Filter | Values |
|---|---|
| Status | **Open (default)** / Closed / Merged / All |
| Account | all / each configured account |
| Repository | all / each repo present in results |
| Role | all / authored / review requested / assigned |
| Attention | all / draft / conflicted / CI failing / approved |
| Search | title + repository, case-insensitive |

Status re-queries the daemon (it changes the GraphQL qualifier). Everything
else filters client-side. Put the filter logic in
`src/desktop/PullRequests/filters.ts` as pure functions and unit-test it,
mirroring `src/desktop/grouping.ts`.

**Each row shows:** title · `repo#number` · author · role badges · draft badge
· CI status · review decision · conflict warning · age. Sort by most recently
updated.

**Clicking a row opens the existing `PrDetail`** — it already takes exactly
`repo`, `number`, `onClose`, `onChanged`, `onResolve`. Edit, comment, close,
merge, and the conflict resolver all come for free. **Do not reimplement any of
it.** This view is the door those features never had.

### 5.4 Conflicted PRs resolve from the list

A conflicted PR is where the user most needs to act, so it must not hide one
level down in the detail pane.

- Conflicted rows are visually flagged and carry an inline **Resolve
  conflicts** action opening the same Phase 7 `ConflictResolver.tsx` that
  `PrDetail` opens. Both routes call **one** handler — no second implementation.
- **Prerequisite the resolver has never faced:** it needs a configured local
  clone (`repos.set`), but this view lists PRs from every repository, most of
  which will have none. The view already loads `repos.list` for its filters, so
  use that:
  - Clone configured → action enabled.
  - No clone → still show the action, but explain why it cannot run ("No local
    clone configured for acme/api") with a button jumping to the
    **Repositories** tab. Hiding it silently leaves the user unable to discover
    why the feature appears missing.
- `Mergeability::Unknown` means GitHub has not finished computing yet. Treat
  **only** an explicit `Conflicted` as conflicted; never flag on `Unknown`.

---

## 6. Part 2 — "Ready to merge" notification

`ItemKind::ReadyToMerge` — your authored PR is approved, green, not draft, and
mergeable. The one moment your own PR genuinely needs you.

- **Poller** (`crates/gitsurveild/src/github/client.rs`): restore the
  `authored:` alias, now fetching `reviewDecision`, `mergeable`, `isDraft`,
  `statusCheckRollup`. Emit an item **only** when *all* of:
  `reviewDecision == APPROVED`, `mergeable == MERGEABLE`, `!isDraft`, CI not
  failing. Authored PRs failing that test produce **nothing** — they belong to
  the view, not the inbox.
- **Base score 65** → High band. Below `ReviewRequested` (80, where someone
  else is blocked on you), above `Assigned` (40): your own work is one click
  from landing.
- Fires once, on the transition, because the existing diff marks it `New` only
  when it first appears.
- Wire the new variant through: `priority.rs` (`base_score` + the
  `base_scores_map_to_expected_severities` table test), `store.rs`
  (`kind_to_str`/`kind_from_str`), `notifications.rs` (`kind_label` → "Ready to
  merge"), `src/types.ts` (`ItemKind` union + `KIND_LABELS`),
  `src/desktop/Dashboard.tsx` (`ALL_KINDS`).

---

## 7. Implementation order

Each step builds, passes tests, and is committed on its own.

| # | Step | Commit message |
|---|---|---|
| 0 | Revert the two stray edits; confirm 69/23 green | *(no commit — restoring baseline)* |
| 1 | `PullRequestSummary` + friends in proto | `Add pull request summary types` |
| 2 | `list_pull_requests` + role-union merge + tests | `Add pull request listing with role merging` |
| 3 | `prs.list` API method | `Add prs.list API method` |
| 4 | Tauri command + TS types/ipc | `Expose pull request listing to the app` |
| 5 | `filters.ts` pure logic + tests | `Add pull request list filtering` |
| 6 | `PullRequests.tsx` + nav entry | `Add Pull Requests view` |
| 7 | Inline conflict resolution + missing-clone handling | `Resolve conflicts directly from the pull request list` |
| 8 | `ReadyToMerge` kind + poller derivation + tests | `Notify when your pull request is ready to merge` |
| 9 | Specs + README | `Document the Pull Requests view` |

---

## 8. Gotchas

| Risk | Mitigation |
|---|---|
| Duplicate rows for self-assigned authored PRs | `roles` is a set; merge by `(repo, number)`. Test it explicitly. |
| Notification spam about your own PRs | Only emit `ReadyToMerge`, and only on the full predicate. |
| `Mergeability::Unknown` treated as conflicted | Match only explicit `Conflicted`. GitHub computes this asynchronously. |
| Resolve action on a repo with no clone | Explain + link to Repositories; never fail obscurely. |
| Rate limits | One request per account per view open/refilter. Do not poll this view on a timer. |
| Reimplementing PR management | `PrDetail` and `ConflictResolver` already exist. Wire, don't rewrite. |
| Token leaking | `prs.list` returns summaries only; no token field anywhere near them. |

---

## 9. Acceptance criteria

Every criterion must pass. If you cannot demonstrate it, it has not passed. Do
not mark one done by reading your own code — run it.

> **Status** column is filled in by the implementing agent as criteria are
> demonstrated, and re-checked by the human at sign-off. `Pass` means the
> agent ran it; `Not verified` means it was not demonstrated (almost always
> because it needs a live token or a human click).

### AC-1 Baseline restored

| # | Criterion | Fail | Status |
|---|---|---|---|
| 1.1 | `cargo build --workspace` clean after the revert | Any error. | Pass |
| 1.2 | 69 Rust / 23 frontend tests pass before new work | Fewer, or any failure. | Pass |

### AC-2 Data correctness

| # | Criterion | Verify | Fail | Status |
|---|---|---|---|---|
| 2.1 | A PR in two result sets yields **one** summary with **two** roles | Unit test on the merge | Two rows, or one role. | Pass |
| 2.2 | Status filter maps to the right qualifier | `prs.list` with each state via `nc -U` | Merged returns open PRs, etc. | Not verified |
| 2.3 | `roles` never empty | Unit test | A summary with no role. | Pass |
| 2.4 | Multi-account results carry the right `account_id` | Two accounts configured | Rows attributed to the wrong account. | Not verified |

### AC-3 Filters

| # | Criterion | Fail | Status |
|---|---|---|---|
| 3.1 | Each dimension narrows correctly in isolation | Any filter returns wrong rows. | Pass |
| 3.2 | Filters combine as AND | Combining broadens instead of narrows. | Pass |
| 3.3 | Search matches title **and** repo, case-insensitively | Either missed. | Pass |
| 3.4 | Clearing filters restores the full list | Rows stay hidden. | Pass |
| 3.5 | Status change re-queries; others do not | A client-side filter triggers a network call, or status does not. | Not verified |

### AC-4 Conflict resolution from the list

| # | Criterion | Fail | Status |
|---|---|---|---|
| 4.1 | Conflicted rows visually flagged | Indistinguishable from clean ones. | Not verified |
| 4.2 | **Resolve conflicts** on the row opens the Phase 7 resolver | Missing, or opens something else. | Not verified |
| 4.3 | Row route and `PrDetail` route share one handler | Two implementations. | Pass |
| 4.4 | No configured clone → explains why + links to Repositories | Silently hidden, or an obscure error. | Not verified |
| 4.5 | `Unknown` mergeability is **not** flagged conflicted | A fresh PR shows a false conflict warning. | Pass |

### AC-5 Ready-to-merge notification

| # | Criterion | Fail | Status |
|---|---|---|---|
| 5.1 | Fires only when approved **and** mergeable **and** not draft **and** CI not failing | Any near-miss produces an item. Test all four. | Pass |
| 5.2 | Fires once, not every poll | Repeats on unchanged state. | Pass |
| 5.3 | Scores 65 → High band | Wrong band. | Pass |
| 5.4 | Merely-open authored PRs produce **no** action item | Opening a PR notifies you about it. | Pass |

### AC-6 Regressions

| # | Criterion | Command | Fail | Status |
|---|---|---|---|---|
| 6.1 | Rust tests up from 69 | `cargo test --workspace` | Fewer, or failure. | Pass |
| 6.2 | Frontend tests up from 23 | `pnpm test` | Fewer, or failure. | Pass |
| 6.3 | Docs clean | `cargo doc --no-deps` | Any warning. | Pass |
| 6.4 | Build clean | `cargo build --workspace` | Any warning. | Pass |
| 6.5 | Phases 1–7 still work | Popover, dashboard, dismiss, tray colour | Anything broke. | Not verified |
| 6.6 | Banned names absent | `grep -riE "gitify\|catlight" --include="*.rs" --include="*.ts" --include="*.tsx" README.md specs/ crates/ src/` | Any match. | Pass |
| 6.7 | No `Co-Authored-By` | `git log origin/main..HEAD --format=%B \| grep -i co-authored` | Any match. | Pass |

### AC-7 Documentation

| # | Criterion | Fail | Status |
|---|---|---|---|
| 7.1 | README documents the view and its filters | Undocumented. | Pass |
| 7.2 | README lists `prs.list` | Table stale. | Pass |
| 7.3 | README base-score table includes Ready to merge | Stale. | Pass |
| 7.4 | `specs/desktop-ui.md` describes the view | Spec and code disagree. | Pass |
| 7.5 | README claims only what ships | Any aspirational claim. | Pass |

### AC-8 End to end (**mandatory**, needs a real token)

This also closes a standing gap: **no PR operation has ever run against live
GitHub.** `pr.detail`'s mergeability mapping is unverified, and Phase 7 is
built on top of it.

Build with the Tauri CLI — a plain `cargo build` produces an app whose webview
points at the dev server and renders blank:

```bash
pnpm tauri build --config crates/gitsurveil-app/tauri.conf.json --no-bundle
cargo build --release -p gitsurveild
```

| # | Criterion | Fail | Status |
|---|---|---|---|
| 8.1 | Your own open PRs appear in the view | Empty when you have open PRs. | Not verified |
| 8.2 | A self-assigned authored PR appears **once**, two badges | Duplicated. | Not verified |
| 8.3 | Status → Merged shows merged PRs; back to Open restores | Wrong set. | Not verified |
| 8.4 | Click a PR → `PrDetail` loads with real data | Fails — this would also invalidate Phase 6. | Not verified |
| 8.5 | Conflicted PR + configured clone → resolve, commit, push, GitHub reports mergeable | Any step fails. | Not verified |
| 8.6 | Conflicted PR, no clone → explanation + link | Obscure failure. | Not verified |
| 8.7 | Approve your PR with CI green → "Ready to merge" notification, tray orange | No notification, or wrong severity. | Not verified |

### Sign-off

Done when AC-1…AC-8 pass **and a human has run the app and confirmed it**.
Report honestly which criteria you verified yourself and which you could not —
an unverified criterion is not a passing one. Prior phases shipped bugs only a
human clicking found: a duplicate tray icon, a stray title bar, a dead Quit
menu item.

> **Status as of this handover's implementation:** AC-1, AC-2.1, AC-2.3, AC-3.1–3.4,
> AC-4.3, AC-4.5, AC-5, AC-6.1–6.4, AC-6.6, AC-6.7, AC-7 pass (verified by unit
> tests + the listed commands). AC-2.2, AC-2.4, AC-3.5, AC-4.1, AC-4.2, AC-4.4,
> AC-6.5, and all of AC-8 are **not verified** — they need a live token, a
> runtime observation, or a human click. The human at sign-off should work
> through exactly those rows.

---

## 10. Conventions

**Commits** — group by type, one coherent change each. No `Co-Authored-By`
trailers ever. A dependency added *because* a feature needs it belongs with
that feature. If the message needs "and" for unrelated changes, split it.

**Naming** — two competitor product names are banned repo-wide; see
`.claude/rules/naming.md`. Describe behavior directly.

**Untracked** — `specs/`, `CLAUDE.md`, `.claude/` are deliberately untracked by
the owner's choice. They exist on disk and are authoritative: read them, keep
them updated, do not `git add` them without asking.

//! Session lifecycle for conflict resolution (`specs/conflict-resolver.md`).
//!
//! A `Session` is one in-progress resolution of a PR's conflicts. It lives in
//! a temporary worktree the daemon owns, never in the user's clone — that is
//! the phase's hard rule and the whole point of this module.
//!
//! Lifecycle, all `!Send` git2 work (callers wrap each function in
//! `tokio::task::spawn_blocking`):
//!
//! 1. [`prepare`] fetches the clone, verifies the user's checkout is clean
//!    (dirty → hard stop, AC-1.1), creates a worktree under the data dir,
//!    checks out the PR head, and merges the base in to produce the conflicted
//!    index. Returns the session plus per-file conflict summaries.
//! 2. Resolution edits the worktree's files (Step 4's `conflicts.save`).
//! 3. [`abort`] unregisters and deletes the worktree — idempotent, zero trace
//!    (AC-2.1, AC-2.2).
//! 4. [`prune_orphaned`] is the startup hook: worktrees a crash left behind
//!    are unregistered so they can't accumulate (AC-2.5).
//!
//! Public functions are wired into the socket layer (Step 4): `conflicts.*`
//! methods call these behind `spawn_blocking`.

use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    AnnotatedCommit, Cred, FetchOptions, MergeOptions, PushOptions, RemoteCallbacks, Repository,
    Status, WorktreePruneOptions,
};
use gitsurveil_proto::{ConflictFile, ConflictFileSummary};

use crate::error::{DaemonError, Result};

/// Every worktree the daemon creates is named with this prefix, so the
/// startup prune and `abort` can recognize (and never touch) their own.
pub const WORKTREE_PREFIX: &str = "gitsurveil-";

/// Files above this size get whole-file pick-ours/theirs only — the three-pane
/// editor is not virtualized (`specs/conflict-resolver.md`, "Edge cases").
const MAX_EDITABLE_BYTES: u64 = 5 * 1024 * 1024;

/// Everything [`prepare`] needs, gathered by the socket layer from the PR
/// detail (base/head branches) and the account (login/token).
pub struct PrepareInputs {
    /// `"owner/name"` as configured in `repos.set`.
    pub repo: String,
    /// Base branch name (where the PR merges into).
    pub base: String,
    /// Head branch name (the PR's own branch).
    pub head: String,
    /// The user's local clone of `repo`.
    pub clone_path: PathBuf,
    /// Parent directory for temp worktrees — always the data dir, never
    /// inside the clone (AC-1.3).
    pub worktree_root: PathBuf,
    /// Account login, used when the remote URL carries no username.
    pub login: String,
    /// Account token, used for the fetch; also kept for the later push.
    pub token: String,
}

/// One live conflict-resolution session.
#[derive(Debug, Clone)]
pub struct Session {
    /// The `"owner/name"` of the repo. Also the session id the API uses:
    /// there is exactly one session per repo, so the slug addresses it.
    pub id: String,
    pub base: String,
    pub head: String,
    pub clone_path: PathBuf,
    pub worktree_path: PathBuf,
    /// Name of the worktree (and of its branch) in the clone.
    pub worktree_name: String,
    pub login: String,
    pub token: String,
    /// Paths of the files this session must resolve. `conflicts.commit` checks
    /// exactly these for leftover markers and stages exactly these — it never
    /// touches anything else in the worktree.
    pub conflicted_paths: Vec<String>,
}

/// Fetches origin, checks the user's clone is clean, creates the temp
/// worktree, checks out the PR head, and merges the base in.
///
/// Returns the session plus one [`ConflictFileSummary`] per conflicted file.
/// On any failure after the worktree was created, the worktree is removed
/// before the error propagates — a failed prepare leaves zero trace. Errors
/// carry the `config_error` code via [`DaemonError::Config`] (AC-4.7).
pub fn prepare(inputs: &PrepareInputs) -> Result<(Session, Vec<ConflictFileSummary>)> {
    let repo = Repository::open(&inputs.clone_path).map_err(|e| {
        DaemonError::Config(format!("{} is not a git repository: {e}", inputs.clone_path.display()))
    })?;

    fetch_origin(&repo, &inputs.login, &inputs.token)?;

    if !worktree_is_clean(&repo)? {
        return Err(DaemonError::Config(
            "your clone has uncommitted changes — commit or stash first; \
             gitsurveil never touches your working tree"
                .into(),
        ));
    }

    let worktree_name = worktree_name_for(&inputs.repo);
    let worktree_path = inputs.worktree_root.join(&worktree_name);
    // A crash before prune can leave the branch and directory behind; clear
    // them so `worktree()` starts from a clean slate.
    if let Ok(mut branch) = repo.find_branch(&worktree_name, git2::BranchType::Local) {
        branch.delete().ok();
    }
    if worktree_path.exists() {
        std::fs::remove_dir_all(&worktree_path)?;
    }
    std::fs::create_dir_all(&inputs.worktree_root)?;

    let result = repo
        .worktree(&worktree_name, &worktree_path, None)
        .map_err(|e| {
            DaemonError::Config(format!(
                "could not create the temp worktree (git worktree support is required): {e}"
            ))
        })
        .and_then(|_| setup_worktree(inputs, &worktree_name, &worktree_path));
    match result {
        Err(e) => {
            let _ = prune_worktree(&repo, &worktree_name, &worktree_path);
            Err(scrub_error(e, &inputs.token))
        }
        Ok(session) => Ok(session),
    }
}

/// Removes a session's worktree, its directory, and the daemon's local branch
/// for it. Idempotent: a missing worktree or directory is success, so `abort`
/// can be called twice (AC-2.2) and a re-entrant teardown can't wedge.
pub fn abort(session: &Session) -> Result<()> {
    let repo = Repository::open(&session.clone_path)?;
    prune_worktree(&repo, &session.worktree_name, &session.worktree_path)?;
    if let Ok(mut branch) = repo.find_branch(&session.worktree_name, git2::BranchType::Local) {
        branch.delete().ok();
    }
    Ok(())
}

/// Unregisters every daemon-owned worktree in a clone. Run at startup against
/// each configured repo so sessions killed with the daemon don't leave
/// registered worktrees behind (AC-2.5).
pub fn prune_orphaned(clone_path: &Path) -> Result<()> {
    let repo = Repository::open(clone_path)?;
    let names: Vec<String> = repo
        .worktrees()?
        .iter()
        .filter_map(|s| s.ok().flatten().map(str::to_owned))
        .filter(|name| name.starts_with(WORKTREE_PREFIX))
        .collect();
    for name in names {
        if let Ok(worktree) = repo.find_worktree(&name) {
            let path = worktree.path().to_path_buf();
            let _ = prune_worktree(&repo, &name, &path);
        }
    }
    Ok(())
}

/// The worktree name for a repo slug, e.g. `acme/api` →
/// `gitsurveil-acme-api`. Used as the worktree's name, directory, and branch.
pub fn worktree_name_for(repo: &str) -> String {
    format!("{WORKTREE_PREFIX}{}", repo.replace('/', "-"))
}

/// Reads one file from the worktree and parses its conflict regions
/// (`conflicts.file`). Binary and over-threshold files return no segments —
/// the UI offers whole-file pick for those instead (AC-5.5).
pub fn read_file(session: &Session, path: &str) -> Result<ConflictFile> {
    let full = worktree_file_path(session, path)?;
    let bytes = std::fs::read(&full)?;
    let binary = is_binary(&bytes);
    let large = bytes.len() as u64 > MAX_EDITABLE_BYTES;
    let segments = if binary || large {
        Vec::new()
    } else {
        crate::conflicts::parse::parse_conflicts(&String::from_utf8_lossy(&bytes))
    };
    Ok(ConflictFile {
        path: path.to_string(),
        binary,
        large,
        segments,
    })
}

/// Writes resolved content into a worktree file (`conflicts.save`). The
/// content is byte-for-byte what the UI's center pane produced, so the file
/// on disk always matches what a subsequent `conflicts.file` reports (AC-4.3)
/// and ultimately what gets committed (AC-6.5).
pub fn save_file(session: &Session, path: &str, content: &str) -> Result<()> {
    let full = worktree_file_path(session, path)?;
    std::fs::write(&full, content)?;
    Ok(())
}

/// Copies one whole conflicted file from a side of the index into the
/// worktree (`conflicts.save` with `pick`). This is the only way binary and
/// >5 MB files can be resolved, since their content never enters the UI.
/// `ours` is the PR head branch, `theirs` the base (matching the panes).
pub fn pick_file(session: &Session, path: &str, ours: bool) -> Result<()> {
    let repo = Repository::open(&session.worktree_path)?;
    let index = repo.index()?;
    let mut conflicts = index.conflicts()?;
    while let Some(conflict) = conflicts.next() {
        let conflict = conflict?;
        let side = if ours {
            conflict.our.as_ref()
        } else {
            conflict.their.as_ref()
        };
        if let Some(entry) = side {
            let entry_path = String::from_utf8_lossy(&entry.path).into_owned();
            if entry_path == path {
                let blob = repo.find_blob(entry.id)?;
                let full = worktree_file_path(session, path)?;
                std::fs::write(&full, blob.content())?;
                return Ok(());
            }
        }
    }
    Err(DaemonError::Config(format!(
        "{path} is not a conflicted file in this session"
    )))
}

/// Stages the resolved files and creates the merge commit. Refuses a commit
/// while any conflicted file still contains marker lines — the daemon-side
/// guard against pushing `<<<<<<<` to a shared branch (AC-4.4, release
/// blocker). Leaves the worktree in place: `conflicts.push` owns the teardown,
/// so `abort` stays possible from every state (AC-2.3).
pub fn commit_resolution(session: &Session, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        return Err(DaemonError::InvalidParams(
            "a commit message is required".into(),
        ));
    }
    let repo = Repository::open(&session.worktree_path)?;
    for path in &session.conflicted_paths {
        let content = std::fs::read(worktree_file_path(session, path)?)?;
        if contains_markers(&String::from_utf8_lossy(&content)) {
            return Err(DaemonError::Config(format!(
                "{path} still contains conflict markers — resolve it before committing"
            )));
        }
    }
    let signature = repo.signature().map_err(|_| {
        DaemonError::Config(
            "the clone has no git identity — set user.name and user.email in its config".into(),
        )
    })?;

    let mut index = repo.index()?;
    for path in &session.conflicted_paths {
        index.add_path(Path::new(path))?;
    }
    index.write()?;
    let tree = repo.find_tree(index.write_tree_to(&repo)?)?;

    let head_ref = format!("refs/remotes/origin/{}", session.head);
    let base_ref = format!("refs/remotes/origin/{}", session.base);
    let head = repo.find_reference(&head_ref)?.peel_to_commit()?;
    let base = repo.find_reference(&base_ref)?.peel_to_commit()?;

    let branch_ref = format!("refs/heads/{}", session.worktree_name);
    repo.commit(
        Some(&branch_ref),
        &signature,
        &signature,
        message,
        &tree,
        &[&head, &base],
    )?;
    Ok(())
}

/// Pushes the resolution branch to the PR's head on origin. On success the
/// worktree, its branch, and (via the socket layer) the session are torn down.
/// On rejection nothing is torn down — the user can fix and retry or abort —
/// and git's error is surfaced with the token scrubbed (AC-4.5, AC-4.8).
pub fn push_resolution(session: &Session) -> Result<()> {
    let repo = Repository::open(&session.clone_path)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| DaemonError::Config(format!("no `origin` remote: {e}")))?;
    let mut callbacks = RemoteCallbacks::new();
    let token = session.token.clone();
    let login = session.login.clone();
    callbacks.credentials(move |_url, username, _allowed| {
        Cred::userpass_plaintext(username.unwrap_or(&login), &token)
    });
    let mut options = PushOptions::new();
    options.remote_callbacks(callbacks);
    let refspec = format!(
        "refs/heads/{}:refs/heads/{}",
        session.worktree_name, session.head
    );
    remote
        .push(&[refspec], Some(&mut options))
        .map_err(|e| DaemonError::Config(scrub_token(&e.to_string(), &session.token)))?;
    abort(session)
}

/// Whether text still contains unresolved conflict marker lines. The commit
/// gate checks this directly rather than trusting the parser, so even a
/// malformed block the parser degrades to context still blocks the commit.
fn contains_markers(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.starts_with("<<<<<<<") || line.starts_with(">>>>>>>"))
}

/// Resolves a worktree-relative path and rejects anything that would escape
/// the worktree root (a session address can't legitimately contain `..`).
fn worktree_file_path(session: &Session, path: &str) -> Result<PathBuf> {
    if path.split('/').any(|part| part == "..") {
        return Err(DaemonError::InvalidParams(format!(
            "{path:?} escapes the worktree"
        )));
    }
    Ok(session.worktree_path.join(path))
}

/// Performs the parts of `prepare` that run after the worktree exists: point
/// the worktree at the PR head, merge the base in, and summarize the
/// conflicts. On a clean merge there is nothing to resolve, which is an error
/// (the caller tears the worktree down).
fn setup_worktree(
    inputs: &PrepareInputs,
    worktree_name: &str,
    worktree_path: &Path,
) -> Result<(Session, Vec<ConflictFileSummary>)> {
    let wt_repo = Repository::open(worktree_path)?;

    let head_ref = format!("refs/remotes/origin/{}", inputs.head);
    let head_commit = wt_repo
        .find_reference(&head_ref)
        .map_err(|_| {
            DaemonError::Config(format!(
                "the PR head branch `{}` was not found on origin (fork PRs need \
                 auto-clone support and aren't supported yet)",
                inputs.head
            ))
        })?
        .peel_to_commit()?;

    // The worktree's branch starts at the clone's HEAD; move it to the PR head
    // commit before merging, so the merge commit we produce later sits on top
    // of the PR branch exactly where a resolution should.
    let branch_ref = format!("refs/heads/{worktree_name}");
    wt_repo.reference(&branch_ref, head_commit.id(), true, "point at PR head")?;
    wt_repo.set_head(&branch_ref)?;
    wt_repo
        .checkout_head(Some(CheckoutBuilder::new().force()))
        .map_err(|e| DaemonError::Config(format!("could not check out the PR head: {e}")))?;

    let base_ref = format!("refs/remotes/origin/{}", inputs.base);
    let base_commit = wt_repo
        .find_reference(&base_ref)
        .map_err(|_| {
            DaemonError::Config(format!(
                "the base branch `{}` was not found on origin",
                inputs.base
            ))
        })?
        .peel_to_commit()?;
    let base_annotated: AnnotatedCommit<'_> = wt_repo.find_annotated_commit(base_commit.id())?;
    wt_repo
        .merge(&[&base_annotated], Some(&mut MergeOptions::new()), Some(&mut CheckoutBuilder::new()))
        .map_err(|e| DaemonError::Config(format!("could not reproduce the PR merge: {e}")))?;

    let files = conflicted_file_summaries(&wt_repo, worktree_path)?;
    if files.is_empty() {
        return Err(DaemonError::Config(
            "the PR merges cleanly — there are no conflicts to resolve".into(),
        ));
    }

    Ok((
        Session {
            id: inputs.repo.clone(),
            base: inputs.base.clone(),
            head: inputs.head.clone(),
            clone_path: inputs.clone_path.clone(),
            worktree_path: worktree_path.to_path_buf(),
            worktree_name: worktree_name.to_string(),
            login: inputs.login.clone(),
            token: inputs.token.clone(),
            conflicted_paths: files.iter().map(|f| f.path.clone()).collect(),
        },
        files,
    ))
}

/// Summaries for every conflicted file in a worktree's index. Binary files
/// and files above the editable-size threshold report a single whole-file
/// "conflict"; text files report the marker-block count.
fn conflicted_file_summaries(
    repo: &Repository,
    worktree_path: &Path,
) -> Result<Vec<ConflictFileSummary>> {
    let mut files = Vec::new();
    let index = repo.index()?;
    let mut conflicts = index.conflicts()?;
    while let Some(conflict) = conflicts.next() {
        let conflict = conflict?;
        let entry = conflict
            .our
            .as_ref()
            .or(conflict.their.as_ref())
            .or(conflict.ancestor.as_ref());
        let path = entry
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        let bytes = std::fs::read(worktree_path.join(&path))?;
        let binary = is_binary(&bytes);
        let large = bytes.len() as u64 > MAX_EDITABLE_BYTES;
        let count = if binary || large {
            1
        } else {
            crate::conflicts::parse::conflict_count(&String::from_utf8_lossy(&bytes))
        };
        files.push(ConflictFileSummary {
            path: path.to_string(),
            conflicts: count,
            binary,
            large,
        });
    }
    Ok(files)
}

/// Fetches all origin branches into `refs/remotes/origin/*`, using the
/// account token as HTTPS credentials. Network errors map to
/// [`DaemonError::Config`] with the token scrubbed (AC-4.8).
fn fetch_origin(repo: &Repository, login: &str, token: &str) -> Result<()> {
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| DaemonError::Config(format!("no `origin` remote: {e}")))?;
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, username, _allowed| {
        Cred::userpass_plaintext(username.unwrap_or(login), token)
    });
    let mut options = FetchOptions::new();
    options.remote_callbacks(callbacks);
    remote
        .fetch(
            &["+refs/heads/*:refs/remotes/origin/*"],
            Some(&mut options),
            None,
        )
        .map_err(|e| DaemonError::Config(scrub_token(&e.to_string(), token)))
        .map(|_| ())
}

/// True when the clone has no changes on tracked files (staged or unstaged).
/// Untracked files don't block — nothing we do can disturb them — but any
/// modification of a tracked file means "commit or stash first" (AC-1.1).
fn worktree_is_clean(repo: &Repository) -> Result<bool> {
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true);
    let statuses = repo.statuses(Some(&mut options))?;
    for entry in statuses.iter() {
        let status = entry.status();
        if status.contains(Status::IGNORED) {
            continue;
        }
        if status == Status::WT_NEW {
            continue;
        }
        if !status.is_empty() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Unregisters a worktree and removes its directory. `prune` with
/// `working_tree(true)` clears the registration even though the worktree is
/// mid-merge; the directory still has to go by hand (git2 has no
/// `remove_worktree` — verified in the Phase 7 spike).
fn prune_worktree(repo: &Repository, name: &str, path: &Path) -> Result<()> {
    if let Ok(worktree) = repo.find_worktree(name) {
        let mut options = WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        worktree.prune(Some(&mut options)).ok();
    }
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Git's binary heuristic: a NUL byte in the first 8 KiB (the same window git
/// itself uses before refusing a text merge). Binary files get whole-file pick
/// only, since marker parsing is meaningless for them.
fn is_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(8000)];
    window.contains(&0)
}

/// Replaces the token with a placeholder anywhere it leaked into an error
/// string (a git error echoing the remote URL would otherwise carry it).
fn scrub_error(error: DaemonError, token: &str) -> DaemonError {
    match error {
        DaemonError::Config(message) => DaemonError::Config(scrub_token(&message, token)),
        other => other,
    }
}

/// Removes `token` from `message`. A token is high-entropy; a substring
/// replace is the practical scrub.
fn scrub_token(message: &str, token: &str) -> String {
    if token.is_empty() {
        message.to_string()
    } else {
        message.replace(token, "***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// `git` CLI runner scoped to a directory; fixtures are built with the
    /// CLI (like `gitops.rs`) so the tests don't hand-roll objects.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    struct Fixture {
        clone: PathBuf,
        worktree_root: PathBuf,
    }

    /// Builds an offline, real-git fixture: a bare "remote" plus a clone of it
    /// with a divergent `main` and `feature` (both pushed, so the clone's
    /// `refs/remotes/origin/*` is populated and `prepare`'s fetch is a local
    /// no-op). Returns the clone and a scratch worktree root.
    fn fixture() -> Fixture {
        let base = std::env::temp_dir().join(format!("gs-session-{}", uuid::Uuid::new_v4()));
        let remote = base.join("remote.git");
        let clone = base.join("clone");
        std::fs::create_dir_all(&base).unwrap();

        let remote_url = remote.to_str().unwrap();
        let clone_url = clone.to_str().unwrap();
        git(&base, &["init", "--bare", "-b", "main", remote_url]);
        git(&base, &["clone", remote_url, clone_url]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "Test"]);

        std::fs::write(clone.join("file.txt"), "base content\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "initial"]);
        git(&clone, &["push", "-u", "origin", "main"]);

        // feature branch changes the file; main changes it differently →
        // a genuine conflict when feature is merged with main.
        git(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("file.txt"), "feature content\n").unwrap();
        git(&clone, &["commit", "-am", "feature change"]);
        git(&clone, &["push", "-u", "origin", "feature"]);

        git(&clone, &["checkout", "main"]);
        std::fs::write(clone.join("file.txt"), "main content\n").unwrap();
        git(&clone, &["commit", "-am", "main change"]);
        git(&clone, &["push", "-u", "origin", "main"]);

        Fixture {
            clone,
            worktree_root: base.join("worktrees"),
        }
    }

    fn inputs(fixture: &Fixture) -> PrepareInputs {
        PrepareInputs {
            repo: "acme/api".into(),
            base: "main".into(),
            head: "feature".into(),
            clone_path: fixture.clone.clone(),
            worktree_root: fixture.worktree_root.clone(),
            login: "octocat".into(),
            token: "test-token".into(),
        }
    }

    fn cleanup(fixture: &Fixture) {
        let _ = std::fs::remove_dir_all(fixture.clone.parent().unwrap());
    }

    #[test]
    fn prepare_lists_conflicts_and_leaves_the_clone_untouched() {
        let fixture = fixture();
        let (session, files) = prepare(&inputs(&fixture)).expect("prepare should succeed");
        assert_eq!(session.id, "acme/api");
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "file.txt");
        assert!(file.conflicts >= 1);
        assert!(!file.binary);
        assert!(session.worktree_path.exists(), "worktree directory must exist");
        assert_eq!(session.worktree_name, "gitsurveil-acme-api");

        let repo = Repository::open(&fixture.clone).unwrap();
        assert_eq!(
            repo.head().unwrap().name().unwrap(),
            "refs/heads/main",
            "the user's checked-out branch must not change"
        );
        assert!(worktree_is_clean(&repo).unwrap(), "the user's clone must stay clean");

        abort(&session).unwrap();
        cleanup(&fixture);
    }

    #[test]
    fn prepare_refuses_a_dirty_clone_and_creates_nothing() {
        let fixture = fixture();
        let dirty = fixture.clone.join("file.txt");
        std::fs::write(&dirty, "uncommitted edit\n").unwrap();

        let err = prepare(&inputs(&fixture)).unwrap_err();
        std::fs::write(&dirty, "main content\n").unwrap(); // restore for teardown
        assert!(err.to_string().contains("commit or stash first"));

        let repo = Repository::open(&fixture.clone).unwrap();
        let names: Vec<String> = repo.worktrees().unwrap().iter().filter_map(|n| n.ok().flatten().map(str::to_owned)).collect();
        assert!(
            !names.iter().any(|n| n.starts_with(WORKTREE_PREFIX)),
            "a dirty clone must never get a worktree"
        );
        cleanup(&fixture);
    }

    #[test]
    fn abort_is_idempotent_and_leaves_zero_trace() {
        let fixture = fixture();
        let (session, _) = prepare(&inputs(&fixture)).unwrap();

        abort(&session).expect("first abort should succeed");
        abort(&session).expect("second abort must be a benign no-op");

        assert!(!session.worktree_path.exists(), "worktree directory must be gone");
        let repo = Repository::open(&fixture.clone).unwrap();
        let names: Vec<String> = repo.worktrees().unwrap().iter().filter_map(|n| n.ok().flatten().map(str::to_owned)).collect();
        assert!(
            !names.iter().any(|n| n.starts_with(WORKTREE_PREFIX)),
            "no daemon worktree may remain registered"
        );
        assert!(worktree_is_clean(&repo).unwrap(), "the clone's state must be untouched");
        cleanup(&fixture);
    }

    #[test]
    fn prepare_failure_tears_the_worktree_down() {
        let fixture = fixture();
        let mut inputs = inputs(&fixture);
        inputs.head = "no-such-branch".into();
        let err = prepare(&inputs).unwrap_err();
        assert!(err.to_string().contains("not found on origin"));

        let repo = Repository::open(&fixture.clone).unwrap();
        let names: Vec<String> = repo.worktrees().unwrap().iter().filter_map(|n| n.ok().flatten().map(str::to_owned)).collect();
        assert!(
            !names.iter().any(|n| n.starts_with(WORKTREE_PREFIX)),
            "a failed prepare must clean up after itself"
        );
        cleanup(&fixture);
    }

    #[test]
    fn prune_orphaned_clears_crash_left_worktrees() {
        let fixture = fixture();
        // Simulate a crash: prepare a session and never abort it.
        let (session, _) = prepare(&inputs(&fixture)).unwrap();

        prune_orphaned(&fixture.clone).expect("prune should remove the orphan");
        assert!(!session.worktree_path.exists(), "orphaned directory must be removed");
        let repo = Repository::open(&fixture.clone).unwrap();
        let names: Vec<String> = repo.worktrees().unwrap().iter().filter_map(|n| n.ok().flatten().map(str::to_owned)).collect();
        assert!(!names.iter().any(|n| n.starts_with(WORKTREE_PREFIX)));
        cleanup(&fixture);
    }

    #[test]
    fn commit_refuses_leftover_markers_and_then_commits_a_resolution() {
        let fixture = fixture();
        let (session, _) = prepare(&inputs(&fixture)).unwrap();

        // Leave the conflicted file untouched → the commit gate must refuse it.
        let err = commit_resolution(&session, "merge main").unwrap_err();
        assert!(
            err.to_string().contains("still contains conflict markers"),
            "commit must refuse unresolved markers, got: {err}"
        );

        // Resolve it (take "theirs") and commit.
        pick_file(&session, "file.txt", false).unwrap();
        commit_resolution(&session, "merge main").expect("a resolved commit should succeed");

        let repo = Repository::open(&session.worktree_path).unwrap();
        let head = repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        assert_eq!(
            commit.parent_count(),
            2,
            "the resolution commit must merge head + base"
        );
        let message = commit.message().unwrap();
        assert_eq!(message, "merge main");

        abort(&session).unwrap();
        cleanup(&fixture);
    }

    #[test]
    fn save_persists_then_file_reports_the_new_content() {
        let fixture = fixture();
        let (session, files) = prepare(&inputs(&fixture)).unwrap();
        let path = files[0].path.clone();

        let before = read_file(&session, &path).unwrap();
        assert!(!before.segments.is_empty(), "a conflicted file has segments");

        save_file(&session, &path, "resolved content\n").unwrap();
        let after = read_file(&session, &path).unwrap();
        assert!(
            after.segments.is_empty() || after.segments.iter().all(|s| matches!(s, gitsurveil_proto::ConflictSegment::Context { .. })),
            "saved content must parse back with no conflict hunks"
        );

        abort(&session).unwrap();
        cleanup(&fixture);
    }

    #[test]
    fn push_ships_the_resolution_to_origin_and_tears_down() {
        let fixture = fixture();
        let (session, _) = prepare(&inputs(&fixture)).unwrap();
        pick_file(&session, "file.txt", false).unwrap();
        commit_resolution(&session, "merge main").unwrap();

        push_resolution(&session).expect("push to the bare origin should succeed");

        assert!(!session.worktree_path.exists(), "worktree must be torn down after push");
        let repo = Repository::open(&fixture.clone).unwrap();
        let names: Vec<String> = repo.worktrees().unwrap().iter().filter_map(|n| n.ok().flatten().map(str::to_owned)).collect();
        assert!(!names.iter().any(|n| n.starts_with(WORKTREE_PREFIX)));

        // origin/feature must now point at the resolution commit.
        let remote = repo.find_reference("refs/remotes/origin/feature").unwrap();
        assert_eq!(
            remote.peel_to_commit().unwrap().message().unwrap(),
            "merge main",
            "the pushed branch must carry the resolution commit"
        );
        cleanup(&fixture);
    }
}

//! Per-repo worktree management (`specs/desktop-ui.md`, "Worktrees").
//!
//! User-created worktrees are *derived* from the clone's git metadata on every
//! request — there is no table for them, so worktrees created or removed with
//! the git CLI, an IDE, or another tool show up too. Three hard rules shape
//! this module:
//!
//! - **Never touch the user's working tree.** This module reads the main
//!   checkout and registers new worktrees elsewhere; it never writes to the
//!   clone itself.
//! - **Never overwrite.** A worktree target that already exists and is not
//!   empty is an error, never a cleanup opportunity (hard rule in `CLAUDE.md`).
//! - **Never touch conflict sessions.** Worktrees named `gitsurveil-*` belong
//!   to the conflict resolver; they're transient, pruned at startup, and the
//!   removal path refuses them outright.
//!
//! Like `gitops.rs`, every call here is cheap and self-contained (`git2`
//! objects are `!Send`), so async callers wrap them in
//! `tokio::task::spawn_blocking`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{BranchType, Repository, WorktreePruneOptions};

use crate::conflicts::session::WORKTREE_PREFIX;
use crate::error::{DaemonError, Result};
use gitsurveil_proto::{WorktreeInfo, WorktreesResult};

/// Lists a repo's user-created worktrees and the branches a new one can be
/// created from (`repos.worktrees`). Conflict-session worktrees
/// (`gitsurveil-*`) are filtered out, as are worktrees that fail to reopen.
///
/// `branches` is every local branch plus every `origin/*` remote-tracking
/// branch that doesn't shadow a local one, presented as short names — typing
/// one back into `add` resolves to the existing branch, exactly like
/// `git worktree add`.
pub fn list(clone_path: &Path) -> Result<WorktreesResult> {
    let repo = Repository::open(clone_path)?;

    let mut worktrees = Vec::new();
    for name in repo.worktrees()?.iter().filter_map(|s| s.ok().flatten()) {
        if name.starts_with(WORKTREE_PREFIX) {
            continue;
        }
        if let Ok(worktree) = repo.find_worktree(name) {
            if let Ok(info) = worktree_info(&worktree) {
                worktrees.push(info);
            }
        }
    }

    let mut branches: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for branch in repo.branches(Some(BranchType::Local))? {
        if let Some((b, _)) = branch.ok() {
            if let Some(name) = b.name().ok().flatten() {
                seen.insert(name.to_string());
                branches.push(name.to_string());
            }
        }
    }
    for branch in repo.branches(Some(BranchType::Remote))? {
        if let Some((b, _)) = branch.ok() {
            if let Some(name) = b.name().ok().flatten() {
                // `origin/x` is offered as `x`; if a local `x` exists it's already
                // in the list, and if it doesn't, the remote name resolves to it.
                let short = name.strip_prefix("origin/").unwrap_or(name);
                if seen.insert(short.to_string()) {
                    branches.push(short.to_string());
                }
            }
        }
    }
    branches.sort();

    Ok(WorktreesResult { worktrees, branches })
}

/// Creates a worktree for `branch` at `path` and checks the branch out there
/// (`repos.worktree_add`). The worktree name is derived from the path's last
/// component, mirroring how the git CLI names worktrees.
///
/// Branch resolution follows the git CLI (`git worktree add`): an existing
/// local branch is checked out; otherwise an `origin/{branch}` remote is
/// resolved into a fresh local branch tracking it; otherwise `branch` is
/// created new at the clone's HEAD. The branch is never created or moved in
/// the user's main checkout.
pub fn add(clone_path: &Path, branch: &str, path: &str) -> Result<WorktreeInfo> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(DaemonError::InvalidParams(
            "a branch name is required".into(),
        ));
    }
    let repo = Repository::open(clone_path)?;

    let target = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        // Relative paths resolve against the clone's parent, matching the
        // `wt-{owner}-{name}-{branch}` sibling default the UI pre-fills.
        clone_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    };
    if target == clone_path {
        return Err(DaemonError::Config(
            "the worktree path must differ from the clone itself".into(),
        ));
    }
    // Hard rule: a pre-existing non-empty target is an error, not a cleanup
    // opportunity — the daemon never deletes what it didn't create.
    if target.exists() && std::fs::read_dir(&target)?.next().is_some() {
        return Err(DaemonError::Config(format!(
            "{} already exists and is not empty — pick a different path",
            target.display()
        )));
    }

    let (short_name, branch_ref, created_branch) = resolve_branch(&repo, branch)?;
    if let Some(where_) = branch_checked_out_elsewhere(&repo, &short_name)? {
        return Err(DaemonError::Config(format!(
            "branch `{short_name}` is already checked out in {where_} — pick another branch"
        )));
    }

    let name = worktree_name_from_path(&target)?;
    if repo.find_worktree(&name).is_ok() {
        return Err(DaemonError::Config(format!(
            "a worktree named `{name}` already exists for this repo"
        )));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let worktree = repo
        .worktree(&name, &target, None)
        .map_err(|e| DaemonError::Config(format!("could not create the worktree: {e}")))?;

    // Point the new worktree at the resolved branch and materialize its files.
    // The initial `worktree` call leaves HEAD detached at the clone's HEAD; the
    // branch ref and a force checkout finish the job, as `conflicts/session.rs`
    // does for its temp worktrees.
    let result = (|| {
        let wt = Repository::open(&target)?;
        wt.set_head(&branch_ref)?;
        wt.checkout_head(Some(CheckoutBuilder::new().force()))
            .map_err(|e| DaemonError::Config(format!("could not check out `{short_name}`: {e}")))?;
        Ok(())
    })();
    if let Err(e) = result {
        // Roll the half-created worktree back: unregister it, drop the
        // directory, and remove a branch this call created.
        let mut prune = WorktreePruneOptions::new();
        prune.valid(true).working_tree(true);
        worktree.prune(Some(&mut prune)).ok();
        std::fs::remove_dir_all(&target).ok();
        if created_branch {
            if let Ok(mut branch) = repo.find_branch(&short_name, BranchType::Local) {
                branch.delete().ok();
            }
        }
        return Err(e);
    }

    worktree_info(&worktree)
}

/// Removes a registered worktree and its working directory (`repos.worktree_remove`).
///
/// The checked-out branch is kept — the worktree is only unregistered and its
/// files removed, matching `git worktree remove`. A worktree with uncommitted
/// changes or untracked files is refused, as is any `gitsurveil-*` conflict
/// session. Pass `force` to skip the dirty-check (uncommitted changes are
/// silently discarded, matching `git worktree remove --force`).
pub fn remove(clone_path: &Path, name: &str, force: bool) -> Result<()> {
    let repo = Repository::open(clone_path)?;
    if name.starts_with(WORKTREE_PREFIX) {
        return Err(DaemonError::Config(format!(
            "{name} is a gitsurveil conflict-session worktree and can't be removed here"
        )));
    }
    let worktree = repo.find_worktree(name).map_err(|_| {
        DaemonError::Config(format!(
            "no worktree named `{name}` is registered for this repo"
        ))
    })?;

    if !force {
        let wt_path = worktree.path().to_path_buf();
        let wt = Repository::open(&wt_path)?;
        if !wt.statuses(None)?.iter().next().is_none() {
            return Err(DaemonError::Config(format!(
                "the worktree at {} has uncommitted changes or untracked files — \
                 commit or stash them before deleting",
                wt_path.display()
            )));
        }
    }

    let mut prune = WorktreePruneOptions::new();
    prune.valid(true).working_tree(true);
    worktree.prune(Some(&mut prune)).map_err(|e| {
        DaemonError::Config(format!("could not remove the worktree: {e}"))
    })?;
    Ok(())
}

/// Resolves a typed branch name to `(short_name, refs/heads/... reference, created)`.
///
/// Priority: an existing local branch; else an `origin/{name}` remote (creating
/// a local tracking branch); else a brand-new branch at the clone's HEAD (the
/// `git worktree add -b` path). `created` reports whether the branch was made
/// by this call so a failed add can clean it up.
fn resolve_branch(repo: &Repository, branch: &str) -> Result<(String, String, bool)> {
    if let Ok(b) = repo.find_branch(branch, BranchType::Local) {
        let short = b.name().ok().flatten().unwrap_or(branch).to_string();
        let full = b.get().name()?.to_string();
        return Ok((short, full, false));
    }

    let remote_short = branch.strip_prefix("origin/").unwrap_or(branch);
    let remote_full = format!("refs/remotes/origin/{remote_short}");
    if repo.find_reference(&remote_full).is_ok() {
        if let Ok(local) = repo.find_branch(remote_short, BranchType::Local) {
            let short = local.name().ok().flatten().unwrap_or(remote_short).to_string();
            let full = local.get().name()?.to_string();
            return Ok((short, full, false));
        }
        let commit = repo.find_reference(&remote_full)?.peel_to_commit()?;
        repo.branch(remote_short, &commit, false).map_err(|e| {
            DaemonError::Config(format!("could not create branch `{remote_short}`: {e}"))
        })?;
        return Ok((remote_short.to_string(), remote_full.replace("remotes/origin", "heads"), true));
    }

    let head = repo.head().map_err(|_| {
        DaemonError::Config("the clone has no HEAD yet — nothing to branch from".into())
    })?;
    let commit = head.peel_to_commit()?;
    repo.branch(branch, &commit, false).map_err(|e| {
        DaemonError::Config(format!("could not create branch `{branch}`: {e}"))
    })?;
    Ok((branch.to_string(), format!("refs/heads/{branch}"), true))
}

/// Where `branch` is currently checked out, if anywhere. `None` means the
/// branch is free for a new worktree. Mirrors git's refusal to check the same
/// branch out twice.
fn branch_checked_out_elsewhere(repo: &Repository, branch: &str) -> Result<Option<String>> {
    if current_branch(repo).as_deref() == Some(branch) {
        return Ok(Some("the main checkout".into()));
    }
    for name in repo.worktrees()?.iter().filter_map(|s| s.ok().flatten()) {
        if name.starts_with(WORKTREE_PREFIX) {
            continue;
        }
        let Some(worktree) = repo.find_worktree(name).ok() else { continue };
        let Ok(wt) = Repository::open(worktree.path()) else { continue };
        if current_branch(&wt).as_deref() == Some(branch) {
            return Ok(Some(format!("worktree `{name}`")));
        }
    }
    Ok(None)
}

/// The checked-out branch short name of `repo`, or `None` when HEAD is
/// detached or unreadable.
fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    head.shorthand().ok().map(str::to_string)
}

/// A git-safe worktree name from the target path's last component, like the
/// git CLI's name-from-path convention.
fn worktree_name_from_path(path: &Path) -> Result<String> {
    let base = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| DaemonError::Config("the worktree path has no usable file name".into()))?;
    let name: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        return Err(DaemonError::Config(
            "the worktree path must end in a non-empty directory name".into(),
        ));
    }
    Ok(name)
}

/// Snapshot of one registered worktree for the wire: name, path, checked-out
/// branch (or `(detached)`), and a short HEAD id.
fn worktree_info(worktree: &git2::Worktree) -> Result<WorktreeInfo> {
    let name = worktree.name()?.unwrap_or_default().to_string();
    let path = worktree.path().to_path_buf();
    let wt = Repository::open(&path)?;
    let head = wt.head().ok();
    let branch = match head.as_ref().and_then(|h| h.shorthand().ok()) {
        Some(short) => short.to_string(),
        None => "(detached)".to_string(),
    };
    let head_id = head
        .as_ref()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|commit| commit.id().to_string())
        .map(|oid| oid[..oid.len().min(7)].to_string())
        .unwrap_or_default();
    Ok(WorktreeInfo {
        name,
        path: path.to_string_lossy().into_owned(),
        branch,
        head: head_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Runs `git` in `dir`, asserting success so a fixture failure fails the
    /// test with git's stderr instead of a confusing later error.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Builds a bare remote + clone with `main` and `feature` pushed and the
    /// clone sitting on `main`. Returns the clone path.
    fn fixture() -> PathBuf {
        let base = std::env::temp_dir().join(format!("gs-worktrees-{}", uuid::Uuid::new_v4()));
        let clone = base.join("clone");
        std::fs::create_dir_all(&base).unwrap();
        git(&base, &["init", "--bare", "-b", "main", "remote.git"]);
        git(&base, &["clone", "remote.git", "clone"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "Test"]);
        std::fs::write(clone.join("file.txt"), "base\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "initial"]);
        git(&clone, &["push", "-u", "origin", "main"]);
        git(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("file.txt"), "feature\n").unwrap();
        git(&clone, &["commit", "-am", "feature"]);
        git(&clone, &["push", "-u", "origin", "feature"]);
        git(&clone, &["checkout", "main"]);
        clone
    }

    /// Cleanup helper: removes the fixture's base directory (clone's parent).
    fn cleanup(clone: &Path) {
        std::fs::remove_dir_all(clone.parent().unwrap()).ok();
    }

    #[test]
    fn list_reports_branches_and_no_worktrees() {
        let clone = fixture();
        let result = list(&clone).unwrap();
        assert_eq!(result.worktrees.len(), 0);
        assert!(result.branches.contains(&"main".to_string()));
        assert!(result.branches.contains(&"feature".to_string()));
        cleanup(&clone);
    }

    #[test]
    fn list_excludes_conflict_session_worktrees() {
        let clone = fixture();
        let repo = Repository::open(&clone).unwrap();
        let base = clone.parent().unwrap().join("base");
        std::fs::create_dir_all(&base).unwrap();
        // The conflict resolver's naming: any `gitsurveil-*` worktree is a
        // session, transient and pruned at startup — never surfaced to the UI.
        repo.worktree("gitsurveil-acme-api", &base.join("wt"), None).unwrap();
        let result = list(&clone).unwrap();
        assert_eq!(result.worktrees.len(), 0, "conflict sessions are filtered out");
        cleanup(&clone);
    }

    #[test]
    fn add_checks_out_an_existing_branch() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-acme-api-feature");
        let info = add(&clone, "feature", target.to_str().unwrap()).unwrap();
        assert_eq!(info.branch, "feature");
        // Canonicalize both sides: `/var` is a symlink to `/private/var` on
        // macOS, and on Windows `canonicalize` returns the `\\?\` verbatim
        // form the daemon's cleaner path won't literally match.
        let expected = std::fs::canonicalize(&target).unwrap();
        assert_eq!(std::fs::canonicalize(&info.path).unwrap(), expected);
        assert!(!info.head.is_empty());
        let repo = Repository::open(&clone).unwrap();
        assert!(
            repo.worktrees().unwrap().iter().any(|s| s.ok().flatten() == Some("wt-acme-api-feature")),
            "worktree must be registered in the clone"
        );
        cleanup(&clone);
    }

    #[test]
    fn add_creates_a_typed_new_branch_at_head() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-wip");
        let info = add(&clone, "wip-123", target.to_str().unwrap()).unwrap();
        assert_eq!(info.branch, "wip-123");
        let repo = Repository::open(&clone).unwrap();
        assert!(
            repo.find_branch("wip-123", BranchType::Local).is_ok(),
            "a typed-new branch must exist in the clone after the add"
        );
        cleanup(&clone);
    }

    #[test]
    fn add_from_a_remote_branch_creates_a_local_tracking_branch() {
        let clone = fixture();
        git(&clone, &["push", "origin", "main:server-only"]);
        git(&clone, &["fetch", "origin"]);
        let target = clone.parent().unwrap().join("wt-server");
        let info = add(&clone, "server-only", target.to_str().unwrap()).unwrap();
        assert_eq!(info.branch, "server-only");
        let repo = Repository::open(&clone).unwrap();
        assert!(
            repo.find_branch("server-only", BranchType::Local).is_ok(),
            "remote-only branch must become local in the worktree"
        );
        cleanup(&clone);
    }

    #[test]
    fn add_resolves_relative_paths_against_the_clone_parent() {
        let clone = fixture();
        let info = add(&clone, "feature", "wt-relative").unwrap();
        let expected = std::fs::canonicalize(clone.parent().unwrap().join("wt-relative")).unwrap();
        assert_eq!(std::fs::canonicalize(&info.path).unwrap(), expected);
        cleanup(&clone);
    }

    #[test]
    fn add_refuses_a_preexisting_non_empty_target_without_touching_it() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("occupied");
        std::fs::create_dir_all(&target).unwrap();
        let survivor = target.join("keep.txt");
        std::fs::write(&survivor, "do not delete\n").unwrap();
        let err = add(&clone, "feature", target.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("already exists and is not empty"));
        assert!(survivor.exists(), "preexisting content must survive a refused add");
        assert_eq!(std::fs::read_to_string(&survivor).unwrap(), "do not delete\n");
        cleanup(&clone);
    }

    #[test]
    fn add_refuses_a_branch_already_checked_out_elsewhere() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-again");
        let err = add(&clone, "main", target.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("already checked out in the main checkout"));
        assert!(!target.exists(), "no worktree may be created for a busy branch");
        cleanup(&clone);
    }

    #[test]
    fn remove_deletes_a_worktree_but_keeps_the_branch() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-acme-api-feature");
        add(&clone, "feature", target.to_str().unwrap()).unwrap();
        remove(&clone, "wt-acme-api-feature", false).unwrap();
        assert!(!target.exists(), "worktree directory must be removed");
        let repo = Repository::open(&clone).unwrap();
        assert!(
            repo.find_worktree("wt-acme-api-feature").is_err(),
            "worktree registration must be pruned"
        );
        assert!(
            repo.find_branch("feature", BranchType::Local).is_ok(),
            "the checked-out branch survives a worktree removal"
        );
        cleanup(&clone);
    }

    #[test]
    fn remove_refuses_a_dirty_worktree() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-acme-api-feature");
        add(&clone, "feature", target.to_str().unwrap()).unwrap();
        std::fs::write(target.join("untracked.txt"), "dirty\n").unwrap();
        let err = remove(&clone, "wt-acme-api-feature", false).unwrap_err();
        assert!(err.to_string().contains("uncommitted changes or untracked files"));
        assert!(target.exists(), "a refused removal must not delete the worktree");
        cleanup(&clone);
    }

    #[test]
    fn remove_force_skips_dirty_check() {
        let clone = fixture();
        let target = clone.parent().unwrap().join("wt-acme-api-feature");
        add(&clone, "feature", target.to_str().unwrap()).unwrap();
        std::fs::write(target.join("untracked.txt"), "dirty\n").unwrap();
        remove(&clone, "wt-acme-api-feature", true).unwrap();
        assert!(!target.exists(), "force removal must delete the dirty worktree");
        let repo = Repository::open(&clone).unwrap();
        assert!(
            repo.find_worktree("wt-acme-api-feature").is_err(),
            "worktree registration must be pruned"
        );
        assert!(
            repo.find_branch("feature", BranchType::Local).is_ok(),
            "the checked-out branch survives a force removal"
        );
        cleanup(&clone);
    }

    #[test]
    fn remove_refuses_unknown_names() {
        let clone = fixture();
        let err = remove(&clone, "nope", false).unwrap_err();
        assert!(err.to_string().contains("no worktree named `nope`"));
        cleanup(&clone);
    }

    #[test]
    fn remove_refuses_conflict_session_worktrees() {
        let clone = fixture();
        let err = remove(&clone, "gitsurveil-acme-api", false).unwrap_err();
        assert!(err.to_string().contains("conflict-session worktree"));
        cleanup(&clone);
    }
}

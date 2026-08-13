//! Pull-request reads and mutations (`specs/pr-management.md`).
//!
//! Every mutating call here is reached only through an explicit user action in
//! the UI — nothing in the poll loop calls into this module. That's the
//! "nothing is posted to GitHub without an explicit user action" rule from
//! `CLAUDE.md`, enforced structurally rather than by convention.
//!
//! Uses the same plain `reqwest` client as the rest of the GitHub layer so
//! request headers and error handling stay uniform.

use gitsurveil_proto::{
    Check, Comment, MergeMethod, Mergeability, PullRequestDetail, Reviewer,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::Result;
use crate::github::GitHubClient;

/// Fields we edit on an existing PR. `None` means "leave unchanged", so the
/// UI can send a partial update without having to echo back every field it
/// didn't touch.
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct PrPatch {
    /// New title.
    pub title: Option<String>,
    /// New body.
    pub body: Option<String>,
    /// New base branch.
    pub base: Option<String>,
    /// Draft/ready-for-review toggle.
    pub draft: Option<bool>,
    /// Replaces the label set entirely.
    pub labels: Option<Vec<String>>,
    /// Adds these reviewers; GitHub has no "replace reviewers" call.
    pub reviewers: Option<Vec<String>>,
}

impl GitHubClient {
    /// Fetches everything the PR detail pane renders.
    pub async fn pr_detail(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let pr: RawPr = self
            .get_json(&format!("/repos/{repo}/pulls/{number}"))
            .await?;

        // Reviews and checks are separate endpoints; fetched concurrently
        // since neither depends on the other and the detail pane needs both
        // before it can render.
        let reviews_path = format!("/repos/{repo}/pulls/{number}/reviews");
        let checks_path = format!("/repos/{repo}/commits/{}/check-runs", pr.head.sha);
        let (reviews, checks) = tokio::join!(
            self.get_json::<Vec<RawReview>>(&reviews_path),
            self.get_json::<RawCheckRuns>(&checks_path)
        );

        let mut reviewers: Vec<Reviewer> = reviews
            .unwrap_or_default()
            .into_iter()
            .map(|r| Reviewer {
                login: r.user.map(|u| u.login).unwrap_or_default(),
                state: r.state.to_lowercase(),
            })
            .collect();
        // Reviewers who haven't responded yet aren't in the reviews list at
        // all; without this they'd silently vanish from the pane.
        for requested in pr.requested_reviewers.iter().flatten() {
            if !reviewers.iter().any(|r| r.login == requested.login) {
                reviewers.push(Reviewer {
                    login: requested.login.clone(),
                    state: "pending".to_string(),
                });
            }
        }

        let checks = checks
            .map(|c| {
                c.check_runs
                    .into_iter()
                    .map(|run| Check {
                        name: run.name,
                        conclusion: run.conclusion.unwrap_or_else(|| run.status.clone()),
                        url: run.html_url,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(PullRequestDetail {
            repo: repo.to_string(),
            number,
            title: pr.title,
            body: pr.body.unwrap_or_default(),
            state: if pr.merged_at.is_some() {
                "merged".to_string()
            } else {
                pr.state
            },
            draft: pr.draft.unwrap_or(false),
            base: pr.base.ref_name,
            head: pr.head.ref_name,
            author: pr.user.map(|u| u.login).unwrap_or_default(),
            labels: pr.labels.into_iter().map(|l| l.name).collect(),
            reviewers,
            checks,
            mergeability: mergeability_of(&pr.mergeable, pr.mergeable_state.as_deref()),
            url: pr.html_url,
            head_sha: pr.head.sha,
        })
    }

    /// Creates a pull request and returns its detail.
    pub async fn pr_create(
        &self,
        repo: &str,
        base: &str,
        head: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<PullRequestDetail> {
        let created: RawPr = self
            .post_json(
                &format!("/repos/{repo}/pulls"),
                json!({
                    "title": title,
                    "body": body,
                    "base": base,
                    "head": head,
                    "draft": draft,
                }),
            )
            .await?;
        self.pr_detail(repo, created.number).await
    }

    /// Applies a partial update to an existing PR.
    ///
    /// Labels and reviewers live on different endpoints from the PR itself,
    /// so a full update is up to three calls; they're issued in sequence and
    /// the first failure aborts, leaving the rest unapplied rather than
    /// guessing at a rollback.
    pub async fn pr_update(
        &self,
        repo: &str,
        number: u64,
        patch: &PrPatch,
    ) -> Result<PullRequestDetail> {
        let mut body = serde_json::Map::new();
        if let Some(title) = &patch.title {
            body.insert("title".into(), json!(title));
        }
        if let Some(text) = &patch.body {
            body.insert("body".into(), json!(text));
        }
        if let Some(base) = &patch.base {
            body.insert("base".into(), json!(base));
        }
        if let Some(draft) = patch.draft {
            body.insert("draft".into(), json!(draft));
        }
        if !body.is_empty() {
            let _: RawPr = self
                .patch_json(
                    &format!("/repos/{repo}/pulls/{number}"),
                    serde_json::Value::Object(body),
                )
                .await?;
        }

        if let Some(labels) = &patch.labels {
            let _: serde_json::Value = self
                .put_json(
                    &format!("/repos/{repo}/issues/{number}/labels"),
                    json!({ "labels": labels }),
                )
                .await?;
        }

        if let Some(reviewers) = &patch.reviewers {
            if !reviewers.is_empty() {
                let _: serde_json::Value = self
                    .post_json(
                        &format!("/repos/{repo}/pulls/{number}/requested_reviewers"),
                        json!({ "reviewers": reviewers }),
                    )
                    .await?;
            }
        }

        self.pr_detail(repo, number).await
    }

    /// Closes a PR without merging, optionally leaving a comment first so the
    /// explanation lands before the close event in the timeline.
    pub async fn pr_close(&self, repo: &str, number: u64, comment: Option<&str>) -> Result<()> {
        if let Some(text) = comment.filter(|c| !c.trim().is_empty()) {
            self.pr_comment(repo, number, text).await?;
        }
        let _: RawPr = self
            .patch_json(
                &format!("/repos/{repo}/pulls/{number}"),
                json!({ "state": "closed" }),
            )
            .await?;
        Ok(())
    }

    /// Merges a PR.
    ///
    /// `head_sha` is sent as `sha`, so GitHub rejects the merge if the branch
    /// moved since the UI loaded it — a stale merge is exactly the mistake
    /// worth making impossible rather than merely unlikely.
    pub async fn pr_merge(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        head_sha: &str,
        commit_title: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("merge_method".into(), json!(method.as_api_str()));
        body.insert("sha".into(), json!(head_sha));
        if let Some(title) = commit_title {
            body.insert("commit_title".into(), json!(title));
        }
        let _: serde_json::Value = self
            .put_json(
                &format!("/repos/{repo}/pulls/{number}/merge"),
                serde_json::Value::Object(body),
            )
            .await?;
        Ok(())
    }

    /// Fetches the conversation: issue comments plus review comments, oldest
    /// first. Both are shown in one list because that's how the conversation
    /// actually reads.
    pub async fn pr_comments(&self, repo: &str, number: u64) -> Result<Vec<Comment>> {
        let issue_path = format!("/repos/{repo}/issues/{number}/comments");
        let review_path = format!("/repos/{repo}/pulls/{number}/comments");
        let (issue, review) = tokio::join!(
            self.get_json::<Vec<RawComment>>(&issue_path),
            self.get_json::<Vec<RawComment>>(&review_path)
        );

        let mut comments: Vec<Comment> = issue
            .unwrap_or_default()
            .into_iter()
            .chain(review.unwrap_or_default())
            .map(|c| Comment {
                id: c.id,
                author: c.user.map(|u| u.login).unwrap_or_default(),
                body: c.body,
                created_at: c.created_at,
                path: c.path,
            })
            .collect();
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(comments)
    }

    /// Posts a top-level comment on a PR.
    pub async fn pr_comment(&self, repo: &str, number: u64, body: &str) -> Result<Comment> {
        let created: RawComment = self
            .post_json(
                &format!("/repos/{repo}/issues/{number}/comments"),
                json!({ "body": body }),
            )
            .await?;
        Ok(Comment {
            id: created.id,
            author: created.user.map(|u| u.login).unwrap_or_default(),
            body: created.body,
            created_at: created.created_at,
            path: created.path,
        })
    }

    /// Branches in a repository, for the create-PR form's pickers.
    pub async fn list_branches(&self, repo: &str) -> Result<Vec<String>> {
        let branches: Vec<RawBranch> = self
            .get_json(&format!("/repos/{repo}/branches?per_page=100"))
            .await?;
        Ok(branches.into_iter().map(|b| b.name).collect())
    }
}

/// Maps GitHub's two mergeability signals onto one enum.
///
/// `mergeable` is null while GitHub computes it in the background, which is
/// why "unknown" is a first-class state rather than an error.
fn mergeability_of(mergeable: &Option<bool>, state: Option<&str>) -> Mergeability {
    match (mergeable, state) {
        (Some(false), _) => Mergeability::Conflicted,
        (Some(true), Some("blocked")) => Mergeability::Blocked,
        (Some(true), _) => Mergeability::Clean,
        (None, Some("dirty")) => Mergeability::Conflicted,
        (None, _) => Mergeability::Unknown,
    }
}

// ---- GitHub wire shapes -------------------------------------------------
// Only the fields used above; GitHub returns far more.

#[derive(Debug, Deserialize)]
struct RawPr {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    draft: Option<bool>,
    html_url: String,
    merged_at: Option<String>,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    user: Option<RawUser>,
    base: RawRef,
    head: RawRef,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    requested_reviewers: Option<Vec<RawUser>>,
}

#[derive(Debug, Deserialize)]
struct RawRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawReview {
    user: Option<RawUser>,
    state: String,
}

#[derive(Debug, Deserialize)]
struct RawCheckRuns {
    check_runs: Vec<RawCheckRun>,
}

#[derive(Debug, Deserialize)]
struct RawCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawComment {
    id: u64,
    user: Option<RawUser>,
    body: String,
    created_at: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawBranch {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicted_wins_over_every_other_signal() {
        assert_eq!(
            mergeability_of(&Some(false), Some("blocked")),
            Mergeability::Conflicted
        );
        assert_eq!(
            mergeability_of(&None, Some("dirty")),
            Mergeability::Conflicted
        );
    }

    #[test]
    fn blocked_is_distinguished_from_clean() {
        assert_eq!(
            mergeability_of(&Some(true), Some("blocked")),
            Mergeability::Blocked
        );
        assert_eq!(
            mergeability_of(&Some(true), Some("clean")),
            Mergeability::Clean
        );
    }

    #[test]
    fn null_mergeable_is_unknown_not_an_error() {
        // GitHub computes mergeability asynchronously, so a fresh PR reports
        // null for a moment. Treating that as "cannot merge" would flash a
        // false blocker every time the pane opens.
        assert_eq!(mergeability_of(&None, None), Mergeability::Unknown);
        assert_eq!(
            mergeability_of(&None, Some("unknown")),
            Mergeability::Unknown
        );
    }
}

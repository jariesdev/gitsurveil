//! The priority engine (`specs/priority-engine.md`).
//!
//! Everything here is a pure function over plain data: no I/O, no clock reads
//! (the caller passes `now`), no storage. That is deliberate — this module
//! decides when the user gets interrupted, which is the behavior most worth
//! being able to test exhaustively and the most annoying to get wrong.
//!
//! Scoring is: base score for the item's kind, plus every matching rule's
//! modifier, plus age escalation. The result maps to a
//! [`Severity`] band that drives the tray color and the notification gate.

use chrono::{DateTime, Utc};
use gitsurveil_proto::{ActionItem, ItemKind, ScoredItem, Severity};
use serde::{Deserialize, Serialize};

/// Base score per item kind, before rules and age
/// (`specs/priority-engine.md`, "Default base scores").
fn base_score(kind: ItemKind) -> i64 {
    match kind {
        // A broken build blocks you and everyone downstream of you.
        ItemKind::CiFailed => 100,
        // Someone else is blocked until you act.
        ItemKind::ReviewRequested => 80,
        ItemKind::ReviewStateChanged => 70,
        ItemKind::Mentioned => 50,
        ItemKind::Assigned => 40,
        ItemKind::Participating => 20,
    }
}

/// Age escalation: an item gains a point every four hours it stays open, up to
/// a cap. Keeps old review requests from being buried forever by fresher,
/// higher-base items without ever letting age alone dominate a real emergency.
const AGE_POINTS_PER_HOURS: i64 = 4;
const MAX_AGE_BONUS: i64 = 30;

/// What an item must look like for a [`Rule`] to apply. Every field that is
/// present must match (logical AND); absent fields don't constrain.
///
/// `label` and `draft` from the spec are intentionally absent: the poller
/// doesn't fetch either yet, so a rule keyed on them could never match and
/// would be a silently dead config option. They arrive with the data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleMatch {
    /// Match any of these kinds.
    #[serde(default)]
    pub kind: Option<Vec<ItemKind>>,
    /// Match a repository, optionally with a trailing `*` wildcard
    /// (`"acme/*"`, `"acme/api"`).
    #[serde(default)]
    pub repo: Option<String>,
    /// Match any of these author logins.
    #[serde(default)]
    pub author: Option<Vec<String>>,
}

/// What a matching [`Rule`] does to an item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleEffect {
    /// Added to the score; may be negative. The final score is clamped to at
    /// least 1, so a modifier can demote an item but never erase it.
    #[serde(default)]
    pub add: Option<i64>,
    /// Forces the severity band regardless of score. Used to park a noisy
    /// source at `info` without distorting its ordering.
    #[serde(default)]
    pub pin_severity: Option<Severity>,
    /// Silences desktop notifications for matching items. They still appear in
    /// lists — muting silences, it does not hide.
    #[serde(default)]
    pub mute_notifications: Option<bool>,
}

/// One user-configurable priority rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Stable identifier, used by the UI rule editor (Phase 5).
    pub id: String,
    /// Disabled rules are kept in config but skipped.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Conditions.
    #[serde(rename = "match", default)]
    pub match_: RuleMatch,
    /// Consequences.
    #[serde(default)]
    pub effect: RuleEffect,
}

fn default_true() -> bool {
    true
}

/// The rules shipped when the user has no config of their own.
///
/// Just one: `Participating` items are things you're subscribed to rather than
/// named on, and notifying about every one of them is the fastest way to make
/// a tool like this feel like spam. They stay visible in the list.
pub fn default_rules() -> Vec<Rule> {
    vec![Rule {
        id: "mute-participating".to_string(),
        enabled: true,
        match_: RuleMatch {
            kind: Some(vec![ItemKind::Participating]),
            ..RuleMatch::default()
        },
        effect: RuleEffect {
            mute_notifications: Some(true),
            ..RuleEffect::default()
        },
    }]
}

/// Matches a repo against a pattern that may end in `*`.
///
/// Deliberately not a glob crate: the only pattern users actually write for a
/// repository is `owner/*`, and a dependency for one `strip_suffix` would be
/// hard to justify.
fn repo_matches(pattern: &str, repo: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => repo.starts_with(prefix),
        None => pattern == repo,
    }
}

fn rule_applies(rule: &Rule, item: &ActionItem) -> bool {
    if !rule.enabled {
        return false;
    }
    if let Some(kinds) = &rule.match_.kind {
        if !kinds.contains(&item.kind) {
            return false;
        }
    }
    if let Some(pattern) = &rule.match_.repo {
        if !repo_matches(pattern, &item.repo) {
            return false;
        }
    }
    if let Some(authors) = &rule.match_.author {
        if !authors.iter().any(|a| a == &item.author) {
            return false;
        }
    }
    true
}

/// Age bonus for an item, derived from when it was created.
///
/// An unparseable timestamp yields no bonus rather than an error: a weird date
/// from an API should cost the item some priority, never take down the poll.
fn age_bonus(item: &ActionItem, now: DateTime<Utc>) -> i64 {
    let Ok(created) = DateTime::parse_from_rfc3339(&item.created_at) else {
        return 0;
    };
    let hours = now
        .signed_duration_since(created.with_timezone(&Utc))
        .num_hours();
    if hours <= 0 {
        return 0;
    }
    (hours / AGE_POINTS_PER_HOURS).min(MAX_AGE_BONUS)
}

/// Scores one item against `rules` as of `now`.
///
/// Pure: same inputs always give the same output, which is what makes the
/// notification behavior testable.
pub fn score_item(item: &ActionItem, rules: &[Rule], now: DateTime<Utc>) -> ScoredItem {
    let mut score = base_score(item.kind) + age_bonus(item, now);
    let mut pinned = None;
    let mut muted = false;

    for rule in rules.iter().filter(|r| rule_applies(r, item)) {
        if let Some(add) = rule.effect.add {
            score += add;
        }
        if let Some(severity) = rule.effect.pin_severity {
            // Last matching rule wins, so config order is the tie-breaker the
            // user can see and reorder.
            pinned = Some(severity);
        }
        if rule.effect.mute_notifications == Some(true) {
            muted = true;
        }
    }

    // Clamp: a rule may demote an item but never make it score zero, which is
    // reserved for "nothing open at all".
    let score = score.clamp(1, u32::MAX as i64) as u32;

    ScoredItem {
        item: item.clone(),
        score,
        severity: pinned.unwrap_or_else(|| Severity::from_score(score)),
        muted,
    }
}

/// Scores every item and returns them most-urgent first.
///
/// Ties break on `updated_at`, newest first, so the ordering is stable across
/// polls instead of flickering between equally-scored items.
pub fn score_all(items: &[ActionItem], rules: &[Rule], now: DateTime<Utc>) -> Vec<ScoredItem> {
    let mut scored: Vec<ScoredItem> = items.iter().map(|i| score_item(i, rules, now)).collect();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.item.updated_at.cmp(&a.item.updated_at))
    });
    scored
}

/// The severity the tray icon should show: that of the highest-scoring open
/// item, or [`Severity::Idle`] when nothing is open.
///
/// Muted items still count. Muting suppresses the *interruption*, not the
/// ambient signal — the whole point of the tray color is that it can tell you
/// something without demanding attention.
pub fn top_severity(scored: &[ScoredItem]) -> Severity {
    scored
        .iter()
        .map(|s| s.severity)
        .max()
        .unwrap_or(Severity::Idle)
}

/// Decides whether a new or newly-urgent item earns a desktop notification.
///
/// The rule that gives the product its character: you are interrupted only by
/// something that outranks whatever was already at the top of your list.
/// Everything else lands silently and shows up in the tray color instead.
///
/// `prev_top_score` is the highest score *before* this poll's changes; `None`
/// means nothing was open.
pub fn should_notify(prev_top_score: Option<u32>, candidate: &ScoredItem) -> bool {
    if candidate.muted {
        return false;
    }
    // A broken build always interrupts, even mid-flow on something bigger:
    // it is usually blocking other people too.
    if candidate.severity == Severity::Critical {
        return true;
    }
    match prev_top_score {
        None => true,
        Some(top) => candidate.score > top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitsurveil_proto::{CiStatus, ItemState};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-13T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn item(kind: ItemKind) -> ActionItem {
        ActionItem {
            id: "a".into(),
            account_id: "acc".into(),
            kind,
            state: ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "u".into(),
            author: "someone".into(),
            // Same instant as `now()`, so age contributes nothing unless a
            // test deliberately backdates it.
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            first_seen_at: "2026-08-13T12:00:00Z".into(),
            last_seen_at: "2026-08-13T12:00:00Z".into(),
            ci_status: CiStatus::None,
            raw_kind: "x".into(),
        }
    }

    // ---- base scores and severity bands ------------------------------

    #[test]
    fn base_scores_map_to_expected_severities() {
        let cases = [
            (ItemKind::CiFailed, 100, Severity::Critical),
            (ItemKind::ReviewRequested, 80, Severity::High),
            (ItemKind::ReviewStateChanged, 70, Severity::High),
            (ItemKind::Mentioned, 50, Severity::Normal),
            (ItemKind::Assigned, 40, Severity::Normal),
            (ItemKind::Participating, 20, Severity::Info),
        ];
        for (kind, expected_score, expected_severity) in cases {
            let scored = score_item(&item(kind), &[], now());
            assert_eq!(scored.score, expected_score, "score for {kind:?}");
            assert_eq!(scored.severity, expected_severity, "severity for {kind:?}");
        }
    }

    #[test]
    fn severity_band_boundaries_are_exact() {
        assert_eq!(Severity::from_score(0), Severity::Idle);
        assert_eq!(Severity::from_score(1), Severity::Info);
        assert_eq!(Severity::from_score(29), Severity::Info);
        assert_eq!(Severity::from_score(30), Severity::Normal);
        assert_eq!(Severity::from_score(59), Severity::Normal);
        assert_eq!(Severity::from_score(60), Severity::High);
        assert_eq!(Severity::from_score(99), Severity::High);
        assert_eq!(Severity::from_score(100), Severity::Critical);
    }

    // ---- age escalation ------------------------------------------------

    #[test]
    fn age_adds_one_point_per_four_hours_up_to_the_cap() {
        let mut old = item(ItemKind::Assigned);
        old.created_at = "2026-08-13T04:00:00Z".into(); // 8h -> +2
        assert_eq!(score_item(&old, &[], now()).score, 42);

        let mut ancient = item(ItemKind::Assigned);
        ancient.created_at = "2020-01-01T00:00:00Z".into(); // capped at +30
        assert_eq!(score_item(&ancient, &[], now()).score, 70);
    }

    #[test]
    fn future_or_unparseable_timestamps_score_without_age_bonus() {
        let mut future = item(ItemKind::Assigned);
        future.created_at = "2030-01-01T00:00:00Z".into();
        assert_eq!(score_item(&future, &[], now()).score, 40);

        let mut broken = item(ItemKind::Assigned);
        broken.created_at = "not a date".into();
        assert_eq!(score_item(&broken, &[], now()).score, 40);
    }

    // ---- rules -----------------------------------------------------------

    fn rule(effect: RuleEffect, match_: RuleMatch) -> Rule {
        Rule {
            id: "r".into(),
            enabled: true,
            match_,
            effect,
        }
    }

    #[test]
    fn matching_rule_adjusts_score_and_non_matching_does_not() {
        let boost = rule(
            RuleEffect {
                add: Some(25),
                ..Default::default()
            },
            RuleMatch {
                repo: Some("acme/api".into()),
                ..Default::default()
            },
        );
        assert_eq!(score_item(&item(ItemKind::Assigned), &[boost], now()).score, 65);

        let other_repo = rule(
            RuleEffect {
                add: Some(25),
                ..Default::default()
            },
            RuleMatch {
                repo: Some("other/repo".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            score_item(&item(ItemKind::Assigned), &[other_repo], now()).score,
            40
        );
    }

    #[test]
    fn repo_wildcard_matches_by_prefix_only() {
        assert!(repo_matches("acme/*", "acme/api"));
        assert!(repo_matches("acme/*", "acme/web"));
        assert!(!repo_matches("acme/*", "other/api"));
        assert!(repo_matches("acme/api", "acme/api"));
        assert!(!repo_matches("acme/api", "acme/apiary"));
    }

    #[test]
    fn disabled_rules_are_ignored() {
        let mut disabled = rule(
            RuleEffect {
                add: Some(50),
                ..Default::default()
            },
            RuleMatch::default(),
        );
        disabled.enabled = false;
        assert_eq!(
            score_item(&item(ItemKind::Assigned), &[disabled], now()).score,
            40
        );
    }

    #[test]
    fn negative_modifier_cannot_push_score_below_one() {
        let crush = rule(
            RuleEffect {
                add: Some(-1000),
                ..Default::default()
            },
            RuleMatch::default(),
        );
        let scored = score_item(&item(ItemKind::Assigned), &[crush], now());
        assert_eq!(scored.score, 1);
        assert_eq!(scored.severity, Severity::Info);
    }

    #[test]
    fn pinned_severity_overrides_the_score_band() {
        let pin = rule(
            RuleEffect {
                pin_severity: Some(Severity::Info),
                ..Default::default()
            },
            RuleMatch::default(),
        );
        let scored = score_item(&item(ItemKind::CiFailed), &pin_slice(&pin), now());
        assert_eq!(scored.score, 100, "score is unchanged");
        assert_eq!(scored.severity, Severity::Info, "band is overridden");
    }

    fn pin_slice(r: &Rule) -> Vec<Rule> {
        vec![r.clone()]
    }

    #[test]
    fn author_match_is_any_of() {
        let by_author = rule(
            RuleEffect {
                add: Some(10),
                ..Default::default()
            },
            RuleMatch {
                author: Some(vec!["nobody".into(), "someone".into()]),
                ..Default::default()
            },
        );
        assert_eq!(
            score_item(&item(ItemKind::Assigned), &[by_author], now()).score,
            50
        );
    }

    #[test]
    fn default_rules_mute_participating_but_leave_it_visible() {
        let scored = score_item(&item(ItemKind::Participating), &default_rules(), now());
        assert!(scored.muted);
        assert_eq!(scored.score, 20, "still scored, so it still lists");
    }

    // ---- ordering ---------------------------------------------------------

    #[test]
    fn score_all_orders_by_score_then_recency() {
        let mut low = item(ItemKind::Participating);
        low.id = "low".into();
        let mut high = item(ItemKind::CiFailed);
        high.id = "high".into();
        let mut mid_old = item(ItemKind::Assigned);
        mid_old.id = "mid-old".into();
        mid_old.updated_at = "2026-08-13T01:00:00Z".into();
        let mut mid_new = item(ItemKind::Assigned);
        mid_new.id = "mid-new".into();
        mid_new.updated_at = "2026-08-13T11:00:00Z".into();

        let scored = score_all(&[low, mid_old, high, mid_new], &[], now());
        let order: Vec<&str> = scored.iter().map(|s| s.item.id.as_str()).collect();
        assert_eq!(order, vec!["high", "mid-new", "mid-old", "low"]);
    }

    #[test]
    fn top_severity_is_idle_when_nothing_is_open() {
        assert_eq!(top_severity(&[]), Severity::Idle);
    }

    #[test]
    fn top_severity_counts_muted_items() {
        // Muting silences notifications; it must not blind the tray, or a
        // muted-but-critical item would leave the user with no signal at all.
        let muted_critical = rule(
            RuleEffect {
                mute_notifications: Some(true),
                ..Default::default()
            },
            RuleMatch::default(),
        );
        let scored = score_all(&[item(ItemKind::CiFailed)], &[muted_critical], now());
        assert!(scored[0].muted);
        assert_eq!(top_severity(&scored), Severity::Critical);
    }

    // ---- the notification gate -------------------------------------------

    fn scored(kind: ItemKind, muted: bool) -> ScoredItem {
        let mut s = score_item(&item(kind), &[], now());
        s.muted = muted;
        s
    }

    #[test]
    fn gate_notifies_when_nothing_was_open() {
        assert!(should_notify(None, &scored(ItemKind::Participating, false)));
    }

    #[test]
    fn gate_notifies_only_when_the_item_outranks_the_current_top() {
        let review = scored(ItemKind::ReviewRequested, false); // 80
        assert!(should_notify(Some(40), &review), "80 outranks 40");
        assert!(!should_notify(Some(80), &review), "equal does not outrank");
        assert!(!should_notify(Some(90), &review), "lower does not outrank");
    }

    #[test]
    fn gate_always_notifies_for_critical_even_below_the_current_top() {
        // The one exception to outranking: a broken build interrupts whatever
        // you're doing, because it usually blocks other people too.
        let ci = scored(ItemKind::CiFailed, false);
        assert!(should_notify(Some(u32::MAX), &ci));
    }

    #[test]
    fn gate_never_notifies_for_muted_items_even_when_critical() {
        // Mute is the user's explicit "stop telling me about this", so it wins
        // over the critical override.
        assert!(!should_notify(None, &scored(ItemKind::CiFailed, true)));
    }
}

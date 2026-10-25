// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAgentGoal` — the read that reports the optimization
//! goal a `(profileArn, id)` pair resolves to
//! (`GET /api/v1/agent-profiles/{profileArn}/goals/{goalId}`,
//! `smithy_query(query="operation", operation="GetAgentGoal")`).
//!
//! The round trip of every row of the shared goal table — a create followed by the
//! read that reports it back, member for member — is pinned by `super::agent_goals`,
//! together with the error envelopes of an unresolvable or malformed identifier. What
//! this suite adds is the rules that only show up with SEVERAL goals in the store, each
//! over a table of rows rather than one request:
//!
//! 1. CROSS-RECORD ISOLATION. A read is answered from the goal its `(profileArn, id)`
//!    pair identifies and from NO other: with goals stored that differ along every
//!    member a goal carries — pillar set, title size, whether the optional `description`
//!    is present and how big it is — no read reports a neighbour's `id`, `title`,
//!    `pillars` or `description`, and `description` is reported exactly for the rows
//!    that sent one.
//! 2. PURE READ. The read reports; it does not write. Repeated reads of every stored
//!    goal are identical to the first, a read that MISSES (an absent goal id, an absent
//!    profile) records nothing, the stored goal count stays exactly what the creates put
//!    there, and a goal nothing has modified never grows the `lastModified…` members.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the goal and
//! agent-profile request builders and the shared table of goal rows live in
//! `super::agent_goals`.

use serde_json::Value;

use super::agent_goals::{
    absent_profile_arn, assert_create_instant, create_goal, get_goal, goal_rows, profile_arn_of,
};
use super::workload_harness::{Answer, Client};

/// A well-formed `UUID` (the modelled
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`) that no goal
/// carries, so a read of it is a genuine miss rather than a refusal at the boundary.
fn absent_goal_id(nibble: char) -> String {
    let group = |len: usize| std::iter::repeat_n(nibble, len).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        group(8),
        group(4),
        group(4),
        group(4),
        group(12)
    )
}

/// The `goal` member of a successful answer.
fn goal_of(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("goal")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a goal member in {answer:?}"))
}

/// Assert that `projection` is the `GoalSummary` of the goal `sent` created under
/// `profile_arn`: every member the request carried comes back as sent, the members the
/// SERVICE owns are its own, and the members nothing has set yet are absent.
fn assert_goal_projection(projection: &Value, sent: &Value, profile_arn: &str, case: &str) {
    for member_name in ["pillars", "title", "description"] {
        assert_eq!(
            projection.get(member_name),
            sent.get(member_name),
            "case {case:?}: expected {member_name} as sent, got {projection:?}"
        );
    }
    assert_eq!(
        projection.get("profileArn").and_then(Value::as_str),
        Some(profile_arn),
        "case {case:?}: expected the goal to report the profile it hangs off, \
         got {projection:?}"
    );
    assert_eq!(
        projection.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}: createdBy must be the account the request is attributed to, \
         got {projection:?}"
    );
    assert_create_instant(projection.get("createdAt"), case, "createdAt");

    // Nothing has modified a goal that was only created, so the optional
    // `lastModified…` members of `GoalSummary` stay absent rather than echoing the
    // create.
    for modified in ["lastModifiedBy", "lastModifiedAt"] {
        assert!(
            projection.get(modified).is_none_or(Value::is_null),
            "case {case:?}: {modified} must be absent on an unmodified goal, \
             got {projection:?}"
        );
    }
}

/// The goals of one profile, as `(case, id, summary)` triples, created over HTTP. The
/// ids are asserted DISTINCT, which is what makes the isolation checks below meaningful:
/// rows that shared an id could not be told apart by a read.
async fn seeded_goals(client: &Client, profile_arn: &str) -> Vec<(&'static str, String, Value)> {
    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for (case, body) in goal_rows() {
        let answer = create_goal(client, profile_arn, body.clone()).await;
        let summary = goal_of(&answer, case);
        assert_goal_projection(&summary, &body, profile_arn, case);
        let id = summary
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a goal id in {summary:?}"))
            .to_string();
        assert!(
            created.iter().all(|(_, other, _)| other != &id),
            "case {case:?}: every goal of a profile gets its own id, got a repeat of {id:?}"
        );
        created.push((case, id, summary));
    }
    created
}

/// How many goals the profile holds, read straight off the state the handlers share.
/// The behaviour under test still crosses the HTTP boundary; this is only the witness
/// that a read recorded nothing.
fn stored_goal_count(client: &Client, profile_arn: &str) -> usize {
    client.store().lock().agent_goal_count(profile_arn)
}

/// Rule 1: with every row of the shared table stored under ONE profile, no read reports a
/// neighbour's members. The rows differ along every member a goal carries, so a read
/// served from the wrong record shows up as a member of another row.
#[tokio::test]
async fn no_read_reports_a_neighbouring_goals_members() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-read-host", 1).await;
    let created = seeded_goals(&client, &profile_arn).await;

    for (case, id, summary) in &created {
        let read = goal_of(&get_goal(&client, &profile_arn, id).await, case);
        assert_eq!(
            read.get("id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: the read must report the id it was asked for, got {read:?}"
        );
        for (other_case, other_id, other) in &created {
            if other_id == id {
                continue;
            }
            // `title` alone would catch most mix-ups, but a record is distinguished by
            // every member the rows vary along, so each is checked against the neighbour.
            for member_name in ["id", "title", "pillars", "description"] {
                if summary.get(member_name) == other.get(member_name) {
                    continue;
                }
                assert_ne!(
                    read.get(member_name),
                    other.get(member_name),
                    "case {case:?}: must not report the {member_name} of {other_case:?}, \
                     got {read:?}"
                );
            }
        }
    }
}

/// Rule 1, for the OPTIONAL member: `description` is reported exactly for the rows whose
/// create carried one, and is absent — not empty, not defaulted — for the rows that sent
/// none.
#[tokio::test]
async fn the_optional_description_is_reported_only_where_the_create_carried_one() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-description-host", 1).await;

    for (case, body) in goal_rows() {
        let created = goal_of(&create_goal(&client, &profile_arn, body.clone()).await, case);
        let id = created
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a goal id in {created:?}"))
            .to_string();
        let read = goal_of(&get_goal(&client, &profile_arn, &id).await, case);

        match body.get("description").and_then(Value::as_str) {
            Some(sent) => assert_eq!(
                read.get("description").and_then(Value::as_str),
                Some(sent),
                "case {case:?}: expected the description as sent, got {read:?}"
            ),
            None => assert!(
                read.get("description").is_none_or(Value::is_null),
                "case {case:?}: a goal created without a description reports none, \
                 got {read:?}"
            ),
        }
    }
}

/// Rule 2: the read is REPORTING. Reading every stored goal several times reports the
/// identical summary each time, and the stored goal set is exactly what the creates put
/// there — a read neither records a goal nor rewrites one.
#[tokio::test]
async fn repeated_reads_do_not_change_what_a_later_caller_sees() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-reread-host", 1).await;
    let created = seeded_goals(&client, &profile_arn).await;
    let expected_count = created.len();

    // Three passes, interleaving the rows, so a read of one goal cannot be what makes
    // the next read of another correct.
    for pass in 1..=3u32 {
        for (case, id, summary) in &created {
            let read = goal_of(&get_goal(&client, &profile_arn, id).await, case);
            assert_eq!(
                &read, summary,
                "case {case:?}/pass {pass}: a repeated read must report the same \
                 summary, got {read:?}"
            );
        }
        let held = stored_goal_count(&client, &profile_arn);
        assert_eq!(
            held, expected_count,
            "pass {pass}: reads must not change how many goals the profile holds, \
             got {held}"
        );
    }
}

/// Rule 2, for the read that MISSES: a 404 — for a goal id no goal of the profile
/// carries, for a goal of ANOTHER profile, and for a `profileArn` that names no stored
/// profile — records nothing. Afterwards every stored goal still reports its original
/// summary, the profile holds exactly the goals it held, and the absent profile holds
/// none.
#[tokio::test]
async fn a_read_that_misses_records_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-miss-host", 1).await;
    let neighbour = profile_arn_of(&client, "goal-miss-neighbour", 0).await;
    let created = seeded_goals(&client, &profile_arn).await;
    let expected_count = created.len();

    let missing_profile = absent_profile_arn("no-such-agent-profile");
    // The misses differ along WHICH half of the pair is unresolvable: an absent goal id
    // under a stored profile, a stored goal id under another stored profile, and a
    // stored goal id under a profile that does not exist.
    let mut misses: Vec<(String, String, String)> = vec![
        (
            "absent-goal-id-all-zero".to_string(),
            profile_arn.clone(),
            absent_goal_id('0'),
        ),
        (
            "absent-goal-id-all-f".to_string(),
            profile_arn.clone(),
            absent_goal_id('f'),
        ),
    ];
    for (case, id, _) in &created {
        misses.push((
            format!("{case}-under-the-neighbour-profile"),
            neighbour.clone(),
            id.clone(),
        ));
        misses.push((
            format!("{case}-under-an-absent-profile"),
            missing_profile.clone(),
            id.clone(),
        ));
    }

    for (case, arn, id) in &misses {
        let answer = get_goal(&client, arn, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    for (case, id, summary) in &created {
        let read = goal_of(&get_goal(&client, &profile_arn, id).await, case);
        assert_eq!(
            &read, summary,
            "case {case:?}: a missed read must leave the stored goal untouched, \
             got {read:?}"
        );
    }
    for (label, arn, expected) in [
        ("the goal host", profile_arn.as_str(), expected_count),
        ("the neighbour profile", neighbour.as_str(), 0),
        ("an absent profile", missing_profile.as_str(), 0),
    ] {
        let count = stored_goal_count(&client, arn);
        assert_eq!(
            count, expected,
            "{label}: a missed read must record no goal, expected {expected}, got {count}"
        );
    }
}

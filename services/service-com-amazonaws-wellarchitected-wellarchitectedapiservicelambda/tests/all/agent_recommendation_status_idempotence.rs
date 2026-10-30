// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the retry `UpdateAgentRecommendationStatus` is modelled
//! `@idempotent` to make safe.
//!
//! The rule pinned here, over TABLES of inputs rather than one request: this write carries
//! no client token, so the handler itself is the only thing that can make a retry
//! indistinguishable from the request it retries — a request that asks for the status and
//! the reason the recommendation ALREADY holds records nothing, the instant the
//! recommendation reports as last modified included, while a request that differs in a
//! member the caller sent is a new update and is attributed to its own instant and to the
//! caller.
//!
//! The suites span the published `RecommendationStatus` catalog crossed with the forms the
//! optional reason takes (none at all, the modelled minimum, a middling value, the modelled
//! maximum), the state a recommendation is PRODUCED in, and each single member an update can
//! differ from the one it follows in.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. `UpdateAgentRecommendationStatusResponse` carries no members, so what was
//! recorded is observed through `GetAgentRecommendation`; only the SEED that ages a recorded
//! instant reaches state directly, through the accessor the handlers themselves resolve a
//! `RecommendationArn` by.
//!
//! The HTTP client lives in `super::workload_harness`; the profile and the generation whose
//! completion produces the recommendations come from the suites beside this one.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::agent_goals::{profile_arn_of, PILLARS};
use super::agent_recommendations::{
    recommendations_of, start_generation, text, until_complete, update_status,
    RECOMMENDATION_TYPES,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `RecommendationStatus` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationStatus")`), paired with the
/// `RecommendationState` the OPEN/CLOSED reading of that lifecycle puts it in. Spelled out
/// here so the suite pins the catalog rather than reading it back out of the code under
/// test.
const STATUS_STATES: [(&str, &str); 3] = [
    ("ACTIVE", "OPEN"),
    ("SUPPRESSED", "CLOSED"),
    ("COMPLETED", "CLOSED"),
];

/// The modelled bounds on the reason an update may carry
/// (`smithy_query(query="operation", operation="UpdateAgentRecommendationStatus")`:
/// `updateReason` `@length(1, 4096)`).
const REASON_BOUNDS: (usize, usize) = (1, 4096);

/// How far into the past a recorded last-modification instant is moved before the request
/// under test, so a write the handler makes to it is observable whatever the clock's
/// granularity between two in-process requests — and so a write it must NOT make cannot
/// hide behind that granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// `GET /api/v1/agent-recommendations/{recommendationArn}`: the ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK sends
/// it.
async fn get_recommendation(client: &Client, recommendation_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-recommendations/{}",
                encode_path_segment(recommendation_arn)
            ),
            None,
        )
        .await
}

/// What a read of the recommendation reports.
async fn reported(client: &Client, recommendation_arn: &str, case: &str) -> Value {
    let answer = get_recommendation(client, recommendation_arn).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetAgentRecommendation 200, got {answer:?}"
    );
    answer.body
}

/// The instant the stored recommendation reports as its last modification, read through the
/// accessor the handlers themselves resolve a `RecommendationArn` by.
fn stored_last_modified(
    client: &Client,
    recommendation_arn: &str,
    case: &str,
) -> Option<SystemTime> {
    let state = client.store();
    let guard = state.lock();
    guard
        .agent_recommendation(recommendation_arn)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the recommendation under test must still be stored")
        })
        .last_modified_at
}

/// Move a recorded last-modification instant back by `age`, so a write the handler makes to
/// it — or does not make — is observable between two in-process requests. A recommendation
/// no update has changed carries none, and is left carrying none.
fn age_last_modified(client: &Client, recommendation_arn: &str, age: Duration, case: &str) {
    let state = client.store();
    let mut guard = state.lock();
    let record = guard
        .agent_recommendation_mut(recommendation_arn)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the recommendation under test must still be stored")
        });
    if let Some(recorded) = record.last_modified_at {
        record.last_modified_at = Some(recorded - age);
    }
}

/// A profile whose one generation over every published pillar has completed, answered as the
/// `RecommendationArn` of each recommendation it produced, in the listing's order.
async fn generated(client: &Client, name: &str) -> Vec<String> {
    let profile_arn = profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, &PILLARS, RECOMMENDATION_TYPES[0]).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;

    let listed = recommendations_of(client, &profile_arn, name).await;
    assert_eq!(
        listed.len(),
        PILLARS.len(),
        "case {name:?}: a generation scoping every pillar produces one recommendation per \
         pillar, got {listed:?}"
    );
    listed
        .iter()
        .map(|item| text(item, "recommendationArn"))
        .collect()
}

/// One row of a table: the status an update asks for, the state derived from it, and the
/// reason the update carries.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    status: &'static str,
    state: &'static str,
    reason: Option<String>,
}

/// One row, with its derived `RecommendationState` resolved from the published catalog
/// rather than written down beside the status.
fn row(case: &str, status: &'static str, reason: Option<&str>) -> Row {
    let state = STATUS_STATES
        .iter()
        .find(|(published, _)| *published == status)
        .map(|(_, state)| *state)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {status:?} is not a published RecommendationStatus")
        });
    Row {
        case: case.to_string(),
        status,
        state,
        reason: reason.map(str::to_string),
    }
}

/// The rows: EVERY published `RecommendationStatus` crossed with the forms the optional
/// reason takes — none at all, exactly the modelled minimum, a middling value and exactly
/// the modelled maximum — so the table spans both the status catalog and the size of the
/// reason.
fn rows() -> Vec<Row> {
    let reasons: [Option<String>; 4] = [
        None,
        Some("!".repeat(REASON_BOUNDS.0)),
        Some("picked up by the platform team".to_string()),
        Some("r".repeat(REASON_BOUNDS.1)),
    ];
    let mut rows = Vec::new();
    for (status, _) in STATUS_STATES {
        for reason in &reasons {
            let size = reason.as_ref().map_or(0, |value| value.chars().count());
            rows.push(row(
                &format!("{status}/reason-{size}"),
                status,
                reason.as_deref(),
            ));
        }
    }
    rows
}

/// The request body one row sends: an optional member the row does not carry is ABSENT from
/// the body rather than sent as null, exactly as an SDK omits it.
fn status_body(row: &Row) -> Value {
    let mut body = json!({ "status": row.status });
    if let Some(reason) = &row.reason {
        body["updateReason"] = json!(reason);
    }
    body
}

/// An update `row` is certain to differ from: a published status that is not the row's, with
/// a reason of its own. Sent before the row under test so the row's own update writes
/// whatever state the recommendation was left in.
fn seeding_row(under_test: &Row) -> Row {
    let status = STATUS_STATES
        .iter()
        .map(|(published, _)| *published)
        .find(|published| *published != under_test.status)
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the status catalog must publish more than one value",
                under_test.case
            )
        });
    row(
        &format!("{} (seed)", under_test.case),
        status,
        Some("seeded before the row under test"),
    )
}

/// Assert a served update: the empty 200 the model declares.
fn assert_served(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateAgentRecommendationStatus 200, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(|members| members.is_empty()),
        "case {case:?}: the response is modelled with no members, got {answer:?}"
    );
}

/// Assert a read of the recommendation reports the status, derived state and reason `row`
/// asked for.
fn assert_recorded(reported: &Value, row: &Row) {
    let case = &row.case;
    assert_eq!(
        (text(reported, "status"), text(reported, "state")),
        (row.status.to_string(), row.state.to_string()),
        "case {case:?}: expected the status the update asked for and the state derived from \
         it, got {reported:?}"
    );
    match &row.reason {
        Some(reason) => assert_eq!(
            text(reported, "updateReason"),
            *reason,
            "case {case:?}: updateReason is the reason of the most recent update, got \
             {reported:?}"
        ),
        None => assert!(
            reported.get("updateReason").is_none_or(Value::is_null),
            "case {case:?}: an update carrying no reason leaves none behind, got {reported:?}"
        ),
    }
}

/// The rule: a repeat of the update a recommendation already holds records nothing — every
/// member a read of it carries, the instant it was last modified at included, is the one the
/// first request left.
#[tokio::test]
async fn repeating_an_identical_status_update_leaves_the_recommendation_as_it_stood() {
    let client = Client::new();
    let arns = generated(&client, "status-idempotence").await;

    for (index, row) in rows().iter().enumerate() {
        let arn = &arns[index % arns.len()];
        let case = &row.case;

        // Seeded with a state the row's own update DIFFERS from, so the row's update is a
        // write on every row — including the one asking for the state a recommendation is
        // produced in — and the repeats below have a recorded instant to leave alone.
        let seed = seeding_row(row);
        let answer = update_status(&client, arn, status_body(&seed)).await;
        assert_served(&answer, &seed.case);

        let answer = update_status(&client, arn, status_body(row)).await;
        assert_served(&answer, case);
        assert_recorded(&reported(&client, arn, case).await, row);

        // Aged, so a re-stamp the handler must not make could not hide behind the clock's
        // granularity between two in-process requests.
        age_last_modified(&client, arn, AGE, case);
        let before = reported(&client, arn, case).await;
        let before_instant = stored_last_modified(&client, arn, case);

        for repeat in 0..2 {
            let again = update_status(&client, arn, status_body(row)).await;
            assert_served(&again, &format!("{case}/repeat-{repeat}"));

            let after = reported(&client, arn, case).await;
            assert_eq!(
                after, before,
                "case {case:?}: repeat {repeat} of the same update must leave every member of \
                 the recommendation as it stood {before:?}, got {after:?}"
            );
            let instant = stored_last_modified(&client, arn, case);
            assert_eq!(
                instant, before_instant,
                "case {case:?}: repeat {repeat} must leave the instant the recommendation was \
                 last modified at {before_instant:?}, got {instant:?}"
            );
        }
    }
}

/// The other side of the rule: an update that differs in a member the caller sent is a new
/// update, so it records what it asks for and is attributed to its own instant and to the
/// caller.
#[tokio::test]
async fn an_update_that_differs_in_a_member_the_caller_sent_is_a_new_update() {
    let client = Client::new();
    let arns = generated(&client, "status-new-update").await;

    // Each pair is an update that stands, then one differing from it in exactly ONE member
    // the caller sent: the status, the reason's text, the reason's size, a reason where none
    // stood, and no reason where one stood.
    let pairs: Vec<(Row, Row)> = vec![
        (
            row(
                "status-differs",
                STATUS_STATES[2].0,
                Some("implemented in the spring change window"),
            ),
            row(
                "status-differs (variant)",
                STATUS_STATES[1].0,
                Some("implemented in the spring change window"),
            ),
        ),
        (
            row(
                "reason-text-differs",
                STATUS_STATES[1].0,
                Some("accepted risk for this quarter"),
            ),
            row(
                "reason-text-differs (variant)",
                STATUS_STATES[1].0,
                Some("accepted risk for next quarter"),
            ),
        ),
        (
            row(
                "reason-size-differs",
                STATUS_STATES[0].0,
                Some(&"!".repeat(REASON_BOUNDS.0)),
            ),
            row(
                "reason-size-differs (variant)",
                STATUS_STATES[0].0,
                Some(&"!".repeat(REASON_BOUNDS.0 + 1)),
            ),
        ),
        (
            row("reason-added", STATUS_STATES[0].0, None),
            row(
                "reason-added (variant)",
                STATUS_STATES[0].0,
                Some("picked up by the platform team"),
            ),
        ),
        (
            row("reason-dropped", STATUS_STATES[2].0, Some("done")),
            row("reason-dropped (variant)", STATUS_STATES[2].0, None),
        ),
    ];
    assert!(
        pairs.len() <= arns.len(),
        "case \"setup\": every pair needs a recommendation of its own, got {} for {}",
        pairs.len(),
        arns.len()
    );

    for ((seed, variant), arn) in pairs.iter().zip(&arns) {
        let answer = update_status(&client, arn, status_body(seed)).await;
        assert_served(&answer, &seed.case);
        assert_recorded(&reported(&client, arn, &seed.case).await, seed);

        age_last_modified(&client, arn, AGE, &seed.case);
        let before = reported(&client, arn, &seed.case).await;
        let before_instant = stored_last_modified(&client, arn, &seed.case);

        let case = &variant.case;
        let answer = update_status(&client, arn, status_body(variant)).await;
        assert_served(&answer, case);

        let after = reported(&client, arn, case).await;
        assert_recorded(&after, variant);
        let instant = stored_last_modified(&client, arn, case);
        assert!(
            instant.is_some_and(|at| before_instant.is_none_or(|stood| at > stood)),
            "case {case:?}: an update differing in a member the caller sent is attributed to \
             its own instant, past {before_instant:?}, got {instant:?}"
        );
        assert_eq!(
            text(&after, "lastModifiedBy"),
            engine::emulator_account_id(),
            "case {case:?}: the update is attributed to this emulator's account, got {after:?}"
        );
        assert_ne!(
            after.get("lastModifiedAt"),
            before.get("lastModifiedAt"),
            "case {case:?}: a read must report the new update's instant rather than {:?}, got \
             {after:?}",
            before.get("lastModifiedAt")
        );
    }
}

/// The sharpest form of the rule: the state a recommendation is PRODUCED in is a state it
/// already holds, so an update asking for it records nothing at all — the recommendation
/// still reports no last modification, as one nobody has acted on does.
#[tokio::test]
async fn an_update_to_the_state_the_recommendation_was_produced_in_records_nothing() {
    let client = Client::new();
    let arns = generated(&client, "status-as-produced").await;

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("as-produced-{index}");
        let before = reported(&client, arn, &case).await;
        assert_eq!(
            text(&before, "status"),
            STATUS_STATES[0].0,
            "case {case:?}: a recommendation nobody has acted on is ACTIVE, got {before:?}"
        );

        let answer = update_status(&client, arn, json!({ "status": STATUS_STATES[0].0 })).await;
        assert_served(&answer, &case);

        let after = reported(&client, arn, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: an update asking for the state the recommendation was produced in \
             must leave every member as it stood {before:?}, got {after:?}"
        );
        assert!(
            after.get("lastModifiedBy").is_none_or(Value::is_null)
                && after.get("lastModifiedAt").is_none_or(Value::is_null),
            "case {case:?}: a recommendation no update has changed reports no last \
             modification, got {after:?}"
        );
        let instant = stored_last_modified(&client, arn, &case);
        assert!(
            instant.is_none(),
            "case {case:?}: nothing was recorded, so no last-modification instant is held, \
             got {instant:?}"
        );
    }
}

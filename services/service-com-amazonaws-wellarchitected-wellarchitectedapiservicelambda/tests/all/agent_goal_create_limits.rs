// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the LIMITS `CreateAgentGoal` holds a create to, the part of
//! the operation's modelled surface the round-trip suite beside this one
//! (`super::agent_goals`) does not reach: its `ServiceQuotaExceededException` and the
//! clauses the model puts on the members it records.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. Optimization goals are metered PER PROFILE: a profile records goals up to the
//!    ceiling, the create past it is refused `402 ServiceQuotaExceededException` naming
//!    the profile as sent, the kind of resource the quota meters and the `@required`
//!    `QuotaCode` / `ServiceCode`, and nothing is recorded — while a profile below its
//!    own ceiling still records goals.
//! 2. Every clause the model puts on `pillars` bounds the create: a list outside
//!    `@length(1, 5)`, one repeating an element against `@uniqueItems`, one naming a
//!    value outside the `Pillar` enum and an absent `@required` member are each refused
//!    `400 ValidationException` naming the member, and record nothing; every published
//!    value and both published list bounds are accepted and read back as sent.
//! 3. The same holds for the strings a goal carries: `title` (`@length(1, 1000)`,
//!    `@required`) and `description` (`@length(1, 4000)`, optional) are accepted ON each
//!    published bound and refused one PAST it.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_GOALS_PER_AGENT_PROFILE;

use super::agent_goals::{create_goal, get_goal, member, profile_arn_of, PILLARS};
use super::workload_harness::{Answer, Client};

/// `ServiceQuotaExceededException$ResourceType` for an optimization goal:
/// `ExceptionResourceType` is a free-form string, and this service words the kind of
/// resource a goal failure is about this way on every operation that resolves or meters
/// one.
const GOAL_RESOURCE_TYPE: &str = "GOAL";

/// One member of the goal an answer carries.
fn goal_member<'a>(answer: &'a Answer, name: &str) -> Option<&'a Value> {
    answer.body.get("goal").and_then(|goal| goal.get(name))
}

/// The id of the goal a create was ACCEPTED for, so a row can read it back. The modelled
/// `UUID` shape of that id is pinned by the round-trip suite (`super::agent_goals`), so
/// what this suite asserts about an accepted create is the status and what the store then
/// reports.
fn accepted_goal_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentGoal 200, got {answer:?}"
    );
    let id = goal_member(answer, "id").and_then(Value::as_str);
    assert!(
        id.is_some_and(|id| !id.is_empty()),
        "case {case:?}: the answered goal must carry its @required id, got {answer:?}"
    );
    id.unwrap_or_default().to_string()
}

/// A valid `CreateAgentGoal` body, with `overrides` applied over it so a row states only
/// what it varies. A `null` override REMOVES the member, which is how a row omits one.
fn goal_body(title: &str, overrides: Value) -> Value {
    let mut body = json!({ "pillars": ["SECURITY"], "title": title });
    for (name, value) in overrides.as_object().expect("overrides are an object") {
        if value.is_null() {
            body.as_object_mut().expect("the body is an object").remove(name);
        } else {
            body[name] = value.clone();
        }
    }
    body
}

/// Rule 1: the per-profile goal quota. Checked at the ceiling, which is accepted, one
/// past it, which is the modelled 402, and on a SECOND profile, whose own count is below
/// the ceiling — so the refusal is the profile's quota and not a global one.
#[tokio::test]
async fn the_goal_quota_bounds_one_profile_and_is_metered_per_profile() {
    // The ceiling the emulator meters a profile's goals against (100 at the time of
    // writing). Read from the constant the handler reads, so what this suite asserts is
    // the BEHAVIOUR at and past the ceiling rather than the number itself.
    const QUOTA: usize = MAX_GOALS_PER_AGENT_PROFILE;

    let client = Client::new();
    let host = profile_arn_of(&client, "goal-quota-host", 1).await;
    let neighbour = profile_arn_of(&client, "goal-quota-neighbour", 1).await;

    let mut last_id = String::new();
    for index in 1..=QUOTA {
        let case = format!("goal-{index}");
        let answer = create_goal(
            &client,
            &host,
            goal_body(
                &format!("goal {index} inside the quota"),
                json!({ "pillars": [PILLARS[index % PILLARS.len()]] }),
            ),
        )
        .await;
        last_id = accepted_goal_id(&answer, &case);
    }
    assert_eq!(
        client.store().lock().agent_goal_count(&host),
        QUOTA,
        "case \"at-the-quota\": every goal inside the quota must be recorded"
    );

    let past = create_goal(
        &client,
        &host,
        goal_body("the goal one past the quota", json!({})),
    )
    .await;
    assert_eq!(
        past.status, 402,
        "case \"one-past-the-quota\": expected HTTP 402, got {past:?}"
    );
    assert_eq!(
        past.error_code, "ServiceQuotaExceededException",
        "case \"one-past-the-quota\": expected ServiceQuotaExceededException, got {past:?}"
    );
    assert_eq!(
        member(&past, "ResourceId"),
        host,
        "case \"one-past-the-quota\": ResourceId must be the profile as sent, got {past:?}"
    );
    assert_eq!(
        member(&past, "ResourceType"),
        GOAL_RESOURCE_TYPE,
        "case \"one-past-the-quota\": ResourceType must name what the quota meters, \
         got {past:?}"
    );
    assert!(
        !member(&past, "QuotaCode").is_empty() && !member(&past, "ServiceCode").is_empty(),
        "case \"one-past-the-quota\": QuotaCode and ServiceCode are @required, got {past:?}"
    );
    assert_eq!(
        client.store().lock().agent_goal_count(&host),
        QUOTA,
        "case \"one-past-the-quota\": a refused create must record nothing"
    );

    // The refusal is the quota and not a broken store: the last goal inside it resolves.
    let last = get_goal(&client, &host, &last_id).await;
    assert_eq!(
        last.status, 200,
        "case \"at-the-quota\": the last goal inside the quota must resolve, got {last:?}"
    );

    // Metered per profile: another profile, below its own ceiling, still records goals.
    for (case, pillars) in [
        ("neighbour-first", vec![PILLARS[0]]),
        ("neighbour-second", PILLARS.to_vec()),
    ] {
        let answer = create_goal(
            &client,
            &neighbour,
            goal_body(&format!("a {case} goal"), json!({ "pillars": pillars })),
        )
        .await;
        let id = accepted_goal_id(&answer, case);
        let read = get_goal(&client, &neighbour, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the neighbour's goal must resolve, got {read:?}"
        );
    }
    assert_eq!(
        client.store().lock().agent_goal_count(&neighbour),
        2,
        "case \"neighbour\": the quota of one profile does not meter another"
    );
}

/// Rule 2: every clause the model puts on `pillars`. The accepted rows are every
/// published value on its own and the whole catalog (the `@length` maximum, which
/// `uniqueItems` makes the longest list there is); the refused rows differ along WHICH
/// clause they break.
#[tokio::test]
async fn the_pillars_clauses_bound_the_goal_create() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-pillars-profile", 1).await;

    // Accepted: each published `Pillar` value alone — the `@length` minimum — and the
    // whole published catalog, which is the `@length` maximum.
    let accepted: Vec<(String, Vec<&str>)> = PILLARS
        .iter()
        .map(|pillar| ((*pillar).to_string(), vec![*pillar]))
        .chain([("the-whole-catalog".to_string(), PILLARS.to_vec())])
        .collect();
    let stored = accepted.len();
    for (case, pillars) in accepted {
        let sent = json!(pillars);
        let answer = create_goal(
            &client,
            &profile_arn,
            goal_body(
                &format!("a goal naming {case}"),
                json!({ "pillars": sent.clone() }),
            ),
        )
        .await;
        let id = accepted_goal_id(&answer, &case);
        let read = get_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created goal must resolve, got {read:?}"
        );
        assert_eq!(
            goal_member(&read, "pillars"),
            Some(&sent),
            "case {case:?}: the stored pillars must be the list as sent, got {read:?}"
        );
    }

    // Refused: one row per clause the model puts on the member.
    let refused: Vec<(&str, Value)> = vec![
        (
            "empty-below-the-length-minimum",
            json!({ "pillars": [] }),
        ),
        (
            // The enum publishes five values, so a list of six is necessarily one of them
            // twice: both the `@length` maximum and `@uniqueItems` refuse it.
            "six-entries-past-the-length-maximum",
            json!({ "pillars": [
                PILLARS[0], PILLARS[1], PILLARS[2], PILLARS[3], PILLARS[4], PILLARS[0],
            ] }),
        ),
        (
            "repeated-against-unique-items",
            json!({ "pillars": [PILLARS[1], PILLARS[1]] }),
        ),
        (
            // A real Well-Architected pillar that the `Pillar` enum does not publish.
            "outside-the-published-value-set",
            json!({ "pillars": ["SUSTAINABILITY"] }),
        ),
        ("absent-required-member", json!({ "pillars": null })),
    ];
    for (case, overrides) in refused {
        let answer = create_goal(
            &client,
            &profile_arn,
            goal_body(&format!("a refused goal: {case}"), overrides),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        // A value outside an enumerated type's set is reported as the schema finding that
        // quotes the value and the set, which names no member; every other clause here
        // names the member the request sent the value for.
        let expected = if case == "outside-the-published-value-set" {
            r#"instance value ("SUSTAINABILITY") not found in enum"#
        } else {
            "pillars"
        };
        assert!(
            member(&answer, "Message").contains(expected),
            "case {case:?}: the refusal must state {expected:?}, got {answer:?}"
        );
    }
    assert_eq!(
        client.store().lock().agent_goal_count(&profile_arn),
        stored,
        "case \"refused-rows\": a refused create records nothing"
    );
}

/// Rule 3: the `@length` bounds on the strings a goal carries, checked ON each published
/// bound (accepted) and one PAST it (refused).
#[tokio::test]
async fn the_goal_string_length_bounds_are_enforced_on_each_published_bound() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-strings-profile", 1).await;

    // (case, member, value) — the published bounds of `title` (@length(1, 1000)) and of
    // `description` (@length(1, 4000)), each accepted and read back as sent.
    let accepted: Vec<(&str, &str, String)> = vec![
        ("title-at-the-minimum", "title", "t".to_string()),
        ("title-at-the-maximum", "title", "t".repeat(1000)),
        ("description-at-the-minimum", "description", "d".to_string()),
        ("description-at-the-maximum", "description", "d".repeat(4000)),
    ];
    for (case, name, value) in accepted {
        let body = if name == "title" {
            goal_body(&value, json!({}))
        } else {
            goal_body(
                &format!("a goal with a {case} description"),
                json!({ "description": value }),
            )
        };
        let answer = create_goal(&client, &profile_arn, body).await;
        let id = accepted_goal_id(&answer, case);
        let read = get_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created goal must resolve, got {read:?}"
        );
        assert_eq!(
            goal_member(&read, name).and_then(Value::as_str),
            Some(value.as_str()),
            "case {case:?}: the stored {name} must be the value as sent, got {read:?}"
        );
    }

    // One past each published bound, and the absent `@required` title.
    let refused: Vec<(&str, &str, Value)> = vec![
        ("title-below-the-minimum", "title", json!({ "title": "" })),
        (
            "title-past-the-maximum",
            "title",
            json!({ "title": "t".repeat(1001) }),
        ),
        (
            "description-below-the-minimum",
            "description",
            json!({ "description": "" }),
        ),
        (
            "description-past-the-maximum",
            "description",
            json!({ "description": "d".repeat(4001) }),
        ),
        ("absent-required-title", "title", json!({ "title": null })),
    ];
    let stored = client.store().lock().agent_goal_count(&profile_arn);
    for (case, name, overrides) in refused {
        let answer = create_goal(
            &client,
            &profile_arn,
            goal_body(&format!("a refused goal: {case}"), overrides),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            member(&answer, "Message").contains(name),
            "case {case:?}: the refusal must name the member, got {answer:?}"
        );
    }
    assert_eq!(
        client.store().lock().agent_goal_count(&profile_arn),
        stored,
        "case \"refused-rows\": a refused create records nothing"
    );

    // The optional `description` may be absent: the goal is recorded without it.
    let answer = create_goal(
        &client,
        &profile_arn,
        goal_body("a goal with no description", json!({})),
    )
    .await;
    let id = accepted_goal_id(&answer, "absent-optional-description");
    let read = get_goal(&client, &profile_arn, &id).await;
    assert_eq!(
        goal_member(&read, "description"),
        None,
        "case \"absent-optional-description\": an omitted optional member stays absent, \
         got {read:?}"
    );
}

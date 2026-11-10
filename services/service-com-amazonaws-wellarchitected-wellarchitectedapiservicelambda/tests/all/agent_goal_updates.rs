// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAgentGoal`: the operation that changes a stored
//! optimization goal of an agent profile, read back through the `GetAgentGoal` beside it.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. An update MERGES: every member the request carries replaces the stored one, every
//!    member it leaves out keeps the value the goal already had, and the goal's identity
//!    (`id`, `profileArn`, `createdBy`, `createdAt`) is untouched. The answer is the
//!    stored record, so `GetAgentGoal` reports the identical summary.
//! 2. A successful update records WHEN and BY WHOM: `lastModifiedAt` is the instant of
//!    the call and `lastModifiedBy` the account the request is attributed to, and
//!    successive updates compose on the STORED goal rather than on the created one.
//! 3. The `(profileArn, id)` pair is RESOLVED: a well-formed `profileArn` naming no
//!    stored profile, a well-formed `id` naming no goal of it, and the id of a goal
//!    belonging to ANOTHER profile are each `404 ResourceNotFoundException` naming the
//!    identifier as sent — and change nothing.
//! 4. A `clientToken` makes the update idempotent: the same token with the same
//!    parameters answers the goal as the original update left it, without stamping the
//!    modification members again; a fresh token applies the next update.
//! 5. Every clause the model puts on an updatable member bounds the update exactly as it
//!    bounds the create: `pillars` (`@length(1, 5)`, `@uniqueItems`, the `Pillar` enum),
//!    `title` (`@length(1, 1000)`) and `description` (`@length(1, 4000)`) are accepted on
//!    each published bound and refused one past it, with `400 ValidationException` naming
//!    the member and leaving the stored goal as it was.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::agent_goals::{
    absent_profile_arn, assert_create_instant, create_goal, delete_goal, get_goal, member,
    profile_arn_of, PILLARS,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// `ResourceNotFoundException$ResourceType` for a goal and for the agent profile it hangs
/// off, as this service words each (`ExceptionResourceType` is a free-form string).
const GOAL_RESOURCE_TYPE: &str = "GOAL";
const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// `PUT /api/v1/agent-profiles/{profileArn}/goals/{id}` — the modelled route of
/// `UpdateAgentGoal` (`smithy_query(query="operation", operation="UpdateAgentGoal")`).
///
/// `pub(crate)` because a goal is changed by exactly ONE operation, so the suite about the
/// update that supplies no member (`super::agent_no_op_updates`) sends its requests through
/// this builder rather than building the same request again.
pub(crate) async fn update_goal(
    client: &Client,
    profile_arn: &str,
    id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/api/v1/agent-profiles/{}/goals/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            Some(body),
        )
        .await
}

/// The `goal` an accepted answer carries.
fn accepted_goal(answer: &Answer, case: &str, operation: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{operation}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("goal")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}/{operation}: expected a goal, got {answer:?}"))
}

/// The body every row of this suite starts its goal from: a goal carrying all three
/// updatable members, so a row that omits one is distinguishable from a row that sends it.
fn base_body() -> Value {
    json!({
        "pillars": ["SECURITY", "COST_OPTIMIZATION"],
        "title": "the stored title",
        "description": "the stored description",
    })
}

/// A goal of `profile_arn` carrying [`base_body`], with the summary its create answered.
async fn stored_goal(client: &Client, profile_arn: &str, case: &str) -> (String, Value) {
    let created = create_goal(client, profile_arn, base_body()).await;
    let goal = accepted_goal(&created, case, "CreateAgentGoal");
    let id = goal
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: the created goal must carry its @required id, got {goal:?}"
    );
    (id, goal)
}

/// Rule 1 and rule 2: each row sends a different SUBSET of the updatable members, and the
/// goal that comes back carries the sent members as sent, the omitted members as stored,
/// its identity unchanged, and the modification members of this call.
#[tokio::test]
async fn an_update_merges_the_members_it_carries_and_leaves_the_rest() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-update-host", 1).await;

    // The rows differ along WHICH members the request carries (one, two, all) and along
    // the values themselves — the single-pillar and five-pillar bounds of
    // `@length(1, 5)`, the one-character and 1000-character bounds of `title`'s
    // `@length(1, 1000)`, and the one-character and 4000-character bounds of
    // `description`'s `@length(1, 4000)`. A request that carries NONE of them asks for no
    // change, so it stamps no modification and is pinned by `super::agent_no_op_updates`
    // instead.
    let rows: Vec<(&str, Value)> = vec![
        ("pillars-only", json!({ "pillars": ["RESILIENCE"] })),
        ("pillars-every-published-value", json!({ "pillars": PILLARS })),
        ("title-only", json!({ "title": "a retitled goal" })),
        ("shortest-title", json!({ "title": "x" })),
        ("longest-title", json!({ "title": "t".repeat(1000) })),
        (
            "description-only",
            json!({ "description": "a redescribed goal" }),
        ),
        ("shortest-description", json!({ "description": "d" })),
        (
            "longest-description",
            json!({ "description": "e".repeat(4000) }),
        ),
        (
            "pillars-and-title",
            json!({ "pillars": ["PERFORMANCE", "OPERATIONAL_EXCELLENCE"], "title": "both" }),
        ),
        (
            "every-member",
            json!({
                "pillars": ["COST_OPTIMIZATION"],
                "title": "all three members at once",
                "description": "all three members at once",
            }),
        ),
        (
            "the-stored-values-again",
            json!({
                "pillars": ["SECURITY", "COST_OPTIMIZATION"],
                "title": "the stored title",
                "description": "the stored description",
            }),
        ),
    ];

    for (case, patch) in rows {
        let (id, created) = stored_goal(&client, &profile_arn, case).await;

        let answer = update_goal(&client, &profile_arn, &id, patch.clone()).await;
        let updated = accepted_goal(&answer, case, "UpdateAgentGoal");

        // A member the request carried reads back as sent; one it omitted reads back as
        // the create stored it.
        for name in ["pillars", "title", "description"] {
            let expected = patch.get(name).or_else(|| created.get(name));
            assert_eq!(
                updated.get(name),
                expected,
                "case {case:?}: expected {name} {expected:?}, got {updated:?}"
            );
        }
        // The goal's identity is not updatable.
        for name in ["id", "profileArn", "createdBy", "createdAt"] {
            assert_eq!(
                updated.get(name),
                created.get(name),
                "case {case:?}: {name} must be untouched by an update, got {updated:?}"
            );
        }
        assert_eq!(
            updated.get("lastModifiedBy").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: lastModifiedBy must be the account the request is attributed \
             to, got {updated:?}"
        );
        assert_create_instant(updated.get("lastModifiedAt"), case, "lastModifiedAt");

        // The answer is the STORED record, so the reader reports the identical summary.
        let fetched = get_goal(&client, &profile_arn, &id).await;
        let read_back = accepted_goal(&fetched, case, "GetAgentGoal");
        assert_eq!(
            read_back, updated,
            "case {case:?}: GetAgentGoal must report the updated goal, got {fetched:?}"
        );
    }
}

/// Rule 2, for composition: a second update merges onto what the FIRST one left, not onto
/// what the create stored, and never moves the modification instant backwards. The rows
/// are the order the two members are updated in, so neither order is privileged.
#[tokio::test]
async fn successive_updates_compose_on_the_stored_goal() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-update-sequence", 1).await;

    let orders: [(&str, [(&str, Value); 2]); 2] = [
        (
            "pillars-then-title",
            [
                ("pillars", json!({ "pillars": ["RESILIENCE", "PERFORMANCE"] })),
                ("title", json!({ "title": "retitled after the pillars" })),
            ],
        ),
        (
            "title-then-pillars",
            [
                ("title", json!({ "title": "retitled before the pillars" })),
                ("pillars", json!({ "pillars": ["OPERATIONAL_EXCELLENCE"] })),
            ],
        ),
    ];

    for (case, steps) in orders {
        let (id, created) = stored_goal(&client, &profile_arn, case).await;
        let mut expected = created.clone();
        let mut previous_instant = 0.0_f64;

        for (step, patch) in steps {
            let answer = update_goal(&client, &profile_arn, &id, patch.clone()).await;
            let updated = accepted_goal(&answer, case, "UpdateAgentGoal");
            for name in ["pillars", "title", "description"] {
                if let Some(value) = patch.get(name) {
                    expected[name] = value.clone();
                }
                assert_eq!(
                    updated.get(name),
                    expected.get(name),
                    "case {case:?}/{step}: expected {name} {:?}, got {updated:?}",
                    expected.get(name)
                );
            }
            let instant = updated
                .get("lastModifiedAt")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            assert!(
                instant >= previous_instant,
                "case {case:?}/{step}: lastModifiedAt must not move backwards \
                 (was {previous_instant}), got {instant}"
            );
            previous_instant = instant;
        }

        let fetched = get_goal(&client, &profile_arn, &id).await;
        let read_back = accepted_goal(&fetched, case, "GetAgentGoal");
        for name in ["pillars", "title", "description"] {
            assert_eq!(
                read_back.get(name),
                expected.get(name),
                "case {case:?}: the stored goal must hold both updates, got {fetched:?}"
            );
        }
    }
}

/// Rule 3: the pair the update resolves. The rows are the three ways it can fail to
/// resolve — an absent profile, an absent goal id (each extreme of the modelled `UUID`
/// value space), and a goal of another profile — and each leaves every stored goal as it
/// was.
#[tokio::test]
async fn an_update_of_an_unresolvable_goal_resolves_to_a_not_found_and_writes_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-update-lookup", 1).await;
    let other_arn = profile_arn_of(&client, "goal-update-other", 0).await;
    let (id, created) = stored_goal(&client, &profile_arn, "present").await;
    let (other_id, other_created) = stored_goal(&client, &other_arn, "other-profile").await;

    let patch = json!({ "title": "an update that must not land" });

    // An absent PROFILE is reported as the missing profile, naming the ARN as sent.
    let absent_arns: Vec<(&str, String)> = vec![
        ("absent-hex-resource", absent_profile_arn(&"0".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-9")),
        (
            "absent-in-another-region",
            format!(
                "arn:aws:wellarchitected:eu-west-1:{}:agent-profile/{}",
                engine::emulator_account_id(),
                "f".repeat(32)
            ),
        ),
    ];
    for (case, arn) in &absent_arns {
        let answer = update_goal(&client, arn, &id, patch.clone()).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an absent profile must not resolve, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            AGENT_PROFILE_RESOURCE_TYPE,
            "case {case:?}: expected ResourceType {AGENT_PROFILE_RESOURCE_TYPE}, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            arn.as_str(),
            "case {case:?}: ResourceId must be the ARN as sent, got {answer:?}"
        );
    }

    // An absent GOAL of a stored profile is reported as the missing goal, naming the
    // composite `<profileArn>.<id>` as sent.
    let absent_ids = [
        ("all-zeros", "00000000-0000-0000-0000-000000000000"),
        ("all-f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("mixed-hex", "0123abcd-4567-89ef-0123-456789abcdef"),
    ];
    for (case, absent_id) in absent_ids {
        let answer = update_goal(&client, &profile_arn, absent_id, patch.clone()).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an absent goal must not resolve, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            GOAL_RESOURCE_TYPE,
            "case {case:?}: expected ResourceType {GOAL_RESOURCE_TYPE}, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            format!("{profile_arn}.{absent_id}"),
            "case {case:?}: ResourceId must be the composite as sent, got {answer:?}"
        );
    }

    // A goal is reached only THROUGH the profile that holds it, in either direction.
    for (case, arn, goal_id) in [
        ("other-profiles-goal", &profile_arn, &other_id),
        ("this-profiles-goal", &other_arn, &id),
    ] {
        let answer = update_goal(&client, arn, goal_id, patch.clone()).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a goal of another profile must not resolve, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            format!("{arn}.{goal_id}"),
            "case {case:?}: ResourceId must be the composite as sent, got {answer:?}"
        );
    }

    // Nothing any refused request named was written: both stored goals still report the
    // summary their create answered, modification members and all.
    for (case, arn, goal_id, expected) in [
        ("present", &profile_arn, &id, &created),
        ("other-profile", &other_arn, &other_id, &other_created),
    ] {
        let fetched = get_goal(&client, arn, goal_id).await;
        assert_eq!(
            fetched.body.get("goal"),
            Some(expected),
            "case {case:?}: a refused update must leave the goal untouched, got {fetched:?}"
        );
    }
}

/// Rule 4: `clientToken` replay. The rows are what a token can be — replayed with the
/// same parameters, replayed with different ones, and fresh — each checked against what
/// the store then reports.
#[tokio::test]
async fn a_client_token_makes_the_goal_update_idempotent() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-update-token", 1).await;
    let (id, _created) = stored_goal(&client, &profile_arn, "token").await;

    let original = json!({
        "pillars": ["RESILIENCE"],
        "title": "the first update",
        "clientToken": "goal-update-token-1",
    });
    let first = update_goal(&client, &profile_arn, &id, original.clone()).await;
    let after_first = accepted_goal(&first, "same-token-same-parameters", "UpdateAgentGoal");

    // A retry of the SAME request acts a second time on nothing: the modification members
    // are the first update's, not this call's.
    let replay = update_goal(&client, &profile_arn, &id, original.clone()).await;
    let replayed = accepted_goal(&replay, "same-token-same-parameters", "UpdateAgentGoal");
    assert_eq!(
        replayed, after_first,
        "case \"same-token-same-parameters\": the replay must answer the goal the original \
         update left, got {replay:?}"
    );
    assert_eq!(
        client.store().lock().update_agent_goal_tokens.len(),
        1,
        "case \"same-token-same-parameters\": a replay remembers no second token"
    );

    // The SAME body WITHOUT the token is not a replay, so it is applied: the modification
    // instant moves, which is exactly what the replay above must not do.
    let mut untokened = original.clone();
    untokened
        .as_object_mut()
        .expect("the request body is an object")
        .remove("clientToken");
    let repeated = update_goal(&client, &profile_arn, &id, untokened).await;
    let after_repeat = accepted_goal(&repeated, "no-token-same-parameters", "UpdateAgentGoal");
    let replay_instant = replayed
        .get("lastModifiedAt")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let repeat_instant = after_repeat
        .get("lastModifiedAt")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    assert!(
        repeat_instant >= replay_instant,
        "case \"no-token-same-parameters\": an untokened repeat is applied, so its \
         lastModifiedAt is not before the replay's ({replay_instant}), got {repeat_instant}"
    );
    assert_eq!(
        client.store().lock().update_agent_goal_tokens.len(),
        1,
        "case \"no-token-same-parameters\": a request carrying no token remembers none"
    );

    // The same token carrying DIFFERENT parameters is a different request, and this
    // operation models no conflict for one, so it is served on its own merits.
    let mut changed = original.clone();
    changed["title"] = json!("the second update");
    let second = update_goal(&client, &profile_arn, &id, changed).await;
    let after_second = accepted_goal(&second, "same-token-other-parameters", "UpdateAgentGoal");
    assert_eq!(
        after_second.get("title").and_then(Value::as_str),
        Some("the second update"),
        "case \"same-token-other-parameters\": the update must land, got {second:?}"
    );

    // A fresh token applies the next update, and the one before it is what it merges onto.
    let third = update_goal(
        &client,
        &profile_arn,
        &id,
        json!({
            "description": "the third update",
            "clientToken": "goal-update-token-2",
        }),
    )
    .await;
    let after_third = accepted_goal(&third, "fresh-token", "UpdateAgentGoal");
    assert_eq!(
        after_third.get("title").and_then(Value::as_str),
        Some("the second update"),
        "case \"fresh-token\": the merge is onto the stored goal, got {third:?}"
    );
    assert_eq!(
        after_third.get("description").and_then(Value::as_str),
        Some("the third update"),
        "case \"fresh-token\": the update must land, got {third:?}"
    );
    assert_eq!(
        client.store().lock().update_agent_goal_tokens.len(),
        2,
        "case \"token-rows\": exactly the two tokens the rows spent are remembered"
    );

    // A token resolving to a goal that is gone is forgotten with it, so nothing outlives
    // the goal it names.
    let deleted = delete_goal(&client, &profile_arn, &id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete-the-token-goal\": expected 200, got {deleted:?}"
    );
    assert!(
        client.store().lock().update_agent_goal_tokens.is_empty(),
        "case \"delete-the-token-goal\": the update tokens of a deleted goal are forgotten"
    );
}

/// Rule 5: every clause the model puts on an updatable member bounds the update too. The
/// rows break one clause each, one PAST a published bound where the bound is a range (the
/// bounds themselves are accepted by the merge suite above).
#[tokio::test]
async fn a_member_outside_its_modelled_clause_is_refused_and_the_goal_is_untouched() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-update-validation", 1).await;
    let (id, created) = stored_goal(&client, &profile_arn, "validation").await;

    let rows: Vec<(&str, &str, Value)> = vec![
        ("empty-pillars", "pillars", json!({ "pillars": [] })),
        (
            "six-pillars",
            "pillars",
            json!({ "pillars": ["SECURITY", "RESILIENCE", "PERFORMANCE",
                                "COST_OPTIMIZATION", "OPERATIONAL_EXCELLENCE", "SECURITY"] }),
        ),
        (
            "repeated-pillar",
            "pillars",
            json!({ "pillars": ["SECURITY", "SECURITY"] }),
        ),
        (
            "pillar-outside-the-enum",
            "pillars",
            json!({ "pillars": ["SUSTAINABILITY"] }),
        ),
        ("empty-title", "title", json!({ "title": "" })),
        (
            "title-past-the-bound",
            "title",
            json!({ "title": "t".repeat(1001) }),
        ),
        ("empty-description", "description", json!({ "description": "" })),
        (
            "description-past-the-bound",
            "description",
            json!({ "description": "e".repeat(4001) }),
        ),
    ];

    for (case, refused_member, patch) in rows {
        let answer = update_goal(&client, &profile_arn, &id, patch).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        // A value outside an enumerated type's set is reported as the schema finding that
        // quotes the value and the set, which names no member; every other clause here
        // names the member the request sent the value for.
        let expected = if case == "pillar-outside-the-enum" {
            r#"instance value ("SUSTAINABILITY") not found in enum"#
        } else {
            refused_member
        };
        assert!(
            member(&answer, "Message").contains(expected),
            "case {case:?}: the refusal must state {expected:?}, got {answer:?}"
        );

        let fetched = get_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            fetched.body.get("goal"),
            Some(&created),
            "case {case:?}: a refused update must leave the goal untouched, got {fetched:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `UpdateRescoreExecutionPlan`, end to end over the wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one call:
//!
//! * an update replaces exactly the members the request carries and leaves every other
//!   stored member of the plan as it was, which `DescribeRescoreExecutionPlan` reads
//!   back;
//! * an accepted update takes the plan out of `ACTIVE` into the transient `UPDATING`
//!   status the model declares, and the clock owes it `ACTIVE` again a span later;
//! * while a plan is not `ACTIVE`, a control-plane call that mutates it (another
//!   update, a delete) is refused with the modelled `ConflictException` and
//!   changes nothing;
//! * an update addressed to an id no plan was created for is refused with
//!   `ResourceNotFoundException`.
//!
//! Both refusals travel with this service's client-error status (400 on its RPC
//! protocols — see `wire_error_status.rs`), and the error TYPE in the body is what
//! distinguishes them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, create_plan, emulator, error_code, sleep_past_the_transition};

/// Describe `id`, asserting the call succeeded, and return the body.
async fn describe(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, described) = call(router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe {id}: got {described}");
    described
}

/// Create a plan with every member set, so an update that carries one member can be
/// seen to leave the others alone, and wait out the transient `CREATING` status so the
/// plan is `ACTIVE` and can take an update.
async fn create_full_plan(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": name,
            "Description": "the description the plan was created with",
            "CapacityUnits": { "RescoreCapacityUnits": 1 },
        }),
    )
    .await;
    assert_eq!(status, 200, "create {name}: got {created}");
    sleep_past_the_transition();
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name}: no Id in {created}"))
        .to_string()
}

/// One row of the update table: the request's members, and what the plan must read back
/// as afterwards.
struct UpdateCase {
    case: &'static str,
    /// The `UpdateRescoreExecutionPlan` members beside `Id`.
    request: Value,
    expected_name: String,
    expected_description: String,
    expected_capacity: i64,
}

#[tokio::test]
async fn an_update_replaces_exactly_the_members_it_carries() {
    // The rule generalises over WHICH members a request carries and over the values
    // they take: one member, two, all three, none at all, and a value at a published
    // bound. `Name`'s bound is `@length(1, 1000)` and `RescoreCapacityUnits`'s minimum
    // is 0 (smithy_query: UpdateRescoreExecutionPlan.Name -> RescoreExecutionPlanName,
    // CapacityUnitsConfiguration$RescoreCapacityUnits range min 0).
    let created_name = "base-plan";
    let created_description = "the description the plan was created with";
    let cases = vec![
        UpdateCase {
            case: "Name only",
            request: json!({ "Name": "renamed-plan" }),
            expected_name: "renamed-plan".to_string(),
            expected_description: created_description.to_string(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "Description only",
            request: json!({ "Description": "a new description" }),
            expected_name: created_name.to_string(),
            expected_description: "a new description".to_string(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "CapacityUnits only",
            request: json!({ "CapacityUnits": { "RescoreCapacityUnits": 7 } }),
            expected_name: created_name.to_string(),
            expected_description: created_description.to_string(),
            expected_capacity: 7,
        },
        UpdateCase {
            case: "Name and Description together",
            request: json!({ "Name": "renamed-again", "Description": "and redescribed" }),
            expected_name: "renamed-again".to_string(),
            expected_description: "and redescribed".to_string(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "every updatable member at once",
            request: json!({
                "Name": "wholly-updated",
                "Description": "wholly updated",
                "CapacityUnits": { "RescoreCapacityUnits": 3 },
            }),
            expected_name: "wholly-updated".to_string(),
            expected_description: "wholly updated".to_string(),
            expected_capacity: 3,
        },
        UpdateCase {
            case: "no member beyond the id",
            request: json!({}),
            expected_name: created_name.to_string(),
            expected_description: created_description.to_string(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "an empty Description, at the bottom of its 0..1000 length",
            request: json!({ "Description": "" }),
            expected_name: created_name.to_string(),
            expected_description: String::new(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "a Name at the top of its 1..1000 length",
            request: json!({ "Name": "n".repeat(1000) }),
            expected_name: "n".repeat(1000),
            expected_description: created_description.to_string(),
            expected_capacity: 1,
        },
        UpdateCase {
            case: "RescoreCapacityUnits at the bottom of its published range",
            request: json!({ "CapacityUnits": { "RescoreCapacityUnits": 0 } }),
            expected_name: created_name.to_string(),
            expected_description: created_description.to_string(),
            expected_capacity: 0,
        },
    ];

    for UpdateCase {
        case,
        request,
        expected_name,
        expected_description,
        expected_capacity,
    } in cases
    {
        // A fresh emulator per row, so each row updates a plan that is ACTIVE and the
        // rows cannot see each other's writes.
        let router = emulator();
        let id = create_full_plan(&router, created_name).await;
        let before = describe(&router, &id).await;

        let mut body = request.clone();
        body["Id"] = json!(id);
        let (status, updated) = call(&router, "UpdateRescoreExecutionPlan", body).await;
        assert_eq!(status, 200, "case {case:?}: got {updated}");

        let after = describe(&router, &id).await;
        assert_eq!(
            after["Name"],
            json!(expected_name),
            "case {case:?}: expected Name {expected_name:?}, got {after}"
        );
        assert_eq!(
            after["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {after}"
        );
        assert_eq!(
            after["CapacityUnits"]["RescoreCapacityUnits"],
            json!(expected_capacity),
            "case {case:?}: expected RescoreCapacityUnits {expected_capacity}, got {after}"
        );
        // The plan's identity is not something an update can change, and its creation
        // instant is not either.
        assert_eq!(
            after["Id"], before["Id"],
            "case {case:?}: an update must not change the Id, got {after}"
        );
        assert_eq!(
            after["Arn"], before["Arn"],
            "case {case:?}: an update must not change the Arn, got {after}"
        );
        assert_eq!(
            after["CreatedAt"], before["CreatedAt"],
            "case {case:?}: an update must not change CreatedAt, got {after}"
        );
        let created_at = after["CreatedAt"].as_f64().unwrap_or(f64::NAN);
        let updated_at = after["UpdatedAt"].as_f64().unwrap_or(f64::NAN);
        assert!(
            updated_at >= created_at,
            "case {case:?}: expected UpdatedAt >= CreatedAt, got {updated_at} and {created_at}"
        );

        // And the listing agrees with the describe, so the update reached the one
        // stored plan both readers answer from.
        let (status, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
        assert_eq!(status, 200, "case {case:?}: got {listed}");
        let summaries = listed["SummaryItems"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: an update must not create a plan, got {listed}"
        );
        assert_eq!(
            summaries[0]["Name"],
            json!(expected_name),
            "case {case:?}: the listing must report the updated Name, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_accepted_update_puts_the_plan_through_updating_before_it_is_active_again() {
    // The rule holds whichever member the update carries, so the table spans the same
    // classes of request as the persistence rule above.
    let requests: Vec<(&str, Value)> = vec![
        ("Name only", json!({ "Name": "status-renamed" })),
        ("Description only", json!({ "Description": "status" })),
        (
            "CapacityUnits only",
            json!({ "CapacityUnits": { "RescoreCapacityUnits": 4 } }),
        ),
        ("no member beyond the id", json!({})),
    ];

    for (case, request) in requests {
        let router = emulator();
        let id = create_full_plan(&router, "status-plan").await;
        let before = describe(&router, &id).await;
        assert_eq!(
            before["Status"],
            json!(state::STATUS_ACTIVE),
            "case {case:?}: a created plan must be ACTIVE before the update, got {before}"
        );

        let mut body = request.clone();
        body["Id"] = json!(id);
        let (status, updated) = call(&router, "UpdateRescoreExecutionPlan", body).await;
        assert_eq!(status, 200, "case {case:?}: got {updated}");

        // Straight after the call the transient status is observable, in both readers.
        let during = describe(&router, &id).await;
        assert_eq!(
            during["Status"],
            json!(state::STATUS_UPDATING),
            "case {case:?}: expected Status {:?} straight after the update, got {during}",
            state::STATUS_UPDATING
        );
        let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
        assert_eq!(
            listed["SummaryItems"][0]["Status"],
            json!(state::STATUS_UPDATING),
            "case {case:?}: the listing must report the same transient status, got {listed}"
        );

        // The transition is owed by the clock alone: nothing but time passing settles it.
        sleep_past_the_transition();
        let settled = describe(&router, &id).await;
        assert_eq!(
            settled["Status"],
            json!(state::STATUS_ACTIVE),
            "case {case:?}: expected Status {:?} once the span has passed, got {settled}",
            state::STATUS_ACTIVE
        );
        let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
        assert_eq!(
            listed["SummaryItems"][0]["Status"],
            json!(state::STATUS_ACTIVE),
            "case {case:?}: the listing must settle with the describe, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_mutating_call_on_a_plan_that_is_not_active_is_refused_with_conflict_exception() {
    // The rule spans the mutating control-plane calls a plan mid-transition can
    // receive: another update (whichever member it carries) and a delete.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a second update while the first is in flight",
            "UpdateRescoreExecutionPlan",
            json!({ "Name": "conflicting-rename" }),
        ),
        (
            "a second update carrying only capacity",
            "UpdateRescoreExecutionPlan",
            json!({ "CapacityUnits": { "RescoreCapacityUnits": 9 } }),
        ),
        (
            "a second update carrying no member beyond the id",
            "UpdateRescoreExecutionPlan",
            json!({}),
        ),
        (
            "a delete while the update is in flight",
            "DeleteRescoreExecutionPlan",
            json!({}),
        ),
    ];

    for (case, operation, request) in cases {
        let router = emulator();
        let id = create_full_plan(&router, "conflict-plan").await;
        let (status, updated) = call(
            &router,
            "UpdateRescoreExecutionPlan",
            json!({ "Id": id, "Description": "the update that opens the window" }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: the first update: got {updated}");

        let mut body = request.clone();
        body["Id"] = json!(id);
        let (status, refused) = call(&router, operation, body.clone()).await;
        assert_eq!(status, 400, "case {case:?} on {operation}: got {refused}");
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?} on {operation}: got {refused}"
        );

        // A refused call changed nothing: the plan is still there, still carrying what
        // the accepted update wrote.
        let during = describe(&router, &id).await;
        assert_eq!(
            during["Description"],
            json!("the update that opens the window"),
            "case {case:?}: a refused call must not write, got {during}"
        );
        assert_eq!(
            during["Status"],
            json!(state::STATUS_UPDATING),
            "case {case:?}: a refused call must not move the status, got {during}"
        );

        // Once the clock has settled the plan, the very same call is accepted — so the
        // refusal is about the plan's status and not about the request.
        sleep_past_the_transition();
        let (status, accepted) = call(&router, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?} on {operation}: accepted once the plan is {:?}, got {accepted}",
            state::STATUS_ACTIVE
        );
    }
}

#[tokio::test]
async fn an_update_of_an_id_no_plan_was_created_for_is_not_found() {
    let router = emulator();
    // A live plan beside it, so the refusal is about the id and not about an empty store.
    let live = create_plan(&router, "live-plan").await;

    // Ids of the modelled 36-character shape that no create in this test minted: the
    // all-zero uuid, one differing from the live plan's id in its last character, and
    // an all-hyphen-free one.
    let mut altered: Vec<char> = live.chars().collect();
    let last = altered.len() - 1;
    altered[last] = if altered[last] == 'f' { 'e' } else { 'f' };
    let unknown_ids = vec![
        ("the all-zero uuid".to_string(), "00000000-0000-4000-8000-000000000000".to_string()),
        (
            "a live id with its last character changed".to_string(),
            altered.into_iter().collect::<String>(),
        ),
        (
            "36 characters with no hyphen at all".to_string(),
            "a".repeat(36),
        ),
    ];

    for (case, id) in unknown_ids {
        for (member_case, request) in [
            ("Name", json!({ "Name": "no-such-plan" })),
            ("Description", json!({ "Description": "no such plan" })),
            ("no member", json!({})),
        ] {
            let mut body = request.clone();
            body["Id"] = json!(id);
            let (status, refused) = call(&router, "UpdateRescoreExecutionPlan", body).await;
            assert_eq!(
                status, 400,
                "case {case:?} with {member_case}: got {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?} with {member_case}: got {refused}"
            );
        }
    }

    // And nothing was created or changed by any of those refusals.
    let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
    let summaries = listed["SummaryItems"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "a refused update must not create a plan, got {listed}"
    );
    assert_eq!(
        summaries[0]["Id"],
        json!(live),
        "the live plan must be the only one, got {listed}"
    );
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused_and_writes_nothing() {
    let router = emulator();
    let id = create_full_plan(&router, "constrained-plan").await;

    // One row per constraint class on this operation's input (smithy_query:
    // UpdateRescoreExecutionPlan.Id @required @length(36,36), Name @length(1,1000)
    // @pattern "^[a-zA-Z0-9][a-zA-Z0-9_-]*$", Description @length(0,1000),
    // CapacityUnitsConfiguration$RescoreCapacityUnits range min 0).
    let cases: Vec<(&str, Value)> = vec![
        (
            "a Name with a character outside its pattern",
            json!({ "Id": id, "Name": "renamed plan!" }),
        ),
        (
            "a Name starting outside its pattern",
            json!({ "Id": id, "Name": "-renamed" }),
        ),
        ("an empty Name", json!({ "Id": id, "Name": "" })),
        (
            "a Name one past its 1000-character bound",
            json!({ "Id": id, "Name": "n".repeat(1001) }),
        ),
        (
            "a Description one past its 1000-character bound",
            json!({ "Id": id, "Description": "d".repeat(1001) }),
        ),
        (
            "a negative RescoreCapacityUnits",
            json!({ "Id": id, "CapacityUnits": { "RescoreCapacityUnits": -1 } }),
        ),
        (
            "an id that is not 36 characters",
            json!({ "Id": "too-short", "Name": "renamed" }),
        ),
        ("an absent required Id", json!({ "Name": "renamed" })),
    ];

    for (case, body) in cases {
        let (status, refused) = call(&router, "UpdateRescoreExecutionPlan", body).await;
        assert_eq!(status, 400, "case {case:?}: got {refused}");
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: got {refused}"
        );
    }

    // Every refusal above left the plan exactly as it was created, including its status.
    let after = describe(&router, &id).await;
    assert_eq!(
        after["Name"],
        json!("constrained-plan"),
        "a refused update must not write, got {after}"
    );
    assert_eq!(
        after["Description"],
        json!("the description the plan was created with"),
        "a refused update must not write, got {after}"
    );
    assert_eq!(
        after["CapacityUnits"]["RescoreCapacityUnits"],
        json!(1),
        "a refused update must not write, got {after}"
    );
    assert_eq!(
        after["Status"],
        json!(state::STATUS_ACTIVE),
        "a refused update must not move the status, got {after}"
    );
}

#[tokio::test]
async fn an_update_touches_only_the_plan_it_addresses() {
    let router = emulator();
    let names = ["plan-one", "plan-two", "plan-three", "plan-four"];
    let mut ids = Vec::new();
    for name in names {
        ids.push(create_full_plan(&router, name).await);
    }
    // Update each plan in turn (one per emulator-wide span, so no conflict), and after
    // each one every OTHER plan must still carry the name it was created with.
    for (index, id) in ids.iter().enumerate() {
        let renamed = format!("renamed-{index}");
        let (status, updated) = call(
            &router,
            "UpdateRescoreExecutionPlan",
            json!({ "Id": id, "Name": renamed }),
        )
        .await;
        assert_eq!(status, 200, "updating plan {index}: got {updated}");

        for (other, other_id) in ids.iter().enumerate() {
            let described = describe(&router, other_id).await;
            let expected = if other <= index {
                format!("renamed-{other}")
            } else {
                names[other].to_string()
            };
            assert_eq!(
                described["Name"],
                json!(expected),
                "after updating plan {index}, plan {other}: expected Name {expected:?}, got {described}"
            );
        }
        sleep_past_the_transition();
    }
}

// SPDX-License-Identifier: CC0-1.0

//! Three control-plane rules of this service, each asserted over a TABLE of inputs
//! rather than one recorded call:
//!
//! 1. A request outside a modelled constraint is refused with this service's validation
//!    report: the violation count, then one `Value at '<member>' failed to satisfy
//!    constraint: <requirement>` clause per violation — the offending value never
//!    rendered, and a `@pattern` quoted without the `^` / `$` anchors the model spells
//!    it with.
//! 2. A rescore-execution-plan id that names no plan this service holds is answered with
//!    the CONSTANT not-found message, whichever operation looked it up and whether the
//!    plan never existed or was deleted.
//! 3. Creating a plan is asynchronous: the plan is reported `CREATING` until the
//!    transition the clock owes it falls due and only then `ACTIVE`, and `Rescore`
//!    against a plan that is not `ACTIVE` is refused with the modelled
//!    `ValidationException` (HTTP 400).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{
    call, create_plan, emulator, error_code, error_message, sleep_past_the_transition,
};

/// A 36-character `RescoreExecutionPlanId` that no create in these tests mints.
const NEVER_CREATED_PLAN_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `Status` `DescribeRescoreExecutionPlan` reports for `id`.
async fn plan_status(router: &MultiServiceRouter, id: &str) -> String {
    let (status, body) = call(router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe {id}: unexpected answer {body}");
    body["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("describe {id}: no Status in {body}"))
        .to_string()
}

/// A minimal `Rescore` request against `plan_id`.
fn rescore_body(plan_id: &str) -> Value {
    json!({
        "RescoreExecutionPlanId": plan_id,
        "SearchQuery": "cats",
        "Documents": [{ "Id": "doc-1", "Title": "cats", "Body": "cats are domestic pets", "OriginalScore": 1.0 }],
    })
}

// ---------------------------------------------------------------------------
// Rule 1: the validation report
// ---------------------------------------------------------------------------

/// One row of the refusal table.
struct RefusalCase {
    /// What this row varies.
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The whole message the report must be.
    expected: String,
    /// A caller-supplied value the message must NOT carry.
    withheld: Option<String>,
}

#[tokio::test]
async fn a_constraint_violation_is_reported_without_the_value_and_without_the_pattern_anchors() {
    let router = emulator();

    // The rows span the constraint classes the model declares on this service's members
    // — `@pattern`, both `@length` bounds, `@required`, both `@range` bounds — over
    // three different operations and four different members, including members the
    // issue did not cite. The requirements come from the model (smithy_query on this
    // crate's `smithy-model/`): `Name` -> RescoreExecutionPlanName @required
    // @length(1,1000) @pattern "^[a-zA-Z0-9][a-zA-Z0-9_-]*$"; `Description` ->
    // Description @length(0,1000) @pattern "^\P{C}*$"; `Id` -> RescoreExecutionPlanId
    // @length(36,36); `MaxResults` -> @range(1,50).
    let name_pattern = "Value at 'name' failed to satisfy constraint: Member must satisfy \
                        regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_-]*";
    let cases = vec![
        RefusalCase {
            case: "a Name whose first character is outside its pattern",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Name": "-krank71b409600f3e" }),
            expected: format!("1 validation error detected: {name_pattern}"),
            withheld: Some("-krank71b409600f3e".to_string()),
        },
        RefusalCase {
            case: "a Name carrying characters outside its pattern",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Name": "bad name!" }),
            expected: format!("1 validation error detected: {name_pattern}"),
            withheld: Some("bad name!".to_string()),
        },
        RefusalCase {
            case: "an empty Name violates both its length minimum and its pattern, so both \
                   are reported",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Name": "" }),
            expected: format!(
                "2 validation errors detected: Value at 'name' failed to satisfy constraint: \
                 Member must have length greater than or equal to 1; {name_pattern}"
            ),
            withheld: None,
        },
        RefusalCase {
            case: "a Name one past its length maximum",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Name": "a".repeat(1001) }),
            expected: "1 validation error detected: Value at 'name' failed to satisfy \
                       constraint: Member must have length less than or equal to 1000"
                .to_string(),
            withheld: Some("a".repeat(1001)),
        },
        RefusalCase {
            case: "an absent required Name has no value to withhold",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Description": "no name" }),
            expected: "1 validation error detected: Value null at 'name' failed to satisfy \
                       constraint: Member must not be null"
                .to_string(),
            withheld: None,
        },
        RefusalCase {
            case: "MaxResults one below its published range",
            operation: "ListRescoreExecutionPlans",
            body: json!({ "MaxResults": 0 }),
            expected: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                       constraint: Member must have value greater than or equal to 1"
                .to_string(),
            withheld: None,
        },
        RefusalCase {
            case: "MaxResults one above its published range",
            operation: "ListRescoreExecutionPlans",
            body: json!({ "MaxResults": 51 }),
            expected: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                       constraint: Member must have value less than or equal to 50"
                .to_string(),
            withheld: None,
        },
        RefusalCase {
            case: "a plan id under the length the model pins",
            operation: "DescribeRescoreExecutionPlan",
            body: json!({ "Id": "too-short" }),
            expected: "1 validation error detected: Value at 'id' failed to satisfy constraint: \
                       Member must have length greater than or equal to 36"
                .to_string(),
            withheld: Some("too-short".to_string()),
        },
        RefusalCase {
            case: "a Description carrying a character outside its pattern",
            operation: "CreateRescoreExecutionPlan",
            body: json!({ "Name": "fine", "Description": "a\u{7}bell" }),
            expected: "1 validation error detected: Value at 'description' failed to satisfy \
                       constraint: Member must satisfy regular expression pattern: \\P{C}*"
                .to_string(),
            withheld: Some("a\u{7}bell".to_string()),
        },
    ];

    for case in cases {
        let (status, answer) = call(&router, case.operation, case.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} on {}: expected HTTP 400, got {status} {answer}",
            case.case, case.operation
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {:?}: expected the modelled error code, got {answer}",
            case.case
        );
        let message = error_message(&answer);
        assert_eq!(
            message, case.expected,
            "case {:?}: expected {:?}, got {message:?}",
            case.case, case.expected
        );
        if let Some(withheld) = &case.withheld {
            assert!(
                !message.contains(withheld.as_str()),
                "case {:?}: the report must not echo the offending value {withheld:?}, \
                 got {message:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_pattern_requirement_never_carries_the_anchors_the_model_spells_it_with() {
    let router = emulator();

    // The same rule, stated as the invariant it is, over every member of this service
    // whose model carries a `@pattern` that a request can violate: whatever expression
    // the report quotes, it starts with neither `^` nor ends with `$`.
    let violating: Vec<(&str, &str, Value)> = vec![
        (
            "Name",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "_leading-underscore" }),
        ),
        (
            "Description",
            "CreateRescoreExecutionPlan",
            json!({ "Name": "fine", "Description": "null\u{0}byte" }),
        ),
        (
            "NextToken",
            "ListRescoreExecutionPlans",
            json!({ "NextToken": "tab\u{1}control" }),
        ),
    ];

    for (member, operation, body) in violating {
        let (status, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "member {member}: expected HTTP 400, got {status} {answer}"
        );
        let message = error_message(&answer);
        let marker = "Member must satisfy regular expression pattern: ";
        let quoted = message
            .rsplit_once(marker)
            .unwrap_or_else(|| {
                panic!("member {member}: expected a pattern requirement, got {message:?}")
            })
            .1;
        assert!(
            !quoted.starts_with('^'),
            "member {member}: the quoted pattern must drop the leading anchor, got {quoted:?}"
        );
        assert!(
            !quoted.ends_with('$'),
            "member {member}: the quoted pattern must drop the trailing anchor, got {quoted:?}"
        );
    }
}

#[tokio::test]
async fn two_violated_members_are_reported_as_two_clauses_under_the_plural_envelope() {
    let router = emulator();
    let (status, answer) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "bad name!", "Description": "a\u{7}bell" }),
    )
    .await;
    assert_eq!(status, 400, "expected HTTP 400, got {status} {answer}");
    let message = error_message(&answer);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "expected the plural envelope, got {message:?}"
    );
    for clause in [
        "Value at 'name' failed to satisfy constraint: Member must satisfy regular expression \
         pattern: [a-zA-Z0-9][a-zA-Z0-9_-]*",
        "Value at 'description' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: \\P{C}*",
    ] {
        assert!(
            message.contains(clause),
            "expected the report to carry {clause:?}, got {message:?}"
        );
    }
    assert!(
        !message.contains("a\u{7}bell") && !message.contains("bad name!"),
        "the report must not echo either offending value, got {message:?}"
    );
}

#[tokio::test]
async fn a_value_at_each_published_bound_is_accepted() {
    let router = emulator();
    create_plan(&router, "bounded").await;

    // Each bound the rows above probe one past: `MaxResults` at 1 and at 50, and a
    // `Name` at each end of its 1..1000 length.
    for max_results in [1, 50] {
        let (status, answer) = call(
            &router,
            "ListRescoreExecutionPlans",
            json!({ "MaxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results}: a value at the bound is accepted, got {answer}"
        );
    }
    for (case, name) in [
        ("a one-character Name", "a".to_string()),
        ("a Name at its 1000-character bound", "n".repeat(1000)),
    ] {
        let (status, answer) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({ "Name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value at the bound is accepted, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the constant not-found message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_that_names_no_plan_is_answered_with_the_constant_not_found_message() {
    let router = emulator();

    // Three distinct ids, from the two ways an id can name no plan: one that was never
    // created, and two whose plans were created and then deleted.
    let mut unknown = vec![NEVER_CREATED_PLAN_ID.to_string()];
    let mut to_delete = Vec::new();
    for name in ["deleted-one", "deleted-two"] {
        to_delete.push(create_plan(&router, name).await);
    }
    // A live plan beside them, so the refusal is about the id and not about an empty
    // store.
    create_plan(&router, "live-plan").await;
    // Every plan is ACTIVE before the deletes: a plan mid-provisioning cannot be
    // deleted, and the live plan has to be able to serve `Rescore`.
    sleep_past_the_transition();
    for id in to_delete {
        let (status, answer) =
            call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": id })).await;
        assert_eq!(status, 200, "delete {id}: got {answer}");
        unknown.push(id);
    }

    let mut seen: Vec<String> = Vec::new();
    for id in &unknown {
        let lookups: Vec<(&str, Value)> = vec![
            ("DescribeRescoreExecutionPlan", json!({ "Id": id })),
            ("DeleteRescoreExecutionPlan", json!({ "Id": id })),
            (
                "UpdateRescoreExecutionPlan",
                json!({ "Id": id, "Name": "no-such-plan" }),
            ),
            ("Rescore", rescore_body(id)),
        ];
        for (operation, body) in lookups {
            let case = format!("{operation} on {id}");
            let (status, answer) = call(&router, operation, body).await;
            // Every client-side failure of this RPC service carries the protocol's
            // client-error status; the error TYPE in the body names the failure.
            assert_eq!(status, 400, "case {case:?}: expected HTTP 400, got {answer}");
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code, got {answer}"
            );
            let message = error_message(&answer);
            assert_eq!(
                message,
                state::PLAN_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {:?}, got {message:?}",
                state::PLAN_NOT_FOUND_MESSAGE
            );
            assert!(
                !message.contains(id.as_str()),
                "case {case:?}: the message must not carry the requested id, got {message:?}"
            );
            seen.push(message);
        }
    }

    // The whole point of the rule: one string for every id and every operation.
    seen.dedup();
    assert_eq!(
        seen.len(),
        1,
        "the not-found message must be request-independent, got {seen:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the transient CREATING status, and the data-plane gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_plan_is_creating_until_the_clock_owes_it_active() {
    let router = emulator();

    // The rule generalises over the payload a plan was created with, not just its name.
    let cases: Vec<(&str, Value)> = vec![
        ("name only", json!({ "Name": "plain-plan" })),
        (
            "every optional member",
            json!({
                "Name": "full-plan",
                "Description": "a described plan",
                "CapacityUnits": { "RescoreCapacityUnits": 3 },
                "Tags": [{ "Key": "team", "Value": "search" }],
                "ClientToken": "tok-creating",
            }),
        ),
        (
            "capacity units at the default of zero",
            json!({ "Name": "zero-capacity", "CapacityUnits": { "RescoreCapacityUnits": 0 } }),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, body) in cases {
        let (status, answer) = call(&router, "CreateRescoreExecutionPlan", body).await;
        assert_eq!(status, 200, "case {case:?}: create got {answer}");
        let id = answer["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {answer}"))
            .to_string();

        // A caller acting straight after create still sees the transient status, twice
        // over: reading it does not move it.
        for read in 1..=2 {
            let status = plan_status(&router, &id).await;
            assert_eq!(
                status,
                state::STATUS_CREATING,
                "case {case:?} (read {read}): a new plan is CREATING, got {status:?}"
            );
        }

        // And the listing agrees with the describe.
        let (_, listed) = call(&router, "ListRescoreExecutionPlans", json!({})).await;
        let summary = listed["SummaryItems"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|item| item["Id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the plan must be listed, got {listed}"));
        assert_eq!(
            summary["Status"],
            json!(state::STATUS_CREATING),
            "case {case:?}: a listing must report the same status as a describe, got {summary}"
        );

        created.push((case, id));
    }

    // Nothing slept for the transition: once it is due, every plan is ACTIVE.
    sleep_past_the_transition();
    for (case, id) in &created {
        let status = plan_status(&router, id).await;
        assert_eq!(
            status,
            state::STATUS_ACTIVE,
            "case {case:?}: the clock owes CREATING -> ACTIVE, got {status:?}"
        );
        // ACTIVE is terminal here: it does not drift back on a later read.
        let again = plan_status(&router, id).await;
        assert_eq!(
            again,
            state::STATUS_ACTIVE,
            "case {case:?}: a settled plan stays ACTIVE, got {again:?}"
        );
    }
}

#[tokio::test]
async fn rescore_is_refused_while_the_plan_is_not_active_and_served_once_it_is() {
    let router = emulator();
    let plan = create_plan(&router, "gated-plan").await;
    assert_eq!(
        plan_status(&router, &plan).await,
        state::STATUS_CREATING,
        "the gate is only meaningful while the plan is CREATING"
    );

    // Every shape of a valid Rescore request is refused while the plan is not ACTIVE:
    // the refusal is about the plan's status, not about the batch.
    let bodies: Vec<(&str, Value)> = vec![
        ("one document", rescore_body(&plan)),
        (
            "several documents, mixed fields",
            json!({
                "RescoreExecutionPlanId": plan,
                "SearchQuery": "intelligent ranking",
                "Documents": [
                    { "Id": "d1", "OriginalScore": 0.4, "Title": "intelligent ranking", "Body": "a note" },
                    { "Id": "d2", "OriginalScore": 0.9, "TokenizedBody": ["ranking"] },
                ],
            }),
        ),
    ];
    for (case, body) in &bodies {
        let (status, answer) = call(&router, "Rescore", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a plan that is not ACTIVE must refuse Rescore, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: unexpected error code, got {answer}"
        );
        assert_eq!(
            error_message(&answer),
            state::PLAN_NOT_ACTIVE_MESSAGE,
            "case {case:?}: unexpected message, got {answer}"
        );
    }

    // A refused Rescore changed nothing: the plan is still there and still CREATING.
    assert_eq!(
        plan_status(&router, &plan).await,
        state::STATUS_CREATING,
        "a refused Rescore must not move the plan's status"
    );

    // An id that names no plan is NOT-FOUND even while another plan is CREATING: the
    // lookup is answered before the status gate.
    let (status, answer) = call(&router, "Rescore", rescore_body(NEVER_CREATED_PLAN_ID)).await;
    assert_eq!(
        status, 400,
        "an unknown id is not-found, not a status refusal, got {status} {answer}"
    );

    // Once the transition is due, the same requests are served.
    sleep_past_the_transition();
    assert_eq!(
        plan_status(&router, &plan).await,
        state::STATUS_ACTIVE,
        "the plan must reach ACTIVE"
    );
    for (case, body) in &bodies {
        let (status, answer) = call(&router, "Rescore", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an ACTIVE plan serves Rescore, got {status} {answer}"
        );
        let items = answer["ResultItems"].as_array().cloned().unwrap_or_default();
        let submitted = body["Documents"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            submitted.len(),
            "case {case:?}: one result item per submitted document, got {answer}"
        );
    }
}

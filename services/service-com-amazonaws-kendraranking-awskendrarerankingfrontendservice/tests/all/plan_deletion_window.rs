// SPDX-License-Identifier: CC0-1.0

//! The asynchronous teardown a `DeleteRescoreExecutionPlan` call starts, end to end
//! over the wire.
//!
//! The RULE every suite here asserts: a delete of a plan ready to serve starts the
//! teardown and returns at once; for the teardown span the plan still exists and every
//! reader reports its transient `DELETING` status, every call that mutates or serves it
//! is refused as the other transient statuses are, and only once the clock has finished
//! the teardown does the id stop resolving — the plan is listed no more, its ARN names
//! nothing and the `ClientToken` that minted it mints a new plan again.
//!
//! The rows differ in what the rule generalises over — the payload the plan was created
//! with, the region it lives in, whether it was updated first, and the operation driven
//! against it — rather than in a name: the recorded case is one row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{
    call, call_in, emulator, error_code, error_message, list_plan_summaries,
    sleep_past_the_transition,
};

/// One plan the delete rule is asserted over: the create payload and the region the
/// plan lives in.
struct PlanCase {
    /// What this row varies.
    case: &'static str,
    /// The region the create is signed for, so the rule is asserted beyond the
    /// emulator's default region.
    region: &'static str,
    body: Value,
}

/// The plans the teardown rule holds for, one row per class of plan rather than per
/// name: the rule is about the delete, so the rows vary the payload the plan carries
/// and the region it lives in.
fn plan_cases() -> Vec<PlanCase> {
    vec![
        PlanCase {
            case: "name only",
            region: "us-east-1",
            body: json!({ "Name": "doomed-plain" }),
        },
        PlanCase {
            case: "every optional member",
            region: "eu-west-1",
            body: json!({
                "Name": "doomed-full",
                "Description": "a described plan",
                "CapacityUnits": { "RescoreCapacityUnits": 4 },
                "Tags": [{ "Key": "team", "Value": "search" }],
                "ClientToken": "tok-doomed",
            }),
        },
        PlanCase {
            case: "capacity units at the default of zero",
            region: "ap-southeast-2",
            body: json!({ "Name": "doomed-zero", "CapacityUnits": { "RescoreCapacityUnits": 0 } }),
        },
    ]
}

/// Create the plan `case` describes and wait out its `CREATING` span, so the delete
/// below is about the delete and not about a plan mid-provisioning. Answers its
/// `(Id, Arn)`.
async fn create_active_plan_for(router: &MultiServiceRouter, case: &PlanCase) -> (String, String) {
    let (status, created) = call_in(
        router,
        case.region,
        "CreateRescoreExecutionPlan",
        case.body.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: create got {status} {created}",
        case.case
    );
    sleep_past_the_transition();
    (
        created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Id in {created}", case.case))
            .to_string(),
        created["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Arn in {created}", case.case))
            .to_string(),
    )
}

/// The `Status` `DescribeRescoreExecutionPlan` reports for `id` in `region`, with the
/// HTTP status the read answered.
async fn described_status(router: &MultiServiceRouter, region: &str, id: &str) -> (u16, String) {
    let (status, described) = call_in(
        router,
        region,
        "DescribeRescoreExecutionPlan",
        json!({ "Id": id }),
    )
    .await;
    (
        status,
        described["Status"].as_str().unwrap_or_default().to_string(),
    )
}

/// The `Status` `ListRescoreExecutionPlans` reports for `id` in `region`, or `None` when
/// the listing does not hold it at all.
async fn listed_status(router: &MultiServiceRouter, region: &str, id: &str) -> Option<String> {
    let (status, listed) = call_in(router, region, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list in {region}: got {status} {listed}");
    listed["SummaryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|item| item["Id"].as_str() == Some(id))
        .map(|item| item["Status"].as_str().unwrap_or_default().to_string())
}

#[tokio::test]
async fn a_deleted_plan_reports_deleting_until_the_clock_finishes_the_teardown() {
    for case in plan_cases() {
        let router = emulator();
        let name = case.case;
        let (id, _) = create_active_plan_for(&router, &case).await;

        let (status, deleted) = call_in(
            &router,
            case.region,
            "DeleteRescoreExecutionPlan",
            json!({ "Id": id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the delete of an ACTIVE plan is accepted, got {status} {deleted}"
        );

        // The teardown is asynchronous, so the plan is still there, in the transient
        // status the model names — read twice over, because a read must not move it.
        for read in 1..=2 {
            let (status, reported) = described_status(&router, case.region, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?} (read {read}): a plan mid-teardown is still described, \
                 got {status} with status {reported:?}"
            );
            assert_eq!(
                reported,
                state::STATUS_DELETING,
                "case {name:?} (read {read}): expected {}, got {reported:?}",
                state::STATUS_DELETING
            );
            let listed = listed_status(&router, case.region, &id).await;
            assert_eq!(
                listed.as_deref(),
                Some(state::STATUS_DELETING),
                "case {name:?} (read {read}): the listing must report the same status as \
                 the describe, got {listed:?}"
            );
        }

        // And once the clock has finished the teardown, the id resolves to nothing —
        // with the service's one not-found sentence — and the plan is listed no more.
        sleep_past_the_transition();
        let (status, described) = call_in(
            &router,
            case.region,
            "DescribeRescoreExecutionPlan",
            json!({ "Id": id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: after the teardown the id names no plan, got {status} {described}"
        );
        assert_eq!(
            error_code(&described),
            "ResourceNotFoundException",
            "case {name:?}: after the teardown, got {described}"
        );
        assert_eq!(
            error_message(&described),
            state::PLAN_NOT_FOUND_MESSAGE,
            "case {name:?}: after the teardown, got {described}"
        );
        let listed = listed_status(&router, case.region, &id).await;
        assert_eq!(
            listed, None,
            "case {name:?}: after the teardown the plan is listed no more, got {listed:?}"
        );
    }
}

/// One operation driven against a plan that is mid-teardown, with the modelled error it
/// is refused with.
struct RefusalCase {
    case: &'static str,
    operation: &'static str,
    /// The request body, without its `Id` — the suite fills in the plan's own.
    body: Value,
    expected_code: &'static str,
}

#[tokio::test]
async fn every_call_that_mutates_or_serves_a_plan_is_refused_while_it_is_deleting() {
    // The rule spans every operation that needs the plan to be ACTIVE, not only the
    // repeated delete: a plan on its way out takes no further change and serves no
    // ranking, exactly as one mid-provisioning does not.
    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a second delete inside the window",
            operation: "DeleteRescoreExecutionPlan",
            body: json!({}),
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "an update renaming it",
            operation: "UpdateRescoreExecutionPlan",
            body: json!({ "Name": "renamed-mid-teardown" }),
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "an update carrying only capacity",
            operation: "UpdateRescoreExecutionPlan",
            body: json!({ "CapacityUnits": { "RescoreCapacityUnits": 7 } }),
            expected_code: "ConflictException",
        },
        RefusalCase {
            case: "an update carrying no member beyond the id",
            operation: "UpdateRescoreExecutionPlan",
            body: json!({}),
            expected_code: "ConflictException",
        },
    ];

    for refusal in cases {
        let router = emulator();
        let case = refusal.case;
        let plan = PlanCase {
            case,
            region: "us-east-1",
            body: json!({ "Name": "refusal-plan" }),
        };
        let (id, _) = create_active_plan_for(&router, &plan).await;
        let (status, deleted) =
            call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": id })).await;
        assert_eq!(status, 200, "case {case:?}: the delete: got {status} {deleted}");

        let mut body = refusal.body.clone();
        body["Id"] = json!(id);
        let (status, refused) = call(&router, refusal.operation, body).await;
        // Every client-side failure of this RPC service travels with the protocol's
        // client-error status; the error TYPE in the body names the failure.
        assert_eq!(
            status, 400,
            "case {case:?} on {}: expected 400, got {status} {refused}",
            refusal.operation
        );
        assert_eq!(
            error_code(&refused),
            refusal.expected_code,
            "case {case:?} on {}: expected {}, got {refused}",
            refusal.operation,
            refusal.expected_code
        );

        // The refusal left the plan as it was: still there, still DELETING.
        let (status, reported) = described_status(&router, "us-east-1", &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused call must not remove the plan, got {status}"
        );
        assert_eq!(
            reported,
            state::STATUS_DELETING,
            "case {case:?}: a refused call must not move the status, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_plan_mid_teardown_serves_no_ranking() {
    let router = emulator();
    let plan = PlanCase {
        case: "a rescore inside the window",
        region: "us-east-1",
        body: json!({ "Name": "rescore-doomed" }),
    };
    let (id, _) = create_active_plan_for(&router, &plan).await;
    let (status, deleted) = call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "the delete: got {status} {deleted}");

    // A plan that is not ACTIVE cannot serve the data plane ("When the value is ACTIVE,
    // the rescore execution plan is ready for use."), and a plan being torn down is one
    // of those: the same refusal a plan mid-provisioning gets, not a served ranking.
    let (status, refused) = call(
        &router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": id,
            "SearchQuery": "anything",
            "Documents": [{ "Id": "d1", "OriginalScore": 0.5, "Body": "a note" }],
        }),
    )
    .await;
    assert_eq!(status, 400, "a rescore mid-teardown: got {status} {refused}");
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a rescore mid-teardown: got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        state::PLAN_NOT_ACTIVE_MESSAGE,
        "a rescore mid-teardown: got {refused}"
    );

    // And once the teardown is finished the id names no plan at all, so the data plane
    // answers the not-found refusal instead.
    sleep_past_the_transition();
    let (status, gone) = call(
        &router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": id,
            "SearchQuery": "anything",
            "Documents": [{ "Id": "d1", "OriginalScore": 0.5, "Body": "a note" }],
        }),
    )
    .await;
    assert_eq!(status, 400, "a rescore after the teardown: got {status} {gone}");
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "a rescore after the teardown: got {gone}"
    );
}

#[tokio::test]
async fn a_plan_mid_teardown_still_carries_its_tags_and_loses_them_with_the_teardown() {
    let router = emulator();
    let plan = PlanCase {
        case: "a tagged plan",
        region: "us-east-1",
        body: json!({ "Name": "tagged-doomed", "Tags": [{ "Key": "team", "Value": "search" }] }),
    };
    let (id, arn) = create_active_plan_for(&router, &plan).await;
    let (status, deleted) = call(&router, "DeleteRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "the delete: got {status} {deleted}");

    // The plan still exists while it is being torn down, so its ARN still names a
    // taggable resource.
    let (status, tags) = call(&router, "ListTagsForResource", json!({ "ResourceARN": arn })).await;
    assert_eq!(
        status, 200,
        "list tags mid-teardown: the plan still exists, got {status} {tags}"
    );
    assert_eq!(
        tags["Tags"],
        json!([{ "Key": "team", "Value": "search" }]),
        "list tags mid-teardown: got {tags}"
    );

    // Once the teardown is finished the ARN names nothing, with the service's one
    // unavailable sentence — the same one every ARN-addressed operation states.
    sleep_past_the_transition();
    for operation in ["ListTagsForResource", "TagResource", "UntagResource"] {
        let body = match operation {
            "TagResource" => {
                json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": "v" }] })
            }
            "UntagResource" => json!({ "ResourceARN": arn, "TagKeys": ["team"] }),
            _ => json!({ "ResourceARN": arn }),
        };
        let (status, refused) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation} after the teardown: got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceUnavailableException",
            "case {operation} after the teardown: got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            state::resource_unavailable_message(&arn),
            "case {operation} after the teardown: got {refused}"
        );
    }
}

#[tokio::test]
async fn the_finished_teardown_frees_the_client_token_the_plan_was_created_with() {
    let router = emulator();
    let token = "tok-reused-after-teardown";
    let (status, first) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "token-plan", "ClientToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the first create: got {status} {first}");
    let first_id = first["Id"].as_str().unwrap_or_default().to_string();
    sleep_past_the_transition();

    let (status, deleted) = call(
        &router,
        "DeleteRescoreExecutionPlan",
        json!({ "Id": first_id }),
    )
    .await;
    assert_eq!(status, 200, "the delete: got {status} {deleted}");

    // Inside the window the plan the token created is still there, so the token still
    // names it: a repeated create is still the idempotent answer.
    let (status, repeated) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "token-plan-again", "ClientToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the repeated create: got {status} {repeated}");
    assert_eq!(
        repeated["Id"],
        json!(first_id),
        "inside the window the token still names the plan it created, got {repeated}"
    );

    // Once the teardown is finished the token names no plan, so the same token mints a
    // new one — and the store holds exactly that one plan.
    sleep_past_the_transition();
    let (status, minted) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({ "Name": "token-plan-fresh", "ClientToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the create after the teardown: got {status} {minted}");
    let minted_id = minted["Id"].as_str().unwrap_or_default().to_string();
    assert_ne!(
        minted_id, first_id,
        "after the teardown the token must mint a new plan, got {minted}"
    );
    let listed: Vec<String> = list_plan_summaries(&router)
        .await
        .iter()
        .map(|item| item["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![minted_id],
        "only the plan minted after the teardown is held, got {listed:?}"
    );
}

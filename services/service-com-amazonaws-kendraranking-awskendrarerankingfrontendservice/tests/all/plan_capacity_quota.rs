// SPDX-License-Identifier: CC0-1.0

//! The published ceiling on the extra capacity a rescore execution plan may be
//! provisioned with, end to end over the wire.
//!
//! The rule under test: a quota the service PUBLISHES but the model cannot express is
//! enforced by the service, so a request asking for more than the published maximum is
//! refused with the modelled quota error that operation declares and the stored resource
//! is left exactly as it was. Here the quota is
//! `CapacityUnitsConfiguration$RescoreCapacityUnits` (smithy_query: `@range {min: 0}` and
//! NO maximum, documented as "A single extra capacity unit for a rescore execution plan
//! provides 0.01 rescore requests per second. You can add up to 1000 extra capacity
//! units."), the refusal is `ServiceQuotaExceededException` — declared by both
//! `CreateRescoreExecutionPlan` and `UpdateRescoreExecutionPlan`, the only two operations
//! that take a `CapacityUnitsConfiguration` — and the ceiling is
//! `state::MAX_RESCORE_CAPACITY_UNITS`.
//!
//! The table spans the published range by its BOUNDS: the entry below the ceiling, the
//! ceiling itself, the first value past it, and values far past it including the top of
//! the integer the member is modelled as. Each row is driven through the create and
//! through the update, and every refusal is checked to have stored nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::ops::types::CapacityUnitsConfiguration;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::scaffolded;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, emulator, error_code, error_message, sleep_past_the_transition};

/// The error type a request past a published quota must be refused with, as both
/// operations declare it.
const QUOTA_ERROR: &str = "ServiceQuotaExceededException";

/// One row of the capacity table: the amount of extra capacity asked for, and whether
/// the service may provision it.
struct CapacityCase {
    case: &'static str,
    requested: i32,
    /// `true` where the amount is within the published ceiling, so it is stored.
    within_quota: bool,
}

/// Every row, built around the published ceiling so the bounds move with the constant.
fn cases() -> Vec<CapacityCase> {
    vec![
        CapacityCase {
            case: "the bottom of the published range",
            requested: 0,
            within_quota: true,
        },
        CapacityCase {
            case: "one unit below the published ceiling",
            requested: state::MAX_RESCORE_CAPACITY_UNITS - 1,
            within_quota: true,
        },
        CapacityCase {
            case: "exactly the published ceiling",
            requested: state::MAX_RESCORE_CAPACITY_UNITS,
            within_quota: true,
        },
        CapacityCase {
            case: "one unit past the published ceiling",
            requested: state::MAX_RESCORE_CAPACITY_UNITS + 1,
            within_quota: false,
        },
        CapacityCase {
            case: "several times the published ceiling",
            requested: state::MAX_RESCORE_CAPACITY_UNITS * 5,
            within_quota: false,
        },
        CapacityCase {
            case: "the top of the integer the member is modelled as",
            requested: i32::MAX,
            within_quota: false,
        },
    ]
}

/// Describe `id`, asserting the call succeeded, and return the body.
async fn describe(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, described) =
        call(router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe {id}: got {status} {described}");
    described
}

/// The `CapacityUnits` a describe reported, exactly as the body carries it:
/// `Value::Null` when the body has no such key at all.
fn reported_capacity(described: &Value) -> Value {
    described
        .get("CapacityUnits")
        .cloned()
        .unwrap_or(Value::Null)
}

/// How many plans the store holds right now.
async fn plan_count(router: &MultiServiceRouter) -> usize {
    let (status, listed) = call(router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list: got {status} {listed}");
    listed["SummaryItems"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Assert `answer` is this service's modelled quota refusal of `requested` units and
/// says something.
fn assert_quota_refusal(case: &str, requested: i32, status: u16, answer: &Value) {
    assert_eq!(
        status,
        scaffolded::CLIENT_ERROR_STATUS,
        "case {case:?}: a capacity past the published ceiling must be refused with the \
         protocol's client-error status, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        QUOTA_ERROR,
        "case {case:?}: expected the error type {QUOTA_ERROR}, got {status} {answer}"
    );
    assert!(
        !error_message(answer).is_empty(),
        "case {case:?}: the refusal must still state what was wrong, got {answer}"
    );
    // The status and the error code above are the wire requirements; the sentence is
    // not, so only the ceiling it must name is asserted — never its exact wording.
    let message = error_message(answer);
    assert!(
        message.contains(&state::MAX_RESCORE_CAPACITY_UNITS.to_string()),
        "case {case:?}: the refusal of {requested} units must name the ceiling that was \
         exceeded, got {message:?}"
    );
}

#[tokio::test]
async fn a_create_past_the_published_capacity_ceiling_is_refused_and_stores_nothing() {
    let router = emulator();
    let mut stored = 0usize;

    for (index, case) in cases().iter().enumerate() {
        let name = format!("capacity-quota-create-{index}");
        let request = json!({
            "Name": name,
            "CapacityUnits": { "RescoreCapacityUnits": case.requested },
        });
        let (status, created) = call(&router, "CreateRescoreExecutionPlan", request).await;

        if case.within_quota {
            assert_eq!(
                status, 200,
                "case {:?}: a capacity within the published ceiling must be provisioned, \
                 got {status} {created}",
                case.case
            );
            stored += 1;
            let id = created["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no Id in {created}", case.case))
                .to_string();
            let described = describe(&router, &id).await;
            let actual = reported_capacity(&described);
            let expected = json!({ "RescoreCapacityUnits": case.requested });
            assert_eq!(
                actual, expected,
                "case {:?}: expected CapacityUnits {expected}, got {actual:?} in {described}",
                case.case
            );
        } else {
            assert_quota_refusal(case.case, case.requested, status, &created);
        }

        let count = plan_count(&router).await;
        assert_eq!(
            count, stored,
            "case {:?}: expected {stored} plan(s) in the store, got {count}",
            case.case
        );
    }
}

#[tokio::test]
async fn an_update_past_the_published_capacity_ceiling_leaves_the_plan_untouched() {
    let router = emulator();
    // The capacity every plan in this suite starts out provisioned with, so a refused
    // update is visible as the value that did NOT move.
    let initial = 7;
    // A name the refused update asks for, so the refusal is checked to have dropped the
    // whole request and not only its capacity.
    let rejected_name_suffix = "-renamed";

    for (index, case) in cases().iter().enumerate() {
        let name = format!("capacity-quota-update-{index}");
        let (status, created) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({
                "Name": name,
                "CapacityUnits": { "RescoreCapacityUnits": initial },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the plan under test must be created, got {status} {created}",
            case.case
        );
        let id = created["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Id in {created}", case.case))
            .to_string();
        // An update is only accepted while the plan is ACTIVE.
        sleep_past_the_transition();

        let (status, updated) = call(
            &router,
            "UpdateRescoreExecutionPlan",
            json!({
                "Id": id,
                "Name": format!("{name}{rejected_name_suffix}"),
                "CapacityUnits": { "RescoreCapacityUnits": case.requested },
            }),
        )
        .await;

        let expected_capacity = if case.within_quota {
            assert_eq!(
                status, 200,
                "case {:?}: a capacity within the published ceiling must be provisioned, \
                 got {status} {updated}",
                case.case
            );
            case.requested
        } else {
            assert_quota_refusal(case.case, case.requested, status, &updated);
            initial
        };

        let described = describe(&router, &id).await;
        let actual = reported_capacity(&described);
        let expected = json!({ "RescoreCapacityUnits": expected_capacity });
        assert_eq!(
            actual, expected,
            "case {:?}: expected CapacityUnits {expected}, got {actual:?} in {described}",
            case.case
        );
        let reported_name = described["Name"].as_str().unwrap_or_default();
        let expected_name = if case.within_quota {
            format!("{name}{rejected_name_suffix}")
        } else {
            name.clone()
        };
        assert_eq!(
            reported_name, expected_name,
            "case {:?}: expected the plan to be named {expected_name:?}, got \
             {reported_name:?} in {described}",
            case.case
        );
    }
}

#[test]
fn the_capacity_ceiling_is_the_published_one() {
    // The published number, from the documentation of
    // `CapacityUnitsConfiguration$RescoreCapacityUnits` (smithy_query query="doc"):
    // "You can add up to 1000 extra capacity units."
    assert_eq!(
        state::MAX_RESCORE_CAPACITY_UNITS, 1000,
        "the enforced ceiling must be the published one, got {}",
        state::MAX_RESCORE_CAPACITY_UNITS
    );

    // The decision itself, over each bound of the published range and one past it, so
    // the refusal is a function of the amount and nothing else.
    let cases: Vec<(i32, bool)> = vec![
        (0, false),
        (1, false),
        (state::MAX_RESCORE_CAPACITY_UNITS - 1, false),
        (state::MAX_RESCORE_CAPACITY_UNITS, false),
        (state::MAX_RESCORE_CAPACITY_UNITS + 1, true),
        (i32::MAX, true),
    ];
    for (requested, refused) in cases {
        let configuration = CapacityUnitsConfiguration {
            rescore_capacity_units: requested,
        };
        let actual = state::capacity_units_over_quota(&configuration).is_some();
        assert_eq!(
            actual, refused,
            "case {requested} extra capacity units: expected refused={refused}, got \
             refused={actual}"
        );
    }
}

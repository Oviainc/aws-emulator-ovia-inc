// SPDX-License-Identifier: CC0-1.0

//! `ScalingPlanVersion` as a SELECTOR of the plan a request addresses, on the operations
//! that identify ONE plan by (`ScalingPlanName`, `ScalingPlanVersion`).
//!
//! One rule is pinned here, over a TABLE of versions rather than the one request that
//! exposed it: a request member the model constrains to no set of values selects the
//! resource a request addresses, so a coordinate that selects no stored resource is a MISS
//! — reported as the empty collection by the operations that bind no not-found error, and
//! refused with the modelled not-found shape only by those that do bind one. The boundary
//! is pinned beside it: the two MUTATING operations bind `ObjectNotFoundException` and
//! refuse the very same coordinate with AWS's request-derived not-found sentence.
//!
//! Rows differ along the version's MAGNITUDE and SIGN across the whole `long` the member
//! is modelled as, along whether the plan NAME beside it is one the service holds, along
//! which operation carries the member, and along whether the request also asks for a page
//! size.
//!
//! The collection read's copy of the member is pinned in
//! [`crate::describe_scaling_plans_version_member`], and what the member is still refused
//! for in [`crate::plan_version_member_validation`].
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`], the version table from
//! [`crate::plan_version_member_validation`] and the two read requests from
//! [`crate::absent_plan_reads::read_request`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::ONLY_VERSION;

use crate::absent_plan_reads::{read_request, READS};
use crate::plan_version_member_validation::unheld_versions;
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan the emulator holds in these tests, so a row probes the VERSION half of the
/// coordinate rather than the name half.
const STORED_PLAN: &str = "held plan";

/// A plan name this service does not hold, so a row probes both halves at once.
const ABSENT_PLAN: &str = "sim-probe-absent";

/// The Auto Scaling group [`simple_instructions`] instructs, and the one resource the
/// stored plan reports at the version it carries.
const RESOURCE_ID: &str = "autoScalingGroup/my-asg";

/// An emulator holding one settled plan with one scaling instruction.
async fn with_one_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": STORED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The not-found sentence AWS builds from the requested name and version.
fn expected_not_found(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

#[tokio::test]
async fn a_read_at_a_version_no_plan_carries_reports_its_collection_empty() {
    let emulator = with_one_stored_plan().await;

    for (label, version) in unheld_versions() {
        for plan in [STORED_PLAN, ABSENT_PLAN] {
            for (operation, member) in READS {
                for paged in [false, true] {
                    let mut request = read_request(operation, plan, version);
                    if paged {
                        if operation != "DescribeScalingPlanResources" {
                            continue;
                        }
                        request["MaxResults"] = json!(1);
                    }
                    let case = format!("{operation}/{label} ({version}) on {plan}, paged {paged}");
                    let (status, body) = emulator.call(operation, &request).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected HTTP 200, got {status} with {body}"
                    );
                    assert_eq!(
                        body[member],
                        json!([]),
                        "case {case:?}: expected an empty {member} list, got {body}"
                    );
                    assert!(
                        body.get("NextToken").is_none_or(Value::is_null),
                        "case {case:?}: an empty answer continues nowhere, got {body}"
                    );
                }
            }
        }
    }

    // The control, at the version the stored plan carries: the held plan is served what it
    // holds, so the empty answers above come from the coordinate missing and not from the
    // operations.
    let described = emulator
        .ok(
            "DescribeScalingPlanResources",
            read_request("DescribeScalingPlanResources", STORED_PLAN, ONLY_VERSION),
        )
        .await;
    assert_eq!(
        described["ScalingPlanResources"]
            .as_array()
            .map(|resources| resources.len()),
        Some(1),
        "case the stored version on the held plan: the stored plan instructs one resource, \
         got {described}"
    );
    assert_eq!(
        described["ScalingPlanResources"][0]["ResourceId"],
        json!(RESOURCE_ID),
        "case the stored version on the held plan: the reported resource must be the \
         instructed one, got {described}"
    );
}

#[tokio::test]
async fn the_operations_that_model_a_not_found_shape_refuse_the_same_coordinate() {
    let emulator = with_one_stored_plan().await;

    // The boundary of the rule above: `DeleteScalingPlan` and `UpdateScalingPlan` bind
    // `ObjectNotFoundException`
    // (`smithy_query(query="operation",operation="DeleteScalingPlan,UpdateScalingPlan")`),
    // so the coordinate the reads report as empty is a refusal there — in that shape, with
    // the name and the version taken from the request.
    for (label, version) in unheld_versions() {
        for name in [STORED_PLAN, ABSENT_PLAN] {
            for operation in ["DeleteScalingPlan", "UpdateScalingPlan"] {
                let request = json!({ "ScalingPlanName": name, "ScalingPlanVersion": version });
                let case = format!("{operation}/{label} ({version}) on {name}");
                let (status, body) = emulator.call(operation, &request).await;
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "ObjectNotFoundException"),
                    "case {case:?}: expected a 400 ObjectNotFoundException, got {status} with \
                     {body}"
                );
                let expected = expected_not_found(name, version);
                let actual = error_message(&body);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }

    // And the plan that IS stored, at the version it carries, is not refused: the writes
    // refuse the coordinate that selects nothing, not the member.
    emulator
        .ok(
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": STORED_PLAN, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
}

#[tokio::test]
async fn a_read_that_selects_nothing_changes_nothing_a_later_caller_sees() {
    let emulator = with_one_stored_plan().await;
    let before = emulator.ok("DescribeScalingPlans", json!({})).await;

    for (label, version) in unheld_versions() {
        for (operation, member) in READS {
            let request = read_request(operation, STORED_PLAN, version);
            let first = emulator.ok(operation, request.clone()).await;
            let again = emulator.ok(operation, request).await;
            assert_eq!(
                first[member], again[member],
                "case {operation}/{label} ({version}): two reads of the same miss must agree, \
                 got {first} then {again}"
            );
        }
    }

    let after = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        after["ScalingPlans"], before["ScalingPlans"],
        "case the reads above: the stored plans must be as they were, got {after} after \
         {before}"
    );
    let described = emulator
        .ok(
            "DescribeScalingPlanResources",
            read_request("DescribeScalingPlanResources", STORED_PLAN, ONLY_VERSION),
        )
        .await;
    assert_eq!(
        described["ScalingPlanResources"]
            .as_array()
            .map(|resources| resources.len()),
        Some(1),
        "case after the reads above: the held plan must still report its one resource, got \
         {described}"
    );
}

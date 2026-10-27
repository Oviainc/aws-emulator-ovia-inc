// SPDX-License-Identifier: CC0-1.0

//! The `ScalingPlanVersion` member on the operations that identify ONE plan by it and
//! model no not-found error.
//!
//! One rule is pinned here, over a TABLE of versions rather than the one request that
//! exposed it: a request member the model documents as having exactly one valid value is a
//! request-level constraint, so a value other than that one is refused with the modelled
//! `ValidationException` (HTTP 400) naming the member, the value and the only valid value,
//! before any lookup — never answered as a lookup that quietly matched nothing. The
//! boundary of the rule is pinned too: the two MUTATING operations bind
//! `ObjectNotFoundException` and keep answering a lookup that finds no plan in that shape.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`], and the two read requests are built by
//! [`crate::absent_plan_reads::read_request`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::ONLY_VERSION;

use crate::absent_plan_reads::{read_request, READS};
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan the emulator holds in these tests, so a row probes the VERSION member rather
/// than the name half of the lookup.
const STORED_PLAN: &str = "held plan";

/// A plan name this service does not hold, so a row shows the member is refused before any
/// lookup runs.
const ABSENT_PLAN: &str = "sim-probe-absent";

/// The Auto Scaling group [`simple_instructions`] instructs, and the one resource the
/// stored plan reports.
const RESOURCE_ID: &str = "autoScalingGroup/my-asg";

/// Versions other than the single documented valid value.
///
/// `ScalingPlanVersion` is a `long`
/// (`smithy_query(query="operation",operation="DescribeScalingPlanResources")` — member
/// `ScalingPlanVersion`, type `long`) documented as "The version number of the scaling
/// plan. Currently, the only valid value is 1", so the rows span that type's MAGNITUDE and
/// SIGN: the version below the documented one, the one above it, a negative version,
/// versions past 32 and past 53 bits, and both ends of the type.
///
/// `pub(crate)` so every suite driving this member spans the same values — including the
/// collection read in [`crate::describe_scaling_plans_version_member`].
pub(crate) fn rejected_versions() -> [(&'static str, i64); 7] {
    [
        ("version zero", ONLY_VERSION - 1),
        ("the next version up", ONLY_VERSION + 1),
        ("a negative version", -7),
        ("a version past 32 bits", 4_294_967_296),
        ("a version past 53 bits", 9_007_199_254_740_993),
        ("the smallest version the type carries", i64::MIN),
        ("the largest version the type carries", i64::MAX),
    ]
}

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

#[tokio::test]
async fn a_version_other_than_the_only_documented_one_is_refused_on_the_member() {
    let emulator = with_one_stored_plan().await;

    // Each rejected version is driven against a plan the emulator HOLDS and a name it does
    // not: the version is a member of the request, so the refusal must not depend on what
    // is stored. The third column adds a page size where the operation takes one, so the
    // refusal does not depend on the request's other members either.
    for (label, version) in rejected_versions() {
        for (operation, member) in READS {
            for plan in [STORED_PLAN, ABSENT_PLAN] {
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
                        (status, error_code(&body)),
                        (400, "ValidationException"),
                        "case {case:?}: expected a 400 ValidationException, got {status} with \
                         {body}"
                    );
                    let message = error_message(&body);
                    assert!(
                        message.contains("ScalingPlanVersion"),
                        "case {case:?}: the refusal must name the member it is about, got \
                         {message:?}"
                    );
                    assert!(
                        message.contains(&version.to_string()),
                        "case {case:?}: the refusal must name the value it refused, got \
                         {message:?}"
                    );
                    assert!(
                        message.contains(&ONLY_VERSION.to_string()),
                        "case {case:?}: the refusal must name the only valid value, got \
                         {message:?}"
                    );
                    assert!(
                        body.get(member).is_none(),
                        "case {case:?}: a refusal carries no {member}, got {body}"
                    );
                }
            }
        }
    }

    // The control, at the one documented version: the held plan is served what it holds and
    // the absent name is served the empty set, so the refusals above are about the member
    // and not about these operations.
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
        "case the documented version on the held plan: the stored plan instructs one \
         resource, got {described}"
    );
    assert_eq!(
        described["ScalingPlanResources"][0]["ResourceId"],
        json!(RESOURCE_ID),
        "case the documented version on the held plan: the reported resource must be the \
         instructed one, got {described}"
    );
    for (operation, member) in READS {
        let body = emulator
            .ok(operation, read_request(operation, ABSENT_PLAN, ONLY_VERSION))
            .await;
        assert_eq!(
            body[member],
            json!([]),
            "case {operation} at the documented version on an absent name: expected an empty \
             {member} list, got {body}"
        );
    }
}

#[tokio::test]
async fn a_refused_version_changes_nothing_a_later_caller_sees() {
    let emulator = with_one_stored_plan().await;
    let before = emulator.ok("DescribeScalingPlans", json!({})).await;

    for (label, version) in rejected_versions() {
        for (operation, _) in READS {
            let request = read_request(operation, STORED_PLAN, version);
            let (first, first_body) = emulator.call(operation, &request).await;
            let (again, again_body) = emulator.call(operation, &request).await;
            assert_eq!(
                (first, error_code(&first_body)),
                (again, error_code(&again_body)),
                "case {operation}/{label} ({version}): two refusals of the same request must \
                 agree, got {first_body} then {again_body}"
            );
        }
    }

    let after = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        after["ScalingPlans"], before["ScalingPlans"],
        "case the refused versions above: the stored plans must be as they were, got {after} \
         after {before}"
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
        "case after the refusals: the held plan must still report its one resource, got \
         {described}"
    );
}

#[tokio::test]
async fn the_operations_that_model_a_not_found_shape_still_answer_in_that_shape() {
    let emulator = with_one_stored_plan().await;

    // The boundary of the rule above: `DeleteScalingPlan` and `UpdateScalingPlan` bind
    // `ObjectNotFoundException`
    // (`smithy_query(query="operation",operation="DeleteScalingPlan,UpdateScalingPlan")`),
    // so a lookup of theirs that finds no plan is reported in that shape — the version
    // member did not take that answer over.
    for (label, name, version) in [
        ("an absent name at the documented version", ABSENT_PLAN, ONLY_VERSION),
        ("a stored name at another version", STORED_PLAN, ONLY_VERSION + 1),
    ] {
        for operation in ["DeleteScalingPlan", "UpdateScalingPlan"] {
            let request = json!({ "ScalingPlanName": name, "ScalingPlanVersion": version });
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ObjectNotFoundException"),
                "case {operation}/{label}: expected a 400 ObjectNotFoundException, got {status} \
                 with {body}"
            );
            let message = error_message(&body);
            let expected = format!(
                "No scaling plan found for scaling plan name: {name}, version: {version}"
            );
            assert_eq!(
                message, expected,
                "case {operation}/{label}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // And the plan that IS stored, at the version it has, is not refused on either path.
    let stored: &Value = &emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        stored["ScalingPlans"].as_array().map(Vec::len),
        Some(1),
        "the refusals above must leave the one stored plan in place, got {stored}"
    );
}

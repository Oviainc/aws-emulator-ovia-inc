// SPDX-License-Identifier: CC0-1.0

//! The `ScalingPlanVersion` member on `DescribeScalingPlans` — the one request shape that
//! carries it and reports a COLLECTION rather than one plan.
//!
//! One rule is pinned here, over a TABLE of versions and of name selectors rather than the
//! one request that exposed it: a request member the model documents as having exactly one
//! valid value is a request-level constraint, so a value other than that one is refused
//! with the modelled `ValidationException` (HTTP 400) naming the member, the value and the
//! only valid value, before any lookup — never folded into the match predicate and answered
//! as a page that quietly matched nothing. The rule holds whatever else the request selects
//! by, so it is driven over held names, absent names and a mixture, with and without a page
//! size, and the refusal is pinned to be the SAME sentence the two sibling reads answer with.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`], the version table from
//! [`crate::plan_version_member_validation`] and the sibling read requests from
//! [`crate::absent_plan_reads`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::ONLY_VERSION;

use crate::absent_plan_reads::{read_request, READS};
use crate::plan_version_member_validation::rejected_versions;
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plans the emulator holds in these tests, so a row probes the VERSION member rather
/// than the name half of the selector.
const HELD_PLANS: [&str; 2] = ["held plan", "second plan"];

/// A plan name this service does not hold, so a row shows the member is refused before any
/// lookup runs.
const ABSENT_PLAN: &str = "sim-probe-absent";

/// An emulator holding two settled plans, each with one scaling instruction.
async fn with_stored_plans() -> Emulator {
    let emulator = Emulator::new();
    for (index, name) in HELD_PLANS.iter().enumerate() {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(index + 1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();
    emulator
}

/// The name selectors a version may accompany: the member is documented "If you specify a
/// scaling plan version, you must also specify a scaling plan name", so every row names at
/// least one — one the service holds, one it does not, a mixture, and every held name.
fn name_selectors() -> [(&'static str, Value); 4] {
    [
        ("one held name", json!([HELD_PLANS[0]])),
        ("an absent name", json!([ABSENT_PLAN])),
        (
            "a held name beside an absent one",
            json!([HELD_PLANS[0], ABSENT_PLAN]),
        ),
        ("every held name", json!(HELD_PLANS)),
    ]
}

#[tokio::test]
async fn a_version_other_than_the_only_documented_one_is_refused_on_the_member() {
    let emulator = with_stored_plans().await;

    for (version_label, version) in rejected_versions() {
        for (selector_label, names) in name_selectors() {
            for paged in [false, true] {
                let mut request =
                    json!({ "ScalingPlanNames": names, "ScalingPlanVersion": version });
                if paged {
                    request["MaxResults"] = json!(1);
                }
                let case = format!(
                    "DescribeScalingPlans/{version_label} ({version}) with \
                     {selector_label}, paged {paged}"
                );
                let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "ValidationException"),
                    "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
                );
                let message = error_message(&body);
                assert!(
                    message.contains("ScalingPlanVersion"),
                    "case {case:?}: the refusal must name the member it is about, got {message:?}"
                );
                assert!(
                    message.contains(&version.to_string()),
                    "case {case:?}: the refusal must name the value it refused, got {message:?}"
                );
                assert!(
                    message.contains(&ONLY_VERSION.to_string()),
                    "case {case:?}: the refusal must name the only valid value, got {message:?}"
                );
                assert!(
                    body.get("ScalingPlans").is_none(),
                    "case {case:?}: a refusal carries no ScalingPlans page, got {body}"
                );
            }
        }
    }

    // The controls, at the one documented version and with the member omitted: the named
    // plan is served, a name the service does not hold is served the empty page, and an
    // unfiltered read still reports every plan — so the refusals above are about the
    // member's value and not about this operation or its selector.
    for (case, request, expected) in [
        (
            "the documented version on a held name",
            json!({ "ScalingPlanNames": [HELD_PLANS[0]], "ScalingPlanVersion": ONLY_VERSION }),
            vec![json!(HELD_PLANS[0])],
        ),
        (
            "the documented version on an absent name",
            json!({ "ScalingPlanNames": [ABSENT_PLAN], "ScalingPlanVersion": ONLY_VERSION }),
            Vec::new(),
        ),
        (
            "the member omitted",
            json!({}),
            HELD_PLANS.iter().map(|name| json!(name)).collect(),
        ),
    ] {
        let body = emulator.ok("DescribeScalingPlans", request).await;
        let served: Vec<Value> = body["ScalingPlans"]
            .as_array()
            .map(|plans| {
                plans
                    .iter()
                    .map(|plan| plan["ScalingPlanName"].clone())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            served, expected,
            "case {case:?}: expected the plans {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn every_operation_carrying_the_member_refuses_it_with_one_sentence() {
    let emulator = with_stored_plans().await;

    // The member is the same member on all three reporting operations that carry it, so a
    // caller that mistypes the version reads one answer whichever it called — not an empty
    // page from the collection read and a refusal from the other two.
    for (version_label, version) in rejected_versions() {
        let collection = json!({ "ScalingPlanNames": [HELD_PLANS[0]], "ScalingPlanVersion": version });
        let (status, body) = emulator.call("DescribeScalingPlans", &collection).await;
        let expected = (400u16, "ValidationException".to_string(), error_message(&body).to_string());
        assert_eq!(
            (status, error_code(&body).to_string()),
            (expected.0, expected.1.clone()),
            "case DescribeScalingPlans/{version_label} ({version}): expected a 400 \
             ValidationException, got {status} with {body}"
        );
        for (operation, member) in READS {
            let request = read_request(operation, HELD_PLANS[0], version);
            let (sibling_status, sibling_body) = emulator.call(operation, &request).await;
            assert_eq!(
                (
                    sibling_status,
                    error_code(&sibling_body).to_string(),
                    error_message(&sibling_body).to_string()
                ),
                expected,
                "case {operation}/{version_label} ({version}): every operation carrying the \
                 member must refuse it the same way, got {sibling_status} with {sibling_body}"
            );
            assert!(
                sibling_body.get(member).is_none(),
                "case {operation}/{version_label} ({version}): a refusal carries no {member}, \
                 got {sibling_body}"
            );
        }
    }
}

#[tokio::test]
async fn the_members_other_documented_rule_and_the_stored_plans_are_unchanged() {
    let emulator = with_stored_plans().await;
    let before = emulator.ok("DescribeScalingPlans", json!({})).await;

    // The member's second documented sentence — "If you specify a scaling plan version, you
    // must also specify a scaling plan name" — still refuses a version with no name, at the
    // documented version and at one that is refused on its value.
    for (case, request, names_member) in [
        (
            "the documented version with no name",
            json!({ "ScalingPlanVersion": ONLY_VERSION }),
            "ScalingPlanVersion",
        ),
        (
            "a refused version with no name",
            json!({ "ScalingPlanVersion": ONLY_VERSION + 1 }),
            "ScalingPlanVersion",
        ),
        (
            "a refused version beside an application source",
            json!({
                "ApplicationSources": [stack_source(1)],
                "ScalingPlanVersion": ONLY_VERSION + 1,
            }),
            // The selector conflict is refused first, with its own fixed sentence; it
            // names the version as the member it conflicts with.
            "version",
        ),
    ] {
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
        );
        assert!(
            error_message(&body).contains(names_member),
            "case {case:?}: the refusal must name {names_member}, the member it is about, got \
             {body}"
        );
    }

    // Reads do not change what a later caller sees: every refusal above left the stored
    // plans exactly as they were.
    for (version_label, version) in rejected_versions() {
        let request = json!({ "ScalingPlanNames": HELD_PLANS, "ScalingPlanVersion": version });
        let (first, first_body) = emulator.call("DescribeScalingPlans", &request).await;
        let (again, again_body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            (first, error_code(&first_body)),
            (again, error_code(&again_body)),
            "case {version_label} ({version}): two refusals of the same request must agree, \
             got {first_body} then {again_body}"
        );
    }
    let after = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        after["ScalingPlans"], before["ScalingPlans"],
        "case the refused versions above: the stored plans must be as they were, got {after} \
         after {before}"
    );
}

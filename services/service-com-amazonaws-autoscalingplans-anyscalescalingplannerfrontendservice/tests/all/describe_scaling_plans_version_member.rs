// SPDX-License-Identifier: CC0-1.0

//! The `ScalingPlanVersion` member on `DescribeScalingPlans` — the one request shape that
//! carries it and reports a COLLECTION rather than one plan.
//!
//! One rule is pinned here, over a TABLE of versions and of name selectors rather than the
//! one request that exposed it: a request member the model constrains to no set of values
//! SELECTS the resources a request reports on, so a version no stored plan carries narrows
//! the name selector to nothing and the operation answers HTTP 200 with an empty
//! `ScalingPlans` page — never a client error, because nothing about the request is
//! malformed. The rule holds whatever else the request selects by, so it is driven over
//! held names, absent names and a mixture, with and without a page size.
//!
//! What the member IS still refused for — accompanying no name, or accompanying an
//! application source — is pinned in [`crate::plan_version_member_validation`], and what
//! the two single-plan reads and the two writes answer for the same coordinate in
//! [`crate::plan_version_selects_a_plan`].
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`] and the version table from
//! [`crate::plan_version_member_validation`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::ONLY_VERSION;

use crate::plan_version_member_validation::unheld_versions;
use crate::scaling_plan_lifecycle::{simple_instructions, stack_source, Emulator};

/// The plans the emulator holds in these tests, so a row probes the VERSION member rather
/// than the name half of the selector.
const HELD_PLANS: [&str; 2] = ["held plan", "second plan"];

/// A plan name this service does not hold, so a row probes both halves at once.
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

/// The names the page served for one request, in the order it reported them.
fn served_names(body: &Value) -> Vec<Value> {
    body["ScalingPlans"]
        .as_array()
        .map(|plans| {
            plans
                .iter()
                .map(|plan| plan["ScalingPlanName"].clone())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_version_no_stored_plan_carries_selects_nothing() {
    let emulator = with_stored_plans().await;

    for (version_label, version) in unheld_versions() {
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
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {body}"
                );
                assert_eq!(
                    body["ScalingPlans"],
                    json!([]),
                    "case {case:?}: expected an empty ScalingPlans page, got {body}"
                );
                assert!(
                    body.get("NextToken").is_none_or(Value::is_null),
                    "case {case:?}: an empty page continues nowhere, got {body}"
                );
            }
        }
    }

    // The controls, at the version the stored plans carry and with the member omitted: the
    // named plan is served, a name the service does not hold is served the empty page, and
    // an unfiltered read still reports every plan — so the empty pages above come from the
    // version selecting nothing and not from this operation or its name selector.
    for (case, request, expected) in [
        (
            "the stored version on a held name",
            json!({ "ScalingPlanNames": [HELD_PLANS[0]], "ScalingPlanVersion": ONLY_VERSION }),
            vec![json!(HELD_PLANS[0])],
        ),
        (
            "the stored version on an absent name",
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
        assert_eq!(
            served_names(&body),
            expected,
            "case {case:?}: expected the plans {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn selecting_nothing_changes_nothing_a_later_caller_sees() {
    let emulator = with_stored_plans().await;
    let before = emulator.ok("DescribeScalingPlans", json!({})).await;

    for (version_label, version) in unheld_versions() {
        let request = json!({ "ScalingPlanNames": HELD_PLANS, "ScalingPlanVersion": version });
        let first = emulator.ok("DescribeScalingPlans", request.clone()).await;
        let again = emulator.ok("DescribeScalingPlans", request).await;
        assert_eq!(
            first["ScalingPlans"], again["ScalingPlans"],
            "case {version_label} ({version}): two reads of the same empty selection must \
             agree, got {first} then {again}"
        );
    }

    let after = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        after["ScalingPlans"], before["ScalingPlans"],
        "case the reads above: the stored plans must be as they were, got {after} after \
         {before}"
    );
}

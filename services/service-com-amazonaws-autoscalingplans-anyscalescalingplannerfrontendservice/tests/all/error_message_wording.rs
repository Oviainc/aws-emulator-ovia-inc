// SPDX-License-Identifier: CC0-1.0

//! The wording of the refusals this service builds from the request.
//!
//! Two rules are pinned here, each over a TABLE of inputs rather than the one request
//! that exposed them:
//!
//! * a lookup by `ScalingPlanName` + `ScalingPlanVersion` that finds no stored plan is
//!   refused with the request-derived sentence
//!   `No scaling plan found for scaling plan name: <name>, version: <version>`, on every
//!   operation that MODELS such a refusal — `DescribeScalingPlanResources` models none
//!   and answers a miss with an empty page instead, which the same rows check here and
//!   `crate::scaling_instruction_validation` covers in full;
//! * a `DescribeScalingPlans` request that combines `ApplicationSources` with the
//!   name-based selector (`ScalingPlanNames` and/or `ScalingPlanVersion`) is refused
//!   with the fixed sentence
//!   `Application sources cannot be provided with scaling plan names or version`.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The operations that REFUSE a lookup by `ScalingPlanName` + `ScalingPlanVersion` that
/// finds no plan, with the error shape each models that refusal in.
///
/// Source: `smithy_query(query="operation",
/// operation="DeleteScalingPlan,UpdateScalingPlan,DescribeScalingPlanResources")` — the
/// two write paths bind `ObjectNotFoundException` (client, httpError 400).
/// `DescribeScalingPlanResources` binds no not-found error at all and its
/// `ScalingPlanResources` output member is not required, so a miss there is a 200
/// carrying an empty list, not a refusal: it is checked that way below.
const LOOKUP_OPERATIONS: [(&str, &str); 2] = [
    ("DeleteScalingPlan", "ObjectNotFoundException"),
    ("UpdateScalingPlan", "ObjectNotFoundException"),
];

/// AWS's fixed text for the application-source / name-selector conflict.
const APPLICATION_SOURCES_CONFLICT: &str =
    "Application sources cannot be provided with scaling plan names or version";

/// The not-found sentence the rule requires for one requested name and version.
fn expected_not_found(name: &str, version: i64) -> String {
    format!("No scaling plan found for scaling plan name: {name}, version: {version}")
}

/// A plan the emulator really holds, so a row that names it probes the VERSION half of
/// the lookup rather than the name half.
const STORED_PLAN: &str = "stored plan";

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
async fn a_lookup_that_finds_no_plan_names_the_requested_name_and_version() {
    let emulator = with_one_stored_plan().await;

    // `ScalingPlanName` is len 1..128, pattern ^[\p{Print}&&[^|:/]]+$ and
    // `ScalingPlanVersion` is a long (Smithy: DeleteScalingPlanRequest members), so the
    // rows differ in name length, name character content and version magnitude — the
    // dimensions the sentence substitutes over.
    let name_at_bound = "a".repeat(128);
    let cases: Vec<(&str, String, i64)> = vec![
        ("absent plain name", "sim-probe-absent".to_string(), 1),
        ("absent one-character name", "x".to_string(), 1),
        (
            "absent name with spaces and punctuation",
            "my plan #2 (staging)".to_string(),
            1,
        ),
        ("absent name at the 128-character bound", name_at_bound, 1),
        (
            "stored name, version past the only one modelled",
            STORED_PLAN.to_string(),
            2,
        ),
        ("stored name, large version", STORED_PLAN.to_string(), 4_294_967_296),
        (
            "absent name, very large version",
            "sim-probe-absent".to_string(),
            9_007_199_254_740_993,
        ),
        ("absent name, version zero", "sim-probe-absent".to_string(), 0),
        ("absent name, negative version", "sim-probe-absent".to_string(), -1),
    ];

    for (label, name, version) in cases {
        let request = json!({ "ScalingPlanName": name, "ScalingPlanVersion": version });
        for (operation, expected_shape) in LOOKUP_OPERATIONS {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation}/{label}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_shape,
                "case {operation}/{label}: expected error code {expected_shape}, got {body}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual,
                expected_not_found(&name, version),
                "case {operation}/{label}: expected {:?}, got {actual:?}",
                expected_not_found(&name, version)
            );
        }

        // The same miss on the operation that models no not-found error: an empty page,
        // which is what AWS answers there.
        let (status, body) = emulator
            .call("DescribeScalingPlanResources", &request)
            .await;
        assert_eq!(
            (status, body.get("ScalingPlanResources")),
            (200, Some(&json!([]))),
            "case DescribeScalingPlanResources/{label}: expected HTTP 200 with an empty \
             ScalingPlanResources list, got {status} with {body}"
        );
    }

    // The control: the plan that IS stored, at the version it has, is not refused — the
    // sentence above is reported for a missing plan, not for every lookup.
    emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": STORED_PLAN, "ScalingPlanVersion": 1 }),
        )
        .await;
}

#[tokio::test]
async fn application_sources_beside_the_name_selector_are_refused_in_awss_words() {
    let emulator = Emulator::new();
    for (n, name) in [(1usize, "by-stack"), (2, "second")] {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(n),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();

    let eleven: Vec<String> = (0..11).map(|n| format!("name-{n}")).collect();
    // Rows differ in WHICH half of the name selector is present, in how many names and
    // sources are supplied, and in whether another rule is broken at the same time.
    let conflicting: Vec<(&str, Value)> = vec![
        (
            "one source and one name",
            json!({ "ScalingPlanNames": ["by-stack"], "ApplicationSources": [stack_source(1)] }),
        ),
        (
            "one source and a version",
            json!({ "ApplicationSources": [stack_source(1)], "ScalingPlanVersion": 1 }),
        ),
        (
            "one source, a name and a version",
            json!({
                "ScalingPlanNames": ["by-stack"],
                "ScalingPlanVersion": 1,
                "ApplicationSources": [stack_source(1)],
            }),
        ),
        (
            "two sources and two names",
            json!({
                "ScalingPlanNames": ["by-stack", "second"],
                "ApplicationSources": [stack_source(1), stack_source(2)],
            }),
        ),
        (
            "eleven names beside a source",
            json!({ "ScalingPlanNames": eleven, "ApplicationSources": [stack_source(1)] }),
        ),
    ];

    for (label, request) in conflicting {
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {label}: expected a ValidationException, got {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, APPLICATION_SOURCES_CONFLICT,
            "case {label}: expected {APPLICATION_SOURCES_CONFLICT:?}, got {actual:?}"
        );
    }

    // Either selector ALONE is accepted, so the rule refuses the combination and not
    // the members themselves.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "sources alone",
            json!({ "ApplicationSources": [stack_source(1)] }),
        ),
        ("names alone", json!({ "ScalingPlanNames": ["by-stack"] })),
        (
            "names and a version",
            json!({ "ScalingPlanNames": ["by-stack"], "ScalingPlanVersion": 1 }),
        ),
        ("no selector at all", json!({})),
    ];
    for (label, request) in accepted {
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200, got {status} with {body}"
        );
    }

    // A version with neither a name nor a source breaks a DIFFERENT rule, so it must not
    // borrow the conflict sentence.
    let (status, body) = emulator
        .call("DescribeScalingPlans", &json!({ "ScalingPlanVersion": 1 }))
        .await;
    let actual = error_message(&body);
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case version without a name or a source: expected a 400 ValidationException, got {body}"
    );
    assert_ne!(
        actual, APPLICATION_SOURCES_CONFLICT,
        "case version without a name or a source: the application-source conflict text must \
         not be reported for it, got {actual:?}"
    );
}

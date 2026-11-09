// SPDX-License-Identifier: CC0-1.0

//! What `ScalingPlanVersion` is still VALIDATED for, now that its value selects rather
//! than gates — and the table of versions every suite driving the member spans.
//!
//! The member carries no `@range` and no enum
//! (`smithy_query(query="operation",operation="DescribeScalingPlanResources")` — member
//! `ScalingPlanVersion`, type `long`), so its VALUE is a selector of the plan a request
//! addresses ([`crate::plan_version_selects_a_plan`] pins what selecting nothing answers,
//! [`crate::describe_scaling_plans_version_member`] the collection read's copy of it).
//! What is still refused on the request is which MEMBERS accompany it: the collection
//! read documents "If you specify a scaling plan version, you must also specify a scaling
//! plan name", and refuses the version beside `ApplicationSources` with the
//! selector-conflict sentence instead. Both are rules about the request's SHAPE, so each
//! is driven here over versions a stored plan carries and versions none does.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::json;

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::ONLY_VERSION;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// AWS's fixed refusal for a request that selects by application source AND by the
/// name-based selector, whose wording is pinned in [`crate::error_message_wording`].
const APPLICATION_SOURCES_CONFLICT: &str =
    "Application sources cannot be provided with scaling plan names or version";

/// Versions no plan this service holds carries.
///
/// `ScalingPlanVersion` is a `long` with no `@range` and no enum, so the rows span that
/// type's MAGNITUDE and SIGN: the version below the one a created plan carries, the one
/// above it, a negative version, versions past 32 and past 53 bits, and both ends of the
/// type.
///
/// `pub(crate)` so every suite driving this member spans the same values.
pub(crate) fn unheld_versions() -> [(&'static str, i64); 7] {
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

/// Every version a request may carry: the one a created plan carries, and the ones no
/// stored plan does — so a rule about the request's shape is shown to hold whatever the
/// value is.
fn every_version() -> Vec<(&'static str, i64)> {
    let mut versions = vec![("the stored version", ONLY_VERSION)];
    versions.extend(unheld_versions());
    versions
}

/// An emulator holding one settled plan, so a refusal below is about the request's shape
/// and not about an empty store.
async fn with_one_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "held plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

#[tokio::test]
async fn a_version_with_no_name_is_refused_whatever_the_version_is() {
    let emulator = with_one_stored_plan().await;

    for (label, version) in every_version() {
        let request = json!({ "ScalingPlanVersion": version });
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label:?} ({version}) with no name: expected a 400 ValidationException, got \
             {status} with {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains("ScalingPlanVersion"),
            "case {label:?} ({version}) with no name: the refusal must name the member it is \
             about, got {message:?}"
        );
        assert_ne!(
            message, APPLICATION_SOURCES_CONFLICT,
            "case {label:?} ({version}) with no name: a request carrying no application \
             source must not borrow the selector-conflict sentence, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_version_beside_an_application_source_is_refused_as_the_selector_conflict() {
    let emulator = with_one_stored_plan().await;

    for (label, version) in every_version() {
        for (names_label, request) in [
            (
                "with no name",
                json!({
                    "ApplicationSources": [stack_source(1)],
                    "ScalingPlanVersion": version,
                }),
            ),
            (
                "with a name",
                json!({
                    "ScalingPlanNames": ["held plan"],
                    "ApplicationSources": [stack_source(1)],
                    "ScalingPlanVersion": version,
                }),
            ),
        ] {
            let case = format!("{label:?} ({version}) {names_label}");
            let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {status} with {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, APPLICATION_SOURCES_CONFLICT,
                "case {case}: expected {APPLICATION_SOURCES_CONFLICT:?}, got {message:?}"
            );
        }
    }

    // The control: the member accompanied by a name and no application source is a request
    // this operation serves, whatever the version selects.
    for (label, version) in every_version() {
        let request = json!({ "ScalingPlanNames": ["held plan"], "ScalingPlanVersion": version });
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            status, 200,
            "case {label:?} ({version}) with a name and no source: expected HTTP 200, got \
             {status} with {body}"
        );
    }
}

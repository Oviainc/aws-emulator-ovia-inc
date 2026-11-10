// SPDX-License-Identifier: CC0-1.0

//! The ONE wording this service gives a documented-entry-count refusal.
//!
//! Several request list members of this service state a maximum entry count in their own
//! documentation while the list shapes they target carry no modelled `@length`
//! (`smithy_query(query="shape",shape="TagFilters,TagValues,TargetTrackingConfigurations,ScalingPlanNames,ApplicationSources")`
//! returns a bare `list` for each), so the bound is the service's to apply and the
//! refusal is the service's to word.
//!
//! The rule pinned here: whichever list member holds more entries than its own
//! documentation allows, the refusal is the SAME 400 `ValidationException` sentence —
//! `At most <max> <entries> are allowed`, stating the maximum that member's documentation
//! states and what the member counts, as its own documentation names them — never a
//! per-member rendering of the same rule.
//!
//! The table spans EVERY list member of this service that carries a documented entry
//! count, each on every operation that carries it, at the counts around its own published
//! bound: well inside it, one below it, exactly on it, one past it, two past it and far
//! past it. For a member that lives inside a list, the refusal is also shown to be the
//! same sentence whichever entry of the enclosing list breaks it, and the accepted side
//! is read back through `DescribeScalingPlans`, so a list at its bound is shown to be
//! stored rather than merely not refused.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::describe_scaling_plans::{
    APPLICATION_SOURCE_ENTRIES, MAX_APPLICATION_SOURCES, MAX_PLAN_NAMES, PLAN_NAME_ENTRIES,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    MAX_TAG_FILTERS, MAX_TAG_VALUES, MAX_TARGET_TRACKING_CONFIGURATIONS, TAG_FILTER_ENTRIES,
    TAG_VALUE_ENTRIES, TARGET_TRACKING_ENTRIES,
};

use crate::scaling_instruction_validation::{customized_configurations, with};
use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan the write paths here update, so an update is answered about its request
/// rather than about a plan that does not exist.
const PLAN: &str = "entry-count-wording";

/// The name a create in this suite uses: no stored plan claims it, so a create is
/// answered about its request too.
const NEW_PLAN: &str = "a-new-plan";

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// The refusal a list past its documented maximum entry count has to carry: the maximum
/// its own documentation states and what the member counts.
///
/// `pub(crate)` so every suite in this target expects ONE rendering of this refusal,
/// whichever member it is reported for.
pub(crate) fn entry_count_refusal(entries_named: &str, bound: usize) -> String {
    format!("At most {bound} {entries_named} are allowed")
}

/// One request list member whose own documentation states a maximum entry count.
struct BoundedList {
    /// What the member counts, as the refusal names them.
    member: &'static str,
    /// The maximum entry count its documentation states.
    bound: usize,
    /// Every `(operation, request)` carrying this member with `count` entries.
    requests: fn(usize) -> Vec<(&'static str, Value)>,
}

/// Every `(operation, request)` that carries ONE `ApplicationSource`.
fn source_requests(source: &Value) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": NEW_PLAN,
                "ApplicationSource": source,
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ScalingPlanVersion": ONLY_VERSION,
                "ApplicationSource": source,
            }),
        ),
        (
            "DescribeScalingPlans",
            json!({ "ApplicationSources": [source] }),
        ),
    ]
}

/// Requests whose `ApplicationSource` carries `count` tag filters.
fn tag_filter_requests(count: usize) -> Vec<(&'static str, Value)> {
    let filters: Vec<Value> = (0..count)
        .map(|n| json!({ "Key": format!("k{n}"), "Values": [format!("v{n}")] }))
        .collect();
    source_requests(&json!({ "TagFilters": filters }))
}

/// Requests whose `ApplicationSource` carries one tag filter holding `count` values.
fn tag_value_requests(count: usize) -> Vec<(&'static str, Value)> {
    let values: Vec<String> = (0..count).map(|n| format!("v{n}")).collect();
    source_requests(&json!({ "TagFilters": [{ "Key": "env", "Values": values }] }))
}

/// Requests whose `ScalingPlanNames` selector carries `count` names.
///
/// `ScalingPlanName` is len 1..128 with pattern `^[\p{Print}&&[^|:/]]+$`
/// (`smithy_query(query="operation",operation="DescribeScalingPlans")`), which every
/// name below satisfies, so only the LIST's length can be at fault.
fn plan_name_requests(count: usize) -> Vec<(&'static str, Value)> {
    let names: Vec<String> = (0..count).map(|n| format!("name-{n}")).collect();
    vec![(
        "DescribeScalingPlans",
        json!({ "ScalingPlanNames": names }),
    )]
}

/// Requests whose `ApplicationSources` selector carries `count` distinct sources.
fn application_source_requests(count: usize) -> Vec<(&'static str, Value)> {
    let sources: Vec<Value> = (0..count).map(stack_source).collect();
    vec![(
        "DescribeScalingPlans",
        json!({ "ApplicationSources": sources }),
    )]
}

/// A scaling-instruction list of `instructions` entries, the one at `position` carrying
/// `count` target tracking configurations and every other one carrying a single
/// configuration.
///
/// Each entry names its own resource, so the one-instruction-per-scalable-resource rule
/// has nothing to say about the list and only the configuration COUNT is at issue.
fn instructions_with(position: usize, instructions: usize, count: usize) -> Value {
    Value::Array(
        (0..instructions)
            .map(|n| {
                let configurations = if n == position {
                    customized_configurations(count)
                } else {
                    customized_configurations(1)
                };
                with(&[
                    ("ResourceId", json!(format!("autoScalingGroup/asg-{n}"))),
                    ("TargetTrackingConfigurations", configurations),
                ])
            })
            .collect::<Vec<Value>>(),
    )
}

/// Requests whose first scaling instruction carries `count` target tracking
/// configurations.
fn instruction_requests(count: usize) -> Vec<(&'static str, Value)> {
    instruction_requests_at(0, 1, count)
}

/// Requests carrying `instructions` scaling instructions, the one at `position` holding
/// `count` target tracking configurations.
fn instruction_requests_at(
    position: usize,
    instructions: usize,
    count: usize,
) -> Vec<(&'static str, Value)> {
    let written = instructions_with(position, instructions, count);
    vec![
        (
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": NEW_PLAN,
                // A source no stored plan claims, so the create is refused for its
                // instructions or served, never for another plan's claim.
                "ApplicationSource": stack_source(9),
                "ScalingInstructions": written,
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ScalingPlanVersion": ONLY_VERSION,
                "ScalingInstructions": written,
            }),
        ),
    ]
}

/// Every list member of this service whose documentation states a maximum entry count,
/// with the bound its own documentation states (cited on the constant each row names).
fn bounded_lists() -> Vec<BoundedList> {
    vec![
        BoundedList {
            member: PLAN_NAME_ENTRIES,
            bound: MAX_PLAN_NAMES,
            requests: plan_name_requests,
        },
        BoundedList {
            member: APPLICATION_SOURCE_ENTRIES,
            bound: MAX_APPLICATION_SOURCES,
            requests: application_source_requests,
        },
        BoundedList {
            member: TAG_FILTER_ENTRIES,
            bound: MAX_TAG_FILTERS,
            requests: tag_filter_requests,
        },
        BoundedList {
            member: TAG_VALUE_ENTRIES,
            bound: MAX_TAG_VALUES,
            requests: tag_value_requests,
        },
        BoundedList {
            member: TARGET_TRACKING_ENTRIES,
            bound: MAX_TARGET_TRACKING_CONFIGURATIONS,
            requests: instruction_requests,
        },
    ]
}

/// A stocked emulator holding [`PLAN`] over a stack source, settled, so an update is
/// admitted and a read has state to filter.
async fn stocked() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// Assert that every request in `requests` is refused with `refusal`.
async fn assert_refused(label: &str, requests: Vec<(&'static str, Value)>, refusal: &str) {
    let emulator = stocked().await;
    for (operation, request) in requests {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label}/{operation}: expected a 400 ValidationException, got {status} \
             with {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, refusal,
            "case {label}/{operation}: expected {refusal:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn every_documented_entry_count_is_refused_in_one_wording() {
    for list in bounded_lists() {
        let refusal = entry_count_refusal(list.member, list.bound);
        // One past the bound, two past it, and far past it: the count differs, the
        // sentence does not.
        for count in [list.bound + 1, list.bound + 2, list.bound * 5] {
            let label = format!("{}/{count} entries", list.member);
            assert_refused(&label, (list.requests)(count), &refusal).await;
        }
    }
}

#[tokio::test]
async fn a_list_within_its_documented_bound_is_never_refused_for_its_length() {
    for list in bounded_lists() {
        let refusal = entry_count_refusal(list.member, list.bound);
        // Inside the bound — including exactly on it — the length rule has nothing to
        // say, whatever else a request may be refused for.
        for count in [0, 1, list.bound - 1, list.bound] {
            let emulator = stocked().await;
            for (operation, request) in (list.requests)(count) {
                let (_, body) = emulator.call(operation, &request).await;
                let actual = error_message(&body);
                assert_ne!(
                    actual, refusal,
                    "case {}/{operation}/{count} entries: a list inside its bound must not \
                     be refused for its length, got {actual:?}",
                    list.member
                );
            }
        }
    }
}

#[tokio::test]
async fn the_entry_count_refusal_is_one_sentence_whichever_entry_breaks_the_bound() {
    // The target tracking bound is per scaling instruction, and the sentence states the
    // bound and what is counted — so it reads the same whichever instruction's list
    // breaks it, rather than being re-worded per position.
    let refusal = entry_count_refusal(TARGET_TRACKING_ENTRIES, MAX_TARGET_TRACKING_CONFIGURATIONS);
    let instructions = 5;
    for position in [0, 1, instructions - 1] {
        let label = format!("the over-long configuration list at instruction {position}");
        assert_refused(
            &label,
            instruction_requests_at(
                position,
                instructions,
                MAX_TARGET_TRACKING_CONFIGURATIONS + 1,
            ),
            &refusal,
        )
        .await;
    }
}

#[tokio::test]
async fn an_instruction_at_the_target_tracking_bound_is_stored_and_read_back() {
    // The accepted side of the rule, through a create and the Describe reader: an
    // instruction carrying the most target tracking configurations its documentation
    // allows is stored exactly as sent.
    for count in [
        1,
        MAX_TARGET_TRACKING_CONFIGURATIONS - 1,
        MAX_TARGET_TRACKING_CONFIGURATIONS,
    ] {
        let emulator = Emulator::new();
        let written = instructions_with(0, 2, count);
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": written,
                }),
            )
            .await;
        emulator.make_all_due();

        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
            .await;
        let stored = described["ScalingPlans"][0]["ScalingInstructions"].clone();
        assert_eq!(
            stored, written,
            "case {count} configurations: the accepted instructions must be stored as \
             sent, got {stored:?}"
        );
    }
}

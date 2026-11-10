// SPDX-License-Identifier: CC0-1.0

//! The documented maximum entry count of EVERY `DescribeScalingPlans` selector list.
//!
//! `DescribeScalingPlansRequest` carries two list selectors and each one's own
//! documentation states its own maximum entry count — "The names of the scaling plans
//! (up to 10)" and "The sources for the applications (up to 10)", from
//! `smithy_query(query="doc",shape="DescribeScalingPlansRequest",member="ScalingPlanNames")`
//! and the same query for `member="ApplicationSources"` — while neither list shape
//! carries a modelled `@length`
//! (`smithy_query(query="shape",shape="ScalingPlanNames,ApplicationSources")`), so the
//! bound is the handler's to apply.
//!
//! The rule pinned here: a request list member holding more entries than its documented
//! maximum is refused with a 400 `ValidationException` naming that member and that
//! bound, and a list within the bound is served.
//!
//! The table spans both members, several entry FORMS per member (so the refusal is shown
//! to count entries rather than to inspect one of them), and the counts around each
//! published bound — well inside it, one below, exactly on it, one past it and far past
//! it. A bound is also shown to be a count check and nothing else: it is reported for an
//! over-long list whatever its entries say, and an at-bound list still filters from
//! stored state.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::describe_scaling_plans::{
    APPLICATION_SOURCE_ENTRIES, MAX_APPLICATION_SOURCES, MAX_PLAN_NAMES, PLAN_NAME_ENTRIES,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// One labelled way of building the n-th entry of a selector list.
type EntryForm = (&'static str, fn(usize) -> Value);

/// The plans stored before every case here, one per application source.
const STORED_PLANS: [&str; 2] = ["by-stack-1", "by-stack-2"];

/// An emulator holding [`STORED_PLANS`], each over its own CloudFormation stack source
/// and settled, so an accepted request has real state to filter.
async fn stocked() -> Emulator {
    let emulator = Emulator::new();
    for (index, name) in STORED_PLANS.iter().enumerate() {
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

/// A `DescribeScalingPlans` request carrying exactly one selector member.
fn selector_request(member: &str, entries: Vec<Value>) -> Value {
    let mut request = serde_json::Map::new();
    request.insert(member.to_string(), Value::Array(entries));
    Value::Object(request)
}

/// The refusal a list past its documented bound has to carry: the maximum its own
/// documentation states and what the member counts.
///
/// The rendering itself is [`crate::documented_entry_count_wording::entry_count_refusal`],
/// the one sentence this service gives that refusal for ANY bounded member.
fn expected_refusal(entries_named: &str, bound: usize) -> String {
    crate::documented_entry_count_wording::entry_count_refusal(entries_named, bound)
}

/// The `ApplicationSource` entry forms used below: a stack ARN, a tag filter, or both
/// kinds alternating. Both forms come from `ApplicationSource`'s own members
/// (`smithy_query(query="shape",shape="ApplicationSource")`: `CloudFormationStackARN`
/// and `TagFilters`).
fn tag_source(n: usize) -> Value {
    json!({ "TagFilters": [{ "Key": "env", "Values": [format!("v{n}")] }] })
}

#[tokio::test]
async fn every_selector_lists_documented_entry_bound_is_enforced() {
    let emulator = stocked().await;

    // Rows differ in the form of the entries: all distinct, one value repeated, two
    // kinds mixed, and (for names) values that do and do not name a stored plan.
    let name_forms: Vec<EntryForm> = vec![
        ("distinct names", |n| json!(format!("plan-{n}"))),
        ("one stored name repeated", |_| json!(STORED_PLANS[0])),
        ("stored and absent names alternating", |n| {
            if n % 2 == 0 {
                json!(STORED_PLANS[0])
            } else {
                json!(format!("absent-{n}"))
            }
        }),
    ];
    let source_forms: Vec<EntryForm> = vec![
        ("distinct stack sources", |n| stack_source(n + 100)),
        ("tag-filter sources", tag_source),
        ("stack and tag sources alternating", |n| {
            if n % 2 == 0 {
                stack_source(n + 100)
            } else {
                tag_source(n)
            }
        }),
        ("one stored stack source repeated", |_| stack_source(1)),
    ];

    for (member, wire_member, bound, forms) in [
        (
            "ScalingPlanNames",
            PLAN_NAME_ENTRIES,
            MAX_PLAN_NAMES,
            name_forms,
        ),
        (
            "ApplicationSources",
            APPLICATION_SOURCE_ENTRIES,
            MAX_APPLICATION_SOURCES,
            source_forms,
        ),
    ] {
        let refusal = expected_refusal(wire_member, bound);
        for (form, entry) in forms {
            // Within the published bound — including exactly on it — the request is
            // served.
            for count in [0, 1, bound - 1, bound] {
                let request = selector_request(member, (0..count).map(entry).collect());
                let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
                assert_eq!(
                    status, 200,
                    "case {member}/{form}/{count} entries: expected HTTP 200, got {status} \
                     with {body}"
                );
            }
            // One past it, and far past it, the request is refused for that member.
            for count in [bound + 1, bound + 2, bound * 10] {
                let request = selector_request(member, (0..count).map(entry).collect());
                let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "ValidationException"),
                    "case {member}/{form}/{count} entries: expected a 400 ValidationException, \
                     got {status} with {body}"
                );
                let actual = error_message(&body);
                assert_eq!(
                    actual, refusal,
                    "case {member}/{form}/{count} entries: expected {refusal:?}, got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_source_bound_counts_entries_and_does_not_inspect_them() {
    let emulator = stocked().await;
    let refusal = expected_refusal(APPLICATION_SOURCE_ENTRIES, MAX_APPLICATION_SOURCES);

    // An entry that identifies no application: refused on its own merits at any count,
    // but an over-long list is refused for its LENGTH first, whatever its entries say.
    let names_nothing = json!({});
    let names_both = json!({
        "CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/s/1",
        "TagFilters": [{ "Key": "env", "Values": ["prod"] }],
    });
    for (label, entries) in [
        (
            "one past the bound, every entry well formed",
            (0..MAX_APPLICATION_SOURCES + 1)
                .map(|n| stack_source(n + 100))
                .collect::<Vec<_>>(),
        ),
        (
            "one past the bound, one entry naming nothing",
            (0..MAX_APPLICATION_SOURCES)
                .map(|n| stack_source(n + 100))
                .chain([names_nothing.clone()])
                .collect(),
        ),
        (
            "one past the bound, one entry naming a stack and tag filters",
            (0..MAX_APPLICATION_SOURCES)
                .map(tag_source)
                .chain([names_both])
                .collect(),
        ),
        (
            "far past the bound, every entry naming nothing",
            vec![names_nothing.clone(); MAX_APPLICATION_SOURCES * 5],
        ),
    ] {
        let request = selector_request("ApplicationSources", entries);
        let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label}: expected a 400 ValidationException, got {status} with {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, refusal,
            "case {label}: expected {refusal:?}, got {actual:?}"
        );
    }

    // At the bound the length rule has nothing to say, so a malformed entry is reported
    // on its own terms and the length sentence must not be borrowed for it.
    let at_bound_with_bad_entry: Vec<Value> = (0..MAX_APPLICATION_SOURCES - 1)
        .map(|n| stack_source(n + 100))
        .chain([names_nothing])
        .collect();
    let request = selector_request("ApplicationSources", at_bound_with_bad_entry);
    let (status, body) = emulator.call("DescribeScalingPlans", &request).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case at the bound with one entry naming nothing: expected a 400 \
         ValidationException, got {status} with {body}"
    );
    let actual = error_message(&body);
    assert_ne!(
        actual, refusal,
        "case at the bound with one entry naming nothing: the length sentence must not be \
         reported for it, got {actual:?}"
    );
}

#[tokio::test]
async fn a_selector_at_its_bound_still_filters_from_stored_state() {
    let emulator = stocked().await;

    // A full-length source filter holding one stored plan's source selects exactly that
    // plan, so the accepted cases above answer from state rather than blanketing a 200.
    let mut sources: Vec<Value> = (0..MAX_APPLICATION_SOURCES - 1)
        .map(|n| stack_source(n + 100))
        .collect();
    sources.push(stack_source(1));
    let body = emulator
        .ok(
            "DescribeScalingPlans",
            selector_request("ApplicationSources", sources),
        )
        .await;
    let selected = plan_names(&body);
    assert_eq!(
        selected,
        vec![STORED_PLANS[0].to_string()],
        "case a full-length source filter naming one stored source: expected only {:?}, \
         got {selected:?}",
        STORED_PLANS[0]
    );

    // The same for the sibling selector, at its own bound.
    let mut names: Vec<Value> = (0..MAX_PLAN_NAMES - 1)
        .map(|n| json!(format!("absent-{n}")))
        .collect();
    names.push(json!(STORED_PLANS[1]));
    let body = emulator
        .ok(
            "DescribeScalingPlans",
            selector_request("ScalingPlanNames", names),
        )
        .await;
    let selected = plan_names(&body);
    assert_eq!(
        selected,
        vec![STORED_PLANS[1].to_string()],
        "case a full-length name filter naming one stored plan: expected only {:?}, got \
         {selected:?}",
        STORED_PLANS[1]
    );
}

/// The `ScalingPlanName` of every plan in a `DescribeScalingPlans` response.
fn plan_names(body: &Value) -> Vec<String> {
    body["ScalingPlans"]
        .as_array()
        .map(|plans| {
            plans
                .iter()
                .map(|plan| plan["ScalingPlanName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// SPDX-License-Identifier: CC0-1.0

//! The documented maximum entry count of every list inside an `ApplicationSource`.
//!
//! `ApplicationSource$TagFilters` documents its own maximum entry count — "A set of
//! tags (up to 50)" — and so does `TagFilter$Values` — "The tag values (0 to 20)" —
//! while neither list shape carries a modelled `@length`
//! (`smithy_query(query="shape",shape="TagFilters,TagValues")`: the only length in
//! either is on the value STRING), so the bound is the service's to apply.
//!
//! The rule pinned here: a request list member holding more entries than its documented
//! maximum is refused with a 400 `ValidationException` naming that member and that
//! bound, and a list within the bound is served. It is asserted on EVERY path that
//! carries an `ApplicationSource` — `CreateScalingPlan`, `UpdateScalingPlan` and the
//! `ApplicationSources` filter of `DescribeScalingPlans` — because one gate serves all
//! three.
//!
//! The table spans both members, several entry FORMS per member (so the refusal is shown
//! to count entries rather than to inspect one of them), and the counts around each
//! published bound — well inside it, one below, exactly on it, one past it and far past
//! it. An at-bound source is also shown to be stored and read back, so an accepted case
//! answers from state rather than blanketing a 200.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    MAX_TAG_FILTERS, MAX_TAG_VALUES, TAG_FILTER_ENTRIES, TAG_VALUE_ENTRIES,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The plan the write paths here operate on.
const PLAN: &str = "tag-filter-bounds";

/// One labelled way of building the n-th tag value of a filter.
///
/// `TagValues`' member is a string of length 1..256
/// (`smithy_query(query="shape",shape="TagValues")`), so every form stays inside it.
type ValueForm = (&'static str, fn(usize) -> String);

/// One labelled way of building the n-th tag filter of a source.
type FilterForm = (&'static str, fn(usize) -> Value);

/// The refusal a list past its documented bound has to carry: the maximum its own
/// documentation states and what the member counts.
///
/// The rendering itself is [`crate::documented_entry_count_wording::entry_count_refusal`],
/// the one sentence this service gives that refusal for ANY bounded member.
fn expected_refusal(entries_named: &str, bound: usize) -> String {
    crate::documented_entry_count_wording::entry_count_refusal(entries_named, bound)
}

/// An `ApplicationSource` whose `TagFilters` holds `count` entries, each built by `form`.
fn filters_source(count: usize, form: fn(usize) -> Value) -> Value {
    json!({ "TagFilters": (0..count).map(form).collect::<Vec<_>>() })
}

/// An `ApplicationSource` with one tag filter whose `Values` holds `count` entries, each
/// built by `form`.
fn values_source(key: &str, count: usize, form: fn(usize) -> String) -> Value {
    json!({
        "TagFilters": [{
            "Key": key,
            "Values": (0..count).map(form).collect::<Vec<_>>(),
        }]
    })
}

/// The tag-filter forms the filter-count rows are built from.
fn filter_forms() -> Vec<FilterForm> {
    vec![
        ("distinct keys, one value each", |n| {
            json!({ "Key": format!("k{n}"), "Values": [format!("v{n}")] })
        }),
        ("one key repeated", |_| {
            json!({ "Key": "env", "Values": ["prod"] })
        }),
        ("distinct keys, no values at all", |n| {
            json!({ "Key": format!("k{n}"), "Values": [] })
        }),
        ("distinct keys, several values each", |n| {
            json!({
                "Key": format!("k{n}"),
                "Values": [format!("a{n}"), format!("b{n}"), format!("c{n}")],
            })
        }),
    ]
}

/// The tag-value forms the value-count rows are built from.
fn value_forms() -> Vec<ValueForm> {
    vec![
        ("distinct values", |n| format!("v{n}")),
        ("one value repeated", |_| "prod".to_string()),
        ("values at the 256-character member bound", |n| {
            format!("{n:0>256}")
        }),
    ]
}

/// A stocked emulator holding [`PLAN`] over a stack source, settled, so the write paths
/// have a plan to update and the read path has state to filter.
///
/// `pub(crate)` so a suite about another rule of the same `ApplicationSource` gate drives
/// the same three paths from the same state rather than building a second harness.
pub(crate) async fn stocked() -> Emulator {
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

/// Every request that carries one `ApplicationSource`, as `(operation, request)`.
///
/// `CreateScalingPlan` names a plan that does not exist and `UpdateScalingPlan` the
/// stored one, so neither is refused for its plan before the source is read.
///
/// `pub(crate)` for the same reason as [`stocked`]: one list of the paths that carry an
/// application source.
pub(crate) fn source_requests(source: &Value) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "a-new-plan",
                "ApplicationSource": source,
                "ScalingInstructions": simple_instructions(),
            }),
        ),
        (
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ScalingPlanVersion": 1,
                "ApplicationSource": source,
            }),
        ),
        (
            "DescribeScalingPlans",
            json!({ "ApplicationSources": [source] }),
        ),
    ]
}

/// Assert that every path carrying `source` refuses it with `refusal`.
///
/// Each case runs against its own freshly stocked emulator, so a case that is SERVED on
/// one path cannot change what the next case sees on another.
///
/// `pub(crate)` for the same reason as [`stocked`].
pub(crate) async fn assert_refused_everywhere(label: &str, source: &Value, refusal: &str) {
    let emulator = stocked().await;
    for (operation, request) in source_requests(source) {
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

/// Assert that no path carrying `source` reports `refusal`: a list inside its bound is
/// never refused for its length, whatever else the source may be refused for.
async fn assert_length_not_reported(label: &str, source: &Value, refusal: &str) {
    let emulator = stocked().await;
    for (operation, request) in source_requests(source) {
        let (_, body) = emulator.call(operation, &request).await;
        let actual = error_message(&body);
        assert_ne!(
            actual, refusal,
            "case {label}/{operation}: a list inside its bound must not be refused for \
             its length, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_tag_filter_list_bound_is_enforced_on_every_application_source_path() {
    let refusal = expected_refusal(TAG_FILTER_ENTRIES, MAX_TAG_FILTERS);

    for (form, entry) in filter_forms() {
        // Within the published bound — including exactly on it — the length rule has
        // nothing to say.
        for count in [0, 1, MAX_TAG_FILTERS - 1, MAX_TAG_FILTERS] {
            let label = format!("{form}/{count} filters");
            assert_length_not_reported(&label, &filters_source(count, entry), &refusal).await;
        }
        // One past it, and far past it, every path refuses it for that member.
        for count in [
            MAX_TAG_FILTERS + 1,
            MAX_TAG_FILTERS + 2,
            MAX_TAG_FILTERS * 10,
        ] {
            let label = format!("{form}/{count} filters");
            assert_refused_everywhere(&label, &filters_source(count, entry), &refusal).await;
        }
    }
}

#[tokio::test]
async fn the_tag_value_list_bound_is_enforced_on_every_application_source_path() {
    let refusal = expected_refusal(TAG_VALUE_ENTRIES, MAX_TAG_VALUES);

    for (form, entry) in value_forms() {
        for count in [0, 1, MAX_TAG_VALUES - 1, MAX_TAG_VALUES] {
            let label = format!("{form}/{count} values");
            let source = values_source("env", count, entry);
            assert_length_not_reported(&label, &source, &refusal).await;
        }
        for count in [MAX_TAG_VALUES + 1, MAX_TAG_VALUES + 2, MAX_TAG_VALUES * 10] {
            let label = format!("{form}/{count} values");
            let source = values_source("env", count, entry);
            assert_refused_everywhere(&label, &source, &refusal).await;
        }
    }
}

#[tokio::test]
async fn the_value_bound_is_reported_for_the_filter_that_breaks_it() {
    // The bound is per filter, and the sentence states the bound and what is counted — so
    // it reads the same whichever filter's value list breaks it.
    let refusal = expected_refusal(TAG_VALUE_ENTRIES, MAX_TAG_VALUES);
    for position in [0, 1, MAX_TAG_FILTERS - 1] {
        let filters: Vec<Value> = (0..MAX_TAG_FILTERS)
            .map(|n| {
                let values: Vec<String> = if n == position {
                    (0..MAX_TAG_VALUES + 1).map(|v| format!("v{v}")).collect()
                } else {
                    vec![format!("v{n}")]
                };
                json!({ "Key": format!("k{n}"), "Values": values })
            })
            .collect();
        let source = json!({ "TagFilters": filters });
        assert_refused_everywhere(
            &format!("the over-long filter at position {position}"),
            &source,
            &refusal,
        )
        .await;
    }
}

#[tokio::test]
async fn the_tag_filter_bounds_count_entries_and_do_not_inspect_them() {
    let refusal = expected_refusal(TAG_FILTER_ENTRIES, MAX_TAG_FILTERS);

    // An over-long filter list is refused for its LENGTH first, whatever its entries say
    // and whatever else the source gets wrong.
    let keyless = json!({ "Values": ["prod"] });
    for (label, source) in [
        (
            "one past the bound, every filter well formed",
            filters_source(MAX_TAG_FILTERS + 1, |n| {
                json!({ "Key": format!("k{n}"), "Values": [format!("v{n}")] })
            }),
        ),
        (
            "far past the bound, every filter carrying no key at all",
            json!({ "TagFilters": vec![keyless.clone(); MAX_TAG_FILTERS * 3] }),
        ),
        (
            "one past the bound, beside a CloudFormation stack ARN",
            json!({
                "CloudFormationStackARN":
                    "arn:aws:cloudformation:us-east-1:000000000000:stack/s9/abc",
                "TagFilters": vec![keyless.clone(); MAX_TAG_FILTERS + 1],
            }),
        ),
    ] {
        assert_refused_everywhere(label, &source, &refusal).await;
    }

    // At the bound the length rule has nothing to say, so a source that breaks a
    // DIFFERENT rule is reported on its own terms and must not borrow the length
    // sentence.
    let at_bound_with_a_stack_arn = json!({
        "CloudFormationStackARN": "arn:aws:cloudformation:us-east-1:000000000000:stack/s9/abc",
        "TagFilters": vec![keyless; MAX_TAG_FILTERS],
    });
    let emulator = stocked().await;
    let (status, body) = emulator
        .call(
            "CreateScalingPlan",
            &json!({
                "ScalingPlanName": "a-new-plan",
                "ApplicationSource": at_bound_with_a_stack_arn,
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    let actual = error_message(&body);
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case at the bound beside a stack ARN: expected a 400 ValidationException, got {body}"
    );
    assert_ne!(
        actual, refusal,
        "case at the bound beside a stack ARN: the length sentence must not be reported \
         for it, got {actual:?}"
    );
}

#[tokio::test]
async fn a_source_at_both_bounds_is_stored_and_read_back() {
    // The accepted side of the rule, through a create and the Describe reader: a source
    // with the most filters and the most values per filter its documentation allows is
    // stored exactly as sent, and selects its own plan.
    for (label, filter_count, value_count) in [
        ("one filter, one value", 1, 1),
        ("one filter at the value bound", 1, MAX_TAG_VALUES),
        ("at the filter bound, one value each", MAX_TAG_FILTERS, 1),
        ("at both bounds", MAX_TAG_FILTERS, MAX_TAG_VALUES),
    ] {
        let emulator = Emulator::new();
        let filters: Vec<Value> = (0..filter_count)
            .map(|n| {
                json!({
                    "Key": format!("k{n}"),
                    "Values": (0..value_count).map(|v| format!("v{n}-{v}")).collect::<Vec<_>>(),
                })
            })
            .collect();
        let source = json!({ "TagFilters": filters });

        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        emulator.make_all_due();

        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
            .await;
        let stored = described["ScalingPlans"][0]["ApplicationSource"].clone();
        assert_eq!(
            stored, source,
            "case {label}: the accepted source must be stored as sent, got {stored:?}"
        );

        // And the same source, used as a filter, selects exactly that plan.
        let selected = emulator
            .ok(
                "DescribeScalingPlans",
                json!({ "ApplicationSources": [source] }),
            )
            .await;
        let names: Vec<String> = selected["ScalingPlans"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|plan| {
                plan["ScalingPlanName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            names,
            vec![PLAN.to_string()],
            "case {label}: the accepted source must select its own plan, got {names:?}"
        );
    }
}

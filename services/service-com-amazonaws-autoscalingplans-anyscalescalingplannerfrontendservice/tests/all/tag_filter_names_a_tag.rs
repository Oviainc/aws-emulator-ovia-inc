// SPDX-License-Identifier: CC0-1.0

//! An entry of an `ApplicationSource`'s tag-filter list names a tag, and the tag is
//! named by that entry's key.
//!
//! `ApplicationSource$TagFilters` is documented "A set of tags (up to 50)", a `TagFilter`
//! "Represents a tag" and its `Key` is "The tag key", modelled
//! `length {min: 1, max: 128}`
//! (`smithy_query(query="shape",shape="ApplicationSource,TagFilter")`), so a filter whose
//! key is absent — or present and empty, which the modelled minimum has the framework
//! refuse before a handler runs — names no tag at all.
//!
//! The rule pinned here: a tag filter that names no tag key is refused with a 400
//! `ValidationException` naming that entry's `key`, on every path that carries an
//! application source, and no such entry ever becomes part of the identity the
//! one-scaling-plan-per-application-source rule is decided on. A filter that DOES name a
//! key is served whatever its values say, because "The tag values (0 to 20)" admits none.
//!
//! The rows differ along what the rule generalises over: the FORM of the keyless entry
//! (no members at all, values but no key, an empty value list), its POSITION in the list
//! (first, last, past other entries, at the documented filter bound), how many entries
//! the list holds, and what the rest of the source says (nothing, a stack ARN, other
//! filters that do name tags). The accepted side spans the value forms a named key may
//! carry and the published bounds of the key's own length.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`] and the application-source paths from
//! [`crate::tag_filter_entry_bounds`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::{
    ApplicationSource, TagFilter,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    application_source_key, MAX_TAG_FILTERS,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};
use crate::tag_filter_entry_bounds::{assert_refused_everywhere, source_requests, stocked};

/// A well-formed CloudFormation stack ARN, for the rows that pair a keyless filter with
/// the other side of the alternative.
const WELL_FORMED_ARN: &str = "arn:aws:cloudformation:eu-west-1:000000000000:stack/tags/0a1b";

/// The refusal a filter that names no tag has to carry: the framework's own rendering of
/// a member that is not there — `1 validation error detected: Value null at '<path>'
/// failed to satisfy constraint: Member must not be null` — over the path that names a
/// list element by its 1-BASED position and the `member` step
/// (`aws_smithy_schema_server::constraints::path`, which both the deserializer and the
/// constraint walk spell a path with).
///
/// Spelled out here rather than imported, so the sentence the service answers with is
/// pinned by this suite and not by the code under test.
fn keyless_refusal(position: usize) -> String {
    format!(
        "1 validation error detected: Value null at \
         'applicationSource.tagFilters.{position}.member.key' failed to satisfy constraint: \
         Member must not be null"
    )
}

/// The member path the refusals above name, without the sentence around it: a filter's
/// key at a 1-based position.
fn key_member_path(position: usize) -> String {
    format!("applicationSource.tagFilters.{position}.member.key")
}

/// The ways a tag filter can carry no tag key: no members at all, values without a key,
/// and an empty value list without a key.
///
/// Every form stays inside the model: `TagFilter` marks neither member `@required`
/// (`smithy_query(query="shape",shape="TagFilter")`), so each of these is a filter the
/// wire accepts and the service has to decide.
fn keyless_forms() -> Vec<(&'static str, Value)> {
    vec![
        ("a filter with no members at all", json!({})),
        ("a filter with values but no key", json!({ "Values": ["prod"] })),
        (
            "a filter with an empty value list and no key",
            json!({ "Values": [] }),
        ),
    ]
}

/// A filter that does name a tag, distinct per `n`.
fn named_filter(n: usize) -> Value {
    json!({ "Key": format!("k{n}"), "Values": [format!("v{n}")] })
}

#[tokio::test]
async fn a_tag_filter_that_names_no_tag_key_is_refused_on_every_application_source_path() {
    for (form, keyless) in keyless_forms() {
        // Rows: the keyless filter alone; first, between and last among filters that do
        // name tags; at the documented filter bound; and beside each spelling of the
        // other side of the alternative. `position` is the 1-based position of the FIRST
        // filter that names no tag, which is what the refusal names.
        let mut cases: Vec<(String, Value, usize)> = vec![
            (
                format!("{form}, alone in the list"),
                json!({ "TagFilters": [keyless.clone()] }),
                1,
            ),
            (
                format!("{form}, twice over"),
                json!({ "TagFilters": [keyless.clone(), keyless.clone()] }),
                1,
            ),
            (
                format!("{form}, before a filter that names a tag"),
                json!({ "TagFilters": [keyless.clone(), named_filter(1)] }),
                1,
            ),
            (
                format!("{form}, after a filter that names a tag"),
                json!({ "TagFilters": [named_filter(1), keyless.clone()] }),
                2,
            ),
            (
                format!("{form}, between two filters that name tags"),
                json!({ "TagFilters": [named_filter(1), keyless.clone(), named_filter(2)] }),
                2,
            ),
            (
                format!("{form}, beside an empty stack ARN"),
                json!({ "CloudFormationStackARN": "", "TagFilters": [keyless.clone()] }),
                1,
            ),
            (
                format!("{form}, beside a well-formed stack ARN"),
                json!({
                    "CloudFormationStackARN": WELL_FORMED_ARN,
                    "TagFilters": [named_filter(1), keyless.clone()],
                }),
                2,
            ),
        ];
        // At the documented filter bound the length rule has nothing to say, so the
        // keyless entry at the very end of the list is still the one reported.
        let mut at_bound: Vec<Value> = (0..MAX_TAG_FILTERS - 1).map(named_filter).collect();
        at_bound.push(keyless);
        cases.push((
            format!("{form}, last of {MAX_TAG_FILTERS} filters"),
            json!({ "TagFilters": at_bound }),
            MAX_TAG_FILTERS,
        ));

        for (label, source, position) in cases {
            assert_refused_everywhere(&label, &source, &keyless_refusal(position)).await;
        }
    }
}

#[tokio::test]
async fn an_absent_tag_key_and_an_empty_one_are_refused_for_the_same_member() {
    // The modelled minimum length refuses a key that is present and empty before any
    // handler runs, and the service refuses one that is absent: the two spellings of "no
    // tag key" are both 400 ValidationExceptions naming the SAME member, so neither is
    // the accepted way of writing the other.
    for (label, filter) in [
        ("an empty key", json!({ "Key": "", "Values": ["prod"] })),
        ("an empty key and no values", json!({ "Key": "" })),
        ("an absent key", json!({ "Values": ["prod"] })),
        ("neither member", json!({})),
    ] {
        let source = json!({ "TagFilters": [filter] });
        let emulator = stocked().await;
        for (operation, request) in source_requests(&source) {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {label}/{operation}: expected a 400 ValidationException, got {status} \
                 with {body}"
            );
            // The two write paths carry the source as `ApplicationSource`, so both
            // layers name the member at exactly that path. The read path carries it
            // inside its `ApplicationSources` selector, whose own refusal is asserted
            // above.
            if operation == "DescribeScalingPlans" {
                continue;
            }
            let actual = error_message(&body);
            let member = key_member_path(1);
            assert!(
                actual.contains(&member),
                "case {label}/{operation}: expected the refusal to name {member:?}, got \
                 {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn no_spelling_of_a_source_that_names_no_application_is_accepted() {
    // The asymmetry this rule removes: every way of writing a source that identifies no
    // application is refused, whether it names nothing on the stack side, nothing on the
    // tag side, or a tag filter that names no tag.
    for (label, source) in [
        ("neither member", json!({})),
        ("an empty stack ARN", json!({ "CloudFormationStackARN": "" })),
        ("an empty filter list", json!({ "TagFilters": [] })),
        (
            "an empty stack ARN and an empty filter list",
            json!({ "CloudFormationStackARN": "", "TagFilters": [] }),
        ),
        ("one filter that names no tag", json!({ "TagFilters": [{}] })),
        (
            "two filters that name no tag",
            json!({ "TagFilters": [{}, { "Values": ["prod"] }] }),
        ),
        (
            "an empty stack ARN beside a filter that names no tag",
            json!({ "CloudFormationStackARN": "", "TagFilters": [{ "Values": [] }] }),
        ),
    ] {
        let emulator = stocked().await;
        for (operation, request) in source_requests(&source) {
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {label}/{operation}: a source that names no application must be \
                 refused, got {status} with {body}"
            );
        }
        // And nothing was stored for it: the plan the create named is not there.
        let described = emulator.ok("DescribeScalingPlans", json!({})).await;
        let names: Vec<String> = described["ScalingPlans"]
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
        assert!(
            !names.iter().any(|name| name == "a-new-plan"),
            "case {label}: the refused create must have stored nothing, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_that_names_a_tag_key_is_stored_and_selects_its_own_plan() {
    // The accepted side: once a filter names a key, its VALUES are free — "The tag values
    // (0 to 20)" admits none at all — and the key may be anything the modelled length
    // 1..128 allows, including each published bound.
    let key_at_bound = "k".repeat(128);
    for (label, filters) in [
        ("a key with no value member", json!([{ "Key": "env" }])),
        (
            "a key with an empty value list",
            json!([{ "Key": "env", "Values": [] }]),
        ),
        (
            "a key with one value",
            json!([{ "Key": "env", "Values": ["prod"] }]),
        ),
        (
            "a key with several values",
            json!([{ "Key": "tier", "Values": ["web", "api"] }]),
        ),
        (
            "two keys, one of them valueless",
            json!([{ "Key": "env", "Values": ["prod"] }, { "Key": "team" }]),
        ),
        (
            "a key at the one-character bound",
            json!([{ "Key": "k", "Values": ["v"] }]),
        ),
        (
            "a key at the 128-character bound",
            json!([{ "Key": key_at_bound, "Values": ["v"] }]),
        ),
    ] {
        let source = json!({ "TagFilters": filters });
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "tagged",
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        emulator.make_all_due();

        let described = emulator
            .ok(
                "DescribeScalingPlans",
                json!({ "ScalingPlanNames": ["tagged"] }),
            )
            .await;
        let stored = described["ScalingPlans"][0]["ApplicationSource"].clone();
        assert_eq!(
            stored, source,
            "case {label}: the accepted source must be stored as sent, got {stored:?}"
        );

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
            vec!["tagged".to_string()],
            "case {label}: the accepted source must select its own plan, got {names:?}"
        );
    }
}

/// One tag filter as the state module sees it: the tag key it names (if any) and the
/// values it lists.
type FilterSpec<'a> = (Option<&'a str>, Vec<&'a str>);

/// One `ApplicationSource` built from tag filters, as the state module sees it.
fn tag_source(filters: Vec<FilterSpec<'_>>) -> ApplicationSource {
    ApplicationSource {
        tag_filters: Some(
            filters
                .into_iter()
                .map(|(key, values)| TagFilter {
                    key: key.map(str::to_string),
                    values: Some(values.into_iter().map(str::to_string).collect()),
                })
                .collect(),
        ),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_identity_of_a_source_carries_no_part_for_a_filter_that_names_no_tag() {
    // The identity key behind "You can create one scaling plan per application source"
    // is built out of the tags a source names, so a filter that names none contributes
    // NOTHING to it: a source identifies the same application whether such a filter is
    // written beside its tags or not, and a source that names only such filters
    // identifies no application at all.
    let named = tag_source(vec![(Some("env"), vec!["prod"])]);
    let expected = application_source_key(&named);
    assert!(
        expected.is_some(),
        "case a source that names one tag: expected an identity, got {expected:?}"
    );

    let keyless_filters: [(&str, FilterSpec<'_>); 3] = [
        ("a filter with no key and no values", (None, Vec::new())),
        ("a filter with values but no key", (None, vec!["prod"])),
        ("a filter with an empty key", (Some(""), vec!["prod"])),
    ];
    for (label, keyless) in keyless_filters {
        let beside = tag_source(vec![(Some("env"), vec!["prod"]), keyless.clone()]);
        let actual = application_source_key(&beside);
        assert_eq!(
            actual, expected,
            "case {label}: a filter that names no tag must not change the identity, got \
             {actual:?}"
        );
        let rendered = actual.unwrap_or_default();
        assert!(
            !rendered.contains("=;") && !rendered.contains(":="),
            "case {label}: the identity must carry no empty key part, got {rendered:?}"
        );

        // On its own, such a filter names no application, exactly as an absent or empty
        // stack ARN does.
        let alone = tag_source(vec![keyless]);
        let actual = application_source_key(&alone);
        assert!(
            actual.is_none(),
            "case {label}: a source naming only that filter must identify no \
             application, got {actual:?}"
        );
    }

    // The control: a source that names a tag the stocked plan does not is still a
    // DIFFERENT application, so dropping the keyless parts does not collapse distinct
    // sources into one.
    let other = tag_source(vec![(Some("env"), vec!["dev"])]);
    let other_key = application_source_key(&other);
    assert_ne!(
        other_key, expected,
        "case two sources naming different tags: expected distinct identities, got \
         {other_key:?}"
    );

    // And end to end: the plan that claims a tag set keeps it, while the source spelled
    // with an extra keyless filter is refused for that filter rather than quietly
    // claiming another application.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "claimed",
                "ApplicationSource": { "TagFilters": [{ "Key": "env", "Values": ["prod"] }] },
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    let (status, body) = emulator
        .call(
            "CreateScalingPlan",
            &json!({
                "ScalingPlanName": "challenger",
                "ApplicationSource": {
                    "TagFilters": [{ "Key": "env", "Values": ["prod"] }, {}]
                },
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    let actual = error_message(&body);
    assert_eq!(
        (status, error_code(&body), actual),
        (400, "ValidationException", keyless_refusal(2).as_str()),
        "case a claimed tag set spelled with an extra keyless filter: expected the keyless \
         refusal, got {status} with {body}"
    );
    // The plan that does claim the set is untouched, and the stack-source control is
    // still creatable beside it.
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "by-stack",
                "ApplicationSource": stack_source(3),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
}

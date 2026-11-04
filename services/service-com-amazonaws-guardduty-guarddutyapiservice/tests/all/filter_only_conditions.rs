// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH `Condition` members each operation taking a
//! `FindingCriteria` offers its callers.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so routing,
//! restJson1 (de)serialization and the handlers are exercised together.
//!
//! `Condition` is the one condition shape a `FindingCriteria.Criterion` targets, so the
//! filter APIs and the findings reads are handed the same structure; the model narrows
//! two of its members to the filter APIs alone — `Matches` is "available only for
//! create-filter and update-filter APIs"
//! (`smithy_query(query="doc",shape="Condition",member="Matches")`) and `NotMatches`
//! says the same of itself
//! (`smithy_query(query="doc",shape="Condition",member="NotMatches")`), while no other
//! member of the shape is narrowed that way
//! (`smithy_query(shape="Condition")`).
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. An operation that QUERIES with a `FindingCriteria` refuses a criterion applying a
//!    member the model offers only to the filter APIs, with HTTP 400 under this
//!    service's client-rejection class and its fixed invalid-value sentence for the
//!    member the request carried — whatever field the criterion keys on, whichever of
//!    the two members it applies, whatever the pattern list holds, and however much
//!    state the detector holds.
//! 2. Every OTHER member of the shape stays available on those same reads, so the
//!    refusal is scoped to what the model narrows and not to pattern-shaped requests in
//!    general.
//! 3. The filter APIs accept the narrowed members and store them: the saved filter
//!    reports back the criterion the create or the update gave it.
//! 4. A saved filter APPLIES them, with `*` standing for any run of characters, so the
//!    conditions are implemented where the model puts them rather than merely echoed.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    all_finding_ids, call, enabled_detector, error_code, new_router, Router,
};
// The criterion builder and the create call the criteria suites share, so every suite
// asserting a rule about a `FindingCriteria` builds one the same way.
use crate::negative_class_rejections::{create_filter, criteria};

/// The client-rejection class every refusal in this slice carries.
const INVALID_INPUT: &str = "InvalidInputException";

/// The `FilterAction` member that archives the findings a filter selects
/// (`smithy_query(shape="FilterAction")`), which is how a saved filter's selection is
/// observable through the findings reads.
const ACTION_ARCHIVE: &str = "ARCHIVE";

/// The finding types the pattern rows run over: three types whose values differ in
/// prefix, infix and suffix, so a pattern can select a proper subset of them.
const SAMPLE_TYPES: [&str; 3] = [
    "Recon:EC2/PortProbeUnprotectedPort",
    "Backdoor:EC2/C&CActivity.B",
    "Discovery:S3/MaliciousIPCaller",
];

/// A field the model lists as available for filtering findings that no finding this
/// emulator stores carries a value for
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="FindingCriteria")`).
const UNPOPULATED_FIELD: &str = "resource.instanceDetails.instanceId";

/// This service's fixed invalid-value sentence for a structured member it will not
/// accept — the one answer every unacceptable `findingCriteria` gets, whichever way it
/// is unacceptable, with nothing from inside the structure echoed.
fn invalid_parameter_value(member: &str) -> String {
    format!("The request was rejected because the parameter {member} has an invalid value.")
}

/// The message an error response carries.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

async fn list_findings(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings"),
        Some(body),
    )
    .await
}

async fn findings_statistics(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/statistics"),
        Some(body),
    )
    .await
}

async fn get_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn update_filter(router: &Router, detector_id: &str, name: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body),
    )
    .await
}

/// Every way a findings read can be asked to apply a `FindingCriteria`: the listing, and
/// both shapes of statistics answer (`smithy_query(operation="ListFindings")`,
/// `smithy_query(operation="GetFindingsStatistics")`).
async fn findings_queries(
    router: &Router,
    detector_id: &str,
    criteria: &Value,
) -> Vec<(&'static str, u16, Value)> {
    let (list_status, list_body) =
        list_findings(router, detector_id, json!({"findingCriteria": criteria})).await;
    let (types_status, types_body) = findings_statistics(
        router,
        detector_id,
        json!({
            "findingStatisticTypes": ["COUNT_BY_SEVERITY"],
            "findingCriteria": criteria,
        }),
    )
    .await;
    let (grouped_status, grouped_body) = findings_statistics(
        router,
        detector_id,
        json!({"groupBy": "SEVERITY", "findingCriteria": criteria}),
    )
    .await;
    vec![
        ("ListFindings", list_status, list_body),
        (
            "GetFindingsStatistics (findingStatisticTypes)",
            types_status,
            types_body,
        ),
        (
            "GetFindingsStatistics (groupBy)",
            grouped_status,
            grouped_body,
        ),
    ]
}

/// Generate one sample finding per entry of `types`.
async fn create_samples(router: &Router, detector_id: &str, types: &[&str]) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings {types:?}: expected 200, got {status} {body:?}"
    );
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`].
async fn detector_with_findings(router: &Router) -> String {
    let detector_id = enabled_detector(router).await;
    create_samples(router, &detector_id, &SAMPLE_TYPES).await;
    let ids = all_finding_ids(router, &detector_id, 50).await;
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "expected one finding per sample type, got {ids:?}"
    );
    detector_id
}

/// The finding TYPES whose findings are archived, read back through `GetFindings` — the
/// set a saved archiving filter selected.
async fn archived_types(router: &Router, detector_id: &str) -> BTreeSet<String> {
    let ids = all_finding_ids(router, detector_id, 50).await;
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "get-findings: expected 200, got {status} {body:?}"
    );
    body["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| finding["service"]["archived"].as_bool().unwrap_or(false))
        .map(|finding| finding["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The expected archived set, by index into [`SAMPLE_TYPES`].
fn types_at(indices: &[usize]) -> BTreeSet<String> {
    indices
        .iter()
        .map(|index| SAMPLE_TYPES[*index].to_string())
        .collect()
}

/// The indices of [`SAMPLE_TYPES`] that are not in `indices`.
fn complement_of(indices: &[usize]) -> BTreeSet<String> {
    (0..SAMPLE_TYPES.len())
        .filter(|index| !indices.contains(index))
        .map(|index| SAMPLE_TYPES[index].to_string())
        .collect()
}

/// Criteria applying a member the model offers only to the filter APIs: (label, the
/// criteria, a text the refusal must not echo).
///
/// The rows differ along which of the two members is applied, the field the criterion
/// keys on (a top-level scalar, another scalar, a field of a resource detail document),
/// the number of patterns in the list (one, several, none at all), the form of the
/// pattern (a literal, a prefix, the bare wildcard), and whether the criterion carries
/// anything else beside the narrowed member (a member of the same condition, another
/// criterion key, the other narrowed member).
fn filter_only_rows() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "matches with one literal pattern",
            criteria("type", json!({"matches": [SAMPLE_TYPES[0]]})),
            SAMPLE_TYPES[0],
        ),
        (
            "matches with a prefix pattern",
            criteria("type", json!({"matches": ["Recon:*"]})),
            "Recon:*",
        ),
        (
            "matches with several patterns",
            criteria("type", json!({"matches": ["Recon:*", "Discovery:*"]})),
            "Discovery:*",
        ),
        (
            "matches with the bare wildcard on another field",
            criteria("accountId", json!({"matches": ["*"]})),
            "accountId",
        ),
        (
            "matches with an empty pattern list",
            criteria("type", json!({"matches": []})),
            "matches",
        ),
        (
            "matches on a field of a resource detail document",
            criteria(UNPOPULATED_FIELD, json!({"matches": ["i-*"]})),
            "i-*",
        ),
        (
            "notMatches with one pattern",
            criteria("region", json!({"notMatches": ["us-*"]})),
            "us-*",
        ),
        (
            "notMatches with several patterns",
            criteria("type", json!({"notMatches": ["Recon:*", "*S3*"]})),
            "*S3*",
        ),
        (
            "notMatches with an empty pattern list",
            criteria("type", json!({"notMatches": []})),
            "notMatches",
        ),
        (
            "matches beside an accepted member of the same condition",
            criteria(
                "type",
                json!({"equals": [SAMPLE_TYPES[1]], "matches": ["*"]}),
            ),
            SAMPLE_TYPES[1],
        ),
        (
            "a second criterion key carrying the narrowed member",
            json!({"criterion": {
                "severity": {"greaterThanOrEqual": 1},
                "type": {"notMatches": ["*"]},
            }}),
            "notMatches",
        ),
        (
            "both narrowed members in one condition",
            criteria(
                "type",
                json!({"matches": ["*"], "notMatches": ["Discovery:*"]}),
            ),
            "Discovery:*",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: a findings read refuses a condition the model offers only to the
// filter APIs.
// ---------------------------------------------------------------------------

/// Each row is run against every findings read, and against a detector holding findings
/// as well as one holding none: whether the condition is available is a property of the
/// OPERATION, so it cannot depend on what a pattern would have selected.
#[tokio::test]
async fn a_findings_read_refuses_a_condition_the_model_offers_only_to_the_filter_apis() {
    let router = new_router();
    let populated = detector_with_findings(&router).await;
    let empty = {
        // A second Region's endpoint would hold a second detector; a second router is
        // the simpler way to a detector with no findings at all.
        let other = new_router();
        let id = enabled_detector(&other).await;
        (other, id)
    };

    for (label, criteria, needle) in filter_only_rows() {
        for (detector_label, router, detector_id) in [
            ("a detector holding findings", &router, populated.as_str()),
            ("a detector holding none", &empty.0, empty.1.as_str()),
        ] {
            for (operation, status, body) in findings_queries(router, detector_id, &criteria).await {
                let case = format!("{label} on {operation}, {detector_label}");
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the condition to be refused, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    INVALID_INPUT,
                    "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
                );
                assert_eq!(
                    message(&body),
                    invalid_parameter_value("findingCriteria"),
                    "case {case:?}: expected the invalid-value sentence for findingCriteria, got {body:?}"
                );
                assert!(
                    !body.to_string().contains(needle),
                    "case {case:?}: the refusal must echo nothing of the criterion, got {body:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: every other member of the shape stays available on the reads.
// ---------------------------------------------------------------------------

/// The rows are the remaining members of `Condition` (`smithy_query(shape="Condition")`)
/// — the equality lists, the four orderings and the four deprecated spellings of each —
/// so the refusal above is scoped to the two members the model narrows and nothing else.
#[tokio::test]
async fn every_other_condition_member_stays_available_on_a_findings_read() {
    let router = new_router();
    let detector_id = detector_with_findings(&router).await;

    // (the field the member is applied to, the condition)
    let rows: [(&str, Value); 12] = [
        ("type", json!({"equals": [SAMPLE_TYPES[0]]})),
        ("type", json!({"notEquals": [SAMPLE_TYPES[0]]})),
        ("type", json!({"eq": [SAMPLE_TYPES[2]]})),
        ("type", json!({"neq": [SAMPLE_TYPES[2]]})),
        ("severity", json!({"greaterThan": 1})),
        ("severity", json!({"greaterThanOrEqual": 1})),
        ("severity", json!({"lessThan": 9})),
        ("severity", json!({"lessThanOrEqual": 9})),
        ("severity", json!({"gt": 1})),
        ("severity", json!({"gte": 1})),
        ("severity", json!({"lt": 9})),
        ("severity", json!({"lte": 9})),
    ];

    for (field, condition) in rows {
        let criteria = criteria(field, condition.clone());
        for (operation, status, body) in findings_queries(&router, &detector_id, &criteria).await {
            assert_eq!(
                status, 200,
                "case {field:?} under {condition} on {operation}: expected the member to stay available, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the filter APIs accept the narrowed members and store them.
// ---------------------------------------------------------------------------

/// The same rows the reads refuse are accepted by `CreateFilter` and `UpdateFilter`, and
/// the saved filter reports back the criterion it was given.
#[tokio::test]
async fn the_filter_apis_accept_the_conditions_a_findings_read_refuses() {
    let router = new_router();
    let detector_id = detector_with_findings(&router).await;

    for (index, (label, narrowed, _)) in filter_only_rows().into_iter().enumerate() {
        let name = format!("filter-only-{index}");
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({"name": name, "findingCriteria": narrowed}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: create-filter expected 200, got {status} {body:?}"
        );
        let (status, saved) = get_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-filter expected 200, got {status} {saved:?}"
        );
        assert_eq!(
            saved["findingCriteria"],
            crate::filter_condition_spellings::reported_criteria(&narrowed),
            "case {label:?}: expected the saved filter to report the criteria the create stored, got {saved:?}"
        );

        // An update may write the same criteria over a filter created with none of
        // them, so the two filter APIs agree on what a filter may carry.
        let other = format!("updated-{index}");
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({
                "name": other,
                "findingCriteria": criteria("type", json!({"equals": [SAMPLE_TYPES[0]]})),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: create-filter (baseline) expected 200, got {status} {body:?}"
        );
        let (status, body) = update_filter(
            &router,
            &detector_id,
            &other,
            json!({"findingCriteria": narrowed}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: update-filter expected 200, got {status} {body:?}"
        );
        let (status, saved) = get_filter(&router, &detector_id, &other).await;
        assert_eq!(
            status, 200,
            "case {label:?}: get-filter after the update expected 200, got {status} {saved:?}"
        );
        assert_eq!(
            saved["findingCriteria"],
            crate::filter_condition_spellings::reported_criteria(&narrowed),
            "case {label:?}: expected the updated filter to report the criteria the update stored, got {saved:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a saved filter APPLIES the narrowed members.
// ---------------------------------------------------------------------------

/// `*` stands for any run of characters (including none), so the pattern forms a caller
/// can write are what the rows span: a literal that is the whole value, a literal that
/// is only a prefix of it, a prefix pattern, a suffix, an infix, several wildcards, the
/// bare wildcard and a pattern nothing matches.
///
/// Each row is asserted twice over: `matches` selects exactly the named findings and
/// `notMatches` selects exactly the others, so the two members cannot both select the
/// same set. The selection is observed through the archiving action — the saved filter's
/// own effect on the store — because the reads do not offer these members.
#[tokio::test]
async fn a_saved_filter_applies_the_wildcard_pattern_it_was_given() {
    // (pattern, the indices of SAMPLE_TYPES it matches)
    let rows: [(&str, &[usize]); 8] = [
        (SAMPLE_TYPES[0], &[0]),            // a literal, the whole value
        ("Recon:EC2/PortProbe", &[]),       // a literal that is only a prefix
        ("Recon:*", &[0]),                  // a prefix
        ("*IPCaller", &[2]),                // a suffix
        ("*EC2*", &[0, 1]),                 // an infix
        ("*:*/*", &[0, 1, 2]),              // several wildcards
        ("*", &[0, 1, 2]),                  // the bare wildcard
        ("*NoSuchFindingFamily*", &[]),     // a pattern nothing matches
    ];

    for (pattern, selected) in rows {
        for member in ["matches", "notMatches"] {
            let router = new_router();
            let detector_id = detector_with_findings(&router).await;
            let (status, body) = create_filter(
                &router,
                &detector_id,
                json!({
                    "name": "pattern-under-test",
                    "action": ACTION_ARCHIVE,
                    "findingCriteria": criteria("type", json!({member: [pattern]})),
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {member} {pattern:?}: create-filter expected 200, got {status} {body:?}"
            );

            let expected = if member == "matches" {
                types_at(selected)
            } else {
                complement_of(selected)
            };
            let archived = archived_types(&router, &detector_id).await;
            assert_eq!(
                archived, expected,
                "case {member} {pattern:?}: expected exactly {expected:?} selected, got {archived:?}"
            );
        }
    }
}

/// A finding carries NO VALUE for a field this emulator's records do not populate, and
/// an absent value matches no pattern: so a pattern on such a field selects nothing and
/// its negation selects every finding. Asserted on the filter surface, the one that
/// offers the pattern members.
#[tokio::test]
async fn a_pattern_on_a_field_no_finding_carries_selects_nothing_and_excludes_nothing() {
    // (the condition, the indices of SAMPLE_TYPES it selects)
    let rows: [(Value, &[usize]); 4] = [
        (json!({"matches": ["*"]}), &[]),
        (json!({"matches": ["i-*"]}), &[]),
        (json!({"notMatches": ["*"]}), &[0, 1, 2]),
        (json!({"notMatches": ["i-0123456789abcdef0"]}), &[0, 1, 2]),
    ];

    for (condition, selected) in rows {
        let router = new_router();
        let detector_id = detector_with_findings(&router).await;
        let (status, body) = create_filter(
            &router,
            &detector_id,
            json!({
                "name": "unpopulated-field",
                "action": ACTION_ARCHIVE,
                "findingCriteria": criteria(UNPOPULATED_FIELD, condition.clone()),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {condition} on {UNPOPULATED_FIELD}: create-filter expected 200, got {status} {body:?}"
        );

        let expected = types_at(selected);
        let archived = archived_types(&router, &detector_id).await;
        assert_eq!(
            archived, expected,
            "case {condition} on {UNPOPULATED_FIELD}: expected exactly {expected:?} selected, got {archived:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! The values a `FindingCriteria` condition may carry, end to end over the wire.
//!
//! The rule under test, asserted over a SET of inputs rather than one recorded call:
//!
//! > A criterion that asserts a finding field EQUALS a value that field cannot hold
//! > is refused — HTTP 400 under the `InvalidInputException` class, with the
//! > invalid-condition-values sentence naming the member that carried the criteria
//! > and echoing nothing of the value — on every operation handed a
//! > `FindingCriteria`, and nothing is saved. A field this service resolves as a
//! > NUMBER cannot hold a value that is not one, and the severity field cannot hold a
//! > number off GuardDuty's severity scale
//! > (`state::FINDING_SEVERITY_MIN`..=`state::FINDING_SEVERITY_MAX`); a field
//! > resolved as text, a field whose value is an instant (two admissible forms) and
//! > a field a stored record does not populate hold any value, and a NEGATION or an
//! > ordering THRESHOLD makes no such assertion, so neither is held to the domain.
//!
//! The rows below span the published bounds of the scale and one past each, the ways
//! a value fails to be a number, the condition members that do and do not claim a
//! value of the field, and the four operations that accept a `FindingCriteria`
//! (`CreateFilter`, `UpdateFilter`, `ListFindings`, `GetFindingsStatistics`).
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, so these
//! tests exercise routing, restJson1 (de)serialization, modelled-constraint
//! validation and the handlers together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    invalid_condition_values_message, FINDING_SEVERITY_MAX, FINDING_SEVERITY_MIN,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The wire name of the member that carries the criteria on all four operations
/// (`smithy_query(operation="CreateFilter")` renders the `jsonName`
/// `findingCriteria`).
const FINDING_CRITERIA_MEMBER: &str = "findingCriteria";

/// The name of the filter `UpdateFilter` is driven against below.
const BASELINE_FILTER: &str = "criteria-baseline";

/// A criteria structure keyed by `field` with `condition`.
fn criteria(field: &str, condition: Value) -> Value {
    json!({"criterion": {field: condition}})
}

/// Criteria no operation may accept: each asserts a field equals a value the field
/// cannot hold.
///
/// The severity rows sit one step past each published bound of the scale and well
/// beyond it; the others name a value that is no number at all on a field this
/// service resolves as one, and the last two use the deprecated `eq` member, which
/// makes the same claim as `equals`.
fn refused_criteria() -> Vec<(String, Value)> {
    let below = FINDING_SEVERITY_MIN - 0.1;
    let above = FINDING_SEVERITY_MAX + 0.1;
    vec![
        (
            "severity one step below the bottom of the scale".to_string(),
            criteria("severity", json!({"equals": [format!("{below}")]})),
        ),
        (
            "severity at zero".to_string(),
            criteria("severity", json!({"equals": ["0"]})),
        ),
        (
            "severity one step above the top of the scale".to_string(),
            criteria("severity", json!({"equals": [format!("{above}")]})),
        ),
        (
            "severity well above the scale".to_string(),
            criteria("severity", json!({"equals": ["15"]})),
        ),
        (
            "a negative severity".to_string(),
            criteria("severity", json!({"equals": ["-5"]})),
        ),
        (
            "a severity that is no number".to_string(),
            criteria("severity", json!({"equals": ["high"]})),
        ),
        (
            "an in-scale severity beside an off-scale one".to_string(),
            criteria("severity", json!({"equals": ["5", "15"]})),
        ),
        (
            "a confidence that is no number".to_string(),
            criteria("confidence", json!({"equals": ["very"]})),
        ),
        (
            "a service.count that is no number".to_string(),
            criteria("service.count", json!({"equals": ["many"]})),
        ),
        (
            "an off-scale severity under the deprecated eq member".to_string(),
            criteria("severity", json!({"eq": ["42"]})),
        ),
        (
            "an empty severity value".to_string(),
            criteria("severity", json!({"equals": [""]})),
        ),
        (
            "an off-scale severity beside an acceptable criterion".to_string(),
            json!({"criterion": {
                "severity": {"equals": ["11"]},
                "type": {"equals": ["Recon:EC2/Portscan"]}
            }}),
        ),
    ]
}

/// Criteria every operation must accept: either the value IS one of the field's, or
/// the condition makes no claim that the field takes it.
fn accepted_criteria() -> Vec<(String, Value)> {
    vec![
        (
            "severity at the bottom of the scale".to_string(),
            criteria("severity", json!({"equals": [format!("{FINDING_SEVERITY_MIN}")]})),
        ),
        (
            "severity at the top of the scale".to_string(),
            criteria("severity", json!({"equals": [format!("{FINDING_SEVERITY_MAX}")]})),
        ),
        (
            "an integral severity inside the scale".to_string(),
            criteria("severity", json!({"equals": ["5"]})),
        ),
        (
            "a fractional severity inside the scale".to_string(),
            criteria("severity", json!({"equals": ["8.5"]})),
        ),
        (
            "several in-scale severities at once".to_string(),
            criteria("severity", json!({"equals": ["2", "5", "8"]})),
        ),
        (
            "an off-scale severity as an EXCLUSION, which claims nothing".to_string(),
            criteria("severity", json!({"notEquals": ["15"]})),
        ),
        (
            "an off-scale severity as a THRESHOLD, which claims nothing".to_string(),
            criteria("severity", json!({"greaterThan": 10})),
        ),
        (
            "a threshold below the scale".to_string(),
            criteria("severity", json!({"lessThan": 0})),
        ),
        (
            "a numeric confidence".to_string(),
            criteria("confidence", json!({"equals": ["7"]})),
        ),
        (
            "a numeric service.count".to_string(),
            criteria("service.count", json!({"equals": ["3"]})),
        ),
        (
            "a timestamp as epoch milliseconds".to_string(),
            criteria("createdAt", json!({"equals": ["1486685375000"]})),
        ),
        (
            "a timestamp as the text a finding reports".to_string(),
            criteria("updatedAt", json!({"equals": ["2026-01-01T00:00:00.000Z"]})),
        ),
        (
            "any value at all on a text field".to_string(),
            criteria("type", json!({"equals": ["Recon:EC2/Portscan"]})),
        ),
        (
            "any value at all on a field a stored record does not populate".to_string(),
            criteria(
                "resource.instanceDetails.instanceId",
                json!({"equals": ["42"]}),
            ),
        ),
        (
            "an empty criterion map".to_string(),
            json!({"criterion": {}}),
        ),
    ]
}

/// The four operations handed a `FindingCriteria`: the case, the request path after
/// the detector id, and the members the request carries beside the criteria.
fn criteria_operations(detector: &str) -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "CreateFilter",
            format!("/detector/{detector}/filter"),
            json!({"name": "value-domain"}),
        ),
        (
            "UpdateFilter",
            format!("/detector/{detector}/filter/{BASELINE_FILTER}"),
            json!({}),
        ),
        (
            "ListFindings",
            format!("/detector/{detector}/findings"),
            json!({}),
        ),
        (
            "GetFindingsStatistics",
            format!("/detector/{detector}/findings/statistics"),
            json!({"groupBy": "SEVERITY"}),
        ),
    ]
}

/// A request body carrying `criteria` plus the operation's own members.
fn request_with(extra: &Value, criteria: &Value) -> Value {
    let mut body = extra.clone();
    body[FINDING_CRITERIA_MEMBER] = criteria.clone();
    body
}

/// Create the filter `UpdateFilter` is driven against.
async fn baseline_filter(router: &Router, detector: &str) {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector}/filter"),
        Some(json!({
            "name": BASELINE_FILTER,
            "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 1}}}
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the baseline filter: expected 200, got {status} {body:?}"
    );
}

/// The names `ListFilters` reports.
async fn filter_names(router: &Router, detector: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", &format!("/detector/{detector}/filter"), None).await;
    assert_eq!(
        status, 200,
        "list-filters: expected 200, got {status} {body:?}"
    );
    body["filterNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// The criteria `GetFilter` reports for the baseline filter.
async fn saved_criteria(router: &Router, detector: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector}/filter/{BASELINE_FILTER}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-filter: expected 200, got {status} {body:?}"
    );
    body["findingCriteria"].clone()
}

/// Assert a response is the classified HTTP 400 carrying the
/// invalid-condition-values sentence, echoing nothing of `sent`.
fn assert_condition_value_refusal(case: &str, status: u16, body: &Value, sent: &[&str]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    let expected = invalid_condition_values_message(FINDING_CRITERIA_MEMBER);
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
    for value in sent {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo the caller's value {value:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A value the field cannot hold is refused by every operation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_condition_value_the_field_cannot_hold_is_refused_everywhere() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    baseline_filter(&router, &detector).await;
    let saved = saved_criteria(&router, &detector).await;

    for (why, criteria) in refused_criteria() {
        for (operation, path, extra) in criteria_operations(&detector) {
            let case = format!("{operation}: {why}");
            let (status, body) =
                call(&router, "POST", &path, Some(request_with(&extra, &criteria))).await;
            assert_condition_value_refusal(&case, status, &body, &["15", "high", "42"]);
        }

        // Nothing was saved: the refused create left no filter, and the refused
        // update left the baseline filter's own criteria where they were.
        let names = filter_names(&router, &detector).await;
        assert_eq!(
            names,
            vec![BASELINE_FILTER.to_string()],
            "case {why:?}: a refused create must save no filter, got {names:?}"
        );
        let still = saved_criteria(&router, &detector).await;
        assert_eq!(
            still, saved,
            "case {why:?}: a refused update must leave the saved criteria alone, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A value the field CAN hold, and a condition that claims nothing, are served.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_condition_the_field_admits_is_served_by_every_operation() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    baseline_filter(&router, &detector).await;

    // A finding to count, so the reads are exercised over a populated detector as
    // well as the saved-filter path.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector}/findings/create"),
        Some(json!({"findingTypes": ["Recon:EC2/Portscan"]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings: expected 200, got {status} {body:?}"
    );

    for (index, (why, criteria)) in accepted_criteria().into_iter().enumerate() {
        for (operation, path, mut extra) in criteria_operations(&detector) {
            // Each accepted CreateFilter row needs a name of its own: a repeated
            // name is a different refusal.
            if operation == "CreateFilter" {
                extra["name"] = json!(format!("accepted-{index}"));
            }
            let case = format!("{operation}: {why}");
            let (status, body) =
                call(&router, "POST", &path, Some(request_with(&extra, &criteria))).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the criteria to be served, got {status} {body:?}"
            );
        }

        // What `CreateFilter` accepted is what `GetFilter` reports back: the
        // criteria were saved, not merely answered.
        let name = format!("accepted-{index}");
        let (status, body) = call(
            &router,
            "GET",
            &format!("/detector/{detector}/filter/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why:?}: the saved filter must read back, got {status} {body:?}"
        );
        assert_eq!(
            body["findingCriteria"],
            crate::filter_condition_spellings::reported_criteria(&criteria),
            "case {why:?}: expected the criteria the create accepted, got {:?}",
            body["findingCriteria"]
        );
    }
}

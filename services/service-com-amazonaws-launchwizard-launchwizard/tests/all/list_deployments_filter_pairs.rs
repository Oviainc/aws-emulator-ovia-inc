// SPDX-License-Identifier: CC0-1.0

//! The rule that a filter is a complete name-and-VALUES pair, driven over HTTP through
//! the same router the production emulator serves with.
//!
//! RULE: every element of a filter list has to carry both halves of the pair — a filter
//! name and a `values` list holding at least one value — and a request carrying a filter
//! that does not is refused with the modelled `ValidationException` (HTTP 400) naming
//! the half that is missing. A half-specified filter is never DROPPED, because dropping
//! it answers a broader result set than the caller asked for.
//!
//! Source for the rule: `DeploymentFilter` is "a filter name and value pair that is used
//! to return more specific results", and this service's other filter shape
//! (`DeploymentPatternVersionFilter`) marks both halves `@required` —
//! `smithy_query(query="shape",shape="DeploymentFilter,DeploymentPatternVersionFilter",
//! model_path="services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/\
//! com-amazonaws-launchwizard-launchwizard.json")`.
//!
//! The rows differ along what the rule generalises over: WHICH half is missing, HOW it
//! is missing (an absent member or an empty list), which filter key carries it, the
//! POSITION of the offending filter in the list, and how MANY filters are degenerate at
//! once — including far more of them than the report's own clause cap, which the refusal
//! must not grow past. The complementary half of the rule — a complete pair still
//! narrows — is asserted over its own table, so a refusal cannot be mistaken for
//! "filters stopped working".

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::DEPLOYMENT_FILTER_KEY_VALUES;
use service_com_amazonaws_launchwizard_launchwizard::state::MAX_FILTER_PAIR_CLAUSES;

use crate::deployment_lifecycle::create;
use crate::harness::{deployment_name, Harness};

/// The wire error code and HTTP status a refused filter carries (`ValidationException`,
/// `errorKind: client`, `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// Two deployments of different workloads, so a filter has something to exclude and a
/// dropped filter would be observable as the broader set. Returns the deployment names
/// `ListDeployments` reports for them, in creation order.
async fn seeded(harness: &Harness) -> Vec<String> {
    let rows = [
        ("SAP", "SapHanaSingle", "filter-pair-sap"),
        ("SQLServer", "SqlServerSingle", "filter-pair-sql"),
    ];
    let mut names = Vec::new();
    for (workload, pattern, name) in rows {
        create(harness, name, workload, pattern, name).await;
        names.push(deployment_name(name));
    }
    names
}

/// The deployment names `ListDeployments` reports for `body`.
fn reported_names(case: &str, body: &Value) -> BTreeSet<String> {
    body["deployments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListDeployments reported no deployments: {body}"))
        .iter()
        .map(|deployment| {
            deployment["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a reported deployment carries no name: {deployment}")
                })
                .to_string()
        })
        .collect()
}

/// The clause this service states for a member that is null, as
/// `tests/all/validation_messages.rs` pins the wording.
pub(crate) fn null_clause(path: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
}

/// The clause for a `values` list that is present but names no value.
pub(crate) fn empty_clause(path: &str) -> String {
    format!(
        "Value at '{path}' failed to satisfy constraint: Member must have length greater than \
         or equal to 1"
    )
}

/// The whole report `clauses` compose, in this service's refusal wording: how many
/// members failed, then one clause each.
pub(crate) fn report(clauses: &[String]) -> String {
    let noun = if clauses.len() == 1 { "error" } else { "errors" };
    format!(
        "{} validation {noun} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// The `message` of a refusal body, whichever casing the envelope spells it with.
fn message_of(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in the refusal body {body}"))
        .to_string()
}

/// A complete filter, used by the rows where only ANOTHER filter is degenerate.
fn complete_filter() -> Value {
    json!({"name": "WORKLOAD_NAME", "values": ["SAP"]})
}

#[tokio::test]
async fn a_filter_missing_either_half_of_the_pair_is_refused_and_never_dropped() {
    let harness = Harness::new();
    let seeded = seeded(&harness).await;

    // The baseline the refusals are measured against: with no filter at all, both
    // deployments are reported — so a dropped filter really would be observable as this
    // broader set rather than as an empty one.
    let unfiltered = harness.ok("no filters", "/listDeployments", json!({})).await;
    let expected: BTreeSet<String> = seeded.iter().cloned().collect();
    let got = reported_names("no filters", &unfiltered);
    assert_eq!(
        got, expected,
        "case \"no filters\": expected the whole stored set {expected:?}, got {got:?}"
    );

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a filter naming a key with no values member at all",
            json!([{"name": "WORKLOAD_NAME"}]),
            report(&[null_clause("filters.1.member.values")]),
        ),
        (
            "a filter naming a key with an empty values list",
            json!([{"name": "WORKLOAD_NAME", "values": []}]),
            report(&[empty_clause("filters.1.member.values")]),
        ),
        (
            "the other filter key, with an empty values list",
            json!([{"name": "DEPLOYMENT_STATUS", "values": []}]),
            report(&[empty_clause("filters.1.member.values")]),
        ),
        (
            "a filter carrying values but no name",
            json!([{"values": ["SAP"]}]),
            report(&[null_clause("filters.1.member.name")]),
        ),
        (
            "a filter carrying neither half",
            json!([{}]),
            report(&[
                null_clause("filters.1.member.name"),
                null_clause("filters.1.member.values"),
            ]),
        ),
        (
            "the degenerate filter is the second of two",
            json!([complete_filter(), {"name": "DEPLOYMENT_STATUS", "values": []}]),
            report(&[empty_clause("filters.2.member.values")]),
        ),
        (
            "the degenerate filter is the second of two, missing its name",
            json!([complete_filter(), {"values": ["COMPLETED"]}]),
            report(&[null_clause("filters.2.member.name")]),
        ),
        (
            "both filters are degenerate, each missing a different half",
            json!([{"name": "WORKLOAD_NAME"}, {"values": ["COMPLETED"]}]),
            report(&[
                null_clause("filters.1.member.values"),
                null_clause("filters.2.member.name"),
            ]),
        ),
        (
            // The name's VOCABULARY is judged at the protocol boundary, from the
            // model's own enum (`DeploymentFilterKey`: WORKLOAD_NAME |
            // DEPLOYMENT_STATUS, in that declared order), so a filter whose name is
            // outside it is refused for the name before the pair rule is reached.
            "an unsupported filter name that also carries no values",
            json!([{"name": "PATTERN_NAME"}]),
            report(&[
                "Value at 'filters.1.member.name' failed to satisfy constraint: Member must \
                 satisfy enum value set: [WORKLOAD_NAME, DEPLOYMENT_STATUS]"
                    .to_string(),
            ]),
        ),
    ];

    for (case, filters, expected) in rows {
        let body = harness
            .refused(
                case,
                "/listDeployments",
                json!({"filters": filters}),
                STATUS,
                CODE,
            )
            .await;
        let message = message_of(case, &body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            body.get("deployments").is_none(),
            "case {case:?}: a refused request must report no deployments, got {body}"
        );
    }
}

#[tokio::test]
async fn the_refusal_for_many_degenerate_filters_stays_a_bounded_report() {
    let harness = Harness::new();
    seeded(&harness).await;

    // `DeploymentFilterList` declares a floor and no ceiling, so how many filters a
    // request carries is the caller's choice; the refusal it is answered with is not
    // allowed to grow with that count. Two clauses per filter, over three times the
    // cap, so the report would be far longer if nothing bounded it.
    let degenerate = vec![json!({}); MAX_FILTER_PAIR_CLAUSES * 3];
    let case = "many degenerate filters";
    let body = harness
        .refused(
            case,
            "/listDeployments",
            json!({"filters": degenerate}),
            STATUS,
            CODE,
        )
        .await;
    let message = message_of(case, &body);
    let clauses = message
        .split_once("detected: ")
        .map(|(_, clauses)| clauses.split("; ").count())
        .unwrap_or_else(|| panic!("case {case:?}: not a validation report: {message:?}"));
    assert_eq!(
        clauses, MAX_FILTER_PAIR_CLAUSES,
        "case {case:?}: expected at most {MAX_FILTER_PAIR_CLAUSES} clauses, got {clauses} in \
         {message:?}"
    );
    assert!(
        message.starts_with(&format!("{MAX_FILTER_PAIR_CLAUSES} validation errors detected: ")),
        "case {case:?}: the report must count the clauses it states, got {message:?}"
    );
}

#[tokio::test]
async fn a_complete_filter_pair_still_narrows_the_reported_deployments() {
    let harness = Harness::new();
    let seeded = seeded(&harness).await;
    assert_eq!(
        DEPLOYMENT_FILTER_KEY_VALUES.len(),
        2,
        "the model declares two DeploymentFilterKey values, got {DEPLOYMENT_FILTER_KEY_VALUES:?}"
    );

    // Each key the model declares, over one value and over several (OR-joined), plus a
    // value nothing stored carries — which reports the EMPTY set, never the broad one.
    let rows: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "one workload",
            json!([{"name": "WORKLOAD_NAME", "values": ["SAP"]}]),
            [seeded[0].clone()].into_iter().collect(),
        ),
        (
            "the other workload",
            json!([{"name": "WORKLOAD_NAME", "values": ["SQLServer"]}]),
            [seeded[1].clone()].into_iter().collect(),
        ),
        (
            "both workloads, OR-joined",
            json!([{"name": "WORKLOAD_NAME", "values": ["SAP", "SQLServer"]}]),
            seeded.iter().cloned().collect(),
        ),
        (
            "a workload nothing was deployed for",
            json!([{"name": "WORKLOAD_NAME", "values": ["ActiveDirectory"]}]),
            BTreeSet::new(),
        ),
        (
            "a status no stored deployment has reached",
            json!([{"name": "DEPLOYMENT_STATUS", "values": ["DELETED"]}]),
            BTreeSet::new(),
        ),
    ];

    for (case, filters, expected) in rows {
        let body = harness
            .ok(case, "/listDeployments", json!({"filters": filters}))
            .await;
        let got = reported_names(case, &body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected deployments {expected:?}, got {got:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `GetEnabledControl` as a REPORTING operation, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches
//! with, so what these tests exercise is the wire behaviour a client sees — status codes
//! and JSON bodies — not the handlers' Rust signatures.
//!
//! The sibling suite `enabled_control_lifecycle.rs` pins what ONE enablement round-trips
//! to while it is the only record in the store. The rules under test here are the two
//! that only a POPULATED store can show, each asserted over a TABLE of inputs:
//!
//! * the identifier selects ONE record: whatever else the store holds, the report is the
//!   configuration the enablement that minted that identifier was given, so no other
//!   record's control, target or parameters appear in it;
//! * the operation is `@readonly`
//!   (`smithy_query(query="operation", operation="GetEnabledControl")` on this crate's
//!   `smithy-model/`), so reading a record — once, repeatedly, or interleaved with reads
//!   of other records — leaves what a later caller sees unchanged.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::{
    control, details, enable, enabled_control_prefix, entries, ou,
};
use crate::landing_zone_lifecycle::{call, past_the_settle_span, set_up_emulator};

/// One enablement a case asks for: a label, the control, the target and the `parameters`
/// list `EnableControl` is given (`Value::Null` for a request that carries no
/// `parameters` member at all).
type Enablement<'a> = (&'a str, String, String, serde_json::Value);

/// `EnableControl` for `spec`, handing back the ARN it minted.
async fn enable_spec(router: &MultiServiceRouter, spec: &Enablement<'_>) -> String {
    let (_, control_identifier, target_identifier, parameters) = spec;
    let mut body = serde_json::json!({
        "controlIdentifier": control_identifier,
        "targetIdentifier": target_identifier,
    });
    if !parameters.is_null() {
        body["parameters"] = parameters.clone();
    }
    enable(router, body).await.0
}

/// The enablements every case in this suite populates the store with: they differ in the
/// control, in the target organizational unit, and in the parameter list's entry count and
/// `Document` kinds — the dimensions `GetEnabledControl`'s report is read off. A
/// configuration names only keys its control declares, so the rows that carry one name the
/// control this emulator's catalog publishes with parameters.
fn populated_store() -> Vec<Enablement<'static>> {
    vec![
        (
            "no parameters member, OU target",
            control("AWS-GR_ENCRYPTED_VOLUMES"),
            ou("1111"),
            serde_json::Value::Null,
        ),
        (
            "an empty parameter list, another OU",
            control("AWS-GR_RESTRICT_ROOT_USER"),
            ou("2222"),
            serde_json::json!([]),
        ),
        (
            "one declared parameter",
            control("AWS-GR_REGION_DENY"),
            ou("4444"),
            serde_json::json!([{ "key": "AllowedRegions", "value": "us-west-2" }]),
        ),
        (
            "three declared parameters of three Document kinds",
            control("AWS-GR_REGION_DENY"),
            ou("5555"),
            serde_json::json!([
                { "key": "AllowedRegions", "value": ["us-west-2", "eu-west-1"] },
                { "key": "ExemptedActions", "value": false },
                { "key": "ExemptedPrincipalArns", "value": { "inner": ["a", "b"] } }
            ]),
        ),
        (
            // The same control as the first row, on another target: the record is keyed
            // by the enablement, not by the control, so both must stay resolvable. That
            // control declares no parameters, so neither row configures one.
            "a control already enabled on another target",
            control("AWS-GR_ENCRYPTED_VOLUMES"),
            ou("3333"),
            serde_json::Value::Null,
        ),
    ]
}

/// RULE: `enabledControlIdentifier` selects one stored enablement, and the report is that
/// enablement's own persisted configuration whatever else the store holds.
#[tokio::test]
async fn each_identifier_reports_its_own_enablement_among_many() {
    let router = set_up_emulator().await;
    let specs = populated_store();

    let mut minted: Vec<String> = Vec::new();
    for spec in &specs {
        minted.push(enable_spec(&router, spec).await);
    }

    let mut distinct = minted.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        specs.len(),
        "expected one distinct ARN per enablement, got {minted:?}"
    );

    for (spec, arn) in specs.iter().zip(&minted) {
        let (case, control_identifier, target_identifier, parameters) = spec;
        let detail = details(&router, arn).await;

        for (member, expected) in [
            ("arn", serde_json::json!(arn)),
            ("controlIdentifier", serde_json::json!(control_identifier)),
            ("targetIdentifier", serde_json::json!(target_identifier)),
        ] {
            assert_eq!(
                detail.get(member).cloned().unwrap_or_default(),
                expected,
                "case {case:?}: {member} expected {expected:?}, got {:?}",
                detail.get(member)
            );
        }
        assert_eq!(
            entries(detail.get("parameters").unwrap_or(&serde_json::Value::Null)),
            entries(parameters),
            "case {case:?}: expected the parameters this enablement was given, in order, \
             got {:?}",
            detail.get("parameters")
        );
        assert!(
            arn.starts_with(&enabled_control_prefix()),
            "case {case:?}: expected an enabledcontrol ARN under {:?}, got {arn:?}",
            enabled_control_prefix()
        );
    }
}

/// RULE: `GetEnabledControl` is `@readonly`, so reading a record leaves what a later
/// caller sees unchanged — however many reads are made, of this record or of its
/// neighbours.
///
/// The rows are the read patterns: one read, the same record twice, and a read of every
/// neighbour between two reads of the subject.
#[tokio::test]
async fn reading_an_enablement_changes_nothing_a_later_caller_sees() {
    let cases = [
        ("one read", 1, false),
        ("two reads", 2, false),
        ("reads interleaved with its neighbours", 2, true),
    ];
    for (case, repeats, interleave_neighbours) in cases {
        let router = set_up_emulator().await;
        let specs = populated_store();
        let mut minted: Vec<String> = Vec::new();
        for spec in &specs {
            minted.push(enable_spec(&router, spec).await);
        }

        // Past the settle span every enablement is terminal, so the whole report — not
        // just the members a read cannot touch — is expected to be identical across
        // reads.
        tokio::time::sleep(past_the_settle_span()).await;

        let subject = minted[2].clone();
        let before = listing(&router).await;
        let first = details(&router, &subject).await;

        for _ in 1..repeats {
            if interleave_neighbours {
                for neighbour in minted.iter().filter(|arn| *arn != &subject) {
                    details(&router, neighbour).await;
                }
            }
            let again = details(&router, &subject).await;
            assert_eq!(
                again, first,
                "case {case:?}: expected every read to report the same record, got \
                 {again:?} after {first:?}"
            );
        }

        let after = listing(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the listing to be unchanged by the reads, got \
             {after:?} after {before:?}"
        );
    }
}

/// Every stored enablement as `ListEnabledControls` reports it, sorted by ARN so a case
/// compares the stored SET rather than the order a page happened to come back in.
async fn listing(router: &MultiServiceRouter) -> Vec<serde_json::Value> {
    let listed = call(
        router,
        "/list-enabled-controls",
        serde_json::json!({ "maxResults": 200 }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "list-enabled-controls: expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    let mut summaries = entries(listed.get("enabledControls"));
    summaries.sort_by_key(|summary| {
        summary
            .get("arn")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    summaries
}

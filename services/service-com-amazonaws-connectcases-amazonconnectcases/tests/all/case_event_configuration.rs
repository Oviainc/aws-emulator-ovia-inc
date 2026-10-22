// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for a domain's case-event publishing
//! configuration (`PutCaseEventConfiguration` / `GetCaseEventConfiguration`),
//! driven through the real `MultiServiceRouter` so every assertion goes over the
//! wire the SDK speaks.
//!
//! The rule under test: a domain's case-event publishing configuration is a
//! property of that domain which `PutCaseEventConfiguration` REPLACES with the
//! `eventBridge` the request carries and `GetCaseEventConfiguration` reports
//! back verbatim — a domain nothing has configured reports delivery disabled,
//! each domain carries its own configuration, and a `domainId` the store does
//! not hold is refused with `ResourceNotFoundException` (404) by both
//! operations.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::MAX_EVENT_INCLUDED_FIELD_IDS;

use crate::case_lifecycle::{domain, Emulator};

/// The URI both operations bind to for `domain_id`
/// (`smithy_query(query="operation", operation="PutCaseEventConfiguration")`:
/// `PUT /domains/{domainId}/case-event-configuration`; the read is the same URI
/// under `POST`).
fn path(domain_id: &str) -> String {
    format!("/domains/{domain_id}/case-event-configuration")
}

/// `PutCaseEventConfiguration` with `configuration` as the whole `eventBridge`.
async fn put(emulator: &Emulator, case: &str, domain_id: &str, configuration: &Value) -> Value {
    emulator
        .ok(
            case,
            "PUT",
            &path(domain_id),
            json!({ "eventBridge": configuration }),
        )
        .await
}

/// `GetCaseEventConfiguration`'s `eventBridge` member.
async fn get(emulator: &Emulator, case: &str, domain_id: &str) -> Value {
    let response = emulator
        .ok(case, "POST", &path(domain_id), json!({}))
        .await;
    response
        .get("eventBridge")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the read reported no eventBridge: {response:?}"))
}

/// The configurations the round trip is asserted over: every shape the model
/// allows an `EventBridgeConfiguration` to take
/// (`smithy_query(query="shape", shape="EventBridgeConfiguration,EventIncludedData,CaseEventIncludedData,RelatedItemEventIncludedData")`
/// — `enabled` required boolean, optional `includedData` carrying an optional
/// `caseData.fields` list of field identifiers and an optional
/// `relatedItemData.includeContent` boolean). The rows differ along the boolean
/// values, whether `includedData` is present, which of its two members are
/// present, and how many field identifiers the list carries.
fn configurations() -> Vec<(&'static str, Value)> {
    vec![
        ("delivery off, no included data", json!({ "enabled": false })),
        ("delivery on, no included data", json!({ "enabled": true })),
        (
            "delivery on, one included case field",
            json!({
                "enabled": true,
                "includedData": { "caseData": { "fields": [{ "id": "title" }] } }
            }),
        ),
        (
            "delivery on, several included case fields",
            json!({
                "enabled": true,
                "includedData": {
                    "caseData": {
                        "fields": [
                            { "id": "title" },
                            { "id": "customer_id" },
                            { "id": "9c4f2b1e-0000-4000-8000-000000000001" }
                        ]
                    }
                }
            }),
        ),
        (
            "delivery on, related item content included",
            json!({
                "enabled": true,
                "includedData": { "relatedItemData": { "includeContent": true } }
            }),
        ),
        (
            "delivery on, both included-data members",
            json!({
                "enabled": true,
                "includedData": {
                    "caseData": { "fields": [{ "id": "status" }] },
                    "relatedItemData": { "includeContent": false }
                }
            }),
        ),
        (
            "delivery off, included data still stored",
            json!({
                "enabled": false,
                "includedData": {
                    "caseData": { "fields": [{ "id": "title" }] },
                    "relatedItemData": { "includeContent": true }
                }
            }),
        ),
    ]
}

/// An `includedData` naming `count` distinct field identifiers.
fn included_fields(count: usize) -> Value {
    let fields: Vec<Value> = (0..count)
        .map(|index| json!({ "id": format!("field-{index}") }))
        .collect();
    json!({ "caseData": { "fields": fields } })
}

#[tokio::test]
async fn a_configuration_that_was_put_is_the_one_the_read_reports() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "event-configuration-round-trip").await;

    // Nothing has been put yet: the domain exists, so the read answers with
    // delivery disabled rather than refusing.
    assert_eq!(
        get(&emulator, "before any put", &domain_id).await,
        json!({ "enabled": false }),
        "case \"before any put\": an unconfigured domain must report delivery disabled"
    );

    // Each row is put against the SAME domain in turn, so every read also
    // asserts the PUT replaced the row before it rather than merging into it.
    for (case, configuration) in configurations() {
        let created = put(&emulator, case, &domain_id, &configuration).await;
        assert_eq!(
            created,
            json!({}),
            "case {case:?}: the write's response carries no members, got {created:?}"
        );
        let reported = get(&emulator, case, &domain_id).await;
        assert_eq!(
            reported, configuration,
            "case {case:?}: expected the stored configuration {configuration:?}, got {reported:?}"
        );
        // A read must not change what a later caller sees.
        let again = get(&emulator, case, &domain_id).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report {reported:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn every_domain_carries_its_own_configuration() {
    let emulator = Emulator::new();
    let configured = domain(&emulator, "configured-domain").await;
    let untouched = domain(&emulator, "untouched-domain").await;

    let configuration = json!({
        "enabled": true,
        "includedData": { "caseData": { "fields": [{ "id": "title" }] } }
    });
    put(&emulator, "configured domain", &configured, &configuration).await;

    assert_eq!(
        get(&emulator, "configured domain", &configured).await,
        configuration,
        "case \"configured domain\": the domain that was put must report that configuration"
    );
    assert_eq!(
        get(&emulator, "untouched domain", &untouched).await,
        json!({ "enabled": false }),
        "case \"untouched domain\": a write to one domain must not configure another"
    );
}

#[tokio::test]
async fn a_domain_the_store_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let absent = "11111111-2222-4333-8444-555555555555";

    for (case, method, body) in [
        (
            "put against an absent domain",
            "PUT",
            Some(json!({ "eventBridge": { "enabled": true } })),
        ),
        ("read of an absent domain", "POST", Some(json!({}))),
    ] {
        let (status, code) = emulator.failure(case, method, &path(absent), body).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {code:?}"
        );
    }
}

#[tokio::test]
async fn the_included_field_list_is_bounded_by_the_models_cap() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "event-configuration-bounds").await;

    // The model's own cap: `CaseEventIncludedData$fields` is `..400`
    // (`smithy_query(query="shape", shape="CaseEventIncludedData")`), restated as
    // `MAX_EVENT_INCLUDED_FIELD_IDS`. Each published bound is exercised, and one
    // past the upper bound.
    for count in [0_usize, 1, MAX_EVENT_INCLUDED_FIELD_IDS] {
        let case = format!("{count} included field ids");
        let configuration = json!({ "enabled": true, "includedData": included_fields(count) });
        put(&emulator, &case, &domain_id, &configuration).await;
        let reported = get(&emulator, &case, &domain_id).await;
        assert_eq!(
            reported, configuration,
            "case {case:?}: expected the stored configuration back, got {reported:?}"
        );
    }

    let case = format!("{} included field ids", MAX_EVENT_INCLUDED_FIELD_IDS + 1);
    let body = json!({
        "eventBridge": {
            "enabled": true,
            "includedData": included_fields(MAX_EVENT_INCLUDED_FIELD_IDS + 1)
        }
    });
    let (status, code) = emulator
        .failure(&case, "PUT", &path(&domain_id), Some(body))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: a list past the cap must be refused with 400 ValidationException, \
         got {status} {code:?}"
    );

    // The refused write left the last accepted configuration in place.
    let case = "the configuration after a refused write";
    let reported = get(&emulator, case, &domain_id).await;
    let expected = json!({
        "enabled": true,
        "includedData": included_fields(MAX_EVENT_INCLUDED_FIELD_IDS)
    });
    assert_eq!(
        reported, expected,
        "case {case:?}: a refused write must store nothing, got {reported:?}"
    );
}

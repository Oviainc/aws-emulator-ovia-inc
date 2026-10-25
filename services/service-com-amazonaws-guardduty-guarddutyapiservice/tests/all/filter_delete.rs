// SPDX-License-Identifier: CC0-1.0

//! `DeleteFilter`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `DeleteFilter` DROPS the named filter off the detector it names: the call answers
//!    HTTP 200 with a body carrying no members (`DeleteFilterResponse` declares none,
//!    `smithy_query(operation="DeleteFilter")`), afterwards every operation that
//!    addresses a filter by name refuses that name — including a second delete — and
//!    `ListFilters` no longer reports it, while the detector's other filters are
//!    unchanged.
//! 2. The record really goes, so the resources it held go with it: the name is free for
//!    a fresh create, and the deleted filter no longer counts against the documented
//!    per-Region filter quota ("The maximum number of saved filters per Amazon Web
//!    Services account per Region is 100",
//!    `smithy_query(query="doc",shape="CreateFilter")`).
//! 3. A filter belongs to one detector, which is regional, so a delete reaches only the
//!    filter held by the detector the request names under the Region the request is
//!    signed for; a same-named filter under another Region's detector survives, and a
//!    delete addressed through the wrong Region is refused and writes nothing.
//!
//! The name forms the rows span — `FilterName` length 3..64 and the alphabet "period
//! (.), underscore (_), dash (-), and alphanumeric characters" — come from
//! `smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`, not from any
//! recorded call.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The only client error GuardDuty's model binds to `DeleteFilter`
/// (`smithy_query(operation="DeleteFilter")`), and the shape restJson1 resolves an
/// error response to through `x-amzn-errortype`.
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// The code the framework names a refusal by when a service states none of its own. No
/// GuardDuty operation declares a shape by this name, so it must never reach a client.
const UNMODELED_CODE: &str = "ValidationException";

/// "The maximum number of saved filters per Amazon Web Services account per Region is
/// 100" (`smithy_query(query="doc",shape="CreateFilter")`).
const FILTER_QUOTA_PER_REGION: usize = 100;

/// A `FindingCriteria` every create can send: `CreateFilter.FindingCriteria` is
/// required (`smithy_query(operation="CreateFilter")`).
fn criteria() -> Value {
    json!({"criterion": {"severity": {"greaterThanOrEqual": 2}}})
}

/// A string of `len` characters drawn from the documented filter-name alphabet, so a
/// length row differs from its neighbours ONLY in length.
fn name_of(len: usize) -> String {
    "a".repeat(len)
}

async fn create_filter_in(router: &Router, region: &str, detector_id: &str, name: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({"name": name, "findingCriteria": criteria()})),
    )
    .await
}

async fn create_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(json!({"name": name, "findingCriteria": criteria()})),
    )
    .await
}

/// Save a filter, failing the test if the create did not succeed.
async fn saved(router: &Router, detector_id: &str, name: &str, case: &str) {
    let (status, body) = create_filter(router, detector_id, name).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateFilter {name:?} expected HTTP 200, got {status} {body:?}"
    );
}

async fn get_filter_in(router: &Router, region: &str, detector_id: &str, name: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn get_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    get_filter_in(router, "us-east-1", detector_id, name).await
}

async fn update_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(json!({"rank": 9})),
    )
    .await
}

async fn delete_filter_in(router: &Router, region: &str, detector_id: &str, name: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await
}

async fn delete_filter(router: &Router, detector_id: &str, name: &str) -> (u16, Value) {
    delete_filter_in(router, "us-east-1", detector_id, name).await
}

/// The names one `ListFilters` page reports, failing the test if the list did not
/// succeed.
async fn list_names(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/filter?maxResults=50"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListFilters expected HTTP 200, got {status} {body:?}"
    );
    body["filterNames"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected filterNames in {body:?}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string name in {body:?}"))
                .to_string()
        })
        .collect()
}

/// Assert one refusal: HTTP 400, the modelled error shape on the wire, and a code the
/// model declares rather than the framework's default.
fn assert_modeled_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_shape(body),
        MODELED_CLIENT_ERROR,
        "case {case}: expected the modelled error shape in x-amzn-errortype, got {body:?}"
    );
    assert_ne!(
        error_code(body),
        UNMODELED_CODE,
        "case {case}: expected a modelled error class, got {body:?}"
    );
}

/// `DeleteFilterResponse` declares no members, so a successful delete answers with an
/// empty body — never an invented field.
fn assert_empty_response(case: &str, body: &Value) {
    let empty = match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case}: expected a DeleteFilter response with no members, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: a delete drops exactly the named filter, and every reader sees it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_filter_drops_exactly_that_filter() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // Siblings created once: every row below must leave them exactly as they are.
    let siblings = ["sibling.one", "sibling_two-3"];
    for sibling in siblings {
        saved(&router, &detector_id, sibling, "the sibling baseline").await;
    }

    // Rows differ in the NAME's form — length at each published bound, and the
    // documented punctuation and digit classes — so what is asserted is the rule and
    // not one name.
    let rows: Vec<(String, &str)> = vec![
        (name_of(3), "the published 3-character minimum"),
        (name_of(64), "the published 64-character maximum"),
        (
            "Dots.and-dashes_9".to_string(),
            "every documented punctuation class, mixed case",
        ),
        ("0123456789".to_string(), "digits only"),
    ];

    for (name, why) in rows {
        let case = format!("{name:?} ({why})");
        saved(&router, &detector_id, &name, &case).await;

        let listed = list_names(&router, &detector_id, &case).await;
        assert!(
            listed.contains(&name),
            "case {case}: expected the saved filter to be listed, got {listed:?}"
        );

        let (status, deleted) = delete_filter(&router, &detector_id, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: DeleteFilter expected HTTP 200, got {status} {deleted:?}"
        );
        assert_empty_response(&case, &deleted);

        // Every operation that addresses a filter by name now refuses it, including a
        // second delete: this service's delete is not idempotent, because the filter
        // must be there to be deleted.
        let (status, refused) = get_filter(&router, &detector_id, &name).await;
        assert_modeled_refusal(&format!("GetFilter after deleting {case}"), status, &refused);
        let (status, refused) = delete_filter(&router, &detector_id, &name).await;
        assert_modeled_refusal(&format!("a second DeleteFilter of {case}"), status, &refused);
        let (status, refused) = update_filter(&router, &detector_id, &name).await;
        assert_modeled_refusal(
            &format!("UpdateFilter after deleting {case}"),
            status,
            &refused,
        );

        // And the list reports the detector's remaining filters, nothing more.
        let listed = list_names(&router, &detector_id, &case).await;
        assert_eq!(
            listed,
            siblings.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "case {case}: expected only the siblings after the delete, got {listed:?}"
        );
        for sibling in siblings {
            let (status, read) = get_filter(&router, &detector_id, sibling).await;
            assert_eq!(
                status, 200,
                "case {case}: sibling {sibling:?} expected HTTP 200, got {status} {read:?}"
            );
            assert_eq!(
                read["version"],
                json!(1),
                "case {case}: deleting another filter must not change {sibling:?}, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_of_something_the_detector_does_not_hold_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    saved(&router, &detector_id, "untouched.filter-1", "the baseline").await;

    // Names no detector holds: two outside the published 3..64 length, one outside the
    // documented alphabet, one carrying whitespace ("A whitespace is considered to be
    // an invalid character") and one well formed but absent.
    let absent: Vec<(String, &str)> = vec![
        (name_of(2), "two characters: below the published minimum of 3"),
        (
            name_of(65),
            "65 characters: one above the published maximum of 64",
        ),
        (
            "bad!name".to_string(),
            "a character outside the documented alphabet",
        ),
        (
            "well%20formed".to_string(),
            "percent-encoded whitespace, which the docs call an invalid character",
        ),
        (
            "absent.but-well_formed".to_string(),
            "well formed, and held by no detector",
        ),
    ];
    for (name, why) in absent {
        let (status, refused) = delete_filter(&router, &detector_id, &name).await;
        assert_modeled_refusal(
            &format!("DeleteFilter on {name:?} ({why})"),
            status,
            &refused,
        );
    }

    // A detector id no create ever minted is refused the same way.
    let (status, refused) = delete_filter(&router, ABSENT_DETECTOR_ID, "untouched.filter-1").await;
    assert_modeled_refusal(
        "DeleteFilter under a detector id nothing minted",
        status,
        &refused,
    );

    // None of those refusals touched the saved filter.
    let listed = list_names(&router, &detector_id, "after the refusals").await;
    assert_eq!(
        listed,
        vec!["untouched.filter-1".to_string()],
        "case after the refusals: expected the saved filter still listed, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the record goes, so the name and the per-Region quota it held are free.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_frees_the_name_and_the_documented_filter_quota() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for index in 0..FILTER_QUOTA_PER_REGION {
        saved(
            &router,
            &detector_id,
            &format!("quota.filter-{index:03}"),
            "filling the quota",
        )
        .await;
    }

    // One past the documented ceiling is refused.
    let (status, refused) = create_filter(&router, &detector_id, "quota.filter-over").await;
    assert_modeled_refusal(
        "CreateFilter one past the documented per-Region quota",
        status,
        &refused,
    );

    // Deleting one frees exactly one slot: the next create lands, the one after it is
    // refused again.
    let victim = "quota.filter-050";
    let (status, deleted) = delete_filter(&router, &detector_id, victim).await;
    assert_eq!(
        status, 200,
        "case DeleteFilter inside a full quota: expected HTTP 200, got {status} {deleted:?}"
    );
    let (status, created) = create_filter(&router, &detector_id, "quota.filter-over").await;
    assert_eq!(
        status, 200,
        "case CreateFilter after a delete freed a slot: expected HTTP 200, got {status} {created:?}"
    );
    let (status, refused) = create_filter(&router, &detector_id, "quota.filter-over-again").await;
    assert_modeled_refusal(
        "CreateFilter once the freed slot is taken again",
        status,
        &refused,
    );

    // The deleted name is gone and the created one is there; the count is unchanged.
    let (status, refused) = get_filter(&router, &detector_id, victim).await;
    assert_modeled_refusal("GetFilter on the filter the delete removed", status, &refused);
    let (status, read) = get_filter(&router, &detector_id, "quota.filter-over").await;
    assert_eq!(
        status, 200,
        "case the filter created in the freed slot: expected HTTP 200, got {status} {read:?}"
    );

    // The name is free for a fresh create, which starts its own lifecycle at version 1
    // rather than resuming the deleted filter's.
    let (status, deleted) = delete_filter(&router, &detector_id, "quota.filter-over").await;
    assert_eq!(
        status, 200,
        "case freeing a slot again: expected HTTP 200, got {status} {deleted:?}"
    );
    saved(&router, &detector_id, victim, "re-creating a deleted name").await;
    let (status, read) = get_filter(&router, &detector_id, victim).await;
    assert_eq!(
        status, 200,
        "case the re-created filter: expected HTTP 200, got {status} {read:?}"
    );
    assert_eq!(
        read["version"],
        json!(1),
        "case the re-created filter: expected version 1, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a delete reaches one detector in one Region.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_reaches_only_the_detector_and_region_it_names() {
    let router = new_router();
    let home = enabled_detector(&router).await;

    // A second detector, in another Region: GuardDuty holds one per account per Region,
    // and the Region comes from the request's credential scope.
    let (status, created) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateDetector in eu-west-1: expected HTTP 200, got {status} {created:?}"
    );
    let away = created["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case CreateDetector in eu-west-1: expected a detectorId, got {created:?}"))
        .to_string();

    // The same filter name under each detector.
    let name = "shared.name-1";
    saved(&router, &home, name, "the home filter").await;
    let (status, created) = create_filter_in(&router, "eu-west-1", &away, name).await;
    assert_eq!(
        status, 200,
        "case CreateFilter in eu-west-1: expected HTTP 200, got {status} {created:?}"
    );

    // Addressing the other Region's detector through this Region's endpoint reaches
    // nothing, and writes nothing.
    let (status, refused) = delete_filter(&router, &away, name).await;
    assert_modeled_refusal(
        "DeleteFilter on another Region's detector through this Region's endpoint",
        status,
        &refused,
    );
    let (status, refused) = delete_filter_in(&router, "eu-west-1", &home, name).await;
    assert_modeled_refusal(
        "DeleteFilter on this Region's detector through another Region's endpoint",
        status,
        &refused,
    );

    // Deleting the home filter leaves the other Region's filter of the same name alone.
    let (status, deleted) = delete_filter(&router, &home, name).await;
    assert_eq!(
        status, 200,
        "case DeleteFilter on the home filter: expected HTTP 200, got {status} {deleted:?}"
    );
    let (status, refused) = get_filter(&router, &home, name).await;
    assert_modeled_refusal("GetFilter on the deleted home filter", status, &refused);
    let (status, read) = get_filter_in(&router, "eu-west-1", &away, name).await;
    assert_eq!(
        status, 200,
        "case the same name under another Region's detector: expected HTTP 200, got {status} {read:?}"
    );
    assert_eq!(
        read["version"],
        json!(1),
        "case the same name under another Region's detector: expected it untouched at version 1, got {read:?}"
    );
}

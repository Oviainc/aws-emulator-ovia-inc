// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListInterfaceRelationships`, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rule these suites pin: the list reports exactly the asset models the
//! named interface asset model was APPLIED to — one summary per stored
//! relationship the interface stands on the interface side of, in a stable
//! order, cut into pages of at most `maxResults` and resumable by the
//! `nextToken` the previous page issued. So the answer follows the store: an
//! interface nothing was applied to lists nothing, an application adds its
//! model, deleting a model removes it again, and a label that resolves to no
//! stored asset model is the modelled refusal rather than an empty page.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router};

/// One measurement property definition under `name`.
fn property(name: &str) -> Value {
    json!({ "name": name, "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}} })
}

/// Create an asset model from `body` and return its id.
async fn create_model(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(
        status, 202,
        "case {case}: create model got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no assetModelId in {created}"))
        .to_string()
}

/// Create an asset model of `model_type` named `name`, carrying one measurement
/// property called `Temperature`, and return its id. `external_id` is the
/// `assetModelExternalId` it carries, when it carries one.
async fn model(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    model_type: &str,
    external_id: Option<&str>,
) -> String {
    let mut body = json!({
        "assetModelName": name,
        "assetModelType": model_type,
        "assetModelProperties": [property("Temperature")],
    });
    if let Some(external_id) = external_id {
        body["assetModelExternalId"] = json!(external_id);
    }
    create_model(router, case, body).await
}

/// Apply the interface asset model `interface` to the asset model `asset_model`,
/// pairing the property both declare by name.
async fn apply(router: &MultiServiceRouter, case: &str, asset_model: &str, interface: &str) {
    let (status, applied) = call(
        router,
        "PUT",
        &format!(
            "/asset-models/{asset_model}/interface/{interface}/asset-model-interface-relationship"
        ),
        Some(json!({
            "propertyMappingConfiguration": { "matchByPropertyName": true }
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case}: applying {interface} to {asset_model} got {status} {applied}"
    );
}

/// `ListInterfaceRelationships`'s route for the `CustomID` label `interface`,
/// with the page size and resume token a caller supplies.
fn list_uri(interface: &str, max_results: Option<u32>, next_token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = format!("/interface/{interface}/asset-models");
    if query.is_empty() {
        uri
    } else {
        format!("{uri}?{}", query.join("&"))
    }
}

/// One page of the list, as `(status, body)`.
async fn list_page(
    router: &MultiServiceRouter,
    interface: &str,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &list_uri(interface, max_results, next_token),
        None,
    )
    .await
}

/// The `id` of every summary of one page, in the order the page reports them.
fn page_ids(body: &Value, case: &str) -> Vec<String> {
    body["interfaceRelationshipSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: no interfaceRelationshipSummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a summary carries no id in {body}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of the list with page size `max_results`, returning the ids
/// in enumeration order. Asserts, per page: HTTP 200, no page longer than the
/// page size asked for, and a `nextToken` only where another page follows.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    interface: &str,
    max_results: Option<u32>,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // The store holds a bounded number of models here, so a walk that does not
    // terminate within this many pages is a paging bug, not a big list.
    for page in 0..64 {
        let (status, body) = list_page(router, interface, max_results, token.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case}: page {page} got {status} {body}"
        );
        let on_page = page_ids(&body, case);
        if let Some(max_results) = max_results {
            assert!(
                on_page.len() <= max_results as usize,
                "case {case}: page {page} holds {} summaries, more than the {max_results} asked \
                 for: {body}",
                on_page.len()
            );
        }
        ids.extend(on_page);
        match body["nextToken"].as_str() {
            Some(next) => {
                assert!(
                    !next.is_empty(),
                    "case {case}: page {page} issued an empty nextToken in {body}"
                );
                token = Some(next.to_string());
            }
            None => return ids,
        }
    }
    panic!("case {case}: paging did not terminate");
}

#[tokio::test]
async fn the_list_reports_every_asset_model_the_interface_was_applied_to_in_pages() {
    // Rows: how many asset models carry the interface. 0 covers the empty
    // enumeration, 1 the single page, and 3 and 5 an enumeration that has to be
    // cut into several pages at the small page sizes below.
    for applied_count in [0_usize, 1, 3, 5] {
        let case = format!("{applied_count} applied");
        let router = router();
        let interface = model(&router, &case, "Interface", "INTERFACE", None).await;

        let mut expected: BTreeSet<String> = BTreeSet::new();
        for index in 0..applied_count {
            let applied_to =
                model(&router, &case, &format!("AppliedTo{index}"), "ASSET_MODEL", None).await;
            apply(&router, &case, &applied_to, &interface).await;
            expected.insert(applied_to);
        }

        // The whole enumeration, in one page: the order every page size has to
        // agree with.
        let whole = walk(&router, &case, &interface, Some(250)).await;
        assert_eq!(
            whole.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {case}: expected the applied models {expected:?}, got {whole:?}"
        );
        assert_eq!(
            whole.len(),
            expected.len(),
            "case {case}: expected {} summaries with no repeats, got {whole:?}",
            expected.len()
        );
        for id in &whole {
            assert!(
                is_modelled_id(id),
                "case {case}: expected a modelled asset model id, got {id:?}"
            );
        }

        // Rows: the page sizes the enumeration is cut at, including no
        // `maxResults` at all, where the documented default of 50 applies and
        // this enumeration fits in one page.
        for max_results in [None, Some(1), Some(2), Some(3), Some(250)] {
            let walked = walk(&router, &case, &interface, max_results).await;
            assert_eq!(
                walked, whole,
                "case {case} (maxResults={max_results:?}): the pages joined must be the whole \
                 enumeration once, in one order; got {walked:?} against {whole:?}"
            );
        }

        // Every id the list reports is addressable as a relationship: the list
        // follows the store rather than inventing ids.
        for id in &whole {
            let (status, read) = call(
                &router,
                "GET",
                &format!(
                    "/asset-models/{id}/interface/{interface}/asset-model-interface-relationship"
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: the listed model {id} must have the relationship the list reports, \
                 got {status} {read}"
            );
        }
    }
}

#[tokio::test]
async fn the_list_is_scoped_to_one_interface_and_follows_the_store() {
    let case = "scoping";
    let router = router();
    let first = model(&router, case, "First", "INTERFACE", Some("interface.first")).await;
    let second = model(&router, case, "Second", "INTERFACE", None).await;
    let unused = model(&router, case, "Unused", "INTERFACE", None).await;
    let plain = model(&router, case, "Plain", "ASSET_MODEL", None).await;
    let alpha = model(&router, case, "Alpha", "ASSET_MODEL", None).await;
    let beta = model(&router, case, "Beta", "ASSET_MODEL", None).await;
    let gamma = model(&router, case, "Gamma", "ASSET_MODEL", None).await;

    apply(&router, case, &alpha, &first).await;
    apply(&router, case, &beta, &first).await;
    apply(&router, case, &gamma, &second).await;

    // Rows: an interface with two applications, one with a single application,
    // one applied to nothing, a model that is no interface at all (so nothing
    // can have been applied from it), and the `externalId:` form of a label that
    // resolves to the first interface.
    let rows: [(&str, String, BTreeSet<String>); 5] = [
        (
            "two applications",
            first.clone(),
            BTreeSet::from([alpha.clone(), beta.clone()]),
        ),
        (
            "one application",
            second.clone(),
            BTreeSet::from([gamma.clone()]),
        ),
        ("applied to nothing", unused, BTreeSet::new()),
        ("not an interface", plain, BTreeSet::new()),
        (
            "externalId label",
            "externalId:interface.first".to_string(),
            BTreeSet::from([alpha.clone(), beta.clone()]),
        ),
    ];
    for (row, label, expected) in rows {
        let walked = walk(&router, row, &label, Some(250)).await;
        assert_eq!(
            walked.iter().cloned().collect::<BTreeSet<String>>(),
            expected,
            "case {row:?}: expected {expected:?}, got {walked:?}"
        );

        // A read must not change what a later caller sees.
        let (status, again) = list_page(&router, &label, Some(250), None).await;
        assert_eq!(status, 200, "case {row:?}: the second read got {status} {again}");
        assert_eq!(
            page_ids(&again, row),
            walked,
            "case {row:?}: a read must not change what the next caller sees, got {again}"
        );
        assert_eq!(
            again["nextToken"],
            Value::Null,
            "case {row:?}: a page that ends the enumeration carries no nextToken, got {again}"
        );
    }

    // The list follows the store both ways: deleting an applied-to model drops
    // its relationship, so the list no longer reports it.
    let (status, deleted) = call(&router, "DELETE", &format!("/asset-models/{beta}"), None).await;
    assert_eq!(status, 202, "case {case}: deleting {beta} got {status} {deleted}");
    let walked = walk(&router, case, &first, Some(250)).await;
    assert_eq!(
        walked,
        vec![alpha.clone()],
        "case {case}: a deleted model must leave the list, got {walked:?}"
    );
}

#[tokio::test]
async fn the_next_token_resumes_the_enumeration_the_previous_page_left_off() {
    let case = "tokens";
    let router = router();
    let interface = model(&router, case, "Interface", "INTERFACE", None).await;
    for index in 0..3 {
        let applied_to =
            model(&router, case, &format!("AppliedTo{index}"), "ASSET_MODEL", None).await;
        apply(&router, case, &applied_to, &interface).await;
    }
    let whole = walk(&router, case, &interface, Some(250)).await;
    assert_eq!(whole.len(), 3, "case {case}: expected 3 applications, got {whole:?}");

    // The token a one-per-page walk issues names the model the NEXT page starts
    // at, encoded the way this service encodes a resume point. `hex_token` is an
    // independent oracle: the handler encodes with the `hex` crate, so one
    // encoding bug cannot agree with itself here.
    let (status, first) = list_page(&router, &interface, Some(1), None).await;
    assert_eq!(status, 200, "case {case}: the first page got {status} {first}");
    assert_eq!(
        page_ids(&first, case),
        vec![whole[0].clone()],
        "case {case}: expected the first model of the enumeration, got {first}"
    );
    assert_eq!(
        first["nextToken"],
        json!(hex_token(&whole[1])),
        "case {case}: expected the token for {}, got {first}",
        whole[1]
    );

    // Resuming from that token yields the rest of the enumeration, once.
    let (status, rest) = list_page(
        &router,
        &interface,
        Some(250),
        first["nextToken"].as_str(),
    )
    .await;
    assert_eq!(status, 200, "case {case}: the resumed page got {status} {rest}");
    assert_eq!(
        page_ids(&rest, case),
        whole[1..].to_vec(),
        "case {case}: expected the tail of the enumeration, got {rest}"
    );
}

#[tokio::test]
async fn a_label_token_or_page_size_outside_the_contract_is_a_modelled_refusal() {
    let case = "refusals";
    let router = router();
    let interface = model(&router, case, "Interface", "INTERFACE", None).await;
    let applied_to = model(&router, case, "AppliedTo", "ASSET_MODEL", None).await;
    apply(&router, case, &applied_to, &interface).await;

    let unknown = "99999999-9999-4999-8999-999999999999";
    let stale = hex_token(unknown);
    let rows: [(&str, String, u16, &str); 4] = [
        (
            "an interface asset model that does not exist",
            list_uri(unknown, None, None),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an externalId label that names nothing stored",
            list_uri("externalId:no_such_model", None, None),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a token for a model not in this enumeration",
            list_uri(&interface, None, Some(&stale)),
            400,
            "InvalidRequestException",
        ),
        (
            "a token this operation never issued",
            list_uri(&interface, None, Some("zzz")),
            400,
            "InvalidRequestException",
        ),
    ];
    for (row, uri, want_status, want_code) in rows {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, want_status,
            "case {row:?}: expected HTTP {want_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(want_code),
            "case {row:?}: expected {want_code}, got {:?} in {body}",
            error_code(&body)
        );
    }

    // `maxResults` carries the modelled range 1..250: each published bound is
    // served and the value just past each bound is refused.
    for (max_results, accepted) in [(0_u32, false), (1, true), (250, true), (251, false)] {
        let (status, body) = list_page(&router, &interface, Some(max_results), None).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: an in-range page size must be served, got \
                 {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults={max_results}: an out-of-range page size must be refused, got \
                 {status} {body}"
            );
        }
    }
}

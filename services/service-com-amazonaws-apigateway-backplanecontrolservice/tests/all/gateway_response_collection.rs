// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetGatewayResponses`: the whole gateway response collection of
//! one REST API.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder, the
//! `CreateRestApi` helper, the single-response helpers and the generated-status-code
//! catalog are this crate's existing harness, reused from `rest_api_lifecycle` and
//! `gateway_response`.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: the
//! collection of an API is, per modelled `GatewayResponseType`, the customization stored
//! for that type or the response API Gateway generates for it — checked over a table of
//! customization sets (none, one, several, every type), against the single read of each
//! type, over a table of `position`/`limit` values the unpaginated collection must
//! ignore, and over repeated reads that must not change what a later caller sees.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::GATEWAY_RESPONSE_TYPE_VALUES;

use crate::gateway_response::{
    default_response_templates, generated_status_code, get_gateway_response, put_gateway_response,
};
use crate::gateway_response_scope::{class_default_of, effective_response};
use crate::rest_api_lifecycle::{assert_failure, create_api, page_items, Answer, Gateway};

/// `GetGatewayResponses` for `api`, with `query` appended verbatim (empty for none).
/// Shared with the sibling `delete_gateway_response` suite, which reads the collection
/// back after clearing a customization.
pub(crate) async fn get_gateway_responses(gateway: &Gateway, api: &str, query: &str) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/gatewayresponses{query}"),
            None,
        )
        .await
}

/// The collection of `api` as a `responseType`-keyed map of the reported records, with
/// every member of each record kept: the listing is a collection of `GatewayResponse`
/// objects, so this is what lets a case compare one against a single read.
pub(crate) async fn collection_by_type(gateway: &Gateway, api: &str, case: &str) -> Vec<(String, Value)> {
    let listed = get_gateway_responses(gateway, api, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected 200 from GetGatewayResponses, got {} with {:?}",
        listed.status, listed.body
    );
    assert!(
        listed.body.get("position").is_none(),
        "case {case}: an unpaginated collection hands back no cursor, got {:?}",
        listed.body
    );
    page_items(&listed.body, case)
        .into_iter()
        .map(|item| {
            let response_type = item["responseType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a responseType, got {item:?}"))
                .to_owned();
            (response_type, item)
        })
        .collect()
}

#[tokio::test]
async fn the_collection_reports_one_gateway_response_per_modelled_response_type() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-responses-defaults").await;

    let listed = collection_by_type(&gateway, &api, "a fresh API").await;
    let types: Vec<&str> = listed.iter().map(|(kind, _)| kind.as_str()).collect();
    let mut sorted = types.clone();
    sorted.sort_unstable();
    sorted.dedup();
    // Sorted on both sides, so the assertion says "the same set of types" and does not
    // depend on either list's order.
    let mut modelled = GATEWAY_RESPONSE_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        sorted.len(),
        types.len(),
        "case a fresh API: expected each response type once, got {types:?}"
    );
    assert_eq!(
        sorted, modelled,
        "case a fresh API: expected the modelled response types, got {types:?}"
    );

    // An API a developer has customized nothing on reports, for every type, the response
    // API Gateway generates: the flag says so, neither caller-supplied map is set, and the
    // status code is the type's own (absent for the two types that have none).
    for (response_type, item) in &listed {
        let expected_status = generated_status_code(response_type);
        assert_eq!(
            item["defaultResponse"].as_bool(),
            Some(true),
            "case {response_type}: an uncustomized type is the default response, got {item:?}"
        );
        assert_eq!(
            item["responseParameters"],
            json!({}),
            "case {response_type}: expected no configured response parameter, got {item:?}"
        );
        assert_eq!(
            item["responseTemplates"],
            default_response_templates(),
            "case {response_type}: expected API Gateway's own body template, got {item:?}"
        );
        assert_eq!(
            item["statusCode"].as_str(),
            expected_status,
            "case {response_type}: expected the generated status code {expected_status:?}, got \
             {item:?}"
        );

        // The listing and the single read answer from one rule, member for member.
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            &read.body, item,
            "case {response_type}: GetGatewayResponse and GetGatewayResponses must agree, got \
             {:?} against {item:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_collection_reports_the_customizations_and_the_generated_rest() {
    // Rows differ along what the rule generalises over: HOW MANY types are customized
    // (none, one, several, every modelled type) and WHAT each customization carries (a
    // status code, one or both caller-supplied maps, nothing at all).
    let one: Vec<(&str, Value)> = vec![("INTEGRATION_TIMEOUT", json!({ "statusCode": "504" }))];
    let several: Vec<(&str, Value)> = vec![
        (
            "DEFAULT_4XX",
            json!({
                "statusCode": "400",
                "responseParameters": {
                    "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                }
            }),
        ),
        (
            "DEFAULT_5XX",
            json!({
                "responseTemplates": { "application/json": "{\"message\":\"boom\"}" }
            }),
        ),
        ("UNAUTHORIZED", json!({ "statusCode": "403" })),
        ("WAF_FILTERED", json!({})),
    ];
    let every: Vec<(&str, Value)> = GATEWAY_RESPONSE_TYPE_VALUES
        .iter()
        .map(|response_type| {
            (
                *response_type,
                json!({ "responseTemplates": { "text/plain": *response_type } }),
            )
        })
        .collect();

    let none: Vec<(&str, Value)> = Vec::new();
    let cases: Vec<(&str, &Vec<(&str, Value)>)> = vec![
        ("no customization", &none),
        ("one customized type", &one),
        ("several customized types", &several),
        ("every type customized", &every),
    ];

    for (case, customizations) in cases {
        let gateway = Gateway::new();
        let (api, _root) = create_api(&gateway, "gateway-responses-mixed").await;

        let mut stored: Vec<(String, Value)> = Vec::new();
        for (response_type, request) in customizations {
            let created =
                put_gateway_response(&gateway, &api, response_type, request.clone()).await;
            assert_eq!(
                created.status, 201,
                "case {case}/{response_type}: expected 201 from PutGatewayResponse, got {} with \
                 {:?}",
                created.status, created.body
            );
            stored.push(((*response_type).to_owned(), created.body));
        }

        let listed = collection_by_type(&gateway, &api, case).await;
        // The collection still covers every modelled type: a customization replaces the
        // generated response of its type, it does not add an entry.
        assert_eq!(
            listed.len(),
            GATEWAY_RESPONSE_TYPE_VALUES.len(),
            "case {case}: expected {} entries, got {}",
            GATEWAY_RESPONSE_TYPE_VALUES.len(),
            listed.len()
        );

        for (response_type, expected) in &stored {
            let reported = listed
                .iter()
                .find(|(kind, _)| kind == response_type)
                .unwrap_or_else(|| {
                    panic!("case {case}/{response_type}: expected the type in the collection")
                });
            assert_eq!(
                &reported.1, expected,
                "case {case}/{response_type}: expected the stored customization {expected:?}, got \
                 {:?}",
                reported.1
            );
        }

        // Every type nobody customized reports the response the API serves for it: the
        // class default standing in for it where this API customized that one, and the
        // generated response otherwise.
        for (response_type, item) in &listed {
            if stored.iter().any(|(kind, _)| kind == response_type) {
                continue;
            }
            let class_default = class_default_of(response_type).and_then(|kind| {
                stored
                    .iter()
                    .find(|(stored_kind, _)| stored_kind == kind)
                    .map(|(_, record)| record)
            });
            let expected = effective_response(response_type, class_default);
            assert_eq!(
                item["defaultResponse"].as_bool(),
                Some(true),
                "case {case}/{response_type}: expected the generated response, got {item:?}"
            );
            assert_eq!(
                item, &expected,
                "case {case}/{response_type}: expected {expected:?}, got {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn pagination_inputs_do_not_change_the_unpaginated_collection() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-responses-pagination").await;
    let customized = put_gateway_response(
        &gateway,
        &api,
        "QUOTA_EXCEEDED",
        json!({ "statusCode": "429" }),
    )
    .await;
    assert_eq!(
        customized.status, 201,
        "case PutGatewayResponse(QUOTA_EXCEEDED): expected 201, got {} with {:?}",
        customized.status, customized.body
    );

    let whole = collection_by_type(&gateway, &api, "no query").await;

    // "The GatewayResponses collection does not support pagination and the limit does not
    // apply here" (`GetGatewayResponsesRequest$limit` / `$position` in this service's
    // model): every row below asks for a page and gets the entire collection. The rows
    // span the documented default and maximum page sizes, values past each bound, a
    // position naming an entry and a position naming nothing.
    let queries = [
        "?limit=1",
        "?limit=25",
        "?limit=500",
        "?limit=501",
        "?limit=0",
        "?position=THROTTLED",
        "?position=not-a-position",
        "?limit=2&position=ACCESS_DENIED",
    ];
    for query in queries {
        let listed = get_gateway_responses(&gateway, &api, query).await;
        assert_eq!(
            listed.status, 200,
            "case {query}: expected 200 from GetGatewayResponses, got {} with {:?}",
            listed.status, listed.body
        );
        let items = page_items(&listed.body, query);
        assert_eq!(
            items.len(),
            whole.len(),
            "case {query}: the entire collection is returned, expected {} entries, got {}",
            whole.len(),
            items.len()
        );
        assert!(
            listed.body.get("position").is_none(),
            "case {query}: an unpaginated collection hands back no cursor, got {:?}",
            listed.body
        );
        let expected: Vec<Value> = whole.iter().map(|(_, item)| item.clone()).collect();
        assert_eq!(
            items, expected,
            "case {query}: expected the whole collection {expected:?}, got {items:?}"
        );
    }

    // Listing is a read: the collection after all those calls is the one before them.
    let again = collection_by_type(&gateway, &api, "after the queries").await;
    assert_eq!(
        again, whole,
        "case after the queries: a read must not change the records, got {again:?} after {whole:?}"
    );
}

#[tokio::test]
async fn an_unknown_api_has_no_gateway_response_collection() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-responses-notfound").await;

    // An identifier naming no API, in the ways one can miss: a well-formed id nobody
    // minted, a shorter one and a longer one.
    for missing in ["zzzzzzzzzz", "abc", "abcdefghijklmnop"] {
        assert_failure(
            &get_gateway_responses(&gateway, missing, "").await,
            404,
            "NotFoundException",
            &format!("GetGatewayResponses({missing})"),
        );
    }

    // The collection of a deleted API goes with it.
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_gateway_responses(&gateway, &api, "").await,
        404,
        "NotFoundException",
        "GetGatewayResponses after DeleteRestApi",
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `GetIntegrations`.
//!
//! The rules these cases assert:
//!
//! * `GetIntegrations` reports the integrations the named api OWNS, each one as the
//!   store holds it, chained through `NextToken` so every integration appears exactly
//!   once whatever page size the caller asks for.
//! * The page is bounded server-side: at most `MaxResults` items, and at most
//!   `MAX_PAGE_SIZE` however large a `MaxResults` the caller names, with the rest
//!   reported through `NextToken`.
//! * An api identifier nothing minted is a 404 `NotFoundException`; a `MaxResults`
//!   that is not a positive integer is a 400 `BadRequestException`.
//! * It is a reporting operation: repeating it reports the same page.
//!
//! The rows vary what the rules generalise over: how many integrations exist (0 to 5,
//! plus one past the page maximum), the page size (1 up to one past the total), the
//! protocol type of the owning api and the integration type and optional members of each
//! record. The recorded case that filed the issue — listing the integration a
//! `CreateIntegration` just stored on an HTTP api — is one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The largest page a paginated ApiGatewayV2 reader serves, and the page size used when
/// a request names no `MaxResults` (`crate::state::MAX_PAGE_SIZE`).
const MAX_PAGE_SIZE: usize = 100;

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    let id = as_str(&created, "integrationId");
    assert_minted_id(case, "IntegrationId", &id);
    id
}

/// One row of the listing table: the api that owns the integration, the request that
/// creates it, and the members the listing must report for it.
struct Row<'a> {
    case: &'static str,
    api_id: &'a String,
    request: Value,
    expected: Vec<(&'static str, Value)>,
}

/// A [`Row`] after its integration was stored, carrying the id the create minted.
struct Stored<'a> {
    case: &'static str,
    api_id: &'a String,
    integration_id: String,
    expected: Vec<(&'static str, Value)>,
}

/// An `HTTP_PROXY` integration request pointing at a distinct upstream.
fn proxy_request(index: usize) -> Value {
    json!({
        "integrationType": "HTTP_PROXY",
        "integrationUri": format!("https://example.com/upstream/{index}"),
        "payloadFormatVersion": "1.0"
    })
}

/// Walk `uri_for(page_size, token)` to exhaustion and answer every `IntegrationId` seen,
/// in the order the pages reported them.
async fn walk_pages(
    emulator: &MultiServiceRouter,
    label: &str,
    api_id: &str,
    page_size: usize,
    total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than pages are possible, so a token that never clears is
    // caught instead of looping forever.
    for _ in 0..total + 2 {
        let uri = match &token {
            Some(t) => {
                format!("/v2/apis/{api_id}/integrations?maxResults={page_size}&nextToken={t}")
            }
            None => format!("/v2/apis/{api_id}/integrations?maxResults={page_size}"),
        };
        let (status, body) = call(emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetIntegrations, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "case {label:?}: a page must hold at most {page_size} items, got {body}"
        );
        seen.extend(items.iter().map(|item| as_str(item, "integrationId")));
        token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {label:?}: expected the chain to end, got a NextToken {token:?} after {} items",
        seen.len()
    );
    seen
}

#[tokio::test]
async fn get_integrations_pages_every_integration_exactly_once_at_every_page_size() {
    // The rule generalises over how many integrations exist and over page size, so the
    // table walks both: each count is paged at every size from 1 up to one past it.
    for total in [0_usize, 1, 2, 5] {
        let emulator = emulator();
        let api_id = create_api(
            &emulator,
            &format!("total={total}"),
            &json!({ "name": format!("paged-integrations-{total}"), "protocolType": "HTTP" }),
        )
        .await;

        let mut expected = BTreeSet::new();
        for index in 0..total {
            expected.insert(
                create_integration(
                    &emulator,
                    &format!("total={total}, integration #{index}"),
                    &api_id,
                    proxy_request(index),
                )
                .await,
            );
        }

        for page_size in 1..=total + 1 {
            let label = format!("total={total}, maxResults={page_size}");
            let seen = walk_pages(&emulator, &label, &api_id, page_size, total).await;
            assert_eq!(
                seen.len(),
                total,
                "case {label:?}: expected {total} integrations across the chain, got {seen:?}"
            );
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<_>>(),
                expected,
                "case {label:?}: expected exactly the stored integrations, got {seen:?}"
            );
        }

        // Unpaged: every integration in one answer, with nothing left to report.
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case total={total}: expected 200 from an unpaged GetIntegrations, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(total),
            "case total={total}: an unpaged GetIntegrations must report every integration, got {body}"
        );
        assert!(
            body.get("nextToken").is_none() || body["nextToken"].is_null(),
            "case total={total}: an unpaged GetIntegrations has nothing left to report, got {body}"
        );
    }
}

#[tokio::test]
async fn get_integrations_reports_each_stored_record_and_only_the_owning_api_s() {
    let emulator = emulator();

    // The rows differ in the owning api's protocol type, the integration type and which
    // optional members the record carries.
    let http = create_api(
        &emulator,
        "an http api",
        &json!({ "name": "listing-http-api", "protocolType": "HTTP" }),
    )
    .await;
    let socket = create_api(
        &emulator,
        "a websocket api",
        &json!({
            "name": "listing-socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;

    let rows = vec![
        Row {
            case: "an AWS_PROXY integration fronting a Lambda function on an HTTP api",
            api_id: &http,
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:recorder",
                "payloadFormatVersion": "2.0"
            }),
            expected: vec![
                ("integrationType", json!("AWS_PROXY")),
                (
                    "integrationUri",
                    json!("arn:aws:lambda:us-east-1:000000000000:function:recorder"),
                ),
                ("payloadFormatVersion", json!("2.0")),
                // Defaulted by the store, not echoed from the request.
                ("connectionType", json!("INTERNET")),
                ("timeoutInMillis", json!(30_000)),
                ("apiGatewayManaged", json!(false)),
            ],
        },
        Row {
            case: "an HTTP_PROXY integration with an explicit method, timeout and description",
            api_id: &http,
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0",
                "timeoutInMillis": 15_000,
                "description": "a proxy to an upstream service"
            }),
            expected: vec![
                ("integrationType", json!("HTTP_PROXY")),
                ("integrationMethod", json!("ANY")),
                ("timeoutInMillis", json!(15_000)),
                ("description", json!("a proxy to an upstream service")),
            ],
        },
        Row {
            case: "a MOCK integration on a WebSocket api, which names no endpoint",
            api_id: &socket,
            request: json!({ "integrationType": "MOCK" }),
            expected: vec![
                ("integrationType", json!("MOCK")),
                ("connectionType", json!("INTERNET")),
                // The WebSocket default, not the HTTP one.
                ("timeoutInMillis", json!(29_000)),
            ],
        },
    ];

    let mut stored: Vec<Stored<'_>> = Vec::new();
    for row in rows {
        let integration_id =
            create_integration(&emulator, row.case, row.api_id, row.request.clone()).await;
        stored.push(Stored {
            case: row.case,
            api_id: row.api_id,
            integration_id,
            expected: row.expected,
        });
    }

    for Stored {
        case,
        api_id,
        integration_id,
        expected,
    } in &stored
    {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrations, got {status} {body}"
        );
        let items = body["items"].as_array().cloned().unwrap_or_default();
        let listed = items
            .iter()
            .find(|item| as_str(item, "integrationId") == *integration_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {integration_id} to be listed, got {body}")
            });
        for (member, value) in expected {
            assert_eq!(
                listed.get(member),
                Some(value),
                "case {case:?}: expected the listed integration to report {member}={value}, got {listed}"
            );
        }

        // The listed record is the one the single-resource reader reports.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegration, got {status} {fetched}"
        );
        for (member, value) in expected {
            assert_eq!(
                fetched.get(member),
                Some(value),
                "case {case:?}: expected GetIntegration to report {member}={value}, got {fetched}"
            );
        }
    }

    // An integration belongs to the one api that owns it: each api lists only its own.
    for (api_id, label) in [(&http, "the HTTP api"), (&socket, "the WebSocket api")] {
        let expected: BTreeSet<String> = stored
            .iter()
            .filter(|entry| entry.api_id == api_id)
            .map(|entry| entry.integration_id.clone())
            .collect();
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetIntegrations, got {status} {body}"
        );
        let listed: BTreeSet<String> = body["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| as_str(item, "integrationId"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {label:?}: expected exactly the api's own integrations, got {body}"
        );
    }
}

#[tokio::test]
async fn get_integrations_is_a_report_that_changes_nothing() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a repeated listing",
        &json!({ "name": "repeat-listing-api", "protocolType": "HTTP" }),
    )
    .await;
    for index in 0..3 {
        create_integration(
            &emulator,
            &format!("integration #{index}"),
            &api_id,
            proxy_request(index),
        )
        .await;
    }

    let uri = format!("/v2/apis/{api_id}/integrations?maxResults=2");
    let (_, first) = call(&emulator, "GET", &uri, None).await;
    for attempt in 0..3 {
        let (status, again) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case attempt={attempt}: expected 200 from GetIntegrations, got {status} {again}"
        );
        assert_eq!(
            again, first,
            "case attempt={attempt}: a report must not change what a later caller sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_get_integrations_page_is_bounded_server_side() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a clamped listing",
        &json!({ "name": "clamped-listing-api", "protocolType": "HTTP" }),
    )
    .await;
    // One past the page maximum, so a page that is not clamped is visible.
    let total = MAX_PAGE_SIZE + 1;
    for index in 0..total {
        create_integration(
            &emulator,
            &format!("integration #{index}"),
            &api_id,
            proxy_request(index),
        )
        .await;
    }

    for (label, uri) in [
        (
            "no MaxResults",
            format!("/v2/apis/{api_id}/integrations"),
        ),
        (
            "MaxResults far above the maximum",
            format!("/v2/apis/{api_id}/integrations?maxResults=1000000"),
        ),
        (
            "MaxResults at the maximum",
            format!("/v2/apis/{api_id}/integrations?maxResults={MAX_PAGE_SIZE}"),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from GetIntegrations, got {status} {body}"
        );
        assert_eq!(
            body["items"].as_array().map(Vec::len),
            Some(MAX_PAGE_SIZE),
            "case {label:?}: a page must be clamped to {MAX_PAGE_SIZE} items, got {body}"
        );
        assert!(
            body.get("nextToken").and_then(Value::as_str).is_some(),
            "case {label:?}: a clamped page must report what is left through NextToken, got {body}"
        );
    }

    // And the clamped chain still reports every integration exactly once.
    let seen = walk_pages(&emulator, "clamped chain", &api_id, MAX_PAGE_SIZE, total).await;
    assert_eq!(
        seen.len(),
        total,
        "case \"clamped chain\": expected {total} integrations, got {}",
        seen.len()
    );
    assert_eq!(
        seen.iter().cloned().collect::<BTreeSet<_>>().len(),
        total,
        "case \"clamped chain\": expected every integration exactly once, got {seen:?}"
    );
}

#[tokio::test]
async fn get_integrations_refuses_an_unknown_api_and_a_bad_max_results() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "a refusals host",
        &json!({ "name": "refusals-listing-api", "protocolType": "HTTP" }),
    )
    .await;

    for case in ["nosuchapi00", "", "0123456789"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{case}/integrations"),
            None,
        )
        .await;
        if case.is_empty() {
            // No api id at all does not address this operation at all; whatever the
            // router answers, it is never a listing.
            assert_ne!(
                status, 200,
                "case \"no api id\": expected a refusal, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetIntegrations, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // `MaxResults` is modelled as `__string`, so the numeric rule is the service's: a
    // positive decimal integer. Each row breaks it in a different way.
    for raw in ["0", "-1", "abc", "1.5", "%20"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case maxResults={raw:?}: expected 400 from GetIntegrations, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case maxResults={raw:?}: expected BadRequestException, got {body}"
        );
    }

    // Each published bound of the accepted range, and one past the page maximum.
    for raw in ["1", "100", "101", "2147483647"] {
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations?maxResults={raw}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case maxResults={raw:?}: expected 200 from GetIntegrations, got {status} {body}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `IntegrationResponse` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateIntegrationResponse` persists an integration response under the
//!   integration that owns it and answers 201 with the `IntegrationResponseId` it
//!   minted**; `GetIntegrationResponse` reports that same record, member for member.
//!   Nothing is echoed from the request without going through the store, and a member
//!   the request left out is reported as carrying nothing rather than as a value
//!   invented for it.
//! * An integration response's key identifies it WITHIN its integration: a second
//!   create under the same key on the same integration is a 409, while the same key on
//!   another integration is a fresh integration response, and an id is only resolvable
//!   through the integration that owns it.
//! * An integration response is a WebSocket-only resource
//!   (`IntegrationResponse$ContentHandlingStrategy`: "Supported only for WebSocket
//!   APIs"), so a create against an api of another protocol type is refused 400 and
//!   stores nothing.
//! * A `TemplateSelectionExpression` is a selection EXPRESSION, so it may name only the
//!   variable namespaces the service evaluates; one naming anything else is refused 400
//!   and stores nothing.
//! * An integration holds at most `MAX_INTEGRATION_RESPONSES_PER_INTEGRATION`
//!   integration responses: past that bound a create is refused 429
//!   `TooManyRequestsException` and stores nothing, so no loop of creates grows the
//!   store without end.
//!
//! The rows vary what the rules generalise over: the integration response key's form,
//! which optional members the request carries, which `ContentHandlingStrategy` value it
//! names, and how many response templates and response parameters it names (none, one,
//! several). The recorded WebSocket case that filed the issue — a `$default` integration
//! response on an integration of a WebSocket api — is one row.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The most integration responses one integration holds, after which a create is
/// refused: the crate's own bound, so the test tracks it rather than restating a number.
const MAX_INTEGRATION_RESPONSES_PER_INTEGRATION: usize =
    service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_INTEGRATION_RESPONSES_PER_INTEGRATION;

/// Every value `ContentHandlingStrategy` accepts, from the model's enum
/// (`smithy_query(query="shape", shape="ContentHandlingStrategy")`:
/// `CONVERT_TO_BINARY | CONVERT_TO_TEXT`). Both are exercised by the rows below.
const CONTENT_HANDLING_STRATEGY_VALUES: &[&str] = &["CONVERT_TO_BINARY", "CONVERT_TO_TEXT"];

/// One row: the `CreateIntegrationResponse` request, and the members the stored
/// integration response must report back.
struct Row {
    case: &'static str,
    request: Value,
    /// Members the answer — and the later `GetIntegrationResponse` — must carry.
    expected: fn() -> Vec<(&'static str, Value)>,
    /// Members the request named none of, so the stored record carries nothing for
    /// them: reported either absent or as an empty collection, never as a value the
    /// service made up.
    expect_empty: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the $default integration response key, only the required member",
            request: json!({ "integrationResponseKey": "$default" }),
            expected: || vec![("integrationResponseKey", json!("$default"))],
            expect_empty: &[
                "contentHandlingStrategy",
                "responseParameters",
                "responseTemplates",
                "templateSelectionExpression",
            ],
        },
        Row {
            case: "a status-code selection key converting the payload to text",
            request: json!({
                "integrationResponseKey": "/400/",
                "contentHandlingStrategy": CONTENT_HANDLING_STRATEGY_VALUES[1]
            }),
            expected: || {
                vec![
                    ("integrationResponseKey", json!("/400/")),
                    (
                        "contentHandlingStrategy",
                        json!(CONTENT_HANDLING_STRATEGY_VALUES[1]),
                    ),
                ]
            },
            expect_empty: &[
                "responseParameters",
                "responseTemplates",
                "templateSelectionExpression",
            ],
        },
        Row {
            case: "a class pattern selection key converting the payload to binary, with a \
                   template selection expression",
            request: json!({
                "integrationResponseKey": "/2\\d\\d/",
                "contentHandlingStrategy": CONTENT_HANDLING_STRATEGY_VALUES[0],
                "templateSelectionExpression": "$request.body.action"
            }),
            expected: || {
                vec![
                    ("integrationResponseKey", json!("/2\\d\\d/")),
                    (
                        "contentHandlingStrategy",
                        json!(CONTENT_HANDLING_STRATEGY_VALUES[0]),
                    ),
                    (
                        "templateSelectionExpression",
                        json!("$request.body.action"),
                    ),
                ]
            },
            expect_empty: &["responseParameters", "responseTemplates"],
        },
        Row {
            case: "a wildcard pattern selection key with one response template",
            request: json!({
                "integrationResponseKey": "/5../",
                "responseTemplates": { "application/json": "{\"ok\":true}" }
            }),
            expected: || {
                vec![
                    ("integrationResponseKey", json!("/5../")),
                    (
                        "responseTemplates",
                        json!({ "application/json": "{\"ok\":true}" }),
                    ),
                ]
            },
            expect_empty: &[
                "contentHandlingStrategy",
                "responseParameters",
                "templateSelectionExpression",
            ],
        },
        Row {
            case: "several response templates and response parameters",
            request: json!({
                "integrationResponseKey": "/3\\d\\d/",
                "templateSelectionExpression": "\\$default",
                "responseTemplates": {
                    "application/json": "{\"body\":$input.json('$')}",
                    "text/plain": "$input.body"
                },
                "responseParameters": {
                    "method.response.header.x-trace":
                        "integration.response.header.x-trace",
                    "method.response.header.x-hint": "'static-hint'"
                }
            }),
            expected: || {
                vec![
                    ("integrationResponseKey", json!("/3\\d\\d/")),
                    ("templateSelectionExpression", json!("\\$default")),
                    (
                        "responseTemplates",
                        json!({
                            "application/json": "{\"body\":$input.json('$')}",
                            "text/plain": "$input.body"
                        }),
                    ),
                    (
                        "responseParameters",
                        json!({
                            "method.response.header.x-trace":
                                "integration.response.header.x-trace",
                            "method.response.header.x-hint": "'static-hint'"
                        }),
                    ),
                ]
            },
            expect_empty: &["contentHandlingStrategy"],
        },
        Row {
            case: "explicitly empty collections, which store nothing",
            request: json!({
                "integrationResponseKey": "/(404|410)/",
                "responseTemplates": {},
                "responseParameters": {}
            }),
            expected: || vec![("integrationResponseKey", json!("/(404|410)/"))],
            expect_empty: &[
                "contentHandlingStrategy",
                "responseParameters",
                "responseTemplates",
                "templateSelectionExpression",
            ],
        },
    ]
}

/// A WebSocket api, the protocol whose integrations carry integration responses.
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

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
    as_str(&created, "integrationId")
}

/// Whether `member` carries nothing: absent, null, or an empty collection.
fn carries_nothing(body: &Value, member: &str) -> bool {
    body.get(member).is_none_or(|value| match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    })
}

#[tokio::test]
async fn create_integration_response_persists_the_record_get_reports_it_back() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "the owning api",
        &websocket_api("integration-response-host"),
    )
    .await;
    let integration_id = create_integration(
        &emulator,
        "the owning integration",
        &api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;

    let mut minted = BTreeSet::new();
    for row in rows() {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateIntegrationResponse, got {status} {created}",
            row.case
        );

        let response_id = as_str(&created, "integrationResponseId");
        assert_minted_id(row.case, "IntegrationResponseId", &response_id);
        assert!(
            minted.insert(response_id.clone()),
            "case {:?}: IntegrationResponseId {response_id:?} was minted twice",
            row.case
        );

        for (member, value) in (row.expected)() {
            assert_eq!(
                created.get(member),
                Some(&value),
                "case {:?}: expected CreateIntegrationResponse to report {member}={value}, \
                 got {created}",
                row.case
            );
        }
        for member in row.expect_empty {
            assert!(
                carries_nothing(&created, member),
                "case {:?}: the request named no {member}, so it must carry nothing, got \
                 {created}",
                row.case
            );
        }

        // The same record, read back through the integration that owns it.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!(
                "/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses/\
                 {response_id}"
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegrationResponse, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, created,
            "case {:?}: GetIntegrationResponse must report the record \
             CreateIntegrationResponse stored",
            row.case
        );
    }

    // Creating an integration response leaves the integration itself as it was: the
    // response is a child resource, not a member of the integration.
    let (status, integration) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegration, got {status} {integration}"
    );
    assert_eq!(
        as_str(&integration, "integrationType"),
        "MOCK",
        "expected the integration to be unchanged by its responses, got {integration}"
    );
}

#[tokio::test]
async fn an_integration_response_belongs_to_the_one_integration_that_owns_it() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &websocket_api("owner-ws-api")).await;
    let other_api = create_api(&emulator, "another api", &websocket_api("other-ws-api")).await;
    let integration_id = create_integration(
        &emulator,
        "the owning integration",
        &api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let other_integration = create_integration(
        &emulator,
        "another integration",
        &api_id,
        json!({ "integrationType": "HTTP", "integrationUri": "https://example.com/hook" }),
    )
    .await;

    let (status, created) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        Some(json!({ "integrationResponseKey": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateIntegrationResponse, got {status} {created}"
    );
    let response_id = as_str(&created, "integrationResponseId");

    // The key identifies the integration response within its integration, so the same
    // integration cannot carry a second response under it.
    let (status, conflict) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
        Some(json!({ "integrationResponseKey": "$default" })),
    )
    .await;
    assert_eq!(
        status, 409,
        "expected 409 from a second integration response under one key, got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictException",
        "expected ConflictException, got {conflict}"
    );

    // Neither another integration of the same api nor another api resolves that id.
    for (case, uri) in [
        (
            "another integration of the same api",
            format!(
                "/v2/apis/{api_id}/integrations/{other_integration}/integrationresponses/\
                 {response_id}"
            ),
        ),
        (
            "another api",
            format!(
                "/v2/apis/{other_api}/integrations/{integration_id}/integrationresponses/\
                 {response_id}"
            ),
        ),
        (
            "an integration response id nothing minted",
            format!(
                "/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses/\
                 nosuchresp0"
            ),
        ),
        (
            "an integration id nothing minted",
            format!(
                "/v2/apis/{api_id}/integrations/nosuchintg0/integrationresponses/{response_id}"
            ),
        ),
        (
            "an api id nothing minted",
            format!(
                "/v2/apis/nosuchapi00/integrations/{integration_id}/integrationresponses/\
                 {response_id}"
            ),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetIntegrationResponse, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // The key is free on another integration: that create succeeds, and each
    // integration reports its OWN response.
    let (status, sibling) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{other_integration}/integrationresponses"),
        Some(json!({
            "integrationResponseKey": "$default",
            "contentHandlingStrategy": "CONVERT_TO_TEXT"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for the same key on another integration, got {status} {sibling}"
    );
    let sibling_id = as_str(&sibling, "integrationResponseId");
    let (status, fetched) = call(
        &emulator,
        "GET",
        &format!(
            "/v2/apis/{api_id}/integrations/{other_integration}/integrationresponses/{sibling_id}"
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegrationResponse on the sibling integration, got {status} \
         {fetched}"
    );
    assert_eq!(
        fetched, sibling,
        "the sibling integration must report its own stored integration response"
    );
    let (status, owner) = call(
        &emulator,
        "GET",
        &format!(
            "/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses/{response_id}"
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegrationResponse on the owning integration, got {status} \
         {owner}"
    );
    assert_eq!(
        owner, created,
        "the owning integration's response must be unchanged by the sibling create, got {owner}"
    );

    // Creating under an integration or an api that does not exist creates nothing.
    for (case, uri) in [
        (
            "an integration id nothing minted",
            format!("/v2/apis/{api_id}/integrations/nosuchintg0/integrationresponses"),
        ),
        (
            "an api id nothing minted",
            format!("/v2/apis/nosuchapi00/integrations/{integration_id}/integrationresponses"),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({ "integrationResponseKey": "$default" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from CreateIntegrationResponse, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_integration_response_is_refused_on_an_api_that_is_not_a_websocket_api() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "an http api",
        &json!({ "name": "http-no-integration-responses", "protocolType": "HTTP" }),
    )
    .await;
    let integration_id = create_integration(
        &emulator,
        "an integration of an http api",
        &api_id,
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/items",
            "payloadFormatVersion": "1.0"
        }),
    )
    .await;

    for (case, request) in [
        (
            "the $default key",
            json!({ "integrationResponseKey": "$default" }),
        ),
        (
            "a key with response templates",
            json!({
                "integrationResponseKey": "/200/",
                "responseTemplates": { "application/json": "$input.body" }
            }),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateIntegrationResponse on an HTTP api, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // A refused create stored nothing: the integration holds no response to read.
    let (status, body) = call(
        &emulator,
        "GET",
        &format!(
            "/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses/nosuchresp0"
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "expected the integration to hold no integration response, got {status} {body}"
    );
}

#[tokio::test]
async fn a_template_selection_expression_naming_an_unknown_variable_is_refused() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "the owning api",
        &websocket_api("expression-ws-api"),
    )
    .await;
    let integration_id = create_integration(
        &emulator,
        "the owning integration",
        &api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses");

    // An unescaped `$`-token has to name a variable namespace the service evaluates;
    // these name none, so each is refused and stores nothing.
    for (case, expression) in [
        ("an unescaped selection key", "$default"),
        ("a namespace the service does not define", "$input.body"),
        ("a braced unknown namespace", "${nosuchthing.value}"),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({
                "integrationResponseKey": format!("key-{case}"),
                "templateSelectionExpression": expression
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for the template selection expression \
             {expression:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // The path's own resources are resolved first: a request naming an integration that
    // does not exist is answered about THAT, whatever its body carries.
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/nosuchintg0/integrationresponses"),
        Some(json!({
            "integrationResponseKey": "$default",
            "templateSelectionExpression": "$nosuchthing"
        })),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 for an unknown integration even with a bad expression, got {status} \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );

    // The namespaces it DOES evaluate are accepted, and an escaped literal is not a
    // variable reference at all.
    for (index, (case, expression)) in [
        ("a request variable", "$request.body.action"),
        ("a context variable", "$context.routeKey"),
        ("an escaped literal", "\\$default"),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({
                // One status-code key per row: an `IntegrationResponseKey` is `$default`
                // or a pattern that can match a status, and one integration holds at most
                // one response per key.
                "integrationResponseKey": format!("/{}/", 200 + index),
                "templateSelectionExpression": expression
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 for the template selection expression \
             {expression:?}, got {status} {body}"
        );
        assert_eq!(
            as_str(&body, "templateSelectionExpression"),
            expression,
            "case {case:?}: expected the stored expression to be reported back, got {body}"
        );
    }
}

#[tokio::test]
async fn an_integration_holds_at_most_the_bounded_number_of_integration_responses() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &websocket_api("bounded-ws-api")).await;
    let integration_id = create_integration(
        &emulator,
        "the owning integration",
        &api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let uri = format!("/v2/apis/{api_id}/integrations/{integration_id}/integrationresponses");

    // Fill the integration to its bound, each response under a key of its own.
    for index in 0..MAX_INTEGRATION_RESPONSES_PER_INTEGRATION {
        let (status, body) = call(
            &emulator,
            "POST",
            &uri,
            Some(json!({ "integrationResponseKey": format!("/{}/", 100 + index) })),
        )
        .await;
        assert_eq!(
            status, 201,
            "integration response {index}: expected 201 while under the bound of \
             {MAX_INTEGRATION_RESPONSES_PER_INTEGRATION}, got {status} {body}"
        );
    }

    // One past it is refused, and the refusal stores nothing.
    let (status, body) = call(
        &emulator,
        "POST",
        &uri,
        Some(json!({ "integrationResponseKey": "/400/" })),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected 429 past the bound of {MAX_INTEGRATION_RESPONSES_PER_INTEGRATION}, got \
         {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {body}"
    );
    assert_eq!(
        as_str(&body, "limitType"),
        "IntegrationResponse",
        "expected the refusal to name the limit it exceeded, got {body}"
    );

    // Another integration of the same api is unaffected: the bound is per integration.
    let sibling = create_integration(
        &emulator,
        "another integration",
        &api_id,
        json!({ "integrationType": "MOCK" }),
    )
    .await;
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations/{sibling}/integrationresponses"),
        Some(json!({ "integrationResponseKey": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 on an integration of its own, got {status} {body}"
    );
}

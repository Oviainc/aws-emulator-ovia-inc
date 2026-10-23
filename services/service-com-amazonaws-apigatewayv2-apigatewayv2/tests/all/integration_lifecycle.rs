// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `Integration` lifecycle.
//!
//! The rules these cases assert:
//!
//! * **`CreateIntegration` persists an integration under its api and answers 201 with the
//!   `IntegrationId` it minted**, resolving the members the request left out to the values
//!   the model documents for them: `ConnectionType` is `INTERNET`, and `TimeoutInMillis`
//!   is 30,000 ms on an HTTP api and 29,000 ms on a WebSocket one. `GetIntegration`
//!   reports that same record, member for member — nothing is echoed from the request
//!   without going through the store.
//! * An integration belongs to ONE api: its id is not found through another api's path,
//!   and an api that does not exist is a 404 rather than an integration created nowhere.
//! * A request that names no endpoint for a type that needs one, no payload format
//!   version on an HTTP api, or a `VPC_LINK` connection with no link, is refused 400.
//!
//! The rows vary what the rule generalises over: the api's protocol type, the integration
//! type, which optional members the request carries (uri, method, subtype, credentials,
//! templates, response parameters, TLS configuration, timeout) and whether the timeout is
//! given or defaulted. The recorded case that filed the issue — an `AWS_PROXY` integration
//! fronting a Lambda function on an HTTP api — is one row.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

/// The `TimeoutInMillis` an HTTP api's integration carries when the request names none,
/// as `CreateIntegration$TimeoutInMillis` documents it ("30 seconds for HTTP APIs").
const DEFAULT_HTTP_TIMEOUT_MILLIS: i64 = 30_000;

/// The same for a WebSocket api ("29 seconds for WebSocket APIs").
const DEFAULT_WEBSOCKET_TIMEOUT_MILLIS: i64 = 29_000;

/// One row: the api to create the integration under, the request, and the members the
/// stored integration must report.
struct Row {
    case: &'static str,
    /// The `CreateApi` request the integration is created under.
    api: Value,
    /// The `CreateIntegration` request.
    request: Value,
    /// Members the answer — and the later `GetIntegration` — must carry.
    expected: fn() -> Vec<(&'static str, Value)>,
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an AWS_PROXY integration fronting a Lambda function on an HTTP api",
            api: http_api("recorder-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri":
                    "arn:aws:lambda:us-east-1:000000000000:function:recorder",
                "payloadFormatVersion": "2.0"
            }),
            expected: || {
                vec![
                    ("integrationType", json!("AWS_PROXY")),
                    (
                        "integrationUri",
                        json!("arn:aws:lambda:us-east-1:000000000000:function:recorder"),
                    ),
                    ("payloadFormatVersion", json!("2.0")),
                    // Defaulted, not echoed: the request named neither.
                    ("connectionType", json!("INTERNET")),
                    ("timeoutInMillis", json!(DEFAULT_HTTP_TIMEOUT_MILLIS)),
                    ("apiGatewayManaged", json!(false)),
                ]
            },
        },
        Row {
            case: "an HTTP_PROXY integration with an explicit method and timeout",
            api: http_api("proxy-api"),
            request: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0",
                "timeoutInMillis": 15_000,
                "description": "a proxy to an upstream service"
            }),
            expected: || {
                vec![
                    ("integrationType", json!("HTTP_PROXY")),
                    ("integrationMethod", json!("ANY")),
                    ("timeoutInMillis", json!(15_000)),
                    ("description", json!("a proxy to an upstream service")),
                    ("connectionType", json!("INTERNET")),
                ]
            },
        },
        Row {
            case: "an HTTP api AWS_PROXY integration with a subtype, parameters and TLS",
            api: http_api("subtype-api"),
            request: json!({
                "integrationType": "AWS_PROXY",
                "integrationSubtype": "SQS-SendMessage",
                "integrationUri": "arn:aws:sqs:us-east-1:000000000000:a-queue",
                "payloadFormatVersion": "1.0",
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw",
                "requestParameters": {
                    "QueueUrl": "$request.header.queue",
                    "MessageBody": "$request.body.message"
                },
                "responseParameters": {
                    "200": { "append:header.x-trace": "$context.requestId" }
                },
                "tlsConfig": { "serverNameToVerify": "upstream.example.com" }
            }),
            expected: || {
                vec![
                    ("integrationSubtype", json!("SQS-SendMessage")),
                    ("credentialsArn", json!("arn:aws:iam::000000000000:role/apigw")),
                    (
                        "requestParameters",
                        json!({
                            "QueueUrl": "$request.header.queue",
                            "MessageBody": "$request.body.message"
                        }),
                    ),
                    (
                        "responseParameters",
                        json!({ "200": { "append:header.x-trace": "$context.requestId" } }),
                    ),
                    ("tlsConfig", json!({ "serverNameToVerify": "upstream.example.com" })),
                    ("timeoutInMillis", json!(DEFAULT_HTTP_TIMEOUT_MILLIS)),
                ]
            },
        },
        Row {
            case: "a MOCK integration on a WebSocket api, which names no endpoint",
            api: websocket_api("mock-socket-api"),
            request: json!({ "integrationType": "MOCK" }),
            expected: || {
                vec![
                    ("integrationType", json!("MOCK")),
                    ("connectionType", json!("INTERNET")),
                    // The WebSocket default, not the HTTP one.
                    ("timeoutInMillis", json!(DEFAULT_WEBSOCKET_TIMEOUT_MILLIS)),
                ]
            },
        },
        Row {
            case: "a WebSocket AWS integration with templates and a passthrough behaviour",
            api: websocket_api("template-socket-api"),
            request: json!({
                "integrationType": "AWS",
                "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:ws",
                "integrationMethod": "POST",
                "passthroughBehavior": "WHEN_NO_TEMPLATES",
                "contentHandlingStrategy": "CONVERT_TO_TEXT",
                "requestTemplates": { "application/json": "{\"body\": $input.json('$')}" },
                "templateSelectionExpression": "$request.body.action"
            }),
            expected: || {
                vec![
                    ("integrationType", json!("AWS")),
                    ("passthroughBehavior", json!("WHEN_NO_TEMPLATES")),
                    ("contentHandlingStrategy", json!("CONVERT_TO_TEXT")),
                    (
                        "requestTemplates",
                        json!({ "application/json": "{\"body\": $input.json('$')}" }),
                    ),
                    ("templateSelectionExpression", json!("$request.body.action")),
                    ("timeoutInMillis", json!(DEFAULT_WEBSOCKET_TIMEOUT_MILLIS)),
                ]
            },
        },
    ]
}

/// Create one api and answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

#[tokio::test]
async fn create_integration_persists_the_record_get_integration_reports_back() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(row.request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from CreateIntegration, got {status} {created}",
            row.case
        );
        let integration_id = as_str(&created, "integrationId");
        assert_minted_id(row.case, "IntegrationId", &integration_id);

        // The same record, read back through the api that owns it.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetIntegration, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "integrationId"),
            integration_id,
            "case {:?}: expected the stored integration's own id, got {fetched}",
            row.case
        );

        for (member, value) in (row.expected)() {
            assert_eq!(
                created.get(member),
                Some(&value),
                "case {:?}: expected CreateIntegration to report {member}={value}, got {created}",
                row.case
            );
            assert_eq!(
                fetched.get(member),
                Some(&value),
                "case {:?}: expected GetIntegration to report {member}={value}, got {fetched}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn an_integration_belongs_to_the_one_api_that_owns_it() {
    let emulator = emulator();

    let first = create_api(&emulator, "the owning api", &http_api("owner-api")).await;
    let second = create_api(&emulator, "another api", &http_api("other-api")).await;

    let mut minted = Vec::new();
    for index in 0..3 {
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{first}/integrations"),
            Some(json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": format!("https://example.com/upstream/{index}"),
                "payloadFormatVersion": "1.0"
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "integration {index}: expected 201 from CreateIntegration, got {status} {created}"
        );
        minted.push(as_str(&created, "integrationId"));
    }
    // Each create minted a distinct id within the api.
    for (index, id) in minted.iter().enumerate() {
        assert_eq!(
            minted.iter().filter(|other| *other == id).count(),
            1,
            "integration {index}: expected distinct ids, got {minted:?}"
        );
    }

    for (case, uri) in [
        (
            "an integration of another api",
            format!("/v2/apis/{second}/integrations/{}", minted[0]),
        ),
        (
            "an integration id nothing minted",
            format!("/v2/apis/{first}/integrations/nosuchint0"),
        ),
        (
            "an api that does not exist",
            format!("/v2/apis/nosuchapi00/integrations/{}", minted[0]),
        ),
    ] {
        let (status, body) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // Creating under an api that does not exist creates nothing.
    let (status, body) = call(
        &emulator,
        "POST",
        "/v2/apis/nosuchapi00/integrations",
        Some(json!({
            "integrationType": "MOCK"
        })),
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 from CreateIntegration on an unknown api, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );
}

#[tokio::test]
async fn an_integration_request_missing_what_its_type_needs_is_refused() {
    let emulator = emulator();

    let http = create_api(&emulator, "an http api", &http_api("refusals-http-api")).await;
    let socket = create_api(
        &emulator,
        "a websocket api",
        &websocket_api("refusals-socket-api"),
    )
    .await;

    for (case, api_id, request) in [
        (
            "an AWS_PROXY integration naming no endpoint",
            &http,
            json!({ "integrationType": "AWS_PROXY", "payloadFormatVersion": "2.0" }),
        ),
        (
            "an HTTP integration naming no endpoint",
            &socket,
            json!({ "integrationType": "HTTP" }),
        ),
        (
            "an HTTP api integration naming no payload format version",
            &http,
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream"
            }),
        ),
        (
            "a VPC_LINK connection naming no link",
            &http,
            json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0",
                "connectionType": "VPC_LINK"
            }),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }

    // A refused create stored nothing: the api that only saw refusals has no
    // integration to read.
    let (status, body) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{http}/integrations/nosuchint0"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "expected the api to hold no integration, got {status} {body}"
    );
}

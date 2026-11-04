// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the error shape this service refuses a bad request with.
//!
//! The rule: **a request refused for failing a rule this service's model declares — a
//! required member absent, or a value outside a declared enum or range — is answered
//! with the error shape the model declares for a request it cannot accept:
//! `BadRequestException`, HTTP 400, whose message names the member and the constraint it
//! violated.** This model declares no `ValidationException` on any operation
//! (`smithy_query(query="operation", operation="CreateApi")` lists exactly
//! BadRequestException/ConflictException/NotFoundException/TooManyRequestsException, and
//! so does every other), so no refusal may carry one: an SDK resolves a modelled error
//! shape by that code, and a code the model does not declare matches no typed exception.
//!
//! The rows span what the rule generalises over: the kind of violation (absent required
//! member, a value outside an enum, a value outside a numeric range), where the member
//! sits (the body of a create, a nested member), the operation refusing it, and both
//! published bounds of the one range this surface carries — plus, from the model's own
//! catalogs, every value of the enums, each of which must be ACCEPTED. The recorded
//! cases that filed the issue — `ProtocolType: 'FTP'` and `TimeoutInMillis: 30001` — are
//! ONE row each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The published bounds of `IntegerWithLengthBetween50And30000`, the shape
/// `CreateIntegration$TimeoutInMillis` targets (`range` 50..30000).
const TIMEOUT_MIN: i64 = 50;
const TIMEOUT_MAX: i64 = 30_000;

/// Create one HTTP api and answer its `ApiId`.
async fn create_http_api(emulator: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// One refusal row: the request, and where it is sent.
struct RefusalRow {
    case: &'static str,
    method: &'static str,
    /// `{api}` is replaced with the `ApiId` of a live HTTP api.
    uri: &'static str,
    body: Value,
}

fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "a ProtocolType outside the enum",
            method: "POST",
            uri: "/v2/apis",
            body: json!({ "name": "bad-protocol", "protocolType": "FTP" }),
        },
        RefusalRow {
            case: "a ProtocolType of the right word in the wrong case",
            method: "POST",
            uri: "/v2/apis",
            body: json!({ "name": "bad-protocol-case", "protocolType": "http" }),
        },
        RefusalRow {
            case: "an empty ProtocolType",
            method: "POST",
            uri: "/v2/apis",
            body: json!({ "name": "empty-protocol", "protocolType": "" }),
        },
        RefusalRow {
            case: "an IpAddressType outside the enum",
            method: "POST",
            uri: "/v2/apis",
            body: json!({
                "name": "bad-ip-type",
                "protocolType": "HTTP",
                "ipAddressType": "IPv4"
            }),
        },
        RefusalRow {
            case: "a required member absent: CreateApi with no Name",
            method: "POST",
            uri: "/v2/apis",
            body: json!({ "protocolType": "HTTP" }),
        },
        RefusalRow {
            case: "a required member absent: CreateApi with no ProtocolType",
            method: "POST",
            uri: "/v2/apis",
            body: json!({ "name": "no-protocol" }),
        },
        RefusalRow {
            case: "a required member absent: CreateRoute with no RouteKey",
            method: "POST",
            uri: "/v2/apis/{api}/routes",
            body: json!({ "operationName": "keyless" }),
        },
        RefusalRow {
            case: "a required member absent: CreateIntegration with no IntegrationType",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({ "integrationUri": "https://example.com/upstream" }),
        },
        RefusalRow {
            case: "an IntegrationType outside the enum",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({
                "integrationType": "FTP",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0"
            }),
        },
        RefusalRow {
            case: "a ConnectionType outside the enum",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0",
                "connectionType": "PRIVATE"
            }),
        },
        RefusalRow {
            case: "a TimeoutInMillis one above the published maximum",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0",
                "timeoutInMillis": TIMEOUT_MAX + 1
            }),
        },
        RefusalRow {
            case: "a TimeoutInMillis one below the published minimum",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0",
                "timeoutInMillis": TIMEOUT_MIN - 1
            }),
        },
        RefusalRow {
            case: "a TimeoutInMillis far above the published maximum",
            method: "POST",
            uri: "/v2/apis/{api}/integrations",
            body: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0",
                "timeoutInMillis": 600_000
            }),
        },
        RefusalRow {
            case: "an AuthorizationType outside the enum",
            method: "POST",
            uri: "/v2/apis/{api}/routes",
            body: json!({ "routeKey": "GET /bad-auth", "authorizationType": "LAMBDA" }),
        },
        RefusalRow {
            case: "an AuthorizerType outside the enum",
            method: "POST",
            uri: "/v2/apis/{api}/authorizers",
            body: json!({
                "name": "bad-type",
                "authorizerType": "LAMBDA",
                "identitySource": ["$request.header.Authorization"]
            }),
        },
        RefusalRow {
            case: "an IpAddressType outside the enum on a patch",
            method: "PATCH",
            uri: "/v2/apis/{api}",
            body: json!({ "ipAddressType": "both" }),
        },
    ]
}

#[tokio::test]
async fn a_request_failing_a_modelled_constraint_is_refused_as_a_bad_request() {
    let emulator = emulator();
    let api_id = create_http_api(&emulator, "constraint-host").await;

    for row in refusal_rows() {
        let uri = row.uri.replace("{api}", &api_id);
        let (status, body) = call(&emulator, row.method, &uri, Some(row.body.clone())).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {:?}: expected BadRequestException, got {body}",
            row.case
        );
        // The message says which member failed and what it failed, which is what "See
        // the accompanying error message for details" (`BadRequestException`) promises.
        let message = as_str(&body, "message");
        assert!(
            !message.is_empty(),
            "case {:?}: the refusal must carry a message, got {body}",
            row.case
        );
        assert!(
            !body.to_string().contains("ValidationException"),
            "case {:?}: no operation of this service models a ValidationException, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn every_published_value_of_a_constrained_member_is_accepted() {
    use service_com_amazonaws_apigatewayv2_apigatewayv2::ops::types::{
        INTEGRATION_TYPE_VALUES, IP_ADDRESS_TYPE_VALUES, PROTOCOL_TYPE_VALUES,
    };

    let emulator = emulator();

    // Every `ProtocolType` the model declares, each with the members that protocol
    // requires: the catalog comes from the MODEL, so a value added to it is covered
    // here without editing this test.
    for protocol_type in PROTOCOL_TYPE_VALUES {
        let mut request = json!({
            "name": format!("protocol-{protocol_type}"),
            "protocolType": protocol_type
        });
        if *protocol_type != "HTTP" {
            request["routeSelectionExpression"] = json!("$request.body.action");
        }
        let (status, created) = call(&emulator, "POST", "/v2/apis", Some(request)).await;
        assert_eq!(
            status, 201,
            "case {protocol_type:?}: a published ProtocolType must be accepted, got {status} {created}"
        );
    }

    // And every `IpAddressType`.
    for ip_address_type in IP_ADDRESS_TYPE_VALUES {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({
                "name": format!("ip-{ip_address_type}"),
                "protocolType": "HTTP",
                "ipAddressType": ip_address_type
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {ip_address_type:?}: a published IpAddressType must be accepted, got {status} {created}"
        );
        assert_eq!(
            as_str(&created, "ipAddressType"),
            *ip_address_type,
            "case {ip_address_type:?}: the stored IpAddressType must be the one sent, got {created}"
        );
    }

    // Every `IntegrationType`, on an api of the protocol that supports it, and both
    // published bounds of `TimeoutInMillis` — a bound is IN range, so it is accepted.
    let http = create_http_api(&emulator, "integration-type-http").await;
    let (_, socket) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "integration-type-socket",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    let socket = as_str(&socket, "apiId");

    for integration_type in INTEGRATION_TYPE_VALUES {
        for timeout in [TIMEOUT_MIN, TIMEOUT_MAX] {
            // The two proxy types are the ones an HTTP api supports; the other three
            // are "Supported only for WebSocket APIs".
            let on_http = matches!(*integration_type, "AWS_PROXY" | "HTTP_PROXY");
            let api_id = if on_http { &http } else { &socket };
            let mut request = json!({
                "integrationType": integration_type,
                "timeoutInMillis": timeout
            });
            if *integration_type != "MOCK" {
                // "AWS: for integrating the route or method request with an AWS service
                // action" and the same clause on AWS_PROXY: those two name the resource
                // the action runs on, so their URI is an ARN, while HTTP/HTTP_PROXY take
                // "a fully-qualified URL" (`Integration$IntegrationUri`).
                request["integrationUri"] = if matches!(*integration_type, "AWS" | "AWS_PROXY") {
                    json!("arn:aws:lambda:eu-west-1:000000000000:function:handler")
                } else {
                    json!("https://example.com/upstream")
                };
            }
            if on_http {
                request["payloadFormatVersion"] = json!("1.0");
            }
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("/v2/apis/{api_id}/integrations"),
                Some(request),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {integration_type:?} at TimeoutInMillis {timeout}: a published value must be accepted, got {status} {created}"
            );
            assert_eq!(
                created["timeoutInMillis"].as_i64(),
                Some(timeout),
                "case {integration_type:?}: the stored TimeoutInMillis must be the one sent, got {created}"
            );
        }
    }
}

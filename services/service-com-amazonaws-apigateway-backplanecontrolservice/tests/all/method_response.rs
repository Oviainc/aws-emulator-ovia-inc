// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a method's responses: `PutMethodResponse` and
//! `GetMethodResponse`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `method_integration`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table spanning the status codes the modelled `StatusCode` pattern
//! admits (including each published bound and one past it), the presence, size and flag
//! values of the two caller-supplied maps, several responses on one method and several
//! methods on one resource, the documented `method.response.header.{name}` form a
//! response parameter key takes, the conflict a status code that already names a response
//! is, and every identifier these operations resolve over the records that do and do not
//! exist.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_method_response::METHOD_RESPONSE_HEADER_PREFIX;

use crate::method_integration::{get_method, resource_with_a_method};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// `PutMethodResponse` for `status` on the `verb` method of `resource`.
async fn put_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
    request: Value,
) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/{status}"),
            Some(request),
        )
        .await
}

/// `GetMethodResponse` for `status` on the `verb` method of `resource`.
async fn get_method_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/responses/{status}"),
            None,
        )
        .await
}

/// A `method.response.header.{name}` mapping expression for `name`, built from the prefix
/// the handler publishes rather than spelled out per case.
fn response_header(name: &str) -> String {
    format!("{METHOD_RESPONSE_HEADER_PREFIX}{name}")
}

#[tokio::test]
async fn a_method_response_put_on_a_method_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: the status code (several values
    // of the modelled `^[1-5]\d\d$` pattern), the method's verb, which of the two
    // caller-supplied maps the request carries, how many entries each carries, and the
    // flag value of a response parameter (required and optional).
    let cases: Vec<(&str, &str, &str, &str, Value, Value)> = vec![
        (
            "a 200 response with neither map",
            "plain",
            "GET",
            "200",
            json!({}),
            json!({ "statusCode": "200" }),
        ),
        (
            "a 201 response whose maps are present but empty",
            "empty-maps",
            "POST",
            "201",
            json!({ "responseParameters": {}, "responseModels": {} }),
            json!({ "statusCode": "201" }),
        ),
        (
            "a 200 response with one required header",
            "one-header",
            "PUT",
            "200",
            json!({ "responseParameters": { response_header("Content-Type"): true } }),
            json!({
                "statusCode": "200",
                "responseParameters": { response_header("Content-Type"): true }
            }),
        ),
        (
            "a 302 response with a required and an optional header",
            "two-headers",
            "HEAD",
            "302",
            json!({
                "responseParameters": {
                    response_header("Location"): true,
                    response_header("X-Request-Id"): false
                }
            }),
            json!({
                "statusCode": "302",
                "responseParameters": {
                    response_header("Location"): true,
                    response_header("X-Request-Id"): false
                }
            }),
        ),
        (
            "a 400 response with one response model",
            "one-model",
            "DELETE",
            "400",
            json!({ "responseModels": { "application/json": "Error" } }),
            json!({
                "statusCode": "400",
                "responseModels": { "application/json": "Error" }
            }),
        ),
        (
            "a 500 response with both maps, several entries each",
            "both-maps",
            "ANY",
            "500",
            json!({
                "responseParameters": {
                    response_header("X-Amzn-ErrorType"): true,
                    response_header("Retry-After"): false
                },
                "responseModels": {
                    "application/json": "Error",
                    "text/plain": "Empty"
                }
            }),
            json!({
                "statusCode": "500",
                "responseParameters": {
                    response_header("X-Amzn-ErrorType"): true,
                    response_header("Retry-After"): false
                },
                "responseModels": {
                    "application/json": "Error",
                    "text/plain": "Empty"
                }
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-round-trip").await;

    for (case, path_part, verb, status, request, expected) in &cases {
        let resource =
            resource_with_a_method(&gateway, &api, &root, path_part, verb, json!({})).await;
        let created =
            put_method_response(&gateway, &api, &resource, verb, status, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutMethodResponse, got {} with {:?}",
            created.status, created.body
        );
        // The body is the stored record: the status code this call created the response
        // for, plus the entries the request carried, and nothing else.
        assert_eq!(
            &created.body, expected,
            "case {case}: expected the method response {expected:?}, got {:?}",
            created.body
        );

        // Read back through GetMethodResponse, and through the method that owns it.
        let read = get_method_response(&gateway, &api, &resource, verb, status).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetMethodResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: GetMethodResponse must report the stored response, got {:?} after {:?}",
            read.body, created.body
        );
        let method = get_method(&gateway, &api, &resource, verb).await;
        assert_eq!(
            method.body["methodResponses"][status], created.body,
            "case {case}: the method must carry the stored response at {status}, got {:?}",
            method.body["methodResponses"]
        );
    }
}

#[tokio::test]
async fn the_status_codes_a_method_response_takes_are_the_ones_the_pattern_admits() {
    // The modelled `StatusCode` pattern is `^[1-5]\d\d$`
    // (`smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")`), so
    // the accepted rows are each published bound — the lowest and the highest code it
    // admits — and values inside it, and the refused rows are one past each bound and the
    // lengths it does not admit.
    let accepted = ["100", "199", "200", "399", "500", "599"];
    let refused = ["099", "600", "000", "999", "20", "2000", "abc"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-status-codes").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;

    for status in accepted {
        let created =
            put_method_response(&gateway, &api, &resource, "GET", status, json!({})).await;
        assert_eq!(
            created.status, 201,
            "case PutMethodResponse({status}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.body["statusCode"].as_str(),
            Some(status),
            "case PutMethodResponse({status}): expected the status code back, got {:?}",
            created.body["statusCode"]
        );
        let read = get_method_response(&gateway, &api, &resource, "GET", status).await;
        assert_eq!(
            read.body["statusCode"].as_str(),
            Some(status),
            "case GetMethodResponse({status}): expected the stored response, got {:?}",
            read.body
        );
    }

    // Every accepted code is a response of its own: one method carries them all at once.
    let method = get_method(&gateway, &api, &resource, "GET").await;
    for status in accepted {
        assert_eq!(
            method.body["methodResponses"][status]["statusCode"].as_str(),
            Some(status),
            "case GetMethod(responses): expected a response at {status}, got {:?}",
            method.body["methodResponses"]
        );
    }

    for status in refused {
        let answer =
            put_method_response(&gateway, &api, &resource, "GET", status, json!({})).await;
        assert_failure(
            &answer,
            400,
            "ValidationException",
            &format!("PutMethodResponse({status})"),
        );
        assert_failure(
            &get_method_response(&gateway, &api, &resource, "GET", status).await,
            400,
            "ValidationException",
            &format!("GetMethodResponse({status})"),
        );
    }
}

#[tokio::test]
async fn a_response_parameter_key_must_be_a_method_response_header_mapping_expression() {
    // Rows span the ways a key can fail the documented
    // `method.response.header.{name}` form: no prefix, the request direction, the
    // integration side, another part of the response, an empty header name, and a header
    // name outside the HTTP field-name token.
    let refused: Vec<(&str, String)> = vec![
        ("a bare header name", "Content-Type".to_string()),
        (
            "a request-side mapping expression",
            "method.request.header.Authorization".to_string(),
        ),
        (
            "an integration-side mapping expression",
            "integration.response.header.X-Custom".to_string(),
        ),
        (
            "another part of the response",
            "method.response.body.message".to_string(),
        ),
        ("an empty header name", response_header("")),
        (
            "a header name carrying a space",
            response_header("X Custom"),
        ),
        (
            "a header name carrying a colon",
            response_header("X:Custom"),
        ),
    ];
    // Accepted keys: header names built from the HTTP field-name token of RFC 9110 §5.1,
    // which is what "a valid … header name" means for a `responseParameters` key.
    let accepted = [
        "Content-Type",
        "X-Custom-Header",
        "x_api_version",
        "If-None-Match",
        "A.B~C",
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-parameters").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;

    for (case, key) in &refused {
        let answer = put_method_response(
            &gateway,
            &api,
            &resource,
            "GET",
            "200",
            json!({ "responseParameters": { key: true } }),
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", case);
        // A refused request stores nothing, so the method still has no 200 response.
        assert_failure(
            &get_method_response(&gateway, &api, &resource, "GET", "200").await,
            404,
            "NotFoundException",
            &format!("{case} / GetMethodResponse after the refusal"),
        );
    }

    for (index, name) in accepted.iter().enumerate() {
        // Each accepted key goes on its own status code, because a status code names at
        // most one response of a method.
        let status = format!("2{index:02}");
        let key = response_header(name);
        let created = put_method_response(
            &gateway,
            &api,
            &resource,
            "GET",
            &status,
            json!({ "responseParameters": { key.clone(): false } }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case PutMethodResponse({key}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        let read = get_method_response(&gateway, &api, &resource, "GET", &status).await;
        assert_eq!(
            read.body["responseParameters"],
            json!({ key.clone(): false }),
            "case GetMethodResponse({key}): expected the stored parameter, got {:?}",
            read.body["responseParameters"]
        );
    }
}

#[tokio::test]
async fn a_status_code_that_already_names_a_response_is_a_conflict_and_the_record_stands() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-conflict").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;

    let first = put_method_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseModels": { "application/json": "Empty" } }),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "case the first put: expected 201, got {} with {:?}",
        first.status, first.body
    );

    // Rows differ in what the second request carries: a put for a status code that
    // already names a response is a conflict whatever its body says, and it changes
    // nothing.
    let seconds: Vec<(&str, Value)> = vec![
        ("the same body again", json!({ "responseModels": { "application/json": "Empty" } })),
        ("a different body", json!({ "responseParameters": { response_header("X-Custom"): true } })),
        ("an empty body", json!({})),
    ];
    for (case, request) in &seconds {
        let answer =
            put_method_response(&gateway, &api, &resource, "GET", "200", request.clone()).await;
        assert_failure(&answer, 409, "ConflictException", case);
        let read = get_method_response(&gateway, &api, &resource, "GET", "200").await;
        assert_eq!(
            read.body, first.body,
            "case {case}: the stored response must stand after the conflict, got {:?} after {:?}",
            read.body, first.body
        );
    }

    // A DIFFERENT status code on the same method is not a conflict: the key is the
    // status code, not the method.
    let other = put_method_response(&gateway, &api, &resource, "GET", "404", json!({})).await;
    assert_eq!(
        other.status, 201,
        "case another status code: expected 201, got {} with {:?}",
        other.status, other.body
    );

    // Reading a method response does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_method_response(&gateway, &api, &resource, "GET", "200").await;
        assert_eq!(
            read.status, 200,
            "case GetMethodResponse(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetMethodResponse repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_method_response_operation() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "method-response-notfound").await;
    let resource =
        resource_with_a_method(&gateway, &api, &root, "pets", "GET", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier these operations resolve, one unknown at a time — plus
    // the method that exists but carries no response at that status code.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        (
            "PUT",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/responses/200"),
            Some(json!({})),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{missing}/methods/GET/responses/200"),
            Some(json!({})),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/responses/200"),
            Some(json!({})),
        ),
        (
            "GET",
            format!("/restapis/{missing}/resources/{resource}/methods/GET/responses/200"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{missing}/methods/GET/responses/200"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/DELETE/responses/200"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let case = format!("{method} {uri}");
        assert_failure(
            &gateway.call(method, &uri, body).await,
            404,
            "NotFoundException",
            &case,
        );
    }

    // The responses of a method on a deleted API go with it.
    let created = put_method_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    assert_eq!(
        created.status, 201,
        "case PutMethodResponse(before DeleteRestApi): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let deleted = gateway.call("DELETE", &format!("/restapis/{api}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_method_response(&gateway, &api, &resource, "GET", "200").await,
        404,
        "NotFoundException",
        "GetMethodResponse after DeleteRestApi",
    );
}

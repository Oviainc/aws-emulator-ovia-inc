// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an integration's responses: `PutIntegrationResponse` and
//! `GetIntegrationResponse`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi`/`CreateResource`/`PutMethod`/`PutIntegration` helpers are this crate's
//! existing harness, reused from `rest_api_lifecycle`, `method_integration` and
//! `method_response`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the create/read
//! round trip over a table spanning the status codes the modelled `StatusCode` pattern
//! admits (each published bound and one past it), the selection pattern, both
//! `ContentHandlingStrategy` values and the presence and size of the two caller-supplied
//! maps; the documented forms a `responseParameters` key and value take; the replacement a
//! put for a status code that already names a response is; and every identifier these
//! operations resolve over the records that do and do not exist.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::put_integration_response::{
    INTEGRATION_RESPONSE_BODY_PREFIX, INTEGRATION_RESPONSE_HEADER_PREFIX,
};

use crate::method_integration::{get_integration, put_integration, resource_with_a_method};
use crate::method_response::response_header;
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// `PutIntegrationResponse` for `status` on the integration of the `verb` method of
/// `resource`.
///
/// Shared with the sibling `control_plane_deletes` suite, which removes the responses it
/// puts, and with `delete_integration`, which stands responses on the integrations it
/// deletes — so every suite writes an integration response the same way.
pub(crate) async fn put_integration_response(
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
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            Some(request),
        )
        .await
}

/// `GetIntegrationResponse` for `status` on the integration of the `verb` method of
/// `resource`. Shared with the sibling `control_plane_deletes` and `delete_integration`
/// suites, which read a removed response back through it; see
/// [`put_integration_response`].
pub(crate) async fn get_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
) -> Answer {
    gateway
        .call(
            "GET",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            None,
        )
        .await
}

/// `DeleteIntegrationResponse` for `status` on the integration of the `verb` method of
/// `resource`, with `verb` sent exactly as the caller spelled it.
///
/// This crate's one copy of that call, shared with the sibling `control_plane_deletes` and
/// `delete_integration_response` suites, so every suite deletes an integration response
/// the same way; see [`put_integration_response`].
pub(crate) async fn delete_integration_response(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    status: &str,
) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/{verb}/integration/responses/{status}"
            ),
            None,
        )
        .await
}

/// A fresh resource under `root` carrying one `verb` method with a MOCK integration on
/// it, answering its resource id: an integration response hangs off the integration, so
/// every case needs one standing first.
///
/// `pub(crate)` so the sibling `update_integration_response` suite, which patches the
/// responses this harness's integrations carry, stands an integration up the same way.
pub(crate) async fn resource_with_an_integration(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
) -> String {
    let resource = resource_with_a_method(gateway, api, root, path_part, verb, json!({})).await;
    let created = put_integration(gateway, api, &resource, verb, json!({ "type": "MOCK" })).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegration({path_part}/{verb}): expected 201, got {} with {:?}",
        created.status, created.body
    );
    resource
}

/// An `integration.response.header.{name}` mapping expression, built from the prefix the
/// handler publishes rather than spelled out per case.
fn integration_header(name: &str) -> String {
    format!("{INTEGRATION_RESPONSE_HEADER_PREFIX}{name}")
}

/// An `integration.response.body.{JSON-expression}` mapping expression; see
/// [`integration_header`].
fn integration_body(expression: &str) -> String {
    format!("{INTEGRATION_RESPONSE_BODY_PREFIX}{expression}")
}

#[tokio::test]
async fn an_integration_response_put_on_an_integration_is_persisted_and_read_back_member_for_member()
{
    // Rows differ along what the rule generalises over: the status code (several values
    // of the modelled `^[1-5]\d\d$` pattern), the method's verb, whether a selection
    // pattern is set and what it is, which of the two caller-supplied maps the request
    // carries and how many entries each carries, and both values of the modelled
    // `ContentHandlingStrategy`.
    let cases: Vec<(&str, &str, &str, &str, Value, Value)> = vec![
        (
            "a 200 response with nothing but the status code",
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
            json!({ "responseParameters": {}, "responseTemplates": {} }),
            json!({ "statusCode": "201" }),
        ),
        (
            "a 400 response selected by a pattern, with one template",
            "pattern",
            "PUT",
            "400",
            json!({
                "selectionPattern": "4\\d{2}",
                "responseTemplates": { "application/json": "{\"message\": $input.json('$.errorMessage')}" }
            }),
            json!({
                "statusCode": "400",
                "selectionPattern": "4\\d{2}",
                "responseTemplates": { "application/json": "{\"message\": $input.json('$.errorMessage')}" }
            }),
        ),
        (
            "a 302 response mapping a static value, a header and a body expression",
            "parameters",
            "HEAD",
            "302",
            json!({
                "selectionPattern": ".+",
                "responseParameters": {
                    response_header("Access-Control-Allow-Origin"): "'*'",
                    response_header("X-Upstream-Id"): integration_header("X-Amzn-RequestId"),
                    response_header("Location"): integration_body("redirect.url")
                }
            }),
            json!({
                "statusCode": "302",
                "selectionPattern": ".+",
                "responseParameters": {
                    response_header("Access-Control-Allow-Origin"): "'*'",
                    response_header("X-Upstream-Id"): integration_header("X-Amzn-RequestId"),
                    response_header("Location"): integration_body("redirect.url")
                }
            }),
        ),
        (
            "a 500 response with both maps and text content handling",
            "both-maps",
            "DELETE",
            "500",
            json!({
                "selectionPattern": "5\\d{2}",
                "contentHandling": "CONVERT_TO_TEXT",
                "responseParameters": {
                    response_header("X-Amzn-ErrorType"): "'InternalFailure'",
                    response_header("Retry-After"): integration_header("Retry-After")
                },
                "responseTemplates": {
                    "application/json": "{\"message\": \"error\"}",
                    "text/plain": "error"
                }
            }),
            json!({
                "statusCode": "500",
                "selectionPattern": "5\\d{2}",
                "contentHandling": "CONVERT_TO_TEXT",
                "responseParameters": {
                    response_header("X-Amzn-ErrorType"): "'InternalFailure'",
                    response_header("Retry-After"): integration_header("Retry-After")
                },
                "responseTemplates": {
                    "application/json": "{\"message\": \"error\"}",
                    "text/plain": "error"
                }
            }),
        ),
        (
            "a 100 response with binary content handling",
            "binary",
            "ANY",
            "100",
            json!({ "contentHandling": "CONVERT_TO_BINARY" }),
            json!({ "statusCode": "100", "contentHandling": "CONVERT_TO_BINARY" }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-round-trip").await;

    for (case, path_part, verb, status, request, expected) in &cases {
        let resource = resource_with_an_integration(&gateway, &api, &root, path_part, verb).await;
        let created =
            put_integration_response(&gateway, &api, &resource, verb, status, request.clone())
                .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegrationResponse, got {} with {:?}",
            created.status, created.body
        );
        // The body is the stored record: the status code this call created the response
        // for, plus what the request carried, and nothing else.
        assert_eq!(
            &created.body, expected,
            "case {case}: expected the integration response {expected:?}, got {:?}",
            created.body
        );

        // Read back through GetIntegrationResponse, and through the integration that owns
        // it.
        let read = get_integration_response(&gateway, &api, &resource, verb, status).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetIntegrationResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: GetIntegrationResponse must report the stored response, got {:?} after {:?}",
            read.body, created.body
        );
        let integration = get_integration(&gateway, &api, &resource, verb).await;
        assert_eq!(
            integration.body["integrationResponses"][status], created.body,
            "case {case}: the integration must carry the stored response at {status}, got {:?}",
            integration.body["integrationResponses"]
        );
    }
}

#[tokio::test]
async fn the_status_codes_an_integration_response_takes_are_the_ones_the_pattern_admits() {
    // The modelled `StatusCode` pattern is `^[1-5]\d\d$`
    // (`smithy_query(query="shape", shape="com.amazonaws.apigateway#StatusCode")`), so
    // the accepted rows are each published bound — the lowest and the highest code it
    // admits — and values inside it, and the refused rows are one past each bound and the
    // lengths it does not admit.
    let accepted = ["100", "199", "200", "399", "500", "599"];
    let refused = ["099", "600", "000", "999", "20", "2000", "abc"];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-status-codes").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;

    for status in accepted {
        let created =
            put_integration_response(&gateway, &api, &resource, "GET", status, json!({})).await;
        assert_eq!(
            created.status, 201,
            "case PutIntegrationResponse({status}): expected 201, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.body["statusCode"].as_str(),
            Some(status),
            "case PutIntegrationResponse({status}): expected the status code back, got {:?}",
            created.body["statusCode"]
        );
        let read = get_integration_response(&gateway, &api, &resource, "GET", status).await;
        assert_eq!(
            read.body["statusCode"].as_str(),
            Some(status),
            "case GetIntegrationResponse({status}): expected the stored response, got {:?}",
            read.body
        );
    }

    // Every accepted code is a response of its own: one integration carries them all at
    // once.
    let integration = get_integration(&gateway, &api, &resource, "GET").await;
    for status in accepted {
        assert_eq!(
            integration.body["integrationResponses"][status]["statusCode"].as_str(),
            Some(status),
            "case GetIntegration(responses): expected a response at {status}, got {:?}",
            integration.body["integrationResponses"]
        );
    }

    for status in refused {
        // Refused by the CONSTRAINT layer in front of the handler, whose error class is
        // `ValidationException` rather than the operation's own `BadRequestException`.
        assert_failure(
            &put_integration_response(&gateway, &api, &resource, "GET", status, json!({})).await,
            400,
            "ValidationException",
            &format!("PutIntegrationResponse({status})"),
        );
        assert_failure(
            &get_integration_response(&gateway, &api, &resource, "GET", status).await,
            400,
            "ValidationException",
            &format!("GetIntegrationResponse({status})"),
        );
    }
}

#[tokio::test]
async fn a_response_parameter_maps_a_method_response_header_from_a_documented_value_form() {
    // `responseParameters`' own documentation: "The mapping key must match the pattern of
    // method.response.header.{name}, where name is a valid and unique header name. The
    // mapped non-static value must match the pattern of
    // integration.response.header.{name} or integration.response.body.{JSON-expression}",
    // beside "a static value enclosed within a pair of single quotes".
    //
    // The refused rows span the ways a key, then a value, can fail that form.
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a bare header name as the key",
            "Content-Type".to_string(),
            "'application/json'".to_string(),
        ),
        (
            "a request-side mapping expression as the key",
            "method.request.header.Authorization".to_string(),
            "'token'".to_string(),
        ),
        (
            "an integration-side mapping expression as the key",
            integration_header("X-Custom"),
            "'1'".to_string(),
        ),
        (
            "an empty header name in the key",
            response_header(""),
            "'1'".to_string(),
        ),
        (
            "a header name carrying a space in the key",
            response_header("X Custom"),
            "'1'".to_string(),
        ),
        (
            "a bare header name as the value",
            response_header("X-Custom"),
            "X-Amzn-RequestId".to_string(),
        ),
        (
            "a method-side mapping expression as the value",
            response_header("X-Custom"),
            "method.response.header.X-Custom".to_string(),
        ),
        (
            "an integration request expression as the value",
            response_header("X-Custom"),
            "integration.request.header.X-Custom".to_string(),
        ),
        (
            "a body expression carrying the $ prefix",
            response_header("X-Custom"),
            integration_body("$.id"),
        ),
        (
            "an empty body expression",
            response_header("X-Custom"),
            integration_body(""),
        ),
        (
            "an empty integration header name",
            response_header("X-Custom"),
            integration_header(""),
        ),
        (
            "an integration header name carrying a space",
            response_header("X-Custom"),
            integration_header("X Custom"),
        ),
        (
            "a static value opened but not closed",
            response_header("X-Custom"),
            "'static".to_string(),
        ),
        (
            "a single quote on its own",
            response_header("X-Custom"),
            "'".to_string(),
        ),
    ];
    // Accepted values: the three documented forms, including an empty static value (a
    // pair of single quotes with nothing between them) and header names built from the
    // HTTP field-name token of RFC 9110 §5.1.
    let accepted: Vec<(&str, String)> = vec![
        ("a static value", "'*'".to_string()),
        ("a static media type", "'application/json'".to_string()),
        ("an empty static value", "''".to_string()),
        (
            "an integration response header",
            integration_header("X-Amzn-RequestId"),
        ),
        (
            "an integration response header with token characters",
            integration_header("x_api.version~1"),
        ),
        (
            "an integration response body expression",
            integration_body("pets.0.id"),
        ),
    ];

    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-parameters").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;

    for (case, key, value) in &refused {
        let answer = put_integration_response(
            &gateway,
            &api,
            &resource,
            "GET",
            "200",
            json!({ "responseParameters": { key.clone(): value.clone() } }),
        )
        .await;
        assert_failure(&answer, 400, "BadRequestException", case);
        // A refused request stores nothing, so the integration still has no 200 response.
        assert_failure(
            &get_integration_response(&gateway, &api, &resource, "GET", "200").await,
            404,
            "NotFoundException",
            &format!("{case} / GetIntegrationResponse after the refusal"),
        );
    }

    for (index, (case, value)) in accepted.iter().enumerate() {
        // Each accepted value goes on its own status code, because a status code names at
        // most one response of an integration.
        let status = format!("2{index:02}");
        let key = response_header("X-Custom");
        let created = put_integration_response(
            &gateway,
            &api,
            &resource,
            "GET",
            &status,
            json!({ "responseParameters": { key.clone(): value.clone() } }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutIntegrationResponse, got {} with {:?}",
            created.status, created.body
        );
        let read = get_integration_response(&gateway, &api, &resource, "GET", &status).await;
        assert_eq!(
            read.body["responseParameters"],
            json!({ key.clone(): value.clone() }),
            "case {case}: expected the stored parameter, got {:?}",
            read.body["responseParameters"]
        );
    }
}

#[tokio::test]
async fn a_status_code_that_already_names_a_response_is_replaced_by_the_last_write() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-replacement").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;

    let first = put_integration_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseTemplates": { "application/json": "$input.json('$')" } }),
    )
    .await;
    assert_eq!(
        first.status, 201,
        "case the first put: expected 201, got {} with {:?}",
        first.status, first.body
    );

    // Rows differ in what the second request carries: a put for a status code that already
    // names a response REPLACES it, whole, whatever its body says — so what the integration
    // then carries is the last write and nothing of the one before it.
    let seconds: Vec<(&str, Value)> = vec![
        (
            "the same body again",
            json!({ "responseTemplates": { "application/json": "$input.json('$')" } }),
        ),
        (
            "a different body",
            json!({ "selectionPattern": ".*", "contentHandling": "CONVERT_TO_TEXT" }),
        ),
        ("an empty body", json!({})),
        (
            "a body setting what the one before it did not",
            json!({ "responseParameters": { "method.response.header.x-made": "'yes'" } }),
        ),
    ];
    for (case, request) in &seconds {
        let answer =
            put_integration_response(&gateway, &api, &resource, "GET", "200", request.clone())
                .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get_integration_response(&gateway, &api, &resource, "GET", "200").await;
        assert_eq!(
            read.body, answer.body,
            "case {case}: the stored response must be the one this write described, got {:?} \
             after {:?}",
            read.body, answer.body
        );
        // Nothing of the write before it survives: a member this request did not set is
        // absent, as it is on a first write.
        for dropped in ["selectionPattern", "responseTemplates", "responseParameters"] {
            if request.get(dropped).is_none() {
                assert_eq!(
                    read.body.get(dropped),
                    None,
                    "case {case}: {dropped} must not survive a write that did not set it, got \
                     {:?}",
                    read.body
                );
            }
        }
    }

    // A DIFFERENT status code on the same integration is its own record: the key is the
    // status code, not the integration.
    let other =
        put_integration_response(&gateway, &api, &resource, "GET", "404", json!({})).await;
    assert_eq!(
        other.status, 201,
        "case another status code: expected 201, got {} with {:?}",
        other.status, other.body
    );

    // Reading an integration response does not change what a later caller sees.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_integration_response(&gateway, &api, &resource, "GET", "200").await;
        assert_eq!(
            read.status, 200,
            "case GetIntegrationResponse(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetIntegrationResponse repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}

#[tokio::test]
async fn an_unknown_identifier_is_a_not_found_on_every_integration_response_operation() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-notfound").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;
    // A method standing WITHOUT an integration: an integration response has nothing to
    // hang off there.
    let bare = resource_with_a_method(&gateway, &api, &root, "bare", "GET", json!({})).await;
    let missing = "zzzzzzzzzz";

    // Rows span each identifier these operations resolve, one unknown at a time — the
    // API, the resource, the method's verb, the method that carries no integration, and
    // the integration that carries no response at that status code.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        (
            "PUT",
            format!(
                "/restapis/{missing}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            Some(json!({})),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration/responses/200"),
            Some(json!({})),
        ),
        (
            "PUT",
            format!(
                "/restapis/{api}/resources/{resource}/methods/DELETE/integration/responses/200"
            ),
            Some(json!({})),
        ),
        (
            "PUT",
            format!("/restapis/{api}/resources/{bare}/methods/GET/integration/responses/200"),
            Some(json!({})),
        ),
        (
            "GET",
            format!(
                "/restapis/{missing}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{missing}/methods/GET/integration/responses/200"),
            None,
        ),
        (
            "GET",
            format!(
                "/restapis/{api}/resources/{resource}/methods/DELETE/integration/responses/200"
            ),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{bare}/methods/GET/integration/responses/200"),
            None,
        ),
        (
            "GET",
            format!("/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"),
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

    // The responses of an integration on a deleted API go with it.
    let created =
        put_integration_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    assert_eq!(
        created.status, 201,
        "case PutIntegrationResponse(before DeleteRestApi): expected 201, got {} with {:?}",
        created.status, created.body
    );
    let deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_integration_response(&gateway, &api, &resource, "GET", "200").await,
        404,
        "NotFoundException",
        "GetIntegrationResponse after DeleteRestApi",
    );
}

#[tokio::test]
async fn putting_the_integration_again_clears_the_responses_that_hung_off_the_old_one() {
    let gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "integration-response-replace").await;
    let resource = resource_with_an_integration(&gateway, &api, &root, "pets", "GET").await;

    let created =
        put_integration_response(&gateway, &api, &resource, "GET", "200", json!({})).await;
    assert_eq!(
        created.status, 201,
        "case the first put: expected 201, got {} with {:?}",
        created.status, created.body
    );

    // An integration response is a child of the integration, so replacing the integration
    // replaces its responses too: the new integration carries none.
    let replaced = put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "HTTP", "httpMethod": "GET", "uri": "https://example.com/pets" }),
    )
    .await;
    assert_eq!(
        replaced.status, 201,
        "case PutIntegration(replacing): expected 201, got {} with {:?}",
        replaced.status, replaced.body
    );
    assert_failure(
        &get_integration_response(&gateway, &api, &resource, "GET", "200").await,
        404,
        "NotFoundException",
        "GetIntegrationResponse after the integration was replaced",
    );

    // And the same status code is free again on the new integration.
    let again = put_integration_response(
        &gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "selectionPattern": "2\\d{2}" }),
    )
    .await;
    assert_eq!(
        again.status, 201,
        "case the put on the replacing integration: expected 201, got {} with {:?}",
        again.status, again.body
    );
    let read = get_integration_response(&gateway, &api, &resource, "GET", "200").await;
    assert_eq!(
        read.body, again.body,
        "case the put on the replacing integration: expected the stored response, got {:?} after {:?}",
        read.body, again.body
    );
}

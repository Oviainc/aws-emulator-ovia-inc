// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an API's gateway responses: `PutGatewayResponse` and
//! `GetGatewayResponse`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle`.
//!
//! The tests assert RULES over sets of inputs, not one recorded call: the
//! customize/read round trip over a table spanning the modelled `GatewayResponseType`
//! values, the status codes the modelled `StatusCode` pattern admits and the presence
//! and size of the two caller-supplied maps; the response API Gateway GENERATES for
//! every one of the 21 modelled response types; the status code a put that sets none
//! keeps; that a second put of a response type replaces the customization; and the
//! refusals each identifier and each modelled constraint produces.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::types::GATEWAY_RESPONSE_TYPE_VALUES;

use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};

/// `PutGatewayResponse` for `response_type` on `api`.
///
/// Shared with the sibling `gateway_response_collection` suite, which customizes types
/// before listing them, and with `control_plane_deletes`, which clears the
/// customizations it puts — so every suite writes one the same way.
pub(crate) async fn put_gateway_response(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
    request: Value,
) -> Answer {
    gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            Some(request),
        )
        .await
}

/// `GetGatewayResponse` for `response_type` on `api`. Shared with the sibling
/// `gateway_response_collection` suite, which compares a single read with the listing,
/// and with `control_plane_deletes`; see [`put_gateway_response`].
pub(crate) async fn get_gateway_response(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
) -> Answer {
    gateway
        .call(
            "GET",
            &format!("/restapis/{api}/gatewayresponses/{response_type}"),
            None,
        )
        .await
}

/// The status code the gateway response API Gateway GENERATES carries, per response
/// type, and `None` for the two types that carry none.
///
/// Source: the API Gateway Developer Guide's gateway response table ("Setting up gateway
/// responses to customize error responses"), which lists each response type beside the
/// status code of the response API Gateway returns for it; the Smithy model carries the
/// type's values only (`smithy_query(query="shape",
/// shape="com.amazonaws.apigateway#GatewayResponseType")`). Written out here rather than
/// read from the service, so the test checks the catalog instead of restating it.
///
/// Deliberately independent of the service's own catalog
/// (`DEFAULT_GATEWAY_STATUS_CODES` in `src/state.rs`, read through
/// `crate::state::default_gateway_status_code`): the two are written from the same
/// published table, so a change to one that the Developer Guide does not back fails here.
pub(crate) const GENERATED_STATUS_CODES: &[(&str, Option<&str>)] = &[
    ("ACCESS_DENIED", Some("403")),
    ("API_CONFIGURATION_ERROR", Some("500")),
    ("AUTHORIZER_CONFIGURATION_ERROR", Some("500")),
    ("AUTHORIZER_FAILURE", Some("500")),
    ("BAD_REQUEST_BODY", Some("400")),
    ("BAD_REQUEST_PARAMETERS", Some("400")),
    ("DEFAULT_4XX", None),
    ("DEFAULT_5XX", None),
    ("EXPIRED_TOKEN", Some("403")),
    ("INTEGRATION_FAILURE", Some("504")),
    ("INTEGRATION_TIMEOUT", Some("504")),
    ("INVALID_API_KEY", Some("403")),
    ("INVALID_SIGNATURE", Some("403")),
    ("MISSING_AUTHENTICATION_TOKEN", Some("403")),
    ("QUOTA_EXCEEDED", Some("429")),
    ("REQUEST_TOO_LARGE", Some("413")),
    ("RESOURCE_NOT_FOUND", Some("404")),
    ("THROTTLED", Some("429")),
    ("UNAUTHORIZED", Some("401")),
    ("UNSUPPORTED_MEDIA_TYPE", Some("415")),
    ("WAF_FILTERED", Some("403")),
];

/// The `responseTemplates` map a gateway response is reported with while a developer has
/// written no template of its own for it: API Gateway's own body template, which renders
/// the refusal's message as a JSON object, under `application/json`.
///
/// Source: the API Gateway REST API reference's `GatewayResponse` representation, where
/// this is the entry every such response carries. Written out here rather than read from
/// the service, so the test checks the template instead of restating it.
pub(crate) fn default_response_templates() -> Value {
    json!({ "application/json": "{\"message\":$context.error.messageString}" })
}

/// The generated status code of `response_type`, from [`GENERATED_STATUS_CODES`].
pub(crate) fn generated_status_code(response_type: &str) -> Option<&'static str> {
    GENERATED_STATUS_CODES
        .iter()
        .find(|(kind, _)| *kind == response_type)
        .unwrap_or_else(|| panic!("case {response_type}: the table must name every response type"))
        .1
}

#[tokio::test]
async fn a_gateway_response_put_on_an_api_is_persisted_and_read_back_member_for_member() {
    // Rows differ along what the rule generalises over: the response type (values from
    // across the modelled enum, including a type that has no generated status code of
    // its own), the status code (several values of the modelled `^[1-5]\d\d$` pattern),
    // which of the two caller-supplied maps the request carries, and how many entries
    // each carries.
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a type with neither map",
            "INTEGRATION_TIMEOUT",
            json!({ "statusCode": "504" }),
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "504",
                "responseParameters": {},
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
        (
            "maps present but empty",
            "DEFAULT_5XX",
            json!({ "statusCode": "500", "responseParameters": {}, "responseTemplates": {} }),
            json!({
                "responseType": "DEFAULT_5XX",
                "statusCode": "500",
                "responseParameters": {},
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
        (
            "one response parameter",
            "DEFAULT_4XX",
            json!({
                "statusCode": "400",
                "responseParameters": {
                    "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                }
            }),
            json!({
                "responseType": "DEFAULT_4XX",
                "statusCode": "400",
                "responseParameters": {
                    "gatewayresponse.header.Access-Control-Allow-Origin": "'*'"
                },
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
        (
            "one response template",
            "UNAUTHORIZED",
            json!({
                "statusCode": "401",
                "responseTemplates": {
                    "application/json": "{\"message\":$context.error.messageString}"
                }
            }),
            json!({
                "responseType": "UNAUTHORIZED",
                "statusCode": "401",
                "responseParameters": {},
                "responseTemplates": {
                    "application/json": "{\"message\":$context.error.messageString}"
                },
                "defaultResponse": false
            }),
        ),
        (
            "both maps, several entries each",
            "THROTTLED",
            json!({
                "statusCode": "429",
                "responseParameters": {
                    "gatewayresponse.header.Retry-After": "'60'",
                    "gatewayresponse.header.X-Reason": "'throttled'"
                },
                "responseTemplates": {
                    "application/json": "{\"message\":\"slow down\"}",
                    "text/plain": "slow down"
                }
            }),
            json!({
                "responseType": "THROTTLED",
                "statusCode": "429",
                "responseParameters": {
                    "gatewayresponse.header.Retry-After": "'60'",
                    "gatewayresponse.header.X-Reason": "'throttled'"
                },
                "responseTemplates": {
                    "application/json": "{\"message\":\"slow down\"}",
                    "text/plain": "slow down"
                },
                "defaultResponse": false
            }),
        ),
        (
            "a status code the generated response does not carry",
            "RESOURCE_NOT_FOUND",
            json!({ "statusCode": "404", "responseTemplates": { "application/json": "{}" } }),
            json!({
                "responseType": "RESOURCE_NOT_FOUND",
                "statusCode": "404",
                "responseParameters": {},
                "responseTemplates": { "application/json": "{}" },
                "defaultResponse": false
            }),
        ),
        (
            "a 1xx status code the pattern admits",
            "WAF_FILTERED",
            json!({ "statusCode": "100" }),
            json!({
                "responseType": "WAF_FILTERED",
                "statusCode": "100",
                "responseParameters": {},
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
    ];

    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-round-trip").await;

    for (case, response_type, request, expected) in &cases {
        let created = put_gateway_response(&gateway, &api, response_type, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            created.status, created.body
        );
        // The body is the stored record: the response type this call customized, the
        // status code and the entries the request carried, and the customization flag.
        assert_eq!(
            &created.body, expected,
            "case {case}: expected the gateway response {expected:?}, got {:?}",
            created.body
        );

        // Read it back: a customization is what the API reports for that type from then
        // on, member for member.
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetGatewayResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, created.body,
            "case {case}: GetGatewayResponse must report the stored response, got {:?} after {:?}",
            read.body, created.body
        );
    }

    // Each row customized its own response type, and they all stand together: a put of
    // one type is not a put of another.
    for (case, response_type, _request, expected) in &cases {
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            &read.body, expected,
            "case {case}: every customization must stand, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_uncustomized_response_type_reports_the_response_api_gateway_generates() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-defaults").await;

    // The catalog covers EVERY modelled response type — the rows are the modelled enum's
    // own values, so a value the table does not name fails here rather than silently
    // going untested.
    assert_eq!(
        GATEWAY_RESPONSE_TYPE_VALUES.len(),
        GENERATED_STATUS_CODES.len(),
        "case the catalog: expected one row per modelled response type, got {} rows for {} values",
        GENERATED_STATUS_CODES.len(),
        GATEWAY_RESPONSE_TYPE_VALUES.len()
    );

    for response_type in GATEWAY_RESPONSE_TYPE_VALUES {
        let expected_status = generated_status_code(response_type);
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            read.status, 200,
            "case {response_type}: expected 200 from GetGatewayResponse, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["responseType"].as_str(),
            Some(*response_type),
            "case {response_type}: expected the response type back, got {:?}",
            read.body
        );
        // Generated, not customized: the flag says so.
        assert_eq!(
            read.body["defaultResponse"].as_bool(),
            Some(true),
            "case {response_type}: an uncustomized type is the default response, got {:?}",
            read.body
        );
        // Both string-to-string maps are reported all the same: no parameter is
        // configured, and the body template is API Gateway's own.
        assert_eq!(
            read.body["responseParameters"],
            json!({}),
            "case {response_type}: expected no configured response parameter, got {:?}",
            read.body["responseParameters"]
        );
        assert_eq!(
            read.body["responseTemplates"],
            default_response_templates(),
            "case {response_type}: expected API Gateway's own body template, got {:?}",
            read.body["responseTemplates"]
        );
        assert_eq!(
            read.body["statusCode"].as_str(),
            expected_status,
            "case {response_type}: expected the generated status code {expected_status:?}, got {:?}",
            read.body["statusCode"]
        );
    }

    // Reading a gateway response does not change what a later caller sees: the type is
    // still uncustomized after three reads.
    let mut seen: Vec<Value> = Vec::new();
    for round in 0..3 {
        let read = get_gateway_response(&gateway, &api, "INTEGRATION_TIMEOUT").await;
        assert_eq!(
            read.status, 200,
            "case GetGatewayResponse(round {round}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        seen.push(read.body);
    }
    assert_eq!(
        seen[0], seen[2],
        "case GetGatewayResponse repeated: a read must not change the record, got {:?} then {:?}",
        seen[0], seen[2]
    );
}

#[tokio::test]
async fn a_put_that_sets_no_status_code_keeps_the_one_the_generated_response_carries() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-status-fallback").await;

    // Rows span the two classes the rule distinguishes: a response type whose generated
    // response carries a status code (several, from different status classes), and the
    // two types whose generated response carries none at all.
    let types = [
        "INTEGRATION_TIMEOUT",
        "UNAUTHORIZED",
        "REQUEST_TOO_LARGE",
        "API_CONFIGURATION_ERROR",
        "BAD_REQUEST_BODY",
        "DEFAULT_4XX",
        "DEFAULT_5XX",
    ];

    for response_type in types {
        let expected_status = generated_status_code(response_type);
        let created = put_gateway_response(
            &gateway,
            &api,
            response_type,
            json!({ "responseTemplates": { "application/json": "{}" } }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {response_type}: expected 201 from PutGatewayResponse, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.body["statusCode"].as_str(),
            expected_status,
            "case {response_type}: expected the generated status code {expected_status:?} to be \
             kept, got {:?}",
            created.body["statusCode"]
        );
        // A customization is reported as one whatever the request left out.
        assert_eq!(
            created.body["defaultResponse"].as_bool(),
            Some(false),
            "case {response_type}: a put stores a customization, got {:?}",
            created.body
        );
        let read = get_gateway_response(&gateway, &api, response_type).await;
        assert_eq!(
            read.body, created.body,
            "case {response_type}: GetGatewayResponse must report the stored response, got {:?} \
             after {:?}",
            read.body, created.body
        );
    }
}

#[tokio::test]
async fn a_second_put_of_a_response_type_replaces_the_customization() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-replace").await;

    // Rows are successive puts of ONE response type, differing in what each carries: the
    // same body again, a different status code, other map entries, and an empty body.
    // Each one is the record the API reports afterwards.
    let puts: Vec<(&str, Value, Value)> = vec![
        (
            "the first put",
            json!({
                "statusCode": "504",
                "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" }
            }),
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "504",
                "responseParameters": {},
                "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" },
                "defaultResponse": false
            }),
        ),
        (
            "the same body again",
            json!({
                "statusCode": "504",
                "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" }
            }),
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "504",
                "responseParameters": {},
                "responseTemplates": { "application/json": "{\"message\":\"timeout\"}" },
                "defaultResponse": false
            }),
        ),
        (
            "another status code and another parameter",
            json!({
                "statusCode": "503",
                "responseParameters": { "gatewayresponse.header.Retry-After": "'30'" }
            }),
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "503",
                "responseParameters": { "gatewayresponse.header.Retry-After": "'30'" },
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
        (
            "an empty body, which keeps the generated status code",
            json!({}),
            json!({
                "responseType": "INTEGRATION_TIMEOUT",
                "statusCode": "504",
                "responseParameters": {},
                "responseTemplates": default_response_templates(),
                "defaultResponse": false
            }),
        ),
    ];

    for (case, request, expected) in &puts {
        let answer = put_gateway_response(
            &gateway,
            &api,
            "INTEGRATION_TIMEOUT",
            request.clone(),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from PutGatewayResponse, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            &answer.body, expected,
            "case {case}: expected the replacing record {expected:?}, got {:?}",
            answer.body
        );
        let read = get_gateway_response(&gateway, &api, "INTEGRATION_TIMEOUT").await;
        assert_eq!(
            &read.body, expected,
            "case {case}: GetGatewayResponse must report the replacing record, got {:?}",
            read.body
        );
    }

    // Another response type of the same API is untouched by all of it: the key is the
    // response type, not the API.
    let other = get_gateway_response(&gateway, &api, "INTEGRATION_FAILURE").await;
    assert_eq!(
        other.body["defaultResponse"].as_bool(),
        Some(true),
        "case another response type: expected the generated response, got {:?}",
        other.body
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-constraints").await;

    // A response type outside the modelled `GatewayResponseType` enum, in the ways a
    // label can miss it: a value shaped like the enum's but not in it, the right value in
    // the wrong case, an unknown word, and a declared value with a trailing character.
    // Each is refused by the CONSTRAINT layer in front of the handler, whose error class
    // is `ValidationException` and not the operation's own `BadRequestException`.
    let refused_types = [
        "DEFAULT_3XX",
        "integration_timeout",
        "NOT_A_RESPONSE_TYPE",
        "DEFAULT_4XX_",
    ];
    for response_type in refused_types {
        assert_failure(
            &put_gateway_response(&gateway, &api, response_type, json!({})).await,
            400,
            "ValidationException",
            &format!("PutGatewayResponse({response_type})"),
        );
        assert_failure(
            &get_gateway_response(&gateway, &api, response_type).await,
            400,
            "ValidationException",
            &format!("GetGatewayResponse({response_type})"),
        );
    }

    // A status code outside the modelled `^[1-5]\d\d$` pattern: one past each published
    // bound and the lengths it does not admit.
    let refused_status_codes = ["099", "600", "000", "999", "20", "2000", "abc"];
    for status in refused_status_codes {
        assert_failure(
            &put_gateway_response(
                &gateway,
                &api,
                "INTEGRATION_TIMEOUT",
                json!({ "statusCode": status }),
            )
            .await,
            400,
            "ValidationException",
            &format!("PutGatewayResponse(statusCode {status})"),
        );
    }

    // Every refusal above stored nothing: the type still reports the generated response.
    let read = get_gateway_response(&gateway, &api, "INTEGRATION_TIMEOUT").await;
    assert_eq!(
        read.body["defaultResponse"].as_bool(),
        Some(true),
        "case after the refusals: expected the generated response, got {:?}",
        read.body
    );
}

#[tokio::test]
async fn an_unknown_api_is_a_not_found_on_every_gateway_response_operation() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-notfound").await;
    let missing = "zzzzzzzzzz";

    // Rows span both operations over the one identifier they resolve.
    let cases: Vec<(&str, String, Option<Value>)> = vec![
        (
            "PUT",
            format!("/restapis/{missing}/gatewayresponses/INTEGRATION_TIMEOUT"),
            Some(json!({ "statusCode": "504" })),
        ),
        (
            "GET",
            format!("/restapis/{missing}/gatewayresponses/INTEGRATION_TIMEOUT"),
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

    // The gateway responses of a deleted API go with it.
    let created = put_gateway_response(
        &gateway,
        &api,
        "INTEGRATION_TIMEOUT",
        json!({ "statusCode": "504" }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case PutGatewayResponse(before DeleteRestApi): expected 201, got {} with {:?}",
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
        &get_gateway_response(&gateway, &api, "INTEGRATION_TIMEOUT").await,
        404,
        "NotFoundException",
        "GetGatewayResponse after DeleteRestApi",
    );
}

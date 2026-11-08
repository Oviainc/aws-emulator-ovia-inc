// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `responseParameters` keys a gateway response customization
//! may carry, on both writers: `PutGatewayResponse` and `UpdateGatewayResponse`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`], `create_api` and `assert_failure` are
//! this crate's one wire-test harness, and the `PutGatewayResponse`/`GetGatewayResponse`
//! callers and the patch builders are reused from the `gateway_response` and
//! `patch_updates` suites.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: a
//! `responseParameters` key is a gateway-response parameter mapping expression —
//! `gatewayresponse.header.{name}` or `gatewayresponse.headers.{name}` with a valid HTTP
//! field name — and a key that is not one is refused with the modelled 400
//! `BadRequestException` by every operation that writes the member, leaving the API's
//! gateway responses as they were. The rows differ along what the rule generalises over:
//! the mapping target (single and multi-valued header), the header name's character set
//! (including each punctuation character the HTTP field-name token allows), the response
//! TYPE the customization is written for, where the bad key sits in a map of several
//! entries, and which writer carries it.

use serde_json::{json, Value};

use crate::gateway_response::{get_gateway_response, put_gateway_response};
use crate::patch_updates::{op, patch};
use crate::rest_api_lifecycle::{assert_failure, create_api, Gateway};

/// The modelled error code a key outside the documented form carries.
const BAD_REQUEST: &str = "BadRequestException";

/// The two mapping targets a gateway response parameter may name.
///
/// Source: the API Gateway Developer Guide's gateway-response parameter mapping, which
/// lists `gatewayresponse.header.<name>` for the header a customization sets and
/// `gatewayresponse.headers.<name>` for its multi-valued form; the Smithy model carries
/// the member as a bare string-to-string map (`smithy_query(query="doc",
/// shape="com.amazonaws.apigateway#PutGatewayResponseRequest", member="responseParameters")`).
/// Written out here rather than read from the service, so the test checks the form instead
/// of restating it.
const MAPPING_PREFIXES: [&str; 2] = ["gatewayresponse.header.", "gatewayresponse.headers."];

/// `UpdateGatewayResponse` for `response_type` on `api`.
async fn patch_gateway_response(
    gateway: &Gateway,
    api: &str,
    response_type: &str,
    operations: Vec<Value>,
) -> crate::rest_api_lifecycle::Answer {
    patch(
        gateway,
        &format!("/restapis/{api}/gatewayresponses/{response_type}"),
        operations,
    )
    .await
}

#[tokio::test]
async fn every_mapping_expression_the_member_publishes_is_accepted_by_both_writers() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-parameters").await;

    // Rows span the mapping targets and the header-name character set: a plain name, a
    // name of digits, and a name carrying each punctuation character the HTTP field-name
    // token (RFC 9110 §5.1 `tchar`) allows.
    let names = [
        "Retry-After",
        "X-Case-42",
        "x",
        "X!#$%&'*+-.^_`|~Name",
    ];
    let mut expected = serde_json::Map::new();
    let mut operations: Vec<Value> = Vec::new();
    for (index, prefix) in MAPPING_PREFIXES.iter().enumerate() {
        for name in names {
            let key = format!("{prefix}{name}");
            let value = format!("'{index}-{name}'");
            expected.insert(key.clone(), Value::from(value.clone()));
            operations.push(op(
                "add",
                &format!("/responseParameters/{key}"),
                Some(&value),
            ));
        }
    }

    // The create writer takes every one of them in one request.
    let answer = put_gateway_response(
        &gateway,
        &api,
        "DEFAULT_4XX",
        json!({ "responseParameters": Value::Object(expected.clone()) }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case PutGatewayResponse(every mapping expression): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["responseParameters"],
        Value::Object(expected.clone()),
        "case PutGatewayResponse(every mapping expression): expected the mappings back, got {:?}",
        answer.body["responseParameters"]
    );

    // And so does the patch writer, on a response type that starts with none of them.
    let answer = patch_gateway_response(&gateway, &api, "DEFAULT_5XX", operations).await;
    assert_eq!(
        answer.status, 200,
        "case UpdateGatewayResponse(every mapping expression): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let read = get_gateway_response(&gateway, &api, "DEFAULT_5XX").await;
    assert_eq!(
        read.body["responseParameters"],
        Value::Object(expected),
        "case GetGatewayResponse(DEFAULT_5XX): expected the patched mappings, got {:?}",
        read.body["responseParameters"]
    );
}

#[tokio::test]
async fn a_key_that_is_not_a_mapping_expression_is_refused_by_both_writers() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-response-bad-parameters").await;

    // One customization already stored, so every refusal below can be shown to have left
    // it alone.
    let stored = json!({ "gatewayresponse.header.Retry-After": "'30'" });
    let kept = put_gateway_response(
        &gateway,
        &api,
        "THROTTLED",
        json!({ "statusCode": "429", "responseParameters": stored }),
    )
    .await;
    assert_eq!(
        kept.status, 201,
        "case PutGatewayResponse(the customization kept): expected 201, got {} with {:?}",
        kept.status, kept.body
    );

    // Rows differ along HOW the key is not a mapping expression: a dotted name that is no
    // mapping target at all, another service's mapping expression, the prefix with no
    // header name, a header name carrying a character the HTTP field-name token excludes,
    // a near-miss spelling of the prefix, and a bare header name.
    let bad_keys = [
        ("a dotted name that is no mapping target", "invalid.param.name"),
        ("a method request mapping expression", "method.request.header.Origin"),
        ("a method response mapping expression", "method.response.header.Origin"),
        ("the single-header prefix with no name", "gatewayresponse.header."),
        ("the multi-header prefix with no name", "gatewayresponse.headers."),
        ("a header name with a colon", "gatewayresponse.header.X:Case"),
        ("a header name with a space", "gatewayresponse.header.X Case"),
        ("a header name with a slash", "gatewayresponse.header.X/Case"),
        ("a near-miss prefix", "gatewayresponse.headers2.X-Case"),
        ("a prefix in the wrong case", "GatewayResponse.Header.X-Case"),
        ("a bare header name", "Retry-After"),
        ("an empty key", ""),
    ];

    for (case, key) in bad_keys {
        // Every response TYPE the key is written for refuses it the same way, so the rule
        // is the member's and not one type's.
        for response_type in ["DEFAULT_4XX", "DEFAULT_5XX", "UNAUTHORIZED"] {
            let case = format!("PutGatewayResponse({response_type}) with {case}");
            let answer = put_gateway_response(
                &gateway,
                &api,
                response_type,
                json!({ "responseParameters": { key: "'foo'" } }),
            )
            .await;
            assert_failure(&answer, 400, BAD_REQUEST, &case);
            // The refusal names the key the caller has to fix.
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(key),
                "case {case}: expected the message to name the key {key:?}, got {message:?}"
            );
            // Nothing was stored for the type the request addressed: it still reports no
            // configured response parameter.
            let read = get_gateway_response(&gateway, &api, response_type).await;
            assert_eq!(
                read.body["responseParameters"],
                json!({}),
                "case {case}: expected no stored mappings, got {:?}",
                read.body["responseParameters"]
            );
        }

        // The same key in a map that ALSO carries valid entries is refused, and none of
        // the entries is stored: the request is refused in whole.
        let case_both = format!("PutGatewayResponse with {case} beside valid entries");
        let answer = put_gateway_response(
            &gateway,
            &api,
            "DEFAULT_4XX",
            json!({
                "responseParameters": {
                    "gatewayresponse.header.X-First": "'first'",
                    key: "'foo'",
                    "gatewayresponse.header.X-Last": "'last'",
                },
            }),
        )
        .await;
        assert_failure(&answer, 400, BAD_REQUEST, &case_both);
        let read = get_gateway_response(&gateway, &api, "DEFAULT_4XX").await;
        assert_eq!(
            read.body["responseParameters"],
            json!({}),
            "case {case_both}: expected no stored mappings, got {:?}",
            read.body["responseParameters"]
        );

        // The patch writer refuses the same key, for both ops that SET an entry — and the
        // refusal is the KEY's, not the patch path's: the message names the key. A key
        // that cannot even be written as one JSON Pointer segment (empty, or carrying a
        // `/`) is refused by the path reader before the key rule is reached, so those two
        // rows are asserted on the Put writer above only.
        if key.is_empty() || key.contains('/') {
            continue;
        }
        for kind in ["add", "replace"] {
            let case_patch = format!("UpdateGatewayResponse({kind}) with {case}");
            let answer = patch_gateway_response(
                &gateway,
                &api,
                "THROTTLED",
                vec![op(
                    kind,
                    &format!("/responseParameters/{key}"),
                    Some("'foo'"),
                )],
            )
            .await;
            assert_failure(&answer, 400, BAD_REQUEST, &case_patch);
            let message = answer.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(key),
                "case {case_patch}: expected the message to name the key {key:?}, got \
                 {message:?}"
            );
        }
    }

    // Every refusal left the stored customization exactly as it was.
    let read = get_gateway_response(&gateway, &api, "THROTTLED").await;
    assert_eq!(
        read.body, kept.body,
        "case GetGatewayResponse(THROTTLED) after the refusals: expected {:?}, got {:?}",
        kept.body, read.body
    );
}

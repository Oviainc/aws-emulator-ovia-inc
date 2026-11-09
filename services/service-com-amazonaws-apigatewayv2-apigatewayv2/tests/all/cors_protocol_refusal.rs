// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how `CreateApi` and `UpdateApi` refuse a `CorsConfiguration`
//! on an api whose protocol does not support one.
//!
//! Every test drives the real `restJson1` request path — `MultiServiceRouter::dispatch`
//! over a signed `http::Request` — so what is asserted is the wire behaviour a client
//! sees.
//!
//! The rule these cases assert, one level above the recorded case (a WEBSOCKET
//! `CreateApi` carrying `AllowOrigins=*`):
//!
//! > `CorsConfiguration` is "Supported only for HTTP APIs"
//! > (`smithy_query(query="doc", shape="CreateApiRequest", member="CorsConfiguration")`),
//! > so a request that configures one on an api of any other `ProtocolType` is refused
//! > with HTTP 400 / `BadRequestException` and the message
//! > `Cors is not supported for <ProtocolType> protocolType ` — API Gateway's own sentence
//! > for THIS member, naming the protocol type that does not support it — and nothing is
//! > written. An HTTP api takes the same document and reports it back.
//!
//! The sentence is a TEMPLATE whose fixed text is part of the contract down to its
//! whitespace: `Cors is not supported for `, the `ProtocolType` the request asked for,
//! then ` protocolType ` INCLUDING the trailing space API Gateway emits. A client that
//! classifies this refusal by comparing the message (the only way to tell it from the
//! generic protocol-restricted-member refusal, which is the same 400 /
//! `BadRequestException`) compares bytes, so the message is asserted untrimmed.
//!
//! The rows span what the rule generalises over: which CORS members the document carries,
//! whether the configuration arrives on the create or on a later patch, and the api's
//! `ProtocolType`.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::validate_cors_protocol;

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};

/// The sentence API Gateway answers a WEBSOCKET api's CORS configuration with, byte for
/// byte — the one the recorded refusal carries, trailing space included. Written out as a
/// literal, so what the service's own gate formats is pinned against the recorded text
/// rather than against a second copy of the gate's format string.
const WEBSOCKET_REFUSAL: &str = "Cors is not supported for WEBSOCKET protocolType ";

/// The recorded sentence with its ONE variable part — the protocol type — substituted, so
/// the expectation for a protocol the recording does not carry is DERIVED from the recorded
/// text instead of rebuilt from the implementation's template.
fn refusal(protocol_type: &str) -> String {
    WEBSOCKET_REFUSAL.replace(WEBSOCKET, protocol_type)
}

/// The service's gate answers the recorded sentence for the protocol type the recording
/// used, and the same fixed text — trailing space included — for any other.
///
/// This drives the gate both the create and the patch call
/// ([`validate_cors_protocol`]), so neither leg can word the refusal differently.
#[test]
fn the_refusal_template_carries_the_fixed_text_down_to_its_trailing_space() {
    // `PROTOCOL_TYPE_VALUES` is `WEBSOCKET | HTTP` (`smithy_query(query="shape",
    // shape="ProtocolType")`), so the first row is the modelled non-HTTP protocol — the
    // recorded sentence, byte for byte — and the second is one a future api version could
    // add, which the sentence must word in its own name.
    for protocol_type in [WEBSOCKET, "A-FUTURE-PROTOCOL"] {
        let sentence = validate_cors_protocol(protocol_type, true)
            .expect_err("a non-HTTP protocol must refuse a CORS configuration");
        assert_eq!(
            sentence,
            refusal(protocol_type),
            "case {protocol_type:?}: expected {:?}, got {sentence:?}",
            refusal(protocol_type)
        );
        assert!(
            sentence.ends_with(" protocolType "),
            "case {protocol_type:?}: expected the fixed text to keep its trailing space, \
             got {sentence:?}"
        );
    }
    // The protocol the member is supported on, and a request that configures nothing, ask
    // for nothing the gate refuses.
    for (case, protocol_type, configured) in [
        ("an HTTP api carrying a document", HTTP, true),
        ("a WEBSOCKET api carrying none", WEBSOCKET, false),
    ] {
        assert_eq!(
            validate_cors_protocol(protocol_type, configured),
            Ok(()),
            "case {case:?}: expected the gate to admit it, got {:?}",
            validate_cors_protocol(protocol_type, configured)
        );
    }
}

/// The one `ProtocolType` whose apis support a CORS configuration, and the one whose do
/// not (`PROTOCOL_TYPE_VALUES` is `HTTP | WEBSOCKET`).
const HTTP: &str = "HTTP";
const WEBSOCKET: &str = "WEBSOCKET";

/// The CORS documents the table spans: a minimal one, one naming every list the shape
/// carries, and the wildcard the recorded request sent.
fn documents() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the wildcard origin the recorded request sent",
            json!({ "allowOrigins": ["*"] }),
        ),
        (
            "one explicit origin",
            json!({ "allowOrigins": ["https://example.com"] }),
        ),
        (
            "origins, methods, headers and a max age",
            json!({
                "allowOrigins": ["https://example.com"],
                "allowMethods": ["GET", "POST"],
                "allowHeaders": ["authorization"],
                "exposeHeaders": ["x-request-id"],
                "maxAge": 600
            }),
        ),
        ("an empty document", json!({})),
    ]
}

/// The CreateApi request for an api of `protocol_type`, carrying `cors` when it is set.
fn create_request(name: &str, protocol_type: &str, cors: Option<&Value>) -> Value {
    let mut request = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == WEBSOCKET {
        // "This property is required for WebSocket APIs"
        // (`CreateApi$RouteSelectionExpression`).
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    if let Some(cors) = cors {
        request["corsConfiguration"] = cors.clone();
    }
    request
}

#[tokio::test]
async fn create_api_refuses_a_cors_configuration_in_the_member_s_own_sentence() {
    for (index, (case, document)) in documents().into_iter().enumerate() {
        let emulator = emulator();

        // The protocol that supports the member takes the document.
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(create_request(
                &format!("cors-http-{index}"),
                HTTP,
                Some(&document),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 on an {HTTP} api, got {status} {created}"
        );

        // The other protocol is refused, in the member's own sentence.
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(create_request(
                &format!("cors-websocket-{index}"),
                WEBSOCKET,
                Some(&document),
            )),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 on a {WEBSOCKET} api, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = as_str(&body, "message");
        assert_eq!(
            message, WEBSOCKET_REFUSAL,
            "case {case:?}: expected the recorded sentence {WEBSOCKET_REFUSAL:?} byte for \
             byte, got {message:?} in {body}"
        );

        // The refused create minted nothing: the one api is the accepted one.
        let (status, listed) = call(&emulator, "GET", "/v2/apis", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApis must answer 200, got {status} {listed}"
        );
        let items = listed
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected only the accepted create to have minted an api, \
             got {listed}"
        );
        assert_eq!(
            as_str(&items[0], "protocolType"),
            HTTP,
            "case {case:?}: expected the minted api to be the {HTTP} one, got {listed}"
        );
    }
}

#[tokio::test]
async fn update_api_refuses_a_cors_configuration_and_leaves_the_api_alone() {
    for (index, (case, document)) in documents().into_iter().enumerate() {
        let emulator = emulator();
        let (status, created) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(create_request(
                &format!("cors-patch-{index}"),
                WEBSOCKET,
                None,
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the api under patch must be created, got {status} {created}"
        );
        let api_id = as_str(&created, "apiId");
        let name_before = as_str(&created, "name");

        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}"),
            Some(json!({ "name": "renamed", "corsConfiguration": document })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 patching a {WEBSOCKET} api, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let message = as_str(&body, "message");
        assert_eq!(
            message, WEBSOCKET_REFUSAL,
            "case {case:?}: expected the recorded sentence {WEBSOCKET_REFUSAL:?} byte for \
             byte, got {message:?} in {body}"
        );

        // Nothing of the refused patch was applied: neither the CORS document nor the
        // name it also carried.
        let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApi must answer 200, got {status} {fetched}"
        );
        assert!(
            carries_nothing(&fetched, "corsConfiguration"),
            "case {case:?}: expected the api to carry no corsConfiguration, got {fetched}"
        );
        assert_eq!(
            as_str(&fetched, "name"),
            name_before,
            "case {case:?}: expected the api to keep the name it had, got {fetched}"
        );
    }
}

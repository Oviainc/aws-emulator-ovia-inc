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
//! > `Cors is not supported for <ProtocolType> protocolType` — API Gateway's own sentence
//! > for THIS member, naming the protocol type that does not support it — and nothing is
//! > written. An HTTP api takes the same document and reports it back.
//!
//! The rows span what the rule generalises over: which CORS members the document carries,
//! whether the configuration arrives on the create or on a later patch, and the api's
//! `ProtocolType`.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};

/// The sentence API Gateway refuses a CORS configuration with on an api of a protocol
/// that does not support one. The protocol type is the variable part.
fn refusal(protocol_type: &str) -> String {
    format!("Cors is not supported for {protocol_type} protocolType")
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
        assert_eq!(
            as_str(&body, "message"),
            refusal(WEBSOCKET),
            "case {case:?}: expected {:?}, got {body}",
            refusal(WEBSOCKET)
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
        assert_eq!(
            as_str(&body, "message"),
            refusal(WEBSOCKET),
            "case {case:?}: expected {:?}, got {body}",
            refusal(WEBSOCKET)
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

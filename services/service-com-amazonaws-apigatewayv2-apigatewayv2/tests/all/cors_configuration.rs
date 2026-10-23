// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteCorsConfiguration`.
//!
//! The rule these cases assert: **`DeleteCorsConfiguration` clears the CORS
//! configuration member of the api its `ApiId` names and answers 204 with an empty
//! body, leaving every other member of that api — and every other api — exactly as it
//! was; it answers 404 `NotFoundException` when no api carries that id.** Clearing a
//! member that is already clear is the state the call asks for, so the operation is
//! idempotent, and the model declares no other error for it (`NotFoundException`,
//! `TooManyRequestsException` — `smithy_query(query="operation",
//! operation="DeleteCorsConfiguration")`).
//!
//! The rows span what the rule generalises over: which `Cors` members the
//! configuration carries, how many entries its lists hold, both published bounds of
//! `Cors$MaxAge` (`range` -1..86400 on `IntegerWithLengthBetweenMinus1And86400`), the
//! api's protocol type, whether the api carries a configuration at all, and how many
//! apis the store holds. The recorded case that filed the issue — delete a CORS
//! configuration and observe it gone — is ONE row of the first table.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// One api whose CORS configuration gets deleted.
struct Row {
    case: &'static str,
    /// The CreateApi request body. Always an HTTP api: "A CORS configuration.
    /// Supported only for HTTP APIs" (`CreateApi$CorsConfiguration`).
    request: Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "every Cors member set, several entries per list",
            request: json!({
                "name": "cors-full",
                "protocolType": "HTTP",
                "description": "an api with a fully specified CORS configuration",
                "corsConfiguration": {
                    "allowOrigins": ["https://example.com", "https://other.example.com"],
                    "allowMethods": ["GET", "POST", "OPTIONS"],
                    "allowHeaders": ["content-type", "x-api-key"],
                    "exposeHeaders": ["x-trace", "x-request-id"],
                    "allowCredentials": true,
                    "maxAge": 600
                }
            }),
        },
        Row {
            case: "one member only: a single allowed origin",
            request: json!({
                "name": "cors-one-origin",
                "protocolType": "HTTP",
                "corsConfiguration": { "allowOrigins": ["https://example.com"] }
            }),
        },
        Row {
            case: "the wildcard origin and no other member",
            request: json!({
                "name": "cors-wildcard",
                "protocolType": "HTTP",
                "corsConfiguration": { "allowOrigins": ["*"] }
            }),
        },
        Row {
            case: "empty lists and credentials off",
            request: json!({
                "name": "cors-empty-lists",
                "protocolType": "HTTP",
                "corsConfiguration": {
                    "allowOrigins": [],
                    "allowMethods": [],
                    "allowHeaders": [],
                    "exposeHeaders": [],
                    "allowCredentials": false
                }
            }),
        },
        Row {
            // The low published bound of `Cors$MaxAge` (`range` min -1).
            case: "MaxAge at its published minimum, -1",
            request: json!({
                "name": "cors-max-age-min",
                "protocolType": "HTTP",
                "corsConfiguration": { "allowOrigins": ["https://example.com"], "maxAge": -1 }
            }),
        },
        Row {
            // The high published bound of `Cors$MaxAge` (`range` max 86400).
            case: "MaxAge at its published maximum, 86400",
            request: json!({
                "name": "cors-max-age-max",
                "protocolType": "HTTP",
                "corsConfiguration": { "allowOrigins": ["https://example.com"], "maxAge": 86400 }
            }),
        },
        Row {
            case: "a CORS configuration beside every other optional member the api stores",
            request: json!({
                "name": "cors-and-friends",
                "protocolType": "HTTP",
                "description": "cors plus the rest of the optional members",
                "version": "2024-01-01",
                "disableExecuteApiEndpoint": true,
                "ipAddressType": "dualstack",
                "apiKeySelectionExpression": "$request.header.x-api-key",
                "corsConfiguration": {
                    "allowOrigins": ["https://example.com"],
                    "allowMethods": ["GET"],
                    "maxAge": 300
                },
                "tags": { "team": "platform" }
            }),
        },
    ]
}

/// Create one api and return `(ApiId, the record GetApi reports)`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (String, Value) {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    let api_id = as_str(&created, "apiId");
    let (status, fetched) = call(emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetApi, got {status} {fetched}"
    );
    (api_id, fetched)
}

/// The record with its `corsConfiguration` member removed: what the api must look
/// like once the configuration is deleted.
fn without_cors(record: &Value) -> Value {
    let mut stripped = record.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("corsConfiguration");
    }
    stripped
}

#[tokio::test]
async fn delete_cors_configuration_clears_the_configuration_and_keeps_the_rest_of_the_api() {
    let emulator = emulator();

    for row in rows() {
        let (api_id, before) = create_api(&emulator, row.case, &row.request).await;
        assert!(
            before.get("corsConfiguration").is_some(),
            "case {:?}: the api must carry the CORS configuration it was created with, got {before}",
            row.case
        );

        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/cors"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteCorsConfiguration, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: a 204 carries no body, got {body}",
            row.case
        );

        let (status, after) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi after the delete, got {status} {after}",
            row.case
        );
        assert_eq!(
            after.get("corsConfiguration"),
            None,
            "case {:?}: the CORS configuration must be gone, got {after}",
            row.case
        );
        assert_eq!(
            after,
            without_cors(&before),
            "case {:?}: the delete must touch nothing but the CORS configuration",
            row.case
        );

        // Deleting what is already deleted asks for the state the api is already in,
        // so it answers exactly as the first call did and still changes nothing.
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/cors"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {:?}: a repeated DeleteCorsConfiguration must answer 204, got {status} {body}",
            row.case
        );
        let (_, again) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            again, after,
            "case {:?}: the repeated delete changed the api",
            row.case
        );
    }
}

#[tokio::test]
async fn delete_cors_configuration_on_an_api_carrying_none_changes_nothing() {
    let emulator = emulator();

    // An api that never had a CORS configuration is already in the state the call
    // asks for. A WebSocket api is such an api by construction — CORS is "Supported
    // only for HTTP APIs" — and the operation models no error for either case
    // (`NotFoundException` is about the ApiId; there is no BadRequestException).
    let cases = [
        (
            "http api created without a CorsConfiguration",
            json!({ "name": "no-cors-http", "protocolType": "HTTP" }),
        ),
        (
            "websocket api, which can carry no CorsConfiguration at all",
            json!({
                "name": "no-cors-websocket",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            }),
        ),
    ];

    for (case, request) in cases {
        let (api_id, before) = create_api(&emulator, case, &request).await;
        assert_eq!(
            before.get("corsConfiguration"),
            None,
            "case {case:?}: this api must carry no CORS configuration, got {before}"
        );

        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/cors"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteCorsConfiguration, got {status} {body}"
        );

        let (_, after) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            after, before,
            "case {case:?}: the delete must leave an api with no CORS configuration untouched"
        );
    }
}

#[tokio::test]
async fn delete_cors_configuration_refuses_an_unknown_api() {
    let emulator = emulator();

    // One real api, so the store is not empty and the refusal is about the id named.
    let (api_id, _) = create_api(
        &emulator,
        "the api that does exist",
        &json!({
            "name": "present",
            "protocolType": "HTTP",
            "corsConfiguration": { "allowOrigins": ["https://example.com"] }
        }),
    )
    .await;

    // A second api, deleted, so its id is one that USED to name an api.
    let (gone_id, _) = create_api(
        &emulator,
        "the api that gets deleted",
        &json!({ "name": "gone", "protocolType": "HTTP" }),
    )
    .await;
    let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{gone_id}"), None).await;
    assert_eq!(
        status, 204,
        "expected 204 from DeleteApi, got {status} {body}"
    );

    let unknown = [
        ("an id no api was ever minted with", "zzzzzzzzzz".to_string()),
        ("a deleted api's id", gone_id),
        ("an id of another shape entirely", "NOT-AN-API-ID".to_string()),
        ("a one-character id", "a".to_string()),
    ];

    for (case, id) in unknown {
        let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{id}/cors"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteCorsConfiguration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // And the api that does exist kept its configuration through every refusal.
    let (_, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        fetched["corsConfiguration"]["allowOrigins"],
        json!(["https://example.com"]),
        "a refused delete must not touch another api's CORS configuration, got {fetched}"
    );
}

#[tokio::test]
async fn delete_cors_configuration_touches_only_the_api_it_names() {
    let emulator = emulator();

    // Several apis, each with its own configuration, so "only the api named" is
    // asserted over a store that holds more than one candidate.
    let mut apis = Vec::new();
    for n in 0..5 {
        let case = format!("api #{n}");
        let (api_id, record) = create_api(
            &emulator,
            &case,
            &json!({
                "name": format!("cors-api-{n}"),
                "protocolType": "HTTP",
                "corsConfiguration": {
                    "allowOrigins": [format!("https://host-{n}.example.com")],
                    "maxAge": 60 * (n + 1)
                }
            }),
        )
        .await;
        apis.push((case, api_id, record));
    }

    // Delete one api's configuration at a time; every api that has not been visited
    // yet must still report exactly the configuration it was created with.
    for index in 0..apis.len() {
        let (case, api_id, _) = apis[index].clone();
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/cors"),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteCorsConfiguration, got {status} {body}"
        );

        for (other_case, other_id, created) in &apis {
            let (_, fetched) = call(&emulator, "GET", &format!("/v2/apis/{other_id}"), None).await;
            let visited = apis
                .iter()
                .take(index + 1)
                .any(|(_, seen_id, _)| seen_id == other_id);
            let expected = if visited {
                without_cors(created)
            } else {
                created.clone()
            };
            assert_eq!(
                fetched, expected,
                "case {case:?}: wrong record for {other_case:?} after the delete"
            );
        }
    }
}

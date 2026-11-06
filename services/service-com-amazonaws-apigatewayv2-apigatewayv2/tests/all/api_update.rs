// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateApi`.
//!
//! Three rules, each asserted over a table rather than over one request:
//!
//! * **`PATCH /v2/apis/{ApiId}` is a patch of the STORED api**: a member the request
//!   carries replaces the stored one, a member it omits leaves the stored value as it
//!   was, and what the call reports is what a later `GetApi` reports. An `ApiId` the
//!   store does not hold — one never minted, or one a `DeleteApi` removed — is
//!   `NotFoundException` (404, `smithy_query(query="operation", operation="UpdateApi")`).
//! * **A CORS configuration that allows credentialed requests cannot answer them with
//!   the wildcard**, so such a document is refused with `BadRequestException` (400) and
//!   nothing is written. The rule belongs to the configuration, so every writer of one
//!   applies it: `CreateApi` and `UpdateApi` are both driven here.
//! * **Quick create's members patch the resources quick create minted**: "The value
//!   provided updates the integration URI and integration type" (`UpdateApi$Target`).
//!
//! The rows span what each rule generalises over: the member's type (string, boolean,
//! structure), how many members one patch names (none, one, several), the api's
//! protocol, which CORS list carries the wildcard, whether credentials are allowed, and
//! the two documented forms of a quick-create `Target` (a function ARN, a URL). The
//! recorded cases that filed the issue — widening `AllowOrigins` to `["*"]` on a
//! credentialed configuration, and an `UpdateApi` on a deleted api — are ONE row each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

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

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Read one api back and answer `(status, body)`.
async fn get_api(emulator: &MultiServiceRouter, api_id: &str) -> (u16, Value) {
    call(emulator, "GET", &format!("/v2/apis/{api_id}"), None).await
}

/// Patch one api and answer `(status, body)`.
async fn update_api(
    emulator: &MultiServiceRouter,
    api_id: &str,
    patch: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}"),
        Some(patch.clone()),
    )
    .await
}

/// The record `created` becomes once `patch` is applied: every member the patch names
/// takes the patch's value, every other member keeps the stored one. That is the patch
/// rule itself, written once, so a row only has to supply the patch.
fn patched_record(created: &Value, patch: &Value) -> Value {
    let mut expected = created.clone();
    for (member, value) in patch.as_object().expect("the patch is a JSON object") {
        expected[member] = value.clone();
    }
    expected
}

/// One patch row: the request body sent to `UpdateApi`.
struct PatchRow {
    case: &'static str,
    /// `true` for an HTTP api, `false` for a WebSocket one — `CorsConfiguration` is
    /// "Supported only for HTTP APIs".
    http: bool,
    patch: Value,
}

fn patch_rows() -> Vec<PatchRow> {
    vec![
        PatchRow {
            case: "one string member",
            http: true,
            patch: json!({ "name": "renamed-api" }),
        },
        PatchRow {
            case: "the description",
            http: true,
            patch: json!({ "description": "a rewritten description" }),
        },
        PatchRow {
            case: "the description cleared to the empty string",
            http: true,
            patch: json!({ "description": "" }),
        },
        PatchRow {
            case: "the version",
            http: true,
            patch: json!({ "version": "2026-01-01" }),
        },
        PatchRow {
            case: "one boolean member, flipped",
            http: true,
            patch: json!({ "disableExecuteApiEndpoint": true }),
        },
        PatchRow {
            case: "the other IpAddressType the enum carries",
            http: true,
            patch: json!({ "ipAddressType": "dualstack" }),
        },
        PatchRow {
            case: "the api key selection expression",
            http: true,
            patch: json!({ "apiKeySelectionExpression": "$request.querystring.apiKey" }),
        },
        PatchRow {
            case: "a structure member: a whole CORS configuration",
            http: true,
            patch: json!({
                "corsConfiguration": {
                    "allowOrigins": ["https://ok.example.com"],
                    "allowMethods": ["GET", "POST"],
                    "allowHeaders": ["content-type"],
                    "exposeHeaders": ["x-trace"],
                    "allowCredentials": true,
                    "maxAge": 600
                }
            }),
        },
        PatchRow {
            case: "a CORS configuration widened to every origin, credentials off",
            http: true,
            patch: json!({
                "corsConfiguration": { "allowOrigins": ["*"], "allowCredentials": false }
            }),
        },
        PatchRow {
            case: "several members of different types at once",
            http: true,
            patch: json!({
                "name": "multi-patched",
                "description": "patched together",
                "version": "9",
                "disableExecuteApiEndpoint": true,
                "ipAddressType": "dualstack"
            }),
        },
        PatchRow {
            case: "a patch naming no member leaves the record as it is",
            http: true,
            patch: json!({}),
        },
        PatchRow {
            case: "a WebSocket api's route selection expression",
            http: false,
            patch: json!({ "routeSelectionExpression": "$request.body.kind" }),
        },
        PatchRow {
            case: "a WebSocket api's schema validation flag",
            http: false,
            patch: json!({ "disableSchemaValidation": true }),
        },
        PatchRow {
            case: "a WebSocket api's name and description together",
            http: false,
            patch: json!({ "name": "socket-renamed", "description": "patched" }),
        },
    ]
}

#[tokio::test]
async fn update_api_replaces_the_members_it_names_and_keeps_the_rest() {
    let emulator = emulator();

    for (index, row) in patch_rows().into_iter().enumerate() {
        // One api per row, each created with every optional member the slice stores, so
        // a patch of any one member is seen against a fully populated record.
        let mut request = if row.http {
            http_api(&format!("api-patch-http-{index}"))
        } else {
            websocket_api(&format!("api-patch-socket-{index}"))
        };
        request["description"] = json!("the api as created");
        request["version"] = json!("2024-01-01");
        request["tags"] = json!({ "team": "platform" });
        if row.http {
            request["corsConfiguration"] = json!({
                "allowOrigins": ["https://created.example.com"],
                "allowCredentials": true,
                "maxAge": 60
            });
        }
        let api_id = create_api(&emulator, row.case, &request).await;
        let (status, created) = get_api(&emulator, &api_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi on the fresh api, got {status} {created}",
            row.case
        );

        let (status, updated) = update_api(&emulator, &api_id, &row.patch).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from UpdateApi, got {status} {updated}",
            row.case
        );
        let expected = patched_record(&created, &row.patch);
        assert_eq!(
            updated, expected,
            "case {:?}: UpdateApi must report the patched record; expected {expected}, got {updated}",
            row.case
        );

        // The write is what a later caller sees, not just what this call answered.
        let (status, fetched) = get_api(&emulator, &api_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetApi after the patch, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, expected,
            "case {:?}: GetApi must report what UpdateApi stored; expected {expected}, got {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "apiId"),
            api_id,
            "case {:?}: the patch must not change the ApiId, got {fetched}",
            row.case
        );
    }
}

#[tokio::test]
async fn update_api_on_an_api_the_store_does_not_hold_is_not_found() {
    let emulator = emulator();

    // One live api, so the store is not empty and the refusal is about the id named.
    let live = create_api(&emulator, "the api that exists", &http_api("patch-live")).await;

    // And one that is deleted again, so its id USED to name an api.
    let gone = create_api(&emulator, "the api that gets deleted", &http_api("patch-gone")).await;
    let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{gone}"), None).await;
    assert_eq!(status, 204, "expected 204 from DeleteApi, got {status} {body}");

    let unknown = [
        ("a deleted api's id", gone),
        ("an id no api was ever minted with", "zzzzzzzzzz".to_string()),
        ("an id of another shape entirely", "NOT-AN-API-ID".to_string()),
        ("a one-character id", "a".to_string()),
    ];
    for (case, api_id) in unknown {
        let (status, body) = update_api(
            &emulator,
            &api_id,
            &json!({ "description": "patching the absent" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // And the api that does exist is still patchable and untouched by the refusals.
    let (status, updated) = update_api(&emulator, &live, &json!({ "description": "still here" })).await;
    assert_eq!(
        status, 201,
        "expected 201 patching the live api, got {status} {updated}"
    );
    assert_eq!(
        as_str(&updated, "description"),
        "still here",
        "the live api must carry the patched description, got {updated}"
    );
}

/// One row of the CORS table: a configuration and whether an api may carry it.
struct CorsRow {
    case: &'static str,
    cors: Value,
    accepted: bool,
}

fn cors_rows() -> Vec<CorsRow> {
    vec![
        CorsRow {
            case: "the wildcard origin with credentials allowed",
            cors: json!({ "allowOrigins": ["*"], "allowCredentials": true }),
            accepted: false,
        },
        CorsRow {
            case: "the wildcard origin among named ones, with credentials allowed",
            cors: json!({
                "allowOrigins": ["https://ok.example.com", "*"],
                "allowCredentials": true
            }),
            accepted: false,
        },
        CorsRow {
            case: "the wildcard method with credentials allowed",
            cors: json!({
                "allowOrigins": ["https://ok.example.com"],
                "allowMethods": ["*"],
                "allowCredentials": true
            }),
            accepted: false,
        },
        CorsRow {
            case: "the wildcard header with credentials allowed",
            cors: json!({
                "allowOrigins": ["https://ok.example.com"],
                "allowHeaders": ["*"],
                "allowCredentials": true
            }),
            accepted: false,
        },
        CorsRow {
            case: "the wildcard origin with credentials not allowed",
            cors: json!({ "allowOrigins": ["*"], "allowCredentials": false }),
            accepted: true,
        },
        CorsRow {
            case: "the wildcard origin with credentials left unsaid",
            cors: json!({ "allowOrigins": ["*"] }),
            accepted: true,
        },
        CorsRow {
            case: "named origins with credentials allowed",
            cors: json!({
                "allowOrigins": ["https://ok.example.com", "https://other.example.com"],
                "allowMethods": ["GET", "POST"],
                "allowHeaders": ["content-type"],
                "allowCredentials": true,
                "maxAge": 600
            }),
            accepted: true,
        },
        CorsRow {
            case: "an origin that merely contains the wildcard character",
            cors: json!({ "allowOrigins": ["https://*.example.com"], "allowCredentials": true }),
            accepted: true,
        },
        CorsRow {
            case: "no origins at all, credentials allowed",
            cors: json!({ "allowCredentials": true, "maxAge": 0 }),
            accepted: true,
        },
    ]
}

#[tokio::test]
async fn a_cors_configuration_allowing_credentials_cannot_allow_the_wildcard() {
    let emulator = emulator();

    for (index, row) in cors_rows().into_iter().enumerate() {
        // (1) CreateApi: the configuration arrives with the api.
        let mut request = http_api(&format!("cors-create-{index}"));
        request["corsConfiguration"] = row.cors.clone();
        let (status, created) = call(&emulator, "POST", "/v2/apis", Some(request)).await;
        if row.accepted {
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from CreateApi, got {status} {created}",
                row.case
            );
            assert_eq!(
                created.get("corsConfiguration"),
                Some(&row.cors),
                "case {:?}: the stored configuration must be the one sent, got {created}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 from CreateApi, got {status} {created}",
                row.case
            );
            assert_eq!(
                error_code(&created),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {created}",
                row.case
            );
            // A refused create stores no api: the name it asked for is on none.
            let (_, listed) = call(&emulator, "GET", "/v2/apis", None).await;
            let names: Vec<String> = listed["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|api| as_str(api, "name"))
                .collect();
            assert!(
                !names.contains(&format!("cors-create-{index}")),
                "case {:?}: a refused CreateApi must store no api, got {names:?}",
                row.case
            );
        }

        // (2) UpdateApi: the same configuration reached as a patch of an api that
        // already allows credentials for one named origin — the recorded "widen the
        // allowed origins" case.
        let mut request = http_api(&format!("cors-patch-{index}"));
        request["corsConfiguration"] = json!({
            "allowOrigins": ["https://ok.example.com"],
            "allowCredentials": true
        });
        let api_id = create_api(&emulator, row.case, &request).await;
        let (_, before) = get_api(&emulator, &api_id).await;

        let (status, updated) =
            update_api(&emulator, &api_id, &json!({ "corsConfiguration": row.cors })).await;
        if row.accepted {
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from UpdateApi, got {status} {updated}",
                row.case
            );
            assert_eq!(
                updated.get("corsConfiguration"),
                Some(&row.cors),
                "case {:?}: the patched configuration must be the one sent, got {updated}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 from UpdateApi, got {status} {updated}",
                row.case
            );
            assert_eq!(
                error_code(&updated),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {updated}",
                row.case
            );
            let (_, after) = get_api(&emulator, &api_id).await;
            assert_eq!(
                after, before,
                "case {:?}: a refused UpdateApi must leave the api as it was",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_cors_configuration_is_refused_on_an_api_of_the_other_protocol() {
    let emulator = emulator();

    // "A CORS configuration. Supported only for HTTP APIs" (`UpdateApi$CorsConfiguration`).
    let api_id = create_api(&emulator, "a websocket api", &websocket_api("cors-socket")).await;
    let (_, before) = get_api(&emulator, &api_id).await;

    for (case, patch) in [
        (
            "a CorsConfiguration",
            json!({ "corsConfiguration": { "allowOrigins": ["https://ok.example.com"] } }),
        ),
        ("a quick-create Target", json!({ "target": "https://example.com/upstream" })),
        ("a quick-create RouteKey", json!({ "routeKey": "GET /items" })),
        (
            "a quick-create CredentialsArn",
            json!({ "credentialsArn": "arn:aws:iam::000000000000:role/apigw" }),
        ),
    ] {
        let (status, body) = update_api(&emulator, &api_id, &patch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let (_, after) = get_api(&emulator, &api_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateApi must leave the api as it was"
        );
    }
}

#[tokio::test]
async fn a_quick_create_target_patch_moves_the_managed_integration() {
    let emulator = emulator();

    // "For HTTP integrations, specify a fully qualified URL. For Lambda integrations,
    // specify a function ARN. The type of the integration will be HTTP_PROXY or
    // AWS_PROXY, respectively" (`UpdateApi$Target`) — both forms, in both directions.
    let rows = [
        (
            "a URL replacing a function ARN",
            "arn:aws:lambda:eu-west-1:000000000000:function:handler",
            "https://example.com/upstream",
            "HTTP_PROXY",
        ),
        (
            "a function ARN replacing a URL",
            "https://example.com/upstream",
            "arn:aws:lambda:eu-west-1:000000000000:function:handler",
            "AWS_PROXY",
        ),
    ];

    for (index, (case, created_target, patched_target, expect_type)) in rows.into_iter().enumerate()
    {
        let mut request = http_api(&format!("quick-create-patch-{index}"));
        request["target"] = json!(created_target);
        let api_id = create_api(&emulator, case, &request).await;

        let (status, updated) = update_api(
            &emulator,
            &api_id,
            &json!({
                "target": patched_target,
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw",
                "routeKey": "ANY /moved"
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from UpdateApi, got {status} {updated}"
        );

        // The integration quick create minted now carries the new target, and the type
        // the new target's form implies.
        let (status, listed) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrations, got {status} {listed}"
        );
        let integrations = listed["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            integrations.len(),
            1,
            "case {case:?}: quick create mints exactly one integration, got {listed}"
        );
        assert_eq!(
            as_str(&integrations[0], "integrationUri"),
            patched_target,
            "case {case:?}: the managed integration must carry the patched Target, got {listed}"
        );
        assert_eq!(
            as_str(&integrations[0], "integrationType"),
            expect_type,
            "case {case:?}: the managed integration's type must follow the Target's form, got {listed}"
        );
        assert_eq!(
            as_str(&integrations[0], "credentialsArn"),
            "arn:aws:iam::000000000000:role/apigw",
            "case {case:?}: the patched CredentialsArn must reach the managed integration, got {listed}"
        );

        // And the quick-create route carries the patched key.
        let (status, routes) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoutes, got {status} {routes}"
        );
        let items = routes["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: quick create mints exactly one route, got {routes}"
        );
        assert_eq!(
            as_str(&items[0], "routeKey"),
            "ANY /moved",
            "case {case:?}: the managed route must carry the patched RouteKey, got {routes}"
        );
    }
}

#[tokio::test]
async fn a_quick_create_member_is_refused_on_an_api_that_has_no_quick_create_resources() {
    let emulator = emulator();

    // An api created WITHOUT quick create carries no API-Gateway-managed integration or
    // route, so there is nothing for these members to replace.
    let api_id = create_api(&emulator, "no quick create", &http_api("no-quick-create")).await;
    let (_, before) = get_api(&emulator, &api_id).await;

    for (case, patch) in [
        ("a Target", json!({ "target": "https://example.com/upstream" })),
        (
            "a CredentialsArn",
            json!({ "credentialsArn": "arn:aws:iam::000000000000:role/apigw" }),
        ),
        ("a RouteKey", json!({ "routeKey": "GET /items" })),
    ] {
        let (status, body) = update_api(&emulator, &api_id, &patch).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let (_, after) = get_api(&emulator, &api_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateApi must leave the api as it was"
        );
    }
}

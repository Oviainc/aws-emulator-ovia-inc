// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the rules `CreateRoute`/`UpdateRoute` hold a route to.
//!
//! Three rules, each asserted over a table rather than over one request:
//!
//! * **A route's references are resolved against its own api before anything is
//!   persisted.** A route references two resources of the api that owns it — the
//!   integration its `Target` addresses (`integrations/{IntegrationId}`) and the
//!   authorizer its `AuthorizerId` names — and a request naming one the api cannot
//!   resolve is "not valid, for example, the input is incomplete or incorrect":
//!   `BadRequestException` (400, `smithy_query(query="operation",
//!   operation="CreateRoute")`), with nothing created. A value that is not of the target
//!   FORM at all is refused by the same error with its own sentence; the whole class of
//!   target forms is `route_target_forms`'s table.
//! * **A path position that is a variable has one name.** Two route keys of one api and
//!   method that agree on every earlier segment cannot declare differently named path
//!   variables at the same position, because a request matching both would have to bind
//!   two different path parameters at once; such a key "would cause a conflict with the
//!   current state of a service resource associated with the request" and is refused
//!   with `ConflictException` (409), the error a duplicate key takes too.
//! * **The duplicate-route-key conflict message is built from the request**: `Route with
//!   key <RouteKey> already exists for this API`, with code `ConflictException` (409).
//!
//! The rows span what each rule generalises over: which reference is unresolvable and
//! why (never minted, deleted, minted on another api, not a reference at all), the
//! authorization type that names an authorizer, the shape of the conflicting route key
//! (a named variable, the greedy variable, a deeper position, a different method, the
//! `ANY` method), and the form of the duplicated key (a method-and-path key, a
//! WebSocket key, the catch-all). The recorded cases that filed the issue — a target of
//! `integrations/abc1234`, an `AuthorizerId` of `nonexistent123`, `GET /items/{proxy+}`
//! beside `GET /items/{id}`, and the `GET /dup` conflict message — are ONE row each.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::UNRESOLVED_AUTHORIZER_MESSAGE;
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

/// Create one HTTP_PROXY integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/upstream",
            "payloadFormatVersion": "1.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

/// Create one `REQUEST` authorizer under an HTTP `api_id` and answer its `AuthorizerId`.
async fn create_authorizer(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "route-authorizer",
            "authorizerType": "REQUEST",
            "identitySource": ["$request.header.Authorization"],
            "authorizerUri": "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:000000000000:function:authorizer/invocations",
            "authorizerPayloadFormatVersion": "2.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAuthorizer, got {status} {created}"
    );
    as_str(&created, "authorizerId")
}

/// Send one CreateRoute and answer `(status, body)`.
async fn create_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    request: &Value,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request.clone()),
    )
    .await
}

/// The route keys the api holds, in the order `GetRoutes` lists them.
async fn route_keys(emulator: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let (status, listed) = call(emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoutes, got {status} {listed}"
    );
    listed["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|route| as_str(route, "routeKey"))
        .collect()
}

#[tokio::test]
async fn a_route_whose_reference_its_api_cannot_resolve_is_refused_and_creates_nothing() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "the api under test", &http_api("reference-host")).await;
    let integration_id = create_integration(&emulator, "the api's integration", &api_id).await;
    let authorizer_id = create_authorizer(&emulator, "the api's authorizer", &api_id).await;

    // A second api holding TWO integrations: each api mints its ids from its own
    // sequence, so this api's second id is one the api under test never minted.
    let other_api = create_api(&emulator, "another api", &http_api("reference-other")).await;
    create_integration(&emulator, "another api's first integration", &other_api).await;
    let foreign_integration =
        create_integration(&emulator, "another api's second integration", &other_api).await;

    // And one integration of the api under test that is deleted again, so its id is one
    // that USED to name an integration here.
    let deleted_integration = create_integration(&emulator, "the integration to delete", &api_id).await;
    let (status, body) = call(
        &emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/integrations/{deleted_integration}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "expected 204 from DeleteIntegration, got {status} {body}"
    );

    let refused = [
        (
            "a target naming an integration no api ever minted",
            json!({ "routeKey": "GET /unknown-target", "target": "integrations/abc1234" }),
            "Unable to find route target of type integrations with ID abc1234".to_string(),
        ),
        (
            "a target naming a deleted integration of this api",
            json!({
                "routeKey": "GET /deleted-target",
                "target": format!("integrations/{deleted_integration}")
            }),
            format!(
                "Unable to find route target of type integrations with ID {deleted_integration}"
            ),
        ),
        (
            "a target naming an integration of ANOTHER api",
            json!({
                "routeKey": "GET /foreign-target",
                "target": format!("integrations/{foreign_integration}")
            }),
            format!(
                "Unable to find route target of type integrations with ID {foreign_integration}"
            ),
        ),
        (
            "a CUSTOM route naming an authorizer no api ever minted",
            json!({
                "routeKey": "GET /unknown-authorizer",
                "authorizationType": "CUSTOM",
                "authorizerId": "nonexistent123"
            }),
            UNRESOLVED_AUTHORIZER_MESSAGE.to_string(),
        ),
        (
            "a JWT route naming an authorizer no api ever minted",
            json!({
                "routeKey": "GET /unknown-jwt-authorizer",
                "authorizationType": "JWT",
                "authorizerId": "nonexistent123"
            }),
            UNRESOLVED_AUTHORIZER_MESSAGE.to_string(),
        ),
        (
            "both references unresolvable at once",
            json!({
                "routeKey": "GET /both-unknown",
                "target": "integrations/abc1234",
                "authorizationType": "CUSTOM",
                "authorizerId": "nonexistent123"
            }),
            // The target is resolved first, so its refusal is the one reported.
            "Unable to find route target of type integrations with ID abc1234".to_string(),
        ),
        (
            "a target of a form that addresses no integration",
            json!({ "routeKey": "GET /not-a-reference", "target": "integrations" }),
            // A value that is not `integrations/{IntegrationId}` is not a target at all,
            // so it is refused on its FORM before any id is resolved — the rule
            // `route_target_forms` holds over the whole class.
            "Unexpected or malformed target in route null. Correct format should be \
             integrations/<integration_id>."
                .to_string(),
        ),
    ];

    for (case, request, expect_message) in &refused {
        let before = route_keys(&emulator, &api_id).await;
        let (status, body) = create_route(&emulator, &api_id, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        // The target's refusal is built from the request — the kind of target the key
        // addresses and the id it carried — and the authorizer's states the requirement
        // the association failed.
        assert_eq!(
            &as_str(&body, "message"),
            expect_message,
            "case {case:?}: wrong refusal message, got {body}"
        );
        assert_eq!(
            route_keys(&emulator, &api_id).await,
            before,
            "case {case:?}: a refused CreateRoute must store no route"
        );
    }

    // The same requests with references this api CAN resolve are created.
    let accepted = [
        (
            "a target naming this api's integration",
            json!({
                "routeKey": "GET /resolved-target",
                "target": format!("integrations/{integration_id}")
            }),
        ),
        (
            "a CUSTOM route naming this api's authorizer",
            json!({
                "routeKey": "GET /resolved-authorizer",
                "authorizationType": "CUSTOM",
                "authorizerId": authorizer_id
            }),
        ),
        (
            "both references resolved at once",
            json!({
                "routeKey": "GET /both-resolved",
                "target": format!("integrations/{integration_id}"),
                "authorizationType": "JWT",
                "authorizerId": authorizer_id
            }),
        ),
        (
            "no references at all",
            json!({ "routeKey": "GET /no-references" }),
        ),
    ];

    for (case, request) in &accepted {
        let (status, body) = create_route(&emulator, &api_id, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRoute, got {status} {body}"
        );
        let route_id = as_str(&body, "routeId");
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoute, got {status} {fetched}"
        );
        assert_eq!(
            fetched, body,
            "case {case:?}: GetRoute must report what CreateRoute stored"
        );
    }
}

#[tokio::test]
async fn a_patch_cannot_move_a_route_onto_a_reference_its_api_cannot_resolve() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "patch references", &http_api("reference-patch")).await;
    let integration_id = create_integration(&emulator, "the api's integration", &api_id).await;
    let authorizer_id = create_authorizer(&emulator, "the api's authorizer", &api_id).await;

    let (status, created) = create_route(
        &emulator,
        &api_id,
        &json!({
            "routeKey": "GET /patched",
            "target": format!("integrations/{integration_id}"),
            "authorizationType": "CUSTOM",
            "authorizerId": authorizer_id
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateRoute, got {status} {created}"
    );
    let route_id = as_str(&created, "routeId");

    for (case, patch) in [
        (
            "a target naming an integration this api does not hold",
            json!({ "target": "integrations/zzz9999" }),
        ),
        (
            "an authorizer this api does not hold",
            json!({ "authorizerId": "nonexistent123" }),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            Some(patch),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        let (_, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            fetched, created,
            "case {case:?}: a refused UpdateRoute must leave the record as it was"
        );
    }
}

/// One row of the route-key compatibility table: the keys already on the api, the key
/// the request asks for, and whether it can live beside them.
struct KeyRow {
    case: &'static str,
    existing: &'static [&'static str],
    requested: &'static str,
    accepted: bool,
    /// For a refused row, the existing key whose variable the requested one collides
    /// with: the refusal message names both.
    conflicts_with: &'static str,
}

fn key_rows() -> Vec<KeyRow> {
    vec![
        KeyRow {
            case: "the greedy variable beside a named one at the same position",
            existing: &["GET /items", "GET /items/{id}"],
            requested: "GET /items/{proxy+}",
            accepted: false,
            conflicts_with: "GET /items/{id}",
        },
        KeyRow {
            case: "two differently named variables at the same position",
            existing: &["GET /items/{id}"],
            requested: "GET /items/{itemId}",
            accepted: false,
            conflicts_with: "GET /items/{id}",
        },
        KeyRow {
            case: "a conflict one position deeper",
            existing: &["GET /items/{id}/comments/{commentId}"],
            requested: "GET /items/{id}/comments/{index}",
            accepted: false,
            conflicts_with: "GET /items/{id}/comments/{commentId}",
        },
        KeyRow {
            case: "a conflict at the first position",
            existing: &["GET /{slug}"],
            requested: "GET /{other}",
            accepted: false,
            conflicts_with: "GET /{slug}",
        },
        KeyRow {
            case: "the ANY method is routed on its own, so it binds its own parameters",
            existing: &["GET /items/{id}"],
            requested: "ANY /items/{proxy+}",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "the same variable name at the same position",
            existing: &["GET /items/{id}"],
            requested: "DELETE /items/{id}",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "a differently named variable under another method",
            existing: &["GET /items/{id}"],
            requested: "POST /items/{itemId}",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "a variable deeper than any existing key reaches",
            existing: &["GET /items/{id}"],
            requested: "GET /items/{id}/comments/{commentId}",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "a literal beside a variable, which is a precedence case not a conflict",
            existing: &["GET /items/{id}"],
            requested: "GET /items/all",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "a variable under a different literal parent",
            existing: &["GET /items/{id}"],
            requested: "GET /orders/{orderId}",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "the catch-all route key, which names no path",
            existing: &["GET /items/{id}"],
            requested: "$default",
            accepted: true,
            conflicts_with: "",
        },
        KeyRow {
            case: "a greedy variable at the root beside a literal path",
            existing: &["GET /items"],
            requested: "ANY /{proxy+}",
            accepted: true,
            conflicts_with: "",
        },
    ]
}

#[tokio::test]
async fn a_route_key_declaring_a_second_variable_at_one_position_is_refused() {
    let emulator = emulator();

    for (index, row) in key_rows().into_iter().enumerate() {
        let api_id = create_api(
            &emulator,
            row.case,
            &http_api(&format!("route-key-{index}")),
        )
        .await;
        for existing in row.existing {
            let (status, body) =
                create_route(&emulator, &api_id, &json!({ "routeKey": existing })).await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 creating the existing key {existing:?}, got {status} {body}",
                row.case
            );
        }

        let (status, body) =
            create_route(&emulator, &api_id, &json!({ "routeKey": row.requested })).await;
        let keys = route_keys(&emulator, &api_id).await;
        if row.accepted {
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 for {:?}, got {status} {body}",
                row.case, row.requested
            );
            assert!(
                keys.iter().any(|key| key == row.requested),
                "case {:?}: the accepted key must be stored, got {keys:?}",
                row.case
            );
        } else {
            assert_eq!(
                status, 409,
                "case {:?}: expected 409 for {:?}, got {status} {body}",
                row.case, row.requested
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {:?}: expected ConflictException, got {body}",
                row.case
            );
            // The message is built from the request: the key it asked for and the key
            // already on the api whose variable sits at that level.
            assert_eq!(
                as_str(&body, "message"),
                format!(
                    "The provided route key \"{}\" has a conflicting variable on the \
                     same hierarchical level as \"{}\"",
                    row.requested, row.conflicts_with
                ),
                "case {:?}: wrong conflict message, got {body}",
                row.case
            );
            assert!(
                !keys.iter().any(|key| key == row.requested),
                "case {:?}: a refused CreateRoute must store no route, got {keys:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_patch_onto_a_conflicting_route_key_is_refused_and_changes_nothing() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "key patch", &http_api("route-key-patch")).await;
    for key in ["GET /items/{id}", "GET /other"] {
        let (status, body) = create_route(&emulator, &api_id, &json!({ "routeKey": key })).await;
        assert_eq!(
            status, 201,
            "expected 201 creating {key:?}, got {status} {body}"
        );
    }
    let (_, listed) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    let route = listed["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|route| as_str(route, "routeKey") == "GET /other")
        })
        .cloned()
        .unwrap_or(Value::Null);
    let route_id = as_str(&route, "routeId");

    let (status, body) = call(
        &emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        Some(json!({ "routeKey": "GET /items/{proxy+}" })),
    )
    .await;
    assert_eq!(
        status, 409,
        "expected 409 patching onto a conflicting key, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body}"
    );
    let (_, fetched) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await;
    assert_eq!(
        fetched, route,
        "a refused patch must leave the route exactly as it was"
    );
}

#[tokio::test]
async fn the_duplicate_route_key_conflict_message_is_built_from_the_request() {
    let emulator = emulator();

    // One key per api shape: the keys a method-and-path api and a WebSocket api carry.
    let cases = [
        ("a method-and-path key", "GET /dup", true),
        ("the catch-all key", "$default", true),
        ("a greedy key", "ANY /{proxy+}", true),
        ("a deep path key", "POST /items/{id}/comments", true),
        ("a WebSocket route key", "sendmessage", false),
        ("the WebSocket connect key", "$connect", false),
    ];

    for (index, (case, route_key, http)) in cases.into_iter().enumerate() {
        let request = if http {
            http_api(&format!("dup-http-{index}"))
        } else {
            websocket_api(&format!("dup-socket-{index}"))
        };
        let api_id = create_api(&emulator, case, &request).await;

        let (status, body) = create_route(&emulator, &api_id, &json!({ "routeKey": route_key })).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 creating {route_key:?}, got {status} {body}"
        );

        let (status, body) = create_route(&emulator, &api_id, &json!({ "routeKey": route_key })).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for a duplicate key, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        // The message is derived from the request, so it is the key the request sent and
        // nothing more: no trailing punctuation.
        assert_eq!(
            as_str(&body, "message"),
            format!("Route with key {route_key} already exists for this API"),
            "case {case:?}: wrong conflict message, got {body}"
        );
        assert_eq!(
            route_keys(&emulator, &api_id).await.len(),
            1,
            "case {case:?}: the refused duplicate must store no second route"
        );
    }
}

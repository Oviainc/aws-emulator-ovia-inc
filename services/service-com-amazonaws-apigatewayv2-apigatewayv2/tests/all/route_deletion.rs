// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteRoute`.
//!
//! The rules these cases assert:
//!
//! * **A route its api holds is removed**: `DeleteRoute` answers 204 with an empty body,
//!   `GetRoute` then reports 404, `GetRoutes` no longer lists it, and a second delete of
//!   the same id is a 404 rather than a second removal. Every other route of the api, and
//!   the api itself, survives; the key the route held is free again, and a route created
//!   under it is a NEW route with an id of its own.
//! * **A route owns its route responses**, so removing the route removes them with it: an
//!   id that resolved before the delete resolves to nothing after, and a route recreated
//!   under the same key starts with none of them.
//! * **A route holds the references, nothing references a route**: the integration a
//!   deleted route targeted is untouched by the delete — and the `DeleteIntegration` that
//!   the reference was blocking then goes through.
//! * **An identifier that resolves to nothing is a 404** `NotFoundException` that persists
//!   nothing: an api that does not exist, a route id nothing minted, and a route addressed
//!   through another api.
//!
//! The rows vary what the rules generalise over: the api's protocol type, the FORM of the
//! route key (`$connect`, `$default`, a method-and-path key, one with a path variable, a
//! WebSocket selection key), whether the route carries a target, how many sibling routes
//! the api holds, and whether API Gateway manages the route (the `$default` route of a
//! quick-created api). The recorded case that filed the issue — a route created on a
//! WebSocket api and then deleted — is one row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator, error_code};

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

/// Create one route under `api_id` and answer its `RouteId`.
async fn create_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    target: Option<String>,
) -> String {
    let mut request = json!({ "routeKey": route_key });
    if let Some(target) = target {
        request["target"] = json!(target);
    }
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
    as_str(&created, "routeId")
}

/// `(status, body)` of `GetRoute`.
async fn get_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    route_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await
}

/// `(status, body)` of `DeleteRoute`.
async fn delete_route(
    emulator: &MultiServiceRouter,
    api_id: &str,
    route_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await
}

/// Every `RouteId` `GetRoutes` lists for `api_id`.
async fn list_route_ids(emulator: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let (status, body) = call(emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(
        status, 200,
        "expected 200 from GetRoutes for {api_id}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "routeId"))
        .collect()
}

/// One row of the "the delete goes through" table: the api the route lives under, the key
/// of the route being deleted, whether that route carries a target, and the keys of the
/// sibling routes created beside it.
struct Removable {
    case: &'static str,
    api: Value,
    route_key: &'static str,
    /// `true` when the row's route is wired to an integration of its api, so the row also
    /// proves the integration survives the route that referenced it.
    targets_an_integration: bool,
    /// The keys of the routes created beside it, none of which the row deletes.
    sibling_keys: &'static [&'static str],
}

fn removable_rows() -> Vec<Removable> {
    vec![
        Removable {
            case: "a WebSocket api's $connect route, with no target",
            api: websocket_api("delete-route-connect-api"),
            route_key: "$connect",
            targets_an_integration: false,
            sibling_keys: &["$disconnect", "sendMessage"],
        },
        Removable {
            case: "a WebSocket api's selection key route wired to an integration",
            api: websocket_api("delete-route-selection-api"),
            route_key: "sendMessage",
            targets_an_integration: true,
            sibling_keys: &[],
        },
        Removable {
            case: "an HTTP api's method-and-path route, the only route of its api",
            api: http_api("delete-route-http-api"),
            route_key: "GET /orders",
            targets_an_integration: false,
            sibling_keys: &[],
        },
        Removable {
            case: "an HTTP api's route whose key carries a path variable",
            api: http_api("delete-route-variable-api"),
            route_key: "DELETE /orders/{orderId}",
            targets_an_integration: true,
            sibling_keys: &["GET /orders", "POST /orders"],
        },
        Removable {
            case: "an HTTP api's $default catch-all route",
            api: http_api("delete-route-default-api"),
            route_key: "$default",
            targets_an_integration: false,
            sibling_keys: &["GET /health"],
        },
    ]
}

/// An integration an api of either protocol accepts, for a row whose route needs a target.
/// `MOCK` is WebSocket-only (`Integration$IntegrationType`: "MOCK: for integrating the
/// route or method request with API Gateway as a \"loopback\" endpoint … Supported only
/// for WebSocket APIs"), so an HTTP api gets a proxy instead.
fn integration_request(api: &Value) -> Value {
    if as_str(api, "protocolType") == "WEBSOCKET" {
        json!({ "integrationType": "MOCK" })
    } else {
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/upstream",
            "payloadFormatVersion": "1.0"
        })
    }
}

/// Create one integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

#[tokio::test]
async fn delete_route_removes_the_record_and_a_later_read_reports_not_found() {
    let emulator = emulator();

    for row in removable_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let integration_id = if row.targets_an_integration {
            Some(
                create_integration(
                    &emulator,
                    row.case,
                    &api_id,
                    integration_request(&row.api),
                )
                .await,
            )
        } else {
            None
        };
        // API Gateway addresses an integration target as `integrations/{IntegrationId}`.
        let target = integration_id
            .as_ref()
            .map(|id| format!("integrations/{id}"));
        let route_id =
            create_route(&emulator, row.case, &api_id, row.route_key, target.clone()).await;
        let mut siblings = Vec::new();
        for key in row.sibling_keys {
            siblings.push(create_route(&emulator, row.case, &api_id, key, None).await);
        }

        let (status, body) = delete_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteRoute, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteRoute body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetRoute after the delete, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // And gone from the listing, while every sibling route stays.
        let listed = list_route_ids(&emulator, &api_id).await;
        assert!(
            !listed.contains(&route_id),
            "case {:?}: expected {route_id} to be gone from GetRoutes, got {listed:?}",
            row.case
        );
        for sibling in &siblings {
            assert!(
                listed.contains(sibling),
                "case {:?}: expected sibling route {sibling} to survive, got {listed:?}",
                row.case
            );
        }

        // A second delete removes nothing: the id now resolves to nothing.
        let (status, body) = delete_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from the second DeleteRoute, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // The api itself is untouched by a route delete.
        let (status, api) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the api to survive the delete, got {status} {api}",
            row.case
        );

        // A route holds the reference to its integration, so the integration is
        // untouched — and the delete the reference was blocking now goes through.
        if let Some(integration_id) = integration_id {
            let (status, fetched) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the targeted integration to survive, got {status} {fetched}",
                row.case
            );
            let (status, body) = call(
                &emulator,
                "DELETE",
                &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 204,
                "case {:?}: expected 204 from DeleteIntegration once no route targets it, \
                 got {status} {body}",
                row.case
            );
        }

        // The key the deleted route held is free again, and the route created under it is
        // a NEW route: a fresh id, not the one the delete removed.
        let recreated = create_route(&emulator, row.case, &api_id, row.route_key, None).await;
        assert_minted_id(row.case, "RouteId", &recreated);
        assert_ne!(
            recreated, route_id,
            "case {:?}: expected a freshly minted RouteId for the recreated route, got \
             the deleted route's id {route_id}",
            row.case
        );
        let (status, fetched) = get_route(&emulator, &api_id, &recreated).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRoute for the recreated route, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched.get("routeKey"),
            Some(&json!(row.route_key)),
            "case {:?}: expected the recreated route to carry the freed key, got {fetched}",
            row.case
        );
    }
}

/// The `$default` route a quick-created api carries is managed by API Gateway
/// (`Route$ApiGatewayManaged`: "If you created an API using quick create, the $default
/// route is managed by API Gateway"). Such a route is the SERVICE's, not the caller's: a
/// `DeleteRoute` that addresses it is accepted with the same empty success every other
/// delete answers, and removes nothing — the managed route stays listed, readable and
/// patchable under the id it already had.
#[tokio::test]
async fn an_api_gateway_managed_route_outlives_a_delete_that_addresses_it() {
    let emulator = emulator();

    let api_id = create_api(
        &emulator,
        "a quick-created api",
        &json!({
            "name": "delete-route-quick-create",
            "protocolType": "HTTP",
            "routeKey": "$default",
            "target": "https://example.com/upstream"
        }),
    )
    .await;

    let listed = list_route_ids(&emulator, &api_id).await;
    assert_eq!(
        listed.len(),
        1,
        "expected quick create to mint exactly one route, got {listed:?}"
    );
    let managed = listed[0].clone();
    let (status, route) = get_route(&emulator, &api_id, &managed).await;
    assert_eq!(status, 200, "expected 200 from GetRoute, got {status} {route}");
    assert_eq!(
        route.get("apiGatewayManaged"),
        Some(&json!(true)),
        "expected the quick-created $default route to be managed by API Gateway, got {route}"
    );

    // Twice, so a caller that retries its delete is told the same thing every time and the
    // route is still there after both.
    for attempt in 1..=2 {
        let (status, body) = delete_route(&emulator, &api_id, &managed).await;
        assert_eq!(
            status, 204,
            "attempt {attempt}: expected 204 from DeleteRoute on the managed route, got \
             {status} {body}"
        );
        let (status, after) = get_route(&emulator, &api_id, &managed).await;
        assert_eq!(
            status, 200,
            "attempt {attempt}: expected the managed route to outlive the delete, got \
             {status} {after}"
        );
        assert_eq!(
            after, route,
            "attempt {attempt}: a delete that removes nothing must leave the managed route \
             exactly as it was, got {after}"
        );
        assert_eq!(
            list_route_ids(&emulator, &api_id).await,
            vec![managed.clone()],
            "attempt {attempt}: expected the api to still list its managed route"
        );
    }
}

/// A route OWNS its route responses, so the delete takes them with it — and a route
/// created afterwards under the same key starts with none of them, whatever ids its own
/// sequence mints.
#[tokio::test]
async fn deleting_a_route_takes_the_route_responses_it_owns_with_it() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "the owning api",
        &websocket_api("delete-route-responses-api"),
    )
    .await;

    // The rows differ in how many route responses hang off the route: none, one, several.
    for (case, response_keys) in [
        ("a route holding no route response", &[][..]),
        ("a route holding one route response", &["$default"][..]),
        (
            "a route holding several route responses",
            &["$default", "echo", "status"][..],
        ),
    ] {
        let route_id = create_route(&emulator, case, &api_id, "$connect", None).await;
        let sibling = create_route(&emulator, case, &api_id, "sendMessage", None).await;

        let mut response_ids = Vec::new();
        for key in response_keys {
            let (status, created) = call(
                &emulator,
                "POST",
                &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
                Some(json!({ "routeResponseKey": key })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateRouteResponse, got {status} {created}"
            );
            response_ids.push(as_str(&created, "routeResponseId"));
        }
        // A route response of the SIBLING route, which this delete must not touch.
        let (status, sibling_response) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes/{sibling}/routeresponses"),
            Some(json!({ "routeResponseKey": "$default" })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateRouteResponse on the sibling, got \
             {status} {sibling_response}"
        );
        let sibling_response_id = as_str(&sibling_response, "routeResponseId");

        let (status, body) = delete_route(&emulator, &api_id, &route_id).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteRoute, got {status} {body}"
        );

        // Nothing resolves an id of a response the deleted route owned.
        for response_id in &response_ids {
            let (status, body) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/{response_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for a route response of the deleted route, \
                 got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {body}"
            );
        }

        // A route recreated under the same key holds none of them, even where its own
        // sequence mints the very same ids.
        let recreated = create_route(&emulator, case, &api_id, "$connect", None).await;
        for response_id in &response_ids {
            let (status, body) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/routes/{recreated}/routeresponses/{response_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the recreated route to hold no route response, \
                 got {status} {body}"
            );
        }

        // The sibling route and its own route response are untouched.
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{sibling}/routeresponses/{sibling_response_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the sibling's route response to survive, got {status} {body}"
        );

        // Clear the api for the next row.
        for route in [recreated, sibling] {
            let (status, body) = delete_route(&emulator, &api_id, &route).await;
            assert_eq!(
                status, 204,
                "case {case:?}: expected 204 while clearing route {route}, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn delete_route_answers_not_found_for_an_identifier_that_resolves_to_nothing() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", &http_api("delete-route-owner")).await;
    let other = create_api(&emulator, "another api", &http_api("delete-route-other")).await;
    let route_id = create_route(
        &emulator,
        "the owning api's route",
        &owner,
        "GET /orders",
        None,
    )
    .await;

    for (case, api_id, target_id) in [
        ("an api that does not exist", "nosuchapi00", route_id.as_str()),
        ("a route id nothing minted", owner.as_str(), "nosuchroute"),
        (
            "a route addressed through another api",
            other.as_str(),
            route_id.as_str(),
        ),
    ] {
        let (status, body) = delete_route(&emulator, api_id, target_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteRoute, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals removed the route they named.
    let (status, body) = get_route(&emulator, &owner, &route_id).await;
    assert_eq!(
        status, 200,
        "expected the route to survive every refused delete, got {status} {body}"
    );
}

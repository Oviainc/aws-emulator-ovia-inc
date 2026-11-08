// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ApiGatewayV2 `UpdateRouteResponse` operation.
//!
//! The rules these cases assert:
//!
//! * **`UpdateRouteResponse` is a PATCH over the stored route response**: a member the
//!   request carries replaces the stored one, and a member it omits leaves the stored
//!   value exactly as it was. A map member is a value like any other, so one the request
//!   carries replaces the stored map wholesale (a map sent empty clears it), and the
//!   `RouteResponseId` is identity and never changes.
//! * What the update answers is what the store holds: a later `GetRouteResponse` reports
//!   the same record, member for member.
//! * A route response's key identifies it WITHIN its route, so an update cannot move a
//!   response onto a key a sibling response of the same route already holds (400), while
//!   re-sending its own key is accepted.
//! * A route response is addressed only through the api and the route that own it: an
//!   unknown api, route or route-response id is a 404 and writes nothing.
//! * A route response is a WebSocket-only resource
//!   (`RouteResponse$ModelSelectionExpression`: "Supported only for WebSocket APIs"), so
//!   an update against an api of another protocol type is refused 400.
//!
//! The rows vary what the rule generalises over: WHICH members the patch carries (none,
//! one, several, all), the form of the new route response key, and how many entries the
//! replacement maps hold (none, one, several). The recorded case that filed the issue —
//! a patch that must preserve the fields it does not name — is the first row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::duplicate_identity::route_response_key_in_use_message;

/// The route response every row starts from: every member the resource carries is set,
/// so a row that omits one proves preservation rather than absence.
fn baseline() -> Value {
    json!({
        "routeResponseKey": "$default",
        "modelSelectionExpression": "$request.body.action",
        "responseModels": { "$default": "Empty", "application/json": "Reply" },
        "responseParameters": {
            "route.response.header.x-trace": { "required": true },
            "route.response.header.x-hint": { "required": false }
        }
    })
}

/// One row: the patch request, and the members the stored route response must report
/// afterwards.
struct Row {
    case: &'static str,
    patch: Value,
    /// Members the update — and the later `GetRouteResponse` — must carry.
    expected: fn() -> Vec<(&'static str, Value)>,
    /// Members that carry nothing after the patch, because the request cleared them.
    expect_empty: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an empty patch preserves every stored member",
            patch: json!({}),
            expected: || baseline_members(),
            expect_empty: &[],
        },
        Row {
            case: "only the route response key, a custom selection key",
            patch: json!({ "routeResponseKey": "echo" }),
            expected: || {
                replacing(&[("routeResponseKey", json!("echo"))])
            },
            expect_empty: &[],
        },
        Row {
            case: "only the model selection expression",
            patch: json!({ "modelSelectionExpression": "$request.body.kind" }),
            expected: || {
                replacing(&[(
                    "modelSelectionExpression",
                    json!("$request.body.kind"),
                )])
            },
            expect_empty: &[],
        },
        Row {
            case: "one response model replaces the stored map wholesale",
            patch: json!({ "responseModels": { "$default": "Reply" } }),
            expected: || replacing(&[("responseModels", json!({ "$default": "Reply" }))]),
            expect_empty: &[],
        },
        Row {
            case: "a response models map sent empty clears it",
            patch: json!({ "responseModels": {} }),
            expected: || {
                baseline_members()
                    .into_iter()
                    .filter(|(member, _)| *member != "responseModels")
                    .collect()
            },
            expect_empty: &["responseModels"],
        },
        Row {
            case: "several response parameters replace the stored map",
            patch: json!({
                "responseParameters": {
                    "route.response.header.x-a": { "required": false },
                    "route.response.header.x-b": { "required": true },
                    "route.response.header.x-c": { "required": true }
                }
            }),
            expected: || {
                replacing(&[(
                    "responseParameters",
                    json!({
                        "route.response.header.x-a": { "required": false },
                        "route.response.header.x-b": { "required": true },
                        "route.response.header.x-c": { "required": true }
                    }),
                )])
            },
            expect_empty: &[],
        },
        Row {
            case: "a response parameters map sent empty clears it",
            patch: json!({ "responseParameters": {} }),
            expected: || {
                baseline_members()
                    .into_iter()
                    .filter(|(member, _)| *member != "responseParameters")
                    .collect()
            },
            expect_empty: &["responseParameters"],
        },
        Row {
            case: "every member at once",
            patch: json!({
                "routeResponseKey": "$connect",
                "modelSelectionExpression": "$default",
                "responseModels": { "text/plain": "Empty" },
                "responseParameters": { "route.response.header.x-only": { "required": true } }
            }),
            expected: || {
                vec![
                    ("routeResponseKey", json!("$connect")),
                    ("modelSelectionExpression", json!("$default")),
                    ("responseModels", json!({ "text/plain": "Empty" })),
                    (
                        "responseParameters",
                        json!({ "route.response.header.x-only": { "required": true } }),
                    ),
                ]
            },
            expect_empty: &[],
        },
        Row {
            case: "re-sending the key it already holds is not a conflict with itself",
            patch: json!({ "routeResponseKey": "$default" }),
            expected: || baseline_members(),
            expect_empty: &[],
        },
    ]
}

/// The baseline's members, as the wire reports them.
fn baseline_members() -> Vec<(&'static str, Value)> {
    vec![
        ("routeResponseKey", json!("$default")),
        ("modelSelectionExpression", json!("$request.body.action")),
        (
            "responseModels",
            json!({ "$default": "Empty", "application/json": "Reply" }),
        ),
        (
            "responseParameters",
            json!({
                "route.response.header.x-trace": { "required": true },
                "route.response.header.x-hint": { "required": false }
            }),
        ),
    ]
}

/// The baseline's members with `overrides` applied: what a patch carrying exactly those
/// members must leave behind.
fn replacing(overrides: &[(&'static str, Value)]) -> Vec<(&'static str, Value)> {
    baseline_members()
        .into_iter()
        .map(|(member, value)| {
            match overrides.iter().find(|(name, _)| *name == member) {
                Some((_, replacement)) => (member, replacement.clone()),
                None => (member, value),
            }
        })
        .collect()
}

/// A WebSocket api, the protocol whose routes carry route responses.
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

async fn create_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(json!({ "routeKey": route_key })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRoute, got {status} {created}"
    );
    as_str(&created, "routeId")
}

async fn create_route_response(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateRouteResponse, got {status} {created}"
    );
    as_str(&created, "routeResponseId")
}

#[tokio::test]
async fn update_route_response_replaces_the_members_it_carries_and_preserves_the_rest() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &websocket_api("patch-ws-api")).await;

    for (index, row) in rows().into_iter().enumerate() {
        // A route of its own per row, so each patch starts from the same baseline and no
        // row's route response key collides with another's.
        let route_id = create_route(&emulator, row.case, &api_id, &format!("row-{index}")).await;
        let route_response_id =
            create_route_response(&emulator, row.case, &api_id, &route_id, &baseline()).await;
        let uri = format!(
            "/v2/apis/{api_id}/routes/{route_id}/routeresponses/{route_response_id}"
        );

        let (status, updated) = call(&emulator, "PATCH", &uri, Some(row.patch.clone())).await;
        assert_eq!(
            status, 201,
            "case {:?}: expected 201 from UpdateRouteResponse, got {status} {updated}",
            row.case
        );
        assert_eq!(
            as_str(&updated, "routeResponseId"),
            route_response_id,
            "case {:?}: the RouteResponseId is identity and must not change, got {updated}",
            row.case
        );

        for (member, value) in (row.expected)() {
            assert_eq!(
                updated.get(member),
                Some(&value),
                "case {:?}: expected UpdateRouteResponse to report {member}={value}, got {updated}",
                row.case
            );
        }
        for member in row.expect_empty {
            assert!(
                carries_nothing(&updated, member),
                "case {:?}: the patch cleared {member}, so it must carry nothing, got {updated}",
                row.case
            );
        }

        // What the update answered is what the store holds.
        let (status, fetched) = call(&emulator, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetRouteResponse, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, updated,
            "case {:?}: GetRouteResponse must report the record UpdateRouteResponse stored",
            row.case
        );
    }
}

#[tokio::test]
async fn update_route_response_cannot_take_a_key_a_sibling_response_holds() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &websocket_api("conflict-ws-api")).await;
    let route_id = create_route(&emulator, "the owning route", &api_id, "$connect").await;

    let first = create_route_response(
        &emulator,
        "the first response",
        &api_id,
        &route_id,
        &json!({ "routeResponseKey": "$default" }),
    )
    .await;
    let second = create_route_response(
        &emulator,
        "the sibling response",
        &api_id,
        &route_id,
        &json!({ "routeResponseKey": "echo" }),
    )
    .await;

    for (case, target, key) in [
        ("onto the sibling's key", &second, "$default"),
        ("onto the first's key", &first, "echo"),
    ] {
        let uri = format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/{target}");
        let before = call(&emulator, "GET", &uri, None).await.1;
        let (status, body) = call(
            &emulator,
            "PATCH",
            &uri,
            Some(json!({ "routeResponseKey": key })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateRouteResponse, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            route_response_key_in_use_message(key),
            "case {case:?}: expected the refusal to name the key that collided, got {body}"
        );
        let after = call(&emulator, "GET", &uri, None).await.1;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the route response unchanged, got {after}"
        );
    }

    // The key is free on another route of the same api: the bound is per route.
    let sibling_route = create_route(&emulator, "another route", &api_id, "sendMessage").await;
    let elsewhere = create_route_response(
        &emulator,
        "a response on another route",
        &api_id,
        &sibling_route,
        &json!({ "routeResponseKey": "echo" }),
    )
    .await;
    let (status, updated) = call(
        &emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/routes/{sibling_route}/routeresponses/{elsewhere}"),
        Some(json!({ "routeResponseKey": "$default" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 for a key another ROUTE holds, got {status} {updated}"
    );
    assert_eq!(
        as_str(&updated, "routeResponseKey"),
        "$default",
        "expected the sibling route's response to take the key, got {updated}"
    );
}

#[tokio::test]
async fn update_route_response_resolves_only_through_the_api_and_route_that_own_it() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the owning api", &websocket_api("owner-ws-api")).await;
    let other_api = create_api(&emulator, "another api", &websocket_api("other-ws-api")).await;
    let route_id = create_route(&emulator, "the owning route", &api_id, "$connect").await;
    let other_route = create_route(&emulator, "another route", &api_id, "sendMessage").await;
    let route_response_id =
        create_route_response(&emulator, "the response", &api_id, &route_id, &baseline()).await;
    let uri = format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/{route_response_id}");
    let before = call(&emulator, "GET", &uri, None).await.1;

    for (case, target) in [
        (
            "another route of the same api",
            format!("/v2/apis/{api_id}/routes/{other_route}/routeresponses/{route_response_id}"),
        ),
        (
            "another api",
            format!("/v2/apis/{other_api}/routes/{route_id}/routeresponses/{route_response_id}"),
        ),
        (
            "a route response id nothing minted",
            format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/nosuchresp0"),
        ),
        (
            "a route id nothing minted",
            format!("/v2/apis/{api_id}/routes/nosuchroute/routeresponses/{route_response_id}"),
        ),
        (
            "an api id nothing minted",
            format!("/v2/apis/nosuchapi00/routes/{route_id}/routeresponses/{route_response_id}"),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &target,
            Some(json!({ "routeResponseKey": "moved" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from UpdateRouteResponse, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    let after = call(&emulator, "GET", &uri, None).await.1;
    assert_eq!(
        after, before,
        "a refused update must write nothing, got {after}"
    );
}

#[tokio::test]
async fn update_route_response_is_refused_on_an_api_that_is_not_a_websocket_api() {
    let emulator = emulator();
    let api_id = create_api(
        &emulator,
        "an http api",
        &json!({ "name": "http-no-route-responses", "protocolType": "HTTP" }),
    )
    .await;
    let route_id = create_route(&emulator, "a route of an http api", &api_id, "GET /items").await;

    for (case, patch) in [
        ("only the key", json!({ "routeResponseKey": "$default" })),
        (
            "the key with response models",
            json!({ "routeResponseKey": "echo", "responseModels": { "$default": "Empty" } }),
        ),
        ("an empty patch", json!({})),
    ] {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses/nosuchresp0"),
            Some(patch),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from UpdateRouteResponse on an HTTP api, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }
}

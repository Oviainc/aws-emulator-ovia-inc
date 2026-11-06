// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteIntegration`.
//!
//! The rules these cases assert:
//!
//! * **An integration no route of its api is wired to is removed**: `DeleteIntegration`
//!   answers 204 with an empty body, `GetIntegration` then reports 404, `GetIntegrations`
//!   no longer lists it, and a second delete of the same id is a 404 rather than a second
//!   removal. Every other integration of the api, and every route, survives.
//! * **An integration a route of the same api still targets cannot be removed**: the
//!   delete is refused 409 `ConflictException` and persists nothing, so the integration is
//!   still readable afterwards. A route references an integration through its
//!   `integrations/{IntegrationId}` `Target`, so the reference is what blocks the delete:
//!   re-point or drop the route and the same delete then succeeds.
//! * **An identifier that resolves to nothing is a 404**: an api that does not exist, an
//!   integration id nothing minted, and an integration addressed through another api.
//!
//! The rows vary what the rules generalise over: the api's protocol type, the integration
//! type, how many routes reference the integration, and the FORM of a route's target
//! (absent, another integration of the same api, a string that is no integration
//! reference at all), plus a route of ANOTHER api carrying the same target string, which
//! references nothing here. The recorded case that filed the issue — one route targeting
//! the integration being deleted — is one row of the refusal table.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// How a route names the integration it is wired to. API Gateway addresses an
/// integration target as `integrations/{IntegrationId}` — the form `CreateRoute$Target`
/// takes and the one every route reader reports back.
fn integration_target(integration_id: &str) -> String {
    format!("integrations/{integration_id}")
}

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

/// A second integration an api of `api`'s protocol accepts, for a row that needs one
/// beside the integration it deletes. `MOCK` is WebSocket-only (`IntegrationType`: "MOCK:
/// for integrating the route or method request with API Gateway as a \"loopback\"
/// endpoint ... Supported only for WebSocket APIs"), so an HTTP api gets a proxy.
fn spare_integration(api: &Value) -> Value {
    if as_str(api, "protocolType") == "WEBSOCKET" {
        json!({ "integrationType": "MOCK" })
    } else {
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/spare",
            "payloadFormatVersion": "1.0"
        })
    }
}

/// Create one api and answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one integration under `api_id` and answer its `IntegrationId`.
async fn create_integration(
    emulator: &smithy_server_runtime::MultiServiceRouter,
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

/// Create one route under `api_id` and answer its `RouteId`.
async fn create_route(
    emulator: &smithy_server_runtime::MultiServiceRouter,
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

/// `(status, body)` of `GetIntegration`.
async fn get_integration(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    integration_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        None,
    )
    .await
}

/// `(status, body)` of `DeleteIntegration`.
async fn delete_integration(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    integration_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        None,
    )
    .await
}

/// Every `IntegrationId` `GetIntegrations` lists for `api_id`.
async fn list_integration_ids(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
) -> Vec<String> {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/integrations"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetIntegrations for {api_id}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "integrationId"))
        .collect()
}

/// One row of the "the delete goes through" table: the api the integration lives under,
/// the integration to create, and a route whose target does NOT reference it.
struct Removable {
    case: &'static str,
    api: Value,
    integration: Value,
    /// The route key of a route created beside the integration.
    route_key: &'static str,
    /// That route's `Target`, built from the ids the case minted: the integration being
    /// deleted (`None` here by construction), another integration, or a free string.
    /// `None` leaves the route with no target at all.
    target: fn(&Context) -> Option<String>,
}

/// The ids one row minted, for a row's `target` to build a reference from.
struct Context {
    /// The integration the row deletes.
    integration_id: String,
    /// A second integration of the same api, which the row never deletes.
    other_integration_id: String,
}

fn removable_rows() -> Vec<Removable> {
    vec![
        Removable {
            case: "an HTTP api AWS_PROXY integration no route is wired to",
            api: http_api("delete-unused-http-api"),
            integration: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:orders",
                "payloadFormatVersion": "2.0"
            }),
            route_key: "GET /orders",
            target: |_| None,
        },
        Removable {
            case: "an integration whose api's route targets ANOTHER integration",
            api: http_api("delete-sibling-target-api"),
            integration: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0"
            }),
            route_key: "POST /orders",
            target: |context| Some(integration_target(&context.other_integration_id)),
        },
        Removable {
            case: "a WebSocket api MOCK integration beside a route targeting no integration",
            api: websocket_api("delete-socket-api"),
            integration: json!({ "integrationType": "MOCK" }),
            route_key: "$connect",
            // A route with no target at all references no integration of this api: a
            // target it DID carry would have to name one
            // (`integrations/{IntegrationId}`), which is what the sibling rows cover.
            target: |_| None,
        },
    ]
}

#[tokio::test]
async fn delete_integration_removes_the_record_and_a_later_read_reports_not_found() {
    let emulator = emulator();

    for row in removable_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let integration_id =
            create_integration(&emulator, row.case, &api_id, row.integration.clone()).await;
        let other_integration_id =
            create_integration(&emulator, row.case, &api_id, spare_integration(&row.api)).await;
        let context = Context {
            integration_id: integration_id.clone(),
            other_integration_id: other_integration_id.clone(),
        };
        // The route the row puts beside the integration: whatever its target, it does
        // not reference the integration being deleted.
        let route_id = create_route(
            &emulator,
            row.case,
            &api_id,
            row.route_key,
            (row.target)(&context),
        )
        .await;
        assert_ne!(
            (row.target)(&context),
            Some(integration_target(&context.integration_id)),
            "case {:?}: this row's route must not target the integration it deletes",
            row.case
        );

        let (status, body) = delete_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteIntegration, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteIntegration body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetIntegration after the delete, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // And gone from the listing, while the api's other integration stays.
        let listed = list_integration_ids(&emulator, &api_id).await;
        assert!(
            !listed.contains(&integration_id),
            "case {:?}: expected {integration_id} to be gone from GetIntegrations, got {listed:?}",
            row.case
        );
        assert!(
            listed.contains(&other_integration_id),
            "case {:?}: expected {other_integration_id} to survive, got {listed:?}",
            row.case
        );

        // A second delete removes nothing: the id now resolves to nothing.
        let (status, body) = delete_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from the second DeleteIntegration, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // The routes of the api are untouched by an integration delete.
        let (status, route) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/routes/{route_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the route to survive the delete, got {status} {route}",
            row.case
        );
    }
}

/// An `IntegrationId` resolves only through the api that owns it, so a route of ANOTHER
/// api targeting an integration of its own blocks nothing here: only the routes of the
/// integration's OWN api can block its delete.
#[tokio::test]
async fn a_route_of_another_api_does_not_block_the_delete() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", &http_api("scoped-owner-api")).await;
    let integration_id = create_integration(
        &emulator,
        "the owning api's integration",
        &owner,
        json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:scoped",
            "payloadFormatVersion": "2.0"
        }),
    )
    .await;

    // A DIFFERENT api, holding an integration of its own — a minted `IntegrationId`
    // denotes ONE integration, so the bystander's carries an id of its own — targeted by a
    // route of that api. A route may only target an integration of its own api, so this is
    // the blocking reference that exists anywhere but in the owner's api.
    let bystander = create_api(&emulator, "the bystander api", &http_api("scoped-other-api")).await;
    let bystander_integration = create_integration(
        &emulator,
        "the bystander api's integration",
        &bystander,
        json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:bystander",
            "payloadFormatVersion": "2.0"
        }),
    )
    .await;
    assert_ne!(
        bystander_integration, integration_id,
        "a minted IntegrationId denotes one integration, so the bystander api's must \
         differ from the owner's; got {bystander_integration} and {integration_id}"
    );
    let bystander_route = create_route(
        &emulator,
        "the bystander api's route",
        &bystander,
        "GET /scoped",
        Some(integration_target(&bystander_integration)),
    )
    .await;

    let (status, body) = delete_integration(&emulator, &owner, &integration_id).await;
    assert_eq!(
        status, 204,
        "expected 204: no route of the OWNING api targets the integration, got {status} {body}"
    );

    let (status, route) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{bystander}/routes/{bystander_route}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the other api's route to be untouched, got {status} {route}"
    );
    assert_eq!(
        route.get("target"),
        Some(&json!(integration_target(&bystander_integration))),
        "expected the other api's route to keep its target, got {route}"
    );

    // And the bystander's own integration, which its route targets, is still refused.
    let (status, body) = delete_integration(&emulator, &bystander, &bystander_integration).await;
    assert_eq!(
        status, 409,
        "expected 409 for the integration the bystander's route targets, got {status} {body}"
    );
}

/// One row of the refusal table: how many routes of the api target the integration, and
/// which route keys they carry.
struct InUse {
    case: &'static str,
    api: Value,
    integration: Value,
    /// The route keys of the routes wired to the integration. Non-empty by construction.
    route_keys: &'static [&'static str],
}

fn in_use_rows() -> Vec<InUse> {
    vec![
        InUse {
            case: "one route targets the integration",
            api: http_api("in-use-single-api"),
            integration: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:us-east-1:000000000000:function:recorder",
                "payloadFormatVersion": "2.0"
            }),
            route_keys: &["GET /recorded"],
        },
        InUse {
            case: "three routes of the api target the one integration",
            api: http_api("in-use-fanin-api"),
            integration: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "payloadFormatVersion": "1.0"
            }),
            route_keys: &["GET /items", "POST /items", "DELETE /items/{id}"],
        },
        InUse {
            case: "a WebSocket api's $default route targets the integration",
            api: websocket_api("in-use-socket-api"),
            integration: json!({ "integrationType": "MOCK" }),
            route_keys: &["$default"],
        },
    ]
}

#[tokio::test]
async fn delete_integration_is_refused_while_a_route_of_its_api_targets_it() {
    let emulator = emulator();

    for row in in_use_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let integration_id =
            create_integration(&emulator, row.case, &api_id, row.integration.clone()).await;
        // A second integration for the routes to be re-pointed at below.
        let spare_integration_id =
            create_integration(&emulator, row.case, &api_id, spare_integration(&row.api)).await;

        let mut route_ids = Vec::new();
        for route_key in row.route_keys {
            route_ids.push(
                create_route(
                    &emulator,
                    row.case,
                    &api_id,
                    route_key,
                    Some(integration_target(&integration_id)),
                )
                .await,
            );
        }

        let (status, body) = delete_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 from DeleteIntegration while {} route(s) target it, got {status} {body}",
            row.case,
            route_ids.len()
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {:?}: expected ConflictException, got {body}",
            row.case
        );

        // The refusal persisted nothing: the integration is still there to read.
        let (status, fetched) = get_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the refused integration to survive, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "integrationId"),
            integration_id,
            "case {:?}: expected the same integration back, got {fetched}",
            row.case
        );

        // What blocks the delete is the REFERENCE, not the integration: re-point every
        // route at the spare integration and the very same delete goes through.
        for route_id in &route_ids {
            let (status, updated) = call(
                &emulator,
                "PATCH",
                &format!("/v2/apis/{api_id}/routes/{route_id}"),
                Some(json!({ "target": integration_target(&spare_integration_id) })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: expected 201 from UpdateRoute, got {status} {updated}",
                row.case
            );
        }

        let (status, body) = delete_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 once no route targets the integration, got {status} {body}",
            row.case
        );

        // And the spare, which every route now targets, is refused in its turn.
        let (status, body) = delete_integration(&emulator, &api_id, &spare_integration_id).await;
        assert_eq!(
            status, 409,
            "case {:?}: expected 409 for the integration the routes now target, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {:?}: expected ConflictException, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn delete_integration_answers_not_found_for_an_identifier_that_resolves_to_nothing() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", &http_api("delete-owner-api")).await;
    let other = create_api(&emulator, "another api", &http_api("delete-other-api")).await;
    let integration_id = create_integration(
        &emulator,
        "the owning api's integration",
        &owner,
        json!({
            "integrationType": "HTTP_PROXY",
            "integrationUri": "https://example.com/upstream",
            "payloadFormatVersion": "1.0"
        }),
    )
    .await;

    for (case, api_id, target_id) in [
        ("an api that does not exist", "nosuchapi00", &integration_id),
        (
            "an integration id nothing minted",
            owner.as_str(),
            &"nosuchint0".to_string(),
        ),
        (
            "an integration addressed through another api",
            other.as_str(),
            &integration_id,
        ),
    ] {
        let (status, body) = delete_integration(&emulator, api_id, target_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteIntegration, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals removed the integration they named.
    let (status, body) = get_integration(&emulator, &owner, &integration_id).await;
    assert_eq!(
        status, 200,
        "expected the integration to survive every refused delete, got {status} {body}"
    );
}

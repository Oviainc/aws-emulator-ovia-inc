// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the IDENTITY a `ReimportApi` leaves its api's children with.
//!
//! The rule these cases assert: putting an OpenAPI definition over an existing api
//! replaces the configuration of the routes and integrations it declares WITHOUT
//! re-identifying the ones it declares again — a route the api already holds under the key
//! a declaration carries keeps its `RouteId`, and the integration that route's `Target`
//! names keeps its `IntegrationId`, so an id a caller captured before the call still
//! addresses the resource that key names. Only a declaration the api holds nothing under
//! is given a freshly minted id, and only a key the document no longer declares is dropped
//! — together with the integration it alone was wired to.
//!
//! Every id is read back through `GetRoutes`/`GetIntegrations`, and the carried ones are
//! then addressed DIRECTLY through `GetRoute`/`GetIntegration`, so what is asserted went
//! through the store rather than through the reimport's own answer.
//!
//! The rows span what the rule generalises over: how many declarations are carried, added
//! and dropped (one of each, all three at once, every key replaced, none at all), what
//! changes about a carried declaration (its integration's URI, its integration's type and
//! payload format version, nothing at all), the document's spelling (OpenAPI 3 and Swagger
//! 2.0, including a route key written as a catch-all `ANY` operation), whether the
//! declaration carries an integration at all, and the `Basepath` the reimport is read
//! under — which is what makes the same bytes declare different keys. The recorded case
//! that filed the issue — an import, then a reimport of an unchanged path, then `GetRoutes`
//! — is the first row.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_import::{import, stored_integrations};
use crate::api_lifecycle::{as_str, assert_minted_id, call, emulator};
use crate::api_reimport::reimport;

/// A Lambda function ARN in the form an `aws_proxy` integration extension names one.
const LAMBDA_URI: &str = "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:123456789012:function:orders/invocations";

/// Whether the reimport may re-identify a route it declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// The api already held this key, so the route and its integration keep the ids the
    /// earlier document gave them.
    Carried,
    /// The api held nothing under this key, so both ids are freshly minted.
    Added,
}

/// One route the api must hold after the reimport.
struct ExpectRoute {
    route_key: &'static str,
    identity: Identity,
    /// The `IntegrationUri` the route's integration must carry afterwards — `None` for a
    /// declaration that names no integration at all, which leaves the route with no target.
    integration_uri: Option<&'static str>,
}

/// One row: the document the api is imported from, the one put over it, and the identity
/// every route of the api must have afterwards.
struct Row {
    case: &'static str,
    first: String,
    second: String,
    /// The query the REIMPORT carries (`""`, or `"?basepath=prepend"`); the first
    /// document is always imported under the default `Basepath`.
    query: &'static str,
    expect_routes: Vec<ExpectRoute>,
    /// Route keys only the FIRST document declared: gone after the reimport, and so is the
    /// integration each one alone was wired to.
    expect_dropped: Vec<&'static str>,
}

/// An `x-amazon-apigateway-integration` extension naming an `HTTP_PROXY` upstream.
fn http_proxy(uri: &str) -> Value {
    json!({
        "type": "http_proxy",
        "httpMethod": "ANY",
        "uri": uri,
        "payloadFormatVersion": "1.0"
    })
}

fn carried(route_key: &'static str, integration_uri: &'static str) -> ExpectRoute {
    ExpectRoute {
        route_key,
        identity: Identity::Carried,
        integration_uri: Some(integration_uri),
    }
}

fn added(route_key: &'static str, integration_uri: &'static str) -> ExpectRoute {
    ExpectRoute {
        route_key,
        identity: Identity::Added,
        integration_uri: Some(integration_uri),
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: the one path is declared again, so its route and the \
                   integration it targets keep their ids while the integration's upstream \
                   is rewritten",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "catalog", "version": "1.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/one")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "catalog", "version": "2.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/two")
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_routes: vec![carried("GET /items", "https://items.example.com/two")],
            expect_dropped: vec![],
        },
        Row {
            case: "an unchanged document put over the api again changes no id and no \
                   configuration",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "stable", "version": "1.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/one")
                    } },
                    "/health": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://health.example.com")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "stable", "version": "1.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/one")
                    } },
                    "/health": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://health.example.com")
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_routes: vec![
                carried("GET /items", "https://items.example.com/one"),
                carried("GET /health", "https://health.example.com"),
            ],
            expect_dropped: vec![],
        },
        Row {
            case: "one key is carried, one is added and two are dropped in a single put",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "mixed", "version": "1.0" },
                "paths": {
                    "/items": {
                        "get": {
                            "x-amazon-apigateway-integration": http_proxy("https://items.example.com/one")
                        },
                        "delete": {
                            "x-amazon-apigateway-integration": http_proxy("https://items.example.com/delete")
                        }
                    },
                    "/legacy": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://legacy.example.com")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "mixed", "version": "2.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/two")
                    } },
                    "/health": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://health.example.com")
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_routes: vec![
                carried("GET /items", "https://items.example.com/two"),
                added("GET /health", "https://health.example.com"),
            ],
            expect_dropped: vec!["DELETE /items", "GET /legacy"],
        },
        Row {
            case: "a carried declaration whose integration changes type and payload format \
                   version keeps the integration id it had",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "retyped", "version": "1.0" },
                "paths": {
                    "/orders": { "post": {
                        "x-amazon-apigateway-integration": http_proxy("https://orders.example.com")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "retyped", "version": "2.0" },
                "paths": {
                    "/orders": { "post": {
                        "x-amazon-apigateway-integration": {
                            "type": "aws_proxy",
                            "httpMethod": "POST",
                            "uri": LAMBDA_URI,
                            "payloadFormatVersion": "2.0"
                        }
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_routes: vec![carried("POST /orders", LAMBDA_URI)],
            expect_dropped: vec![],
        },
        Row {
            case: "the document's spelling changes from OpenAPI 3 to Swagger 2.0 and the \
                   catch-all key it declares is carried all the same",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "catch-all", "version": "1.0" },
                "paths": {
                    "/{proxy+}": { "x-amazon-apigateway-any-method": {
                        "x-amazon-apigateway-integration": http_proxy("https://catchall.example.com/one")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "swagger": "2.0",
                "info": { "title": "catch-all", "version": "2.0" },
                "paths": {
                    "/{proxy+}": { "x-amazon-apigateway-any-method": {
                        "x-amazon-apigateway-integration": http_proxy("https://catchall.example.com/two")
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_routes: vec![carried(
                "ANY /{proxy+}",
                "https://catchall.example.com/two",
            )],
            expect_dropped: vec![],
        },
        Row {
            case: "a declaration that names no integration is carried as the same route \
                   with no target",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "targetless", "version": "1.0" },
                "paths": { "/items": { "get": {} } }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "targetless", "version": "2.0" },
                "paths": { "/items": { "get": {} } }
            })
            .to_string(),
            query: "",
            expect_routes: vec![ExpectRoute {
                route_key: "GET /items",
                identity: Identity::Carried,
                integration_uri: None,
            }],
            expect_dropped: vec![],
        },
        Row {
            case: "read under Basepath=prepend the same bytes declare another key, so \
                   nothing is carried and the previous key is dropped",
            first: json!({
                "swagger": "2.0",
                "info": { "title": "prefixed", "version": "1.0" },
                "basePath": "/live",
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/one")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "swagger": "2.0",
                "info": { "title": "prefixed", "version": "2.0" },
                "basePath": "/live",
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/two")
                    } }
                }
            })
            .to_string(),
            query: "?basepath=prepend",
            expect_routes: vec![added("GET /live/items", "https://items.example.com/two")],
            expect_dropped: vec!["GET /items"],
        },
    ]
}

/// Every route of the api as `RouteKey -> (RouteId, the IntegrationId its Target names)`,
/// read back through `GetRoutes` — every page of it, so the answer is the whole route set.
async fn stored_identities(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
) -> BTreeMap<String, (String, Option<String>)> {
    let mut identities = BTreeMap::new();
    let mut next_token: Option<String> = None;
    loop {
        let uri = match next_token.as_deref() {
            Some(token) => format!("/v2/apis/{api_id}/routes?nextToken={token}"),
            None => format!("/v2/apis/{api_id}/routes"),
        };
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRoutes must answer 200, got {status} {body}"
        );
        for route in body["items"].as_array().cloned().unwrap_or_default() {
            let route_id = as_str(&route, "routeId");
            assert_minted_id(case, "routeId", &route_id);
            let integration_id = route
                .get("target")
                .and_then(Value::as_str)
                .map(|target| {
                    let integration_id = target
                        .strip_prefix("integrations/")
                        .unwrap_or_else(|| {
                            panic!(
                                "case {case:?}: expected a target of the form \
                                 integrations/<IntegrationId>, got {target:?}"
                            )
                        })
                        .to_string();
                    assert_minted_id(case, "integrationId", &integration_id);
                    integration_id
                });
            identities.insert(as_str(&route, "routeKey"), (route_id, integration_id));
        }
        next_token = body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            return identities;
        }
    }
}

/// Address one route directly, the way a caller that captured its `RouteId` does.
async fn read_route(router: &MultiServiceRouter, api_id: &str, route_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v2/apis/{api_id}/routes/{route_id}"),
        None,
    )
    .await
}

/// Address one integration directly, the way a caller that captured its `IntegrationId`
/// does.
async fn read_integration(
    router: &MultiServiceRouter,
    api_id: &str,
    integration_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        None,
    )
    .await
}

/// Two routes of one api may share a target (a caller can point one route at another's
/// integration through `UpdateRoute`), so the carry has to claim each integration for ONE
/// declaration: the api holds as many integrations as the document declares, and neither
/// declaration's configuration is written over the other's.
#[tokio::test]
async fn reimport_gives_a_second_claimant_of_one_integration_an_integration_of_its_own() {
    let case = "two routes share one integration before the put, and both keys are \
                declared again";
    let emulator = emulator();
    let definition = json!({
        "openapi": "3.0.1",
        "info": { "title": "shared", "version": "1.0" },
        "paths": {
            "/items": { "get": {
                "x-amazon-apigateway-integration": http_proxy("https://items.example.com")
            } },
            "/health": { "get": {
                "x-amazon-apigateway-integration": http_proxy("https://health.example.com")
            } }
        }
    })
    .to_string();

    let (status, imported) = import(&emulator, "", &definition).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ImportApi must answer 200, got {status} {imported}"
    );
    let api_id = as_str(&imported, "apiId");
    let before = stored_identities(&emulator, case, &api_id).await;
    let (items_route, shared_integration) = before
        .get("GET /items")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imported GET /items, got {before:?}"));
    let shared_integration = shared_integration
        .unwrap_or_else(|| panic!("case {case:?}: GET /items must front an integration"));
    let (health_route, health_integration) = before
        .get("GET /health")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an imported GET /health, got {before:?}"));

    // Point the second route at the first route's integration, so one integration is the
    // target of two routes and the second route's own integration is left orphaned.
    let (status, patched) = call(
        &emulator,
        "PATCH",
        &format!("/v2/apis/{api_id}/routes/{health_route}"),
        Some(json!({ "target": format!("integrations/{shared_integration}") })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: UpdateRoute must answer 201, got {status} {patched}"
    );

    let (status, reimported) = reimport(&emulator, &api_id, "", &definition).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReimportApi must answer 200, got {status} {reimported}"
    );
    let after = stored_identities(&emulator, case, &api_id).await;

    for (key, route_id) in [("GET /items", &items_route), ("GET /health", &health_route)] {
        let Some((after_route_id, _)) = after.get(key) else {
            panic!("case {case:?}: expected a route under {key:?}, got {after:?}");
        };
        assert_eq!(
            after_route_id, route_id,
            "case {case:?}: {key:?} must keep routeId {route_id:?}, got {after_route_id:?}"
        );
    }
    let items_target = after["GET /items"].1.clone();
    let health_target = after["GET /health"].1.clone();
    assert_ne!(
        items_target, health_target,
        "case {case:?}: each declaration must front an integration of its own, got \
         {items_target:?} for both"
    );
    assert!(
        items_target.as_deref() == Some(shared_integration.as_str())
            || health_target.as_deref() == Some(shared_integration.as_str()),
        "case {case:?}: one declaration must keep the shared integration \
         {shared_integration:?}, got {items_target:?} and {health_target:?}"
    );
    assert!(
        health_integration.is_none_or(|orphaned| {
            items_target.as_deref() != Some(orphaned.as_str())
                && health_target.as_deref() != Some(orphaned.as_str())
        }),
        "case {case:?}: the integration no route targeted is dropped, got {items_target:?} \
         and {health_target:?}"
    );

    let integrations = stored_integrations(&emulator, &api_id).await;
    assert_eq!(
        integrations.len(),
        2,
        "case {case:?}: the api must hold the two integrations the document declares, got {:?}",
        integrations.keys().collect::<Vec<_>>()
    );
    for (key, expected_uri) in [
        ("GET /items", "https://items.example.com"),
        ("GET /health", "https://health.example.com"),
    ] {
        let integration_id = after[key]
            .1
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: {key:?} must front an integration"));
        let (status, integration) = read_integration(&emulator, &api_id, &integration_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIntegration {integration_id:?} must answer 200, got \
             {status} {integration}"
        );
        assert_eq!(
            as_str(&integration, "integrationUri"),
            expected_uri,
            "case {case:?}: {key:?} must front {expected_uri:?}, got {integration}"
        );
    }
}

/// The table case: every declaration the second document declares again keeps the ids the
/// first document's import gave it.
#[tokio::test]
async fn reimport_keeps_the_identity_of_every_declaration_it_declares_again() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let (status, imported) = import(&emulator, "", &row.first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ImportApi must answer 200, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let before = stored_identities(&emulator, case, &api_id).await;

        let (status, reimported) = reimport(&emulator, &api_id, row.query, &row.second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReimportApi must answer 200, got {status} {reimported}"
        );
        let after = stored_identities(&emulator, case, &api_id).await;
        let integrations = stored_integrations(&emulator, &api_id).await;

        assert_eq!(
            after.len(),
            row.expect_routes.len(),
            "case {case:?}: expected {} route(s) after the put, got {after:?}",
            row.expect_routes.len()
        );

        for expected in &row.expect_routes {
            let key = expected.route_key;
            let Some((route_id, integration_id)) = after.get(key) else {
                panic!("case {case:?}: expected a route under {key:?}, got {after:?}");
            };
            let previous = before.get(key);
            match expected.identity {
                Identity::Carried => {
                    let (previous_route_id, previous_integration_id) = previous.unwrap_or_else(
                        || panic!("case {case:?}: {key:?} must have been imported first, got {before:?}"),
                    );
                    assert_eq!(
                        route_id, previous_route_id,
                        "case {case:?}: {key:?} must keep routeId {previous_route_id:?}, \
                         got {route_id:?}"
                    );
                    assert_eq!(
                        integration_id, previous_integration_id,
                        "case {case:?}: {key:?} must keep its target integration \
                         {previous_integration_id:?}, got {integration_id:?}"
                    );
                }
                Identity::Added => {
                    assert!(
                        previous.is_none(),
                        "case {case:?}: {key:?} is declared only by the second document, \
                         yet the api already held {previous:?}"
                    );
                    let minted_before: Vec<&(String, Option<String>)> = before.values().collect();
                    assert!(
                        !before.values().any(|(before_id, _)| before_id == route_id),
                        "case {case:?}: {key:?} must be given a routeId of its own, yet \
                         {route_id:?} is one of {minted_before:?}"
                    );
                    assert!(
                        integration_id.is_none()
                            || !before
                                .values()
                                .any(|(_, before_id)| before_id == integration_id),
                        "case {case:?}: {key:?} must be given an integrationId of its own, \
                         yet {integration_id:?} is one of {minted_before:?}"
                    );
                }
            }

            // The id a caller captured still addresses the route that key names, and the
            // integration behind it carries the configuration the SECOND document declared.
            let (status, route) = read_route(&emulator, &api_id, route_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetRoute {route_id:?} must answer 200, got {status} {route}"
            );
            assert_eq!(
                as_str(&route, "routeKey"),
                key,
                "case {case:?}: GetRoute {route_id:?} must report routeKey {key:?}, got {route}"
            );
            match (expected.integration_uri, integration_id.as_deref()) {
                (Some(expected_uri), Some(integration_id)) => {
                    let (status, integration) =
                        read_integration(&emulator, &api_id, integration_id).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: GetIntegration {integration_id:?} must answer 200, \
                         got {status} {integration}"
                    );
                    assert_eq!(
                        as_str(&integration, "integrationUri"),
                        expected_uri,
                        "case {case:?}: {key:?} must front {expected_uri:?}, got {integration}"
                    );
                    assert!(
                        integrations.contains_key(integration_id),
                        "case {case:?}: GetIntegrations must list {integration_id:?}, \
                         got {:?}",
                        integrations.keys().collect::<Vec<_>>()
                    );
                }
                (None, None) => {}
                (expected_uri, actual) => panic!(
                    "case {case:?}: {key:?} expected integration {expected_uri:?}, \
                     got target integration {actual:?}"
                ),
            }
        }

        // A key the document no longer declares is gone, and so is the integration that
        // key alone was wired to — its id resolves to nothing rather than to a resource.
        for dropped in &row.expect_dropped {
            assert!(
                !after.contains_key(*dropped),
                "case {case:?}: {dropped:?} is declared by neither document, got {after:?}"
            );
            let Some((route_id, integration_id)) = before.get(*dropped) else {
                panic!("case {case:?}: {dropped:?} must have been imported first, got {before:?}");
            };
            let (status, body) = read_route(&emulator, &api_id, route_id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: GetRoute {route_id:?} must answer 404 after the put, \
                 got {status} {body}"
            );
            if let Some(integration_id) = integration_id {
                let (status, body) = read_integration(&emulator, &api_id, integration_id).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: GetIntegration {integration_id:?} must answer 404 after \
                     the put, got {status} {body}"
                );
                assert!(
                    !integrations.contains_key(integration_id),
                    "case {case:?}: GetIntegrations must not list {integration_id:?}, got {:?}",
                    integrations.keys().collect::<Vec<_>>()
                );
            }
        }

        assert_eq!(
            integrations.len(),
            row.expect_routes
                .iter()
                .filter(|expected| expected.integration_uri.is_some())
                .count(),
            "case {case:?}: the api must hold exactly the integrations the document \
             declares, got {:?}",
            integrations.keys().collect::<Vec<_>>()
        );
    }
}

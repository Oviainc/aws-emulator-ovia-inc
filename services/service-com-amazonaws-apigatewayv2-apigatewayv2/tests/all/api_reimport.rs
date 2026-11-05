// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `ReimportApi`, and for what a reimported
//! definition changes about the requests the api's deployed stage answers.
//!
//! The rules these cases assert:
//!
//! * **`ReimportApi` PUTS the definition over the api `ApiId` names** (`PUT
//!   /v2/apis/{ApiId}`, success 201): the api keeps its identity — its `ApiId`, its
//!   `ApiEndpoint`, its `CreatedDate`, its `ProtocolType` and the stages it holds — and
//!   everything a definition describes is REPLACED by what the new one describes: the name,
//!   description and version from `info`, the CORS extension, and exactly the routes and
//!   integrations the document's `paths` declare, each integration carrying the
//!   `payloadFormatVersion` its own `x-amazon-apigateway-integration` extension names.
//! * **That replacement changes which route of the api answers.** On the api's own invoke
//!   endpoint, a request matching a route key only the PREVIOUS document declared is
//!   `404 {"message":"Not Found"}` afterwards, and one matching the new document's key
//!   reaches that route's own integration.
//! * **A reimport this service refuses leaves the api exactly as it was**: an unknown
//!   `ApiId` is `NotFoundException` 404, a definition put over an api of the other protocol
//!   type is `BadRequestException` 400 ("Supported only for HTTP APIs", `ReimportApi$Body`),
//!   an unreadable body is 400, and `FailOnWarnings` turns a warning into that same 400 —
//!   after each, `GetRoutes` still reports the routes of the document the api carried.
//!
//! What an admitted request is answered WITH is the integration's answer, and the two
//! integration types an imported HTTP api can name both address a backend outside this
//! process: an `HTTP_PROXY` upstream is a URI the caller can reach, so the caller is sent
//! there (`307` with the stored `IntegrationUri` in `Location`), while an `AWS_PROXY`
//! integration invokes a Lambda function this emulator has no runtime for, so it is
//! answered the way API Gateway answers an integration that produced no response (`500`).
//! Either answer names the route that served the request — each route of these cases fronts
//! its OWN upstream — which is how "which route answers" is observed at all.
//!
//! The rows span what the rules generalise over: the document's version (OpenAPI 3 and
//! Swagger 2.0), what changes between the two documents (the path, the method, the
//! integration type, the payload format version, the number of routes, the CORS
//! extension), the form of the route key that goes away (a literal path, a greedy
//! catch-all) and the `Basepath` the reimport is read under. The recorded case that filed
//! the issue — an import, then a reimport of a changed body, then a request to the api's
//! invoke URL — is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_import::{import, stored_integrations, stored_routes};
use crate::api_lifecycle::{as_str, call, carries_nothing, emulator, error_code};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The status and sentence an `execute-api` endpoint answers a request matching no route of
/// the api with.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The status the data plane answers a route fronting an `HTTP_PROXY` upstream with: the
/// caller is sent to the upstream rather than the gateway connecting to it.
const TEMPORARY_REDIRECT: u16 = 307;

/// The status the data plane answers a route whose integration produced no response with —
/// an `AWS_PROXY` integration, whose Lambda function this emulator cannot run.
const INTEGRATION_REACHED: u16 = 500;

/// The error code the front door answers when NO service claims a request: the invoke URL
/// must never be answered with it.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// The `PayloadFormatVersion` an imported integration carries when its extension names none
/// ("For all other integrations, 1.0 is the only supported value",
/// `CreateIntegration$PayloadFormatVersion`).
const DEFAULT_PAYLOAD_FORMAT_VERSION: &str = "1.0";

/// A Lambda function ARN in the form an `aws_proxy` integration extension names one.
const LAMBDA_URI: &str = "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/arn:aws:lambda:eu-west-1:123456789012:function:orders/invocations";

/// What one request to the api's invoke endpoint must be answered with.
struct Expect {
    status: u16,
    /// The upstream the caller must be sent to, for a route fronting an `HTTP_PROXY`
    /// integration.
    location: Option<&'static str>,
}

/// One row: the definition the api is imported from, the one reimported over it, and what
/// each one serves.
struct Row {
    case: &'static str,
    first: String,
    second: String,
    /// The query the reimport carries (`""`, or `"?basepath=prepend"`).
    query: &'static str,
    expect_name: &'static str,
    expect_description: Option<&'static str>,
    expect_version: Option<&'static str>,
    /// The `CorsConfiguration` the api must report after the reimport — `None` for a
    /// document that carries no CORS extension, which the PUT leaves the api without.
    expect_cors: Option<Value>,
    /// Every route the reimport must leave on the api: its key, the
    /// `PayloadFormatVersion` of the integration it fronts, and that integration's
    /// `IntegrationUri`.
    expect_routes: Vec<(&'static str, &'static str, &'static str)>,
    /// A request the FIRST document served, as `(method, path)`.
    gone: (&'static str, &'static str),
    /// What that request was answered with BEFORE the reimport.
    gone_before: Expect,
    /// A request only the SECOND document serves.
    serves: (&'static str, &'static str),
    /// What that request must be answered with after the reimport.
    serves_after: Expect,
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

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: the api's one path moves, so the old one stops \
                   answering and the new one starts",
            first: json!({
                "openapi": "3.0.1",
                "info": {
                    "title": "catalog",
                    "description": "the first document",
                    "version": "1.0"
                },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/items")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": {
                    "title": "catalog",
                    "description": "the second document",
                    "version": "2.0"
                },
                "paths": {
                    "/products/{id}": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://products.example.com/products")
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_name: "catalog",
            expect_description: Some("the second document"),
            expect_version: Some("2.0"),
            expect_cors: None,
            expect_routes: vec![(
                "GET /products/{id}",
                "1.0",
                "https://products.example.com/products",
            )],
            gone: ("GET", "/items"),
            gone_before: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://items.example.com/items"),
            },
            serves: ("GET", "/products/7"),
            serves_after: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://products.example.com/products"),
            },
        },
        Row {
            case: "the method and the integration behind it change: an aws_proxy at \
                   payload format 2.0 replaces an http_proxy at 1.0",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "orders", "version": "1.0" },
                "paths": {
                    "/orders": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://orders.example.com/orders")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "orders", "version": "1.0" },
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
            expect_name: "orders",
            expect_description: None,
            expect_version: Some("1.0"),
            expect_cors: None,
            expect_routes: vec![("POST /orders", "2.0", LAMBDA_URI)],
            gone: ("GET", "/orders"),
            gone_before: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://orders.example.com/orders"),
            },
            serves: ("POST", "/orders"),
            serves_after: Expect {
                status: INTEGRATION_REACHED,
                location: None,
            },
        },
        Row {
            case: "a Swagger 2.0 catch-all with a CORS extension is replaced by one \
                   literal OpenAPI 3 route and no CORS",
            first: json!({
                "swagger": "2.0",
                "info": { "title": "catch-all", "version": "0.9" },
                "x-amazon-apigateway-cors": {
                    "allowOrigins": ["https://example.com"],
                    "allowMethods": ["GET"]
                },
                "paths": {
                    "/{proxy+}": { "x-amazon-apigateway-any-method": {
                        "x-amazon-apigateway-integration": http_proxy("https://catchall.example.com")
                    } }
                }
            })
            .to_string(),
            second: json!({
                "openapi": "3.0.1",
                "info": { "title": "health-only" },
                "paths": {
                    "/health": { "get": {
                        "x-amazon-apigateway-integration": {
                            "type": "http_proxy",
                            "uri": "https://health.example.com/health"
                        }
                    } }
                }
            })
            .to_string(),
            query: "",
            expect_name: "health-only",
            expect_description: None,
            expect_version: None,
            expect_cors: None,
            expect_routes: vec![(
                "GET /health",
                DEFAULT_PAYLOAD_FORMAT_VERSION,
                "https://health.example.com/health",
            )],
            gone: ("GET", "/anything/at/all"),
            gone_before: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://catchall.example.com"),
            },
            serves: ("GET", "/health"),
            serves_after: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://health.example.com/health"),
            },
        },
        Row {
            case: "the reimport is read under Basepath=prepend, so the new routes carry \
                   the document's base path",
            first: json!({
                "openapi": "3.0.1",
                "info": { "title": "prefixed", "version": "1.0" },
                "paths": {
                    "/items": { "get": {
                        "x-amazon-apigateway-integration": http_proxy("https://items.example.com/items")
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
                        "x-amazon-apigateway-integration": http_proxy("https://live.example.com/items")
                    } }
                }
            })
            .to_string(),
            query: "?basepath=prepend",
            expect_name: "prefixed",
            expect_description: None,
            expect_version: Some("2.0"),
            expect_cors: None,
            expect_routes: vec![(
                "GET /live/items",
                "1.0",
                "https://live.example.com/items",
            )],
            gone: ("GET", "/items"),
            gone_before: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://items.example.com/items"),
            },
            serves: ("GET", "/live/items"),
            serves_after: Expect {
                status: TEMPORARY_REDIRECT,
                location: Some("https://live.example.com/items"),
            },
        },
    ]
}

/// Call `ReimportApi`: `PUT /v2/apis/{ApiId}` with the definition in the `Body` member.
pub(crate) async fn reimport(
    router: &MultiServiceRouter,
    api_id: &str,
    query: &str,
    definition: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/v2/apis/{api_id}{query}"),
        Some(json!({ "body": definition })),
    )
    .await
}

/// The host of an api's own `ApiEndpoint` — the address a client of a deployed stage calls.
fn endpoint_host(api: &Value) -> String {
    let endpoint = as_str(api, "apiEndpoint");
    endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string()
}

/// Release the api's `$default` stage — "the stage that serves an api's root" — so the
/// routes the document declared are reachable on the invoke endpoint.
async fn deploy(router: &MultiServiceRouter, case: &str, api_id: &str) -> String {
    let (status, stage) = call(
        router,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    let deployment_id = stage
        .get("deploymentId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !deployment_id.is_empty(),
        "case {case:?}: an auto-deploying stage must carry a deployment, got {stage}"
    );
    deployment_id
}

/// Send one request to the api's own invoke endpoint, with no AWS credential — which is
/// what a client of a route whose `AuthorizationType` is the default `NONE` sends.
async fn invoke(
    router: &MultiServiceRouter,
    host: &str,
    (method, path): (&str, &str),
) -> Answer {
    send(router, host, method, path, &[]).await
}

/// Assert one invoke was served by this service's data plane and answered as expected.
fn answered(case: &str, step: &str, answer: &Answer, expected: &Expect) {
    assert_ne!(
        answer.error_type.as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: {step} must be served by this service, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status, expected.status,
        "case {case:?}: {step} expected status {}, got {}",
        expected.status,
        answer.summary()
    );
    assert_eq!(
        answer.location.as_deref(),
        expected.location,
        "case {case:?}: {step} expected the caller to be sent to {:?}, got {:?}",
        expected.location,
        answer.location
    );
}

/// Every route of the api as `RouteKey -> (PayloadFormatVersion, IntegrationUri)`, read
/// back through `GetRoutes` and `GetIntegrations` — so what is asserted went through the
/// store.
async fn served_routes(
    router: &MultiServiceRouter,
    api_id: &str,
) -> Vec<(String, String, String)> {
    let integrations = stored_integrations(router, api_id).await;
    stored_routes(router, api_id)
        .await
        .into_iter()
        .map(|(route_key, target)| {
            let integration = target
                .as_deref()
                .and_then(|target| target.strip_prefix("integrations/"))
                .and_then(|id| integrations.get(id));
            let payload_format_version = integration
                .map(|integration| as_str(integration, "payloadFormatVersion"))
                .unwrap_or_default();
            let uri = integration
                .map(|integration| as_str(integration, "integrationUri"))
                .unwrap_or_default();
            (route_key, payload_format_version, uri)
        })
        .collect()
}

#[tokio::test]
async fn a_reimported_definition_replaces_the_api_and_changes_which_route_answers() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();

        let (status, imported) = import(&emulator, "", &row.first).await;
        assert_eq!(
            status, 201,
            "case {case:?}: ImportApi must answer 201, got {status} {imported}"
        );
        let api_id = as_str(&imported, "apiId");
        let host = endpoint_host(&imported);
        let created_date = imported.get("createdDate").cloned();
        let deployment_id = deploy(&emulator, case, &api_id).await;

        // The first document's route serves the request on the invoke endpoint, so the
        // 404 asserted after the reimport is the REPLACEMENT and not an api that never
        // answered.
        let before = invoke(&emulator, &host, row.gone).await;
        answered(case, "the first document's route", &before, &row.gone_before);

        let (status, reimported) = reimport(&emulator, &api_id, row.query, &row.second).await;
        assert_eq!(
            status, 201,
            "case {case:?}: ReimportApi must answer 201, got {status} {reimported}"
        );

        // The api kept its identity: the same api was PUT over, not a new one minted.
        assert_eq!(
            as_str(&reimported, "apiId"),
            api_id,
            "case {case:?}: ReimportApi must answer for the api it was aimed at, got {reimported}"
        );
        assert_eq!(
            as_str(&reimported, "apiEndpoint"),
            as_str(&imported, "apiEndpoint"),
            "case {case:?}: the api's endpoint must not change, got {reimported}"
        );
        assert_eq!(
            reimported.get("createdDate").cloned(),
            created_date,
            "case {case:?}: the api's CreatedDate must not change, got {reimported}"
        );
        assert_eq!(
            as_str(&reimported, "protocolType"),
            "HTTP",
            "case {case:?}: an imported api speaks HTTP, got {reimported}"
        );

        // …and carries what the NEW document describes, read back through GetApi rather
        // than taken from the answer.
        let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApi must answer 200, got {status} {fetched}"
        );
        for (step, body) in [("ReimportApi", &reimported), ("GetApi", &fetched)] {
            assert_eq!(
                as_str(body, "name"),
                row.expect_name,
                "case {case:?}: {step} must report the new document's name, got {body}"
            );
            assert_eq!(
                body.get("description").and_then(Value::as_str),
                row.expect_description,
                "case {case:?}: {step} must report the new document's description, got {body}"
            );
            assert_eq!(
                body.get("version").and_then(Value::as_str),
                row.expect_version,
                "case {case:?}: {step} must report the new document's version, got {body}"
            );
            match &row.expect_cors {
                Some(cors) => assert_eq!(
                    body.get("corsConfiguration"),
                    Some(cors),
                    "case {case:?}: {step} must report the new document's CORS, got {body}"
                ),
                None => assert!(
                    carries_nothing(body, "corsConfiguration"),
                    "case {case:?}: {step} must carry no CORS for a document with no CORS \
                     extension, got {body}"
                ),
            }
        }

        // The routes and integrations are exactly the new document's.
        let expected: Vec<(String, String, String)> = row
            .expect_routes
            .iter()
            .map(|(key, version, uri)| {
                ((*key).to_string(), (*version).to_string(), (*uri).to_string())
            })
            .collect();
        let stored = served_routes(&emulator, &api_id).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the new document's routes {expected:?}, got {stored:?}"
        );

        // The stage the api was deployed to is still there — and because it auto-deploys,
        // the reimport released the api's NEW routes and integrations to it: "Specifies
        // whether updates to an API automatically trigger a new deployment"
        // (`Stage$AutoDeploy`), so its DeploymentId moved onto the deployment that froze
        // the new configuration, which is what serves the new routes below.
        let (status, stage) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/$default"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stage must survive the reimport, got {status} {stage}"
        );
        let redeployed = as_str(&stage, "deploymentId");
        assert_ne!(
            redeployed, deployment_id,
            "case {case:?}: the auto-deploying stage must be released a new deployment of \
             the reimported api, got {stage}"
        );
        let (status, deployment) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments/{redeployed}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the deployment the stage now serves must be readable, got \
             {status} {deployment}"
        );
        assert_eq!(
            deployment.get("autoDeployed").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: a deployment the service released by itself is autoDeployed, \
             got {deployment}"
        );

        // And the data plane answers what the NEW document declares: the route key only
        // the previous document carried matches nothing, and the new one reaches its own
        // integration.
        let gone = invoke(&emulator, &host, row.gone).await;
        answered(
            case,
            "the previous document's route",
            &gone,
            &Expect {
                status: NOT_FOUND,
                location: None,
            },
        );
        assert_eq!(
            message(&gone),
            NOT_FOUND_MESSAGE,
            "case {case:?}: a request matching no route is {NOT_FOUND_MESSAGE:?}, got {}",
            gone.summary()
        );
        let serves = invoke(&emulator, &host, row.serves).await;
        answered(case, "the new document's route", &serves, &row.serves_after);
    }
}

#[tokio::test]
async fn a_refused_reimport_leaves_the_api_exactly_as_it_was() {
    let emulator = emulator();

    // One api, imported from a definition whose route is the one every refusal below must
    // leave in place.
    let definition = json!({
        "openapi": "3.0.1",
        "info": { "title": "untouched", "version": "1.0" },
        "paths": {
            "/items": { "get": {
                "x-amazon-apigateway-integration": http_proxy("https://items.example.com/items")
            } }
        }
    })
    .to_string();
    let (status, imported) = import(&emulator, "", &definition).await;
    assert_eq!(
        status, 201,
        "ImportApi must answer 201, got {status} {imported}"
    );
    let api_id = as_str(&imported, "apiId");
    let before = served_routes(&emulator, &api_id).await;

    // A WebSocket api, for the protocol gate: "The OpenAPI definition. Supported only for
    // HTTP APIs" (`ReimportApi$Body`).
    let (status, socket) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": "socket-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateApi must answer 201, got {status} {socket}");
    let socket_id = as_str(&socket, "apiId");

    // A definition whose operation declares no integration: the import warns about it, and
    // `FailOnWarnings` turns that warning into a refusal.
    let warns = json!({
        "openapi": "3.0.1",
        "info": { "title": "no-integration", "version": "1.0" },
        "paths": { "/items": { "get": { "summary": "list the items" } } }
    })
    .to_string();

    let cases: Vec<(&str, String, &str, String, u16, &str)> = vec![
        (
            "an ApiId no api of the store holds",
            format!("{api_id}-not-an-api"),
            "",
            definition.clone(),
            404,
            "NotFoundException",
        ),
        (
            "an api whose protocol type is WEBSOCKET",
            socket_id.clone(),
            "",
            definition.clone(),
            400,
            "BadRequestException",
        ),
        (
            "a body that is not an OpenAPI definition at all",
            api_id.clone(),
            "",
            "not a definition".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a definition in the YAML spelling this service does not read",
            api_id.clone(),
            "",
            "openapi: 3.0.1\ninfo:\n  title: yaml\n".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "FailOnWarnings over a definition the import warns about",
            api_id.clone(),
            "?failOnWarnings=true",
            warns,
            400,
            "BadRequestException",
        ),
    ];

    for (case, target, query, body, expect_status, expect_code) in cases {
        let (status, answer) = reimport(&emulator, &target, query, &body).await;
        assert_eq!(
            status, expect_status,
            "case {case:?}: expected {expect_status}, got {status} {answer}"
        );
        assert_eq!(
            error_code(&answer),
            expect_code,
            "case {case:?}: expected {expect_code}, got {status} {answer}"
        );
        let after = served_routes(&emulator, &api_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused reimport must leave the api's routes as they were, \
             expected {before:?}, got {after:?}"
        );
    }
}

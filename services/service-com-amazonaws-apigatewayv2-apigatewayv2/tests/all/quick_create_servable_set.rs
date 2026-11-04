// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for QUICK CREATE: the set of resources a `CreateApi` carrying
//! a `Target` produces alongside the api.
//!
//! The rule these cases assert, one level above the issue's reproduction
//! (`create-api --target <lambda-arn>`, then a stage, then a deployment):
//!
//! > A `CreateApi` that carries a `Target` produces, with the api and in the store every
//! > later reader uses, the whole set the member documents — "an integration, a default
//! > catch-all route, and a default stage which is configured to automatically deploy
//! > changes" (`CreateApi$Target`). The integration carries the target as its
//! > `IntegrationUri` and the type the target's FORM selects ("For HTTP integrations,
//! > specify a fully qualified URL. For Lambda integrations, specify a function ARN. The
//! > type of the integration will be HTTP_PROXY or AWS_PROXY, respectively"); the route
//! > carries the request's `RouteKey`, or `$default` when it names none, and targets
//! > that integration as `integrations/{IntegrationId}`; the stage is named `$default`
//! > with `AutoDeploy` on and serves a deployment the SERVICE released
//! > (`Deployment$AutoDeployed`, status `DEPLOYED`). Each of the three is the resource
//! > API Gateway manages — `Integration$ApiGatewayManaged`, `Route$ApiGatewayManaged`,
//! > `Stage$ApiGatewayManaged` ("If you created an API using quick create, …") — and a
//! > `CreateApi` carrying NO target produces none of them.
//!
//! The rows vary what the rule generalises over: the form of the target (a Lambda
//! function ARN, an `https` URL, an `http` URL, a Lambda ARN carrying a version
//! qualifier), whether the request names a `RouteKey` and its form, and whether it
//! carries the quick-create `CredentialsArn`. The issue's reproduction — an HTTP api
//! quick-created against a Lambda function ARN — is ONE row.
//!
//! What this suite adds over its neighbours: `api_lifecycle` spans the `CreateApi`
//! request surface and `http_api_servable_stage` drives the operations a client calls
//! ONE BY ONE to make an api servable; neither asserts what quick create produces BY
//! ITSELF, which is the whole point of the member — one call and the api is callable.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, error_code,
};

/// The `StageName` of the stage quick create mints: "the $default stage"
/// (`Stage$ApiGatewayManaged`).
const DEFAULT_STAGE_NAME: &str = "$default";

/// The catch-all `RouteKey` quick create uses when the request names none: "If you
/// don't specify a routeKey, a default route of $default is created"
/// (`CreateApi$RouteKey`).
const DEFAULT_ROUTE_KEY: &str = "$default";

/// The form a route's target takes when it is wired to an integration of its api.
const INTEGRATION_TARGET_PREFIX: &str = "integrations/";

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

/// One row: the `CreateApi` request, and what quick create must have produced from its
/// `Target`.
struct Row {
    case: &'static str,
    /// The `CreateApi` request.
    request: Value,
    /// The `IntegrationUri` the minted integration must carry: the row's `Target`.
    target: &'static str,
    /// The `IntegrationType` the target's form selects.
    integration_type: &'static str,
    /// The `PayloadFormatVersion` that type supports — 1.0 or 2.0 for a Lambda proxy,
    /// 1.0 the only value for every other type (`Integration$PayloadFormatVersion`).
    payload_format_version: &'static str,
    /// The `RouteKey` the minted route must carry.
    route_key: &'static str,
    /// The `CredentialsArn` the minted integration must carry, if the request named one.
    credentials_arn: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the issue's reproduction: an HTTP api quick-created against a Lambda \
                   function ARN",
            request: json!({
                "name": "t",
                "protocolType": "HTTP",
                "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
            }),
            target: "arn:aws:lambda:eu-west-1:000000000000:function:handler",
            integration_type: "AWS_PROXY",
            payload_format_version: "2.0",
            route_key: DEFAULT_ROUTE_KEY,
            credentials_arn: None,
        },
        Row {
            case: "a fully qualified https URL, so an HTTP proxy upstream",
            request: json!({
                "name": "upstream-api",
                "protocolType": "HTTP",
                "target": "https://example.com/upstream"
            }),
            target: "https://example.com/upstream",
            integration_type: "HTTP_PROXY",
            payload_format_version: "1.0",
            route_key: DEFAULT_ROUTE_KEY,
            credentials_arn: None,
        },
        Row {
            case: "a plain http URL with an explicit RouteKey, which replaces the \
                   catch-all",
            request: json!({
                "name": "keyed-api",
                "protocolType": "HTTP",
                "target": "http://example.com/legacy",
                "routeKey": "GET /items"
            }),
            target: "http://example.com/legacy",
            integration_type: "HTTP_PROXY",
            payload_format_version: "1.0",
            route_key: "GET /items",
            credentials_arn: None,
        },
        Row {
            case: "a Lambda ARN with a version qualifier, and the quick-create \
                   CredentialsArn the integration needs",
            request: json!({
                "name": "qualified-api",
                "protocolType": "HTTP",
                "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler:3",
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw-invoke",
                "routeKey": "ANY /{proxy+}"
            }),
            target: "arn:aws:lambda:eu-west-1:000000000000:function:handler:3",
            integration_type: "AWS_PROXY",
            payload_format_version: "2.0",
            route_key: "ANY /{proxy+}",
            credentials_arn: Some("arn:aws:iam::000000000000:role/apigw-invoke"),
        },
    ]
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

/// The one route of an api, read back through `GetRoutes`.
async fn only_route(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> Value {
    let (status, routes) = call(emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetRoutes, got {status} {routes}"
    );
    let items = routes["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        items.len(),
        1,
        "case {case:?}: quick create must mint exactly one route, got {routes}"
    );
    items[0].clone()
}

/// Read one resource back and assert it answered 200.
async fn get(emulator: &MultiServiceRouter, case: &str, step: &str, uri: &str) -> Value {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {step} at {uri}, got {status} {body}"
    );
    body
}

#[tokio::test]
async fn quick_create_produces_the_integration_route_and_default_stage() {
    for row in rows() {
        let case = row.case;
        // One emulator per row: each row's api is the only one in its store, so a
        // resource read back can only be the one quick create produced.
        let emulator = emulator();
        let api_id = create_api(&emulator, case, &row.request).await;

        // The route, wired to the integration quick create minted.
        let route = only_route(&emulator, case, &api_id).await;
        assert_eq!(
            as_str(&route, "routeKey"),
            row.route_key,
            "case {case:?}: expected the quick-create route to carry RouteKey {:?}, got {route}",
            row.route_key
        );
        assert_eq!(
            route.get("apiGatewayManaged"),
            Some(&json!(true)),
            "case {case:?}: the route quick create mints with the api is \
             API-Gateway-managed whatever RouteKey it carries, got {route}"
        );
        let target = as_str(&route, "target");
        let integration_id = target
            .strip_prefix(INTEGRATION_TARGET_PREFIX)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a target of the form \
                     {INTEGRATION_TARGET_PREFIX}{{IntegrationId}}, got {target:?}"
                )
            })
            .to_string();
        assert_minted_id(case, "IntegrationId", &integration_id);

        // The integration: the target is ITS endpoint, and its type follows the form
        // the target takes.
        let integration = get(
            &emulator,
            case,
            "GetIntegration",
            &format!("/v2/apis/{api_id}/integrations/{integration_id}"),
        )
        .await;
        assert_eq!(
            as_str(&integration, "integrationUri"),
            row.target,
            "case {case:?}: expected the integration to carry the Target as its \
             IntegrationUri, got {integration}"
        );
        assert_eq!(
            as_str(&integration, "integrationType"),
            row.integration_type,
            "case {case:?}: expected IntegrationType {:?} for target {:?}, got {integration}",
            row.integration_type,
            row.target
        );
        assert_eq!(
            as_str(&integration, "payloadFormatVersion"),
            row.payload_format_version,
            "case {case:?}: expected PayloadFormatVersion {:?}, got {integration}",
            row.payload_format_version
        );
        assert_eq!(
            integration.get("credentialsArn"),
            row.credentials_arn.map(|arn| json!(arn)).as_ref(),
            "case {case:?}: expected CredentialsArn {:?} on the quick-create \
             integration, got {integration}",
            row.credentials_arn
        );
        assert_eq!(
            integration.get("apiGatewayManaged"),
            Some(&json!(true)),
            "case {case:?}: the integration quick create mints is API-Gateway-managed, \
             got {integration}"
        );

        // The default stage, which is what makes the api callable: it auto-deploys, and
        // it serves a deployment the service released rather than one a caller asked
        // for.
        let stage = get(
            &emulator,
            case,
            "GetStage",
            &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
        )
        .await;
        assert_eq!(
            as_str(&stage, "stageName"),
            DEFAULT_STAGE_NAME,
            "case {case:?}: quick create's stage is the $default one, got {stage}"
        );
        assert_eq!(
            stage.get("autoDeploy"),
            Some(&json!(true)),
            "case {case:?}: the quick-create stage is configured to automatically \
             deploy changes, got {stage}"
        );
        assert_eq!(
            stage.get("apiGatewayManaged"),
            Some(&json!(true)),
            "case {case:?}: the $default stage quick create mints is \
             API-Gateway-managed, got {stage}"
        );
        assert_created_now(case, &stage);
        let deployment_id = as_str(&stage, "deploymentId");
        assert_minted_id(case, "DeploymentId", &deployment_id);

        let deployment = get(
            &emulator,
            case,
            "GetDeployment",
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
        )
        .await;
        assert_eq!(
            as_str(&deployment, "deploymentStatus"),
            DEPLOYED,
            "case {case:?}: expected the released deployment to be {DEPLOYED}, got {deployment}"
        );
        assert_eq!(
            deployment.get("autoDeployed"),
            Some(&json!(true)),
            "case {case:?}: the deployment quick create releases is the service's, not \
             a caller's, got {deployment}"
        );
        assert_created_now(case, &deployment);

        // A read must not change what a later caller sees: the whole set reads back
        // identically.
        for (step, uri, first) in [
            (
                "GetIntegration",
                format!("/v2/apis/{api_id}/integrations/{integration_id}"),
                &integration,
            ),
            (
                "GetStage",
                format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
                &stage,
            ),
            (
                "GetDeployment",
                format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
                &deployment,
            ),
        ] {
            let again = get(&emulator, case, step, &uri).await;
            assert_eq!(
                &again, first,
                "case {case:?}: {step} changed the record it reported"
            );
        }
    }
}

#[tokio::test]
async fn an_api_created_without_a_target_gets_none_of_the_quick_create_set() {
    let emulator = emulator();

    // No `Target`, so nothing is quick-created — for an api of either protocol
    // (`Target` is "Supported only for HTTP APIs", so a WebSocket api never has one).
    for (case, request) in [
        (
            "an HTTP api created with no Target",
            json!({ "name": "plain-http", "protocolType": "HTTP" }),
        ),
        (
            "a WebSocket api, which quick create does not support",
            json!({
                "name": "plain-socket",
                "protocolType": "WEBSOCKET",
                "routeSelectionExpression": "$request.body.action"
            }),
        ),
    ] {
        let api_id = create_api(&emulator, case, &request).await;

        let (status, routes) =
            call(&emulator, "GET", &format!("/v2/apis/{api_id}/routes"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetRoutes, got {status} {routes}"
        );
        assert_eq!(
            routes["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an api created with no Target carries no route, got {routes}"
        );

        let (status, integrations) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/integrations"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIntegrations, got {status} {integrations}"
        );
        assert_eq!(
            integrations["items"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an api created with no Target carries no integration, got \
             {integrations}"
        );

        // No stage either, so the name is free for a caller's own `CreateStage`.
        let (status, body) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an api created with no Target carries no stage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": DEFAULT_STAGE_NAME })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage for the free $default name, \
             got {status} {created}"
        );
        assert_eq!(
            created.get("apiGatewayManaged"),
            Some(&json!(false)),
            "case {case:?}: a $default stage the CALLER created is its own, got {created}"
        );
    }
}

#[tokio::test]
async fn the_name_of_quick_create_s_default_stage_is_already_taken() {
    let emulator = emulator();
    let case = "a second $default stage over a quick-created api";

    let api_id = create_api(
        &emulator,
        case,
        &json!({
            "name": "taken-api",
            "protocolType": "HTTP",
            "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
        }),
    )
    .await;
    let managed = get(
        &emulator,
        case,
        "GetStage",
        &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
    )
    .await;

    // The stage quick create minted is a stage of this api like any other, so its name
    // keys the api's stage map: a caller's own `$default` would be a second stage of
    // that one name.
    let (status, body) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": DEFAULT_STAGE_NAME, "autoDeploy": false })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409 for a stage name the api already carries, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );

    // And the refused create left the managed stage exactly as it was.
    let after = get(
        &emulator,
        case,
        "GetStage",
        &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
    )
    .await;
    assert_eq!(
        after, managed,
        "case {case:?}: a refused CreateStage must leave the stored stage, got {after}"
    );

    // A caller can still deploy the api to that stage: deploying moves it onto the new
    // deployment, so the id it serves is no longer the auto-released one.
    let (status, deployment) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": DEFAULT_STAGE_NAME })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {status} {deployment}"
    );
    let released = as_str(&deployment, "deploymentId");
    assert_ne!(
        released,
        as_str(&managed, "deploymentId"),
        "case {case:?}: a caller's release mints its own DeploymentId, got {deployment}"
    );
    assert_eq!(
        deployment.get("autoDeployed"),
        Some(&json!(false)),
        "case {case:?}: a deployment the caller asked for is not auto-released, got {deployment}"
    );
    let moved = get(
        &emulator,
        case,
        "GetStage",
        &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
    )
    .await;
    assert_eq!(
        moved.get("deploymentId"),
        Some(&json!(released)),
        "case {case:?}: expected the stage to serve the deployment just released, got {moved}"
    );
}

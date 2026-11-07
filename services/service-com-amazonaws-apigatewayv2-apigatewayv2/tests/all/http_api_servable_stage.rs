// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SEQUENCE a client runs to make an API servable: an
//! api, an integration, a route pointed at that integration, and a stage over it.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue
//! (an HTTP api whose `$default` stage is created with `AutoDeploy` on, so its 5xx
//! series can drive a CloudWatch alarm):
//!
//! > Every operation in the sequence answers the HTTP status its model binds for
//! > success with the document the model declares, read back out of the store — never
//! > the scaffold channel's out-of-model `NotImplemented` (HTTP 501), which is not among
//! > the errors any of these operations declares.
//!
//! `smithy_query(query="operation", operation="CreateApi,CreateIntegration,CreateRoute,CreateStage")`
//! gives each one `http {POST, …, 201}` and the SAME four errors —
//! `BadRequestException` (400), `ConflictException` (409), `NotFoundException` (404),
//! `TooManyRequestsException` (429) — and their readers `GetIntegration`/`GetRoute`/
//! `GetStage` `http {GET, …, 200}` with `NotFoundException`/`TooManyRequestsException`.
//! `NotImplemented` appears in none of those sets, so a step that answers it has not
//! implemented its contract and the sequence cannot complete.
//!
//! The rows vary what the rule generalises over: the api's protocol type, the
//! integration type the route fronts (a Lambda `AWS_PROXY`, an `HTTP_PROXY` upstream, a
//! `MOCK`), the form of the route key, the form and length of the stage name, and
//! whether the stage asks for auto-deploy. The recorded case — an HTTP api fronting a
//! Lambda `AWS_PROXY` integration on `ANY /{proxy+}`, with a `$default` stage created
//! with `AutoDeploy` on — is ONE row.
//!
//! What this suite adds over its neighbours: `stage_lifecycle` spans the `CreateStage`
//! request surface and its refusals, and `integration_lifecycle` spans the
//! `CreateIntegration` one, but neither drives the WHOLE chain — an integration, a route
//! TARGETING it, then a stage over that api — which is what the issue's reproduction
//! does and what a client needs before any request can reach the api at all.

use serde_json::{json, Value};

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, error_code,
};

/// The upper bound of `StringWithLengthBetween1And128`, the shape
/// `CreateStage$StageName` targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// The HTTP status the scaffold channel answers for an operation that is not
/// implemented (`crate::scaffolded`), paired with [`SCAFFOLD_ERROR_CODE`].
const SCAFFOLD_STATUS: u16 = 501;

/// The error code the scaffold channel answers with. No operation this suite drives
/// declares it — see the module doc — so it is never a legitimate answer here.
const SCAFFOLD_ERROR_CODE: &str = "NotImplemented";

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

/// One row: an api, the integration its route fronts, that route's key, and the stage
/// created over them.
struct Row {
    case: &'static str,
    /// The `CreateApi` request.
    api: Value,
    /// The `CreateIntegration` request the route targets.
    integration: Value,
    /// The `RouteKey` of the route pointed at that integration.
    route_key: &'static str,
    /// The `CreateStage` request.
    stage: Value,
    /// The `StageName` the stored stage is keyed by and reports.
    stage_name: String,
    /// The `AutoDeploy` the stored stage must report — absent when the request configured
    /// none (the member is reported only once a request sets it; its documented default,
    /// "The default value is false" (`CreateStage$AutoDeploy`), governs behaviour only).
    auto_deploy: Option<bool>,
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

/// A WebSocket api: `RouteSelectionExpression` "is required for WebSocket APIs"
/// (`CreateApi$RouteSelectionExpression`).
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a Lambda-backed HTTP api with a $default stage \
                   created with AutoDeploy on",
            api: http_api("fivexx-api"),
            integration: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:boom",
                "payloadFormatVersion": "2.0"
            }),
            route_key: "ANY /{proxy+}",
            stage: json!({ "stageName": "$default", "autoDeploy": true }),
            stage_name: "$default".to_string(),
            auto_deploy: Some(true),
        },
        Row {
            case: "a named stage over a Lambda-backed route, asking for no auto-deploy",
            api: http_api("named-stage-api"),
            integration: json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:orders",
                "payloadFormatVersion": "2.0"
            }),
            route_key: "GET /orders",
            stage: json!({ "stageName": "prod", "description": "the live stage" }),
            stage_name: "prod".to_string(),
            auto_deploy: None,
        },
        Row {
            case: "an HTTP_PROXY upstream behind a stage whose name is at the upper \
                   bound of its shape",
            api: http_api("upstream-api"),
            integration: json!({
                "integrationType": "HTTP_PROXY",
                "integrationUri": "https://example.com/upstream",
                "integrationMethod": "ANY",
                "payloadFormatVersion": "1.0"
            }),
            route_key: "POST /forward",
            stage: json!({
                "stageName": "s".repeat(MAX_STAGE_NAME_LEN),
                "autoDeploy": true,
                "stageVariables": { "upstream": "https://example.com/path" }
            }),
            stage_name: "s".repeat(MAX_STAGE_NAME_LEN),
            auto_deploy: Some(true),
        },
        Row {
            case: "a WebSocket api whose $default route fronts a MOCK integration",
            api: websocket_api("socket-api"),
            integration: json!({ "integrationType": "MOCK" }),
            route_key: "$default",
            stage: json!({ "stageName": "sockets", "autoDeploy": true }),
            stage_name: "sockets".to_string(),
            auto_deploy: Some(true),
        },
    ]
}

/// Assert one step of the sequence answered the status its model binds for success —
/// and, first, that it did not answer the scaffold channel's `NotImplemented`, which
/// its model does not declare.
fn modelled_success(case: &str, step: &str, expected: u16, status: u16, body: &Value) {
    assert!(
        status != SCAFFOLD_STATUS && error_code(body) != SCAFFOLD_ERROR_CODE,
        "case {case:?}: {step} answered the scaffold's {SCAFFOLD_ERROR_CODE}, which its \
         model does not declare among its errors; got {status} {body}"
    );
    assert_eq!(
        status, expected,
        "case {case:?}: expected {expected} from {step}, got {status} {body}"
    );
}

#[tokio::test]
async fn the_sequence_that_makes_an_api_servable_completes_and_reads_back() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();

        // The api. Its `ApiEndpoint` is the address a client calls, so it has to be the
        // address this api is reached at (`api_endpoint_scheme` holds that whole rule: an
        // HTTP api's own `execute-api` host, a WEBSOCKET api's own socket).
        let (status, api) = call(&emulator, "POST", "/v2/apis", Some(row.api.clone())).await;
        modelled_success(case, "CreateApi", 201, status, &api);
        let api_id = as_str(&api, "apiId");
        assert_minted_id(case, "ApiId", &api_id);
        crate::api_endpoint_scheme::assert_announced_endpoint(
            case,
            "CreateApi",
            &as_str(&api, "protocolType"),
            &api_id,
            crate::api_lifecycle::REGION,
            &as_str(&api, "apiEndpoint"),
        );

        // The integration the route will front.
        let (status, integration) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/integrations"),
            Some(row.integration.clone()),
        )
        .await;
        modelled_success(case, "CreateIntegration", 201, status, &integration);
        let integration_id = as_str(&integration, "integrationId");
        assert_minted_id(case, "IntegrationId", &integration_id);

        // The route, pointed at that integration: "integrations/{integration-id}" is the
        // form `Route$Target` takes.
        let target = format!("integrations/{integration_id}");
        let (status, route) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({ "routeKey": row.route_key, "target": target })),
        )
        .await;
        modelled_success(case, "CreateRoute", 201, status, &route);
        let route_id = as_str(&route, "routeId");
        assert_minted_id(case, "RouteId", &route_id);
        assert_eq!(
            as_str(&route, "routeKey"),
            row.route_key,
            "case {case:?}: expected the RouteKey the request named, got {route}"
        );
        assert_eq!(
            as_str(&route, "target"),
            target,
            "case {case:?}: expected the route to target the integration, got {route}"
        );

        // The stage over them — the step the issue reports as unreachable.
        let (status, stage) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(row.stage.clone()),
        )
        .await;
        modelled_success(case, "CreateStage", 201, status, &stage);
        assert_eq!(
            as_str(&stage, "stageName"),
            row.stage_name,
            "case {case:?}: expected the StageName the request named, got {stage}"
        );
        assert_eq!(
            stage.get("autoDeploy"),
            row.auto_deploy.map(|flag| json!(flag)).as_ref(),
            "case {case:?}: expected AutoDeploy {:?}, got {stage}",
            row.auto_deploy
        );
        // "If you created an API using quick create, the $default stage is managed by
        // API Gateway" (`Stage$ApiGatewayManaged`) — a stage the caller asked for is its
        // own, whatever it is called.
        assert_eq!(
            stage.get("apiGatewayManaged"),
            Some(&json!(false)),
            "case {case:?}: a stage the caller created is not API-Gateway-managed, got {stage}"
        );
        assert_created_now(case, &stage);

        // An auto-deploying stage has a deployment RELEASED BY THE SERVICE, which is what
        // makes the api callable on the endpoint above; one that asked for none serves
        // none until a `CreateDeployment` releases one.
        let deployment_id = stage.get("deploymentId").and_then(Value::as_str);
        if row.auto_deploy == Some(true) {
            let deployment_id = deployment_id.unwrap_or_else(|| {
                panic!("case {case:?}: an auto-deploying stage must report a DeploymentId, got {stage}")
            });
            let (status, deployment) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
                None,
            )
            .await;
            modelled_success(case, "GetDeployment", 200, status, &deployment);
            assert_eq!(
                as_str(&deployment, "deploymentStatus"),
                DEPLOYED,
                "case {case:?}: the released deployment must be {DEPLOYED}, got {deployment}"
            );
        } else {
            assert_eq!(
                deployment_id, None,
                "case {case:?}: a stage that asked for no deployment serves none, got {stage}"
            );
        }

        // Each resource reads back out of the store, member for member: the creates
        // answered with what they stored, not with what the request carried. Every stage
        // name in the table is path-safe, `$default` included.
        for (step, uri, created) in [
            (
                "GetIntegration",
                format!("/v2/apis/{api_id}/integrations/{integration_id}"),
                &integration,
            ),
            (
                "GetRoute",
                format!("/v2/apis/{api_id}/routes/{route_id}"),
                &route,
            ),
            (
                "GetStage",
                format!("/v2/apis/{api_id}/stages/{}", row.stage_name),
                &stage,
            ),
        ] {
            let (status, fetched) = call(&emulator, "GET", &uri, None).await;
            modelled_success(case, step, 200, status, &fetched);
            assert_eq!(
                &fetched, created,
                "case {case:?}: {step} must report the record the create stored, got {fetched}"
            );

            // A read must not change what a later caller sees.
            let (status, refetched) = call(&emulator, "GET", &uri, None).await;
            modelled_success(case, step, 200, status, &refetched);
            assert_eq!(
                refetched, fetched,
                "case {case:?}: {step} changed the record it reported"
            );
        }
    }
}

#[tokio::test]
async fn a_stage_is_servable_under_every_api_that_carries_one() {
    // One emulator, several apis: the stage of one api is not the stage of another, and
    // the sequence completing once does not depend on an empty store.
    let emulator = emulator();
    let mut created: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let case = row.case;
        let (status, api) = call(&emulator, "POST", "/v2/apis", Some(row.api.clone())).await;
        modelled_success(case, "CreateApi", 201, status, &api);
        let api_id = as_str(&api, "apiId");

        // Every api gets a stage of the SAME name, so the key is proven to be per api.
        let (status, stage) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({ "stageName": "shared", "autoDeploy": true })),
        )
        .await;
        modelled_success(case, "CreateStage", 201, status, &stage);
        assert_eq!(
            as_str(&stage, "stageName"),
            "shared",
            "case {case:?}: expected the StageName the request named, got {stage}"
        );
        created.push((api_id, as_str(&stage, "stageName")));
    }

    for (api_id, stage_name) in &created {
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        modelled_success("every api carries its own stage", "GetStage", 200, status, &fetched);
        assert_eq!(
            as_str(&fetched, "stageName"),
            *stage_name,
            "api {api_id}: expected the stage it owns, got {fetched}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for RELEASING a deployment to a stage — the step that makes
//! an api callable on `{scheme}://{api-id}.execute-api.{region}.amazonaws.com/{stage}`.
//!
//! The rule these cases assert, one level above the recorded WebSocket case that filed
//! the issue:
//!
//! > `CreateDeployment` answers 201 (`http {POST, /v2/apis/{ApiId}/deployments, 201}`)
//! > with a freshly minted `DeploymentId` whose `DeploymentStatus` is `DEPLOYED`, for an
//! > api of ANY protocol and whatever routes and integrations it carries; and when the
//! > request names a stage of that api, THAT stage — and no other — is moved onto the
//! > new deployment ("The identifier of the Deployment that the Stage is associated
//! > with", `Stage$DeploymentId`), with its `LastUpdatedDate` at the instant of the
//! > release ("The timestamp when the stage was last updated", `Stage$LastUpdatedDate`)
//! > and its `CreatedDate` where it was.
//!
//! The rows vary what the rule generalises over: the api's protocol type, whether it
//! fronts a Lambda `AWS_PROXY` integration, which route keys it carries (the
//! `$connect`/`$default`/`$disconnect` triple a WebSocket api needs, a single route, an
//! HTTP route, none at all), the stage name's form and length, and whether the request
//! carries a description. The recorded case — a WebSocket api with Lambda-backed
//! `$connect`/`$default`/`$disconnect` routes, deployed to a stage — is ONE row.
//!
//! What is deliberately NOT asserted here: a deployment released to a stage whose
//! `AutoDeploy` is on. `Stage$DeploymentId` is "Can't be updated if autoDeploy is
//! enabled", and this change does not decide what the service answers for that; the
//! auto-release path of `CreateStage` is covered by `stage_lifecycle`.

use std::collections::{BTreeMap, BTreeSet};
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{
    as_str, assert_created_now, assert_minted_id, call, emulator, instant,
};

/// The `DeploymentStatus` a released deployment carries, from the values
/// `DeploymentStatus` declares — `PENDING | FAILED | DEPLOYED`
/// (`smithy_query(query="shape", shape="DeploymentStatus")`).
const DEPLOYED: &str = "DEPLOYED";

/// The upper bound of `StringWithLengthBetween1And128`, the shape
/// `CreateDeployment$StageName` targets — so a stage name at this length is a name the
/// release path has to carry.
const MAX_STAGE_NAME_LEN: usize = 128;

/// One row: the api to deploy, what to give it first, the stage to release to, and the
/// `CreateDeployment` body to send.
struct Row {
    case: &'static str,
    /// The `CreateApi` request.
    api: Value,
    /// A `CreateIntegration` request to create under the api, if the row fronts one.
    integration: Option<Value>,
    /// Route keys to create under the api, each targeting the integration above when
    /// the row has one.
    routes: &'static [&'static str],
    /// The name of the stage the deployment is released to.
    stage_name: String,
    /// The `Description` the `CreateDeployment` request carries, if any.
    description: Option<&'static str>,
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

/// A Lambda proxy integration, the integration a WebSocket route is backed by.
fn lambda_proxy(function: &str) -> Value {
    json!({
        "integrationType": "AWS_PROXY",
        "integrationUri": format!("arn:aws:lambda:eu-west-1:000000000000:function:{function}"),
        "integrationMethod": "POST"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a WebSocket api whose $connect/$default/$disconnect routes front a Lambda",
            api: websocket_api("duplex-api"),
            integration: Some(lambda_proxy("duplex-handler")),
            routes: &["$connect", "$default", "$disconnect"],
            stage_name: "dev".to_string(),
            description: Some("the duplex data plane"),
        },
        Row {
            case: "a WebSocket api with a single $connect route and no integration",
            api: websocket_api("connect-only-api"),
            integration: None,
            routes: &["$connect"],
            stage_name: "production".to_string(),
            description: None,
        },
        Row {
            case: "a WebSocket api released to the $default stage",
            api: websocket_api("default-stage-api"),
            integration: Some(lambda_proxy("default-stage-handler")),
            routes: &["$connect", "$disconnect"],
            stage_name: "$default".to_string(),
            description: Some(""),
        },
        Row {
            case: "an HTTP api fronting a Lambda on one route",
            api: http_api("http-api"),
            integration: Some(json!({
                "integrationType": "AWS_PROXY",
                "integrationUri": "arn:aws:lambda:eu-west-1:000000000000:function:http-handler",
                "payloadFormatVersion": "2.0"
            })),
            routes: &["GET /items"],
            stage_name: "test".to_string(),
            description: Some("an http api released to a stage"),
        },
        Row {
            case: "an api with no routes at all, released to a stage named at its upper bound",
            api: http_api("routeless-api"),
            integration: None,
            routes: &[],
            stage_name: "s".repeat(MAX_STAGE_NAME_LEN),
            description: None,
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

/// Create one integration and answer its `IntegrationId`.
async fn create_integration(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateIntegration, got {status} {created}"
    );
    as_str(&created, "integrationId")
}

/// Give `api_id` one route per key, each pointing at `target` when there is one.
async fn create_routes(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_keys: &[&str],
    target: Option<&str>,
) {
    for route_key in route_keys {
        let mut request = json!({ "routeKey": route_key });
        if let Some(integration_id) = target {
            request
                .as_object_mut()
                .expect("the CreateRoute body is an object")
                .insert("target".to_string(), json!(format!("integrations/{integration_id}")));
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
            "case {case:?}: expected 201 from CreateRoute for {route_key:?}, got {status} {created}"
        );
    }
}

/// Create one stage and answer the document the service stored.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
) -> Value {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": stage_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    assert_eq!(
        created.get("deploymentId"),
        None,
        "case {case:?}: a stage created with no DeploymentId serves none yet, got {created}"
    );
    created
}

/// Read one stage back.
async fn get_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
) -> Value {
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStage for {stage_name:?}, got {status} {fetched}"
    );
    fetched
}

/// Release a deployment and answer the document the service stored.
async fn create_deployment(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> Value {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {status} {created}"
    );
    created
}

/// Every member a released deployment reports.
fn assert_released(case: &str, deployment: &Value, description: Option<&str>) {
    assert_minted_id(case, "DeploymentId", &as_str(deployment, "deploymentId"));
    assert_eq!(
        deployment.get("deploymentStatus"),
        Some(&json!(DEPLOYED)),
        "case {case:?}: expected DeploymentStatus {DEPLOYED}, got {deployment}"
    );
    assert_eq!(
        deployment.get("autoDeployed"),
        Some(&json!(false)),
        "case {case:?}: a deployment the caller asked for is not auto-released, got {deployment}"
    );
    assert_eq!(
        deployment.get("description"),
        description.map(|text| json!(text)).as_ref(),
        "case {case:?}: expected Description {description:?}, got {deployment}"
    );
    assert_created_now(case, deployment);
}

#[tokio::test]
async fn deploying_to_a_named_stage_moves_that_stage_onto_the_new_deployment() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let target = match row.integration.as_ref() {
            Some(request) => Some(create_integration(&emulator, row.case, &api_id, request).await),
            None => None,
        };
        create_routes(
            &emulator,
            row.case,
            &api_id,
            row.routes,
            target.as_deref(),
        )
        .await;
        let stage = create_stage(&emulator, row.case, &api_id, &row.stage_name).await;

        let mut request = json!({ "stageName": row.stage_name });
        if let Some(description) = row.description {
            request
                .as_object_mut()
                .expect("the CreateDeployment body is an object")
                .insert("description".to_string(), json!(description));
        }
        let deployment = create_deployment(&emulator, row.case, &api_id, request).await;
        assert_released(row.case, &deployment, row.description);
        let deployment_id = as_str(&deployment, "deploymentId");

        // The stage now serves the deployment that was just released to it, and says so
        // to every later reader.
        let released = get_stage(&emulator, row.case, &api_id, &row.stage_name).await;
        assert_eq!(
            released.get("deploymentId"),
            Some(&json!(deployment_id)),
            "case {:?}: expected the stage to serve the new deployment {deployment_id:?}, \
             got {released}",
            row.case
        );
        // The stage was REWRITTEN, not recreated: it was created when it was created,
        // and last updated no earlier than that, at the release instant.
        assert_eq!(
            released.get("createdDate"),
            stage.get("createdDate"),
            "case {:?}: releasing a deployment must not move the stage's CreatedDate, \
             got {released}",
            row.case
        );
        assert_eq!(
            released.get("lastUpdatedDate"),
            deployment.get("createdDate"),
            "case {:?}: expected the stage's LastUpdatedDate at the release instant, \
             got {released}",
            row.case
        );
        let created_at = instant(row.case, &released, "createdDate");
        let updated_at = instant(row.case, &released, "lastUpdatedDate");
        assert!(
            updated_at >= created_at,
            "case {:?}: expected LastUpdatedDate at or after CreatedDate, got \
             {updated_at} against {created_at}",
            row.case
        );
        // "Supported only for stages with autoDeploy enabled"
        // (`Stage$LastDeploymentStatusMessage`): the caller released this deployment, so
        // the stage has no auto-release status to report.
        assert_eq!(
            released.get("lastDeploymentStatusMessage"),
            None,
            "case {:?}: a stage the caller deployed to reports no auto-release status, \
             got {released}",
            row.case
        );

        // The deployment itself reads back as the document the release answered with.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from GetDeployment, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            fetched, deployment,
            "case {:?}: GetDeployment must report the document CreateDeployment answered with",
            row.case
        );

        // A read must not change what a later caller sees.
        let reread = get_stage(&emulator, row.case, &api_id, &row.stage_name).await;
        assert_eq!(
            reread, released,
            "case {:?}: GetStage changed the record it reported",
            row.case
        );
    }
}

#[tokio::test]
async fn only_the_named_stage_moves_and_every_release_mints_its_own_deployment() {
    let emulator = emulator();
    let case = "one api carrying three stages";

    let api_id = create_api(&emulator, case, &websocket_api("multi-stage-api")).await;
    let target = create_integration(
        &emulator,
        case,
        &api_id,
        &lambda_proxy("multi-stage-handler"),
    )
    .await;
    create_routes(
        &emulator,
        case,
        &api_id,
        &["$connect", "$default", "$disconnect"],
        Some(&target),
    )
    .await;
    let stage_names = ["dev", "test", "prod"];
    for stage_name in stage_names {
        create_stage(&emulator, case, &api_id, stage_name).await;
    }

    // A release that names NO stage moves no stage at all: a deployment is callable
    // only through the stage a release points at it.
    let unreleased = create_deployment(&emulator, case, &api_id, json!({})).await;
    assert_released(case, &unreleased, None);
    for stage_name in stage_names {
        let stage = get_stage(&emulator, case, &api_id, stage_name).await;
        assert_eq!(
            stage.get("deploymentId"),
            None,
            "case {case:?}: a deployment naming no stage must move no stage, got {stage}"
        );
    }

    // Releasing to one stage moves ONLY that stage: after each release, every stage
    // still serves the deployment released to IT, and the ones never released to serve
    // none.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    minted.insert(as_str(&unreleased, "deploymentId"));
    let mut served: BTreeMap<&str, Option<String>> =
        stage_names.iter().map(|name| (*name, None)).collect();
    for stage_name in stage_names {
        let deployment = create_deployment(
            &emulator,
            case,
            &api_id,
            json!({ "stageName": stage_name, "description": format!("release to {stage_name}") }),
        )
        .await;
        let deployment_id = as_str(&deployment, "deploymentId");
        assert!(
            minted.insert(deployment_id.clone()),
            "case {case:?}: every release mints its own DeploymentId, got {deployment_id:?} twice"
        );
        served.insert(stage_name, Some(deployment_id));

        for (other, expected) in &served {
            let stage = get_stage(&emulator, case, &api_id, other).await;
            assert_eq!(
                stage.get("deploymentId"),
                expected.as_ref().map(|id| json!(id)).as_ref(),
                "case {case:?}: after releasing to {stage_name:?}, stage {other:?} must \
                 serve {expected:?}, got {stage}"
            );
        }
    }

    // Re-releasing to a stage moves it onto the NEW deployment, and the one it served
    // before is still there: "An immutable representation of an API" (`Deployment`).
    let previous = as_str(
        &get_stage(&emulator, case, &api_id, "dev").await,
        "deploymentId",
    );
    let redeployed = create_deployment(
        &emulator,
        case,
        &api_id,
        json!({ "stageName": "dev", "description": "the second cut" }),
    )
    .await;
    let redeployed_id = as_str(&redeployed, "deploymentId");
    assert_ne!(
        redeployed_id, previous,
        "case {case:?}: a second release must mint a new DeploymentId, got {redeployed_id:?}"
    );
    let dev = get_stage(&emulator, case, &api_id, "dev").await;
    assert_eq!(
        dev.get("deploymentId"),
        Some(&json!(redeployed_id)),
        "case {case:?}: expected stage \"dev\" to serve the newest deployment, got {dev}"
    );
    let (status, kept) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/{previous}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the deployment the stage served before is still there, got {status} {kept}"
    );
}

/// How long to wait so the release lands in a LATER second than the stage's creation.
/// The service reports its instants at whole-second granularity, so only a wait past a
/// second boundary makes the move of `LastUpdatedDate` observable on the wire.
///
/// A BLOCKING wait on purpose: this workspace's `tokio` carries only the `macros`/`rt`
/// features, so there is no `tokio::time` to await, and one test spending a second is
/// cheaper than giving every suite a timer.
const SECOND_BOUNDARY_WAIT: Duration = Duration::from_millis(1_100);

#[tokio::test]
async fn a_release_moves_the_stage_s_last_updated_date_past_its_creation() {
    let emulator = emulator();
    let case = "a stage released to a deployment a second later";

    let api_id = create_api(&emulator, case, &websocket_api("clock-api")).await;
    create_routes(&emulator, case, &api_id, &["$connect"], None).await;
    let stage = create_stage(&emulator, case, &api_id, "dev").await;
    assert_eq!(
        stage.get("lastUpdatedDate"),
        stage.get("createdDate"),
        "case {case:?}: nothing has rewritten the stage yet, got {stage}"
    );

    sleep(SECOND_BOUNDARY_WAIT);

    let deployment =
        create_deployment(&emulator, case, &api_id, json!({ "stageName": "dev" })).await;
    let released = get_stage(&emulator, case, &api_id, "dev").await;
    let created_at = instant(case, &released, "createdDate");
    let updated_at = instant(case, &released, "lastUpdatedDate");
    assert!(
        updated_at > created_at,
        "case {case:?}: expected LastUpdatedDate to move to the release instant, got \
         {updated_at} against a CreatedDate of {created_at}"
    );
    assert_eq!(
        released.get("lastUpdatedDate"),
        deployment.get("createdDate"),
        "case {case:?}: expected the stage's LastUpdatedDate at the release instant, got {released}"
    );
    assert_eq!(
        released.get("createdDate"),
        stage.get("createdDate"),
        "case {case:?}: the stage's CreatedDate must not move, got {released}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteDeployment`.
//!
//! The rule these cases assert, one level above the recorded case that filed the issue:
//!
//! > `DeleteDeployment` removes the `Deployment` record the named api OWNS and answers
//! > 204 with an empty body — unless a stage of that same api is still serving it
//! > ("The identifier of the Deployment that the Stage is associated with",
//! > `Stage$DeploymentId`), in which case the delete is refused 400
//! > `BadRequestException` and persists nothing; an identifier that resolves to nothing
//! > — an api nothing minted, a deployment id nothing minted, a deployment addressed
//! > through another api — is a 404 `NotFoundException`.
//!
//! The rows vary what the rule generalises over: the owning api's protocol type, whether
//! the deployment carries a description, how many stages the api holds and whether each
//! one serves the deployment being deleted (none, an unbound stage, one, two), whether
//! the CALLER asked for the deployment or the service released it by itself for an
//! auto-deploying stage, and whether the blocking reference is removed by releasing a new
//! deployment to the stage. The recorded unit that filed the issue — a deployment a stage
//! is bound to, deleted and refused — is one row of the refusal table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The sentence API Gateway refuses the delete of a deployment a stage is still serving
/// with (`crate::state`-side constant `ACTIVE_STAGES_MESSAGE` in
/// `src/ops/delete_deployment.rs`).
const ACTIVE_STAGES_MESSAGE: &str =
    "Active stages pointing to this deployment must be moved or deleted";

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

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one deployment under `api_id` and answer its `DeploymentId`.
async fn create_deployment(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> String {
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
    as_str(&created, "deploymentId")
}

/// Create one stage under `api_id` and answer its body.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: Value,
) -> Value {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    created
}

/// `(status, body)` of `GetDeployment`.
async fn get_deployment(
    emulator: &MultiServiceRouter,
    api_id: &str,
    deployment_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
        None,
    )
    .await
}

/// `(status, body)` of `DeleteDeployment`.
async fn delete_deployment(
    emulator: &MultiServiceRouter,
    api_id: &str,
    deployment_id: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
        None,
    )
    .await
}

/// Every `DeploymentId` `GetDeployments` lists for `api_id`.
async fn list_deployment_ids(emulator: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected 200 from GetDeployments for {api_id}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| as_str(item, "deploymentId"))
        .collect()
}

/// The `DeploymentId` a stage of `api_id` reports serving, if any.
async fn stage_deployment_id(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
) -> Option<String> {
    let (status, body) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStage for {stage_name:?}, got {status} {body}"
    );
    body.get("deploymentId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A stage the row creates beside the deployment it deletes.
struct StageSpec {
    stage_name: &'static str,
    /// Whether the stage is created serving the SPARE deployment. A stage created
    /// serving nothing references no deployment at all.
    serves_spare: bool,
}

/// One row of the "the delete goes through" table.
struct Removable {
    case: &'static str,
    api: Value,
    /// The `CreateDeployment` body for the deployment the row deletes.
    request: Value,
    /// The stages the api carries while the delete runs. None of them serves the
    /// deployment being deleted.
    stages: &'static [StageSpec],
}

fn removable_rows() -> Vec<Removable> {
    vec![
        Removable {
            case: "an HTTP api's deployment with no stage in the api at all",
            api: http_api("delete-unused-http-api"),
            request: json!({}),
            stages: &[],
        },
        Removable {
            case: "a described deployment beside a stage that serves nothing",
            api: http_api("delete-unbound-stage-api"),
            request: json!({ "description": "the first cut" }),
            stages: &[StageSpec {
                stage_name: "unbound",
                serves_spare: false,
            }],
        },
        Removable {
            case: "a deployment beside a stage serving ANOTHER deployment of the api",
            api: http_api("delete-other-deployment-api"),
            request: json!({ "description": "superseded" }),
            stages: &[StageSpec {
                stage_name: "live",
                serves_spare: true,
            }],
        },
        Removable {
            case: "a WebSocket api whose two stages both serve another deployment",
            api: websocket_api("delete-socket-api"),
            request: json!({}),
            stages: &[
                StageSpec {
                    stage_name: "live",
                    serves_spare: true,
                },
                StageSpec {
                    stage_name: "beta_2",
                    serves_spare: true,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn delete_deployment_removes_the_record_and_a_later_read_reports_not_found() {
    let emulator = emulator();

    for row in removable_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        // The deployment the row deletes, and a spare the row's stages may serve.
        let deployment_id =
            create_deployment(&emulator, row.case, &api_id, row.request.clone()).await;
        let spare_id = create_deployment(&emulator, row.case, &api_id, json!({})).await;
        for stage in row.stages {
            let mut request = json!({ "stageName": stage.stage_name });
            if stage.serves_spare {
                request["deploymentId"] = json!(spare_id);
            }
            create_stage(&emulator, row.case, &api_id, request).await;
        }

        let (status, body) = delete_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 from DeleteDeployment, got {status} {body}",
            row.case
        );
        assert_eq!(
            body,
            Value::Null,
            "case {:?}: expected an empty DeleteDeployment body, got {body}",
            row.case
        );

        // Read back through the implemented reader: the record is gone.
        let (status, body) = get_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from GetDeployment after the delete, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // And gone from the listing, while the api's other deployment stays.
        let listed = list_deployment_ids(&emulator, &api_id).await;
        assert!(
            !listed.contains(&deployment_id),
            "case {:?}: expected {deployment_id} to be gone from GetDeployments, got {listed:?}",
            row.case
        );
        assert!(
            listed.contains(&spare_id),
            "case {:?}: expected {spare_id} to survive, got {listed:?}",
            row.case
        );

        // A second delete removes nothing: the id now resolves to nothing.
        let (status, body) = delete_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404 from the second DeleteDeployment, got {status} {body}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {:?}: expected NotFoundException, got {body}",
            row.case
        );

        // Every stage of the api survives the delete, still serving what it served.
        for stage in row.stages {
            let serving =
                stage_deployment_id(&emulator, row.case, &api_id, stage.stage_name).await;
            let expected = stage.serves_spare.then(|| spare_id.clone());
            assert_eq!(
                serving, expected,
                "case {:?}: expected stage {:?} to still serve {expected:?}, got {serving:?}",
                row.case, stage.stage_name
            );
        }
    }
}

/// One row of the refusal table: the stage names of the stages created serving the
/// deployment the row then tries to delete. Non-empty by construction.
struct InUse {
    case: &'static str,
    api: Value,
    stage_names: &'static [&'static str],
}

fn in_use_rows() -> Vec<InUse> {
    vec![
        InUse {
            case: "one stage of an HTTP api serves the deployment",
            api: http_api("in-use-single-api"),
            stage_names: &["live"],
        },
        InUse {
            case: "two stages of the api serve the one deployment",
            api: http_api("in-use-fanin-api"),
            stage_names: &["live", "canary"],
        },
        InUse {
            case: "a WebSocket api's $default stage serves the deployment",
            api: websocket_api("in-use-socket-api"),
            stage_names: &["$default"],
        },
    ]
}

#[tokio::test]
async fn delete_deployment_is_refused_while_a_stage_of_its_api_serves_it() {
    let emulator = emulator();

    for row in in_use_rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let deployment_id = create_deployment(&emulator, row.case, &api_id, json!({})).await;
        for stage_name in row.stage_names {
            create_stage(
                &emulator,
                row.case,
                &api_id,
                json!({ "stageName": stage_name, "deploymentId": deployment_id }),
            )
            .await;
        }

        let (status, body) = delete_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400 from DeleteDeployment while {} stage(s) serve it, got {status} {body}",
            row.case,
            row.stage_names.len()
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {:?}: expected BadRequestException, got {body}",
            row.case
        );
        assert_eq!(
            body.get("message"),
            Some(&json!(ACTIVE_STAGES_MESSAGE)),
            "case {:?}: expected the active-stages refusal sentence, got {body}",
            row.case
        );

        // The refusal persisted nothing: the deployment is still there to read.
        let (status, fetched) = get_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the refused deployment to survive, got {status} {fetched}",
            row.case
        );
        assert_eq!(
            as_str(&fetched, "deploymentId"),
            deployment_id,
            "case {:?}: expected the same deployment back, got {fetched}",
            row.case
        );

        // What blocks the delete is the REFERENCE, not the deployment: release a new
        // deployment to every stage that serves it and the very same delete goes through.
        let mut moved_onto: Vec<String> = Vec::new();
        for stage_name in row.stage_names {
            moved_onto.push(
                create_deployment(
                    &emulator,
                    row.case,
                    &api_id,
                    json!({ "stageName": stage_name }),
                )
                .await,
            );
        }

        let (status, body) = delete_deployment(&emulator, &api_id, &deployment_id).await;
        assert_eq!(
            status, 204,
            "case {:?}: expected 204 once no stage serves the deployment, got {status} {body}",
            row.case
        );

        // And each deployment a stage was moved onto is refused in its turn.
        for new_id in &moved_onto {
            let (status, body) = delete_deployment(&emulator, &api_id, new_id).await;
            assert_eq!(
                status, 400,
                "case {:?}: expected 400 for the deployment a stage now serves, got {status} {body}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {:?}: expected BadRequestException, got {body}",
                row.case
            );
        }
    }
}

/// The reference a stage holds blocks the delete whoever released the deployment: a stage
/// whose `AutoDeploy` is on has its deployment released BY THE SERVICE, and that
/// deployment is serving the stage just the same.
#[tokio::test]
async fn an_auto_released_deployment_a_stage_serves_cannot_be_deleted_either() {
    let emulator = emulator();
    let case = "an auto-deploying stage's service-released deployment";

    let api_id = create_api(&emulator, case, &http_api("auto-deploy-api")).await;
    create_stage(
        &emulator,
        case,
        &api_id,
        json!({ "stageName": "live", "autoDeploy": true }),
    )
    .await;
    let auto_released = stage_deployment_id(&emulator, case, &api_id, "live")
        .await
        .expect("an auto-deploying stage reports the deployment the service released to it");

    let (status, body) = delete_deployment(&emulator, &api_id, &auto_released).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 for the deployment the auto-deploying stage serves, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );

    let (status, fetched) = get_deployment(&emulator, &api_id, &auto_released).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the refused deployment to survive, got {status} {fetched}"
    );
    assert_eq!(
        fetched.get("autoDeployed"),
        Some(&json!(true)),
        "case {case:?}: expected the service-released deployment, got {fetched}"
    );
}

/// A `DeploymentId` is unique only within the api that owns it, so a stage of ANOTHER api
/// serving the same id references nothing here: only the stages of the deployment's OWN
/// api can block its delete.
#[tokio::test]
async fn a_stage_of_another_api_does_not_block_the_delete() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", &http_api("scoped-owner-api")).await;
    let deployment_id =
        create_deployment(&emulator, "the owning api's deployment", &owner, json!({})).await;

    // A DIFFERENT api, whose own first deployment carries the very same id — each api
    // mints from its own sequence — with a stage serving it.
    let bystander = create_api(&emulator, "the bystander api", &http_api("scoped-other-api")).await;
    let bystander_deployment = create_deployment(
        &emulator,
        "the bystander api's deployment",
        &bystander,
        json!({}),
    )
    .await;
    assert_eq!(
        bystander_deployment, deployment_id,
        "each api mints its deployment ids from its own sequence, so the first deployment \
         of either api carries the same id; got {bystander_deployment} and {deployment_id}"
    );
    create_stage(
        &emulator,
        "the bystander api's stage",
        &bystander,
        json!({ "stageName": "live", "deploymentId": bystander_deployment }),
    )
    .await;

    let (status, body) = delete_deployment(&emulator, &owner, &deployment_id).await;
    assert_eq!(
        status, 204,
        "expected 204: no stage of the OWNING api serves the deployment, got {status} {body}"
    );

    // The other api's deployment, and the stage serving it, are untouched.
    let (status, fetched) = get_deployment(&emulator, &bystander, &bystander_deployment).await;
    assert_eq!(
        status, 200,
        "expected the other api's deployment to be untouched, got {status} {fetched}"
    );
    let serving = stage_deployment_id(&emulator, "the bystander api", &bystander, "live").await;
    assert_eq!(
        serving,
        Some(bystander_deployment.clone()),
        "expected the other api's stage to keep serving its own deployment, got {serving:?}"
    );
}

#[tokio::test]
async fn delete_deployment_answers_not_found_for_an_identifier_that_resolves_to_nothing() {
    let emulator = emulator();

    let owner = create_api(&emulator, "the owning api", &http_api("delete-owner-api")).await;
    let other = create_api(&emulator, "another api", &http_api("delete-other-api")).await;
    let deployment_id =
        create_deployment(&emulator, "the owning api's deployment", &owner, json!({})).await;

    for (case, api_id, target_id) in [
        ("an api that does not exist", "nosuchapi00", &deployment_id),
        (
            "a deployment id nothing minted",
            owner.as_str(),
            &"nosuchdep0".to_string(),
        ),
        (
            "a deployment addressed through another api",
            other.as_str(),
            &deployment_id,
        ),
    ] {
        let (status, body) = delete_deployment(&emulator, api_id, target_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteDeployment, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
    }

    // None of those refusals removed the deployment they named.
    let (status, body) = get_deployment(&emulator, &owner, &deployment_id).await;
    assert_eq!(
        status, 200,
        "expected the deployment to survive every refused delete, got {status} {body}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteStage`.
//!
//! The rule these cases assert: **`DeleteStage` removes the stage its `(ApiId, StageName)`
//! pair names and answers 204 with an empty body — after which that stage is gone from
//! `GetStage` and from its api's `GetStages`, while every OTHER stage, of that api and of
//! any other, is untouched — and it answers 404 `NotFoundException` when no api carries that
//! id, when that api holds no stage of that name, and for a SECOND delete of the same
//! stage.** Those two are the only errors the model declares for it
//! (`smithy_query(query="operation", operation="DeleteStage")`: `NotFoundException`,
//! `TooManyRequestsException`), so a stage this service holds is never answered with the
//! scaffold channel's out-of-model `NotImplemented`.
//!
//! The rows span what the rule generalises over: the FORM and length of the stage name
//! (plain, hyphened and underscored, the reserved `$default`, the 128-character bound of
//! `StringWithLengthBetween1And128`), which optional members the stage carries (none,
//! auto-deploy, route settings and stage variables, a released deployment), the parent api's
//! protocol type, and how many stages that api holds beside the one deleted. The case this
//! suite was written for — a CloudFormation stack deleting the `$default` stage it created
//! over an api — is one row.
//!
//! A deployment is NOT a stage: a stage that served one is deleted while that deployment
//! stays on the api, which the last case pins.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    invalid_api_identifier_message, invalid_identifier_message, STAGE_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The upper bound of `StringWithLengthBetween1And128`, the shape `CreateStage$StageName`
/// targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// One api whose stages are deleted: the `CreateApi` body, and the `CreateStage` bodies of
/// every stage it holds. The FIRST stage of each row is the one the delete removes; the rest
/// are the ones it must leave alone.
struct Row {
    case: &'static str,
    api: Value,
    stages: Vec<Value>,
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

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the $default stage of an HTTP api, created with auto-deploy on",
            api: http_api("default-stage-api"),
            stages: vec![
                json!({ "stageName": "$default", "autoDeploy": true }),
                json!({ "stageName": "prod" }),
            ],
        },
        Row {
            case: "a named stage carrying route settings and stage variables, beside two others",
            api: http_api("configured-stage-api"),
            stages: vec![
                json!({
                    "stageName": "pre-prod_v2",
                    "description": "the stage under test",
                    "routeSettings": { "GET /items": { "detailedMetricsEnabled": true } },
                    "stageVariables": { "table_name": "items-v1" }
                }),
                json!({ "stageName": "staging" }),
                json!({ "stageName": "$default" }),
            ],
        },
        Row {
            case: "the only stage an api holds",
            api: http_api("single-stage-api"),
            stages: vec![json!({ "stageName": "only" })],
        },
        Row {
            case: "a stage name at the upper bound of its shape: 128 characters",
            api: http_api("long-stage-name-api"),
            stages: vec![json!({ "stageName": "s".repeat(MAX_STAGE_NAME_LEN) })],
        },
        Row {
            case: "a stage of a WebSocket api, which also carries a client certificate",
            api: websocket_api("socket-stage-api"),
            stages: vec![
                json!({ "stageName": "dev", "clientCertificateId": "abcd1234ef" }),
                json!({ "stageName": "$default" }),
            ],
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

/// Create one stage under `api_id` and answer the `StageName` it is keyed by.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    as_str(&created, "stageName")
}

/// Send the delete, with no assertion about its answer: the cases below make their own.
async fn delete_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    stage_name: &str,
) -> (u16, Value) {
    call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await
}

/// The stage names `GetStages` reports for one api.
async fn stage_names(emulator: &MultiServiceRouter, case: &str, api_id: &str) -> Vec<String> {
    let (status, listed) = call(emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStages, got {status} {listed}"
    );
    listed
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|stage| as_str(stage, "stageName"))
        .collect()
}

#[tokio::test]
async fn delete_stage_removes_that_stage_and_leaves_every_other_one() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let api_id = create_api(&emulator, case, &row.api).await;
        let mut created: Vec<String> = Vec::new();
        for stage in &row.stages {
            created.push(create_stage(&emulator, case, &api_id, stage).await);
        }
        let deleted = created[0].clone();
        let kept: Vec<String> = created[1..].to_vec();

        let (status, body) = delete_stage(&emulator, &api_id, &deleted).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204 from DeleteStage for {deleted:?}, got {status} {body}"
        );
        assert_eq!(
            body,
            Value::Null,
            "case {case:?}: a 204 carries no body, got {body}"
        );

        // The stage is gone, from its own read and from the list.
        let (status, read) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{deleted}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetStage for the deleted {deleted:?}, got {status} \
             {read}"
        );
        assert_eq!(
            error_code(&read),
            "NotFoundException",
            "case {case:?}: expected NotFoundException for the deleted {deleted:?}, got {read}"
        );
        assert_eq!(
            as_str(&read, "message"),
            invalid_identifier_message(STAGE_RESOURCE),
            "case {case:?}: expected the refusal this service words for a stage that resolved \
             to nothing, got {read}"
        );

        let mut remaining = stage_names(&emulator, case, &api_id).await;
        remaining.sort();
        let mut expected = kept.clone();
        expected.sort();
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected only the stages the delete did not name to remain, got \
             {remaining:?}"
        );

        // A SECOND delete of the same stage removes nothing again: the stage it names is
        // not there.
        let (status, again) = delete_stage(&emulator, &api_id, &deleted).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from a second DeleteStage of {deleted:?}, got {status} \
             {again}"
        );
        assert_eq!(
            error_code(&again),
            "NotFoundException",
            "case {case:?}: expected NotFoundException from the second delete, got {again}"
        );

        // Every stage the delete did not name is readable, unchanged.
        for stage_name in &kept {
            let (status, read) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/stages/{stage_name}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the untouched stage {stage_name:?} to still be there, \
                 got {status} {read}"
            );
        }
    }
}

#[tokio::test]
async fn delete_stage_refuses_an_identifier_that_resolves_to_nothing() {
    let emulator = emulator();
    let api_id = create_api(&emulator, "the api under test", &http_api("refusal-api")).await;
    create_stage(
        &emulator,
        "the api under test",
        &api_id,
        &json!({ "stageName": "prod" }),
    )
    .await;
    // A second api holding a stage of the SAME name: a stage is addressed within its own
    // api, so the pair is what resolves, never the name alone.
    let other_api = create_api(&emulator, "another api", &http_api("other-refusal-api")).await;
    create_stage(
        &emulator,
        "another api",
        &other_api,
        &json!({ "stageName": "prod" }),
    )
    .await;

    let unknown_api = "zzzzzzzzzz";
    let cases: &[(&str, &str, &str, String)] = &[
        (
            "an api id no api carries",
            unknown_api,
            "prod",
            invalid_api_identifier_message(unknown_api),
        ),
        (
            "a stage name this api holds no stage under",
            &api_id,
            "nosuchstage",
            invalid_identifier_message(STAGE_RESOURCE),
        ),
    ];
    for (case, api, stage_name, message) in cases {
        let (status, body) = delete_stage(&emulator, api, stage_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            *message,
            "case {case:?}: expected the refusal to be worded {message:?}, got {body}"
        );
    }

    // Neither refusal removed anything: both stages are still there.
    for (label, api) in [("the api under test", &api_id), ("another api", &other_api)] {
        assert_eq!(
            stage_names(&emulator, label, api).await,
            vec!["prod".to_string()],
            "case {label:?}: a refused delete must remove nothing"
        );
    }
}

#[tokio::test]
async fn deleting_a_stage_keeps_the_deployment_it_served() {
    let emulator = emulator();
    let case = "a stage serving a deployment";
    let api_id = create_api(&emulator, case, &http_api("deployed-stage-api")).await;
    let stage_name = create_stage(&emulator, case, &api_id, &json!({ "stageName": "prod" })).await;
    let (status, deployment) = call(
        &emulator,
        "POST",
        &format!("/v2/apis/{api_id}/deployments"),
        Some(json!({ "stageName": stage_name })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateDeployment, got {status} {deployment}"
    );
    let deployment_id = as_str(&deployment, "deploymentId");

    let (status, body) = delete_stage(&emulator, &api_id, &stage_name).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from DeleteStage, got {status} {body}"
    );

    // The deployment is the api's own record, so it outlives the stage it was released to.
    let (status, read) = call(
        &emulator,
        "GET",
        &format!("/v2/apis/{api_id}/deployments/{deployment_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the deployment the deleted stage served to remain, got \
         {status} {read}"
    );
}

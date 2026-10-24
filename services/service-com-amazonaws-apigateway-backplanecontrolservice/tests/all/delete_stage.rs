// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteStage`
//! (`DELETE /restapis/{restApiId}/stages/{stageName}`), read back through `GetStage`,
//! `FlushStageCache`, `GetTags` and `GetDeployment`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `CreateStage` / `FlushStageCache` / `GetTags`
//! helpers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `stage_lifecycle`, `flush_stage_cache` and `tag_reads_by_arn`.
//!
//! The rule under test, over a TABLE of stages rather than one recorded call: a delete
//! removes the ONE record the pair `(restApiId, stageName)` names, so every later read of
//! it answers the modelled `NotFoundException` (404) and its name is free again, while
//! every record the delete does not name — the API's other stages, its deployments, and a
//! same-named stage of another API — is untouched. The rows differ along what the rule
//! generalises over: how many stages the API carries and which of them is deleted (the
//! only one, the first, the middle, the last), which call created the deleted stage
//! (`CreateStage` or `CreateDeployment`'s own `stageName`), which optional members it
//! carries (a cache cluster, tags, variables, a canary, a documentation version) and the
//! length of its name (a short one, and one at the published 128-character bound).

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    CACHE_CLUSTER_NOT_AVAILABLE, MAX_STAGE_NAME_CHARS,
};

use crate::flush_stage_cache::flush_cache;
use crate::rest_api_lifecycle::{assert_failure, Answer, Gateway};
use crate::stage_lifecycle::{api_with_a_method, create_deployment, get_stage};
// The ARN builder and the `GetTags` call are the `tag_reads_by_arn` suite's: one spelling
// of each, so two suites cannot address one resource differently.
use crate::tag_reads_by_arn::{apigateway_arn, get_tags};
// `GetDeployment` is `update_deployment`'s helper (it asserts the modelled 200 and
// answers the body), reused here to show a stage delete left the deployments alone.
use crate::update_deployment::get_deployment;

/// One stage a row builds: its name, and the extra `CreateStage` members it carries —
/// `None` when the stage is created by `CreateDeployment`'s own `stageName` instead.
type StageSpec = (String, Option<Value>);

/// `DeleteStage` for `stage_name` of `api`.
async fn delete_stage(gateway: &Gateway, api: &str, stage_name: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/stages/{stage_name}"),
            None,
        )
        .await
}

/// The ARN of one stage: the `/restapis/{restApiId}/stages/{stageName}` resource portion
/// the tagging surface resolves a stage under.
fn stage_arn(api: &str, stage_name: &str) -> String {
    apigateway_arn(&format!("/restapis/{api}/stages/{stage_name}"))
}

/// Create the stage `spec` names under `api` and answer the id of the deployment it
/// serves: the row's shared deployment for a `CreateStage` row, and the deployment
/// `CreateDeployment` mints for a row created that way.
async fn make_stage(
    gateway: &Gateway,
    api: &str,
    deployment: &str,
    spec: &StageSpec,
    case: &str,
) -> String {
    let (stage_name, extra) = spec;
    let Some(extra) = extra else {
        return create_deployment(
            gateway,
            api,
            &format!("{case} / CreateDeployment(stageName={stage_name})"),
            json!({ "stageName": stage_name }),
        )
        .await;
    };
    let mut request = json!({ "stageName": stage_name, "deploymentId": deployment });
    let source = extra
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: a row's extra CreateStage members are an object, got {extra:?}"));
    let target = request
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {case}: the CreateStage request is an object"));
    for (member, value) in source {
        target.insert(member.clone(), value.clone());
    }
    let answer = gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateStage({stage_name}), got {} with {:?}",
        answer.status, answer.body
    );
    deployment.to_string()
}

#[tokio::test]
async fn deleting_a_stage_removes_that_stage_and_leaves_every_other_record() {
    // Source of the name-length row: the `stageName` member's documentation — "Maximum
    // length is 128 characters" (`MAX_STAGE_NAME_CHARS`), so a name at the bound is
    // deleted the same way a short one is.
    let long_name = "a".repeat(MAX_STAGE_NAME_CHARS);
    let rows: Vec<(&str, Vec<StageSpec>, String)> = vec![
        (
            "the only stage of the API",
            vec![("prod".to_string(), Some(json!({})))],
            "prod".to_string(),
        ),
        (
            "the first of three stages",
            vec![
                (
                    "alpha".to_string(),
                    Some(json!({ "description": "the alpha stage" })),
                ),
                (
                    "beta".to_string(),
                    Some(json!({ "variables": { "table": "pets" } })),
                ),
                ("gamma".to_string(), None),
            ],
            "alpha".to_string(),
        ),
        (
            "the middle of three, carrying a cache cluster and tags",
            vec![
                ("alpha".to_string(), Some(json!({}))),
                (
                    "beta".to_string(),
                    Some(json!({
                        "cacheClusterEnabled": true,
                        "cacheClusterSize": "0.5",
                        "tags": { "team": "api" }
                    })),
                ),
                ("gamma".to_string(), None),
            ],
            "beta".to_string(),
        ),
        (
            "the last of three, created by CreateDeployment's stageName",
            vec![
                ("alpha".to_string(), Some(json!({}))),
                ("beta".to_string(), Some(json!({}))),
                ("gamma".to_string(), None),
            ],
            "gamma".to_string(),
        ),
        (
            "a stage carrying every optional member a create may set",
            vec![(
                "staged_1".to_string(),
                Some(json!({
                    "description": "the only stage",
                    "documentationVersion": "v1",
                    "variables": { "table": "pets" },
                    "tracingEnabled": true,
                    "tags": { "team": "api" },
                    "canarySettings": { "percentTraffic": 10.0 }
                })),
            )],
            "staged_1".to_string(),
        ),
        (
            "a stage whose name sits on the published length bound",
            vec![(long_name.clone(), Some(json!({})))],
            long_name.clone(),
        ),
    ];

    for (case, stages, target) in &rows {
        let gateway = Gateway::new();
        let (api, _) = api_with_a_method(&gateway, "stage-deletes").await;
        let shared = create_deployment(
            &gateway,
            &api,
            &format!("{case} / CreateDeployment(shared)"),
            json!({ "description": "the shared deployment" }),
        )
        .await;

        let mut deployments = vec![shared.clone()];
        for spec in stages {
            let served = make_stage(&gateway, &api, &shared, spec, case).await;
            if !deployments.contains(&served) {
                deployments.push(served);
            }
        }

        // What every stage the delete does NOT name reports before it: each must still
        // report exactly this afterwards. No survivor of any row carries a cache cluster,
        // so no member of a snapshot is one the clock moves on its own.
        let mut before: Vec<(String, Value)> = Vec::new();
        for (stage_name, _) in stages {
            let read = get_stage(&gateway, &api, stage_name).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetStage({stage_name}) before the delete, got {} with {:?}",
                read.status, read.body
            );
            if stage_name != target {
                before.push((stage_name.clone(), read.body));
            }
        }

        let deleted = delete_stage(&gateway, &api, target).await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: expected the modelled 202, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {case}: expected an empty body, got {:?}",
            deleted.body
        );

        // The record is gone for every reader that resolves a stage: the read, the
        // delete again, the cache flush addressed under it, and its tags.
        for (reader, answer) in [
            ("GetStage", get_stage(&gateway, &api, target).await),
            ("DeleteStage again", delete_stage(&gateway, &api, target).await),
            ("FlushStageCache", flush_cache(&gateway, &api, target).await),
            ("GetTags", get_tags(&gateway, &stage_arn(&api, target)).await),
        ] {
            assert_failure(
                &answer,
                404,
                "NotFoundException",
                &format!("{case} / {reader}({target})"),
            );
        }

        // Every stage the delete did not name reports exactly what it did before.
        for (stage_name, body) in &before {
            let read = get_stage(&gateway, &api, stage_name).await;
            assert_eq!(
                read.status, 200,
                "case {case}: {stage_name} must survive the delete, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                &read.body, body,
                "case {case}: {stage_name} must be untouched, expected {body:?}, got {:?}",
                read.body
            );
        }

        // A deployment is a record of the API in its own right, so the deployment the
        // deleted stage served outlives it — which is what lets a new stage point at it.
        for deployment in &deployments {
            let body = get_deployment(
                &gateway,
                &api,
                deployment,
                &format!("{case} / the deployment {deployment} must outlive the stage"),
            )
            .await;
            assert_eq!(
                body["id"].as_str(),
                Some(deployment.as_str()),
                "case {case}: expected the stored deployment {deployment} back, got {body:?}"
            );
        }

        // The deleted name is a name no stage of the API holds: creating it again is no
        // conflict, and the new record carries nothing of the old one.
        let recreated = gateway
            .call(
                "POST",
                &format!("/restapis/{api}/stages"),
                Some(json!({ "stageName": target, "deploymentId": shared })),
            )
            .await;
        assert_eq!(
            recreated.status, 201,
            "case {case}: expected the freed name to be creatable again, got {} with {:?}",
            recreated.status, recreated.body
        );
        for member in [
            "description",
            "documentationVersion",
            "variables",
            "tags",
            "canarySettings",
            "cacheClusterSize",
        ] {
            assert!(
                recreated.body[member].is_null(),
                "case {case}: the recreated {target} carries nothing of the old record, got {member} = {:?}",
                recreated.body[member]
            );
        }
        assert_eq!(
            recreated.body["cacheClusterStatus"].as_str(),
            Some(CACHE_CLUSTER_NOT_AVAILABLE),
            "case {case}: the recreated {target} has no cache cluster, got {:?}",
            recreated.body["cacheClusterStatus"]
        );
        let read_back = get_stage(&gateway, &api, target).await;
        assert_eq!(
            read_back.body, recreated.body,
            "case {case}: GetStage must report the recreated stage, got {:?} after {:?}",
            read_back.body, recreated.body
        );
    }
}

#[tokio::test]
async fn a_delete_the_request_cannot_resolve_is_a_not_found_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "stage-delete-notfound").await;
    let (other_api, _) = api_with_a_method(&gateway, "stage-delete-notfound-other").await;
    for (target_api, stage_name) in [
        (&api, "prod"),
        (&api, "beta"),
        (&other_api, "prod"),
    ] {
        create_deployment(
            &gateway,
            target_api,
            &format!("CreateDeployment(stageName={stage_name})"),
            json!({ "stageName": stage_name }),
        )
        .await;
    }
    let missing = "zzzzzzzzzz";

    // Rows span every way the pair `(restApiId, stageName)` fails to name a stored stage:
    // an unknown API, an unknown stage of a stored API, and a stage name stored only on
    // ANOTHER API — a stage is addressed by the pair, so neither half alone resolves it.
    let rows: Vec<(&str, String, String)> = vec![
        ("an unknown API", missing.to_string(), "prod".to_string()),
        (
            "an unknown stage of a stored API",
            api.clone(),
            missing.to_string(),
        ),
        (
            "a stage name stored only on another API",
            other_api.clone(),
            "beta".to_string(),
        ),
    ];
    for (case, target_api, stage_name) in &rows {
        assert_failure(
            &delete_stage(&gateway, target_api, stage_name).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // A refused delete removes nothing: every stage the rows named around is still read.
    for (case, target_api, stage_name) in [
        ("the stage of the first API", &api, "prod"),
        ("the second stage of the first API", &api, "beta"),
        ("the same-named stage of the other API", &other_api, "prod"),
    ] {
        let read = get_stage(&gateway, target_api, stage_name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: a refused delete changes nothing, got {} with {:?}",
            read.status, read.body
        );
    }

    // Deleting one of two same-named stages leaves the other: the pair is the address.
    let deleted = delete_stage(&gateway, &api, "prod").await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteStage(first API / prod): expected the modelled 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_stage(&gateway, &api, "prod").await,
        404,
        "NotFoundException",
        "GetStage(first API / prod) after the delete",
    );
    let survivor = get_stage(&gateway, &other_api, "prod").await;
    assert_eq!(
        survivor.status, 200,
        "case GetStage(other API / prod): the other API's stage of that name must survive, got {} with {:?}",
        survivor.status, survivor.body
    );

    // With the API gone there is no pair left to resolve, so the delete finds no stage.
    let api_deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        api_deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        api_deleted.status, api_deleted.body
    );
    assert_failure(
        &delete_stage(&gateway, &api, "beta").await,
        404,
        "NotFoundException",
        "DeleteStage after DeleteRestApi",
    );
}

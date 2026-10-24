// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDeployment`
//! (`DELETE /restapis/{restApiId}/deployments/{deploymentId}`), read back through
//! `GetDeployment`, `GetDeployments`, `UpdateDeployment`, `CreateStage` and `GetStage`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateResource` / `PutMethod` / `CreateDeployment` /
//! `GetDeployment(s)` / PATCH helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle`, `stage_lifecycle`, `deployment_collection` and `patch_updates`.
//!
//! The RULE under test, over a TABLE of APIs rather than one recorded call: a
//! `DeleteDeployment` removes the ONE stored deployment the pair
//! `(restApiId, deploymentId)` names — so every later read of it answers the modelled
//! `NotFoundException` (404) while every record the delete does not name is untouched —
//! and it is refused with the modelled `BadRequestException` (400), changing nothing,
//! while any stage of that API is still associated with the deployment, because "Deleting
//! a deployment will only succeed if there are no Stage resources associated with it"
//! (`smithy_query(query="operation", operation="DeleteDeployment")`).
//!
//! The rows differ along what the rule generalises over: how many deployments the API
//! carries and which of them is deleted (the only one, the first, the middle, the last),
//! which optional members the deleted record carries (a description absent, ASCII,
//! multibyte, and one at the published `MAX_STRING_CHARS` bound), how a stage comes to be
//! associated with a deployment (`CreateDeployment`'s own `stageName`, `CreateStage`'s
//! `deploymentId`, a canary's `deploymentId`, two stages at once), how many stages are
//! associated, how the association is cleared (deleting the stage, or re-pointing it with
//! `UpdateStage`), and every way the pair fails to name a stored deployment.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::ops::delete_deployment::DEPLOYMENT_IN_USE;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_STRING_CHARS, NO_SUCH_DEPLOYMENT,
};

use crate::deployment_collection::{get_deployment, get_deployments, reported_ids};
use crate::patch_updates::{patch, replace};
use crate::rest_api_lifecycle::{assert_failure, resource_paths, Answer, Gateway};
use crate::stage_lifecycle::{api_with_a_method, create_deployment, get_stage};

/// `DeleteDeployment` for `deployment` of `api`.
async fn delete_deployment(gateway: &Gateway, api: &str, deployment: &str) -> Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/deployments/{deployment}"),
            None,
        )
        .await
}

/// `CreateStage` for `api` with `request`, answering whatever the service said: a row
/// uses it both to build a stage and to show a deleted deployment is no longer a
/// deployment a stage may point at.
async fn create_stage(gateway: &Gateway, api: &str, request: Value) -> Answer {
    gateway
        .call("POST", &format!("/restapis/{api}/stages"), Some(request))
        .await
}

/// Create the stage `name` of `api` serving `deployment`, with `canary` as the deployment
/// its canary serves when the row asks for one, and assert the modelled 201.
async fn make_stage(
    gateway: &Gateway,
    api: &str,
    name: &str,
    deployment: &str,
    canary: Option<&str>,
    case: &str,
) {
    let mut request = json!({ "stageName": name, "deploymentId": deployment });
    if let Some(canary) = canary {
        request["canarySettings"] = json!({ "percentTraffic": 10.0, "deploymentId": canary });
    }
    let answer = create_stage(gateway, api, request).await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201 from CreateStage({name}), got {} with {:?}",
        answer.status, answer.body
    );
}

/// Every deployment of `api` the collection reports, in the order it reports them.
async fn listed_ids(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = get_deployments(gateway, api, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetDeployments, got {} with {:?}",
        answer.status, answer.body
    );
    reported_ids(&answer.body, case)
}

/// Read every deployment of `api` in `ids`, asserting each answers the modelled 200, and
/// answer the `(id, body)` pairs — the snapshot a later read must still match.
async fn snapshot(gateway: &Gateway, api: &str, ids: &[String], case: &str) -> Vec<(String, Value)> {
    let mut records = Vec::new();
    for id in ids {
        let read = get_deployment(gateway, api, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetDeployment({id}), got {} with {:?}",
            read.status, read.body
        );
        records.push((id.clone(), read.body));
    }
    records
}

/// Assert every record of `before` still reads back exactly as it did.
async fn assert_unchanged(gateway: &Gateway, api: &str, before: &[(String, Value)], case: &str) {
    for (id, body) in before {
        let read = get_deployment(gateway, api, id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the deployment {id} must survive, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            &read.body, body,
            "case {case}: the deployment {id} must be untouched, expected {body:?}, got {:?}",
            read.body
        );
    }
}

/// A description of `chars` ASCII characters, for the row that sits on a published bound.
fn filler(chars: usize) -> String {
    "d".repeat(chars)
}

#[tokio::test]
async fn deleting_a_deployment_removes_it_and_leaves_every_other_record() {
    // Source of the length row: `MAX_STRING_CHARS`, the bound this crate applies to a
    // caller-sized string member, so a description at the bound is deleted the same way
    // an absent one is.
    let rows: Vec<(&str, Vec<Value>, usize)> = vec![
        ("the only deployment of the API", vec![json!({})], 0),
        (
            "the first of three",
            vec![
                json!({ "description": "alpha" }),
                json!({ "description": "beta" }),
                json!({}),
            ],
            0,
        ),
        (
            "the middle of three",
            vec![
                json!({ "description": "alpha" }),
                json!({ "description": "beta" }),
                json!({ "description": "gamma" }),
            ],
            1,
        ),
        (
            "the last of three, the only one with no description",
            vec![
                json!({ "description": "alpha" }),
                json!({ "description": "beta" }),
                json!({}),
            ],
            2,
        ),
        (
            "a deployment whose description is multibyte",
            vec![json!({ "description": "déploiement 🐾 版" }), json!({})],
            0,
        ),
        (
            "a deployment whose description sits on the published length bound",
            vec![
                json!({}),
                json!({ "description": filler(MAX_STRING_CHARS) }),
            ],
            1,
        ),
    ];

    for (case, creates, target_index) in &rows {
        let gateway = Gateway::new();
        let (api, _) = api_with_a_method(&gateway, "deployment-deletes").await;
        // A second API carrying its own deployment: a deployment is addressed by the pair
        // `(restApiId, deploymentId)`, so nothing of another API may move.
        let (other_api, _) = api_with_a_method(&gateway, "deployment-deletes-other").await;
        let other_id = create_deployment(
            &gateway,
            &other_api,
            &format!("{case} / the other API's deployment"),
            json!({ "description": "the other API's deployment" }),
        )
        .await;

        let mut ids = Vec::new();
        for (n, request) in creates.iter().enumerate() {
            ids.push(
                create_deployment(
                    &gateway,
                    &api,
                    &format!("{case} / CreateDeployment #{n}"),
                    request.clone(),
                )
                .await,
            );
        }
        let target = ids[*target_index].clone();
        let survivors: Vec<String> = ids.iter().filter(|id| *id != &target).cloned().collect();

        // What every record the delete does NOT name reports beforehand; each must report
        // exactly this afterwards. No member of a deployment is one the clock moves on its
        // own, so these snapshots are stable reads.
        let before = snapshot(&gateway, &api, &survivors, case).await;
        let other_before =
            snapshot(&gateway, &other_api, std::slice::from_ref(&other_id), case).await;
        let paths_before = resource_paths(&gateway, &api).await;
        let listed_before = listed_ids(&gateway, &api, case).await;
        assert_eq!(
            listed_before, ids,
            "case {case}: the collection reports every created deployment in creation order before the delete, got {listed_before:?}"
        );

        let deleted = delete_deployment(&gateway, &api, &target).await;
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

        // The record is gone for every reader that resolves a deployment: the read, the
        // delete again, the patch, and a stage asked to point at it.
        for (reader, answer) in [
            ("GetDeployment", get_deployment(&gateway, &api, &target).await),
            (
                "DeleteDeployment again",
                delete_deployment(&gateway, &api, &target).await,
            ),
            (
                "UpdateDeployment",
                patch(
                    &gateway,
                    &format!("/restapis/{api}/deployments/{target}"),
                    vec![replace("/description", "patched after the delete")],
                )
                .await,
            ),
            (
                "CreateStage pointing at it",
                create_stage(
                    &gateway,
                    &api,
                    json!({ "stageName": "prod", "deploymentId": target }),
                )
                .await,
            ),
        ] {
            let case = format!("{case} / {reader}({target})");
            assert_failure(&answer, 404, "NotFoundException", &case);
            // One sentence for one failure: every operation that resolves a deployment
            // says the same thing about an id that names none (`NO_SUCH_DEPLOYMENT`).
            assert_eq!(
                answer.body["message"].as_str(),
                Some(NO_SUCH_DEPLOYMENT),
                "case {case}: expected {NO_SUCH_DEPLOYMENT:?}, got {:?}",
                answer.body["message"]
            );
        }

        // Every record the delete did not name is exactly as it was: this API's other
        // deployments, the collection that lists them, the live resources the snapshot was
        // taken from, and the other API's deployment.
        assert_unchanged(&gateway, &api, &before, case).await;
        assert_unchanged(&gateway, &other_api, &other_before, case).await;
        let listed_after = listed_ids(&gateway, &api, case).await;
        assert_eq!(
            listed_after, survivors,
            "case {case}: expected the collection to report {survivors:?} in creation order, got {listed_after:?}"
        );
        let paths_after = resource_paths(&gateway, &api).await;
        assert_eq!(
            paths_after, paths_before,
            "case {case}: a deployment delete leaves the API's resources alone, expected {paths_before:?}, got {paths_after:?}"
        );
    }
}

/// How a row clears the stage association that blocks its delete.
#[derive(Debug)]
enum Clear {
    /// Delete the named stages, so no stage names the deployment any more.
    DeleteStages(&'static [&'static str]),
    /// Re-point the stage's `deploymentId` at the deployment at this index.
    Repoint(&'static str, usize),
}

/// One refusal row: the case name, the `CreateDeployment` requests to issue, the stages
/// to create as `(stageName, the deployment it serves, the deployment its canary serves)`
/// indices into those deployments, the index of the deployment the row tries to delete,
/// and how it clears the association afterwards.
type InUseRow = (
    &'static str,
    Vec<Value>,
    Vec<(&'static str, usize, Option<usize>)>,
    usize,
    Clear,
);

#[tokio::test]
async fn a_deployment_a_stage_is_associated_with_is_refused_until_the_association_is_cleared() {
    // Rows span every way a stage of the API comes to be associated with a deployment:
    // the `stageName` of the `CreateDeployment` that created it, a `CreateStage`'s
    // `deploymentId`, the `deploymentId` of a stage's canary, and two stages at once —
    // and both ways a caller clears it.
    let rows: Vec<InUseRow> = vec![
        (
            "the deployment the stage CreateDeployment's own stageName created serves",
            vec![json!({ "stageName": "prod" })],
            vec![],
            0,
            Clear::DeleteStages(&["prod"]),
        ),
        (
            "the deployment a CreateStage stage serves, cleared by re-pointing it",
            vec![json!({}), json!({ "description": "the next deployment" })],
            vec![("prod", 0, None)],
            0,
            Clear::Repoint("prod", 1),
        ),
        (
            "the deployment a stage's CANARY serves",
            vec![json!({}), json!({ "description": "the canary's deployment" })],
            vec![("prod", 0, Some(1))],
            1,
            Clear::DeleteStages(&["prod"]),
        ),
        (
            "a deployment two stages serve",
            vec![json!({})],
            vec![("prod", 0, None), ("beta", 0, None)],
            0,
            Clear::DeleteStages(&["prod", "beta"]),
        ),
        (
            "the last of three deployments, served by one of two stages",
            vec![json!({}), json!({}), json!({})],
            vec![("alpha", 0, None), ("prod", 2, None)],
            2,
            Clear::DeleteStages(&["prod"]),
        ),
    ];

    for (case, creates, stages, target_index, clear) in &rows {
        let gateway = Gateway::new();
        let (api, _) = api_with_a_method(&gateway, "deployment-delete-in-use").await;
        let mut ids = Vec::new();
        for (n, request) in creates.iter().enumerate() {
            ids.push(
                create_deployment(
                    &gateway,
                    &api,
                    &format!("{case} / CreateDeployment #{n}"),
                    request.clone(),
                )
                .await,
            );
        }
        for (name, serves, canary) in stages {
            make_stage(
                &gateway,
                &api,
                name,
                &ids[*serves],
                canary.map(|index| ids[index].as_str()),
                case,
            )
            .await;
        }
        let target = ids[*target_index].clone();

        // The refusal is the modelled `BadRequestException` (400) and changes nothing:
        // every deployment of the API, and every stage, reads back as it did.
        let before = snapshot(&gateway, &api, &ids, case).await;
        let refused = delete_deployment(&gateway, &api, &target).await;
        assert_failure(&refused, 400, "BadRequestException", case);
        assert_eq!(
            refused.body["message"].as_str(),
            Some(DEPLOYMENT_IN_USE),
            "case {case}: expected the documented refusal {DEPLOYMENT_IN_USE:?}, got {:?}",
            refused.body["message"]
        );
        assert_unchanged(&gateway, &api, &before, case).await;
        for (name, _, _) in stages {
            let read = get_stage(&gateway, &api, name).await;
            assert_eq!(
                read.status, 200,
                "case {case}: a refused delete leaves the stage {name} alone, got {} with {:?}",
                read.status, read.body
            );
        }

        // With the association cleared the SAME call succeeds, which is what makes the
        // refusal a rule about the stages and not about the deployment.
        match clear {
            Clear::DeleteStages(names) => {
                for name in *names {
                    let answer = gateway
                        .call("DELETE", &format!("/restapis/{api}/stages/{name}"), None)
                        .await;
                    assert_eq!(
                        answer.status, 202,
                        "case {case}: expected 202 from DeleteStage({name}), got {} with {:?}",
                        answer.status, answer.body
                    );
                }
            }
            Clear::Repoint(name, to) => {
                let answer = patch(
                    &gateway,
                    &format!("/restapis/{api}/stages/{name}"),
                    vec![replace("/deploymentId", &ids[*to])],
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200 from UpdateStage({name}), got {} with {:?}",
                    answer.status, answer.body
                );
            }
        }
        let deleted = delete_deployment(&gateway, &api, &target).await;
        assert_eq!(
            deleted.status, 202,
            "case {case}: once no stage is associated the delete succeeds, got {} with {:?}",
            deleted.status, deleted.body
        );
        assert_failure(
            &get_deployment(&gateway, &api, &target).await,
            404,
            "NotFoundException",
            &format!("{case} / GetDeployment after the cleared delete"),
        );
        // Every other deployment of the API outlived both calls.
        let remaining: Vec<(String, Value)> = before
            .into_iter()
            .filter(|(id, _)| id != &target)
            .collect();
        assert_unchanged(&gateway, &api, &remaining, case).await;
    }
}

#[tokio::test]
async fn a_delete_the_request_cannot_resolve_is_a_not_found_and_changes_nothing() {
    let gateway = Gateway::new();
    let (api, _) = api_with_a_method(&gateway, "deployment-delete-notfound").await;
    let (other_api, _) = api_with_a_method(&gateway, "deployment-delete-notfound-other").await;
    let mine = create_deployment(
        &gateway,
        &api,
        "the first API's deployment",
        json!({ "description": "mine" }),
    )
    .await;
    let theirs = create_deployment(
        &gateway,
        &other_api,
        "the other API's deployment",
        json!({ "description": "theirs" }),
    )
    .await;
    let missing = "zzzzzzzzzz";

    // Rows span every way the pair `(restApiId, deploymentId)` fails to name a stored
    // deployment: an unknown API, an unknown deployment of a stored API, and a deployment
    // id stored only on ANOTHER API — neither half of the pair alone resolves a record.
    let rows: Vec<(&str, String, String)> = vec![
        ("an unknown API", missing.to_string(), mine.clone()),
        (
            "an unknown deployment of a stored API",
            api.clone(),
            missing.to_string(),
        ),
        (
            "a deployment id stored only on another API",
            api.clone(),
            theirs.clone(),
        ),
        (
            "the first API's deployment id against the other API",
            other_api.clone(),
            mine.clone(),
        ),
    ];
    for (case, target_api, deployment) in &rows {
        assert_failure(
            &delete_deployment(&gateway, target_api, deployment).await,
            404,
            "NotFoundException",
            case,
        );
    }

    // A refused delete removes nothing: both deployments the rows named around still read.
    for (case, target_api, deployment) in [
        ("the first API's deployment", &api, &mine),
        ("the other API's deployment", &other_api, &theirs),
    ] {
        let read = get_deployment(&gateway, target_api, deployment).await;
        assert_eq!(
            read.status, 200,
            "case {case}: a refused delete changes nothing, got {} with {:?}",
            read.status, read.body
        );
    }

    // With the API gone there is no pair left to resolve, so the delete finds no
    // deployment — and the other API's deployment is untouched by either call.
    let api_deleted = gateway
        .call("DELETE", &format!("/restapis/{api}"), None)
        .await;
    assert_eq!(
        api_deleted.status, 202,
        "case DeleteRestApi: expected 202, got {} with {:?}",
        api_deleted.status, api_deleted.body
    );
    assert_failure(
        &delete_deployment(&gateway, &api, &mine).await,
        404,
        "NotFoundException",
        "DeleteDeployment after DeleteRestApi",
    );
    let survivor = get_deployment(&gateway, &other_api, &theirs).await;
    assert_eq!(
        survivor.status, 200,
        "case GetDeployment(other API): the other API's deployment must survive, got {} with {:?}",
        survivor.status, survivor.body
    );
}

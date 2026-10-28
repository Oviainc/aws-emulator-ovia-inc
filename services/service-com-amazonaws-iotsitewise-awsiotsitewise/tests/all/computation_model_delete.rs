// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteComputationModel`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete removes EXACTLY the computation model the request addresses,
//!   together with everything this store holds only as a record of it (the
//!   actions taken on it and the executions those actions started), and answers
//!   that model's lifecycle state — so a later read of it resolves to nothing,
//!   it leaves the listing, its ARN stops carrying tags, and every other model,
//!   with its own actions and executions, is left exactly as it was;
//! * a `computationModelId` that resolves to no stored model — never created, or
//!   already deleted — is the modelled 404 naming the identifier the REQUEST
//!   supplied, and removes nothing;
//! * a retried delete carrying the `clientToken` of one that already succeeded
//!   replays that answer rather than the not-found refusal the removal would now
//!   draw, while a DIFFERENT request reusing that token is the modelled 409
//!   naming the model the first call removed, and removes nothing.
//!
//! The rows differ along what the rules generalise over: the model's name and
//! description, the form and size of its data binding, whether it carries tags,
//! whether the delete carries a `clientToken`, whether the model had settled the
//! transition its create owes, and how many actions had been taken on it.
//!
//! The HTTP harness (`call`, `error_code`, `router_with_engine`) is the one the
//! asset suite already has, and the computation-model bodies (`create`,
//! `describe`, `anomaly_detection`, the binding-value forms, `expected_arn`,
//! `id_of`) are the ones the sibling computation-model suites already define;
//! this suite reuses both rather than growing a second copy.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, ACTIVATION_DELAY};

use crate::asset_lifecycle::{call, error_code, router_with_engine};
use crate::computation_model_lifecycle::{
    anomaly_detection, asset_model_property, asset_property, binding_list, create, describe,
    expected_arn, id_of,
};

/// Long enough that a transition the clock owes has certainly fallen due.
fn settle() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// `DELETE /computation-models/{computationModelId}`, with `clientToken` on the
/// query string as the model binds it (`smithy_query(query="operation",
/// operation="DeleteComputationModel")`).
async fn delete_model(
    router: &MultiServiceRouter,
    id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let uri = match client_token {
        Some(token) => format!("/computation-models/{id}?clientToken={token}"),
        None => format!("/computation-models/{id}"),
    };
    call(router, "DELETE", &uri, None).await
}

/// `ListTagsForResource` on `arn` — the reader that proves a deleted model's ARN
/// stops resolving.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// The `computationModelId`s `ListComputationModels` reports, sorted.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", "/computation-models", None).await;
    assert_eq!(status, 200, "ListComputationModels got {status} {body}");
    let mut ids: Vec<String> = body["computationModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("computationModelSummaries must be a list, got {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or("").to_string())
        .collect();
    ids.sort();
    ids
}

/// The `computationModelId`s the store holds, sorted — the record a delete has
/// to have removed from.
fn stored_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let mut ids: Vec<String> = state.lock().computation_models.keys().cloned().collect();
    ids.sort();
    ids
}

/// How many actions and executions the store holds against `computation_model_id`
/// — the records a model's delete has to take with it, which no reader of a
/// deleted model can enumerate any more.
fn recorded_against(engine: &engine::InMemoryEngine, computation_model_id: &str) -> (usize, usize) {
    let state = service_state(engine).expect("the IoT SiteWise store");
    let store = state.lock();
    // The target is read off the record's own `targetResource` rather than
    // through the handler's resolver, so this count is an INDEPENDENT oracle.
    let actions = store
        .actions
        .values()
        .filter(|action| {
            action.target_resource.computation_model_id.as_deref() == Some(computation_model_id)
        })
        .count();
    let executions = store
        .executions
        .values()
        .filter(|execution| {
            execution.target_resource.computation_model_id.as_deref() == Some(computation_model_id)
        })
        .count();
    (actions, executions)
}

/// A `DescribeComputationModel` body without the lifecycle status the clock moves
/// on its own, so two reads taken at different instants are comparable.
fn without_status(body: &Value) -> Value {
    let mut copy = body.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("computationModelStatus");
    }
    copy
}

/// The state a `computationModelStatus` reports.
fn state_of(body: &Value) -> &str {
    body["computationModelStatus"]["state"].as_str().unwrap_or("")
}

/// The model every row keeps beside the one it deletes, so each assertion about
/// what a delete removed is measured against what it had to leave alone.
fn neighbour_body() -> Value {
    json!({
        "computationModelName": "the-model-that-stays",
        "computationModelDescription": "Left alone by every delete",
        "computationModelConfiguration": anomaly_detection("input", "result"),
        "computationModelDataBinding": {
            "input": asset_model_property(&id_of('1'), &id_of('2')),
            "result": asset_model_property(&id_of('1'), &id_of('3')),
        },
        "tags": { "keep": "me" },
    })
}

/// Create a computation model from `body` and answer the id it minted.
async fn create_model(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let (status, created) = create(router, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateComputationModel got {status} {created}"
    );
    created["computationModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no computationModelId in {created}"))
        .to_string()
}

/// One row of the delete table.
struct DeleteCase {
    label: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    tags: Option<Value>,
    /// The configuration and data binding the model is created with, so the rows
    /// span the binding forms `ComputationModelDataBindingValue` carries and
    /// bindings of different sizes.
    configuration: Value,
    binding: Value,
    /// The `clientToken` the delete carries, if any: `ClientToken` is len 36..64,
    /// pattern `^\S{36,64}$`.
    client_token: Option<&'static str>,
    /// Whether the row waits for the transition the create owes before deleting,
    /// so the table covers a model deleted while it is still settling its create
    /// and one deleted once it has settled.
    settled: bool,
}

/// The rows.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "a one-character name, no description, no tags, still CREATING",
            name: "m",
            description: None,
            tags: None,
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            }),
            client_token: None,
            settled: false,
        },
        DeleteCase {
            label: "a described and tagged model, bound to asset properties, settled",
            name: "Pump 4 anomaly #1",
            description: Some("Detects anomalies in pump 4"),
            tags: Some(json!({ "line": "packaging", "owner": "controls" })),
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_property(&id_of('4'), &id_of('5')),
                "result": asset_property(&id_of('4'), &id_of('6')),
            }),
            client_token: Some("11111111-2222-3333-4444-555555555555"),
            settled: true,
        },
        DeleteCase {
            label: "a list binding and a variable nothing references, deleted under a token",
            name: "fleet_anomaly-2024",
            description: Some("Fleet wide model"),
            tags: Some(json!({ "only": "one" })),
            configuration: anomaly_detection("inputs_0", "result_value"),
            binding: json!({
                "inputs_0": binding_list(vec![
                    asset_property(&id_of('7'), &id_of('8')),
                    asset_model_property(&id_of('9'), &id_of('a')),
                ]),
                "result_value": asset_property(&id_of('7'), &id_of('b')),
                "spare": asset_model_property(&id_of('9'), &id_of('c')),
            }),
            client_token: Some("0123456789abcdef0123456789abcdef0123"),
            settled: true,
        },
        DeleteCase {
            label: "a name at the modelled maximum, still CREATING, under a token",
            name: "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\
                   nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\
                   nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn\
                   nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn",
            description: None,
            tags: None,
            configuration: anomaly_detection("input", "result"),
            binding: json!({
                "input": asset_model_property(&id_of('d'), &id_of('e')),
                "result": binding_list(vec![asset_model_property(&id_of('d'), &id_of('f'))]),
            }),
            client_token: Some("deletes-the-longest-named-model-aaaa"),
            settled: false,
        },
    ]
}

/// The create body a row sends.
fn create_body(case: &DeleteCase) -> Value {
    let mut body = json!({
        "computationModelName": case.name,
        "computationModelConfiguration": case.configuration.clone(),
        "computationModelDataBinding": case.binding.clone(),
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert(
            "computationModelDescription".to_string(),
            json!(description),
        );
    }
    if let Some(tags) = case.tags.as_ref() {
        object.insert("tags".to_string(), tags.clone());
    }
    body
}

#[tokio::test]
async fn a_delete_removes_the_addressed_model_and_leaves_every_other_alone() {
    for case in delete_cases() {
        let label = case.label;
        // `RestrictedName` is len 1..256, so the longest row has to stay inside
        // it: the row is about the bound, not about a request the boundary
        // validator would refuse before any handler runs.
        assert!(
            (1..=256).contains(&case.name.chars().count()),
            "case {label:?}: a computationModelName is 1..256 characters, built {} of them",
            case.name.chars().count()
        );

        let (router, engine) = router_with_engine();
        let neighbour = create_model(&router, neighbour_body(), label).await;
        let id = create_model(&router, create_body(&case), label).await;

        if case.settled {
            tokio::time::sleep(settle()).await;
        }

        // The model reads back before the delete, so what the delete removed is
        // measured against what was really stored.
        let (status, before) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read before the delete got {status} {before}"
        );
        let arn = expected_arn(&id);
        assert_eq!(
            before["computationModelArn"].as_str().unwrap_or(""),
            arn,
            "case {label:?}: unexpected computationModelArn in {before}"
        );
        let (status, tags_before) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the model's ARN must resolve before the delete, got {status} \
             {tags_before}"
        );
        let neighbour_before = describe(&router, &neighbour).await.1;

        let (status, answer) = delete_model(&router, &id, case.client_token).await;
        assert_eq!(
            status, 202,
            "case {label:?}: DeleteComputationModel expected 202, got {status} {answer}"
        );
        // "The status of the computation model. It contains a state (DELETING
        // after successfully calling this operation) and any error messages."
        assert_eq!(
            state_of(&answer),
            "DELETING",
            "case {label:?}: an accepted delete reports {answer}"
        );
        assert!(
            answer["computationModelStatus"]["error"].is_null(),
            "case {label:?}: nothing failed, so the status carries no error: {answer}"
        );

        // The model is gone from its reader, from the listing and from the store.
        let (status, after) = describe(&router, &id).await;
        assert_eq!(
            (status, error_code(&after)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: the read after the delete must be a modelled 404, got {status} \
             {after}"
        );
        assert!(
            after["message"].as_str().unwrap_or("").contains(&id),
            "case {label:?}: the refusal must name the id the request addressed, got {after}"
        );
        assert_eq!(
            listed_ids(&router).await,
            vec![neighbour.clone()],
            "case {label:?}: the listing must hold only the neighbour model"
        );
        assert_eq!(
            stored_ids(&engine),
            vec![neighbour.clone()],
            "case {label:?}: the store must hold only the neighbour model"
        );

        // Its ARN stops resolving: the tags it carried went with the record.
        let (status, tags_after) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&tags_after)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: the deleted model's ARN must stop resolving, got {status} \
             {tags_after}"
        );

        // The neighbour model itself is untouched.
        let (status, neighbour_after) = describe(&router, &neighbour).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the neighbour model expected 200, got {status} {neighbour_after}"
        );
        assert_eq!(
            without_status(&neighbour_after),
            without_status(&neighbour_before),
            "case {label:?}: the delete must leave the neighbour model exactly as it was, got \
             {neighbour_after}"
        );

        // A second delete of the same model has nothing left to address: the
        // model is gone, so this is the same refusal an id no create made draws.
        let (status, again) = delete_model(&router, &id, None).await;
        assert_eq!(
            (status, error_code(&again)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: deleting the same model twice must be a modelled 404, got {status} \
             {again}"
        );
    }
}

#[tokio::test]
async fn deleting_a_computation_model_no_create_made_is_a_modelled_refusal() {
    let (router, engine) = router_with_engine();

    // A token a refused delete carried is NOT spent, so it still serves a later
    // call: the refusal removed nothing.
    let unspent = "a-token-no-refused-delete-can-spend";
    let token = format!("{unspent:-<36}");

    // Every row is a modelled `ID` the store holds no model under, so the id the
    // request addresses — and whether it carries a token — is all that varies.
    for (unknown, carried) in [
        (id_of('e'), None),
        (id_of('d'), Some(token.as_str())),
        (id_of('7'), None),
    ] {
        let (status, body) = delete_model(&router, &unknown, carried).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {unknown:?}: deleting an unknown model answered {status} {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(&unknown),
            "case {unknown:?}: the refusal does not name the requested id: {body}"
        );
    }

    assert!(
        stored_ids(&engine).is_empty(),
        "a refused delete wrote a computation model: {:?}",
        stored_ids(&engine)
    );

    // The token those refusals carried serves a delete of a model that IS there.
    let id = create_model(&router, neighbour_body(), "the unspent token").await;
    let (status, answer) = delete_model(&router, &id, Some(&token)).await;
    assert_eq!(
        status, 202,
        "the delete under the token a refusal carried answered {status} {answer}"
    );
    assert_eq!(
        state_of(&answer),
        "DELETING",
        "the accepted delete reports {answer}"
    );
}

/// The `(actionType, actionDefinitionId)` pairs `DescribeComputationModel`
/// reports for `id`, sorted so a row can address them by position.
async fn published_actions(router: &MultiServiceRouter, id: &str) -> Vec<(String, String)> {
    let (status, described) = describe(router, id).await;
    assert_eq!(status, 200, "describe {id}: got {status} {described}");
    let mut published: Vec<(String, String)> = described["actionDefinitions"]
        .as_array()
        .unwrap_or_else(|| panic!("no actionDefinitions in {described}"))
        .iter()
        .map(|definition| {
            (
                definition["actionType"].as_str().unwrap_or("").to_string(),
                definition["actionDefinitionId"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    published.sort();
    published
}

/// `ExecuteAction` of one of the actions `computation_model_id` publishes, and
/// the `actionId` it minted.
async fn execute_action(
    router: &MultiServiceRouter,
    computation_model_id: &str,
    action_definition_id: &str,
    payload: &str,
) -> String {
    let (status, answered) = call(
        router,
        "POST",
        "/actions",
        Some(json!({
            "targetResource": { "computationModelId": computation_model_id },
            "actionDefinitionId": action_definition_id,
            "actionPayload": { "stringValue": payload },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "ExecuteAction on {computation_model_id}: got {status} {answered}"
    );
    answered["actionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no actionId in {answered}"))
        .to_string()
}

/// `ListActions` scoped to one computation model.
async fn list_actions(router: &MultiServiceRouter, computation_model_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/actions?targetResourceType=COMPUTATION_MODEL&\
             targetResourceId={computation_model_id}"
        ),
        None,
    )
    .await
}

/// `ListExecutions` scoped to one computation model.
async fn list_executions(router: &MultiServiceRouter, computation_model_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/executions?targetResourceType=COMPUTATION_MODEL&\
             targetResourceId={computation_model_id}"
        ),
        None,
    )
    .await
}

#[tokio::test]
async fn a_delete_takes_the_actions_and_executions_recorded_on_the_model_with_it() {
    let (router, engine) = router_with_engine();
    let doomed = create_model(&router, neighbour_body(), "the deleted model").await;
    let kept = create_model(
        &router,
        json!({
            "computationModelName": "records-of-its-own",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        }),
        "the kept model",
    )
    .await;

    // An action is taken on a SETTLED resource, so the transient-state refusal
    // `ExecuteAction`'s own suite covers is not what these calls run into.
    tokio::time::sleep(settle()).await;

    // The rows differ in HOW MANY actions were taken on the model, and on which
    // of the actions its computation type publishes: the rule holds for a model
    // nothing has run and for one with several runs recorded against it.
    let doomed_definitions = published_actions(&router, &doomed).await;
    let mut doomed_actions = Vec::new();
    for (at, payload) in [(0usize, "{}"), (1, "x"), (0, "{\"window\":300}")] {
        let definition = &doomed_definitions
            .get(at)
            .unwrap_or_else(|| panic!("the model publishes no action at {at}"))
            .1;
        doomed_actions.push(execute_action(&router, &doomed, definition, payload).await);
    }
    let kept_definitions = published_actions(&router, &kept).await;
    let kept_action = execute_action(&router, &kept, &kept_definitions[0].1, "{}").await;

    assert_eq!(
        recorded_against(&engine, &doomed),
        (doomed_actions.len(), doomed_actions.len()),
        "each action taken on the doomed model records one action and one execution"
    );
    assert_eq!(
        recorded_against(&engine, &kept),
        (1, 1),
        "the kept model holds the one action taken on it"
    );

    let (status, answer) = delete_model(&router, &doomed, None).await;
    assert_eq!(status, 202, "the delete answered {status} {answer}");

    // Nothing is left for a listing to report, and nothing of the model remains
    // in the store for the per-store bound on actions to count.
    assert_eq!(
        recorded_against(&engine, &doomed),
        (0, 0),
        "the delete must take the model's actions and executions with it"
    );
    for (reader, listed) in [
        ("ListActions", list_actions(&router, &doomed).await),
        ("ListExecutions", list_executions(&router, &doomed).await),
    ] {
        let (status, body) = listed;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {reader:?}: a listing scoped to the deleted model answered {status} {body}"
        );
    }
    for action in &doomed_actions {
        let (status, body) = call(&router, "GET", &format!("/actions/{action}"), None).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {action:?}: an action of the deleted model must be gone, got {status} {body}"
        );
    }

    // What was recorded against the OTHER model is untouched.
    assert_eq!(
        recorded_against(&engine, &kept),
        (1, 1),
        "the kept model's action and execution must survive"
    );
    let (status, body) = list_actions(&router, &kept).await;
    assert_eq!(
        status, 200,
        "the kept model's actions must still list, got {status} {body}"
    );
    let listed: Vec<String> = body["actionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an actionSummaries list, got {body}"))
        .iter()
        .map(|summary| summary["actionId"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        listed,
        vec![kept_action.clone()],
        "the kept model must still report exactly its own action, got {body}"
    );
    let (status, body) = list_executions(&router, &kept).await;
    assert_eq!(
        status, 200,
        "the kept model's executions must still list, got {status} {body}"
    );
    assert_eq!(
        body["executionSummaries"].as_array().map(Vec::len),
        Some(1),
        "the kept model must still report its one execution, got {body}"
    );
}

#[tokio::test]
async fn a_client_token_serves_one_delete_and_replays_its_retry() {
    let (router, engine) = router_with_engine();
    let first = create_model(&router, neighbour_body(), "the model the token deletes").await;
    let second = create_model(
        &router,
        json!({
            "computationModelName": "the-other-model",
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        }),
        "the other model",
    )
    .await;
    let token = format!("{:-<36}", "delete-computation-model");

    let (status, accepted) = delete_model(&router, &first, Some(&token)).await;
    assert_eq!(status, 202, "the first delete answered {status} {accepted}");
    assert_eq!(
        state_of(&accepted),
        "DELETING",
        "the accepted delete reports {accepted}"
    );

    // The SAME request under the SAME token is a RETRY: it replays the answer the
    // first call gave — the model it removed, reported DELETING — rather than the
    // not-found refusal the removal itself would now draw, and removes nothing
    // further.
    let (status, replayed) = delete_model(&router, &first, Some(&token)).await;
    assert_eq!(status, 202, "the retry answered {status} {replayed}");
    assert_eq!(
        state_of(&replayed),
        "DELETING",
        "the retry reports {replayed}"
    );
    assert_eq!(
        stored_ids(&engine),
        vec![second.clone()],
        "the retry removed a second model"
    );

    // A DIFFERENT request reusing that spent token belongs to another call: the
    // modelled conflict, naming the model the first call removed, and it removes
    // nothing.
    let (status, refused) = delete_model(&router, &second, Some(&token)).await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictingOperationException"),
        "a different request reusing the token answered {status} {refused}"
    );
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or("")
            .contains("clientToken"),
        "the refusal does not name the dimension that collided: {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str().unwrap_or(""),
        first,
        "the refusal names another resource: {refused}"
    );
    assert_eq!(
        refused["resourceArn"].as_str().unwrap_or(""),
        expected_arn(&first),
        "the refusal names another ARN: {refused}"
    );
    let (status, still_there) = describe(&router, &second).await;
    assert_eq!(
        status, 200,
        "the refused delete removed the model it addressed: {status} {still_there}"
    );

    // A token of its own serves that same different request.
    let fresh = format!("{:-<36}", "delete-computation-model-2");
    let (status, accepted) = delete_model(&router, &second, Some(&fresh)).await;
    assert_eq!(
        status, 202,
        "the delete under a fresh token answered {status} {accepted}"
    );
    assert!(
        stored_ids(&engine).is_empty(),
        "both deletes landed, so the store holds no model: {:?}",
        stored_ids(&engine)
    );

    // A token is scoped to the operation it was sent to, so the value spent on
    // this delete does not stop a create from being served under it.
    let mut created_under_token = neighbour_body();
    created_under_token["clientToken"] = json!(token);
    let (status, created) = create(&router, created_under_token).await;
    assert_eq!(
        status, 202,
        "a create under the token this delete spent answered {status} {created}"
    );
}

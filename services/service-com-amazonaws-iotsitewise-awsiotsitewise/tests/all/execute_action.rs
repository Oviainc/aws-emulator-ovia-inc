// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ExecuteAction` and the read of the single action it
//! records (`DescribeAction`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * taking an action on a target resource RECORDS it: the call mints an
//!   `actionId` of the modelled form, and the action reads back through
//!   `DescribeAction` and is summarised by `ListActions` exactly as it was taken —
//!   the resource it was taken on, the definition it ran, the payload it carried
//!   and what it resolves to;
//! * taking the action also RUNS it: one execution of it appears under the same
//!   target resource, reporting the action's own type and the target's version,
//!   still running until its run falls due and finished from then on;
//! * the `actionDefinitionId` names one of the actions the target resource
//!   publishes, so a target this store does not hold, an action it does not
//!   publish, or a detailed resource that resolves to nothing is a modelled
//!   refusal that records nothing;
//! * a retried call carrying the `clientToken` of one that already took an action
//!   replays that action rather than taking a second one.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `is_modelled_id`, the asset
//! creates) is the one `asset_lifecycle` already defines, and the
//! computation-model create is `computation_model_lifecycle`'s, re-used rather
//! than copied.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTION_EXECUTION_DELAY, ACTIVATION_DELAY, EXECUTION_STATE_COMPLETED, EXECUTION_STATE_RUNNING,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, is_modelled_id, property_catalog, router,
};
use crate::computation_model_lifecycle::{anomaly_detection, asset_model_property, id_of};
use crate::hierarchies_etags_and_refusals::error_message;

/// An id of the modelled `ID` form that no create in these suites mints, so it
/// names a resource the store does not hold.
const ABSENT_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// A second one, so a request can be wrong in two places at once.
const OTHER_ABSENT_ID: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";

/// Long enough that a transition the clock owes has certainly fallen due.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// Long enough that an execution's run has certainly fallen due.
fn run_over() -> Duration {
    ACTION_EXECUTION_DELAY + Duration::from_millis(250)
}

/// The router, the assets a `resolveTo` reference can name, the computation model
/// the actions are taken on and the actions it publishes.
struct Fixture {
    router: MultiServiceRouter,
    asset_a: String,
    asset_b: String,
    computation_model: String,
    /// `(actionType, actionDefinitionId)` for every action the model publishes,
    /// read off `DescribeComputationModel` rather than written out here.
    definitions: Vec<(String, String)>,
}

impl Fixture {
    /// The `actionDefinitionId` of the `at`-th action the model publishes.
    fn definition(&self, at: usize) -> &str {
        &self
            .definitions
            .get(at)
            .unwrap_or_else(|| panic!("the model publishes no action at {at}"))
            .1
    }

    /// The `actionType` of that same action.
    fn action_type(&self, at: usize) -> &str {
        &self
            .definitions
            .get(at)
            .unwrap_or_else(|| panic!("the model publishes no action at {at}"))
            .0
    }

    /// The query string that lists what is recorded against the computation
    /// model.
    fn target_query(&self) -> String {
        format!(
            "targetResourceType=COMPUTATION_MODEL&targetResourceId={}",
            self.computation_model
        )
    }
}

/// `POST /computation-models`, answering the created id. The configuration names
/// the one computation type the model documents actions for.
async fn create_computation_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/computation-models",
        Some(json!({
            "computationModelName": name,
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "create computation model {name}: got {status} {created}"
    );
    created["computationModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create computation model {name}: no id in {created}"))
        .to_string()
}

/// The `(actionType, actionDefinitionId)` pairs `DescribeComputationModel`
/// reports for `id`, sorted by type so a row can address them by position.
async fn published_actions(router: &MultiServiceRouter, id: &str) -> Vec<(String, String)> {
    let (status, described) = call(router, "GET", &format!("/computation-models/{id}"), None).await;
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

/// Build the fixture: two assets, a settled computation model, and the actions
/// that model publishes.
async fn fixture() -> Fixture {
    let router = router();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_a = create_asset(&router, "Boiler-A", &asset_model).await;
    let asset_b = create_asset(&router, "Boiler-B", &asset_model).await;
    let computation_model = create_computation_model(&router, "boiler-anomaly").await;

    // Every action is taken on a SETTLED resource, so the transient-state
    // refusal its own suite covers is not what these rows run into.
    tokio::time::sleep(settled()).await;
    let definitions = published_actions(&router, &computation_model).await;
    assert!(
        definitions.len() >= 2,
        "the fixture needs two published actions, got {definitions:?}"
    );

    Fixture {
        router,
        asset_a,
        asset_b,
        computation_model,
        definitions,
    }
}

/// An `ExecuteAction` body naming the computation model target.
fn execute_body(
    computation_model_id: &str,
    action_definition_id: &str,
    payload: &str,
    resolve_to: Option<&str>,
) -> Value {
    let mut body = json!({
        "targetResource": { "computationModelId": computation_model_id },
        "actionDefinitionId": action_definition_id,
        "actionPayload": { "stringValue": payload },
    });
    if let Some(asset_id) = resolve_to {
        body["resolveTo"] = json!({ "assetId": asset_id });
    }
    body
}

/// `POST /actions` with `body`.
async fn execute(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/actions", Some(body)).await
}

/// `GET /actions/{actionId}`.
async fn describe_action(router: &MultiServiceRouter, action_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/actions/{action_id}"), None).await
}

/// `GET /actions?...` — what is recorded against one target resource.
async fn list_actions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/actions?{query}"), None).await
}

/// `GET /executions?...` — the runs recorded against one target resource.
async fn list_executions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/executions?{query}"), None).await
}

/// The `actionId`s a `ListActions` body reports, in the order it reported them.
fn listed_action_ids(body: &Value) -> Vec<String> {
    body["actionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an actionSummaries list, got {body}"))
        .iter()
        .map(|summary| summary["actionId"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The execution summaries a `ListExecutions` body reports.
fn listed_executions(body: &Value) -> Vec<Value> {
    body["executionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an executionSummaries list, got {body}"))
        .clone()
}

/// One row of the table of actions taken: what distinguishes it from the others
/// is which of the published actions it runs, what payload it carries and
/// whether it resolves to a detailed resource.
struct TakenRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// Which of the actions the model publishes this row runs.
    definition_at: usize,
    /// The `actionPayload.stringValue` the row carries.
    payload: String,
    /// Which fixture asset the row resolves to, when it resolves to one.
    resolve_to: Option<&'static str>,
}

/// The table of actions taken, spanning the dimensions the rule generalises
/// over: each action the resource publishes, a payload of each form and size an
/// `ActionPayloadString` admits (`^[^\u{0}-\u{1F}\u{7F}]+$`, so any run of
/// non-control characters), and the presence of a `resolveTo` reference.
fn taken_table() -> Vec<TakenRow> {
    vec![
        TakenRow {
            case: "the first published action, a minimal payload, resolving to nothing",
            definition_at: 0,
            payload: "{}".to_string(),
            resolve_to: None,
        },
        TakenRow {
            case: "the first published action again, a structured payload, resolving to A",
            definition_at: 0,
            payload: json!({ "window": 300, "unit": "seconds" }).to_string(),
            resolve_to: Some("A"),
        },
        TakenRow {
            case: "the second published action, a payload of non-ASCII text, resolving to B",
            definition_at: 1,
            payload: json!({ "note": "Kühlung — 熱交換器" }).to_string(),
            resolve_to: Some("B"),
        },
        TakenRow {
            case: "the second published action, a one-character payload",
            definition_at: 1,
            payload: "x".to_string(),
            resolve_to: None,
        },
        TakenRow {
            case: "the second published action, a payload of a thousand characters",
            definition_at: 1,
            payload: json!({ "blob": "y".repeat(1_000) }).to_string(),
            resolve_to: Some("A"),
        },
    ]
}

#[tokio::test]
async fn an_action_taken_reads_back_through_the_read_of_a_single_action() {
    let fixture = fixture().await;
    let rows = taken_table();
    let mut taken: Vec<(String, &TakenRow)> = Vec::new();

    for row in &rows {
        let case = row.case;
        let resolve_to = row.resolve_to.map(|which| match which {
            "A" => fixture.asset_a.clone(),
            _ => fixture.asset_b.clone(),
        });
        let (status, answered) = execute(
            &fixture.router,
            execute_body(
                &fixture.computation_model,
                fixture.definition(row.definition_at),
                &row.payload,
                resolve_to.as_deref(),
            ),
        )
        .await;
        // The operation is ACCEPTED: `smithy_query(query="operation",
        // operation="ExecuteAction")` gives `http {code: 202}`.
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {answered}"
        );
        let action_id = answered["actionId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&action_id),
            "case {case:?}: expected an id of the modelled form, got {answered}"
        );

        // --- DescribeAction: the action reads back as it was taken ----------
        let (status, described) = describe_action(&fixture.router, &action_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {described}"
        );
        assert_eq!(
            described["actionId"].as_str(),
            Some(action_id.as_str()),
            "case {case:?}: a read must report the action it was asked for, got {described}"
        );
        assert_eq!(
            described["actionDefinitionId"].as_str(),
            Some(fixture.definition(row.definition_at)),
            "case {case:?}: unexpected actionDefinitionId in {described}"
        );
        assert_eq!(
            described["actionPayload"]["stringValue"].as_str(),
            Some(row.payload.as_str()),
            "case {case:?}: unexpected actionPayload in {described}"
        );
        assert_eq!(
            described["targetResource"]["computationModelId"].as_str(),
            Some(fixture.computation_model.as_str()),
            "case {case:?}: unexpected targetResource in {described}"
        );
        // The member the target's own kind does not occupy stays absent, so one
        // action names one resource rather than two.
        assert!(
            described["targetResource"]["assetId"].is_null(),
            "case {case:?}: a targetResource must name only its own kind, got {described}"
        );
        assert_eq!(
            described["resolveTo"]["assetId"].as_str(),
            resolve_to.as_deref(),
            "case {case:?}: unexpected resolveTo in {described}"
        );
        let ran_at = described["executionTime"].as_f64();
        assert!(
            ran_at.is_some_and(|at| at > 0.0),
            "case {case:?}: expected an epoch-seconds executionTime, got {described}"
        );

        // A read reports: a second one answers the same action unchanged.
        let (_, again) = describe_action(&fixture.router, &action_id).await;
        assert_eq!(
            again, described,
            "case {case:?}: a second read reports {again}"
        );

        taken.push((action_id, row));
    }

    // Every call minted an id of its own: nothing was overwritten or replayed.
    let distinct: BTreeSet<&String> = taken.iter().map(|(id, _)| id).collect();
    assert_eq!(
        distinct.len(),
        taken.len(),
        "two calls answered the same actionId: {taken:?}",
        taken = taken.iter().map(|(id, _)| id).collect::<Vec<_>>()
    );

    // --- ListActions: the same actions, under the same target resource ------
    let (status, listed) = list_actions(&fixture.router, &fixture.target_query()).await;
    assert_eq!(status, 200, "expected 200, got {status} {listed}");
    let listed_ids = listed_action_ids(&listed);
    for (action_id, row) in &taken {
        assert!(
            listed_ids.contains(action_id),
            "case {:?}: the listing of the target resource must hold {action_id}, got {listed}",
            row.case
        );
    }
    assert_eq!(
        listed_ids.len(),
        taken.len(),
        "the listing must hold exactly the actions taken, got {listed}"
    );

    // --- ListExecutions: one run per action taken --------------------------
    let (status, runs) = list_executions(&fixture.router, &fixture.target_query()).await;
    assert_eq!(status, 200, "expected 200, got {status} {runs}");
    let summaries = listed_executions(&runs);
    assert_eq!(
        summaries.len(),
        taken.len(),
        "taking an action must start exactly one execution of it, got {runs}"
    );
    for (_, row) in &taken {
        let case = row.case;
        let wanted = fixture.action_type(row.definition_at);
        assert!(
            summaries
                .iter()
                .any(|summary| summary["actionType"].as_str() == Some(wanted)),
            "case {case:?}: no execution reports the action type {wanted:?} in {runs}"
        );
    }
    for summary in &summaries {
        // The version of the target resource the action ran against: the one
        // version this store holds of a freshly created computation model.
        assert_eq!(
            summary["targetResourceVersion"].as_str(),
            Some("1"),
            "an execution must report the version it ran against, got {summary}"
        );
        assert_eq!(
            summary["targetResource"]["computationModelId"].as_str(),
            Some(fixture.computation_model.as_str()),
            "an execution must name the resource it ran on, got {summary}"
        );
        assert!(
            summary["executionStartTime"].as_f64().is_some_and(|at| at > 0.0),
            "an execution must report when it started, got {summary}"
        );
    }
}

#[tokio::test]
async fn an_execution_runs_for_a_span_and_reports_its_outcome_once_it_is_over() {
    let fixture = fixture().await;

    let (status, answered) = execute(
        &fixture.router,
        execute_body(
            &fixture.computation_model,
            fixture.definition(0),
            "{\"run\":\"once\"}",
            None,
        ),
    )
    .await;
    assert_eq!(status, 202, "expected 202, got {status} {answered}");

    // Read straight after the call: the run has not fallen due, so the
    // execution is still running and has no end time.
    let (_, runs) = list_executions(&fixture.router, &fixture.target_query()).await;
    let summaries = listed_executions(&runs);
    assert_eq!(
        summaries.len(),
        1,
        "case {:?}: expected one execution, got {runs}",
        "straight after the call"
    );
    assert_eq!(
        summaries[0]["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_RUNNING),
        "case {:?}: expected a running execution, got {runs}",
        "straight after the call"
    );
    assert!(
        summaries[0]["executionEndTime"].is_null(),
        "case {:?}: a running execution must report no end time, got {runs}",
        "straight after the call"
    );

    // Once the run falls due the same execution reports its outcome and the
    // instant it ended — derived from the clock, so no further call was needed.
    tokio::time::sleep(run_over()).await;
    let (_, runs) = list_executions(&fixture.router, &fixture.target_query()).await;
    let summaries = listed_executions(&runs);
    assert_eq!(
        summaries.len(),
        1,
        "case {:?}: expected one execution, got {runs}",
        "once the run is over"
    );
    assert_eq!(
        summaries[0]["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_COMPLETED),
        "case {:?}: expected a finished execution, got {runs}",
        "once the run is over"
    );
    assert!(
        summaries[0]["executionEndTime"]
            .as_f64()
            .is_some_and(|at| at > 0.0),
        "case {:?}: a finished execution must report when it ended, got {runs}",
        "once the run is over"
    );
}

#[tokio::test]
async fn a_retry_carrying_the_token_of_a_call_that_already_acted_replays_that_action() {
    let fixture = fixture().await;

    // Tokens of each form the member admits: `clientToken (ClientToken, len
    // 36..64, pattern ^\S{36,64}$)`, so both published bounds are covered.
    let cases: Vec<(&str, String)> = vec![
        ("a token at the lower bound of 36 characters", "a".repeat(36)),
        ("a token at the upper bound of 64 characters", "b".repeat(64)),
        (
            "a token of hyphenated UUID form",
            "1b4e28ba-2fa1-11d2-883f-0016d3cca427".to_string(),
        ),
    ];

    let mut replayed: Vec<String> = Vec::new();
    for (case, token) in &cases {
        let mut body = execute_body(
            &fixture.computation_model,
            fixture.definition(0),
            &json!({ "token": case }).to_string(),
            Some(&fixture.asset_a),
        );
        body["clientToken"] = json!(token);

        let (status, first) = execute(&fixture.router, body.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {first}"
        );
        let action_id = first["actionId"].as_str().unwrap_or("").to_string();

        let before = list_actions(&fixture.router, &fixture.target_query()).await;
        let (status, retried) = execute(&fixture.router, body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {retried}"
        );
        assert_eq!(
            retried["actionId"].as_str(),
            Some(action_id.as_str()),
            "case {case:?}: a retry must replay the action the token already took, got {retried}"
        );
        let after = list_actions(&fixture.router, &fixture.target_query()).await;
        assert_eq!(
            after, before,
            "case {case:?}: a retry must record no second action"
        );
        replayed.push(action_id);
    }

    // Each distinct token took an action of its own: the replay is keyed on the
    // token, not on the request's shape.
    let distinct: BTreeSet<&String> = replayed.iter().collect();
    assert_eq!(
        distinct.len(),
        replayed.len(),
        "each token must take its own action, got {replayed:?}"
    );

    // A call carrying NO token acts every time, which is what makes the token
    // the thing that makes a call idempotent.
    let untokened = execute_body(
        &fixture.computation_model,
        fixture.definition(0),
        "{\"token\":\"none\"}",
        None,
    );
    let (_, first) = execute(&fixture.router, untokened.clone()).await;
    let (_, second) = execute(&fixture.router, untokened).await;
    assert_ne!(
        first["actionId"], second["actionId"],
        "two calls carrying no token must take two actions, got {first} and {second}"
    );
}

#[tokio::test]
async fn a_target_or_an_action_this_store_does_not_hold_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let definition = fixture.definition(0).to_string();
    let model = fixture.computation_model.clone();
    let asset_a = fixture.asset_a.clone();

    let actions_before = list_actions(&fixture.router, &fixture.target_query()).await;
    let runs_before = list_executions(&fixture.router, &fixture.target_query()).await;

    // Rows span every way a request can address something this store does not
    // hold, plus the two ways a `targetResource` can fail to name one resource.
    // Each expected message is written out from the service's own refusal
    // wording, built from the row's own identifier.
    let cases: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "a targetResource naming neither kind",
            json!({
                "targetResource": {},
                "actionDefinitionId": definition,
                "actionPayload": { "stringValue": "{}" },
            }),
            400,
            "InvalidRequestException",
            "targetResource must name exactly one of assetId, computationModelId; \
             this request named 0."
                .to_string(),
        ),
        (
            "a targetResource naming both kinds",
            json!({
                "targetResource": { "assetId": asset_a, "computationModelId": model },
                "actionDefinitionId": definition,
                "actionPayload": { "stringValue": "{}" },
            }),
            400,
            "InvalidRequestException",
            "targetResource must name exactly one of assetId, computationModelId; \
             this request named 2."
                .to_string(),
        ),
        (
            "a computation model no store holds",
            execute_body(ABSENT_ID, &definition, "{}", None),
            404,
            "ResourceNotFoundException",
            format!("Computation Model with ID {ABSENT_ID} not found"),
        ),
        (
            "an asset no store holds",
            json!({
                "targetResource": { "assetId": ABSENT_ID },
                "actionDefinitionId": definition,
                "actionPayload": { "stringValue": "{}" },
            }),
            404,
            "ResourceNotFoundException",
            format!("Asset with ID {ABSENT_ID} not found"),
        ),
        (
            "an action definition the target resource does not publish",
            execute_body(&model, OTHER_ABSENT_ID, "{}", None),
            404,
            "ResourceNotFoundException",
            format!("Action Definition with ID {OTHER_ABSENT_ID} not found"),
        ),
        (
            "an action definition of another resource, on an asset that publishes none",
            json!({
                "targetResource": { "assetId": asset_a },
                "actionDefinitionId": definition,
                "actionPayload": { "stringValue": "{}" },
            }),
            404,
            "ResourceNotFoundException",
            format!("Action Definition with ID {definition} not found"),
        ),
        (
            "a resolveTo asset no store holds",
            execute_body(&model, &definition, "{}", Some(ABSENT_ID)),
            404,
            "ResourceNotFoundException",
            format!("Asset with ID {ABSENT_ID} not found"),
        ),
    ];

    for (case, body, status_wanted, code_wanted, message_wanted) in cases {
        let (status, answered) = execute(&fixture.router, body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (status_wanted, code_wanted),
            "case {case:?}: unexpected refusal, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            message_wanted,
            "case {case:?}: unexpected message, got {answered}"
        );
    }

    // A refused call records nothing: neither an action nor a run of one.
    let actions_after = list_actions(&fixture.router, &fixture.target_query()).await;
    let runs_after = list_executions(&fixture.router, &fixture.target_query()).await;
    assert_eq!(
        actions_after, actions_before,
        "a refused call must record no action"
    );
    assert_eq!(
        runs_after, runs_before,
        "a refused call must start no execution"
    );

    // The read of a single action refuses an id no action is recorded under, in
    // that same wording.
    let (status, answered) = describe_action(&fixture.router, ABSENT_ID).await;
    assert_eq!(
        (status, error_code(&answered)),
        (404, "ResourceNotFoundException"),
        "an actionId no action is recorded under must be refused, got {answered}"
    );
    assert_eq!(
        error_message(&answered),
        format!("Action with ID {ABSENT_ID} not found"),
        "unexpected message, got {answered}"
    );
}

#[tokio::test]
async fn an_action_on_a_target_resource_that_is_still_settling_is_the_modelled_conflict() {
    let router = router();
    let model = create_computation_model(&router, "settling-anomaly").await;
    // The actions a model publishes are materialised by its create, so they are
    // known before the transition that create owes has fallen due.
    let definitions = published_actions(&router, &model).await;
    let definition = definitions
        .first()
        .map(|(_, id)| id.clone())
        .unwrap_or_else(|| panic!("the model publishes no action: {definitions:?}"));

    // Straight after the create, the model has an operation in flight on it.
    let (status, refused) = execute(
        &router,
        execute_body(&model, &definition, "{\"when\":\"creating\"}", None),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictingOperationException"),
        "an action on a resource that is still settling must collide, got {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str(),
        Some(model.as_str()),
        "the conflict must name the resource both operations are on, got {refused}"
    );

    // Nothing was recorded, and the same call succeeds once the transition the
    // create owes has fallen due.
    let query = format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={model}");
    let (_, listed) = list_actions(&router, &query).await;
    assert_eq!(
        listed_action_ids(&listed),
        Vec::<String>::new(),
        "a refused call must record no action, got {listed}"
    );

    tokio::time::sleep(settled()).await;
    let (status, taken) = execute(
        &router,
        execute_body(&model, &definition, "{\"when\":\"active\"}", None),
    )
    .await;
    assert_eq!(
        status, 202,
        "the same call must succeed once the resource has settled, got {status} {taken}"
    );
    let (_, listed) = list_actions(&router, &query).await;
    assert_eq!(
        listed_action_ids(&listed),
        vec![taken["actionId"].as_str().unwrap_or("").to_string()],
        "the action taken must be the one the listing holds, got {listed}"
    );
}

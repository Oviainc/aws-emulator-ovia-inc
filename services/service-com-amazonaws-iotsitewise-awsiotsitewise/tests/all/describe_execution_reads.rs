// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeExecution`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: the read of ONE execution reports, from the shared
//! store, the execution its `executionId` names — the action type it ran, the
//! resource and resource version it ran against, what it resolves to, when it
//! started and the run as the clock has it (still running until its run is due to
//! end, its outcome and the instant it ended from then on) — identically to the
//! listing that holds it, and writes nothing; an id no execution is recorded under
//! is a modelled refusal rather than a fabricated answer.
//!
//! `ExecuteAction` is the operation that records an execution, and it records one
//! only against a target resource that publishes the action it is asked to run, so
//! one suite below drives that round trip end to end and the table suite records
//! its rows directly in the shared store this handler reads — the same store a
//! handler writes through, which is what the `ListExecutions` suite does for its
//! own table.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, the asset
//! creates) is the one `asset_lifecycle` already defines and the
//! computation-model pieces are `computation_model_lifecycle`'s, re-used rather
//! than copied.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ResolveTo, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ExecutionRecord, ACTION_EXECUTION_DELAY, ACTIVATION_DELAY,
    EXECUTION_STATE_COMPLETED, EXECUTION_STATE_FAILED, EXECUTION_STATE_RUNNING,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, is_modelled_id, property_catalog,
    router_with_engine,
};
use crate::computation_model_lifecycle::{anomaly_detection, asset_model_property, id_of};
use crate::hierarchies_etags_and_refusals::error_message;

/// An id of the modelled `ID` form that no create in these suites mints, so it
/// names an execution the store does not hold.
const ABSENT_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// Long enough that a transition the clock owes a resource has fallen due.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// Long enough that an execution's run has fallen due.
fn run_over() -> Duration {
    ACTION_EXECUTION_DELAY + Duration::from_millis(250)
}

/// Which seeded resource an execution is recorded against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The asset the fixture creates.
    Asset,
    /// The computation model the fixture creates.
    ComputationModel,
}

/// One row of the seeded execution table.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The id the execution is stored and read back under.
    id: &'static str,
    /// The resource the action was taken on.
    target: Target,
    /// The type of action executed, when the execution carries one.
    action_type: Option<&'static str>,
    /// How many seconds before the suite's anchor the execution started.
    started_ago: u64,
    /// How many seconds from the anchor the run is due to end: negative for a run
    /// already over, positive for one still in flight.
    settles_in: i64,
    /// The terminal state the run settles into.
    outcome: &'static str,
    /// Whether the execution resolves to the fixture's detailed asset.
    resolves: bool,
    /// The version of the target resource the action ran against.
    target_resource_version: &'static str,
    /// The execution entity version, when the execution carries one.
    entity_version: Option<&'static str>,
}

/// The row whose `actionType` is seeded at the published upper bound of its
/// length, which is too long to spell out in the table itself.
const LONGEST_ACTION_ROW: &str = "bbbbbbb2-2222-4222-8222-222222222222";

/// An `actionType` at the published upper bound of its length: the member is a
/// `Name`, `len 1..256`. Source: `smithy_query(query="operation",
/// operation="DescribeExecution")` — `actionType (Name, len 1..256, pattern
/// ^[^\u0000-\u001F\u007F]+$)`, a free string rather than an enum, so the rows
/// span the forms that pattern admits instead of a published catalog.
fn longest_action_type() -> String {
    "l".repeat(256)
}

/// The seeded executions: one table spanning the dimensions the read's rule
/// generalises over — which KIND of resource the execution targets, whether it
/// carries an `actionType` and in which of the forms the member admits, whether
/// it resolves to a detailed resource, which version of the target it ran
/// against, whether its run is over, which terminal state it settles into and
/// whether it carries an entity version.
fn execution_table() -> Vec<Row> {
    vec![
        Row {
            case: "on the asset, settled, completed, shortest action type",
            id: "bbbbbbb1-1111-4111-8111-111111111111",
            target: Target::Asset,
            // The published lower bound of the `Name` length, 1.
            action_type: Some("a"),
            started_ago: 900,
            settles_in: -880,
            outcome: EXECUTION_STATE_COMPLETED,
            resolves: false,
            target_resource_version: "1",
            entity_version: None,
        },
        Row {
            case: "on the asset, settled, failed, longest action type, resolving to an asset",
            id: LONGEST_ACTION_ROW,
            target: Target::Asset,
            action_type: None, // replaced by `longest_action_type()` when seeded
            started_ago: 800,
            settles_in: -700,
            outcome: EXECUTION_STATE_FAILED,
            resolves: true,
            target_resource_version: "7",
            entity_version: Some("2"),
        },
        Row {
            case: "on the asset, run still in flight, carrying no action type",
            id: "bbbbbbb3-3333-4333-8333-333333333333",
            target: Target::Asset,
            action_type: None,
            started_ago: 10,
            settles_in: 600,
            outcome: EXECUTION_STATE_COMPLETED,
            resolves: false,
            target_resource_version: "1",
            entity_version: None,
        },
        Row {
            case: "on the computation model, settled, completed, resolving to an asset",
            id: "bbbbbbb4-4444-4444-8444-444444444444",
            target: Target::ComputationModel,
            action_type: Some("Amazon Web Services/ANOMALY_DETECTION_TRAINING"),
            started_ago: 500,
            settles_in: -400,
            outcome: EXECUTION_STATE_COMPLETED,
            resolves: true,
            target_resource_version: "1",
            entity_version: Some("1"),
        },
        Row {
            case: "on the computation model, run still in flight, an action type of punctuation",
            id: "bbbbbbb5-5555-4555-8555-555555555555",
            target: Target::ComputationModel,
            action_type: Some("Amazon Web Services/ANOMALY_DETECTION_INFERENCE"),
            started_ago: 1,
            settles_in: 1_200,
            outcome: EXECUTION_STATE_FAILED,
            resolves: false,
            // A version past a single digit, so the member is read rather than
            // assumed to be the "1" a fresh resource carries.
            target_resource_version: "1234567890",
            entity_version: Some("9999999999"),
        },
    ]
}

/// The router, the resources the seeded executions name, and the table itself.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    asset: String,
    /// The asset a row's `resolveTo` reference names.
    resolved_asset: String,
    computation_model: String,
    /// `(actionType, actionDefinitionId)` for every action the computation model
    /// publishes, read off `DescribeComputationModel` rather than written here.
    definitions: Vec<(String, String)>,
    /// The anchor every seeded instant is measured from.
    anchor: SystemTime,
    rows: Vec<Row>,
}

impl Fixture {
    /// The stored id of a seeded target.
    fn id_of_target(&self, target: Target) -> &str {
        match target {
            Target::Asset => &self.asset,
            Target::ComputationModel => &self.computation_model,
        }
    }

    /// The `actionType` a row is seeded with.
    fn action_type_of(&self, row: &Row) -> Option<String> {
        if row.id == LONGEST_ACTION_ROW {
            Some(longest_action_type())
        } else {
            row.action_type.map(str::to_string)
        }
    }

    /// When a row's execution started, as the epoch seconds the wire carries.
    fn start_of(&self, row: &Row) -> i64 {
        epoch_seconds(self.anchor) - row.started_ago as i64
    }

    /// When a row's run is due to end, as the epoch seconds the wire carries.
    fn settles_at_of(&self, row: &Row) -> i64 {
        epoch_seconds(self.anchor) + row.settles_in
    }

    /// The query string listing what is recorded against a row's target
    /// resource.
    fn target_query(&self, target: Target) -> String {
        let (kind, id) = match target {
            Target::Asset => ("ASSET", &self.asset),
            Target::ComputationModel => ("COMPUTATION_MODEL", &self.computation_model),
        };
        format!("targetResourceType={kind}&targetResourceId={id}")
    }
}

/// `at` as whole epoch seconds.
fn epoch_seconds(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the Unix epoch")
        .as_secs() as i64
}

/// `anchor` moved by `seconds`, which may be negative.
fn shifted(anchor: SystemTime, seconds: i64) -> SystemTime {
    if seconds < 0 {
        anchor - Duration::from_secs(seconds.unsigned_abs())
    } else {
        anchor + Duration::from_secs(seconds as u64)
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
/// reports for `id`, sorted by type so a caller can address them by position.
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

/// Build the fixture: two assets and a settled computation model created through
/// the API, then the execution table recorded in the shared store.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let asset = create_asset(&router, "Boiler-A", &asset_model).await;
    let resolved_asset = create_asset(&router, "Boiler-B", &asset_model).await;
    let computation_model = create_computation_model(&router, "boiler-anomaly").await;

    // The actions a model publishes are readable once the create it owes has
    // settled, which is also the state an action may be taken in.
    tokio::time::sleep(settled()).await;
    let definitions = published_actions(&router, &computation_model).await;

    let fixture = Fixture {
        router,
        engine,
        asset,
        resolved_asset,
        computation_model,
        definitions,
        anchor: SystemTime::now(),
        rows: execution_table(),
    };

    {
        let state = service_state(&*fixture.engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for row in &fixture.rows {
            let target_id = fixture.id_of_target(row.target).to_string();
            let target_resource = match row.target {
                Target::Asset => TargetResource {
                    asset_id: Some(target_id),
                    computation_model_id: None,
                },
                Target::ComputationModel => TargetResource {
                    asset_id: None,
                    computation_model_id: Some(target_id),
                },
            };
            store.executions.insert(
                row.id.to_string(),
                ExecutionRecord {
                    execution_id: row.id.to_string(),
                    action_type: fixture.action_type_of(row),
                    target_resource,
                    target_resource_version: row.target_resource_version.to_string(),
                    resolve_to: row.resolves.then(|| ResolveTo {
                        asset_id: fixture.resolved_asset.clone(),
                    }),
                    execution_start_time: fixture.anchor - Duration::from_secs(row.started_ago),
                    settles_at: shifted(fixture.anchor, row.settles_in),
                    outcome: row.outcome.to_string(),
                    execution_entity_version: row.entity_version.map(str::to_string),
                },
            );
        }
    }

    fixture
}

/// `GET /executions/{executionId}`.
async fn describe_execution(router: &MultiServiceRouter, execution_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/executions/{execution_id}"), None).await
}

/// `GET /executions?...` — the runs recorded against one target resource.
async fn list_executions(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(router, "GET", &format!("/executions?{query}"), None).await
}

/// The execution summaries a `ListExecutions` body reports, by `executionId`.
fn summaries_by_id(body: &Value) -> BTreeMap<String, Value> {
    body["executionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an executionSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            (
                summary["executionId"].as_str().unwrap_or("").to_string(),
                summary.clone(),
            )
        })
        .collect()
}

/// Every execution this store holds, as `(id, state, end time)` read off the
/// record rather than off a response: an oracle for "a read wrote nothing".
fn stored_executions(
    engine: &Arc<engine::InMemoryEngine>,
    now: SystemTime,
) -> BTreeMap<String, (String, Option<SystemTime>)> {
    let state = service_state(&**engine).expect("the iotsitewise store");
    let store = state.lock();
    store
        .executions
        .iter()
        .map(|(id, record)| {
            (
                id.clone(),
                (
                    record.state(now).to_string(),
                    record.execution_end_time(now),
                ),
            )
        })
        .collect()
}

/// Assert a `DescribeExecution` body reports exactly what `row` was seeded with.
fn assert_reports_row(fixture: &Fixture, row: &Row, described: &Value) {
    let case = row.case;
    assert_eq!(
        described["executionId"].as_str(),
        Some(row.id),
        "case {case:?}: a read must report the execution it was asked for, got {described}"
    );
    assert_eq!(
        described["actionType"].as_str().map(str::to_string),
        fixture.action_type_of(row),
        "case {case:?}: unexpected actionType in {described}"
    );
    let expected_target = fixture.id_of_target(row.target);
    let (occupied, empty) = match row.target {
        Target::Asset => ("assetId", "computationModelId"),
        Target::ComputationModel => ("computationModelId", "assetId"),
    };
    assert_eq!(
        described["targetResource"][occupied].as_str(),
        Some(expected_target),
        "case {case:?}: unexpected targetResource in {described}"
    );
    // The member the target's own kind does not occupy stays absent, so one
    // execution names one resource rather than two.
    assert!(
        described["targetResource"][empty].is_null(),
        "case {case:?}: a targetResource must name only its own kind, got {described}"
    );
    assert_eq!(
        described["targetResourceVersion"].as_str(),
        Some(row.target_resource_version),
        "case {case:?}: unexpected targetResourceVersion in {described}"
    );
    assert_eq!(
        described["resolveTo"]["assetId"].as_str(),
        row.resolves.then_some(fixture.resolved_asset.as_str()),
        "case {case:?}: unexpected resolveTo in {described}"
    );
    assert_eq!(
        described["executionStartTime"].as_f64().map(|at| at as i64),
        Some(fixture.start_of(row)),
        "case {case:?}: unexpected executionStartTime in {described}"
    );
    assert_eq!(
        described["executionEntityVersion"].as_str(),
        row.entity_version,
        "case {case:?}: unexpected executionEntityVersion in {described}"
    );

    // The run as the clock has it: a row whose end is still ahead of the anchor
    // reports RUNNING with no end time, and one whose end has passed reports the
    // state it settled into and the instant it ended.
    let still_running = row.settles_in > 0;
    let expected_state = if still_running {
        EXECUTION_STATE_RUNNING
    } else {
        row.outcome
    };
    assert_eq!(
        described["executionStatus"]["state"].as_str(),
        Some(expected_state),
        "case {case:?}: unexpected executionStatus in {described}"
    );
    let expected_end = (!still_running).then(|| fixture.settles_at_of(row));
    assert_eq!(
        described["executionEndTime"].as_f64().map(|at| at as i64),
        expected_end,
        "case {case:?}: unexpected executionEndTime in {described}"
    );

    // Nothing in this emulator runs an anomaly detection model, so there are no
    // model metrics and no result for one: the two maps that would carry them
    // stay absent rather than reporting a figure nothing computed.
    for member in ["executionResult", "executionDetails"] {
        assert!(
            described[member].is_null(),
            "case {case:?}: {member} reports {} for a model nothing ran",
            described[member]
        );
    }
}

#[tokio::test]
async fn every_stored_execution_reads_back_through_the_read_of_a_single_execution() {
    let fixture = fixture().await;

    for row in &fixture.rows {
        let case = row.case;
        let (status, described) = describe_execution(&fixture.router, row.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for {}, got {status} {described}",
            row.id
        );
        assert_reports_row(&fixture, row, &described);

        // A read REPORTS: a second one answers the same execution unchanged.
        let (_, again) = describe_execution(&fixture.router, row.id).await;
        assert_eq!(
            again, described,
            "case {case:?}: a second read reports {again}"
        );
    }
}

#[tokio::test]
async fn the_read_of_one_execution_and_the_listing_that_holds_it_agree() {
    let fixture = fixture().await;

    for target in [Target::Asset, Target::ComputationModel] {
        let (status, listed) = list_executions(&fixture.router, &fixture.target_query(target)).await;
        assert_eq!(
            status, 200,
            "case {target:?}: expected 200, got {status} {listed}"
        );
        let summaries = summaries_by_id(&listed);
        let expected: Vec<&Row> = fixture
            .rows
            .iter()
            .filter(|row| row.target == target)
            .collect();
        assert_eq!(
            summaries.len(),
            expected.len(),
            "case {target:?}: the listing must hold every execution of the target, got {listed}"
        );

        for row in expected {
            let case = row.case;
            let summary = summaries
                .get(row.id)
                .unwrap_or_else(|| panic!("case {case:?}: {} is not listed in {listed}", row.id));
            let (status, described) = describe_execution(&fixture.router, row.id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {described}"
            );
            // Every member the summary carries is what the read of that same
            // execution reports: one execution, not two views of it.
            for member in [
                "executionId",
                "actionType",
                "targetResource",
                "targetResourceVersion",
                "resolveTo",
                "executionStartTime",
                "executionEndTime",
                "executionStatus",
                "executionEntityVersion",
            ] {
                assert_eq!(
                    described[member], summary[member],
                    "case {case:?}: {member} differs between the read and the summary of {}: \
                     {} vs {}",
                    row.id, described[member], summary[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn an_execution_an_action_started_reads_back_and_reports_its_run_as_the_clock_has_it() {
    let fixture = fixture().await;
    let (action_type, definition) = fixture
        .definitions
        .first()
        .expect("the computation model publishes an action")
        .clone();

    let (status, answered) = call(
        &fixture.router,
        "POST",
        "/actions",
        Some(json!({
            "targetResource": { "computationModelId": fixture.computation_model },
            "actionDefinitionId": definition,
            "actionPayload": { "stringValue": "{\"run\":\"once\"}" },
            "resolveTo": { "assetId": fixture.resolved_asset },
        })),
    )
    .await;
    assert_eq!(status, 202, "ExecuteAction: got {status} {answered}");

    // The execution the action started is the one this target resource gained,
    // so its id comes off the listing rather than being minted by the test.
    let (_, listed) = list_executions(
        &fixture.router,
        &fixture.target_query(Target::ComputationModel),
    )
    .await;
    let started: Vec<String> = summaries_by_id(&listed)
        .into_keys()
        .filter(|id| !fixture.rows.iter().any(|row| row.id == id))
        .collect();
    assert_eq!(
        started.len(),
        1,
        "taking an action must start exactly one execution of it, got {listed}"
    );
    let execution_id = &started[0];
    assert!(
        is_modelled_id(execution_id),
        "expected an executionId of the modelled form, got {execution_id}"
    );

    // Read straight after the call: the run has not fallen due, so the execution
    // is still running and reports no end time.
    let (status, running) = describe_execution(&fixture.router, execution_id).await;
    assert_eq!(status, 200, "expected 200, got {status} {running}");
    assert_eq!(
        running["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_RUNNING),
        "an execution whose run is not over must still be running, got {running}"
    );
    assert!(
        running["executionEndTime"].is_null(),
        "an execution still running has not ended, got {running}"
    );
    assert_eq!(
        running["actionType"].as_str(),
        Some(action_type.as_str()),
        "an execution must report the type of the action it ran, got {running}"
    );
    assert_eq!(
        running["targetResource"]["computationModelId"].as_str(),
        Some(fixture.computation_model.as_str()),
        "an execution must name the resource it ran on, got {running}"
    );
    assert_eq!(
        running["targetResourceVersion"].as_str(),
        Some("1"),
        "an execution must report the version it ran against, got {running}"
    );
    assert_eq!(
        running["resolveTo"]["assetId"].as_str(),
        Some(fixture.resolved_asset.as_str()),
        "an execution must report what it resolves to, got {running}"
    );
    let started_at = running["executionStartTime"].as_f64();
    assert!(
        started_at.is_some_and(|at| at > 0.0),
        "an execution must report when it started, got {running}"
    );

    // Once the run falls due the same read reports the state it settled into and
    // the instant it ended — the clock moved it, not the read.
    tokio::time::sleep(run_over()).await;
    let (status, finished) = describe_execution(&fixture.router, execution_id).await;
    assert_eq!(status, 200, "expected 200, got {status} {finished}");
    assert_eq!(
        finished["executionStatus"]["state"].as_str(),
        Some(EXECUTION_STATE_COMPLETED),
        "an execution whose run is over must report its outcome, got {finished}"
    );
    let ended_at = finished["executionEndTime"].as_f64();
    assert!(
        ended_at.is_some_and(|ended| started_at.is_some_and(|started| ended >= started)),
        "an execution that ended must report when, no earlier than it started, got {finished}"
    );
    assert_eq!(
        finished["executionStartTime"], running["executionStartTime"],
        "the instant an execution started must not move, got {finished}"
    );

    // A read reports: two more of them answer the same execution unchanged.
    let (_, again) = describe_execution(&fixture.router, execution_id).await;
    assert_eq!(again, finished, "a second read reports {again}");
}

#[tokio::test]
async fn an_execution_id_no_run_is_recorded_under_is_a_modelled_refusal() {
    let fixture = fixture().await;

    // An action taken gives an `actionId`, which addresses the ACTION and not the
    // execution of it: the two are recorded under ids of their own.
    let definition = fixture
        .definitions
        .first()
        .expect("the computation model publishes an action")
        .1
        .clone();
    let (status, answered) = call(
        &fixture.router,
        "POST",
        "/actions",
        Some(json!({
            "targetResource": { "computationModelId": fixture.computation_model },
            "actionDefinitionId": definition,
            "actionPayload": { "stringValue": "{}" },
        })),
    )
    .await;
    assert_eq!(status, 202, "ExecuteAction: got {status} {answered}");
    let action_id = answered["actionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no actionId in {answered}"))
        .to_string();

    let now = SystemTime::now();
    let before = stored_executions(&fixture.engine, now);

    // Every way a well-formed id can address no stored execution: one nothing
    // minted, one naming a stored resource of each kind rather than an execution,
    // one naming an action definition, and one naming the action an execution
    // belongs to.
    let cases: Vec<(&str, String)> = vec![
        ("an id nothing minted", ABSENT_ID.to_string()),
        ("the id of a stored asset", fixture.asset.clone()),
        (
            "the id of a stored computation model",
            fixture.computation_model.clone(),
        ),
        ("the id of an action definition", definition),
        ("the id of the action an execution ran", action_id),
    ];

    for (case, requested) in cases {
        let (status, refused) = describe_execution(&fixture.router, &requested).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: unexpected refusal for {requested}, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("Execution with ID {requested} not found"),
            "case {case:?}: unexpected refusal wording, got {refused}"
        );
    }

    // `executionId` carries the modelled length 36..36 (`smithy_query(
    // query="operation", operation="DescribeExecution")`), so an id outside that
    // length is a BAD REQUEST rather than an absent execution. The bound is
    // checked on each side: one character short and one past it.
    for (case, requested) in [
        (
            "one character short of the modelled length",
            "cacacaca-1111-4111-8111-11111111111",
        ),
        (
            "one character past the modelled length",
            "cacacaca-1111-4111-8111-1111111111111",
        ),
    ] {
        let (status, refused) = describe_execution(&fixture.router, requested).await;
        // The one client refusal this operation models for a request it cannot
        // make sense of: `smithy_query(query="operation",
        // operation="DescribeExecution")` declares `InvalidRequestException`
        // (HTTP 400), so the code is pinned rather than matched against a set an
        // unmodelled error shape would also satisfy.
        assert_eq!(
            (status, error_code(&refused)),
            (400, "InvalidRequestException"),
            "case {case:?}: unexpected refusal for {requested}, got {refused}"
        );
    }

    // No refusal wrote anything, and the executions the store does hold still
    // read back exactly as they were recorded.
    assert_eq!(
        stored_executions(&fixture.engine, now),
        before,
        "a refused read must write nothing"
    );
    for row in &fixture.rows {
        let (status, described) = describe_execution(&fixture.router, row.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 after the refusals, got {status} {described}",
            row.case
        );
        assert_reports_row(&fixture, row, &described);
    }
}

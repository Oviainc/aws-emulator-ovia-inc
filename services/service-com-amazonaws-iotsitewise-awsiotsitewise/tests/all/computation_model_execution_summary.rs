// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeComputationModelExecutionSummary`, driven over
//! HTTP through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the summary of a computation model is DERIVED from the executions the shared
//!   store holds against THAT model: one entry per `ExecutionState` the model
//!   declares, each carrying how many of those executions are in that state as the
//!   clock has it — an execution still in flight counted as running until its run
//!   is due to end and under its outcome from then on;
//! * a request naming a resolved resource narrows the summary to the executions
//!   that resolve to it, and reports that resource back;
//! * an action executed on a model through the API shows up in that model's
//!   summary, and in no other model's;
//! * a `computationModelId` no create made, a resolved resource no create made,
//!   and a resolved-resource reference stated by only one of its two members are
//!   modelled refusals;
//! * reading a summary changes nothing a later caller sees.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, the asset
//! creates) is the one `asset_lifecycle` already defines, and the
//! computation-model create is `computation_model_lifecycle`'s, re-used rather
//! than copied.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ResolveTo, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ExecutionRecord, ACTION_EXECUTION_DELAY, ACTIVATION_DELAY,
    EXECUTION_STATE_COMPLETED, EXECUTION_STATE_FAILED, EXECUTION_STATE_RUNNING, EXECUTION_STATES,
};

use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, router_with_engine,
};
use crate::computation_model_lifecycle::{anomaly_detection, asset_model_property, create, id_of};
use crate::hierarchies_etags_and_refusals::error_message;

/// An id of the modelled `ID` form that no create in this suite mints, so it names
/// a resource the store does not hold.
const ABSENT_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";

/// Long enough that a transition the clock owes a computation model has certainly
/// fallen due.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(250)
}

/// Long enough that an execution's run has certainly fallen due.
fn run_over() -> Duration {
    ACTION_EXECUTION_DELAY + Duration::from_millis(250)
}

/// Which computation model of the fixture a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Model {
    /// The model most of the seeded executions are recorded against.
    Busy,
    /// A second model, so a summary cannot report another model's executions.
    Other,
    /// A model no execution is recorded against.
    Idle,
}

/// Which fixture asset a row resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asset {
    A,
    B,
}

/// What an execution is recorded against: a computation model, or an asset — the
/// other kind of target `TargetResource` carries, which a model's summary must not
/// count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Model(Model),
    Asset(Asset),
}

/// One row of the seeded execution table.
#[derive(Debug, Clone)]
struct ExecutionRow {
    /// What this row is in the suite, named in every assertion.
    case: &'static str,
    /// The id the execution is stored under.
    id: &'static str,
    /// The resource the action was taken on.
    target: Target,
    /// The detailed resource the execution resolves to, when it resolves to one.
    resolve_to: Option<Asset>,
    /// How many seconds from the fixture's anchor the run is due to end: negative
    /// for a run already over, positive for one still in flight.
    settles_in: i64,
    /// The terminal state the run settles into.
    outcome: &'static str,
}

impl ExecutionRow {
    /// The `ExecutionState` this row is in when the suite reads it: still running
    /// while its end is ahead of the anchor, its outcome once that has passed.
    fn state(&self) -> &'static str {
        if self.settles_in > 0 {
            EXECUTION_STATE_RUNNING
        } else {
            self.outcome
        }
    }
}

/// The seeded executions: one table spanning the dimensions the summary rule
/// generalises over — which resource the execution targets (two different
/// computation models, and an asset), whether it resolves to a detailed resource
/// and which one, whether its run is over, and which terminal state it settles
/// into.
fn execution_table() -> Vec<ExecutionRow> {
    vec![
        ExecutionRow {
            case: "busy model, resolving to nothing, run over, completed",
            id: "aaaaaaa1-1111-4111-8111-111111111111",
            target: Target::Model(Model::Busy),
            resolve_to: None,
            settles_in: -600,
            outcome: EXECUTION_STATE_COMPLETED,
        },
        ExecutionRow {
            case: "busy model, resolving to A, run over, completed",
            id: "aaaaaaa2-2222-4222-8222-222222222222",
            target: Target::Model(Model::Busy),
            resolve_to: Some(Asset::A),
            settles_in: -500,
            outcome: EXECUTION_STATE_COMPLETED,
        },
        ExecutionRow {
            case: "busy model, resolving to A, run over, failed",
            id: "aaaaaaa3-3333-4333-8333-333333333333",
            target: Target::Model(Model::Busy),
            resolve_to: Some(Asset::A),
            settles_in: -400,
            outcome: EXECUTION_STATE_FAILED,
        },
        ExecutionRow {
            case: "busy model, resolving to A, still in flight",
            id: "aaaaaaa4-4444-4444-8444-444444444444",
            target: Target::Model(Model::Busy),
            resolve_to: Some(Asset::A),
            settles_in: 600,
            outcome: EXECUTION_STATE_COMPLETED,
        },
        ExecutionRow {
            case: "busy model, resolving to B, still in flight, settles failed",
            id: "aaaaaaa5-5555-4555-8555-555555555555",
            target: Target::Model(Model::Busy),
            resolve_to: Some(Asset::B),
            settles_in: 700,
            outcome: EXECUTION_STATE_FAILED,
        },
        ExecutionRow {
            case: "other model, resolving to A, run over, failed",
            id: "aaaaaaa6-6666-4666-8666-666666666666",
            target: Target::Model(Model::Other),
            resolve_to: Some(Asset::A),
            settles_in: -300,
            outcome: EXECUTION_STATE_FAILED,
        },
        ExecutionRow {
            case: "an execution of an ASSET, resolving to A, run over, completed",
            id: "aaaaaaa7-7777-4777-8777-777777777777",
            target: Target::Asset(Asset::A),
            resolve_to: Some(Asset::A),
            settles_in: -200,
            outcome: EXECUTION_STATE_COMPLETED,
        },
    ]
}

/// The router, the assets a resolved-resource reference can name, and the
/// computation models the seeded executions are recorded against.
struct Fixture {
    router: MultiServiceRouter,
    asset_a: String,
    asset_b: String,
    busy: String,
    other: String,
    idle: String,
    rows: Vec<ExecutionRow>,
}

impl Fixture {
    /// The id of one of the fixture's computation models.
    fn model_id(&self, model: Model) -> &str {
        match model {
            Model::Busy => &self.busy,
            Model::Other => &self.other,
            Model::Idle => &self.idle,
        }
    }

    /// The id of one of the fixture's assets.
    fn asset_id(&self, asset: Asset) -> &str {
        match asset {
            Asset::A => &self.asset_a,
            Asset::B => &self.asset_b,
        }
    }
}

/// `POST /computation-models`, answering the created id. The configuration names
/// the one computation type the model documents actions for.
pub(crate) async fn create_computation_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = create(
        router,
        json!({
            "computationModelName": name,
            "computationModelConfiguration": anomaly_detection("input", "result"),
            "computationModelDataBinding": {
                "input": asset_model_property(&id_of('1'), &id_of('2')),
                "result": asset_model_property(&id_of('1'), &id_of('3')),
            },
        }),
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

/// Build the fixture: two assets and three computation models created through the
/// API, then the execution table recorded in the shared store.
///
/// `ExecuteAction` is the operation that records an execution, and it records one
/// only against a model that is settled and only with the outcome a run that
/// succeeds reports, so the table — which spans runs still in flight and runs that
/// failed as well — is recorded directly in the shared store this handler reads,
/// the way the `ListExecutions` suite records its own.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_a = create_asset(&router, "Boiler-A", &asset_model).await;
    let asset_b = create_asset(&router, "Boiler-B", &asset_model).await;
    let busy = create_computation_model(&router, "boiler-anomaly").await;
    let other = create_computation_model(&router, "pump-anomaly").await;
    let idle = create_computation_model(&router, "idle-anomaly").await;

    let fixture = Fixture {
        router,
        asset_a,
        asset_b,
        busy,
        other,
        idle,
        rows: execution_table(),
    };

    let anchor = SystemTime::now();
    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for row in &fixture.rows {
            let target_resource = match row.target {
                Target::Model(model) => TargetResource {
                    asset_id: None,
                    computation_model_id: Some(fixture.model_id(model).to_string()),
                },
                Target::Asset(asset) => TargetResource {
                    asset_id: Some(fixture.asset_id(asset).to_string()),
                    computation_model_id: None,
                },
            };
            store.executions.insert(
                row.id.to_string(),
                ExecutionRecord {
                    execution_id: row.id.to_string(),
                    action_type: Some("Amazon Web Services/ANOMALY_DETECTION_TRAINING".to_string()),
                    target_resource,
                    target_resource_version: "1".to_string(),
                    resolve_to: row.resolve_to.map(|asset| ResolveTo {
                        asset_id: fixture.asset_id(asset).to_string(),
                    }),
                    execution_start_time: anchor - Duration::from_secs(900),
                    settles_at: shifted(anchor, row.settles_in),
                    outcome: row.outcome.to_string(),
                    execution_entity_version: None,
                },
            );
        }
    }

    fixture
}

/// `anchor` moved by `seconds`, which may be negative.
fn shifted(anchor: SystemTime, seconds: i64) -> SystemTime {
    if seconds < 0 {
        anchor - Duration::from_secs(seconds.unsigned_abs())
    } else {
        anchor + Duration::from_secs(seconds as u64)
    }
}

/// `GET /computation-models/{id}/execution-summary`, with `query` appended when it
/// is not empty.
async fn describe_summary(
    router: &MultiServiceRouter,
    computation_model_id: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/computation-models/{computation_model_id}/execution-summary")
    } else {
        format!("/computation-models/{computation_model_id}/execution-summary?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// The query string that narrows a summary to one resolved asset.
fn resolve_to_query(asset_id: &str) -> String {
    format!("resolveToResourceType=ASSET&resolveToResourceId={asset_id}")
}

/// The reported summary map, as `key -> value` pairs.
fn reported_counts(body: &Value) -> BTreeMap<String, String> {
    body["computationModelExecutionSummary"]
        .as_object()
        .unwrap_or_else(|| panic!("expected a computationModelExecutionSummary map, got {body}"))
        .iter()
        .map(|(key, value)| {
            let count = value
                .as_str()
                .unwrap_or_else(|| panic!("a non-string summary value in {body}"));
            (key.clone(), count.to_string())
        })
        .collect()
}

/// The summary the seeded table implies for `model`, narrowed to `resolved` when a
/// resolved resource is named: one entry per declared `ExecutionState`, each the
/// count of the rows in that state.
fn expected_counts(
    rows: &[ExecutionRow],
    model: Model,
    resolved: Option<Asset>,
) -> BTreeMap<String, String> {
    let mut counts: BTreeMap<String, u64> = EXECUTION_STATES
        .iter()
        .map(|state| ((*state).to_string(), 0))
        .collect();
    for row in rows
        .iter()
        .filter(|row| row.target == Target::Model(model))
        .filter(|row| resolved.is_none() || row.resolve_to == resolved)
    {
        *counts
            .get_mut(row.state())
            .unwrap_or_else(|| panic!("case {:?}: an unmodelled state", row.case)) += 1;
    }
    counts
        .into_iter()
        .map(|(state, count)| (state, count.to_string()))
        .collect()
}

/// One row of the table of summaries read: which model it asks about, and which
/// resolved resource it narrows the answer to.
#[derive(Debug)]
struct ReadCase {
    case: &'static str,
    model: Model,
    resolved: Option<Asset>,
}

/// The reads, spanning the dimensions the rule generalises over: a model with
/// executions in several states, a model with one, a model with none, and each
/// read taken both unnarrowed and narrowed to each asset a row resolves to —
/// including one that narrows to an asset no execution of that model resolves to.
fn read_table() -> Vec<ReadCase> {
    vec![
        ReadCase {
            case: "the busy model, every execution of it",
            model: Model::Busy,
            resolved: None,
        },
        ReadCase {
            case: "the busy model, narrowed to asset A",
            model: Model::Busy,
            resolved: Some(Asset::A),
        },
        ReadCase {
            case: "the busy model, narrowed to asset B",
            model: Model::Busy,
            resolved: Some(Asset::B),
        },
        ReadCase {
            case: "the other model, every execution of it",
            model: Model::Other,
            resolved: None,
        },
        ReadCase {
            case: "the other model, narrowed to an asset none of its executions resolve to",
            model: Model::Other,
            resolved: Some(Asset::B),
        },
        ReadCase {
            case: "a model no execution is recorded against",
            model: Model::Idle,
            resolved: None,
        },
        ReadCase {
            case: "a model no execution is recorded against, narrowed to asset A",
            model: Model::Idle,
            resolved: Some(Asset::A),
        },
    ]
}

#[tokio::test]
async fn a_summary_counts_the_models_own_executions_by_state() {
    let fixture = fixture().await;

    for read in read_table() {
        let case = read.case;
        let model_id = fixture.model_id(read.model).to_string();
        let query = read
            .resolved
            .map(|asset| resolve_to_query(fixture.asset_id(asset)))
            .unwrap_or_default();
        let (status, body) = describe_summary(&fixture.router, &model_id, &query).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["computationModelId"].as_str(),
            Some(model_id.as_str()),
            "case {case:?}: unexpected computationModelId in {body}"
        );
        assert_eq!(
            body["resolveTo"]["assetId"].as_str(),
            read.resolved.map(|asset| fixture.asset_id(asset)),
            "case {case:?}: unexpected resolveTo in {body}"
        );

        let expected = expected_counts(&fixture.rows, read.model, read.resolved);
        let reported = reported_counts(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the counts {expected:?} the store holds, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_action_executed_through_the_api_shows_in_that_models_summary() {
    let fixture = fixture().await;
    let model_id = create_computation_model(&fixture.router, "fresh-anomaly").await;

    // A model nothing has been executed on reports every declared state at zero.
    let (status, body) = describe_summary(&fixture.router, &model_id, "").await;
    assert_eq!(
        status, 200,
        "a fresh model: expected 200, got {status} {body}"
    );
    let zeroed: BTreeMap<String, String> = EXECUTION_STATES
        .iter()
        .map(|state| ((*state).to_string(), "0".to_string()))
        .collect();
    assert_eq!(
        reported_counts(&body),
        zeroed,
        "a fresh model: expected every state at zero, got {body}"
    );
    assert!(
        body["resolveTo"].is_null(),
        "a request naming no resolved resource must report none, got {body}"
    );

    // An action is executed only on a settled model, so wait for the transition the
    // create owes, then take the actions the model's computation type publishes.
    tokio::time::sleep(settled()).await;
    let (status, described) = call(
        &fixture.router,
        "GET",
        &format!("/computation-models/{model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe {model_id}: got {status} {described}");
    let definitions: Vec<String> = described["actionDefinitions"]
        .as_array()
        .unwrap_or_else(|| panic!("no actionDefinitions in {described}"))
        .iter()
        .map(|definition| {
            definition["actionDefinitionId"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert!(
        definitions.len() >= 2,
        "this suite needs two published actions, got {definitions:?}"
    );

    // Two runs: one resolving to an asset, one resolving to nothing, so the
    // narrowed summary and the unnarrowed one answer different counts.
    for (at, resolve_to) in [(0, Some(fixture.asset_a.clone())), (1, None)] {
        let mut request = json!({
            "targetResource": { "computationModelId": model_id },
            "actionDefinitionId": definitions[at],
            "actionPayload": { "stringValue": "{}" },
        });
        if let Some(asset_id) = resolve_to {
            request["resolveTo"] = json!({ "assetId": asset_id });
        }
        let (status, taken) = call(&fixture.router, "POST", "/actions", Some(request)).await;
        assert_eq!(status, 202, "execute action {at}: got {status} {taken}");
    }

    // However the clock has fallen since, both runs are accounted for exactly once:
    // the counts the summary reports add up to the two executions the store holds.
    let (status, body) = describe_summary(&fixture.router, &model_id, "").await;
    assert_eq!(status, 200, "after two executes: got {status} {body}");
    let total: u64 = reported_counts(&body)
        .values()
        .map(|count| count.parse::<u64>().expect("a decimal count"))
        .sum();
    assert_eq!(
        total, 2,
        "after two executes: expected the two executions to be counted once each, got {body}"
    );

    // Once both runs are due to have ended, the summary reports them under the
    // state a run that finished reports.
    tokio::time::sleep(run_over()).await;
    let (status, body) = describe_summary(&fixture.router, &model_id, "").await;
    assert_eq!(status, 200, "after the runs ended: got {status} {body}");
    let mut expected: BTreeMap<String, String> = zeroed.clone();
    expected.insert(EXECUTION_STATE_COMPLETED.to_string(), "2".to_string());
    assert_eq!(
        reported_counts(&body),
        expected,
        "after the runs ended: expected both runs finished, got {body}"
    );

    // Narrowed to the asset one of the two runs resolves to, only that run counts.
    let (status, body) = describe_summary(
        &fixture.router,
        &model_id,
        &resolve_to_query(&fixture.asset_a),
    )
    .await;
    assert_eq!(status, 200, "narrowed to asset A: got {status} {body}");
    let mut expected: BTreeMap<String, String> = zeroed.clone();
    expected.insert(EXECUTION_STATE_COMPLETED.to_string(), "1".to_string());
    assert_eq!(
        reported_counts(&body),
        expected,
        "narrowed to asset A: expected only the run resolving to it, got {body}"
    );
    assert_eq!(
        body["resolveTo"]["assetId"].as_str(),
        Some(fixture.asset_a.as_str()),
        "narrowed to asset A: expected that asset reported back, got {body}"
    );

    // The executions of one model are not the executions of another: the model the
    // seeded table calls idle still reports every state at zero.
    let (status, body) = describe_summary(&fixture.router, fixture.model_id(Model::Idle), "").await;
    assert_eq!(status, 200, "the idle model: got {status} {body}");
    assert_eq!(
        reported_counts(&body),
        zeroed,
        "the idle model: another model's executions must not be counted, got {body}"
    );
}

/// One row of the table of refusals: the request, and the answer the rule owes it.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    /// Which model the request names, or `None` for an id no create made.
    model: Option<Model>,
    /// The query string the request carries.
    query: String,
    status: u16,
    code: &'static str,
    /// The message the refusal carries.
    message: String,
}

#[tokio::test]
async fn a_summary_of_what_the_store_does_not_hold_is_a_modelled_refusal() {
    let fixture = fixture().await;
    let asset_a = fixture.asset_a.clone();

    let cases = vec![
        RefusalCase {
            case: "a computationModelId no create made",
            model: None,
            query: String::new(),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Computation Model with ID {ABSENT_ID} not found"),
        },
        RefusalCase {
            case: "a computationModelId no create made, with a resolved resource",
            model: None,
            query: resolve_to_query(&asset_a),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Computation Model with ID {ABSENT_ID} not found"),
        },
        RefusalCase {
            case: "a resolved resource no create made",
            model: Some(Model::Busy),
            query: resolve_to_query(ABSENT_ID),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Asset with ID {ABSENT_ID} not found"),
        },
        RefusalCase {
            case: "a resolved-resource type with no id",
            model: Some(Model::Busy),
            query: "resolveToResourceType=ASSET".to_string(),
            status: 400,
            code: "InvalidRequestException",
            message: "resolveToResourceId is required when resolveToResourceType is specified."
                .to_string(),
        },
        RefusalCase {
            case: "a resolved-resource id with no type",
            model: Some(Model::Busy),
            query: format!("resolveToResourceId={asset_a}"),
            status: 400,
            code: "InvalidRequestException",
            message: "resolveToResourceType is required when resolveToResourceId is specified."
                .to_string(),
        },
    ];

    for refusal in &cases {
        let case = refusal.case;
        let model_id = refusal
            .model
            .map(|model| fixture.model_id(model).to_string())
            .unwrap_or_else(|| ABSENT_ID.to_string());
        let (status, body) =
            describe_summary(&fixture.router, &model_id, &refusal.query).await;
        assert_eq!(
            status, refusal.status,
            "case {case:?}: expected {}, got {status} {body}",
            refusal.status
        );
        assert!(
            error_code(&body).ends_with(refusal.code),
            "case {case:?}: expected {}, got {body}",
            refusal.code
        );
        assert_eq!(
            error_message(&body),
            refusal.message.as_str(),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }
}

#[tokio::test]
async fn reading_a_summary_changes_nothing_a_later_caller_sees() {
    let fixture = fixture().await;
    let model_id = fixture.model_id(Model::Busy).to_string();
    let executions_query =
        format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={model_id}");

    let (_, before) = call(
        &fixture.router,
        "GET",
        &format!("/executions?{executions_query}"),
        None,
    )
    .await;

    let (status, first) = describe_summary(&fixture.router, &model_id, "").await;
    assert_eq!(status, 200, "the first read: got {status} {first}");
    let (status, second) = describe_summary(&fixture.router, &model_id, "").await;
    assert_eq!(status, 200, "the second read: got {status} {second}");
    assert_eq!(
        reported_counts(&first),
        reported_counts(&second),
        "two reads of one summary must agree, got {first} then {second}"
    );

    let (_, after) = call(
        &fixture.router,
        "GET",
        &format!("/executions?{executions_query}"),
        None,
    )
    .await;
    assert_eq!(
        before["executionSummaries"], after["executionSummaries"],
        "reading a summary must leave the executions it counted untouched, got {before} \
         then {after}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue blueprint RUN slice: `StartBlueprintRun` and the
//! `GetBlueprintRun` that reads back what it recorded.
//!
//! RULE: a start of a blueprint whose registration has finished records ONE run of that
//! blueprint under a fresh id, lays out the workflow that run produces, and answers with the
//! id; every later read of the id reports the run the start wrote — the blueprint it is of,
//! the `Parameters` and `RoleArn` the request carried, the workflow it created, the instant
//! it started, and the modelled running state until the span the clock owed it has passed —
//! while a start naming no registered blueprint, a start of a registration still in flight,
//! and a read of an id the named blueprint holds no run under are each refused and record
//! nothing.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: whether the start carries
//! `Parameters` and in what form and length, the form and length of the `RoleArn`, the form
//! of the blueprint name, how many runs of one blueprint are in flight, how long ago a run
//! started, and how a requested blueprint or run id misses.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateWorkflowInput;
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, WorkflowEntry, BLUEPRINT_RUN_SETTLES_AFTER,
    BLUEPRINT_SETTLES_AFTER, MAX_WORKFLOWS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::far_future;
use crate::wire::{assert_echoed, call, emulator_on, error_code, error_message, ok};

/// The longest `RoleArn` the model accepts: `OrchestrationIAMRoleArn`, "len 1..1024"
/// (`smithy_query operation=StartBlueprintRun`).
const MAX_ROLE_ARN_LENGTH: usize = 1024;

/// The longest `Parameters` the model accepts: `BlueprintParameters`, "len 1..131072"
/// (`smithy_query operation=StartBlueprintRun`).
const MAX_PARAMETERS_LENGTH: usize = 131_072;

/// The longest `BlueprintName` a start may name: `OrchestrationNameString`, "len 1..128"
/// (`smithy_query operation=StartBlueprintRun`).
const MAX_NAME_LENGTH: usize = 128;

/// The `BlueprintRunState` a run is in until the clock owes it the next one, and the one it
/// owes a run that created the workflow it lays out: the modelled values "Running — The
/// blueprint run is in progress", "Succeeded — The blueprint run completed successfully" and
/// "Failed — The blueprint run failed and rollback is complete" (`smithy_query
/// shape=BlueprintRun`, member `State`).
const STATE_RUNNING: &str = "RUNNING";
const STATE_SUCCEEDED: &str = "SUCCEEDED";
const STATE_FAILED: &str = "FAILED";

/// The `BlueprintStatus` a registration carries while it is still in flight: "Creating — The
/// blueprint registration is in progress" (`smithy_query shape=Blueprint`, member `Status`).
/// A blueprint in it has no active definition for a run to lay out.
const STATUS_CREATING: &str = "CREATING";

/// An IAM role ARN of exactly `length` characters, built as the modelled pattern
/// `^arn:aws[^:]*:iam::[0-9]*:role/.+$` requires.
fn role_arn_of_length(length: usize) -> String {
    let prefix = "arn:aws:iam::000000000000:role/";
    format!("{prefix}{}", "r".repeat(length - prefix.len()))
}

/// A `Parameters` JSON string of exactly `length` characters: one member whose value is
/// padded out, so the string stays the JSON a blueprint's parameters are sent as.
fn parameters_of_length(length: usize) -> String {
    let wrapper = r#"{"WorkflowName":""}"#;
    format!(
        "{{\"WorkflowName\":\"{}\"}}",
        "w".repeat(length - wrapper.len())
    )
}

/// An emulator and the engine behind it, so a suite can move a stored record's instants.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    (router, engine)
}

/// Pull a stored blueprint's instants — and those of every run of it — back by `elapsed`, so
/// a suite can observe what the clock owes them without sleeping for the span.
pub(crate) fn rewind(engine: &Arc<engine::InMemoryEngine>, name: &str, elapsed: Duration) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(name)
        .unwrap_or_else(|| panic!("{name} was registered"));
    let back = |at: SystemTime| at.checked_sub(elapsed).expect("an instant in range");
    entry.created_on = back(entry.created_on);
    entry.last_modified_on = back(entry.last_modified_on);
    entry.settles_at = entry.settles_at.map(back);
    for run in &mut entry.runs {
        run.started_on = back(run.started_on);
        run.settles_at = run.settles_at.map(back);
        run.completed_on = run.completed_on.map(back);
    }
}

/// Put the instants the clock owes a stored blueprint, and every run of it still in flight,
/// at `at`.
///
/// A row that means "the clock has not reached the due instant yet" holds that instant far
/// enough ahead that the wall clock cannot cross it while the row's calls are made: the
/// status and the state turn on the stored due instant, so positioning the instant is what
/// fixes which side of it a call lands on, and a margin measured against the clock is not.
fn set_due_instants(engine: &Arc<engine::InMemoryEngine>, name: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(name)
        .unwrap_or_else(|| panic!("{name} was registered"));
    if entry.settles_at.is_some() {
        entry.settles_at = Some(at);
    }
    for run in &mut entry.runs {
        if run.settles_at.is_some() {
            run.settles_at = Some(at);
        }
    }
}

/// How many runs the store holds of `name`.
pub(crate) fn stored_runs(engine: &Arc<engine::InMemoryEngine>, name: &str) -> usize {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .blueprint(name)
        .map_or(0, |blueprint| blueprint.runs.len())
}

/// Register `name` and move its registration past the span the clock owed it, so it is the
/// finished registration a run may be started of.
pub(crate) async fn register_active(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    ok(
        router,
        "CreateBlueprint",
        &json!({
            "Name": name,
            "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
        }),
    )
    .await;
    rewind(engine, name, BLUEPRINT_SETTLES_AFTER);
}

/// Start a run and return the `RunId` the service minted for it.
pub(crate) async fn start(router: &MultiServiceRouter, body: &Value) -> String {
    let started = ok(router, "StartBlueprintRun", body).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartBlueprintRun must report a RunId, got {started}"))
        .to_string()
}

/// The `BlueprintRun` a `GetBlueprintRun` answer reports.
pub(crate) async fn get_run(router: &MultiServiceRouter, blueprint: &str, run_id: &str) -> Value {
    let read = ok(
        router,
        "GetBlueprintRun",
        &json!({ "BlueprintName": blueprint, "RunId": run_id }),
    )
    .await;
    read.get("BlueprintRun")
        .cloned()
        .unwrap_or_else(|| panic!("GetBlueprintRun must report a BlueprintRun, got {read}"))
}

// ---------------------------------------------------------------------------
// Start, then read back
// ---------------------------------------------------------------------------

/// What `StartBlueprintRun` records is what `GetBlueprintRun` reports: the start answers
/// with a fresh id, and the run that id addresses carries the blueprint it is of and every
/// member the request sent.
#[tokio::test]
async fn started_runs_are_read_back_from_the_store() {
    let long_role_arn = role_arn_of_length(MAX_ROLE_ARN_LENGTH);
    let long_parameters = parameters_of_length(MAX_PARAMETERS_LENGTH);
    let long_name = "n".repeat(MAX_NAME_LENGTH);

    // (case, the blueprint the run is of, the start request's own members)
    let cases: Vec<(&str, String, Value)> = vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "required members only",
            "etl-lifecycle".to_string(),
            json!({ "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole" }),
        ),
        (
            "parameters as a JSON object string",
            "parameterised".to_string(),
            json!({
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": r#"{"WorkflowName":"ingest","DatabaseName":"sales"}"#,
            }),
        ),
        (
            "parameters as the shortest string the model admits",
            "one-char-parameters".to_string(),
            json!({
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": "{",
            }),
        ),
        (
            "parameters at the modelled length bound",
            "long-parameters".to_string(),
            json!({
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": long_parameters,
            }),
        ),
        (
            // The pattern admits any partition, so a China or GovCloud role ARN is a role
            // ARN.
            "a role ARN in another partition",
            "other-partition".to_string(),
            json!({ "RoleArn": "arn:aws-cn:iam::000000000000:role/GlueBlueprintRole" }),
        ),
        (
            "a role ARN carrying a path",
            "role-path".to_string(),
            json!({
                "RoleArn": "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole",
            }),
        ),
        (
            "a role ARN at the modelled length bound",
            "long-role".to_string(),
            json!({ "RoleArn": long_role_arn }),
        ),
        (
            // The blueprint name's pattern (`^[\.\-_A-Za-z0-9]+$`) admits both cases and
            // every one of `.`, `-`, `_`.
            "a blueprint name carrying dots, dashes, underscores and mixed case",
            "Mixed.Case-blueprint_42".to_string(),
            json!({ "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole" }),
        ),
        (
            "a blueprint name at the modelled length bound",
            long_name,
            json!({ "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole" }),
        ),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, blueprint, members) in &cases {
        let (router, engine) = emulator_with_engine();
        register_active(&router, &engine, blueprint).await;

        let mut request = members.clone();
        request["BlueprintName"] = json!(blueprint);
        let run_id = start(&router, &request).await;

        // "The run ID for this blueprint run": an `IdString`, "len 1..255", and the id the
        // service minted rather than anything the request chose.
        assert!(
            (1..=255).contains(&run_id.chars().count()),
            "case {case}: a RunId must be an IdString of 1..255 characters, got {run_id:?}"
        );
        assert!(
            run_id
                .chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_uppercase()),
            "case {case}: expected a minted lowercase hexadecimal run id, got {run_id:?}"
        );
        assert!(
            !minted.contains(&run_id),
            "case {case}: every run gets its own id, got {run_id:?} twice in {minted:?}"
        );
        minted.push(run_id.clone());

        let read = get_run(&router, blueprint, &run_id).await;
        // Every member the start carried reappears unchanged, beside the address of the run.
        let expected = json!({
            "BlueprintName": blueprint,
            "RunId": run_id,
            "RoleArn": members["RoleArn"],
            "Parameters": members.get("Parameters").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);

        // "Running — The blueprint run is in progress": the run is still in flight the
        // instant the start returns, so it has not completed and nothing has gone wrong.
        assert_eq!(
            read["State"].as_str(),
            Some(STATE_RUNNING),
            "case {case}: expected {STATE_RUNNING} straight after the start, got {read}"
        );
        assert!(
            read["StartedOn"].as_f64().is_some_and(|at| at > 0.0),
            "case {case}: expected a StartedOn instant, got {read}"
        );
        for member in ["CompletedOn", "ErrorMessage", "RollbackErrorMessage"] {
            assert!(
                read.get(member).is_none_or(Value::is_null),
                "case {case}: a run in flight reports no {member}, got {read}"
            );
        }
        // "The name of a workflow that is created as a result of a successful blueprint run":
        // the run laid one out, so it names it.
        assert!(
            read["WorkflowName"].as_str().is_some_and(|name| !name.is_empty()),
            "case {case}: expected the workflow the run created, got {read}"
        );

        // The run is the one thing the start wrote: the blueprint holds exactly it.
        assert_eq!(
            stored_runs(&engine, blueprint),
            1,
            "case {case}: one start records one run"
        );
    }
}

/// Each start records its OWN run: a second start of a blueprint whose first run is still in
/// flight is served, gets its own id, and neither run's members are replaced by the other's.
#[tokio::test]
async fn each_start_records_its_own_run() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "concurrent").await;

    // (case, the parameters this start carries)
    let rows: &[(&str, &str)] = &[
        ("the first run", r#"{"WorkflowName":"first"}"#),
        ("a second run while the first is in flight", r#"{"WorkflowName":"second"}"#),
        ("a third run while both are in flight", r#"{"WorkflowName":"third"}"#),
    ];

    let mut started: Vec<(&str, String, &str)> = Vec::new();
    for (case, parameters) in rows {
        let run_id = start(
            &router,
            &json!({
                "BlueprintName": "concurrent",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": parameters,
            }),
        )
        .await;
        assert!(
            !started.iter().any(|(_, already, _)| already == &run_id),
            "case {case}: expected an id of its own, got {run_id:?} again"
        );
        started.push((case, run_id, parameters));
    }

    assert_eq!(
        stored_runs(&engine, "concurrent"),
        rows.len(),
        "case the whole table: each start records one run"
    );

    for (case, run_id, parameters) in &started {
        let read = get_run(&router, "concurrent", run_id).await;
        assert_eq!(
            read["Parameters"].as_str(),
            Some(*parameters),
            "case {case}: each run keeps its own Parameters, got {read}"
        );
        assert_eq!(
            read["RunId"].as_str(),
            Some(run_id.as_str()),
            "case {case}: a read reports the run it was asked for, got {read}"
        );
        assert_eq!(
            read["State"].as_str(),
            Some(STATE_RUNNING),
            "case {case}: a run in flight is still running, got {read}"
        );
    }
}

/// A run leaves the running state for the terminal one when the instant the clock owed it
/// has passed, and not before — and the change is materialised by whichever read happens
/// next, so nothing sleeps and two reads of one run agree. A run that created the workflow
/// it lays out comes to rest in the modelled succeeded state, carrying that workflow's name
/// and no error.
///
/// The state turns on the stored DUE instant, so each row positions that instant: a row that
/// reads a run still in flight holds it ahead of the clock, and a row that reads a settled
/// one leaves it at or behind the clock, where the clock only moves further.
#[tokio::test]
async fn a_run_reaches_its_terminal_state_once_the_clock_owes_it() {
    // (case, how long ago the run started, whether its due instant is held ahead of the
    //  clock, the state a read must report)
    let rows: &[(&str, Duration, bool, &str)] = &[
        ("the instant it started", Duration::ZERO, false, STATE_RUNNING),
        (
            "halfway through the span, the due instant still ahead",
            BLUEPRINT_RUN_SETTLES_AFTER / 2,
            true,
            STATE_RUNNING,
        ),
        (
            // A run as old as the span still reports the state its DUE instant says, not
            // one computed from how long ago it started.
            "a hair short of the span, the due instant still ahead",
            BLUEPRINT_RUN_SETTLES_AFTER - Duration::from_millis(1),
            true,
            STATE_RUNNING,
        ),
        ("exactly at the due instant", BLUEPRINT_RUN_SETTLES_AFTER, false, STATE_SUCCEEDED),
        ("well past the due instant", BLUEPRINT_RUN_SETTLES_AFTER * 4, false, STATE_SUCCEEDED),
    ];

    for (case, elapsed, due_held_ahead, expected) in rows {
        let (router, engine) = emulator_with_engine();
        register_active(&router, &engine, "settling").await;
        let run_id = start(
            &router,
            &json!({
                "BlueprintName": "settling",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": r#"{"WorkflowName":"settling-workflow"}"#,
            }),
        )
        .await;
        rewind(&engine, "settling", *elapsed);
        if *due_held_ahead {
            set_due_instants(&engine, "settling", far_future());
        }

        let read = get_run(&router, "settling", &run_id).await;
        assert_eq!(
            read["State"].as_str(),
            Some(*expected),
            "case {case}: expected {expected} {elapsed:?} after the start, got {read}"
        );
        // "The name of a workflow that is created as a result of a successful blueprint run":
        // the workflow this run laid out, whatever state the clock has it in.
        assert_eq!(
            read["WorkflowName"].as_str(),
            Some("settling-workflow"),
            "case {case}: expected the workflow the run created, got {read}"
        );
        // A run this store served has seen no error, in flight or finished.
        assert!(
            read.get("ErrorMessage").is_none_or(Value::is_null),
            "case {case}: a run that created its workflow carries no error, got {read}"
        );

        if *expected == STATE_RUNNING {
            assert!(
                read.get("CompletedOn").is_none_or(Value::is_null),
                "case {case}: a run still in flight has not completed, got {read}"
            );
        } else {
            // "The date and time that the blueprint run completed": the DUE instant, which
            // is on or after the instant it started.
            let started_on = read["StartedOn"].as_f64();
            let completed_on = read["CompletedOn"].as_f64();
            assert!(
                completed_on.is_some_and(|at| started_on.is_some_and(|start| at >= start)),
                "case {case}: expected a CompletedOn at or after StartedOn, got {read}"
            );
        }

        // Reading again reports the same run: the state comes from the DUE instant the start
        // recorded, not from the instant somebody read it, and the start's own members are
        // untouched by the read.
        let again = get_run(&router, "settling", &run_id).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one run must agree, got {again} then {read}"
        );
        assert_eq!(
            read["Parameters"].as_str(),
            Some(r#"{"WorkflowName":"settling-workflow"}"#),
            "case {case}: the recorded run must survive the state change, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// The workflow a run produces
// ---------------------------------------------------------------------------

/// A run produces a WORKFLOW: the start lays one out, a `GetWorkflow` reads it back, and it
/// carries "the details of the blueprint that this particular workflow is created from" —
/// the blueprint and the run that created it. The name is the one the caller asked the run
/// to create the workflow under, and the blueprint's own name when the parameters name none
/// this store can use. The rows differ along the form of the parameters the start carried.
#[tokio::test]
async fn a_run_lays_out_the_workflow_its_parameters_name() {
    // The longest workflow name the model admits: `NameString`, "len 1..255"
    // (`smithy_query operation=CreateWorkflow`).
    const MAX_WORKFLOW_NAME_LENGTH: usize = 255;
    let long_workflow_name = "w".repeat(MAX_WORKFLOW_NAME_LENGTH);
    let too_long_workflow_name = "w".repeat(MAX_WORKFLOW_NAME_LENGTH + 1);

    // (case, the Parameters the start carries, the workflow the run must create)
    let rows: Vec<(&str, Option<String>, String)> = vec![
        ("no parameters at all", None, "layout-blueprint".to_string()),
        (
            "parameters naming the workflow",
            Some(r#"{"WorkflowName":"ingest-sales"}"#.to_string()),
            "ingest-sales".to_string(),
        ),
        (
            "parameters naming the workflow beside other keys",
            Some(r#"{"DatabaseName":"sales","WorkflowName":"ingest-v2","Depth":3}"#.to_string()),
            "ingest-v2".to_string(),
        ),
        (
            "a workflow name at the modelled length bound",
            Some(format!(r#"{{"WorkflowName":"{long_workflow_name}"}}"#)),
            long_workflow_name.clone(),
        ),
        (
            "a workflow name one character past the length bound",
            Some(format!(r#"{{"WorkflowName":"{too_long_workflow_name}"}}"#)),
            "layout-blueprint".to_string(),
        ),
        (
            "an empty workflow name",
            Some(r#"{"WorkflowName":""}"#.to_string()),
            "layout-blueprint".to_string(),
        ),
        (
            "a workflow name that is not a string",
            Some(r#"{"WorkflowName":7}"#.to_string()),
            "layout-blueprint".to_string(),
        ),
        (
            "parameters carrying no workflow name",
            Some(r#"{"DatabaseName":"sales"}"#.to_string()),
            "layout-blueprint".to_string(),
        ),
        (
            "parameters that are not JSON",
            Some("WorkflowName=ingest".to_string()),
            "layout-blueprint".to_string(),
        ),
        (
            "parameters that are a JSON list",
            Some(r#"["WorkflowName","ingest"]"#.to_string()),
            "layout-blueprint".to_string(),
        ),
    ];

    for (case, parameters, expected) in &rows {
        let (router, engine) = emulator_with_engine();
        register_active(&router, &engine, "layout-blueprint").await;

        let mut request = json!({
            "BlueprintName": "layout-blueprint",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        });
        if let Some(parameters) = parameters {
            request["Parameters"] = json!(parameters);
        }
        let run_id = start(&router, &request).await;

        let read = get_run(&router, "layout-blueprint", &run_id).await;
        assert_eq!(
            read["WorkflowName"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the run to create workflow {expected:?}, got {read}"
        );

        // The workflow the run produced is a workflow of this account: a read of it reports
        // the blueprint run that created it.
        let workflow_read = ok(&router, "GetWorkflow", &json!({ "Name": expected })).await;
        let workflow = workflow_read
            .get("Workflow")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a Workflow, got {workflow_read}"));
        assert_eq!(
            workflow["Name"].as_str(),
            Some(expected.as_str()),
            "case {case}: expected the workflow the run created, got {workflow}"
        );
        let expected_details = json!({
            "BlueprintName": "layout-blueprint",
            "RunId": run_id,
        });
        assert_echoed(case, &json!({ "BlueprintDetails": expected_details }), &workflow);
        // The members a `CreateWorkflow` sets come from the published archive, which nothing
        // here reads, so none of them is invented for the workflow a run laid out.
        for member in ["Description", "DefaultRunProperties", "MaxConcurrentRuns", "LastRun"] {
            assert!(
                workflow.get(member).is_none_or(Value::is_null),
                "case {case}: {member} comes from the published archive and must not be \
                 invented, got {workflow}"
            );
        }
    }
}

/// A run never overwrites a workflow this account already holds: the start is served, the
/// run creates nothing and comes to rest in the modelled failed state naming the workflow it
/// could not create, and the stored workflow is left exactly as its create wrote it.
#[tokio::test]
async fn a_run_whose_workflow_name_is_taken_creates_nothing() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "clashing").await;
    ok(
        &router,
        "CreateWorkflow",
        &json!({
            "Name": "already-there",
            "Description": "Registered by a create.",
            "MaxConcurrentRuns": 3,
        }),
    )
    .await;

    // (case, the Parameters the start carries, the workflow it clashes with)
    let rows: &[(&str, &str, &str)] = &[
        (
            "a workflow a create registered",
            r#"{"WorkflowName":"already-there"}"#,
            "already-there",
        ),
        (
            // The first run of this blueprint lays out the workflow named after it; a second
            // run with the same parameters finds that workflow already there.
            "the workflow an earlier run of the same blueprint laid out",
            r#"{"DatabaseName":"sales"}"#,
            "clashing",
        ),
    ];

    // The earlier run the second row clashes with.
    start(
        &router,
        &json!({
            "BlueprintName": "clashing",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        }),
    )
    .await;

    for (case, parameters, workflow) in rows {
        let run_id = start(
            &router,
            &json!({
                "BlueprintName": "clashing",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": parameters,
            }),
        )
        .await;
        rewind(&engine, "clashing", BLUEPRINT_RUN_SETTLES_AFTER);

        let read = get_run(&router, "clashing", &run_id).await;
        assert_eq!(
            read["State"].as_str(),
            Some(STATE_FAILED),
            "case {case}: a run that could create no workflow fails, got {read}"
        );
        // "If a blueprint run has an error, there will not be a workflow created."
        assert!(
            read.get("WorkflowName").is_none_or(Value::is_null),
            "case {case}: a failed run names no workflow, got {read}"
        );
        assert_eq!(
            read["ErrorMessage"].as_str(),
            Some(
                format!(
                    "The blueprint run could not create workflow {workflow}: a workflow of that \
                     name already exists."
                )
                .as_str()
            ),
            "case {case}: the run must say which workflow it could not create, got {read}"
        );
    }

    // The workflow the create registered is still the create's, member for member: a run
    // that clashed with it changed nothing.
    let workflow_read = ok(&router, "GetWorkflow", &json!({ "Name": "already-there" })).await;
    let workflow = workflow_read["Workflow"].clone();
    assert_echoed(
        "the workflow a create registered",
        &json!({
            "Name": "already-there",
            "Description": "Registered by a create.",
            "MaxConcurrentRuns": 3,
            "BlueprintDetails": Value::Null,
        }),
        &workflow,
    );
}

/// A run produces a workflow, so a store already holding as many workflows as it will
/// cannot serve a start that would create another: it is refused with the modelled
/// `ResourceNumberLimitExceededException` and records no run. A start whose workflow name is
/// already held creates nothing anyway, so it is still served at the ceiling — and freeing a
/// slot lets a start that creates one through again.
#[tokio::test]
async fn a_start_that_would_pass_the_workflow_ceiling_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "at-the-ceiling").await;

    // Reaching the ceiling over the wire would take a thousand requests and would exercise
    // the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_WORKFLOWS {
            let input = CreateWorkflowInput {
                name: format!("seeded-{index}"),
                ..CreateWorkflowInput::default()
            };
            store.insert_workflow(WorkflowEntry::from_create_input(&input, SystemTime::now()));
        }
    }

    let fresh_workflow = json!({
        "BlueprintName": "at-the-ceiling",
        "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        "Parameters": r#"{"WorkflowName":"one-too-many"}"#,
    });
    let (status, body) = call(&router, "StartBlueprintRun", &fresh_workflow).await;
    assert_eq!(
        status, 400,
        "case workflow {} of {MAX_WORKFLOWS}: the start must be refused, got {status} with \
         {body}",
        MAX_WORKFLOWS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one workflow too many: expected ResourceNumberLimitExceededException, got {body}"
    );
    assert_eq!(
        stored_runs(&engine, "at-the-ceiling"),
        0,
        "case one workflow too many: a refused start records no run"
    );

    // A run whose workflow is already there creates none, so the ceiling does not stop it.
    let run_id = start(
        &router,
        &json!({
            "BlueprintName": "at-the-ceiling",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
            "Parameters": r#"{"WorkflowName":"seeded-0"}"#,
        }),
    )
    .await;
    rewind(&engine, "at-the-ceiling", BLUEPRINT_RUN_SETTLES_AFTER);
    let read = get_run(&router, "at-the-ceiling", &run_id).await;
    assert_eq!(
        read["State"].as_str(),
        Some(STATE_FAILED),
        "case a workflow already held: the run creates nothing and fails, got {read}"
    );

    // Freeing a slot lets a start that creates a workflow through: the ceiling bounds what is
    // stored rather than latching on how many starts were ever made.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().workflows.remove("seeded-1");
    }
    let run_id = start(&router, &fresh_workflow).await;
    let read = get_run(&router, "at-the-ceiling", &run_id).await;
    assert_eq!(
        read["WorkflowName"].as_str(),
        Some("one-too-many"),
        "case a freed slot: the start must create its workflow, got {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A start for a name this account holds no blueprint under is refused with the modelled
/// `EntityNotFoundException` naming the blueprint that was asked for, and records nothing.
/// The rows differ along how the requested name misses — the model documents no case fold
/// for a blueprint name, so a differently-cased name is a different blueprint.
#[tokio::test]
async fn starting_a_run_of_an_unknown_blueprint_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "StoredBlueprint").await;

    for (case, name) in [
        ("never registered", "no-such-blueprint"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Blueprint"),
        ("a stored name lowercased", "storedblueprint"),
        ("a stored name uppercased", "STOREDBLUEPRINT"),
    ] {
        let (status, body) = call(
            &router,
            "StartBlueprintRun",
            &json!({
                "BlueprintName": name,
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::BLUEPRINT_NOT_FOUND,
            "case {case}: the refusal is the recorded blueprint miss, got {body}"
        );
    }

    // The blueprint that IS registered can still be run, so the misses above are misses and
    // not a broken lookup — and only that one start recorded a run.
    start(
        &router,
        &json!({
            "BlueprintName": "StoredBlueprint",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        }),
    )
    .await;
    assert_eq!(
        stored_runs(&engine, "StoredBlueprint"),
        1,
        "case the stored name: only the served start recorded a run"
    );
}

/// A run may only be started of a blueprint with an ACTIVE definition: while the
/// registration is still in flight the start is refused with the modelled
/// `IllegalBlueprintStateException` saying the blueprint has no active definition, and
/// nothing is recorded; from the instant the clock owes the registration its terminal status,
/// the same start is served.
///
/// The refusal turns on the registration's stored DUE instant, so each row positions that
/// instant: a row whose start is refused holds it ahead of the clock, and a row whose start
/// is served leaves it at or behind the clock, where the clock only moves further.
#[tokio::test]
async fn starting_a_run_of_a_registration_in_flight_is_refused() {
    // (case, how long ago the blueprint was registered, whether its due instant is held
    //  ahead of the clock, whether the start is served)
    let rows: &[(&str, Duration, bool, bool)] = &[
        ("the instant it was registered", Duration::ZERO, false, false),
        (
            "halfway through the registration span, the due instant still ahead",
            BLUEPRINT_SETTLES_AFTER / 2,
            true,
            false,
        ),
        (
            // A registration as old as the span is still in flight while its DUE instant
            // is ahead of the clock.
            "a hair short of the registration span, the due instant still ahead",
            BLUEPRINT_SETTLES_AFTER - Duration::from_millis(1),
            true,
            false,
        ),
        ("exactly at the due instant", BLUEPRINT_SETTLES_AFTER, false, true),
        ("well past the due instant", BLUEPRINT_SETTLES_AFTER * 4, false, true),
    ];

    for (case, elapsed, due_held_ahead, served) in rows {
        let (router, engine) = emulator_with_engine();
        ok(
            &router,
            "CreateBlueprint",
            &json!({
                "Name": "in-flight",
                "BlueprintLocation": "s3://blueprint-bucket/in-flight.zip",
            }),
        )
        .await;
        rewind(&engine, "in-flight", *elapsed);
        if *due_held_ahead {
            set_due_instants(&engine, "in-flight", far_future());
        }

        let request = json!({
            "BlueprintName": "in-flight",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        });
        let (status, body) = call(&router, "StartBlueprintRun", &request).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case}: the start must be served, got {status} with {body}"
            );
            assert_eq!(
                stored_runs(&engine, "in-flight"),
                1,
                "case {case}: a served start records one run"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: the start must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("IllegalBlueprintStateException"),
            "case {case}: expected IllegalBlueprintStateException, got {body}"
        );
        // The blueprint the refused start named is the one whose registration is still in
        // flight, so it has no active definition for a run to lay out.
        let registration = ok(&router, "GetBlueprint", &json!({ "Name": "in-flight" })).await;
        assert_eq!(
            registration["Blueprint"]["Status"].as_str(),
            Some(STATUS_CREATING),
            "case {case}: expected a registration in flight, got {registration}"
        );
        assert_eq!(
            error_message(&body),
            "No active definition found for Blueprint: in-flight",
            "case {case}: the refusal must name the blueprint asked for, got {body}"
        );
        // Refused means nothing was recorded.
        assert_eq!(
            stored_runs(&engine, "in-flight"),
            0,
            "case {case}: a refused start records no run"
        );
    }
}

/// A read for an id the named blueprint holds no run under is refused with the modelled
/// `EntityNotFoundException` naming the address that was asked for: a run is addressed by
/// its blueprint AND its id, so one blueprint's run id is not another's.
#[tokio::test]
async fn reading_a_run_the_blueprint_holds_none_of_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "holder").await;
    register_active(&router, &engine, "other-holder").await;

    let held = start(
        &router,
        &json!({
            "BlueprintName": "holder",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        }),
    )
    .await;
    let elsewhere = start(
        &router,
        &json!({
            "BlueprintName": "other-holder",
            "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
        }),
    )
    .await;

    // (case, the blueprint the read names, the run id it asks for)
    let rows: Vec<(&str, &str, String)> = vec![
        ("an id nothing minted", "holder", "0".repeat(32)),
        (
            "another blueprint's run id",
            "holder",
            elsewhere.clone(),
        ),
        ("its own run id, under another blueprint", "other-holder", held.clone()),
        ("a prefix of a held id", "holder", held[..held.len() - 1].to_string()),
        ("a held id with one character added", "holder", format!("{held}0")),
        ("a held id uppercased", "holder", held.to_uppercase()),
    ];

    for (case, blueprint, run_id) in &rows {
        let (status, body) = call(
            &router,
            "GetBlueprintRun",
            &json!({ "BlueprintName": blueprint, "RunId": run_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {run_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {run_id:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Blueprint run {run_id} not found for blueprint {blueprint}."),
            "case {case}: the refusal must name the address asked for, got {body}"
        );
    }

    // Each held run still resolves under its OWN blueprint, so the misses above are misses.
    for (blueprint, run_id) in [("holder", &held), ("other-holder", &elsewhere)] {
        let read = get_run(&router, blueprint, run_id).await;
        assert_eq!(
            read["BlueprintName"].as_str(),
            Some(blueprint),
            "case the held run of {blueprint}: expected the stored run, got {read}"
        );
    }
}

/// A start outside the members' modelled constraints is refused at the boundary and records
/// nothing: each published length bound is served, one past it is refused, and a `RoleArn`
/// that is not an IAM role ARN of the modelled form (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`)
/// is refused whatever else it looks like.
#[tokio::test]
async fn a_start_outside_the_modelled_constraints_is_refused() {
    let (router, engine) = emulator_with_engine();
    register_active(&router, &engine, "constrained").await;

    // (case, the start request, whether it is served)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a role ARN with no arn prefix",
            json!({ "BlueprintName": "constrained", "RoleArn": "GlueBlueprintRole" }),
            false,
        ),
        (
            "an ARN of another service",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:s3:::blueprint-bucket/role/Glue",
            }),
            false,
        ),
        (
            "an IAM ARN that names no role",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:iam::000000000000:user/emulator",
            }),
            false,
        ),
        (
            "a role ARN with an empty role name",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:iam::000000000000:role/",
            }),
            false,
        ),
        (
            "an empty role ARN",
            json!({ "BlueprintName": "constrained", "RoleArn": "" }),
            false,
        ),
        (
            "a role ARN one character past the length bound",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": role_arn_of_length(MAX_ROLE_ARN_LENGTH + 1),
            }),
            false,
        ),
        (
            "no role ARN at all",
            json!({ "BlueprintName": "constrained" }),
            false,
        ),
        (
            "parameters one character past the length bound",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": parameters_of_length(MAX_PARAMETERS_LENGTH + 1),
            }),
            false,
        ),
        (
            "empty parameters",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": "",
            }),
            false,
        ),
        (
            "a blueprint name carrying a character the pattern excludes",
            json!({
                "BlueprintName": "constrained blueprint",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
            }),
            false,
        ),
        (
            "a blueprint name one character past the length bound",
            json!({
                "BlueprintName": "n".repeat(MAX_NAME_LENGTH + 1),
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
            }),
            false,
        ),
        (
            "an empty blueprint name",
            json!({
                "BlueprintName": "",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
            }),
            false,
        ),
        (
            "no blueprint name at all",
            json!({ "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole" }),
            false,
        ),
        (
            "the shortest role ARN the pattern admits",
            json!({ "BlueprintName": "constrained", "RoleArn": "arn:aws:iam:::role/r" }),
            true,
        ),
        (
            "a role ARN at the length bound",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": role_arn_of_length(MAX_ROLE_ARN_LENGTH),
            }),
            true,
        ),
        (
            "parameters at the length bound",
            json!({
                "BlueprintName": "constrained",
                "RoleArn": "arn:aws:iam::000000000000:role/GlueBlueprintRole",
                "Parameters": parameters_of_length(MAX_PARAMETERS_LENGTH),
            }),
            true,
        ),
    ];

    let mut served_starts = 0;
    for (case, request, served) in &rows {
        let (status, body) = call(&router, "StartBlueprintRun", request).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case}: the start must be served, got {status} with {body}"
            );
            served_starts += 1;
        } else {
            assert_eq!(
                status, 400,
                "case {case}: the start must be refused, got {status} with {body}"
            );
        }
        // Only the served rows left a run behind, so a refusal recorded nothing.
        assert_eq!(
            stored_runs(&engine, "constrained"),
            served_starts,
            "case {case}: only a served start records a run"
        );
    }
}

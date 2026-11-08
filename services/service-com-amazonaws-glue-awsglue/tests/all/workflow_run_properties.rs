// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the run properties of a Glue workflow run:
//! `PutWorkflowRunProperties` and the `GetWorkflowRunProperties` that reads them back.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a run's properties start as the workflow's `DefaultRunProperties`
//! with the start's own `RunProperties` over them, a put writes the keys it carries over the
//! ones the run already holds and leaves every other key as it was, and a read reports that
//! map as it stands — through `GetWorkflowRunProperties` and on the run itself. The
//! properties belong to the RUN, so a put on one run leaves the workflow's other runs alone;
//! a workflow or a run this account holds no record of is refused with 400 and the modelled
//! `EntityNotFoundException`.
//!
//! The rows differ along which properties the workflow and the start carried, how many puts
//! follow, how many keys each put carries, whether a put adds a key or overrides one, and
//! the form of the keys and values (empty, non-ASCII, many).

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::messages::{
    workflow_run_not_found, WORKFLOW_NOT_FOUND,
};
use service_com_amazonaws_glue_awsglue::state::MAX_RUN_PROPERTIES_PER_RUN;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The role ARN the job every entry-point trigger runs is registered with.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The job the entry-point trigger of every workflow below runs.
const JOB: &str = "run-properties-target-job";

/// The modelled error a miss is refused with (`smithy_query operation=GetWorkflowRunProperties`).
const NOT_FOUND: &str = "EntityNotFoundException";

/// The modelled error a put past the store's ceiling is refused with
/// (`smithy_query operation=PutWorkflowRunProperties`).
const LIMIT_EXCEEDED: &str = "ResourceNumberLimitExceededException";

/// Register the job the triggers below name, so their actions resolve.
pub async fn register_job(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateJob",
        &json!({
            "Name": JOB,
            "Role": ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
        }),
    )
    .await;
}

/// Register a workflow under `name` with the default run properties `defaults`, and bind an
/// on-demand entry point to it so runs of it can be started.
pub async fn register_workflow(router: &MultiServiceRouter, name: &str, defaults: &Value) {
    let mut create = json!({ "Name": name });
    if !defaults.is_null() {
        create["DefaultRunProperties"] = defaults.clone();
    }
    ok(router, "CreateWorkflow", &create).await;
    ok(
        router,
        "CreateTrigger",
        &json!({
            "Name": format!("{name}-entry-point"),
            "Type": "ON_DEMAND",
            "WorkflowName": name,
            "Actions": [{ "JobName": JOB }],
        }),
    )
    .await;
}

/// Start a run of `workflow` with the run properties `properties`, answering its `RunId`.
pub async fn start_run(router: &MultiServiceRouter, workflow: &str, properties: &Value) -> String {
    let mut start = json!({ "Name": workflow });
    if !properties.is_null() {
        start["RunProperties"] = properties.clone();
    }
    let started = ok(router, "StartWorkflowRun", &start).await;
    started
        .get("RunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartWorkflowRun must answer a RunId, got {started}"))
        .to_string()
}

/// The properties `GetWorkflowRunProperties` reports for one run, with an absent map read as
/// the empty one awsJson may omit.
pub async fn get_properties(router: &MultiServiceRouter, workflow: &str, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetWorkflowRunProperties",
        &json!({ "Name": workflow, "RunId": run_id }),
    )
    .await;
    match body.get("RunProperties") {
        None | Some(Value::Null) => json!({}),
        Some(properties) => properties.clone(),
    }
}

/// The properties the run itself reports through `GetWorkflowRun`, read the same way.
async fn run_properties(router: &MultiServiceRouter, workflow: &str, run_id: &str) -> Value {
    let body = ok(
        router,
        "GetWorkflowRun",
        &json!({ "Name": workflow, "RunId": run_id }),
    )
    .await;
    match body["Run"].get("WorkflowRunProperties") {
        None | Some(Value::Null) => json!({}),
        Some(properties) => properties.clone(),
    }
}

/// Write `properties` onto one run.
async fn put_properties(
    router: &MultiServiceRouter,
    workflow: &str,
    run_id: &str,
    properties: &Value,
) {
    ok(
        router,
        "PutWorkflowRunProperties",
        &json!({ "Name": workflow, "RunId": run_id, "RunProperties": properties }),
    )
    .await;
}

/// One row of the properties table: what the workflow and the start carried, the puts made
/// of the run, and the map the run must then hold.
struct PropertiesCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The workflow's `DefaultRunProperties`, or `null` for a workflow that named none.
    defaults: Value,
    /// The start's `RunProperties`, or `null` for a start that named none.
    started_with: Value,
    /// The `RunProperties` of each put, applied in order.
    puts: Vec<Value>,
    /// The map the run holds once every put has landed — written out, not derived.
    expected: Value,
}

/// The rows every suite below drives.
fn properties_cases() -> Vec<PropertiesCase> {
    vec![
        PropertiesCase {
            // The recorded shape: a run started with properties, then put to.
            case: "a put adding a key to the properties the start carried",
            defaults: Value::Null,
            started_with: json!({ "run-mode": "backfill" }),
            puts: vec![json!({ "checkpoint": "2026-10-25" })],
            expected: json!({ "run-mode": "backfill", "checkpoint": "2026-10-25" }),
        },
        PropertiesCase {
            case: "a put onto a run with no properties at all",
            defaults: Value::Null,
            started_with: Value::Null,
            puts: vec![json!({ "stage": "extract" })],
            expected: json!({ "stage": "extract" }),
        },
        PropertiesCase {
            case: "a put over the workflow's defaults",
            defaults: json!({ "region-tier": "primary", "stage": "extract" }),
            started_with: Value::Null,
            puts: vec![json!({ "stage": "transform" })],
            expected: json!({ "region-tier": "primary", "stage": "transform" }),
        },
        PropertiesCase {
            case: "a put overriding a key the start set over a default",
            defaults: json!({ "stage": "extract", "retries": "0" }),
            started_with: json!({ "stage": "transform" }),
            puts: vec![json!({ "stage": "load" })],
            expected: json!({ "stage": "load", "retries": "0" }),
        },
        PropertiesCase {
            case: "two puts, the second overriding the first",
            defaults: Value::Null,
            started_with: json!({ "stage": "extract" }),
            puts: vec![
                json!({ "stage": "transform", "rows": "1" }),
                json!({ "stage": "load" }),
            ],
            expected: json!({ "stage": "load", "rows": "1" }),
        },
        PropertiesCase {
            case: "a put carrying no properties leaves the run as it was",
            defaults: json!({ "stage": "extract" }),
            started_with: json!({ "rows": "7" }),
            puts: vec![json!({})],
            expected: json!({ "stage": "extract", "rows": "7" }),
        },
        PropertiesCase {
            case: "a put with an empty value and a non-ASCII key and value",
            defaults: Value::Null,
            started_with: json!({ "stage": "extract" }),
            puts: vec![json!({
                "stage": "",
                "jeu-de-données": "ventes-été",
            })],
            expected: json!({ "stage": "", "jeu-de-données": "ventes-été" }),
        },
        PropertiesCase {
            case: "a put carrying many keys at once",
            defaults: json!({ "stage": "extract" }),
            started_with: Value::Null,
            puts: vec![json!({
                "key-0": "0",
                "key-1": "1",
                "key-2": "2",
                "key-3": "3",
                "key-4": "4",
                "key-5": "5",
                "key-6": "6",
                "key-7": "7",
                "key-8": "8",
                "key-9": "9",
            })],
            expected: json!({
                "stage": "extract",
                "key-0": "0",
                "key-1": "1",
                "key-2": "2",
                "key-3": "3",
                "key-4": "4",
                "key-5": "5",
                "key-6": "6",
                "key-7": "7",
                "key-8": "8",
                "key-9": "9",
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Put, then read back
// ---------------------------------------------------------------------------

/// What a put writes is what both readers report: the keys the put carried are written over
/// the ones the run held and every other key is left as it was.
#[tokio::test]
async fn put_run_properties_are_read_back_from_the_store() {
    let router = emulator();
    register_job(&router).await;

    for (index, row) in properties_cases().iter().enumerate() {
        let case = row.case;
        let workflow = format!("run-properties-workflow-{index}");
        register_workflow(&router, &workflow, &row.defaults).await;
        let run_id = start_run(&router, &workflow, &row.started_with).await;

        // The run starts as the workflow's defaults with the start's properties over them.
        let mut started = row.defaults.as_object().cloned().unwrap_or_else(Map::new);
        if let Some(sent) = row.started_with.as_object() {
            for (property, value) in sent {
                started.insert(property.clone(), value.clone());
            }
        }
        let before = get_properties(&router, &workflow, &run_id).await;
        assert_eq!(
            before,
            Value::Object(started),
            "case {case}: a fresh run holds the defaults with the start's properties over \
             them, got {before}"
        );

        for put in &row.puts {
            put_properties(&router, &workflow, &run_id, put).await;
        }

        let after = get_properties(&router, &workflow, &run_id).await;
        assert_eq!(
            after, row.expected,
            "case {case}: the run holds what the puts wrote over what it held, got {after}"
        );

        // The run itself reports the same map: both readers read one record.
        let on_the_run = run_properties(&router, &workflow, &run_id).await;
        assert_eq!(
            on_the_run, row.expected,
            "case {case}: the run reports the properties the put wrote, got {on_the_run}"
        );

        // A read does not change what the next caller sees.
        let again = get_properties(&router, &workflow, &run_id).await;
        assert_eq!(
            again, row.expected,
            "case {case}: the read must be repeatable, got {again}"
        );
    }
}

/// The properties belong to the run: a put on one run of a workflow leaves that workflow's
/// other runs holding what they held.
#[tokio::test]
async fn a_put_writes_to_one_run_only() {
    let router = emulator();
    register_job(&router).await;

    let workflow = "run-properties-two-runs";
    register_workflow(&router, workflow, &json!({ "stage": "extract" })).await;
    let first = start_run(&router, workflow, &json!({ "run": "first" })).await;
    let second = start_run(&router, workflow, &json!({ "run": "second" })).await;

    let case = "a put on the first of two runs";
    put_properties(
        &router,
        workflow,
        &first,
        &json!({ "stage": "load", "checkpoint": "2026-10-25" }),
    )
    .await;

    let written = get_properties(&router, workflow, &first).await;
    assert_eq!(
        written,
        json!({ "run": "first", "stage": "load", "checkpoint": "2026-10-25" }),
        "case {case}: the run the put named holds what it wrote, got {written}"
    );
    let untouched = get_properties(&router, workflow, &second).await;
    assert_eq!(
        untouched,
        json!({ "run": "second", "stage": "extract" }),
        "case {case}: the other run is left as it was, got {untouched}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A workflow or a run this account holds no record of is refused, for the read and the
/// write alike: 400 with the modelled `EntityNotFoundException` naming what resolved to
/// nothing. The rows differ along which of the two names misses and how — never created, a
/// differently-cased name, and a run id of another workflow.
#[tokio::test]
async fn addressing_a_run_the_store_does_not_hold_is_refused() {
    let router = emulator();
    register_job(&router).await;

    let workflow = "run-properties-refusals";
    register_workflow(&router, workflow, &Value::Null).await;
    let run_id = start_run(&router, workflow, &json!({ "stage": "extract" })).await;

    let other = "run-properties-other-workflow";
    register_workflow(&router, other, &Value::Null).await;

    let missing_workflows = [
        ("a workflow never created", "no-such-workflow"),
        ("a stored name uppercased", "RUN-PROPERTIES-REFUSALS"),
    ];
    for (case, name) in missing_workflows {
        for operation in ["GetWorkflowRunProperties", "PutWorkflowRunProperties"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "Name": name, "RunId": run_id, "RunProperties": { "stage": "load" } }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} / {operation}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with(NOT_FOUND),
                "case {case} / {operation}: expected {NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                WORKFLOW_NOT_FOUND,
                "case {case} / {operation}: the recorded refusal for a workflow that \
                 resolved to nothing, got {body}"
            );
        }
    }

    let missing_runs = [
        ("a run id never minted", workflow, "wr_0000000000000000"),
        ("a run of another workflow", other, run_id.as_str()),
    ];
    for (case, named_workflow, missing) in missing_runs {
        for operation in ["GetWorkflowRunProperties", "PutWorkflowRunProperties"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({
                    "Name": named_workflow,
                    "RunId": missing,
                    "RunProperties": { "stage": "load" },
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} / {operation}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with(NOT_FOUND),
                "case {case} / {operation}: expected {NOT_FOUND}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                workflow_run_not_found(named_workflow, missing),
                "case {case} / {operation}: the refusal names the run that resolved to nothing, \
                 got {body}"
            );
        }
    }

    // Nothing a refused call named was written: the run still holds what it started with.
    let held = get_properties(&router, workflow, &run_id).await;
    assert_eq!(
        held,
        json!({ "stage": "extract" }),
        "case a refused put: the stored run is left untouched, got {held}"
    );
}

/// One run holds a bounded number of properties: the put that fills it to the ceiling
/// lands, a put that would take it past the ceiling is refused with the modelled
/// `ResourceNumberLimitExceededException` and writes nothing, and a put of keys the run
/// already holds still lands at the ceiling — the bound is on what the run would hold, not
/// on how many keys a put carries.
#[tokio::test]
async fn a_put_past_the_property_ceiling_is_refused() {
    let router = emulator();
    register_job(&router).await;

    let workflow = "run-properties-ceiling";
    register_workflow(&router, workflow, &Value::Null).await;
    let run_id = start_run(&router, workflow, &Value::Null).await;

    let mut at_the_ceiling = Map::new();
    for index in 0..MAX_RUN_PROPERTIES_PER_RUN {
        at_the_ceiling.insert(format!("key-{index}"), json!(index.to_string()));
    }
    let filled = Value::Object(at_the_ceiling);
    put_properties(&router, workflow, &run_id, &filled).await;

    let case = "one property too many";
    let (status, body) = call(
        &router,
        "PutWorkflowRunProperties",
        &json!({
            "Name": workflow,
            "RunId": run_id,
            "RunProperties": { "one-key-too-many": "1" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 past {MAX_RUN_PROPERTIES_PER_RUN} properties, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).ends_with(LIMIT_EXCEEDED),
        "case {case}: expected {LIMIT_EXCEEDED}, got {body}"
    );
    let held = get_properties(&router, workflow, &run_id).await;
    assert_eq!(
        held, filled,
        "case {case}: a refused put leaves the run as it was, got {held}"
    );

    // A put of keys the run already holds overrides their values without growing the run,
    // so it lands even though the run is at the ceiling.
    let case = "a put at the ceiling that only overrides";
    put_properties(&router, workflow, &run_id, &json!({ "key-0": "rewritten" })).await;
    let held = get_properties(&router, workflow, &run_id).await;
    assert_eq!(
        held.get("key-0").and_then(Value::as_str),
        Some("rewritten"),
        "case {case}: the override lands, got {held}"
    );
    assert_eq!(
        held.as_object().map(Map::len),
        Some(MAX_RUN_PROPERTIES_PER_RUN),
        "case {case}: the run still holds exactly the properties it did, got {held}"
    );
}

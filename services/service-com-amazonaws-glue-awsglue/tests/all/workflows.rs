// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue workflow slice: `CreateWorkflow` and the `GetWorkflow`
//! that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip suite is a table: each row is one input along the axis the rule
//! generalises over (whether a description is carried and how long it is, how many default
//! run properties the workflow holds, whether a concurrency ceiling was named and how
//! large it is, the form and length of the name, whether tags were sent), and a bare
//! name-only workflow — the shape an orchestration client sends before attaching anything
//! to it — is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateWorkflowInput;
use service_com_amazonaws_glue_awsglue::state::messages::WORKFLOW_NOT_FOUND;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, WorkflowEntry, MAX_WORKFLOWS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..255" on `NameString`
/// (`smithy_query operation=CreateWorkflow`).
const MAX_NAME_LENGTH: usize = 255;

/// The longest `Description` the model accepts: "len 0..120000" on
/// `WorkflowDescriptionString` (`smithy_query operation=CreateWorkflow`).
const MAX_DESCRIPTION_LENGTH: usize = 120_000;

/// The largest `Tags` map the model accepts: "len 0..50" on `TagsMap`
/// (`smithy_query operation=CreateWorkflow`).
const MAX_TAGS: usize = 50;

/// Create a workflow and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateWorkflow", body).await
}

/// The `Workflow` structure `GetWorkflow` reports for `name`.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    read(router, &json!({ "Name": name })).await
}

/// The same, for a request the caller composed (so a row can ask for the graph).
async fn read(router: &MultiServiceRouter, request: &Value) -> Value {
    let body = ok(router, "GetWorkflow", request).await;
    body.get("Workflow")
        .cloned()
        .unwrap_or_else(|| panic!("GetWorkflow must report a Workflow, got {body}"))
}

/// The rows every suite below drives: one workflow definition per axis the create's
/// members span.
fn definitions() -> Vec<(&'static str, Value)> {
    let long_name = "w".repeat(MAX_NAME_LENGTH);
    let long_description = "d".repeat(MAX_DESCRIPTION_LENGTH);
    let many_properties: serde_json::Map<String, Value> = (0..32)
        .map(|index| (format!("property-{index}"), json!(format!("value-{index}"))))
        .collect();
    let many_tags: serde_json::Map<String, Value> = (0..MAX_TAGS)
        .map(|index| (format!("tag-{index}"), json!(format!("tag-value-{index}"))))
        .collect();

    vec![
        (
            // The shape an orchestration client sends: the one required member alone.
            "required member only",
            json!({ "Name": "event-driven-workflow" }),
        ),
        (
            "a description",
            json!({
                "Name": "described-workflow",
                "Description": "nightly ingest and enrichment",
            }),
        ),
        (
            "a description at the modelled length bound",
            json!({ "Name": "long-description", "Description": long_description }),
        ),
        (
            "one default run property",
            json!({
                "Name": "one-property",
                "DefaultRunProperties": { "env": "staging" },
            }),
        ),
        (
            "many default run properties",
            json!({
                "Name": "many-properties",
                "DefaultRunProperties": Value::Object(many_properties),
            }),
        ),
        (
            "a concurrency ceiling of one",
            json!({ "Name": "serial-workflow", "MaxConcurrentRuns": 1 }),
        ),
        (
            "a larger concurrency ceiling, with properties and a description",
            json!({
                "Name": "parallel-workflow",
                "Description": "fan-out over partitions",
                "DefaultRunProperties": { "shards": "16", "mode": "backfill" },
                "MaxConcurrentRuns": 25,
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": long_name }),
        ),
        (
            "a name carrying spaces and a tab",
            json!({ "Name": "workflow one\ttwo", "Description": "named with whitespace" }),
        ),
        (
            "tags at the modelled maximum",
            json!({
                "Name": "tagged-workflow",
                "Description": "carries tags",
                "Tags": Value::Object(many_tags),
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateWorkflow` persists is what `GetWorkflow` reports: the create answers with
/// the name it stored, and every member of the workflow the request carried comes back
/// from the store unchanged, beside the two instants the service sets.
#[tokio::test]
async fn created_workflows_are_read_back_from_the_store() {
    let router = emulator();
    let cases = definitions();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // "The name of the workflow which was provided as part of the request": the
        // create reports the name it stored, which is what a later read addresses it by.
        let created = create(&router, request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateWorkflow must report the stored name, got {created}"
        );

        let workflow = get(&router, name).await;
        // Every member of the definition the request carried reappears unchanged; a
        // `null` expectation means the member carries no value.
        let expected = json!({
            "Name": name,
            "Description": request.get("Description").cloned().unwrap_or(Value::Null),
            "DefaultRunProperties": request
                .get("DefaultRunProperties")
                .cloned()
                .unwrap_or(Value::Null),
            "MaxConcurrentRuns": request
                .get("MaxConcurrentRuns")
                .cloned()
                .unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &workflow);

        // `Tags` is not a member of the `Workflow` shape, and neither is anything only a
        // run or a blueprint produces: a workflow a create wrote has had no execution and
        // was created from no blueprint.
        for absent in ["Tags", "LastRun", "BlueprintDetails"] {
            assert!(
                workflow.get(absent).is_none_or(Value::is_null),
                "case {case}: a created workflow reports no {absent}, got {workflow}"
            );
        }

        // Both server-set instants are reported, and a workflow nothing has updated was
        // last modified when it was created.
        let created_on = workflow
            .get("CreatedOn")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case}: CreatedOn must be a timestamp, got {workflow}")
            });
        assert!(
            created_on > 0.0,
            "case {case}: expected a wall-clock CreatedOn, got {created_on}"
        );
        assert_eq!(
            workflow.get("LastModifiedOn").and_then(Value::as_f64),
            Some(created_on),
            "case {case}: a workflow nothing has updated was last modified at its create, \
             got {workflow}"
        );

        // A read does not change what the next caller sees.
        let again = get(&router, name).await;
        assert_eq!(
            again, workflow,
            "case {case}: GetWorkflow must be repeatable, got {again}"
        );
    }

    // Each workflow is its own record: reading one back after every create has landed
    // still reports that workflow's own definition, so one create never overwrote another.
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let workflow = get(&router, name).await;
        assert_eq!(
            workflow.get("Description").cloned().unwrap_or(Value::Null),
            request.get("Description").cloned().unwrap_or(Value::Null),
            "case {case}: each workflow keeps its own Description, got {workflow}"
        );
    }
}

/// `IncludeGraph` decides whether the read carries a graph at all: a reader that asked
/// for none is answered without one, and a reader that asked for the graph of a workflow
/// nothing is attached to yet gets that graph — present, with no nodes and no edges.
#[tokio::test]
async fn the_graph_is_reported_only_when_the_read_asks_for_it() {
    let router = emulator();
    for (case, definition) in definitions() {
        create(&router, &definition).await;
        let name = definition["Name"]
            .as_str()
            .expect("the case carries a Name")
            .to_string();

        for (variant, include_graph) in [
            ("no IncludeGraph member", Value::Null),
            ("IncludeGraph false", json!(false)),
        ] {
            let mut request = json!({ "Name": name });
            if !include_graph.is_null() {
                request["IncludeGraph"] = include_graph;
            }
            let workflow = read(&router, &request).await;
            assert!(
                workflow.get("Graph").is_none_or(Value::is_null),
                "case {case} / {variant}: expected no Graph, got {workflow}"
            );
        }

        let workflow = read(&router, &json!({ "Name": name, "IncludeGraph": true })).await;
        let graph = workflow
            .get("Graph")
            .unwrap_or_else(|| panic!("case {case}: IncludeGraph must report a Graph, got {workflow}"));
        for member in ["Nodes", "Edges"] {
            let list = graph
                .get(member)
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case}: Graph.{member} must be a list, got {graph}"));
            assert!(
                list.is_empty(),
                "case {case}: a workflow with no components attached has an empty \
                 Graph.{member}, got {graph}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A workflow name is unique per account: the second create under a name already taken is
/// refused with the modelled `AlreadyExistsException`, and the stored workflow is left as
/// the first create wrote it.
#[tokio::test]
async fn a_duplicate_workflow_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, a description added",
            json!({ "Name": "duplicate-one" }),
            json!({ "Name": "duplicate-one", "Description": "second" }),
        ),
        (
            "same name, different run properties",
            json!({
                "Name": "duplicate-two",
                "DefaultRunProperties": { "env": "first" },
            }),
            json!({
                "Name": "duplicate-two",
                "DefaultRunProperties": { "env": "second" },
            }),
        ),
        (
            "same name, a different concurrency ceiling",
            json!({ "Name": "duplicate-three", "MaxConcurrentRuns": 2 }),
            json!({ "Name": "duplicate-three", "MaxConcurrentRuns": 9 }),
        ),
        (
            "same name and the identical definition",
            json!({ "Name": "duplicate-four", "Description": "same", "MaxConcurrentRuns": 3 }),
            json!({ "Name": "duplicate-four", "Description": "same", "MaxConcurrentRuns": 3 }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;
        let before = get(
            &router,
            first["Name"].as_str().expect("the case carries a Name"),
        )
        .await;

        let (status, body) = call(&router, "CreateWorkflow", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        // The refusal states the resource kind and withholds the requested name, as this
        // service's other already-exists refusals do.
        assert!(
            !error_message(&body).contains(second["Name"].as_str().unwrap_or_default()),
            "case {case}: the refusal must not disclose the requested name, got {body}"
        );

        // The stored workflow is still the FIRST create's, member for member.
        let after = get(
            &router,
            first["Name"].as_str().expect("the case carries a Name"),
        )
        .await;
        assert_eq!(
            after, before,
            "case {case}: a refused create leaves the stored workflow untouched, got {after}"
        );
    }
}

/// A read for a name this account holds no workflow under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the requested name misses: never
/// created, a prefix of a stored name, a suffix of one, and a stored name in another case
/// — the model documents no case fold for a workflow name, so a differently-cased name is
/// a different workflow.
#[tokio::test]
async fn reading_an_unknown_workflow_is_refused() {
    let router = emulator();
    create(&router, &json!({ "Name": "StoredWorkflow" })).await;

    for (case, name) in [
        ("never created", "no-such-workflow"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Workflow"),
        ("a stored name lowercased", "storedworkflow"),
        ("a stored name uppercased", "STOREDWORKFLOW"),
    ] {
        let (status, body) = call(&router, "GetWorkflow", &json!({ "Name": name })).await;
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
            WORKFLOW_NOT_FOUND,
            "case {case}: expected the recorded refusal for a workflow that resolved to \
             nothing, got {body}"
        );
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let workflow = get(&router, "StoredWorkflow").await;
    assert_eq!(
        workflow.get("Name").and_then(Value::as_str),
        Some("StoredWorkflow"),
        "case the stored name: expected the stored workflow, got {workflow}"
    );
}

/// The store holds a bounded number of workflows: the create at the ceiling succeeds, the
/// one past it is refused with the modelled limit error, and freeing a slot lets a create
/// succeed again — so the ceiling bounds what is stored rather than latching on how many
/// creates were ever made.
#[tokio::test]
async fn a_create_past_the_workflow_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_WORKFLOWS - 1 {
            let input = CreateWorkflowInput {
                name: format!("seeded-{index}"),
                ..CreateWorkflowInput::default()
            };
            store.insert_workflow(WorkflowEntry::from_create_input(
                &input,
                std::time::SystemTime::now(),
            ));
        }
    }

    let last = json!({ "Name": "last-workflow" });
    let past = json!({ "Name": "one-too-many" });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateWorkflow", &past).await;
    assert_eq!(
        status, 400,
        "case workflow {} of {MAX_WORKFLOWS}: expected 400, got {status} with {body}",
        MAX_WORKFLOWS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one workflow too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().workflows.remove("seeded-0");
    }
    create(&router, &past).await;
}

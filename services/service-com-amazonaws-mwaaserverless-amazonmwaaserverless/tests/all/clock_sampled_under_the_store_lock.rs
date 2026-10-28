// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN this service reads the wall clock while serving a request.
//!
//! The rule under test: the instant a handler materialises the clock from — and stamps
//! its own writes with — is sampled with this service's store already held
//! (`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::advance_clock`), so every
//! change the clock already owed when the handler reads the store has been applied
//! first, however long that handler took to get there. The two write paths take the
//! longest: both read the workflow definition out of Amazon S3 before they touch the
//! store, and any handler can also wait behind the request ahead of it.
//!
//! The consequence a client sees: a workflow whose `DELETING` span has elapsed by the
//! time a request reaches the store is gone for THAT request too — its name is free for
//! a create, and every reference to it resolves to nothing — and a record a write
//! stamps carries the instant that write took the store, never an earlier one that a
//! writer holding the store in between could already have passed.
//!
//! Every test below holds the store lock on a thread of its own for longer than the
//! `WORKFLOW_DELETED_AFTER` span a real `DeleteWorkflow` starts, so the request under
//! test really does begin before that teardown falls due and reach the store after it
//! has — the window is the handler's own, not the wall clock's, so nothing here is
//! raced. The table spans the class the rule generalises over: every operation that
//! touches the workflow store, the write paths and the reporting ones alike, and each of
//! the three ways this service answers a workflow that is gone.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    WORKFLOW_DELETED_AFTER, WORKFLOW_STATUS_READY,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;
use smithy_server_runtime::MultiServiceRouter;

use crate::missing_resource_lookups::LOOKUP_MISS_STATUS;
use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow, definition_router_on, stage, BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// The wire error code every unresolvable reference of this service is refused with —
/// the modelled shape (`missing_resource_lookups` is the suite that pins the refusal
/// itself).
const LOOKUP_MISS_TYPE: &str = "ResourceNotFoundException";

/// The key the definition every workflow below snapshots is staged at.
const DEFINITION_KEY: &str = "dags/clock.yaml";

/// A definition that declares tasks and NO schedule, so the only thing the clock owes
/// while a request waits below is the teardown the test started.
const DEFINITION: &str = "name: clock\ntasks:\n  - id: extract\n";

/// How much longer than the teardown span the store lock is held, so the removal is due
/// well before the lock is released however loaded the machine is.
const MARGIN: Duration = Duration::from_millis(250);

/// How long the store lock is held in the timestamp test: the span a write's stamped
/// instant would be wrong by if it had been sampled before the handler reached the
/// store.
const STAMP_HOLD: Duration = Duration::from_millis(400);

/// How far a response timestamp may sit below the instant it must be at or after, to
/// absorb the rounding of writing a `SystemTime` out as epoch seconds.
const TIMESTAMP_TOLERANCE: f64 = 0.001;

/// What a row expects once the teardown the clock already owed has been finished.
enum Expected {
    /// The operation serves the request: whatever the torn-down workflow held is gone,
    /// so the request is as good as one issued against a store that never held it.
    Served,
    /// The modelled `ResourceNotFoundException`, the way every single-resource
    /// operation of this service answers a reference that resolves to nothing.
    LookupMiss,
    /// HTTP 200 with the named list EMPTY: a listing whose model declares no
    /// `ResourceNotFoundException` answers an absent parent with an empty page.
    EmptyPage(&'static str),
}

/// One row: an operation called while the store lock is held past the removal deadline
/// of the workflow it references, and what it must answer.
struct Case {
    case: &'static str,
    operation: &'static str,
    body: fn(&TornDown) -> Value,
    expected: Expected,
}

/// A workflow whose teardown is running: created, given one run, then deleted.
struct TornDown {
    arn: String,
    name: String,
    run_id: String,
    /// An instant at or after the one the removal deadline is measured from, so a lock
    /// released later than `deleted_at + WORKFLOW_DELETED_AFTER` is certainly released
    /// after the removal fell due.
    deleted_at: SystemTime,
}

/// Every row: each operation of this service that touches the workflow store, against
/// the workflow whose teardown falls due while the request waits.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "CreateWorkflow takes the name the finished teardown freed",
            operation: "CreateWorkflow",
            body: |torn| {
                json!({
                    "Name": torn.name,
                    "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": DEFINITION_KEY},
                    "RoleArn": role_arn(),
                })
            },
            expected: Expected::Served,
        },
        Case {
            case: "UpdateWorkflow finds no workflow to append a version to",
            operation: "UpdateWorkflow",
            body: |torn| {
                json!({
                    "WorkflowArn": torn.arn,
                    "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": DEFINITION_KEY},
                    "RoleArn": role_arn(),
                })
            },
            expected: Expected::LookupMiss,
        },
        Case {
            case: "GetWorkflow resolves the removed workflow to nothing",
            operation: "GetWorkflow",
            body: |torn| json!({"WorkflowArn": torn.arn}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "DeleteWorkflow has nothing left to tear down",
            operation: "DeleteWorkflow",
            body: |torn| json!({"WorkflowArn": torn.arn}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "StartWorkflowRun finds no version to run",
            operation: "StartWorkflowRun",
            body: |torn| json!({"WorkflowArn": torn.arn}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "StopWorkflowRun finds no run of the removed workflow",
            operation: "StopWorkflowRun",
            body: |torn| json!({"WorkflowArn": torn.arn, "RunId": torn.run_id}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "GetWorkflowRun finds no run of the removed workflow",
            operation: "GetWorkflowRun",
            body: |torn| json!({"WorkflowArn": torn.arn, "RunId": torn.run_id}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "TagResource finds no workflow to tag",
            operation: "TagResource",
            body: |torn| json!({"ResourceArn": torn.arn, "Tags": {"team": "data"}}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "UntagResource finds no workflow to untag",
            operation: "UntagResource",
            body: |torn| json!({"ResourceArn": torn.arn, "TagKeys": ["team"]}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "ListTagsForResource finds no workflow to report tags for",
            operation: "ListTagsForResource",
            body: |torn| json!({"ResourceArn": torn.arn}),
            expected: Expected::LookupMiss,
        },
        Case {
            case: "ListWorkflows reports the removed workflow no more",
            operation: "ListWorkflows",
            body: |_| json!({"MaxResults": 100}),
            expected: Expected::EmptyPage("Workflows"),
        },
        Case {
            case: "ListWorkflowVersions walks no version of the removed workflow",
            operation: "ListWorkflowVersions",
            body: |torn| json!({"WorkflowArn": torn.arn}),
            expected: Expected::EmptyPage("WorkflowVersions"),
        },
        Case {
            case: "ListWorkflowRuns walks no run of the removed workflow",
            operation: "ListWorkflowRuns",
            body: |torn| json!({"WorkflowArn": torn.arn}),
            expected: Expected::EmptyPage("WorkflowRuns"),
        },
        Case {
            case: "ListTaskInstances walks no task instance of the removed workflow",
            operation: "ListTaskInstances",
            body: |torn| json!({"WorkflowArn": torn.arn, "RunId": torn.run_id}),
            expected: Expected::EmptyPage("TaskInstances"),
        },
    ]
}

/// Hold this service's store lock for `hold` on a thread of its own, and answer the
/// handle that reports the instant the guard was dropped.
///
/// This is the window a handler's own work sits in before it reaches the store, made
/// explicit: a request issued once this returns really does block on the store until the
/// span has elapsed, whatever it did first. Returns only once the lock is actually held,
/// so the request under test cannot slip in ahead of it.
fn hold_store_lock(
    engine: &Arc<engine::InMemoryEngine>,
    hold: Duration,
) -> thread::JoinHandle<SystemTime> {
    let state = engine.state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let (acquired, held) = mpsc::channel();
    let holder = thread::spawn(move || {
        let guard = state.lock();
        acquired
            .send(())
            .expect("the test waits for the store lock to be held");
        thread::sleep(hold);
        let released_at = SystemTime::now();
        drop(guard);
        released_at
    });
    held.recv().expect("the holder thread takes the store lock");
    holder
}

/// The instant the holder thread released the store lock.
fn released_at(holder: thread::JoinHandle<SystemTime>) -> SystemTime {
    holder.join().expect("the holder thread releases the lock")
}

/// A router with this service and Amazon S3 on one engine, with the definition staged,
/// and the engine the test holds the store lock through.
async fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = definition_router_on(Arc::clone(&engine));
    create_bucket(&router).await;
    stage(&router, DEFINITION_KEY, DEFINITION.as_bytes()).await;
    (engine, router)
}

/// Create a workflow named `name`, give it one run, and delete it — so its removal falls
/// due `WORKFLOW_DELETED_AFTER` later and its name, versions and runs are taken until
/// then.
async fn tear_down_a_workflow(router: &MultiServiceRouter, name: &'static str) -> TornDown {
    let arn = create_workflow(router, name, DEFINITION_KEY).await;
    let (status, started) = call(router, "StartWorkflowRun", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "setup {name:?}: StartWorkflowRun must succeed, got {status} {started:?}"
    );
    let run_id = started["RunId"]
        .as_str()
        .expect("setup: a started run has a RunId")
        .to_string();
    let (status, deleted) = call(router, "DeleteWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "setup {name:?}: DeleteWorkflow must succeed, got {status} {deleted:?}"
    );
    TornDown {
        arn,
        name: name.to_string(),
        run_id,
        deleted_at: SystemTime::now(),
    }
}

/// A response timestamp as fractional epoch seconds — the form `awsJson1_0` writes a
/// timestamp in.
fn epoch_seconds(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// A `SystemTime` as fractional epoch seconds, so it compares with a response timestamp.
fn since_epoch(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn a_create_takes_the_name_a_teardown_that_fell_due_while_it_waited_had_freed() {
    let (engine, router) = emulator().await;
    let torn = tear_down_a_workflow(&router, "recycled.workflow").await;

    // The window: the create starts while the teardown is still running and reaches the
    // store after its removal has fallen due.
    let holder = hold_store_lock(&engine, WORKFLOW_DELETED_AFTER + MARGIN);
    let (status, created) = call(
        &router,
        "CreateWorkflow",
        json!({
            "Name": torn.name,
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": DEFINITION_KEY},
            "RoleArn": role_arn(),
        }),
    )
    .await;
    let released = released_at(holder);

    let waited = released
        .duration_since(torn.deleted_at)
        .unwrap_or_default();
    assert!(
        waited >= WORKFLOW_DELETED_AFTER,
        "case recycled name: the store must stay locked past the removal deadline \
         ({WORKFLOW_DELETED_AFTER:?} after the delete), but it was released {waited:?} \
         after it"
    );
    assert_eq!(
        status, 200,
        "case recycled name: the teardown was due before the create read the store, so \
         the name is free, got {status} {created:?}"
    );

    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();
    assert_ne!(
        arn, torn.arn,
        "case recycled name: the create must mint a new workflow, not answer the \
         removed one's ARN"
    );

    // Read the create back: what the response reported is what the store holds.
    let (status, fetched) = call(&router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case recycled name: GetWorkflow must resolve the new workflow, got {status} \
         {fetched:?}"
    );
    assert_eq!(
        fetched["Name"].as_str(),
        Some(torn.name.as_str()),
        "case recycled name: the new workflow must hold the recycled name, got {:?}",
        fetched["Name"]
    );
    assert_eq!(
        fetched["WorkflowStatus"].as_str(),
        Some(WORKFLOW_STATUS_READY),
        "case recycled name: a created workflow is returned ready, got {:?}",
        fetched["WorkflowStatus"]
    );
    assert_eq!(
        fetched["CreatedAt"], created["CreatedAt"],
        "case recycled name: GetWorkflow must report the CreatedAt the create reported, \
         got {:?}",
        fetched["CreatedAt"]
    );

    // And the removed workflow is gone for every reader, not just for the create.
    let (status, listed) = call(&router, "ListWorkflows", json!({"MaxResults": 100})).await;
    assert_eq!(
        status, 200,
        "case recycled name: ListWorkflows must succeed, got {status} {listed:?}"
    );
    let arns: Vec<&str> = listed["Workflows"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["WorkflowArn"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![arn.as_str()],
        "case recycled name: only the new workflow is left, got {listed:?}"
    );
}

#[tokio::test]
async fn every_operation_reads_a_store_whose_owed_teardown_was_finished_first() {
    for case in cases() {
        let Case {
            case,
            operation,
            body,
            expected,
        } = case;
        // A fresh emulator per row, so one row's workflow cannot answer another's call.
        let (engine, router) = emulator().await;
        let torn = tear_down_a_workflow(&router, "teardown.workflow").await;

        let holder = hold_store_lock(&engine, WORKFLOW_DELETED_AFTER + MARGIN);
        let (status, answered) = call(&router, operation, body(&torn)).await;
        let released = released_at(holder);

        let waited = released
            .duration_since(torn.deleted_at)
            .unwrap_or_default();
        assert!(
            waited >= WORKFLOW_DELETED_AFTER,
            "case {case:?}: the store must stay locked past the removal deadline \
             ({WORKFLOW_DELETED_AFTER:?} after the delete), but it was released \
             {waited:?} after it"
        );

        match expected {
            Expected::Served => assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be served once the teardown was \
                 finished, got {status} {answered:?}"
            ),
            Expected::LookupMiss => {
                assert_eq!(
                    status, LOOKUP_MISS_STATUS,
                    "case {case:?}: expected HTTP {LOOKUP_MISS_STATUS} for the removed \
                     workflow, got {status} {answered:?}"
                );
                assert!(
                    answered["__type"]
                        .as_str()
                        .is_some_and(|kind| kind.contains(LOOKUP_MISS_TYPE)),
                    "case {case:?}: expected a {LOOKUP_MISS_TYPE}, got {:?}",
                    answered["__type"]
                );
            }
            Expected::EmptyPage(member) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a listing answers an absent parent with an empty \
                     page, got {status} {answered:?}"
                );
                assert_eq!(
                    answered[member].as_array().map_or(0, Vec::len),
                    0,
                    "case {case:?}: expected {member} to be empty once the teardown was \
                     finished, got {answered:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_write_stamps_the_instant_it_reached_the_store_not_an_earlier_one() {
    let (engine, router) = emulator().await;

    // CreateWorkflow: the first of the two write paths that read Amazon S3 before they
    // touch the store.
    let holder = hold_store_lock(&engine, STAMP_HOLD);
    let (status, created) = call(
        &router,
        "CreateWorkflow",
        json!({
            "Name": "stamped.workflow",
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": DEFINITION_KEY},
            "RoleArn": role_arn(),
        }),
    )
    .await;
    let released = since_epoch(released_at(holder));
    assert_eq!(
        status, 200,
        "case CreateWorkflow: the create must succeed, got {status} {created:?}"
    );
    let created_at = epoch_seconds(&created["CreatedAt"]).unwrap_or_else(|| {
        panic!(
            "case CreateWorkflow: expected a CreatedAt timestamp, got {:?}",
            created["CreatedAt"]
        )
    });
    assert!(
        created_at + TIMESTAMP_TOLERANCE >= released,
        "case CreateWorkflow: CreatedAt must be the instant the create took the store \
         (at or after {released}), got {created_at}"
    );

    let arn = created["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();

    // UpdateWorkflow: the other one, over the record the create just wrote.
    let holder = hold_store_lock(&engine, STAMP_HOLD);
    let (status, updated) = call(
        &router,
        "UpdateWorkflow",
        json!({
            "WorkflowArn": arn,
            "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": DEFINITION_KEY},
            "RoleArn": role_arn(),
            "Description": "the update that waited for the store",
        }),
    )
    .await;
    let released = since_epoch(released_at(holder));
    assert_eq!(
        status, 200,
        "case UpdateWorkflow: the update must succeed, got {status} {updated:?}"
    );
    let modified_at = epoch_seconds(&updated["ModifiedAt"]).unwrap_or_else(|| {
        panic!(
            "case UpdateWorkflow: expected a ModifiedAt timestamp, got {:?}",
            updated["ModifiedAt"]
        )
    });
    assert!(
        modified_at + TIMESTAMP_TOLERANCE >= released,
        "case UpdateWorkflow: ModifiedAt must be the instant the update took the store \
         (at or after {released}), got {modified_at}"
    );

    // Both instants are the stored ones, and the update left the create's alone.
    let (status, fetched) = call(&router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case readback: GetWorkflow must resolve the workflow, got {status} {fetched:?}"
    );
    assert_eq!(
        fetched["CreatedAt"], created["CreatedAt"],
        "case readback: an update must not move CreatedAt, got {:?}",
        fetched["CreatedAt"]
    );
    assert_eq!(
        fetched["ModifiedAt"], updated["ModifiedAt"],
        "case readback: GetWorkflow must report the ModifiedAt the update reported, got \
         {:?}",
        fetched["ModifiedAt"]
    );
}

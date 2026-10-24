// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the teardown `DeleteWorkflow` starts.
//!
//! The rule under test: a delete that takes EVERY version of a workflow deletes the
//! workflow, and that is an asynchronous cleanup over its associated resources —
//! "Amazon Managed Workflows for Apache Airflow Serverless ensures that all associated
//! resources are properly cleaned up, including stopping any running executions,
//! removing scheduled triggers, and cleaning up execution history"
//! (`smithy_query(query="digest")`, `DeleteWorkflow`). While that cleanup runs the
//! workflow reports the modelled `WorkflowStatus` `DELETING` — the only other value the
//! enum carries (`smithy_query(query="shape", shape="WorkflowStatus")` declares
//! `READY | DELETING`) — so a client that deletes and then describes or lists sees
//! `DELETING`, and the record (with its versions, runs, name and idempotency tokens)
//! disappears only once the removal the clock owes falls due,
//! `WORKFLOW_DELETED_AFTER` after the delete. A delete that takes only ONE of several
//! versions is not the workflow's teardown: the workflow stays `READY`.
//!
//! A workflow in `DELETING` is on its way out, so it accepts no change: `UpdateWorkflow`
//! and `StartWorkflowRun` refuse it with the modelled `ConflictException` ("the resource
//! is in a state that prevents the requested operation",
//! `smithy_query(query="shape", shape="ConflictException")`), its name is still taken,
//! and repeating the delete starts nothing further.
//!
//! The tables below span the class the rule generalises over — how many versions the
//! workflow holds, whether the delete names a version, whether it holds runs, and where
//! the removal deadline sits relative to the read — and the deadline-driven suites seed
//! the stored clock field (the same way `run_lifecycle_progression` does) so every phase
//! is asserted exactly rather than raced against the wall clock. One suite then drives a
//! real delete on the real clock and checks the order a polling client observes.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    Workflow, WORKFLOW_DELETED_AFTER, WORKFLOW_STATUS_DELETING, WORKFLOW_STATUS_READY,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ComAmazonawsMwaaserverlessAmazonmwaaserverlessState;

use crate::workflow_lifecycle::{role_arn, Harness};

/// Create a workflow holding `versions` immutable versions: the create makes the first
/// one and each update appends another. Answers its ARN.
async fn workflow_with_versions(harness: &Harness, name: &str, versions: usize) -> String {
    assert!(versions >= 1, "setup: a workflow always holds one version");
    let (status, created) = harness
        .call(
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
                "RoleArn": role_arn(),
                "Tags": {"team": "data"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "setup {name:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
    );
    let arn = created["WorkflowArn"]
        .as_str()
        .expect("setup: a created workflow has an ARN")
        .to_string();
    for index in 1..versions {
        let (status, updated) = harness
            .call(
                "UpdateWorkflow",
                serde_json::json!({
                    "WorkflowArn": arn,
                    "DefinitionS3Location": {
                        "Bucket": "defs",
                        "ObjectKey": format!("dags/v{}.yaml", index + 1),
                    },
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {name:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
    }
    arn
}

/// Edit the stored workflow in place. `deletion_due_at` is the clock field the teardown
/// is driven from, so setting it is how a row puts the removal deadline in the past (or
/// safely out of reach) without sleeping for it.
fn with_workflow<R>(harness: &Harness, arn: &str, edit: impl FnOnce(&mut Workflow) -> R) -> R {
    let state = harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>();
    let mut store = state.lock();
    let workflow = store
        .workflows
        .get_mut(arn)
        .expect("setup: the workflow is stored");
    edit(workflow)
}

/// The instant the store says this workflow's removal falls due, and `None` both for a
/// workflow no delete has touched and for one already removed.
fn pending_removal(harness: &Harness, arn: &str) -> Option<SystemTime> {
    harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
        .lock()
        .workflows
        .get(arn)
        .and_then(|workflow| workflow.deletion_due_at)
}

/// Whether the store still holds a name -> ARN entry for `name`: the index
/// `CreateWorkflow` conflicts on.
fn name_is_held(harness: &Harness, name: &str) -> bool {
    harness
        .engine()
        .state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>()
        .lock()
        .workflow_names
        .contains_key(name)
}

/// The instant `offset_millis` after `reference`.
fn shifted(reference: SystemTime, offset_millis: i64) -> SystemTime {
    if offset_millis >= 0 {
        reference + Duration::from_millis(offset_millis.unsigned_abs())
    } else {
        reference - Duration::from_millis(offset_millis.unsigned_abs())
    }
}

/// The `WorkflowStatus` `ListWorkflows` reports for `arn`, and `None` when the listing
/// does not hold it at all.
async fn listed_status(harness: &Harness, arn: &str) -> Option<String> {
    let (status, listed) = harness
        .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
        .await;
    assert_eq!(status, 200, "ListWorkflows failed: {listed:?}");
    listed["Workflows"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["WorkflowArn"].as_str() == Some(arn))
                .cloned()
        })
        .and_then(|entry| entry["WorkflowStatus"].as_str().map(str::to_string))
}

/// One row of the delete table: how many versions the workflow holds and whether the
/// delete names the version it takes. Every row's delete takes EVERY version the
/// workflow has left, so every row is the workflow's teardown.
struct DeleteCase {
    case: &'static str,
    name: &'static str,
    /// How many versions the create (plus updates) leaves the workflow holding.
    versions: usize,
    /// Whether a version is deleted by name first, so the delete under test takes the
    /// only version left.
    thin_to_one_version: bool,
    /// Whether the delete under test names the version it takes.
    name_the_version: bool,
    /// How many runs the workflow holds when it is deleted.
    runs: usize,
}

/// The rows. They differ along the version count, whether the delete names a version,
/// whether a version was already removed by name, and how many runs the teardown has to
/// clean up — not merely along the workflow's name.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "one version, version omitted, no run",
            name: "teardownOne",
            versions: 1,
            thin_to_one_version: false,
            name_the_version: false,
            runs: 0,
        },
        DeleteCase {
            case: "one version, that version named, one run",
            name: "teardownNamed",
            versions: 1,
            thin_to_one_version: false,
            name_the_version: true,
            runs: 1,
        },
        DeleteCase {
            case: "three versions, version omitted, two runs",
            name: "teardownThree",
            versions: 3,
            thin_to_one_version: false,
            name_the_version: false,
            runs: 2,
        },
        DeleteCase {
            case: "two versions, one removed by name, the last one named",
            name: "teardownThinned",
            versions: 2,
            thin_to_one_version: true,
            name_the_version: true,
            runs: 1,
        },
        DeleteCase {
            case: "two versions, one removed by name, version omitted",
            name: "teardownThinnedOmitted",
            versions: 2,
            thin_to_one_version: true,
            name_the_version: false,
            runs: 0,
        },
    ]
}

/// Drive a row up to (and including) the delete that starts the teardown, and answer
/// `(WorkflowArn, the version the delete answered, a RunId the workflow holds)`.
async fn start_teardown(harness: &Harness, row: &DeleteCase) -> (String, String, Option<String>) {
    let arn = workflow_with_versions(harness, row.name, row.versions).await;

    let mut run_id = None;
    for index in 0..row.runs {
        let (status, started) = harness
            .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected run {index} to start, got {status} {started:?}",
            row.case
        );
        run_id = started["RunId"].as_str().map(str::to_string);
    }

    if row.thin_to_one_version {
        // Remove one version by name: that is a change to the workflow, not its
        // teardown, so the delete under test is still the one that takes them all.
        let versions = harness.stored_versions(&arn);
        let doomed = versions
            .iter()
            .find(|(_, is_latest)| !is_latest)
            .map(|(version, _)| version.clone())
            .expect("setup: a workflow with two versions has a non-latest one");
        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": doomed}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the version-scoped delete to succeed, got {status} \
             {answered:?}",
            row.case
        );
        assert_eq!(
            pending_removal(harness, &arn),
            None,
            "case {:?}: one version of several is not the workflow's teardown, so no \
             removal may be pending",
            row.case
        );
    }

    let latest = harness
        .stored_versions(&arn)
        .into_iter()
        .find(|(_, is_latest)| *is_latest)
        .map(|(version, _)| version)
        .expect("setup: a stored workflow has a latest version");
    let mut request = serde_json::json!({"WorkflowArn": arn});
    if row.name_the_version {
        request["WorkflowVersion"] = serde_json::Value::String(latest.clone());
    }
    let (status, deleted) = harness.call("DeleteWorkflow", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}",
        row.case
    );
    assert_eq!(
        deleted["WorkflowArn"].as_str(),
        Some(arn.as_str()),
        "case {:?}: DeleteWorkflow must echo the ARN it is removing, got {:?}",
        row.case,
        deleted["WorkflowArn"]
    );
    assert_eq!(
        deleted["WorkflowVersion"].as_str(),
        Some(latest.as_str()),
        "case {:?}: DeleteWorkflow must echo the version it is removing, got {:?}",
        row.case,
        deleted["WorkflowVersion"]
    );

    (arn, latest, run_id)
}

#[tokio::test]
async fn deleting_every_version_starts_a_teardown_that_reports_deleting() {
    for row in delete_cases() {
        let harness = Harness::new("us-west-2");
        let (arn, version, run_id) = start_teardown(&harness, &row).await;

        // The removal is OWED, at an instant the delete recorded rather than slept for.
        let due = pending_removal(&harness, &arn).unwrap_or_else(|| {
            panic!(
                "case {:?}: the delete must leave the removal pending at a recorded \
                 instant",
                row.case
            )
        });
        assert!(
            due > SystemTime::now(),
            "case {:?}: the teardown span must be non-zero, so the removal falls due \
             later than the delete; got a deadline {due:?} already past",
            row.case
        );

        // Put the deadline out of reach, so what follows asserts the DELETING phase
        // itself rather than racing the wall clock.
        with_workflow(&harness, &arn, |workflow| {
            workflow.deletion_due_at = Some(SystemTime::now() + Duration::from_secs(3_600));
        });

        // A describe during the cleanup reports the workflow, in DELETING.
        let request = serde_json::json!({"WorkflowArn": arn});
        let (status, fetched) = harness.call("GetWorkflow", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetWorkflow during the teardown must still succeed, got {status} \
             {fetched:?}",
            row.case
        );
        assert_eq!(
            fetched["WorkflowStatus"].as_str(),
            Some(WORKFLOW_STATUS_DELETING),
            "case {:?}: expected WorkflowStatus {WORKFLOW_STATUS_DELETING}, got {:?}",
            row.case,
            fetched["WorkflowStatus"]
        );

        // A read changes nothing a later caller sees: the second describe answers
        // identically.
        let (status, again) = harness.call("GetWorkflow", request).await;
        assert_eq!(
            status, 200,
            "case {:?}: the repeated describe must succeed, got {status} {again:?}",
            row.case
        );
        assert_eq!(
            again, fetched,
            "case {:?}: a read must not change what a later caller sees, got {again:?} \
             after {fetched:?}",
            row.case
        );

        // The version it reports, the listing, its tags and its runs are all still
        // readable while the cleanup runs.
        let (status, versioned) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": version}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: a versioned describe during the teardown must succeed, got \
             {status} {versioned:?}",
            row.case
        );
        assert_eq!(
            versioned["WorkflowStatus"].as_str(),
            Some(WORKFLOW_STATUS_DELETING),
            "case {:?}: a versioned describe reports the workflow's status, got {:?}",
            row.case,
            versioned["WorkflowStatus"]
        );
        assert_eq!(
            listed_status(&harness, &arn).await.as_deref(),
            Some(WORKFLOW_STATUS_DELETING),
            "case {:?}: ListWorkflows must report the workflow as \
             {WORKFLOW_STATUS_DELETING} while its cleanup runs",
            row.case
        );
        let (status, tags) = harness
            .call(
                "ListTagsForResource",
                serde_json::json!({"ResourceArn": arn}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource during the teardown must succeed, got \
             {status} {tags:?}",
            row.case
        );
        if let Some(run_id) = run_id.as_deref() {
            let (status, run) = harness
                .call(
                    "GetWorkflowRun",
                    serde_json::json!({"WorkflowArn": arn, "RunId": run_id}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {:?}: a run of a workflow still being cleaned up is still \
                 readable, got {status} {run:?}",
                row.case
            );
        }

        // Now let the clock finish the teardown: the removal was owed at `due`, so the
        // record, its versions, its runs and its name are gone.
        with_workflow(&harness, &arn, |workflow| {
            workflow.deletion_due_at = Some(shifted(SystemTime::now(), -1));
        });

        let (status, missing) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: once the teardown is due the workflow is gone, so GetWorkflow \
             must be a lookup miss, got {status} {missing:?}",
            row.case
        );
        assert_eq!(
            listed_status(&harness, &arn).await,
            None,
            "case {:?}: a removed workflow must not be listed",
            row.case
        );
        assert_eq!(
            harness.stored_run_count(&arn),
            0,
            "case {:?}: the removed workflow's runs must be gone with it",
            row.case
        );
        assert!(
            !name_is_held(&harness, row.name),
            "case {:?}: the removed workflow must release the name it held",
            row.case
        );
        let (status, refused) = harness
            .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: deleting an already removed workflow must be a lookup miss, got \
             {status} {refused:?}",
            row.case
        );
        // The name is free again, so the same name can be created once more.
        let (status, recreated) = harness
            .call(
                "CreateWorkflow",
                serde_json::json!({
                    "Name": row.name,
                    "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/again.yaml"},
                    "RoleArn": role_arn(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: the name a removed workflow held must be reusable, got {status} \
             {recreated:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_workflow_being_deleted_accepts_no_change() {
    let name = "noChangeWhileDeleting";
    let harness = Harness::new("eu-west-1");
    let arn = workflow_with_versions(&harness, name, 2).await;
    let (status, started) = harness
        .call("StartWorkflowRun", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected StartWorkflowRun to succeed, got {status} {started:?}"
    );
    let runs_before = harness.stored_run_count(&arn);
    let versions_before = harness.stored_versions(&arn);

    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );
    // Hold the deadline out of reach, so each refusal below is asserted against the
    // DELETING phase and not against an already removed workflow.
    with_workflow(&harness, &arn, |workflow| {
        workflow.deletion_due_at = Some(SystemTime::now() + Duration::from_secs(3_600));
    });
    let due = pending_removal(&harness, &arn).expect("the removal is pending");

    // Each row is an operation that would CHANGE the workflow. `ConflictException` is
    // modelled by all three ("You cannot create a resource that already exists, or the
    // resource is in a state that prevents the requested operation"), and the model
    // gives it HTTP 409 (`smithy_query(query="shape", shape="ConflictException")`).
    let rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "update the workflow",
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/late.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
        (
            "start a run",
            "StartWorkflowRun",
            serde_json::json!({"WorkflowArn": arn}),
        ),
        (
            "create another workflow under the name it still holds",
            "CreateWorkflow",
            serde_json::json!({
                "Name": name,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/other.yaml"},
                "RoleArn": role_arn(),
            }),
        ),
    ];
    for (case, operation, body) in rows {
        let (status, refused) = harness.call(operation, body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a workflow being deleted is in a state that prevents the \
             operation, so {operation} must answer 409, got {status} {refused:?}"
        );
        assert!(
            refused["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ConflictException")),
            "case {case:?}: expected a ConflictException, got {:?}",
            refused["__type"]
        );
    }

    // Repeating the delete starts nothing further: it answers the identity of what is
    // being removed and leaves the pending removal and its deadline exactly where they
    // were.
    let (status, again) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case repeated delete: expected DeleteWorkflow to succeed, got {status} {again:?}"
    );
    assert_eq!(
        again, deleted,
        "case repeated delete: the second delete must answer exactly as the first, got \
         {again:?} after {deleted:?}"
    );
    assert_eq!(
        pending_removal(&harness, &arn),
        Some(due),
        "case repeated delete: the removal deadline must not move"
    );
    assert_eq!(
        harness.stored_run_count(&arn),
        runs_before,
        "case refusals: nothing may have added or dropped a run"
    );
    assert_eq!(
        harness.stored_versions(&arn),
        versions_before,
        "case refusals: nothing may have added or dropped a version"
    );
}

#[tokio::test]
async fn deleting_one_version_of_several_leaves_the_workflow_ready() {
    // Rows differ along how many versions the workflow holds and whether the deleted
    // one was the latest, because "if not specified, all versions of the workflow are
    // deleted" makes the teardown a property of the SET of versions, not of one delete.
    for (case, versions, delete_the_latest) in [
        ("two versions, the latest deleted", 2, true),
        ("two versions, the older deleted", 2, false),
        ("three versions, the latest deleted", 3, true),
        ("three versions, an older one deleted", 3, false),
    ] {
        let harness = Harness::new("ap-south-1");
        let name = "versionScopedDelete";
        let arn = workflow_with_versions(&harness, name, versions).await;
        let stored = harness.stored_versions(&arn);
        let doomed = stored
            .iter()
            .find(|(_, is_latest)| *is_latest == delete_the_latest)
            .map(|(version, _)| version.clone())
            .expect("setup: the workflow holds both a latest and an older version");

        let (status, answered) = harness
            .call(
                "DeleteWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": doomed}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version-scoped delete to succeed, got {status} \
             {answered:?}"
        );
        assert_eq!(
            pending_removal(&harness, &arn),
            None,
            "case {case:?}: one version of {versions} is not every version, so the \
             workflow owes no removal"
        );

        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the workflow must survive a version-scoped delete, got \
             {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowStatus"].as_str(),
            Some(WORKFLOW_STATUS_READY),
            "case {case:?}: a workflow that keeps a version stays \
             {WORKFLOW_STATUS_READY}, got {:?}",
            fetched["WorkflowStatus"]
        );
        assert_eq!(
            harness.stored_versions(&arn).len(),
            versions - 1,
            "case {case:?}: exactly the named version may be gone"
        );

        // Nothing is owed, so waiting out a whole teardown span changes nothing.
        tokio::time::sleep(WORKFLOW_DELETED_AFTER + Duration::from_millis(200)).await;
        let (status, still_there) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: no removal was owed, so the workflow is still there after \
             {WORKFLOW_DELETED_AFTER:?}, got {status} {still_there:?}"
        );
        assert!(
            name_is_held(&harness, name),
            "case {case:?}: the surviving workflow keeps its name"
        );
    }
}

#[tokio::test]
async fn a_client_that_deletes_and_polls_observes_deleting_then_absence() {
    let harness = Harness::new("us-east-1");
    let name = "polledTeardown";
    let arn = workflow_with_versions(&harness, name, 1).await;
    let (status, deleted) = harness
        .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
    );

    // Poll the way a client waiting for the delete to finish does. Nothing but the clock
    // drives the teardown, so polling neither hurries it nor holds it back.
    let mut observed: Vec<String> = Vec::new();
    let deadline = SystemTime::now() + Duration::from_secs(10);
    while SystemTime::now() < deadline {
        let (status, answered) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        let phase = match status {
            200 => answered["WorkflowStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            400 => "GONE".to_string(),
            other => panic!("case poll: unexpected GetWorkflow status {other}: {answered:?}"),
        };
        if observed.last() != Some(&phase) {
            observed.push(phase.clone());
        }
        if phase == "GONE" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(
        observed,
        vec![WORKFLOW_STATUS_DELETING.to_string(), "GONE".to_string()],
        "case poll: a client must see {WORKFLOW_STATUS_DELETING} before the workflow \
         disappears, and nothing else, within {WORKFLOW_DELETED_AFTER:?}; got {observed:?}"
    );
    assert!(
        !name_is_held(&harness, name),
        "case poll: the finished teardown releases the workflow's name"
    );
}

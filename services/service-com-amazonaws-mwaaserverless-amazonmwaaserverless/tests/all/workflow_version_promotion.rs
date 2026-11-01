// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which version a workflow reports once the set of versions it
//! holds has changed.
//!
//! The rule under test: a workflow has exactly ONE latest version at any instant, and it
//! is the newest version APPENDED to its history — the position a version listing walks
//! (`ListWorkflowVersions` reports the history in that order) — so whatever changes the
//! set of versions, that one version is the one `IsLatestVersion` flags, the one a read
//! with no version named resolves, and the one the workflow's current configuration is
//! taken from. The append position decides, not the creation instant: two versions can
//! carry the same `CreatedAt` and that must not make two of them latest, nor leave the
//! promoted version disagreeing with the configuration the workflow reports.
//!
//! Rows differ along what the rule generalises over: how many versions the history
//! holds, which one the delete removes (the latest, a middle one, the oldest) and how the
//! stored creation instants fall relative to the append order — all one instant, rising
//! with it, or falling against it. Those instants are a stored clock field, so the table
//! seeds them the way the teardown and run-lifecycle suites seed their deadlines rather
//! than racing the wall clock.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    mint_revision_id, Workflow,
};

use crate::workflow_definition_snapshot::{
    call, create_bucket, create_workflow_with_mode, definition_router_on, stage,
    with_stored_workflow, BUCKET,
};
use crate::workflow_deletion_teardown::{with_workflow, workflow_with_versions};
use crate::workflow_lifecycle::{role_arn, Harness};

/// How a row arranges the stored creation instants of a workflow's versions relative to
/// the order they were appended in.
#[derive(Debug, Clone, Copy)]
enum StoredClock {
    /// Every version carries ONE instant, which is what a coarse-grained clock produces
    /// for versions appended in quick succession.
    Tied,
    /// A later-appended version carries a later instant, the way the wall clock usually
    /// separates them.
    Rising,
    /// A later-appended version carries an EARLIER instant, so the newest appended
    /// version is the oldest by the clock.
    Falling,
}

/// Rewrite the stored creation instants of every version of `arn` to follow `clock`, the
/// coherent state the create and the updates left with the instants moved.
fn seed_version_clock(harness: &Harness, arn: &str, clock: StoredClock) {
    let base = SystemTime::now() - Duration::from_secs(3_600);
    with_workflow(harness, arn, |workflow: &mut Workflow| {
        let count = workflow.versions.len() as u64;
        for (index, record) in workflow.versions.iter_mut().enumerate() {
            let index = index as u64;
            record.created_at = match clock {
                StoredClock::Tied => base,
                StoredClock::Rising => base + Duration::from_secs(index),
                StoredClock::Falling => base + Duration::from_secs(count - index),
            };
        }
    });
}

/// The instant the store says this workflow's trigger fires next, and `None` for a
/// workflow the service does not execute on its own.
fn trigger(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Option<SystemTime> {
    with_stored_workflow(engine, arn, |workflow: &mut Workflow| {
        workflow.next_scheduled_run_at
    })
}

/// What a workflow reports as its current configuration in the store: the version a read
/// with no version named resolves, and the revision identifier derived from it.
fn reported_version(harness: &Harness, arn: &str) -> (String, String) {
    with_workflow(harness, arn, |workflow: &mut Workflow| {
        (
            workflow.current_version.clone(),
            workflow.revision_id.clone(),
        )
    })
}

/// The `WorkflowVersions` a listing reports for a workflow, over the wire.
async fn listed_versions(harness: &Harness, arn: &str, case: &str) -> Vec<serde_json::Value> {
    let (status, listed) = harness
        .call(
            "ListWorkflowVersions",
            serde_json::json!({"WorkflowArn": arn, "MaxResults": 100}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkflowVersions to succeed, got {status} {listed:?}"
    );
    listed["WorkflowVersions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the promotion table.
struct Row {
    case: &'static str,
    /// How many versions the history holds before the delete.
    versions: usize,
    /// Which version the delete names, as a position in the append order.
    deleted: usize,
    clock: StoredClock,
}

const ROWS: &[Row] = &[
    Row {
        case: "two versions, one instant, the latest deleted",
        versions: 2,
        deleted: 1,
        clock: StoredClock::Tied,
    },
    Row {
        case: "three versions, one instant, the latest deleted",
        versions: 3,
        deleted: 2,
        clock: StoredClock::Tied,
    },
    Row {
        case: "three versions, falling clock, the latest deleted",
        versions: 3,
        deleted: 2,
        clock: StoredClock::Falling,
    },
    Row {
        case: "four versions, rising clock, the latest deleted",
        versions: 4,
        deleted: 3,
        clock: StoredClock::Rising,
    },
    Row {
        case: "three versions, one instant, a middle one deleted",
        versions: 3,
        deleted: 1,
        clock: StoredClock::Tied,
    },
    Row {
        case: "four versions, falling clock, the oldest deleted",
        versions: 4,
        deleted: 0,
        clock: StoredClock::Falling,
    },
];

#[tokio::test]
async fn exactly_one_version_is_latest_after_a_version_scoped_delete() {
    for row in ROWS {
        let case = row.case;
        let harness = Harness::new("eu-central-1");
        let arn = workflow_with_versions(&harness, "versionPromotion", row.versions).await;
        seed_version_clock(&harness, &arn, row.clock);

        let stored = harness.stored_versions(&arn);
        assert_eq!(
            stored.len(),
            row.versions,
            "setup {case:?}: expected {} stored versions, got {stored:?}",
            row.versions
        );
        let doomed = stored[row.deleted].0.clone();
        // The newest APPENDED survivor: the last position of the history that the delete
        // does not remove. `workflow_with_versions` writes the definition of the version
        // at position `index` to `dags/v{index + 1}.yaml`, so the promoted version's own
        // configuration is identifiable on the wire.
        let promoted = (0..row.versions)
            .rfind(|index| *index != row.deleted)
            .expect("setup: a version-scoped delete leaves at least one version");
        let expected_latest = stored[promoted].0.clone();
        let expected_object_key = format!("dags/v{}.yaml", promoted + 1);

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
            answered["WorkflowVersion"].as_str(),
            Some(doomed.as_str()),
            "case {case:?}: the delete reports the version it removed, got {:?}",
            answered["WorkflowVersion"]
        );

        // The store holds exactly one latest version, and it is the newest appended
        // survivor — whatever the creation instants say.
        let survivors = harness.stored_versions(&arn);
        let flagged: Vec<String> = survivors
            .iter()
            .filter(|(_, is_latest)| *is_latest)
            .map(|(version, _)| version.clone())
            .collect();
        assert_eq!(
            flagged,
            vec![expected_latest.clone()],
            "case {case:?}: expected the newest appended survivor {expected_latest:?} to be \
             the only latest version, got {survivors:?}"
        );

        // The listing reports that same single flag, and the removed version is gone.
        let items = listed_versions(&harness, &arn, case).await;
        let listed_ids: Vec<&str> = items
            .iter()
            .map(|item| item["WorkflowVersion"].as_str().unwrap_or(""))
            .collect();
        assert!(
            !listed_ids.contains(&doomed.as_str()),
            "case {case:?}: the deleted version {doomed:?} must not be listed, got \
             {listed_ids:?}"
        );
        for item in &items {
            let version = item["WorkflowVersion"].as_str().unwrap_or("");
            let expected = version == expected_latest;
            assert_eq!(
                item["IsLatestVersion"].as_bool(),
                Some(expected),
                "case {case:?}: expected IsLatestVersion {expected} for {version:?}, got {:?}",
                item["IsLatestVersion"]
            );
        }

        // A read with no version named resolves the promoted version, and the
        // configuration it answers is the one THAT version froze.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowVersion"].as_str(),
            Some(expected_latest.as_str()),
            "case {case:?}: expected the default read to resolve {expected_latest:?}, got {:?}",
            fetched["WorkflowVersion"]
        );
        assert_eq!(
            fetched["DefinitionS3Location"]["ObjectKey"].as_str(),
            Some(expected_object_key.as_str()),
            "case {case:?}: expected the promoted version's own definition \
             {expected_object_key:?}, got {:?}",
            fetched["DefinitionS3Location"]
        );

        // The derived fields agree with the version the default read resolved: the
        // snapshot instant is that version's own, and the revision identifier is the one
        // that version mints.
        let (status, by_name) = harness
            .call(
                "GetWorkflow",
                serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": expected_latest}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version-named read to succeed, got {status} \
             {by_name:?}"
        );
        assert_eq!(
            fetched["CodeSnapshottedAt"], by_name["CodeSnapshottedAt"],
            "case {case:?}: expected the workflow's snapshot instant to be the promoted \
             version's own {:?}, got {:?}",
            by_name["CodeSnapshottedAt"], fetched["CodeSnapshottedAt"]
        );
        let (current_version, revision_id) = reported_version(&harness, &arn);
        assert_eq!(
            current_version, expected_latest,
            "case {case:?}: expected the stored current version to be {expected_latest:?}, \
             got {current_version:?}"
        );
        assert_eq!(
            revision_id,
            mint_revision_id(&arn, &expected_latest),
            "case {case:?}: expected the revision identifier of {expected_latest:?}, got \
             {revision_id:?}"
        );
    }
}

/// One row of the trigger table: whether the definition the EARLIER version snapshotted
/// declares a schedule, whether the one the LATEST version snapshotted does, and whether
/// the service still executes the workflow on its own once the latest version has been
/// deleted — which is the earlier version's answer, because the promoted version's
/// definition is the one whose schedule is in force.
struct TriggerRow {
    case: &'static str,
    earlier_declares: bool,
    latest_declares: bool,
}

const TRIGGER_ROWS: &[TriggerRow] = &[
    TriggerRow {
        case: "a version declaring a schedule deleted over one declaring none",
        earlier_declares: false,
        latest_declares: true,
    },
    TriggerRow {
        case: "a version declaring none deleted over one declaring a schedule",
        earlier_declares: true,
        latest_declares: false,
    },
    TriggerRow {
        case: "a version declaring a schedule deleted over another declaring one",
        earlier_declares: true,
        latest_declares: true,
    },
    TriggerRow {
        case: "neither version declaring a schedule",
        earlier_declares: false,
        latest_declares: false,
    },
];

/// A definition for a row: one declaring a DAG-level cron schedule, or one declaring
/// none.
fn definition(declares: bool) -> &'static str {
    if declares {
        "schedule: \"0 2 * * *\"\ntasks:\n  - id: extract\n"
    } else {
        "tasks:\n  - id: extract\n"
    }
}

#[tokio::test]
async fn the_promoted_version_puts_its_own_trigger_in_force() {
    for row in TRIGGER_ROWS {
        let case = row.case;
        // The trigger is configured from the definition each version snapshotted, so this
        // suite's workflows are created through the router that also serves the emulated
        // Amazon S3 the definitions live in.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        let key = "dags/promoted.yaml";
        stage(&router, key, definition(row.earlier_declares).as_bytes()).await;
        let arn = create_workflow_with_mode(&router, "promotedTrigger", key, None).await;

        stage(&router, key, definition(row.latest_declares).as_bytes()).await;
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let latest = updated["WorkflowVersion"]
            .as_str()
            .expect("setup: an update reports the version it appended")
            .to_string();
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            row.latest_declares,
            "setup {case:?}: the latest version's trigger is the one in force before the \
             delete, got {:?}",
            trigger(&engine, &arn)
        );

        let (status, deleted) = call(
            &router,
            "DeleteWorkflow",
            serde_json::json!({"WorkflowArn": arn, "WorkflowVersion": latest}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version-scoped delete to succeed, got {status} \
             {deleted:?}"
        );
        assert_eq!(
            trigger(&engine, &arn).is_some(),
            row.earlier_declares,
            "case {case:?}: expected automated execution {} after the promotion, got {:?}",
            row.earlier_declares,
            trigger(&engine, &arn)
        );
        let (status, fetched) = call(&router, "GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        assert_eq!(
            fetched["WorkflowDefinition"].as_str(),
            Some(definition(row.earlier_declares)),
            "case {case:?}: expected the promoted version's own definition, got {:?}",
            fetched["WorkflowDefinition"]
        );
        assert_eq!(
            fetched["ScheduleConfiguration"]["CronExpression"].is_string(),
            row.earlier_declares,
            "case {case:?}: the promoted version reports the schedule its own definition \
             declares, got {:?}",
            fetched["ScheduleConfiguration"]
        );
    }
}

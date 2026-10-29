// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE derivation of what a workflow reports as its ACTIVE
//! version, driven through every operation that changes the set of versions a workflow
//! holds.
//!
//! The rule under test: whatever changes a workflow's version history, the workflow
//! reports as active the version holding the greatest append position — that version is
//! the only one flagged latest, it is the one a read with no version named resolves, and
//! the workflow's revision identifier, its code-snapshot instant and the automated
//! trigger in force are all derived from THAT version. The create of the first version,
//! the update that appends a further one and the version-scoped delete that removes the
//! latest are the three events that change the active version, and all three settle it
//! identically: a workflow fresh from a create cannot report one thing and the same
//! workflow after its first update another.
//!
//! Rows differ along what the rule generalises over: the schedule the CREATED version's
//! definition declares and the one the APPENDED version's declares — none, a five-field
//! cron expression, an Apache Airflow preset standing for one, and a six-field
//! expression — because the trigger in force is part of what the active version decides,
//! and a row's two definitions make the created version and the appended one
//! distinguishable in every field the derivation touches.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    mint_revision_id, Workflow,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_definition_snapshot::{
    call, create_bucket, definition_router_on, stage, with_stored_workflow, BUCKET,
};
use crate::workflow_lifecycle::role_arn;

/// What one version's definition declares as its schedule, and the cron expression a read
/// of that version therefore reports.
///
/// An Apache Airflow preset is a shorthand, so what it reports is the expression the
/// preset stands for rather than the preset itself (the preset table in
/// `crate::state::workflow_definition`, `CRON_PRESETS`, from Airflow's DAG scheduling
/// documentation).
#[derive(Debug, Clone, Copy)]
struct Declared {
    /// The value the definition's DAG-level schedule key carries, or `None` for a
    /// definition declaring no schedule at all.
    value: Option<&'static str>,
    /// The `CronExpression` a read of the version reports, or `None` when the version's
    /// definition declares no schedule the service configures a trigger from.
    reported: Option<&'static str>,
}

/// A definition declaring no schedule.
const NONE: Declared = Declared {
    value: None,
    reported: None,
};

/// The YAML definition for `declared`: the DAG-level schedule key when the row declares
/// one, and one task so the definition describes a DAG either way.
fn definition(declared: Declared) -> String {
    match declared.value {
        Some(value) => format!("schedule: \"{value}\"\ntasks:\n  - id: extract\n"),
        None => "tasks:\n  - id: extract\n".to_string(),
    }
}

/// Assert the active-version derivation against the version history itself — in the
/// store, then over the wire — and answer the version it settled on.
///
/// `event` names the operation that last changed the history, so a failure says which of
/// the three settled the active version differently from the others.
async fn assert_active_version(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    case: &str,
    event: &str,
    expected: Declared,
) -> (String, String) {
    let expected_cron = expected.reported;
    let (active, revision_id) = with_stored_workflow(engine, arn, |workflow: &mut Workflow| {
        let newest = workflow
            .versions
            .iter()
            .max_by_key(|record| record.sequence)
            .cloned()
            .expect("a stored workflow holds at least one version");
        let flagged: Vec<String> = workflow
            .versions
            .iter()
            .filter(|record| record.is_latest)
            .map(|record| record.version.clone())
            .collect();
        assert_eq!(
            flagged,
            vec![newest.version.clone()],
            "case {case:?} after the {event}: expected the newest appended version {:?} to be \
             the only latest one, got {flagged:?}",
            newest.version
        );
        assert_eq!(
            workflow.current_version, newest.version,
            "case {case:?} after the {event}: expected the reported current version to be the \
             newest appended one {:?}, got {:?}",
            newest.version, workflow.current_version
        );
        assert_eq!(
            workflow.revision_id,
            mint_revision_id(arn, &newest.version),
            "case {case:?} after the {event}: expected the revision identifier minted for \
             {:?}, got {:?}",
            newest.version,
            workflow.revision_id
        );
        assert_eq!(
            workflow.code_snapshotted_at, newest.created_at,
            "case {case:?} after the {event}: expected the snapshot instant of the active \
             version {:?}, got {:?}",
            newest.created_at, workflow.code_snapshotted_at
        );
        let frozen = newest
            .snapshot
            .schedule_configuration
            .as_ref()
            .and_then(|schedule| schedule.cron_expression.clone());
        assert_eq!(
            frozen.as_deref(),
            expected_cron,
            "case {case:?} after the {event}: expected the active version to have frozen the \
             schedule {expected_cron:?}, got {frozen:?}"
        );
        assert_eq!(
            workflow.next_scheduled_run_at.is_some(),
            expected_cron.is_some(),
            "case {case:?} after the {event}: expected an automated trigger in force: {}, got \
             {:?}",
            expected_cron.is_some(),
            workflow.next_scheduled_run_at
        );
        (newest.version.clone(), workflow.revision_id.clone())
    });

    // A read with no version named resolves that same version, and reports the
    // configuration and the schedule THAT version froze.
    let (status, fetched) = call(router, "GetWorkflow", json!({"WorkflowArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?} after the {event}: expected GetWorkflow to succeed, got {status} \
         {fetched:?}"
    );
    assert_eq!(
        fetched["WorkflowVersion"].as_str(),
        Some(active.as_str()),
        "case {case:?} after the {event}: expected the default read to resolve {active:?}, got \
         {:?}",
        fetched["WorkflowVersion"]
    );
    assert_eq!(
        fetched["ScheduleConfiguration"]["CronExpression"].as_str(),
        expected_cron,
        "case {case:?} after the {event}: expected the active version's own schedule \
         {expected_cron:?}, got {:?}",
        fetched["ScheduleConfiguration"]
    );
    let (status, by_name) = call(
        router,
        "GetWorkflow",
        json!({"WorkflowArn": arn, "WorkflowVersion": active}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} after the {event}: expected the version-named read to succeed, got \
         {status} {by_name:?}"
    );
    assert_eq!(
        fetched["CodeSnapshottedAt"], by_name["CodeSnapshottedAt"],
        "case {case:?} after the {event}: expected the workflow's snapshot instant to be the \
         active version's own {:?}, got {:?}",
        by_name["CodeSnapshottedAt"], fetched["CodeSnapshottedAt"]
    );

    // The listing flags exactly that one version as the latest, out of the whole history.
    let (status, listed) = call(
        router,
        "ListWorkflowVersions",
        json!({"WorkflowArn": arn, "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} after the {event}: expected ListWorkflowVersions to succeed, got \
         {status} {listed:?}"
    );
    let items = listed["WorkflowVersions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let listed_latest: Vec<&str> = items
        .iter()
        .filter(|item| item["IsLatestVersion"].as_bool() == Some(true))
        .map(|item| item["WorkflowVersion"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        listed_latest,
        vec![active.as_str()],
        "case {case:?} after the {event}: expected {active:?} to be the only version listed as \
         latest, got {items:?}"
    );

    (active, revision_id)
}

/// One row of the table: the schedule the created version's definition declares, and the
/// one the version an update appends declares.
struct Row {
    case: &'static str,
    created: Declared,
    appended: Declared,
}

const ROWS: &[Row] = &[
    Row {
        case: "neither version declares a schedule",
        created: NONE,
        appended: NONE,
    },
    Row {
        case: "created without a schedule, a five-field expression appended",
        created: NONE,
        appended: Declared {
            value: Some("0 2 * * *"),
            reported: Some("0 2 * * *"),
        },
    },
    Row {
        case: "created with a preset, no schedule appended",
        created: Declared {
            value: Some("@daily"),
            reported: Some("0 0 * * *"),
        },
        appended: NONE,
    },
    Row {
        case: "created with a step expression, a six-field expression appended",
        created: Declared {
            value: Some("*/15 * * * *"),
            reported: Some("*/15 * * * *"),
        },
        appended: Declared {
            value: Some("0 3 ? * MON-FRI *"),
            reported: Some("0 3 ? * MON-FRI *"),
        },
    },
];

#[tokio::test]
async fn create_update_and_version_scoped_delete_settle_the_active_version_alike() {
    for row in ROWS {
        let case = row.case;
        // The trigger is configured from the definition each version snapshotted, so the
        // workflows below are created through the router that also serves the emulated
        // Amazon S3 the definitions live in.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = definition_router_on(Arc::clone(&engine));
        create_bucket(&router).await;
        let key = "dags/active.yaml";

        // The create: the first version appended to a brand-new history becomes the
        // workflow's active one.
        stage(&router, key, definition(row.created).as_bytes()).await;
        let (status, created) = call(
            &router,
            "CreateWorkflow",
            json!({
                "Name": "activeVersionDerivation",
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateWorkflow to succeed, got {status} {created:?}"
        );
        let arn = created["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let (first, first_revision) =
            assert_active_version(&router, &engine, &arn, case, "create", row.created).await;
        // What the create ANSWERED is that same derivation, not a second one.
        assert_eq!(
            created["WorkflowVersion"].as_str(),
            Some(first.as_str()),
            "case {case:?}: expected the create to report the version it made active, got {:?}",
            created["WorkflowVersion"]
        );
        assert_eq!(
            created["RevisionId"].as_str(),
            Some(first_revision.as_str()),
            "case {case:?}: expected the create to report the active version's revision \
             identifier, got {:?}",
            created["RevisionId"]
        );
        assert_eq!(
            created["IsLatestVersion"].as_bool(),
            Some(true),
            "case {case:?}: expected the create to report its version as the latest, got {:?}",
            created["IsLatestVersion"]
        );

        // The update: the appended version takes over as the active one, with its own
        // schedule in force.
        stage(&router, key, definition(row.appended).as_bytes()).await;
        let (status, updated) = call(
            &router,
            "UpdateWorkflow",
            json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": BUCKET, "ObjectKey": key},
                "RoleArn": role_arn(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
        );
        let (second, _) =
            assert_active_version(&router, &engine, &arn, case, "update", row.appended).await;
        assert_ne!(
            second, first,
            "case {case:?}: expected the update to append a version of its own, got {second:?}"
        );
        assert_eq!(
            updated["WorkflowVersion"].as_str(),
            Some(second.as_str()),
            "case {case:?}: expected the update to report the version it made active, got {:?}",
            updated["WorkflowVersion"]
        );

        // The version-scoped delete: with the appended version gone, the version the
        // CREATE appended is the newest one left, so the derivation settles back onto it —
        // the same values, recomputed the same single way.
        let (status, deleted) = call(
            &router,
            "DeleteWorkflow",
            json!({"WorkflowArn": arn, "WorkflowVersion": second}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the version-scoped delete to succeed, got {status} \
             {deleted:?}"
        );
        let (promoted, promoted_revision) = assert_active_version(
            &router,
            &engine,
            &arn,
            case,
            "version-scoped delete",
            row.created,
        )
        .await;
        assert_eq!(
            promoted, first,
            "case {case:?}: expected the created version {first:?} to be active again, got \
             {promoted:?}"
        );
        assert_eq!(
            promoted_revision, first_revision,
            "case {case:?}: expected the created version's revision identifier \
             {first_revision:?} back, got {promoted_revision:?}"
        );
    }
}

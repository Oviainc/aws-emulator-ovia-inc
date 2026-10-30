// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a repeated `CreateWorkflow` `ClientToken` answers once
//! LATER calls have moved the workflow on.
//!
//! The rule under test: repeating an `@idempotencyToken` answers what the token's FIRST
//! use answered — the version that call created, the revision and latest-version flag it
//! reported for that version, and the status and creation instant it reported for the
//! workflow — evolved only by what the clock already owed at that first call, and never
//! by a value a later caller's call produced. A created workflow is answered ready and
//! owes the clock no transition of its own, so every member of a create's answer can
//! move only when someone else calls: an update appends a version and makes it the
//! active one, a version-scoped delete removes one, and a delete starts the teardown.
//!
//! The whole response is compared, not a chosen member: `CreateWorkflowResponse` is
//! `WorkflowArn`, `CreatedAt`, `RevisionId`, `WorkflowStatus`, `WorkflowVersion`,
//! `IsLatestVersion` and `Warnings`
//! (`smithy_query(query="operation", operation="CreateWorkflow")`).
//!
//! The table spans the class the rule generalises over: WHICH later calls moved the
//! workflow, how MANY of them, and therefore which parts of the live record have moved
//! away from the first answer. Every row first asserts that the live record really did
//! move, so no row can pass because nothing happened.

use crate::workflow_lifecycle::{role_arn, Harness};

/// A call made between the token's first use and its repeat — the kinds of call that can
/// move what a create reported.
#[derive(Clone, Copy, Debug)]
enum Later {
    /// `UpdateWorkflow` with this definition object key: appends a version and makes it
    /// the workflow's active one.
    Update(&'static str),
    /// `DeleteWorkflow` scoped to the workflow's CURRENT version: removes that version
    /// and promotes the newest one left.
    DeleteCurrentVersion,
    /// `DeleteWorkflow` of the whole workflow: starts the teardown, so the live record
    /// reports `DELETING` for its span.
    DeleteWorkflow,
}

/// One row: the later calls made before the token is repeated.
#[derive(Clone, Debug)]
struct Row {
    case: &'static str,
    later: Vec<Later>,
}

/// The rows, differing in which later calls ran and in how many: none, one update,
/// several updates, a version removed again, a teardown started, and a combination.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no later call at all",
            later: vec![],
        },
        Row {
            case: "one update appends a second version and activates it",
            later: vec![Later::Update("dags/v2.yaml")],
        },
        Row {
            case: "three updates leave the workflow three versions past the create",
            later: vec![
                Later::Update("dags/v2.yaml"),
                Later::Update("dags/v3.yaml"),
                Later::Update("dags/v4.yaml"),
            ],
        },
        Row {
            case: "an update, then the version that update created deleted again",
            later: vec![Later::Update("dags/v2.yaml"), Later::DeleteCurrentVersion],
        },
        Row {
            case: "the workflow's own teardown has started",
            later: vec![Later::DeleteWorkflow],
        },
        Row {
            case: "an update and then the teardown",
            later: vec![Later::Update("dags/v2.yaml"), Later::DeleteWorkflow],
        },
    ]
}

/// What the LIVE record reports right now: the version and status `GetWorkflow` answers,
/// and every `(WorkflowVersion, IsLatestVersion)` pair the store holds.
async fn live(harness: &Harness, case: &str, arn: &str) -> (serde_json::Value, Vec<(String, bool)>) {
    let (status, got) = harness
        .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetWorkflow to still report the workflow, got {status} {got:?}"
    );
    (
        serde_json::json!({
            "WorkflowVersion": got["WorkflowVersion"].clone(),
            "WorkflowStatus": got["WorkflowStatus"].clone(),
        }),
        harness.stored_versions(arn),
    )
}

/// Run one later call against `arn`.
async fn apply(harness: &Harness, case: &str, arn: &str, action: Later) {
    match action {
        Later::Update(definition_key) => {
            let (status, updated) = harness
                .call(
                    "UpdateWorkflow",
                    serde_json::json!({
                        "WorkflowArn": arn,
                        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": definition_key},
                        "RoleArn": role_arn(),
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateWorkflow to succeed, got {status} {updated:?}"
            );
        }
        Later::DeleteCurrentVersion => {
            let (version, _) = live(harness, case, arn).await;
            let (status, deleted) = harness
                .call(
                    "DeleteWorkflow",
                    serde_json::json!({
                        "WorkflowArn": arn,
                        "WorkflowVersion": version["WorkflowVersion"].clone(),
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the version-scoped DeleteWorkflow to succeed, got \
                 {status} {deleted:?}"
            );
        }
        Later::DeleteWorkflow => {
            let (status, deleted) = harness
                .call("DeleteWorkflow", serde_json::json!({"WorkflowArn": arn}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteWorkflow to succeed, got {status} {deleted:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_client_token_answers_the_first_create_whatever_later_calls_did() {
    for (index, row) in rows().into_iter().enumerate() {
        // A fresh emulator per row, so this row's ledger and its workflow name are its
        // own and the listing below counts only what this row created.
        let harness = Harness::new("us-west-2");
        let token = "replayed-token";
        let request = serde_json::json!({
            "Name": format!("replayRow{index}"),
            "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
            "RoleArn": role_arn(),
            "Description": "the configuration the token's first use created",
            "ClientToken": token,
        });

        let (status, first) = harness.call("CreateWorkflow", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the first CreateWorkflow to succeed, got {status} {first:?}",
            row.case
        );
        let arn = first["WorkflowArn"]
            .as_str()
            .expect("a created workflow has an ARN")
            .to_string();
        let created_version = first["WorkflowVersion"]
            .as_str()
            .expect("a create answers the version it created")
            .to_string();

        let before = live(&harness, row.case, &arn).await;
        // At least one of the later calls must really move the live record away from
        // what the create answered, so no row passes because nothing happened. A row
        // can move it and then put it back, which is why every step is checked rather
        // than only the end state.
        let mut moved_during = false;
        for action in &row.later {
            apply(&harness, row.case, &arn, *action).await;
            moved_during |= live(&harness, row.case, &arn).await != before;
        }
        let moved = live(&harness, row.case, &arn).await;
        assert_eq!(
            moved_during,
            !row.later.is_empty(),
            "case {:?}: expected the later calls to move the live record away from what the \
             create answered, got {moved:?} against {before:?}",
            row.case
        );

        // The repeat of the token: the first answer again, member for member.
        let (status, replay) = harness.call("CreateWorkflow", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the token replay to succeed, got {status} {replay:?}",
            row.case
        );
        assert_eq!(
            replay, first,
            "case {:?}: a repeated ClientToken must answer what its first use answered \
             ({first:?}), got {replay:?}",
            row.case
        );
        // Spelled out for the members the later calls move, so a failure names which one.
        for member in ["WorkflowVersion", "RevisionId", "WorkflowStatus", "IsLatestVersion"] {
            assert_eq!(
                replay[member], first[member],
                "case {:?}: expected the replay to report the {member} the create answered \
                 ({:?}), got {:?}",
                row.case, first[member], replay[member]
            );
        }
        // And it is a version the store really holds, not a value made up for the answer.
        let held = harness.stored_versions(&arn);
        assert!(
            held.iter().any(|(version, _)| version == &created_version),
            "case {:?}: expected the store to still hold the replayed version \
             {created_version:?}, got {held:?}",
            row.case
        );

        // The replay produced nothing and moved nothing: one workflow, and a live record
        // exactly where the later calls left it.
        let (status, listed) = harness
            .call("ListWorkflows", serde_json::json!({"MaxResults": 100}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected ListWorkflows to succeed, got {status} {listed:?}",
            row.case
        );
        assert_eq!(
            listed["Workflows"].as_array().map(Vec::len),
            Some(1),
            "case {:?}: expected exactly one workflow per ClientToken, got {:?}",
            row.case,
            listed["Workflows"]
        );
        let after_replay = live(&harness, row.case, &arn).await;
        assert_eq!(
            after_replay, moved,
            "case {:?}: the replay must not change what a later caller sees, got \
             {after_replay:?} after {moved:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_replay_after_an_update_disagrees_with_the_workflow_it_created() {
    // The movement itself, asserted once rather than per row: the update makes the live
    // workflow report a version the create never created, so the replay and the live
    // record must now differ — the replay reporting the created version, the record the
    // updated one.
    let harness = Harness::new("eu-west-1");
    let token = "update-then-replay";
    let request = serde_json::json!({
        "Name": "replayAfterUpdate",
        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v1.yaml"},
        "RoleArn": role_arn(),
        "ClientToken": token,
    });

    let (status, first) = harness.call("CreateWorkflow", request.clone()).await;
    assert_eq!(
        status, 200,
        "expected the first CreateWorkflow to succeed, got {status} {first:?}"
    );
    let arn = first["WorkflowArn"]
        .as_str()
        .expect("a created workflow has an ARN")
        .to_string();

    let (status, updated) = harness
        .call(
            "UpdateWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/v2.yaml"},
                "RoleArn": role_arn(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected UpdateWorkflow to succeed, got {status} {updated:?}"
    );
    assert_ne!(
        updated["WorkflowVersion"], first["WorkflowVersion"],
        "setup: expected the update to append a version of its own, got {:?}",
        updated["WorkflowVersion"]
    );

    let (status, replay) = harness.call("CreateWorkflow", request).await;
    assert_eq!(
        status, 200,
        "expected the token replay to succeed, got {status} {replay:?}"
    );
    assert_ne!(
        replay["WorkflowVersion"], updated["WorkflowVersion"],
        "a replay must not report the version a later update created, got {:?}",
        replay["WorkflowVersion"]
    );
    assert_eq!(
        replay["IsLatestVersion"], first["IsLatestVersion"],
        "expected the replay to report the latest-version flag the create answered ({:?}), got \
         {:?}",
        first["IsLatestVersion"], replay["IsLatestVersion"]
    );

    // The version the replay reports is the created one, and it is still readable with
    // the configuration that create froze — so the replay names a real version of this
    // workflow, not the active one.
    let (status, got) = harness
        .call(
            "GetWorkflow",
            serde_json::json!({
                "WorkflowArn": arn,
                "WorkflowVersion": replay["WorkflowVersion"].clone(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected GetWorkflow to resolve the replayed version, got {status} {got:?}"
    );
    assert_eq!(
        got["DefinitionS3Location"]["ObjectKey"].as_str(),
        Some("dags/v1.yaml"),
        "expected the replayed version to be the one the create froze, got {:?}",
        got["DefinitionS3Location"]
    );
}

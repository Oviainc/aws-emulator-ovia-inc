// SPDX-License-Identifier: CC0-1.0

//! How this service CONSTRUCTS the refusals it builds out of a request's ids,
//! end to end over HTTP.
//!
//! `tests/all/refusal_constructions.rs` covers the conflict and query-statement
//! constructions; this suite covers the three that name a workspace or an entity
//! the caller addressed:
//!
//! 1. LOOK-UP PATH. Every operation that resolves a workspace in order to read
//!    or write something it holds reports the same construction for a workspace
//!    this Region's store does not hold — "Unable to find workspace for id:
//!    <workspaceId>" — whichever of the modelled refusals that operation answers
//!    with (a read of the workspace or of something inside it is a 404
//!    `ResourceNotFoundException`; a create or a listing under it is a 400
//!    `ValidationException`, the error those operations model).
//! 2. UPDATE PATH. The workspace's own update reports the OTHER construction for
//!    the same miss, scoped by the account the workspace id belongs to —
//!    "Workspace with id <accountId>|<workspaceId> does not exist."
//! 3. CHILD ENTITIES. A delete of an entity that still has children, without
//!    `isRecursive`, names BOTH the entity it addressed and the workspace it
//!    addressed it in — "Cannot delete entity <entityId> in <workspaceId> as it
//!    has child entities."
//!
//! Each rule is asserted over a table whose rows differ along what it
//! generalises over: the OPERATION the refusal comes from and the FORM of the
//! workspace id for the first two, and the number and depth of the children, the
//! form of the entity id and the workspace it sits in for the third.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::SYNC_SOURCE_SITEWISE;

use crate::harness::{Emulator, Resp, SYNC_ROLE};

/// The message an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// One operation of this service, driven against a workspace id.
///
/// Named rather than taken as a closure so the table below stays a table: each
/// row is the operation's name plus the status and error code the MODEL declares
/// for it.
async fn call_against_workspace(emulator: &Emulator, operation: &str, workspace_id: &str) -> Resp {
    match operation {
        "GetWorkspace" => emulator.get_workspace(workspace_id).await,
        "DeleteWorkspace" => emulator.delete_workspace(workspace_id).await,
        "GetEntity" => emulator.get_entity(workspace_id, "someEntity").await,
        "DeleteEntity" => emulator.delete_entity(workspace_id, "someEntity", false).await,
        "GetComponentType" => {
            emulator
                .get_component_type(workspace_id, "com.example.Pump")
                .await
        }
        "GetScene" => emulator.get_scene(workspace_id, "someScene").await,
        "CreateEntity" => {
            emulator
                .create_entity(workspace_id, json!({"entityName": "someEntity"}))
                .await
        }
        "ListEntities" => emulator.list_entities(workspace_id, json!({})).await,
        "ListScenes" => emulator.list_scenes(workspace_id, json!({})).await,
        "CreateSyncJob" => {
            emulator
                .create_sync_job(
                    workspace_id,
                    SYNC_SOURCE_SITEWISE,
                    json!({"syncRole": SYNC_ROLE}),
                )
                .await
        }
        other => panic!("the table names no operation {other:?}"),
    }
}

/// RULE: a path that LOOKS a workspace up names, in one construction, the
/// workspace it could not find for the id it was given.
#[tokio::test]
async fn a_look_up_path_names_the_workspace_it_could_not_find() {
    let emulator = Emulator::new();

    // (operation, status, error code) — the refusal the MODEL declares for that
    // operation: the operations that address an existing resource model
    // `ResourceNotFoundException`, while the creates and listings under a
    // workspace model only `ValidationException`.
    let operations: [(&str, u16, &str); 10] = [
        ("GetWorkspace", 404, "ResourceNotFoundException"),
        ("DeleteWorkspace", 404, "ResourceNotFoundException"),
        ("GetEntity", 404, "ResourceNotFoundException"),
        ("DeleteEntity", 404, "ResourceNotFoundException"),
        ("GetComponentType", 404, "ResourceNotFoundException"),
        ("GetScene", 404, "ResourceNotFoundException"),
        ("CreateEntity", 400, "ValidationException"),
        ("ListEntities", 400, "ValidationException"),
        ("ListScenes", 400, "ValidationException"),
        ("CreateSyncJob", 400, "ValidationException"),
    ];

    // The forms a `workspaceId` takes: the modelled `Id` is
    // `@length(1..128)` and `^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$`
    // (`smithy_query(query="operation",operation="GetWorkspace")`), so the rows
    // run from the shortest legal id to the longest, through the punctuation and
    // the digits the pattern admits.
    let ids: [(&str, String); 5] = [
        ("shortest", "ab".to_string()),
        ("dashed", "missing-workspace-7".to_string()),
        ("underscored", "_missing_workspace_0".to_string()),
        ("digits", "0123456789".to_string()),
        ("longest", format!("w{}z", "a".repeat(126))),
    ];

    for (operation, status, code) in operations {
        for (id_case, workspace_id) in &ids {
            let case = format!("{operation} on a {id_case} id");
            let refused = call_against_workspace(&emulator, operation, workspace_id).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (status, Some(code)),
                "case {case:?}: expected {status} {code}, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                format!("Unable to find workspace for id: {workspace_id}"),
                "case {case:?}: expected the look-up construction, got {refused:?}"
            );
        }
    }
}

/// RULE: the workspace's own UPDATE path reports the account-scoped
/// construction for a workspace that does not exist — a different sentence from
/// the look-up path's, so a caller can tell a read miss from an update miss.
#[tokio::test]
async fn the_update_path_names_the_account_scoped_workspace_id() {
    let emulator = Emulator::new();
    // The account every ARN and every account-scoped message this emulator
    // builds carries, read off the runtime rather than written out here.
    let account = emulator_server::emulator_account_id();

    // The rows differ along the id's form, as above, and along the patch the
    // update carries: the refusal is about the workspace, not about the patch.
    let rows: [(&str, String, serde_json::Value); 4] = [
        ("never created", "neverCreated".to_string(), json!({})),
        (
            "never created, description patch",
            "never_created_2".to_string(),
            json!({"description": "post-delete-update"}),
        ),
        (
            "never created, longest id",
            format!("w{}z", "b".repeat(126)),
            json!({"role": SYNC_ROLE}),
        ),
        (
            "never created, s3Location patch",
            "nc3".to_string(),
            json!({"s3Location": "arn:aws:s3:::twinmaker-bucket"}),
        ),
    ];

    for (case, workspace_id, patch) in &rows {
        let refused = emulator.update_workspace(workspace_id, patch.clone()).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Workspace with id {account}|{workspace_id} does not exist."),
            "case {case:?}: expected the update construction, got {refused:?}"
        );
    }

    // A workspace that DID exist and was deleted is the same miss on both paths,
    // each in its own construction: that is how a caller tells them apart.
    let workspace_id = "deletedWorkspace";
    assert_eq!(emulator.create_workspace(workspace_id).await.status, 200);
    assert_eq!(emulator.delete_workspace(workspace_id).await.status, 200);
    let read = emulator.get_workspace(workspace_id).await;
    assert_eq!(
        (read.status, read.error_type.as_deref(), message(&read)),
        (
            404,
            Some("ResourceNotFoundException"),
            format!("Unable to find workspace for id: {workspace_id}")
        ),
        "case \"deleted, read path\": expected the look-up construction, got {read:?}"
    );
    let updated = emulator
        .update_workspace(workspace_id, json!({"description": "post-delete-update"}))
        .await;
    assert_eq!(
        (updated.status, updated.error_type.as_deref(), message(&updated)),
        (
            404,
            Some("ResourceNotFoundException"),
            format!("Workspace with id {account}|{workspace_id} does not exist.")
        ),
        "case \"deleted, update path\": expected the update construction, got {updated:?}"
    );
}

/// RULE: a delete refused because the entity still has children names the
/// entity and the workspace that holds it, and nothing about how many children
/// there are — the caller's fix is `isRecursive`, not a count.
#[tokio::test]
async fn a_blocked_delete_names_the_entity_and_its_workspace() {
    let emulator = Emulator::new();

    // (case, workspaceId, entityId, how many children hang off it, how deep the
    // chain below it runs). The rows differ along what the construction
    // generalises over: the workspace, the form of the `entityId` (the modelled
    // `EntityId` admits both a uuid and a label, from
    // `smithy_query(query="operation",operation="DeleteEntity")`), and the shape
    // of the subtree that blocks the delete.
    let rows: [(&str, &str, &str, usize, usize); 4] = [
        ("one child", "childrenWsA", "parentEntity", 1, 1),
        (
            "a uuid entity id, one child",
            "childrenWsA",
            "a793021f-afb7-4515-adc7-853db7e4048f",
            1,
            1,
        ),
        ("several children", "childrenWsB", "wide.parent:1", 4, 1),
        ("a deep chain", "childrenWsB", "deep_parent", 1, 3),
    ];

    for (case, workspace_id, entity_id, children, depth) in rows {
        if emulator.get_workspace(workspace_id).await.status != 200 {
            assert_eq!(
                emulator.create_workspace(workspace_id).await.status,
                200,
                "case {case:?}: the workspace must exist"
            );
        }
        assert_eq!(
            emulator
                .create_entity(
                    workspace_id,
                    json!({"entityId": entity_id, "entityName": "Parent"}),
                )
                .await
                .status,
            200,
            "case {case:?}: the parent entity must exist"
        );
        // The children hang off the parent, and the chain hangs off the first of
        // them, so the subtree below the parent differs row by row.
        let mut parent = entity_id.to_string();
        for child in 0..children {
            let child_id = format!("{entity_id}-child-{child}");
            assert_eq!(
                emulator
                    .create_entity(
                        workspace_id,
                        json!({
                            "entityId": child_id,
                            "entityName": child_id,
                            "parentEntityId": entity_id,
                        }),
                    )
                    .await
                    .status,
                200,
                "case {case:?}: child {child} must exist"
            );
            if child == 0 {
                parent = child_id;
            }
        }
        for level in 1..depth {
            let descendant = format!("{entity_id}-level-{level}");
            assert_eq!(
                emulator
                    .create_entity(
                        workspace_id,
                        json!({
                            "entityId": descendant,
                            "entityName": descendant,
                            "parentEntityId": parent,
                        }),
                    )
                    .await
                    .status,
                200,
                "case {case:?}: level {level} must exist"
            );
            parent = descendant;
        }

        let refused = emulator.delete_entity(workspace_id, entity_id, false).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            format!("Cannot delete entity {entity_id} in {workspace_id} as it has child entities."),
            "case {case:?}: expected the blocked-delete construction, got {refused:?}"
        );
        // The refusal changed nothing: the entity is still there, and the delete
        // takes once the caller asks for the subtree.
        assert_eq!(
            emulator.get_entity(workspace_id, entity_id).await.status,
            200,
            "case {case:?}: a refused delete must leave the entity in place"
        );
        assert_eq!(
            emulator.delete_entity(workspace_id, entity_id, true).await.status,
            200,
            "case {case:?}: the recursive delete must be served"
        );
    }
}

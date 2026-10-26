// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteApplication`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a delete answers 204 with no body and removes EXACTLY the application the
//!   request names, leaving every other application of that workspace, and every
//!   application of every other workspace, alone;
//! * the deleted application stops resolving everywhere it used to: its reader
//!   answers the modelled 404 and its ARN no longer carries tags;
//! * an application is addressed by `(workspaceName, id)`, so a delete naming a
//!   workspace this service does not hold, a delete naming an id that lives in
//!   another workspace, and a delete of an id that resolves to no stored
//!   application — never created, or already deleted — are all modelled not-found
//!   refusals that remove nothing.
//!
//! The rows differ along what the rule generalises over: the shape and length of
//! the workspace the application lives in, whether the application carried tags
//! (none, one, several) and a description, whether its create carried a
//! `clientToken`, how many applications its workspace held beside it (none, one,
//! several) and whether another workspace held applications too.
//!
//! The HTTP harness (`call`, `error_code`, `is_modelled_id`, `router_with_engine`,
//! `REGION`) is the one the asset suite already has, and the application helpers
//! (`create`, `describe`, `create_workspace`) are the ones the application
//! lifecycle suite already has; this suite reuses both rather than growing a
//! second copy.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use super::application_lifecycle::{create, create_workspace, describe};
use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};

/// One row of the delete table.
#[derive(Debug)]
struct DeleteCase {
    label: &'static str,
    /// The workspace the deleted application lives in, varied along what
    /// `WorkspaceName` admits (smithy_query query="operation"
    /// operation="DeleteApplication": len 1..64, pattern `^[a-zA-Z0-9_-]+$`).
    workspace: String,
    /// The description the application was created with, if any.
    description: Option<&'static str>,
    /// The tags the application was created with, if any.
    tags: Option<Value>,
    /// The `clientToken` the create carried, if any.
    client_token: Option<&'static str>,
    /// How many further applications the SAME workspace holds.
    neighbours: usize,
    /// How many applications a SECOND workspace holds.
    other_workspace: usize,
}

/// The rows.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "the smallest workspace name, the only application anywhere",
            workspace: "w".to_string(),
            description: None,
            tags: None,
            client_token: None,
            neighbours: 0,
            other_workspace: 0,
        },
        DeleteCase {
            label: "a hyphenated workspace, one tag, one neighbour",
            workspace: "plant-floor".to_string(),
            description: Some("the application the floor operators open"),
            tags: Some(json!({ "team": "ops" })),
            client_token: None,
            neighbours: 1,
            other_workspace: 1,
        },
        DeleteCase {
            label: "an underscored workspace, several tags, several neighbours",
            workspace: "plant_floor_2".to_string(),
            description: Some("second floor"),
            tags: Some(json!({ "team": "data", "env": "test" })),
            client_token: Some("11111111-2222-3333-4444-555555555555"),
            neighbours: 3,
            other_workspace: 0,
        },
        DeleteCase {
            label: "a workspace at the top of its 64-character length",
            workspace: "w".repeat(64),
            description: None,
            tags: Some(json!({ "only": "one" })),
            client_token: Some("66666666-7777-8888-9999-aaaaaaaaaaaa"),
            neighbours: 2,
            other_workspace: 2,
        },
    ]
}

/// The second workspace of a row, which every delete must leave alone.
const OTHER_WORKSPACE: &str = "the-other-workspace";

/// `DELETE /workspaces/{workspaceName}/applications/{id}`, as the model binds the
/// route (smithy_query query="operation" operation="DeleteApplication").
async fn delete_application(
    router: &MultiServiceRouter,
    workspace: &str,
    id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/workspaces/{workspace}/applications/{id}"),
        None,
    )
    .await
}

/// `ListTagsForResource` on `arn` — the reader that proves a deleted
/// application's ARN stops resolving.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags?resourceArn={arn}"), None).await
}

/// The ARN form this emulator mints for an application, spelled out here as an
/// independent oracle.
fn expected_arn(id: &str) -> String {
    format!(
        "arn:aws:iotsitewise:{REGION}:{}:application/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Create one application in `workspace` and return `(id, arn)`.
async fn create_application(
    router: &MultiServiceRouter,
    case: &str,
    workspace: &str,
    name: &str,
    description: Option<&str>,
    tags: Option<&Value>,
    client_token: Option<&str>,
) -> (String, String) {
    let mut body = json!({
        "workspaceName": workspace,
        "name": name,
        "idcInstanceArn": "arn:aws:sso:::instance/ssoins-7777777777777777",
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = description {
        object.insert("description".to_string(), json!(description));
    }
    if let Some(tags) = tags {
        object.insert("tags".to_string(), tags.clone());
    }
    if let Some(token) = client_token {
        object.insert("clientToken".to_string(), json!(token));
    }

    let (status, created) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApplication expected 200, got {status} {created}"
    );
    let id = created["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_modelled_id(&id),
        "case {case:?}: the created id must be a modelled one, got {created}"
    );
    let arn = created["arn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        arn,
        expected_arn(&id),
        "case {case:?}: unexpected ARN in {created}"
    );
    (id, arn)
}

/// The ids of every application the store holds, sorted.
fn stored_application_ids(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the iotsitewise store");
    let mut ids: Vec<String> = state.lock().applications.keys().cloned().collect();
    ids.sort();
    ids
}

/// The members of a `DescribeApplication` answer a delete of ANOTHER application
/// cannot move. `status` is left out on purpose: the clock moves it from
/// `CREATING` to `ACTIVE` on its own, so comparing it would pin the clock rather
/// than the delete.
const STABLE_MEMBERS: [&str; 9] = [
    "arn",
    "id",
    "name",
    "workspaceName",
    "dnsSubdomain",
    "idcApplicationArn",
    "description",
    "createdAt",
    "updatedAt",
];

#[tokio::test]
async fn a_delete_removes_exactly_the_named_application_and_nothing_else() {
    for case in delete_cases() {
        let (router, engine) = router_with_engine();
        create_workspace(&router, &case.workspace).await;
        create_workspace(&router, OTHER_WORKSPACE).await;

        let (doomed, arn) = create_application(
            &router,
            case.label,
            &case.workspace,
            "TheApplicationThatGoes",
            case.description,
            case.tags.as_ref(),
            case.client_token,
        )
        .await;

        // The applications that must survive, in the same workspace and in another.
        let mut survivors: Vec<(String, String)> = Vec::new();
        for at in 0..case.neighbours {
            let (id, _) = create_application(
                &router,
                case.label,
                &case.workspace,
                &format!("Neighbour{at}"),
                Some("a neighbour of the application that goes"),
                Some(&json!({ "keep": "me" })),
                None,
            )
            .await;
            survivors.push((case.workspace.clone(), id));
        }
        for at in 0..case.other_workspace {
            let (id, _) = create_application(
                &router,
                case.label,
                OTHER_WORKSPACE,
                &format!("Elsewhere{at}"),
                None,
                None,
                None,
            )
            .await;
            survivors.push((OTHER_WORKSPACE.to_string(), id));
        }

        // What the store really held before the delete, so what the delete removed
        // is measured against it.
        let (status, before) = describe(&router, &case.workspace, &doomed).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeApplication before the delete expected 200, got {status} {before}",
            case.label
        );
        let mut survivors_before: Vec<Value> = Vec::new();
        for (workspace, id) in &survivors {
            let (status, body) = describe(&router, workspace, id).await;
            assert_eq!(
                status, 200,
                "case {:?}: the surviving application {id} expected 200, got {status} {body}",
                case.label
            );
            survivors_before.push(body);
        }

        let (status, answer) = delete_application(&router, &case.workspace, &doomed).await;
        assert_eq!(
            status, 204,
            "case {:?}: DeleteApplication expected 204, got {status} {answer}",
            case.label
        );
        assert!(
            answer.is_null() || answer.as_object().is_some_and(Map::is_empty),
            "case {:?}: DeleteApplication's modelled output carries no members, got {answer}",
            case.label
        );

        // The application is gone from its reader and from the store.
        let (status, after) = describe(&router, &case.workspace, &doomed).await;
        assert_eq!(
            (status, error_code(&after)),
            (404, "ResourceNotFoundException"),
            "case {:?}: DescribeApplication after the delete must be a modelled 404, \
             got {status} {after}",
            case.label
        );
        let mut expected: Vec<String> = survivors.iter().map(|(_, id)| id.clone()).collect();
        expected.sort();
        assert_eq!(
            stored_application_ids(&engine),
            expected,
            "case {:?}: the store must hold exactly the applications the delete spared",
            case.label
        );

        // Its ARN stops resolving: the tags it carried went with the record.
        let (status, tags) = list_tags(&router, &arn).await;
        assert_eq!(
            (status, error_code(&tags)),
            (404, "ResourceNotFoundException"),
            "case {:?}: the deleted application's ARN must stop resolving, got {status} {tags}",
            case.label
        );

        // A second delete of the same id is absence, not a silent success.
        let (status, again) = delete_application(&router, &case.workspace, &doomed).await;
        assert_eq!(
            (status, error_code(&again)),
            (404, "ResourceNotFoundException"),
            "case {:?}: a second delete of the same id must be a modelled 404, \
             got {status} {again}",
            case.label
        );

        // Every other application is exactly as it was, and its tags still resolve.
        for ((workspace, id), was) in survivors.iter().zip(&survivors_before) {
            let (status, now) = describe(&router, workspace, id).await;
            assert_eq!(
                status, 200,
                "case {:?}: the surviving application {id} expected 200, got {status} {now}",
                case.label
            );
            for member in STABLE_MEMBERS {
                assert_eq!(
                    now[member], was[member],
                    "case {:?}: the delete moved {member} of the surviving application {id}: \
                     {now}",
                    case.label
                );
            }
            let (status, tags) = list_tags(&router, &expected_arn(id)).await;
            assert_eq!(
                status, 200,
                "case {:?}: the surviving application {id} must still carry tags, \
                 got {status} {tags}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn a_delete_resolves_the_application_by_its_workspace_and_its_id() {
    let (router, engine) = router_with_engine();
    create_workspace(&router, "plant-floor").await;
    create_workspace(&router, "other-workspace").await;
    let (stored, _) = create_application(
        &router,
        "scoped",
        "plant-floor",
        "Scoped",
        Some("as created"),
        Some(&json!({ "stay": "put" })),
        None,
    )
    .await;
    let before = describe(&router, "plant-floor", &stored).await.1;

    // One application that the delete below really removes, so "already deleted" is
    // a row of the table rather than a case nothing stands behind.
    let (removed, _) = create_application(
        &router,
        "twice-deleted",
        "plant-floor",
        "TwiceDeleted",
        None,
        None,
        None,
    )
    .await;
    let (status, answer) = delete_application(&router, "plant-floor", &removed).await;
    assert_eq!(
        status, 204,
        "the first delete expected 204, got {status} {answer}"
    );

    // Every way a delete can address no stored application: the right id in the
    // wrong workspace, a workspace no create opened, an id no create minted (and
    // the neighbour of the nil id), and an id a delete already removed.
    let cases = [
        (
            "the-stored-id-from-another-workspace",
            "other-workspace".to_string(),
            stored.clone(),
            stored.clone(),
        ),
        (
            "a-workspace-this-service-does-not-hold",
            "no-such-workspace".to_string(),
            stored.clone(),
            "no-such-workspace".to_string(),
        ),
        (
            "an-id-no-create-minted",
            "plant-floor".to_string(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "the-neighbour-of-the-nil-id",
            "plant-floor".to_string(),
            "00000000-0000-0000-0000-000000000001".to_string(),
            "00000000-0000-0000-0000-000000000001".to_string(),
        ),
        (
            "an-id-a-delete-already-removed",
            "plant-floor".to_string(),
            removed.clone(),
            removed.clone(),
        ),
    ];

    for (case, workspace, id, named) in cases {
        let (status, refused) = delete_application(&router, &workspace, &id).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a delete addressing no stored application must be a modelled \
             404, got {status} {refused}"
        );
        // The refusal names the identifier the REQUEST supplied — the absent
        // workspace where that is what is missing, the application id otherwise.
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {refused}"
        );
    }

    // Every refusal wrote nothing: the one application that was never deleted is
    // still there, unchanged, and it is the only one the store holds.
    let (status, after) = describe(&router, "plant-floor", &stored).await;
    assert_eq!(status, 200, "the stored application: got {status} {after}");
    for member in STABLE_MEMBERS {
        assert_eq!(
            after[member], before[member],
            "a refused delete must leave {member} of the stored application alone: {after}"
        );
    }
    assert_eq!(
        stored_application_ids(&engine),
        vec![stored.clone()],
        "the store must hold exactly the application no delete removed"
    );
}

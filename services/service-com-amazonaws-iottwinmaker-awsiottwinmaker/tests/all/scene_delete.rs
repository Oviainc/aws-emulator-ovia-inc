// SPDX-License-Identifier: CC0-1.0

//! `DeleteScene`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: a delete removes the scene from the workspace that contains it,
//! answers the empty 200 the response models and stops its ARN resolving,
//! whatever the scene carried; a label that names a resource the store does not
//! hold is a `ResourceNotFoundException`; and a delete touches exactly the one
//! scene it addresses, leaving its siblings — and the id itself, which becomes
//! free again — exactly as they were.

use serde_json::{json, Value};

use crate::harness::{scene_arn, Emulator};

/// Whether a response body carries no members at all: `DeleteSceneResponse`
/// models none, so the removal is reported by the 200 alone — an empty JSON
/// object and an empty payload are the same statement on the wire.
fn carries_no_members(body: &Value) -> bool {
    match body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    }
}

/// The table the removal suite is built from: `(sceneId, create request)`.
///
/// The rows differ along what the rule generalises over — which optional
/// members the scene carries, the number and order of its `capabilities`, the
/// size of its `sceneMetadata` and `tags` maps, the shape of its `sceneId` and
/// the form of its `contentLocation` (the `S3Url` pattern
/// `^[sS]3://[A-Za-z0-9._/-]+$` admits either case of the scheme and a nested
/// key) — never along a mere rename.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("bare", json!({"contentLocation": "s3://bucket/bare.json"})),
        (
            "described",
            json!({
                "contentLocation": "S3://bucket/nested/path/described.json",
                "description": "a described scene",
            }),
        ),
        (
            "withCapabilities",
            json!({
                "contentLocation": "s3://bucket/caps.json",
                // Deliberately NOT sorted: `SceneCapabilities` is a list.
                "capabilities": ["zeta", "alpha", "MATTERPORT"],
            }),
        ),
        (
            "scene_with_metadata",
            json!({
                "contentLocation": "s3://bucket/meta.json",
                "sceneMetadata": {"floor": "3", "building_id": "B-12"},
            }),
        ),
        (
            "Scene-Tagged",
            json!({
                "contentLocation": "s3://bucket/tagged.json",
                "tags": {"team": "plant", "env": "test"},
            }),
        ),
        (
            "0scene9",
            json!({
                "contentLocation": "s3://bucket/dotted.name_v1.json",
                "description": "every optional member at once",
                "capabilities": ["one", "two"],
                "sceneMetadata": {"k": ""},
                "tags": {"only": "tag"},
            }),
        ),
    ]
}

/// RULE: a delete removes the scene from the workspace that contains it —
/// whatever the scene carries — reports the removal with the empty HTTP 200 its
/// response models, and stops the ARN it was minted under resolving, so neither
/// a read of the scene nor a read of its ARN's tags finds anything.
#[tokio::test]
async fn deleting_a_scene_removes_it() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("sceneDelete").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    for (scene_id, request) in rows() {
        let case = scene_id;
        let mut body = request.clone();
        body["sceneId"] = json!(scene_id);
        let created = emulator.create_scene("sceneDelete", body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {created:?}"
        );
        let before = emulator.get_scene("sceneDelete", scene_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the scene must be readable before the delete, got {before:?}"
        );

        let deleted = emulator.delete_scene("sceneDelete", scene_id).await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected a plain 200 from DeleteScene, got {deleted:?}"
        );
        assert!(
            carries_no_members(&deleted.body),
            "case {case:?}: DeleteSceneResponse models no members, got {deleted:?}"
        );

        let gone = emulator.get_scene("sceneDelete", scene_id).await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the scene must be gone, got {gone:?}"
        );
        let arn = scene_arn("sceneDelete", scene_id);
        let tags = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted scene's ARN must stop resolving, got {tags:?}"
        );
    }

    // Nothing the cases created is left, so the workspace is empty and
    // deletable: `DeleteWorkspace` refuses one that still holds a scene, which
    // is what makes this a check on the removal and not only on what a read
    // reports.
    let workspace_gone = emulator.delete_workspace("sceneDelete").await;
    assert_eq!(
        workspace_gone.status, 200,
        "the workspace must be empty once every case's scene is deleted, got {workspace_gone:?}"
    );
}

/// RULE: both of this operation's labels address an EXISTING resource, so one
/// naming something the store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) — the same reading `GetScene` gives
/// the same two labels — and the refusal says what was missing.
///
/// The rows differ along WHICH label misses: the workspace, the scene, a scene
/// that exists only in ANOTHER workspace, and a scene a previous delete already
/// removed.
#[tokio::test]
async fn deleting_what_the_store_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    for workspace_id in ["sceneMiss", "otherSceneMiss"] {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases are looked for in must exist"
        );
    }
    assert_eq!(
        emulator
            .create_scene(
                "otherSceneMiss",
                json!({"sceneId": "elsewhere", "contentLocation": "s3://bucket/elsewhere.json"}),
            )
            .await
            .status,
        200,
        "the scene the foreign-workspace case addresses must exist somewhere"
    );
    assert_eq!(
        emulator
            .create_scene(
                "sceneMiss",
                json!({"sceneId": "once", "contentLocation": "s3://bucket/once.json"}),
            )
            .await
            .status,
        200,
        "the scene the repeated-delete case removes must exist"
    );
    assert_eq!(
        emulator.delete_scene("sceneMiss", "once").await.status,
        200,
        "the first delete of the repeated-delete case must succeed"
    );

    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "no-such-workspace",
            "noSuchSceneWs",
            "absentScene",
            "noSuchSceneWs",
        ),
        ("no-such-scene", "sceneMiss", "absentScene", "absentScene"),
        // A scene stored in another workspace: a scene is addressed inside ITS
        // OWN workspace, so the id does not resolve here.
        ("foreign-workspace", "sceneMiss", "elsewhere", "elsewhere"),
        ("already-deleted", "sceneMiss", "once", "once"),
    ];

    for (case, workspace_id, scene_id, named) in rows {
        let refused = emulator.delete_scene(workspace_id, scene_id).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(named)),
            "case {case:?}: the refusal must name what was missing, got {refused:?}"
        );
    }

    // A refused delete persists nothing and removes nothing: the scene it
    // named in the other workspace is untouched.
    let untouched = emulator.get_scene("otherSceneMiss", "elsewhere").await;
    assert_eq!(
        untouched.status, 200,
        "a refused delete must leave the scene it did not address alone, got {untouched:?}"
    );
}

/// RULE: a delete touches exactly the one scene its labels address — every
/// other scene of the workspace reads back unchanged — and it frees the id, so
/// the same `sceneId` can be created again and then reports the NEW scene's
/// members under a freshly resolving ARN.
///
/// The rows differ along which scene of the workspace is deleted (the first,
/// a middle one, the last), because the rule generalises over the position of
/// the removal within the workspace's contents.
#[tokio::test]
async fn a_delete_touches_only_the_scene_it_addresses() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("sceneIsolation").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    let held = ["first", "middle", "last"];
    for target in held {
        // Stand every scene of the workspace up again for this row, each with
        // its own content so a confusion between them is visible.
        for scene_id in held {
            let created = emulator
                .create_scene(
                    "sceneIsolation",
                    json!({
                        "sceneId": scene_id,
                        "contentLocation": format!("s3://bucket/{scene_id}.json"),
                        "description": format!("the {scene_id} scene"),
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {target:?}: scene {scene_id:?} must be created, got {created:?}"
            );
        }

        let deleted = emulator.delete_scene("sceneIsolation", target).await;
        assert_eq!(
            deleted.status, 200,
            "case {target:?}: the delete must succeed, got {deleted:?}"
        );

        for scene_id in held.iter().filter(|held| **held != target) {
            let sibling = emulator.get_scene("sceneIsolation", scene_id).await;
            assert_eq!(
                (
                    sibling.status,
                    sibling.member("contentLocation").as_str(),
                    sibling.member("description").as_str(),
                ),
                (
                    200,
                    Some(format!("s3://bucket/{scene_id}.json").as_str()),
                    Some(format!("the {scene_id} scene").as_str()),
                ),
                "case {target:?}: sibling {scene_id:?} must read back unchanged, got {sibling:?}"
            );
        }

        // The id is free again: re-creating it is a 200 (not the 409 a taken id
        // earns), and the new scene is what the read and the ARN report.
        let recreated = emulator
            .create_scene(
                "sceneIsolation",
                json!({
                    "sceneId": target,
                    "contentLocation": "s3://bucket/recreated.json",
                    "tags": {"round": "two"},
                }),
            )
            .await;
        assert_eq!(
            (recreated.status, recreated.member("arn").as_str()),
            (200, Some(scene_arn("sceneIsolation", target).as_str())),
            "case {target:?}: a deleted id must be free to create again, got {recreated:?}"
        );
        let read = emulator.get_scene("sceneIsolation", target).await;
        assert_eq!(
            (read.status, read.member("contentLocation").as_str()),
            (200, Some("s3://bucket/recreated.json")),
            "case {target:?}: the read must report the NEW scene, got {read:?}"
        );
        assert!(
            read.member("description").is_null(),
            "case {target:?}: the re-created scene carries none of the deleted one's \
             members, got {read:?}"
        );
        let tags = emulator
            .list_tags_for_resource(&scene_arn("sceneIsolation", target), json!({}))
            .await;
        assert_eq!(
            (tags.status, tags.member("tags")),
            (200, &json!({"round": "two"})),
            "case {target:?}: the re-created scene's ARN resolves to its own tags, got {tags:?}"
        );

        // Leave the workspace empty for the next row.
        for scene_id in held {
            let gone = emulator.delete_scene("sceneIsolation", scene_id).await;
            assert_eq!(
                gone.status, 200,
                "case {target:?}: scene {scene_id:?} must be deletable, got {gone:?}"
            );
        }
    }
}

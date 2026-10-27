// SPDX-License-Identifier: CC0-1.0

//! `UpdateScene` as a PATCH on a stored scene, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The RULE, asserted over a table rather than one case: an update replaces
//! exactly the members the request names — a collection it names whole, so the
//! empty list or map clears it rather than merging into what was stored — leaves
//! every member it does not name as the scene stored it, and answers the instant
//! of the change, which is the `updateDateTime` the next `GetScene` reports. A
//! workspace or a scene this Region's store does not hold is refused 404
//! `ResourceNotFoundException`, and a refused update persists nothing.

use serde_json::{json, Value};

use crate::harness::{scene_arn, Emulator, Resp};

/// The workspace and the scene every patch below is applied to.
const WORKSPACE: &str = "patchedScenes";
const SCENE: &str = "patchedScene";

/// What the scene is created with, so each patch has something to leave alone:
/// a content location, a description, a (deliberately unsorted) capability list
/// and a two-entry metadata map.
fn created() -> Value {
    json!({
        "sceneId": SCENE,
        "contentLocation": "s3://bucket/created.json",
        "description": "the created description",
        "capabilities": ["zeta", "alpha"],
        "sceneMetadata": {"floor": "3", "building_id": "B-12"},
    })
}

/// The member `name` as the read reports it.
fn reported(resp: &Resp, name: &str) -> Value {
    resp.member(name).clone()
}

/// The patchable members, which every row below states in full.
const PATCHABLE: [&str; 4] = [
    "contentLocation",
    "description",
    "capabilities",
    "sceneMetadata",
];

/// RULE: a patch replaces the members it names — a collection whole — keeps the
/// rest, and the instant it answers is the one the next read reports.
#[tokio::test]
async fn a_scene_patch_replaces_what_it_names_and_keeps_the_rest() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the scene is created in must exist"
    );
    let create = emulator.create_scene(WORKSPACE, created()).await;
    assert_eq!(
        create.status, 200,
        "the scene the patches apply to must be created, got {create:?}"
    );

    // (case, the patch, the members the scene must report after it). The rows
    // differ along WHICH members the patch names, and along the size, the order
    // and the emptiness of the collections it names — each member alone, every
    // one at once, and none at all — because that is what the rule generalises
    // over; the expectations carry the whole set, so a member the patch does not
    // name is asserted unchanged by the same row.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            // The `S3Url` pattern `^[sS]3://[A-Za-z0-9._/-]+$` admits either
            // case of the scheme and a nested key.
            "the contentLocation alone",
            json!({"contentLocation": "S3://bucket/nested/path/patched.json"}),
            json!({
                "contentLocation": "S3://bucket/nested/path/patched.json",
                "description": "the created description",
                "capabilities": ["zeta", "alpha"],
                "sceneMetadata": {"floor": "3", "building_id": "B-12"},
            }),
        ),
        (
            "the description alone",
            json!({"description": "patched once"}),
            json!({
                "contentLocation": "S3://bucket/nested/path/patched.json",
                "description": "patched once",
                "capabilities": ["zeta", "alpha"],
                "sceneMetadata": {"floor": "3", "building_id": "B-12"},
            }),
        ),
        (
            // Longer than what was stored, and in an order of its own:
            // `capabilities` is a list, so its order is part of the value.
            "the capabilities alone, reordered and grown",
            json!({"capabilities": ["alpha", "MATTERPORT", "zeta"]}),
            json!({
                "contentLocation": "S3://bucket/nested/path/patched.json",
                "description": "patched once",
                "capabilities": ["alpha", "MATTERPORT", "zeta"],
                "sceneMetadata": {"floor": "3", "building_id": "B-12"},
            }),
        ),
        (
            // One entry replacing two: the stored key the patch does not name
            // is GONE, because a named map is the whole value and not a merge.
            "the sceneMetadata alone, replaced not merged",
            json!({"sceneMetadata": {"floor": "9"}}),
            json!({
                "contentLocation": "S3://bucket/nested/path/patched.json",
                "description": "patched once",
                "capabilities": ["alpha", "MATTERPORT", "zeta"],
                "sceneMetadata": {"floor": "9"},
            }),
        ),
        (
            "the empty collections, which clear them",
            json!({"capabilities": [], "sceneMetadata": {}}),
            json!({
                "contentLocation": "S3://bucket/nested/path/patched.json",
                "description": "patched once",
                "capabilities": [],
                "sceneMetadata": {},
            }),
        ),
        (
            "every member at once",
            json!({
                "contentLocation": "s3://bucket/final.name_v1.json",
                "description": "",
                "capabilities": ["one"],
                "sceneMetadata": {"a": "", "b": "2", "c": "3"},
            }),
            json!({
                "contentLocation": "s3://bucket/final.name_v1.json",
                "description": "",
                "capabilities": ["one"],
                "sceneMetadata": {"a": "", "b": "2", "c": "3"},
            }),
        ),
        (
            "no member at all",
            json!({}),
            json!({
                "contentLocation": "s3://bucket/final.name_v1.json",
                "description": "",
                "capabilities": ["one"],
                "sceneMetadata": {"a": "", "b": "2", "c": "3"},
            }),
        ),
    ];

    for (case, patch, expected) in &rows {
        let before = emulator.get_scene(WORKSPACE, SCENE).await;
        let updated = emulator.update_scene(WORKSPACE, SCENE, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateScene, got {updated:?}"
        );

        let read = emulator.get_scene(WORKSPACE, SCENE).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetScene, got {read:?}"
        );
        for member in PATCHABLE {
            assert_eq!(
                reported(&read, member),
                expected[member],
                "case {case:?}: expected {member} to be {:?}, got {read:?}",
                expected[member]
            );
        }
        // The instant the update answers is the scene's own `updateDateTime`,
        // so the two cannot disagree; it moved forward, while the creation
        // instant the scene was minted with did not.
        assert_eq!(
            reported(&read, "updateDateTime"),
            reported(&updated, "updateDateTime"),
            "case {case:?}: the read must report the instant the update answered, got {read:?}"
        );
        assert_eq!(
            reported(&read, "creationDateTime"),
            reported(&before, "creationDateTime"),
            "case {case:?}: an update must not move the creation instant, got {read:?}"
        );
        let (was, now) = (
            reported(&before, "updateDateTime")
                .as_f64()
                .unwrap_or_default(),
            reported(&read, "updateDateTime").as_f64().unwrap_or_default(),
        );
        assert!(
            now >= was,
            "case {case:?}: expected the update instant to move forward from {was}, got {now}"
        );
        // The scene's identity is not patchable: no member of the request
        // reaches the ids or the ARN.
        assert_eq!(
            (
                reported(&read, "workspaceId"),
                reported(&read, "sceneId"),
                reported(&read, "arn"),
            ),
            (
                json!(WORKSPACE),
                json!(SCENE),
                json!(scene_arn(WORKSPACE, SCENE)),
            ),
            "case {case:?}: an update must not move the ids or the ARN, got {read:?}"
        );
    }
}

/// RULE: both of an update's labels address an EXISTING resource, so a request
/// naming a workspace or a scene this service does not hold is refused 404
/// `ResourceNotFoundException` and persists nothing.
#[tokio::test]
async fn a_scene_update_the_service_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the scene is created in must exist"
    );
    assert_eq!(
        emulator.create_scene(WORKSPACE, created()).await.status,
        200,
        "the scene the refusals must leave alone has to exist"
    );
    assert_eq!(
        emulator.create_workspace("otherPatchedScenes").await.status,
        200,
        "the second workspace, which holds no scene of this id, must exist"
    );

    // The rows differ in WHICH label misses and in how much the patch carried.
    for (case, workspace_id, scene_id, patch) in [
        (
            "absent workspace",
            "absentWorkspace",
            SCENE,
            json!({"description": "nowhere to land"}),
        ),
        (
            "absent scene",
            WORKSPACE,
            "absentScene",
            json!({"contentLocation": "s3://bucket/absent.json"}),
        ),
        (
            // A scene is addressed inside ITS OWN workspace, so the id does not
            // resolve through another workspace that holds no such scene.
            "foreign workspace",
            "otherPatchedScenes",
            SCENE,
            json!({
                "contentLocation": "s3://bucket/foreign.json",
                "description": "from another workspace",
                "capabilities": ["foreign"],
                "sceneMetadata": {"floor": "0"},
            }),
        ),
    ] {
        let refused = emulator.update_scene(workspace_id, scene_id, patch).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(scene_id)
                    || message.contains(workspace_id)),
            "case {case:?}: the refusal must name what was missing, got {refused:?}"
        );

        let read = emulator.get_scene(WORKSPACE, SCENE).await;
        for (member, value) in [
            ("contentLocation", json!("s3://bucket/created.json")),
            ("description", json!("the created description")),
            ("capabilities", json!(["zeta", "alpha"])),
            (
                "sceneMetadata",
                json!({"floor": "3", "building_id": "B-12"}),
            ),
        ] {
            assert_eq!(
                reported(&read, member),
                value,
                "case {case:?}: a refused update must leave {member} as created, got {read:?}"
            );
        }
    }
}

/// RULE: an update is scoped to the Region it is signed for, exactly as the read
/// is — the same ids in another Region name another scene, so an update
/// addressed there finds none.
#[tokio::test]
async fn a_scene_update_only_reaches_the_region_it_is_signed_for() {
    let emulator = Emulator::new();
    assert_eq!(emulator.create_workspace(WORKSPACE).await.status, 200);
    assert_eq!(
        emulator.create_scene(WORKSPACE, created()).await.status,
        200
    );

    let elsewhere = emulator
        .call_signed_for(
            Some("eu-central-1"),
            "PUT",
            &format!("/workspaces/{WORKSPACE}/scenes/{SCENE}"),
            Some(json!({"description": "from another Region"})),
        )
        .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "another Region holds no such workspace, got {elsewhere:?}"
    );
    let read = emulator.get_scene(WORKSPACE, SCENE).await;
    assert_eq!(
        reported(&read, "description"),
        json!("the created description"),
        "the refused update must have changed nothing, got {read:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! The workspace -> scene lifecycle, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: what a `CreateScene` persists is exactly what `GetScene` reports,
//! whatever mix of optional members the create carried; a scene is minted under
//! the ARN its workspace nests it in; a taken id conflicts and leaves the
//! stored record alone; a request naming something the service does not hold is
//! refused with the modelled error; and a scene is part of its workspace's
//! contents, tags included.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{scene_arn, Emulator};

/// The `capabilities` a read reports, as a set of strings — used where only
/// membership matters; order is asserted separately against the created list.
fn reported_capabilities(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

/// The table every round-trip row is built from: `(sceneId, request)`.
///
/// The rows differ along what the rule generalises over — which optional
/// members the create carries, the number and ORDER of the `capabilities` list,
/// the size of the `sceneMetadata` map, the shape of the `sceneId`, and the
/// form of the `contentLocation` (the `S3Url` pattern
/// `^[sS]3://[A-Za-z0-9._/-]+$` admits either case of the scheme and a nested
/// key) — never along a mere rename.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("bare", json!({"contentLocation": "s3://bucket/bare.json"})),
        (
            "described",
            json!({
                "contentLocation": "s3://bucket/nested/path/scene-1.json",
                "description": "a described scene",
            }),
        ),
        (
            "emptyDescription",
            json!({"contentLocation": "s3://bucket/empty.json", "description": ""}),
        ),
        (
            "oneCapability",
            json!({
                "contentLocation": "S3://bucket/one.json",
                "capabilities": ["MATTERPORT"],
            }),
        ),
        (
            "orderedCapabilities",
            json!({
                "contentLocation": "s3://bucket/ordered.json",
                // Deliberately NOT sorted: `SceneCapabilities` is a list, so
                // the order the create gave is part of the value.
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

/// RULE: a scene read back reports exactly the members its create persisted —
/// the ones the create named, and the resolved value of the ones it did not —
/// under the ARN the create minted, with `updateDateTime` at the creation
/// instant until something updates it.
#[tokio::test]
async fn a_scene_reads_back_exactly_what_was_created() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("sceneWorkspace").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    for (scene_id, request) in rows() {
        let mut body = request.clone();
        body["sceneId"] = json!(scene_id);
        let created = emulator.create_scene("sceneWorkspace", body).await;
        assert_eq!(
            created.status, 200,
            "case {scene_id:?}: expected 200 from CreateScene, got {created:?}"
        );
        assert_eq!(
            created.member("arn").as_str(),
            Some(scene_arn("sceneWorkspace", scene_id).as_str()),
            "case {scene_id:?}: expected the minted ARN, got {created:?}"
        );
        let creation_date_time = created.member("creationDateTime").as_f64();
        assert!(
            creation_date_time.is_some_and(|seconds| seconds > 0.0),
            "case {scene_id:?}: expected an epoch-seconds creationDateTime, got {created:?}"
        );

        let read = emulator.get_scene("sceneWorkspace", scene_id).await;
        assert_eq!(
            read.status, 200,
            "case {scene_id:?}: expected 200 from GetScene, got {read:?}"
        );
        assert_eq!(
            (
                read.member("workspaceId").as_str(),
                read.member("sceneId").as_str(),
                read.member("arn").as_str(),
            ),
            (
                Some("sceneWorkspace"),
                Some(scene_id),
                Some(scene_arn("sceneWorkspace", scene_id).as_str()),
            ),
            "case {scene_id:?}: expected the created identity, got {read:?}"
        );
        assert_eq!(
            read.member("contentLocation").as_str(),
            request["contentLocation"].as_str(),
            "case {scene_id:?}: expected the created contentLocation, got {read:?}"
        );
        assert_eq!(
            read.member("description").as_str(),
            request["description"].as_str(),
            "case {scene_id:?}: expected the created description, got {read:?}"
        );
        assert_eq!(
            read.member("creationDateTime").as_f64(),
            creation_date_time,
            "case {scene_id:?}: the read must report the create's own instant, got {read:?}"
        );
        assert_eq!(
            read.member("updateDateTime").as_f64(),
            creation_date_time,
            "case {scene_id:?}: nothing updated it, so updateDateTime is the creation \
             instant, got {read:?}"
        );

        // A collection the create did not name is reported EMPTY rather than
        // absent, and one it did name is reported verbatim — order included,
        // because `capabilities` is a list.
        let created_capabilities = request["capabilities"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            read.member("capabilities"),
            &json!(created_capabilities),
            "case {scene_id:?}: expected the created capabilities in order, got {read:?}"
        );
        assert_eq!(
            reported_capabilities(read.member("capabilities")),
            reported_capabilities(&json!(created_capabilities)),
            "case {scene_id:?}: expected the created capability set, got {read:?}"
        );
        assert_eq!(
            read.member("sceneMetadata"),
            &json!(request["sceneMetadata"]
                .as_object()
                .cloned()
                .unwrap_or_default()),
            "case {scene_id:?}: expected the created sceneMetadata, got {read:?}"
        );
        // `generatedSceneMetadata` is metadata the SERVICE derives from the
        // scene's content; this emulator reaches no S3 object, so it derives
        // none and reports the empty map rather than inventing entries.
        assert_eq!(
            read.member("generatedSceneMetadata"),
            &json!({}),
            "case {scene_id:?}: expected no generated metadata, got {read:?}"
        );
        // The only code the model's `SceneErrorCode` admits is
        // `MATTERPORT_ERROR`, which belongs to an import this emulator never
        // performs, so a stored scene carries no error.
        assert!(
            read.member("error").is_null(),
            "case {scene_id:?}: a scene that failed at nothing reports no error, got {read:?}"
        );
    }
}

/// RULE: a scene's `sceneId` is the caller's and unique inside its workspace,
/// so re-creating a taken id is refused with HTTP 409 `ConflictException` and
/// the stored scene is left exactly as the first create wrote it.
#[tokio::test]
async fn re_creating_a_taken_scene_id_conflicts_and_changes_nothing() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("conflictScenes").await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    assert_eq!(
        emulator
            .create_scene(
                "conflictScenes",
                json!({
                    "sceneId": "taken",
                    "contentLocation": "s3://bucket/first.json",
                    "description": "the first one",
                    "capabilities": ["first"],
                }),
            )
            .await
            .status,
        200,
        "the scene every row re-creates must exist first"
    );

    // The rows differ in what the second create would have written.
    let rows: Vec<(&str, Value)> = vec![
        (
            "same content",
            json!({"sceneId": "taken", "contentLocation": "s3://bucket/first.json"}),
        ),
        (
            "other content",
            json!({"sceneId": "taken", "contentLocation": "s3://bucket/second.json"}),
        ),
        (
            "other members",
            json!({
                "sceneId": "taken",
                "contentLocation": "s3://bucket/second.json",
                "description": "the second one",
                "capabilities": ["second", "third"],
                "sceneMetadata": {"floor": "9"},
            }),
        ),
    ];

    for (case, request) in &rows {
        let again = emulator.create_scene("conflictScenes", request.clone()).await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: re-creating a taken id must conflict, got {again:?}"
        );
        let read = emulator.get_scene("conflictScenes", "taken").await;
        assert_eq!(
            (
                read.member("contentLocation").as_str(),
                read.member("description").as_str(),
            ),
            (Some("s3://bucket/first.json"), Some("the first one")),
            "case {case:?}: a refused create must not overwrite the stored scene, got {read:?}"
        );
        assert_eq!(
            read.member("capabilities"),
            &json!(["first"]),
            "case {case:?}: a refused create must persist nothing, got {read:?}"
        );
        assert_eq!(
            read.member("sceneMetadata"),
            &json!({}),
            "case {case:?}: a refused create must persist nothing, got {read:?}"
        );
    }
}

/// RULE: a scene request that names something this service does not hold is
/// refused with the modelled error for that refusal — a create into an absent
/// workspace with the `ValidationException` the operation models (it declares
/// no `ResourceNotFoundException`), a read of an absent workspace or scene with
/// HTTP 404 `ResourceNotFoundException` — and the refusal says why.
#[tokio::test]
async fn a_scene_request_the_service_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("refusalScenes").await.status,
        200,
        "the workspace the cases are created in must exist"
    );
    assert_eq!(
        emulator
            .create_scene(
                "refusalScenes",
                json!({"sceneId": "present", "contentLocation": "s3://bucket/present.json"}),
            )
            .await
            .status,
        200,
        "the scene the found case reads must exist"
    );

    // A create into a workspace that does not exist, over rows that differ in
    // how much the create carried: none of them may be stored anywhere.
    for (case, request) in [
        (
            "bare into absent workspace",
            json!({"sceneId": "orphan", "contentLocation": "s3://bucket/orphan.json"}),
        ),
        (
            "full into absent workspace",
            json!({
                "sceneId": "orphan",
                "contentLocation": "s3://bucket/orphan.json",
                "description": "nowhere to live",
                "capabilities": ["a"],
                "tags": {"t": "v"},
            }),
        ),
    ] {
        let refused = emulator.create_scene("absentWorkspace", request).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains("absentWorkspace")),
            "case {case:?}: the refusal must name what was missing, got {refused:?}"
        );
    }

    // A read of either resource this service does not hold is a 404.
    for (case, workspace_id, scene_id) in [
        ("absent workspace", "absentWorkspace", "present"),
        ("absent scene", "refusalScenes", "absentScene"),
    ] {
        let read = emulator.get_scene(workspace_id, scene_id).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404, got {read:?}"
        );
        assert!(
            read.member("message").is_string(),
            "case {case:?}: a refusal must say why, got {read:?}"
        );
    }

    // A scene is addressed inside ITS OWN workspace: the id of a scene created
    // in another workspace does not resolve here.
    assert_eq!(
        emulator.create_workspace("otherScenes").await.status,
        200,
        "the second workspace must exist"
    );
    let foreign = emulator.get_scene("otherScenes", "present").await;
    assert_eq!(
        (foreign.status, foreign.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"foreign workspace\": a scene is scoped to its workspace, got {foreign:?}"
    );
}

/// RULE: a scene is part of its workspace's contents — the tags its create
/// carried are the tag set of its own ARN, and a workspace that still holds
/// scenes is not deletable.
#[tokio::test]
async fn a_scene_belongs_to_its_workspace() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace("sceneContents").await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The rows differ in how many tags the create carried, and in how many
    // scenes the workspace holds by the time the delete is attempted.
    let rows: Vec<(&str, Value)> = vec![
        ("held1", json!({})),
        ("held2", json!({"only": "tag"})),
        ("held3", json!({"team": "plant", "env": "test"})),
    ];

    for (count, (scene_id, tags)) in rows.iter().enumerate() {
        let count = count + 1;
        let created = emulator
            .create_scene(
                "sceneContents",
                json!({
                    "sceneId": scene_id,
                    "contentLocation": format!("s3://bucket/{scene_id}.json"),
                    "tags": tags,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {scene_id:?}: expected 200 from CreateScene, got {created:?}"
        );

        // The create's tags are reachable under the scene's own ARN, through
        // the one tagging surface every resource of this service shares.
        let listed = emulator
            .list_tags_for_resource(&scene_arn("sceneContents", scene_id), json!({}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {scene_id:?}: a scene ARN must be taggable, got {listed:?}"
        );
        assert_eq!(
            listed.member("tags"),
            tags,
            "case {scene_id:?}: expected the created tags, got {listed:?}"
        );

        let refused = emulator.delete_workspace("sceneContents").await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {scene_id:?}: a workspace holding scenes must not be deletable, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(&count.to_string())),
            "case {scene_id:?}: the refusal must count what the workspace still holds, \
             got {refused:?}"
        );

        for (held, _) in rows.iter().take(count) {
            let read = emulator.get_scene("sceneContents", held).await;
            assert_eq!(
                read.status, 200,
                "case {scene_id:?}: scene {held:?} must still be readable, got {read:?}"
            );
        }
    }
}

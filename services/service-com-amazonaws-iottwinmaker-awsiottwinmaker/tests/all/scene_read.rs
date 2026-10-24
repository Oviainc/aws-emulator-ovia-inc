// SPDX-License-Identifier: CC0-1.0

//! What `GetScene` REPORTS, and what reading a scene must never change.
//!
//! `scene_lifecycle` pins the create -> read round trip for a table of creates.
//! This suite pins the two rules that outlive any single create:
//!
//! * every member `GetScene` reports satisfies the constraint the model puts on
//!   it — the `Id`, `S3Url` and `TwinMakerArn` patterns and lengths, the 0..50
//!   bounds on the collections — whatever the create carried, including at each
//!   published bound; a create one past a bound is refused and nothing is
//!   stored for it;
//! * `GetScene` is a REPORTING operation: reading a scene (or being refused a
//!   read) changes nothing a later caller sees — not the scene, not its
//!   siblings, not the workspace's contents and not the store's key space.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`), so
//! the route, the status code and the serialized body are all exercised.

use regex::Regex;
use serde_json::{json, Value};

use crate::harness::{scene_arn, Emulator};

/// The modelled constraints this suite asserts against, copied from the model:
/// `smithy_query(query="shape", shape="Id,S3Url,TwinMakerArn,SceneCapabilities,
/// SceneMetadataMap,Description")`.
const ID_PATTERN: &str = r"^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$";
const ID_MAX: usize = 128;
const S3_URL_PATTERN: &str = r"^[sS]3://[A-Za-z0-9._/-]+$";
const S3_URL_MAX: usize = 256;
const TWINMAKER_ARN_PATTERN: &str =
    r"^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$";
const TWINMAKER_ARN_MIN: usize = 20;
const TWINMAKER_ARN_MAX: usize = 2048;
/// `SceneMetadataMap`/`GeneratedSceneMetadataMap` key target (`Name`).
const NAME_PATTERN: &str = r"^[a-zA-Z_\-0-9]+$";
/// The 0..50 bound `SceneCapabilities`, `SceneMetadataMap` and
/// `GeneratedSceneMetadataMap` all carry.
const COLLECTION_MAX: usize = 50;
/// `SceneCapability` is `@length(0..256)`.
const CAPABILITY_MAX: usize = 256;
/// `Description` and `SceneMetadataValue` are both `@length(0..2048)`.
const TEXT_MAX: usize = 2048;

/// Whether `value` matches a modelled pattern, compiled per call: these tables
/// are small and a pattern is cheap next to a dispatched request.
fn matches(pattern: &str, value: &str) -> bool {
    Regex::new(pattern)
        .unwrap_or_else(|error| panic!("the modelled pattern {pattern:?} must compile: {error}"))
        .is_match(value)
}

/// Assert a reported string is within its modelled pattern and length, counting
/// characters (Smithy's `@length` on a string counts characters, not bytes).
fn assert_string(case: &str, member: &str, value: &Value, pattern: &str, min: usize, max: usize) {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a string, got {value:?}"));
    let length = text.chars().count();
    assert!(
        length >= min && length <= max,
        "case {case:?}: {member} must be {min}..{max} characters, got {length} in {value:?}"
    );
    assert!(
        matches(pattern, text),
        "case {case:?}: {member} must match {pattern}, got {value:?}"
    );
}

/// A string of `length` ASCII characters built from `fill`.
fn filled(fill: char, length: usize) -> String {
    std::iter::repeat_n(fill, length).collect()
}

/// A `contentLocation` of exactly `length` characters (`length >= 7`), built so
/// it stays inside the `S3Url` pattern.
fn content_location_of(length: usize) -> String {
    format!("s3://b/{}", filled('a', length - "s3://b/".len()))
}

/// A `capabilities` list of `count` distinct entries.
fn capabilities_of(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("cap{index}")).collect()
}

/// A `sceneMetadata` map of `count` entries, keyed inside the `Name` pattern.
fn metadata_of(count: usize) -> serde_json::Map<String, Value> {
    (0..count)
        .map(|index| (format!("k{index}"), json!(format!("v{index}"))))
        .collect()
}

/// The reporting table: `(case, sceneId, create request without `sceneId`)`.
///
/// The rows differ along what the rule generalises over — the shape and LENGTH
/// of the `sceneId` (the two characters the `Id` pattern needs at minimum, and
/// the member's published 128-character maximum), the form and length of the
/// `contentLocation` (either case of the `s3://` scheme, a nested key, the
/// `S3Url` maximum of 256), the COUNT of the collections (none, one, the
/// modelled 50) and the LENGTH of their entries (a 256-character capability, a
/// 2048-character metadata value), and the description (absent, empty, 2048).
fn reporting_rows() -> Vec<(&'static str, String, Value)> {
    let mut rows: Vec<(&'static str, String, Value)> = vec![
        (
            "shortestId",
            "ab".to_string(),
            json!({"contentLocation": "s3://b"}),
        ),
        (
            "idAtMaxLength",
            format!("{}z", filled('a', ID_MAX - 1)),
            json!({"contentLocation": "S3://bucket/UPPER-scheme.json"}),
        ),
        (
            "underscoredId",
            "0_scene-9".to_string(),
            json!({
                "contentLocation": "s3://bucket/nested/deep/key.name_v1.json",
                "description": "",
            }),
        ),
        (
            "contentLocationAtMaxLength",
            "maxLocation".to_string(),
            json!({"contentLocation": content_location_of(S3_URL_MAX)}),
        ),
        (
            "oneCapabilityOneMetadataEntry",
            "oneEach".to_string(),
            json!({
                "contentLocation": "s3://bucket/one.json",
                "capabilities": capabilities_of(1),
                "sceneMetadata": metadata_of(1),
            }),
        ),
        (
            "collectionsAtMaxCount",
            "maxCollections".to_string(),
            json!({
                "contentLocation": "s3://bucket/max.json",
                "capabilities": capabilities_of(COLLECTION_MAX),
                "sceneMetadata": metadata_of(COLLECTION_MAX),
            }),
        ),
        (
            "entriesAtMaxLength",
            "maxEntries".to_string(),
            json!({
                "contentLocation": "s3://bucket/long.json",
                "description": filled('d', TEXT_MAX),
                "capabilities": [filled('c', CAPABILITY_MAX)],
                "sceneMetadata": {"k": filled('v', TEXT_MAX)},
            }),
        ),
    ];
    rows.push((
        "multiByteText",
        "multiByte".to_string(),
        json!({
            "contentLocation": "s3://bucket/multi.json",
            "description": "Kühlpumpe — 冷却ポンプ",
            "capabilities": ["Kühlung"],
            "sceneMetadata": {"floor": "３"},
        }),
    ));
    rows
}

/// RULE: every member `GetScene` reports is within the constraint the model puts
/// on it — pattern and length for the strings, the 0..50 bound for the
/// collections — and reports exactly what the create persisted, whatever the
/// create carried.
#[tokio::test]
async fn get_scene_reports_members_within_their_modelled_constraints() {
    let emulator = Emulator::new();
    let workspace_id = "sceneReports";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    for (case, scene_id, request) in reporting_rows() {
        let mut body = request.clone();
        body["sceneId"] = json!(scene_id);
        let created = emulator.create_scene(workspace_id, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateScene, got {created:?}"
        );

        let read = emulator.get_scene(workspace_id, &scene_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetScene, got {read:?}"
        );

        // Every member the model marks required is present.
        for member in [
            "arn",
            "workspaceId",
            "sceneId",
            "contentLocation",
            "creationDateTime",
            "updateDateTime",
        ] {
            assert!(
                read.body.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: the required member {member} must be reported, got {read:?}"
            );
        }

        assert_string(
            case,
            "workspaceId",
            read.member("workspaceId"),
            ID_PATTERN,
            1,
            ID_MAX,
        );
        assert_string(case, "sceneId", read.member("sceneId"), ID_PATTERN, 1, ID_MAX);
        assert_string(
            case,
            "contentLocation",
            read.member("contentLocation"),
            S3_URL_PATTERN,
            0,
            S3_URL_MAX,
        );
        assert_string(
            case,
            "arn",
            read.member("arn"),
            TWINMAKER_ARN_PATTERN,
            TWINMAKER_ARN_MIN,
            TWINMAKER_ARN_MAX,
        );
        assert_eq!(
            read.member("arn").as_str(),
            Some(scene_arn(workspace_id, &scene_id).as_str()),
            "case {case:?}: expected the ARN its workspace nests it under, got {read:?}"
        );
        assert_eq!(
            (
                read.member("workspaceId").as_str(),
                read.member("sceneId").as_str(),
            ),
            (Some(workspace_id), Some(scene_id.as_str())),
            "case {case:?}: expected the created identity, got {read:?}"
        );
        assert_eq!(
            read.member("contentLocation").as_str(),
            request["contentLocation"].as_str(),
            "case {case:?}: expected the created contentLocation, got {read:?}"
        );

        // A reported `description` stays inside `Description`'s 0..2048.
        if let Some(description) = read.body.get("description").and_then(Value::as_str) {
            assert!(
                description.chars().count() <= TEXT_MAX,
                "case {case:?}: description must be at most {TEXT_MAX} characters, got {} in \
                 {read:?}",
                description.chars().count()
            );
        }
        assert_eq!(
            read.member("description").as_str(),
            request["description"].as_str(),
            "case {case:?}: expected the created description, got {read:?}"
        );

        // `capabilities` is a LIST, so its order is part of the value, and each
        // entry stays inside `SceneCapability`'s 0..256.
        let capabilities = read
            .member("capabilities")
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: capabilities must be a list, got {read:?}"));
        assert!(
            capabilities.len() <= COLLECTION_MAX,
            "case {case:?}: capabilities must hold at most {COLLECTION_MAX} entries, got {}",
            capabilities.len()
        );
        for entry in capabilities {
            let entry = entry.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a capability must be a string, got {entry:?}")
            });
            assert!(
                entry.chars().count() <= CAPABILITY_MAX,
                "case {case:?}: a capability must be at most {CAPABILITY_MAX} characters, got {} \
                 in {entry:?}",
                entry.chars().count()
            );
        }
        assert_eq!(
            read.member("capabilities"),
            &json!(request["capabilities"].as_array().cloned().unwrap_or_default()),
            "case {case:?}: expected the created capabilities in order, got {read:?}"
        );

        // Both metadata maps stay inside the 0..50 bound, with keys inside
        // `Name`'s pattern; `generatedSceneMetadata` is derived by the SERVICE
        // from content this emulator never reads, so it derives none.
        for member in ["sceneMetadata", "generatedSceneMetadata"] {
            let map = read.body[member].as_object().unwrap_or_else(|| {
                panic!("case {case:?}: {member} must be a map, got {read:?}")
            });
            assert!(
                map.len() <= COLLECTION_MAX,
                "case {case:?}: {member} must hold at most {COLLECTION_MAX} entries, got {}",
                map.len()
            );
            for (key, value) in map {
                assert!(
                    matches(NAME_PATTERN, key),
                    "case {case:?}: {member} key {key:?} must match {NAME_PATTERN}"
                );
                let value = value.as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: {member}[{key:?}] must be a string, got {value:?}")
                });
                assert!(
                    value.chars().count() <= TEXT_MAX,
                    "case {case:?}: {member}[{key:?}] must be at most {TEXT_MAX} characters, got {}",
                    value.chars().count()
                );
            }
        }
        assert_eq!(
            read.member("sceneMetadata"),
            &json!(request["sceneMetadata"].as_object().cloned().unwrap_or_default()),
            "case {case:?}: expected the created sceneMetadata, got {read:?}"
        );
        assert_eq!(
            read.member("generatedSceneMetadata"),
            &json!({}),
            "case {case:?}: this emulator derives no scene metadata, got {read:?}"
        );

        // Both timestamps are required, so both are reported, as epoch seconds.
        for member in ["creationDateTime", "updateDateTime"] {
            assert!(
                read.body[member].as_f64().is_some_and(|seconds| seconds > 0.0),
                "case {case:?}: {member} must be reported as epoch seconds, got {read:?}"
            );
        }
        assert!(
            read.member("error").is_null(),
            "case {case:?}: a scene that failed at nothing reports no error, got {read:?}"
        );
    }
}

/// RULE: a create whose member falls outside a modelled constraint is refused
/// with HTTP 400 `ValidationException`, and nothing is stored for it — so the
/// read of the id it named is still the modelled 404.
///
/// The rows take each constrained member one past its published bound, or
/// outside its published pattern.
#[tokio::test]
async fn a_scene_member_outside_its_modelled_constraint_is_refused() {
    let emulator = Emulator::new();
    let workspace_id = "sceneBounds";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    let past_bound: Vec<(&str, String, Value)> = vec![
        (
            "contentLocationPastMaxLength",
            "pastLocation".to_string(),
            json!({"contentLocation": content_location_of(S3_URL_MAX + 1)}),
        ),
        (
            "contentLocationOutsidePattern",
            "badLocation".to_string(),
            json!({"contentLocation": "https://bucket/not-s3.json"}),
        ),
        (
            "descriptionPastMaxLength",
            "pastDescription".to_string(),
            json!({
                "contentLocation": "s3://bucket/past.json",
                "description": filled('d', TEXT_MAX + 1),
            }),
        ),
        (
            "capabilityPastMaxLength",
            "pastCapability".to_string(),
            json!({
                "contentLocation": "s3://bucket/past.json",
                "capabilities": [filled('c', CAPABILITY_MAX + 1)],
            }),
        ),
        (
            "capabilitiesPastMaxCount",
            "pastCapabilities".to_string(),
            json!({
                "contentLocation": "s3://bucket/past.json",
                "capabilities": capabilities_of(COLLECTION_MAX + 1),
            }),
        ),
        (
            "sceneMetadataPastMaxCount",
            "pastMetadata".to_string(),
            json!({
                "contentLocation": "s3://bucket/past.json",
                "sceneMetadata": metadata_of(COLLECTION_MAX + 1),
            }),
        ),
        (
            "sceneIdPastMaxLength",
            filled('a', ID_MAX + 1),
            json!({"contentLocation": "s3://bucket/past.json"}),
        ),
        (
            "sceneIdOutsidePattern",
            "-leadingDash".to_string(),
            json!({"contentLocation": "s3://bucket/past.json"}),
        ),
    ];

    for (case, scene_id, request) in past_bound {
        let mut body = request.clone();
        body["sceneId"] = json!(scene_id);
        let refused = emulator.create_scene(workspace_id, body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a member outside its modelled constraint must be refused, got \
             {refused:?}"
        );

        // Nothing was stored for it. A read of a WELL-FORMED id the store does
        // not hold is the modelled 404; an id outside `Id` itself never reaches
        // the lookup, so it is refused by the same constraint as the create.
        let id_is_well_formed = scene_id.chars().count() <= ID_MAX && matches(ID_PATTERN, &scene_id);
        let read = emulator.get_scene(workspace_id, &scene_id).await;
        let expected = if id_is_well_formed {
            (404, Some("ResourceNotFoundException"))
        } else {
            (400, Some("ValidationException"))
        };
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            expected,
            "case {case:?}: a refused create must leave nothing to read, got {read:?}"
        );
        assert!(
            !emulator
                .state()
                .lock()
                .workspaces
                .get(workspace_id)
                .is_some_and(|workspace| workspace.scenes.contains_key(&scene_id)),
            "case {case:?}: a refused create must persist no scene"
        );
    }
}

/// RULE: `GetScene` REPORTS — a read, or a refused read, changes nothing a later
/// caller sees: the scene reads back identically however often it is read, its
/// siblings are untouched, the workspace's contents are unchanged, and a read of
/// something the service does not hold materialises nothing.
#[tokio::test]
async fn reading_a_scene_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let workspace_id = "sceneReads";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "the workspace the cases are created in must exist"
    );

    // The rows differ in how much the create carried, so purity is asserted for
    // a bare scene and for one holding every optional member.
    let rows: Vec<(&str, Value)> = vec![
        ("bare", json!({"contentLocation": "s3://bucket/bare.json"})),
        (
            "described",
            json!({
                "contentLocation": "s3://bucket/described.json",
                "description": "a described scene",
            }),
        ),
        (
            "full",
            json!({
                "contentLocation": "S3://bucket/full.json",
                "description": "every optional member at once",
                "capabilities": ["zeta", "alpha"],
                "sceneMetadata": {"floor": "3"},
                "tags": {"team": "plant"},
            }),
        ),
    ];

    let mut first_reads: Vec<(&str, Value)> = Vec::new();
    for (scene_id, request) in &rows {
        let mut body = request.clone();
        body["sceneId"] = json!(scene_id);
        let created = emulator.create_scene(workspace_id, body).await;
        assert_eq!(
            created.status, 200,
            "case {scene_id:?}: expected 200 from CreateScene, got {created:?}"
        );
        let read = emulator.get_scene(workspace_id, scene_id).await;
        assert_eq!(
            read.status, 200,
            "case {scene_id:?}: expected 200 from GetScene, got {read:?}"
        );
        first_reads.push((scene_id, read.body));
    }

    let scenes_before = emulator
        .state()
        .lock()
        .workspaces
        .get(workspace_id)
        .map(|workspace| workspace.scenes.len());

    // Read everything repeatedly, interleaved, including reads the service must
    // refuse: not one of them may change a later read.
    for round in 1..=3u32 {
        for (scene_id, first) in &first_reads {
            let again = emulator.get_scene(workspace_id, scene_id).await;
            assert_eq!(
                (again.status, &again.body),
                (200, first),
                "case {scene_id:?} (round {round}): a read must report exactly what the previous \
                 read did, got {again:?}"
            );
        }
        for (case, workspace, scene) in [
            ("absent scene", workspace_id, "absentScene"),
            ("absent workspace", "absentWorkspace", "bare"),
        ] {
            let refused = emulator.get_scene(workspace, scene).await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} (round {round}): expected the modelled 404, got {refused:?}"
            );
        }
    }

    // Nothing the reads touched was created, moved or counted. The guard lives
    // in its own scope, so it is never held across one of the awaits below.
    {
        let state = emulator.state();
        let state = state.lock();
        assert_eq!(
            state
                .workspaces
                .get(workspace_id)
                .map(|workspace| workspace.scenes.len()),
            scenes_before,
            "reads must not change how many scenes the workspace holds"
        );
        assert!(
            !state.workspaces.contains_key("absentWorkspace"),
            "a refused read must not materialise the workspace it named"
        );
        assert!(
            state
                .workspaces
                .get(workspace_id)
                .is_some_and(|workspace| !workspace.scenes.contains_key("absentScene")),
            "a refused read must not materialise the scene it named"
        );
    }

    // The tags a scene's create carried are still exactly its tag set, and the
    // scene the reads refused is still creatable — proof the 404 reserved
    // nothing.
    let listed = emulator
        .list_tags_for_resource(&scene_arn(workspace_id, "full"), json!({}))
        .await;
    assert_eq!(
        listed.member("tags"),
        &json!({"team": "plant"}),
        "reads must not change a scene's tag set, got {listed:?}"
    );
    let created_after = emulator
        .create_scene(
            workspace_id,
            json!({"sceneId": "absentScene", "contentLocation": "s3://bucket/late.json"}),
        )
        .await;
    assert_eq!(
        created_after.status, 200,
        "an id a read was refused for must still be creatable, got {created_after:?}"
    );
    let late = emulator.get_scene(workspace_id, "absentScene").await;
    assert_eq!(
        (late.status, late.member("contentLocation").as_str()),
        (200, Some("s3://bucket/late.json")),
        "the late create must read back, got {late:?}"
    );
}

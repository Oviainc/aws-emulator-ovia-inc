// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateWorkspace` may and may not reach, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! `tests/all/workspace_update.rs` pins the patch itself (the members a request
//! names replace what the workspace held, the ones it leaves out keep their
//! stored values). This suite pins the two rules AROUND that patch:
//!
//! 1. The update writes ONE stored workspace, so every reader of that workspace
//!    reports the patched values and one `updateDateTime` — the dedicated read,
//!    the read addressed by ARN and the listing alike — and nothing the workspace
//!    CONTAINS (its tags, entities, component types, scenes, sync jobs) is
//!    touched, because no member of the request names any of them.
//! 2. A value outside the modelled bound of the member it is sent as is refused
//!    at the boundary with HTTP 400 `ValidationException` BEFORE the workspace is
//!    looked up, and the stored workspace is left exactly as it was; a value AT a
//!    published bound is accepted and stored.

use serde_json::{json, Value};

use crate::harness::{seed_sync_job, workspace_arn, Emulator, Resp};

/// The workspace the readers and the refusals are asserted against.
const WORKSPACE: &str = "scopedWorkspace";

/// The prefix a `RoleArn` must carry: the `role` member's modelled pattern
/// (`^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/`) matched with this
/// Region's emulated account, so padding it stays a legal role ARN.
const ROLE_PREFIX: &str = "arn:aws:iam::000000000000:role/";

/// The prefix an `S3Location` must carry: the `s3Location` member's modelled
/// pattern (`(^arn:((aws)|(aws-cn)|(aws-us-gov)):s3:::)([a-zA-Z0-9_-]+$)`).
const S3_PREFIX: &str = "arn:aws:s3:::";

/// What the workspace is created with, so each case has something to leave
/// alone.
fn created() -> Value {
    json!({
        "description": "the created description",
        "role": "arn:aws:iam::000000000000:role/TwinMakerWorkspaceRole",
        "s3Location": "arn:aws:s3:::twinmaker-scoped-bucket",
        "tags": {"team": "controls", "tier": "gold"},
    })
}

/// The `message` a refusal carries.
fn message(resp: &Resp) -> String {
    resp.member("message")
        .as_str()
        .unwrap_or_else(|| panic!("a refusal carries a message, got {resp:?}"))
        .to_string()
}

/// The summary `ListWorkspaces` reports for `workspace_id`.
async fn summary(emulator: &Emulator, workspace_id: &str) -> Value {
    let listed = emulator.list_workspaces(json!({})).await;
    assert_eq!(
        listed.status, 200,
        "ListWorkspaces must serve the page the summary is read from, got {listed:?}"
    );
    listed
        .member("workspaceSummaries")
        .as_array()
        .unwrap_or_else(|| panic!("the listing reports an array of summaries, got {listed:?}"))
        .iter()
        .find(|row| row["workspaceId"] == json!(workspace_id))
        .unwrap_or_else(|| panic!("the listing must hold {workspace_id:?}, got {listed:?}"))
        .clone()
}

/// RULE: the update writes one stored workspace, so every reader of it reports
/// the patched values and the single instant the update answered.
#[tokio::test]
async fn every_reader_of_the_workspace_reports_the_patch_and_one_instant() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace_with(WORKSPACE, created()).await.status,
        200,
        "the workspace the patches apply to must be created"
    );

    // The rows differ along WHICH members the patch names, since that is what
    // the rule generalises over; every row is read back through all three
    // readers.
    let rows: Vec<(&str, Value)> = vec![
        ("the description alone", json!({"description": "read by three"})),
        (
            "the role and the location together",
            json!({
                "role": "arn:aws-cn:iam::123456789012:role/service-role/Other",
                "s3Location": "arn:aws:s3:::twinmaker-second-bucket",
            }),
        ),
        (
            "every member at once",
            json!({
                "description": "the last description",
                "role": "arn:aws-us-gov:iam::999999999999:role/g",
                "s3Location": "arn:aws:s3:::twinmaker-third-bucket",
            }),
        ),
        ("no member at all", json!({})),
    ];

    for (case, patch) in &rows {
        let updated = emulator.update_workspace(WORKSPACE, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateWorkspace, got {updated:?}"
        );

        let by_id = emulator.get_workspace(WORKSPACE).await;
        let by_arn = emulator
            .get_workspace(&crate::harness::encode_label(&workspace_arn(WORKSPACE)))
            .await;
        let listed = summary(&emulator, WORKSPACE).await;

        // The read addressed by ARN is the read addressed by id: one record,
        // two ways of naming it.
        assert_eq!(
            by_arn.body, by_id.body,
            "case {case:?}: the ARN and the id must name one workspace, got {by_arn:?}"
        );
        // Each patched member is what the read reports.
        for (member, value) in patch.as_object().expect("the patch is an object") {
            assert_eq!(
                by_id.member(member),
                value,
                "case {case:?}: expected {member} to be {value:?}, got {by_id:?}"
            );
        }
        // The instant the update answered is the one every reader reports.
        for (reader, reported) in [
            ("GetWorkspace", by_id.member("updateDateTime")),
            ("ListWorkspaces", &listed["updateDateTime"]),
        ] {
            assert_eq!(
                reported,
                updated.member("updateDateTime"),
                "case {case:?}: {reader} must report the instant the update answered, got \
                 {reported:?}"
            );
        }
        // The listing reports the same workspace the read does, member for
        // member, for every member a summary carries (`WorkspaceSummary`).
        for member in [
            "workspaceId",
            "arn",
            "description",
            "creationDateTime",
            "updateDateTime",
        ] {
            assert_eq!(
                &listed[member],
                by_id.member(member),
                "case {case:?}: the listing and the read must agree on {member}, got {listed:?} \
                 and {by_id:?}"
            );
        }
    }
}

/// RULE: no member of an update names anything the workspace contains, so a
/// patch leaves the workspace's tags and every resource inside it exactly as
/// they were.
#[tokio::test]
async fn a_patch_leaves_everything_the_workspace_contains_alone() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace_with(WORKSPACE, created()).await.status,
        200
    );
    assert_eq!(
        emulator
            .create_entity(
                WORKSPACE,
                json!({"entityId": "pump1", "entityName": "Pump One"}),
            )
            .await
            .status,
        200,
        "the entity the patch must not touch has to exist"
    );
    assert_eq!(
        emulator
            .create_component_type(WORKSPACE, "com.example.pump", json!({}))
            .await
            .status,
        200,
        "the component type the patch must not touch has to exist"
    );
    assert_eq!(
        emulator
            .create_scene(
                WORKSPACE,
                json!({"sceneId": "floor", "contentLocation": "s3://bucket/floor.json"}),
            )
            .await
            .status,
        200,
        "the scene the patch must not touch has to exist"
    );
    // A sync job is what links another service into a workspace, so the
    // workspace's `linkedServices` is only non-empty with one in it.
    seed_sync_job(&emulator, WORKSPACE, "SITEWISE");

    let arn = workspace_arn(WORKSPACE);
    let before_workspace = emulator.get_workspace(WORKSPACE).await;
    let before_tags = emulator.list_tags_for_resource(&arn, json!({})).await;
    let before_entity = emulator.get_entity(WORKSPACE, "pump1").await;
    let before_scene = emulator.get_scene(WORKSPACE, "floor").await;
    let before_component_type = emulator
        .get_component_type(WORKSPACE, "com.example.pump")
        .await;
    let before_sync_job = emulator.get_sync_job(Some(WORKSPACE), "SITEWISE").await;

    // The rows differ along which members the patch names; none of them may
    // reach anything asserted below.
    let rows: Vec<(&str, Value)> = vec![
        ("the description alone", json!({"description": "contents untouched"})),
        (
            "every member at once",
            json!({
                "description": "",
                "role": "arn:aws:iam::000000000000:role/Another",
                "s3Location": "arn:aws:s3:::twinmaker-other-bucket",
            }),
        ),
        ("no member at all", json!({})),
    ];

    for (case, patch) in &rows {
        let updated = emulator.update_workspace(WORKSPACE, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateWorkspace, got {updated:?}"
        );

        let tags = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            tags.body, before_tags.body,
            "case {case:?}: an update must not touch the workspace's tags, got {tags:?}"
        );
        let entity = emulator.get_entity(WORKSPACE, "pump1").await;
        assert_eq!(
            entity.body, before_entity.body,
            "case {case:?}: an update must not touch the workspace's entities, got {entity:?}"
        );
        let scene = emulator.get_scene(WORKSPACE, "floor").await;
        assert_eq!(
            scene.body, before_scene.body,
            "case {case:?}: an update must not touch the workspace's scenes, got {scene:?}"
        );
        let sync_job = emulator.get_sync_job(Some(WORKSPACE), "SITEWISE").await;
        assert_eq!(
            sync_job.body, before_sync_job.body,
            "case {case:?}: an update must not touch the workspace's sync jobs, got {sync_job:?}"
        );
        // A component type's own clock settles it from `CREATING` to `ACTIVE`,
        // so its `state` moves on its own; its identity and its instants are
        // what an unrelated update may not move.
        let component_type = emulator
            .get_component_type(WORKSPACE, "com.example.pump")
            .await;
        for member in ["componentTypeId", "arn", "creationDateTime", "updateDateTime"] {
            assert_eq!(
                component_type.member(member),
                before_component_type.member(member),
                "case {case:?}: an update must not move a component type's {member}, got \
                 {component_type:?}"
            );
        }
        // The services linked into the workspace are read off its sync jobs, so
        // they are unchanged for the same reason the jobs are.
        let read = emulator.get_workspace(WORKSPACE).await;
        for member in ["workspaceId", "arn", "creationDateTime", "linkedServices"] {
            assert_eq!(
                read.member(member),
                before_workspace.member(member),
                "case {case:?}: an update must not move the workspace's {member}, got {read:?}"
            );
        }
    }
}

/// One boundary case: what the request sends, and the clause the refusal must
/// state about it.
struct Refusal {
    case: &'static str,
    /// The id in the request path — a row about the `workspaceId` label sends an
    /// id that is itself out of bounds.
    workspace_id: String,
    patch: Value,
    /// The tail of the `ValidationException` message: the member path and the
    /// requirement the value failed.
    clause: String,
}

impl Refusal {
    fn new(case: &'static str, workspace_id: &str, patch: Value, clause: String) -> Self {
        Self {
            case,
            workspace_id: workspace_id.to_string(),
            patch,
            clause,
        }
    }
}

/// The clause naming a member that exceeded its modelled maximum length.
fn too_long(member: &str, max: usize) -> String {
    format!(
        "at '{member}' failed to satisfy constraint: Member must have length less than or equal \
         to {max}"
    )
}

/// The clause naming a member that fell outside its modelled pattern.
fn off_pattern(member: &str, pattern: &str) -> String {
    format!(
        "at '{member}' failed to satisfy constraint: Member must satisfy regular expression \
         pattern: {pattern}"
    )
}

/// RULE: a value outside the modelled bound of the member it is sent as is
/// refused with HTTP 400 `ValidationException` before the workspace is looked
/// up, and the stored workspace is left exactly as it was.
///
/// Each row violates exactly ONE bound, so the clause the refusal states is the
/// one the row is about. The members' bounds and patterns are the modelled ones
/// (`smithy_query(query="operation",operation="UpdateWorkspace")`): `description`
/// 0..2048, `role` 20..2048, `s3Location` 0..1024, `workspaceId` 1..128. A
/// `role` shorter than its minimum has no row of its own: the pattern it must
/// also satisfy is longer than that minimum, so no value violates the minimum
/// alone.
#[tokio::test]
async fn a_value_outside_its_modelled_bound_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace_with(WORKSPACE, created()).await.status,
        200
    );
    let before = emulator.get_workspace(WORKSPACE).await;

    let rows = vec![
        Refusal::new(
            "a description one character past its maximum",
            WORKSPACE,
            json!({"description": "d".repeat(2049)}),
            too_long("description", 2048),
        ),
        Refusal::new(
            "a role one character past its maximum",
            WORKSPACE,
            json!({"role": format!("{ROLE_PREFIX}{}", "r".repeat(2049 - ROLE_PREFIX.len()))}),
            too_long("role", 2048),
        ),
        Refusal::new(
            "a role that is not a role ARN",
            WORKSPACE,
            json!({"role": "arn:aws:s3:::twinmaker-not-a-role-at-all"}),
            off_pattern(
                "role",
                r"^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/",
            ),
        ),
        Refusal::new(
            "an s3Location one character past its maximum",
            WORKSPACE,
            json!({"s3Location": format!("{S3_PREFIX}{}", "b".repeat(1025 - S3_PREFIX.len()))}),
            too_long("s3Location", 1024),
        ),
        Refusal::new(
            "an s3Location whose bucket carries an illegal character",
            WORKSPACE,
            json!({"s3Location": "arn:aws:s3:::bad bucket"}),
            off_pattern(
                "s3Location",
                r"(^arn:((aws)|(aws-cn)|(aws-us-gov)):s3:::)([a-zA-Z0-9_-]+$)",
            ),
        ),
        Refusal::new(
            "a workspaceId one character past its maximum",
            &"w".repeat(129),
            json!({"description": "never stored"}),
            too_long("workspaceId", 128),
        ),
        Refusal::new(
            "a workspaceId too short to satisfy its pattern",
            "a",
            json!({"description": "never stored"}),
            off_pattern(
                "workspaceId",
                r"^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$",
            ),
        ),
    ];

    for row in &rows {
        let case = row.case;
        let refused = emulator
            .update_workspace(&row.workspace_id, row.patch.clone())
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected a 400 ValidationException, got {refused:?}"
        );
        let stated = message(&refused);
        assert!(
            stated.starts_with("1 validation error detected: "),
            "case {case:?}: expected the one-error envelope, got {stated:?}"
        );
        assert!(
            stated.ends_with(&row.clause),
            "case {case:?}: expected the message to end with {:?}, got {stated:?}",
            row.clause
        );
        // Refused before the workspace was reached, so nothing moved — not even
        // its update instant.
        let read = emulator.get_workspace(WORKSPACE).await;
        assert_eq!(
            read.body, before.body,
            "case {case:?}: a refused update must change nothing, got {read:?}"
        );
    }
}

/// RULE: a value AT a published bound is inside it, so the update takes and the
/// next read reports the value.
///
/// The bounds are the modelled ones
/// (`smithy_query(query="operation",operation="UpdateWorkspace")`), one row per
/// member that carries a maximum, plus `description`'s minimum of 0 — the empty
/// string.
#[tokio::test]
async fn a_value_at_a_published_bound_is_stored() {
    let emulator = Emulator::new();
    // A 128-character id is itself a boundary case, so the row about it patches
    // a workspace created under exactly that id.
    let longest_id = "w".repeat(128);
    for workspace_id in [WORKSPACE, longest_id.as_str()] {
        assert_eq!(
            emulator
                .create_workspace_with(workspace_id, created())
                .await
                .status,
            200,
            "the workspace {workspace_id:?} must be created"
        );
    }

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a description at its minimum of 0",
            WORKSPACE,
            json!({"description": ""}),
        ),
        (
            "a description at its maximum of 2048",
            WORKSPACE,
            json!({"description": "d".repeat(2048)}),
        ),
        (
            "a role at its maximum of 2048",
            WORKSPACE,
            json!({"role": format!("{ROLE_PREFIX}{}", "r".repeat(2048 - ROLE_PREFIX.len()))}),
        ),
        (
            "an s3Location at its maximum of 1024",
            WORKSPACE,
            json!({"s3Location": format!("{S3_PREFIX}{}", "b".repeat(1024 - S3_PREFIX.len()))}),
        ),
        (
            "a workspaceId at its maximum of 128",
            longest_id.as_str(),
            json!({"description": "patched through the longest id"}),
        ),
    ];

    for (case, workspace_id, patch) in &rows {
        let updated = emulator.update_workspace(workspace_id, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateWorkspace, got {updated:?}"
        );
        let read = emulator.get_workspace(workspace_id).await;
        for (member, value) in patch.as_object().expect("the patch is an object") {
            assert_eq!(
                read.member(member),
                value,
                "case {case:?}: expected {member} to be stored, got {read:?}"
            );
        }
    }
}

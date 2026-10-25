// SPDX-License-Identifier: CC0-1.0

//! `UpdateWorkspace` as a PATCH on a stored workspace, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The RULE, asserted over a table rather than one case: an update replaces
//! exactly the members the request names, leaves every member it does not name
//! as the workspace stored it, and answers the instant of the change — which is
//! the `updateDateTime` the next `GetWorkspace` reports. A workspace this
//! Region's store does not hold is refused 404 `ResourceNotFoundException`, in
//! the construction the update path uses (`tests/all/not_found_constructions.rs`
//! covers that construction against the look-up path's).

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every patch below is applied to.
const WORKSPACE: &str = "patchedWorkspace";

/// What the workspace is created with, so each patch has something to leave
/// alone: a description, an execution role and an S3 location.
fn created() -> Value {
    json!({
        "description": "the created description",
        "role": "arn:aws:iam::000000000000:role/TwinMakerWorkspaceRole",
        "s3Location": "arn:aws:s3:::twinmaker-created-bucket",
    })
}

/// The member `name` as the read reports it.
fn reported(resp: &Resp, name: &str) -> Value {
    resp.member(name).clone()
}

/// RULE: a patch replaces the members it names and keeps the rest, and the
/// instant it answers is the one the next read reports.
#[tokio::test]
async fn a_patch_replaces_what_it_names_and_keeps_the_rest() {
    let emulator = Emulator::new();
    let create = emulator.create_workspace_with(WORKSPACE, created()).await;
    assert_eq!(
        create.status, 200,
        "the workspace the patches apply to must be created, got {create:?}"
    );

    // (case, the patch, the members the workspace must report after it). The
    // rows differ along WHICH members the patch names — each one alone, several
    // at once, and none at all — because that is what the rule generalises over;
    // the expectations carry the whole triple, so a member the patch does not
    // name is asserted to be unchanged by the same row.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the description alone",
            json!({"description": "patched once"}),
            json!({
                "description": "patched once",
                "role": "arn:aws:iam::000000000000:role/TwinMakerWorkspaceRole",
                "s3Location": "arn:aws:s3:::twinmaker-created-bucket",
            }),
        ),
        (
            "the role alone",
            json!({"role": "arn:aws-cn:iam::123456789012:role/service-role/plant/Other"}),
            json!({
                "description": "patched once",
                "role": "arn:aws-cn:iam::123456789012:role/service-role/plant/Other",
                "s3Location": "arn:aws:s3:::twinmaker-created-bucket",
            }),
        ),
        (
            "the s3Location alone",
            json!({"s3Location": "arn:aws:s3:::twinmaker-other-bucket"}),
            json!({
                "description": "patched once",
                "role": "arn:aws-cn:iam::123456789012:role/service-role/plant/Other",
                "s3Location": "arn:aws:s3:::twinmaker-other-bucket",
            }),
        ),
        (
            "every member at once",
            json!({
                "description": "",
                "role": "arn:aws-us-gov:iam::999999999999:role/g",
                "s3Location": "arn:aws:s3:::twinmaker-final-bucket",
            }),
            json!({
                "description": "",
                "role": "arn:aws-us-gov:iam::999999999999:role/g",
                "s3Location": "arn:aws:s3:::twinmaker-final-bucket",
            }),
        ),
        (
            "no member at all",
            json!({}),
            json!({
                "description": "",
                "role": "arn:aws-us-gov:iam::999999999999:role/g",
                "s3Location": "arn:aws:s3:::twinmaker-final-bucket",
            }),
        ),
    ];

    for (case, patch, expected) in &rows {
        let before = emulator.get_workspace(WORKSPACE).await;
        let updated = emulator.update_workspace(WORKSPACE, patch.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateWorkspace, got {updated:?}"
        );

        let read = emulator.get_workspace(WORKSPACE).await;
        for member in ["description", "role", "s3Location"] {
            assert_eq!(
                reported(&read, member),
                expected[member],
                "case {case:?}: expected {member} to be {:?}, got {read:?}",
                expected[member]
            );
        }
        // The instant the update answers is the workspace's own
        // `updateDateTime`, so the two cannot disagree; it moved forward, while
        // the creation instant the workspace was minted with did not.
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
            reported(&before, "updateDateTime").as_f64().unwrap_or_default(),
            reported(&read, "updateDateTime").as_f64().unwrap_or_default(),
        );
        assert!(
            now >= was,
            "case {case:?}: expected the update instant to move forward from {was}, got {now}"
        );
        // The workspace's identity is not patchable: no member of the request
        // reaches the id or the ARN.
        assert_eq!(
            (reported(&read, "workspaceId"), reported(&read, "arn")),
            (reported(&before, "workspaceId"), reported(&before, "arn")),
            "case {case:?}: an update must not move the id or the ARN, got {read:?}"
        );
    }
}

/// RULE: an update is scoped to the Region it is signed for, exactly as the read
/// is — the same id in another Region names another workspace, so an update
/// addressed there finds none.
#[tokio::test]
async fn an_update_only_reaches_the_region_it_is_signed_for() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace_with(WORKSPACE, created()).await.status,
        200
    );

    let elsewhere = emulator
        .call_signed_for(
            Some("eu-central-1"),
            "PUT",
            &format!("/workspaces/{WORKSPACE}"),
            Some(json!({"description": "from another Region"})),
        )
        .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "another Region holds no such workspace, got {elsewhere:?}"
    );
    let read = emulator.get_workspace(WORKSPACE).await;
    assert_eq!(
        reported(&read, "description"),
        json!("the created description"),
        "the refused update must have changed nothing, got {read:?}"
    );
}

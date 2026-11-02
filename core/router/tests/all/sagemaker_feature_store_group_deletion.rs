// SPDX-License-Identifier: CC0-1.0

//! The deletion half of the SageMaker Feature Store hand-off: the CONTROL plane deletes a
//! feature group, and the FeatureStore Runtime DATA plane stops serving its OnlineStore.
//!
//! RULE: from the `DeleteFeatureGroup` that asks for a group's deletion, the data plane
//! resolves its `FeatureGroupName` no more — "Data cannot be accessed from the OnlineStore
//! immediately after DeleteFeatureGroup is called" — so every runtime call naming it is
//! refused with the modelled `ValidationError` (HTTP 400), under either spelling
//! `FeatureGroupNameOrArn` admits, while the control plane still reports the group in
//! `FeatureGroupStatus`'s `Deleting` until the clock completes its removal.
//!
//! The two services agree only because the control plane records each group in ONE shared
//! directory (`service_common::feature_store_groups`), so the withdrawal can only be
//! observed where both halves run on the SAME router and the SAME engine — which is what
//! this suite drives, through the harness the hand-off suite beside it declares.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::sagemaker_feature_store_control_plane_handoff::{
    build_router, create_feature_group, dispatch, record, Answer,
};

/// Longer than the control plane's `FEATURE_GROUP_RELEASED_AFTER` (300 ms), so the clock
/// owes a requested deletion its completion by the time the next call runs.
const PAST_RELEASE_DUE: Duration = Duration::from_millis(600);

/// Longer than the control plane's `FEATURE_GROUP_CREATED_AFTER` (300 ms), so the clock
/// owes a created group its `Created` status — which a group must have reached before the
/// control plane will delete it.
const PAST_CREATED_DUE: Duration = Duration::from_millis(600);

/// `DescribeFeatureGroup` on the control plane, in its awsJson1_1 RPC form.
async fn describe_feature_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[("x-amz-target", "SageMaker.DescribeFeatureGroup")],
        Some(json!({ "FeatureGroupName": name })),
    )
    .await
}

/// `DeleteFeatureGroup` on the control plane, in its awsJson1_1 RPC form.
async fn delete_feature_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[("x-amz-target", "SageMaker.DeleteFeatureGroup")],
        Some(json!({ "FeatureGroupName": name })),
    )
    .await
}

#[tokio::test]
async fn deleting_a_feature_group_withdraws_its_online_store_from_the_data_plane() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    // Two groups, so the rule is "only the deleted one": the data plane keeps serving the
    // other through the whole suite.
    for name in ["deleted-group", "kept-group"] {
        let created = create_feature_group(&router, name, true).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected CreateFeatureGroup 200, got {} with body {}",
            created.status, created.body
        );
        let put = dispatch(
            &router,
            "PUT",
            &format!("/FeatureGroup/{name}"),
            &[],
            Some(json!({ "Record": record("R1", "2026-01-01T00:00:00Z", "original") })),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {name:?}: expected PutRecord 200 for a group the control plane created, \
             got {} with body {}",
            put.status, put.body
        );
    }
    let arn = describe_feature_group(&router, "deleted-group")
        .await
        .body
        .get("FeatureGroupArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert!(
        !arn.is_empty(),
        "case \"the created group\": expected DescribeFeatureGroup to report its ARN"
    );

    // A feature group is deletable only once it EXISTS, so the provisioning the clock owes
    // is left to fall due before the delete is asked for.
    tokio::time::sleep(PAST_CREATED_DUE).await;
    let deleted = delete_feature_group(&router, "deleted-group").await;
    assert_eq!(
        deleted.status, 200,
        "case \"DeleteFeatureGroup\": expected HTTP 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // The group itself is still reported, in `Deleting`: the removal is the clock's.
    let described = describe_feature_group(&router, "deleted-group").await;
    assert_eq!(
        (
            described.status,
            described
                .body
                .get("FeatureGroupStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
        (200, "Deleting"),
        "case \"the deleted group\": expected the control plane to report Deleting straight \
         after the delete, got {} with body {}",
        described.status,
        described.body
    );

    // The class the rule spans: how a runtime request spells the group it names, and
    // whether that group was the deleted one. The deleted group's OnlineStore is
    // unreachable from the delete onwards, under either spelling.
    //
    // (case, the FeatureGroupName as the request carries it, whether it is still served)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the deleted group, by name",
            "deleted-group".to_owned(),
            false,
        ),
        (
            "the deleted group, by the ARN the control plane minted",
            arn.replace(':', "%3A").replace('/', "%2F"),
            false,
        ),
        ("the group that was kept, by name", "kept-group".to_owned(), true),
    ];

    for round in ["straight after the delete", "once the removal completed"] {
        for (case, group, served) in &rows {
            let got = dispatch(
                &router,
                "GET",
                &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString=R1"),
                &[],
                None,
            )
            .await;
            let put = dispatch(
                &router,
                "PUT",
                &format!("/FeatureGroup/{group}"),
                &[],
                Some(json!({ "Record": record("R2", "2026-01-02T00:00:00Z", "later") })),
            )
            .await;
            if *served {
                assert_eq!(
                    (got.status, put.status),
                    (200, 200),
                    "case {case:?} ({round}): expected the surviving group to keep being \
                     served, got GetRecord {} with {} and PutRecord {} with {}",
                    got.status,
                    got.body,
                    put.status,
                    put.body
                );
            } else {
                assert_eq!(
                    (got.status, got.error_code().as_str()),
                    (400, "ValidationError"),
                    "case {case:?} ({round}): expected GetRecord to refuse a deleted \
                     FeatureGroupName with HTTP 400 ValidationError, got {} {:?} with body {}",
                    got.status,
                    got.error_code(),
                    got.body
                );
                assert_eq!(
                    (put.status, put.error_code().as_str()),
                    (400, "ValidationError"),
                    "case {case:?} ({round}): expected PutRecord to refuse a deleted \
                     FeatureGroupName with HTTP 400 ValidationError, got {} {:?} with body {}",
                    put.status,
                    put.error_code(),
                    put.body
                );
            }
        }
        tokio::time::sleep(PAST_RELEASE_DUE).await;
    }

    // And once the clock completed the removal, the control plane answers as if the group
    // had never existed, while the one that was never deleted is still there.
    let gone = describe_feature_group(&router, "deleted-group").await;
    assert_eq!(
        (gone.status, gone.error_code().as_str()),
        (400, "ResourceNotFound"),
        "case \"the deleted group\": expected 400 ResourceNotFound once the removal \
         completed, got {} {:?} with body {}",
        gone.status,
        gone.error_code(),
        gone.body
    );
    let kept = describe_feature_group(&router, "kept-group").await;
    assert_eq!(
        kept.status, 200,
        "case \"the group that was kept\": expected it to survive the other's deletion, got \
         {} with body {}",
        kept.status, kept.body
    );
}

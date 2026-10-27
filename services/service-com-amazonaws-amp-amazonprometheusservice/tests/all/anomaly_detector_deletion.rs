// SPDX-License-Identifier: CC0-1.0

//! `DeleteAnomalyDetector`, end to end over HTTP: the status rule the deletion of a
//! detector holds to.
//!
//! The rule under test: a lifecycle-managed record of this service is deletable only
//! while it — and, for a child, the workspace it lives in — is in the one usable status,
//! `ACTIVE`; a record mid-transition is the inconsistent state the modelled
//! `ConflictException` (409) stands for, and the refused call leaves the stored record
//! and everything the clock owed it exactly where they were. A deletion already under
//! way is the `@idempotent` replay, so it is accepted again without pushing the owed
//! removal out.
//!
//! The rows span what that rule generalises over: every pair the model's OWN status
//! catalogs admit (`WORKSPACE_STATUS_CODE_VALUES` ×
//! `ANOMALY_DETECTOR_STATUS_CODE_VALUES`), so a status this file never names by hand is
//! covered too, plus the transient statuses the service's own calls actually produce —
//! a create still settling, a `PutAnomalyDetector` retraining, a workspace being renamed
//! and a workspace being deleted.
//!
//! The harness (`Amp`, `Reply`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` already builds, and `Amp::store` reaches the SAME
//! store the handlers read — which is how a status this emulator's own calls never reach
//! is put in front of the operation.

use crate::anomaly_detector_lifecycle::{create_detector, delete_detector, describe_detector};
use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    ANOMALY_DETECTOR_STATUS_CODE_VALUES, WORKSPACE_STATUS_CODE_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    AnomalyDetector, ANOMALY_DETECTOR_RESOURCE_TYPE, STATUS_ACTIVE, STATUS_CREATING,
    STATUS_DELETING, STATUS_UPDATING, WORKSPACE_RESOURCE_TYPE,
};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

/// The instant a record forced into a given status was last modified: a fixed past one,
/// so a call that wrote the record shows as a `modifiedAt` that MOVED and a refused call
/// as one that did not.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// One detector specification carrying only the required `query`.
fn configuration(query: &str) -> Value {
    json!({ "randomCutForest": { "query": query } })
}

/// One detector inside the workspace named, created through `CreateAnomalyDetector`, so
/// it starts in the `CREATING` that create writes.
async fn create_one(amp: &Amp, region: &str, workspace_id: &str, alias: &str) -> String {
    let created = create_detector(
        amp,
        region,
        workspace_id,
        &json!({
            "alias": alias,
            "configuration": configuration("sum(rate(requests_total[5m]))"),
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {alias:?}: seeding a detector must be accepted, got {created:?}"
    );
    text(&created.body["anomalyDetectorId"])
}

/// A workspace holding one detector, both created through the service's own operations
/// with nothing waited for — the starting point for a row that then FORCES the pair of
/// statuses the call under test meets, which leaves the clock nothing to settle.
async fn created_pair(amp: &Amp, region: &str, alias: &str) -> (String, String) {
    let created = amp
        .call("POST", "/workspaces", region, &json!({ "alias": alias }).to_string())
        .await;
    assert_eq!(
        created.status, 202,
        "case {alias:?}: seeding a workspace must be accepted, got {created:?}"
    );
    let workspace_id = text(&created.body["workspaceId"]);
    let detector_id = create_one(amp, region, &workspace_id, alias).await;
    (workspace_id, detector_id)
}

/// A workspace holding one detector, both brought into existence through the service's
/// own operations, with the workspace settled to `ACTIVE` and the detector left in the
/// `CREATING` its create wrote.
async fn seeded_pair(amp: &Amp, region: &str, alias: &str) -> (String, String) {
    let workspace_id = amp.active_workspace(region, alias).await;
    let detector_id = create_one(amp, region, &workspace_id, alias).await;
    (workspace_id, detector_id)
}

/// The same pair with the detector settled to `ACTIVE` as well — the starting point for
/// every row that asks what a delete does to a usable detector.
async fn active_pair(amp: &Amp, region: &str, alias: &str) -> (String, String) {
    let pair = seeded_pair(amp, region, alias).await;
    wait_for_due_transition();
    pair
}

/// Put the workspace and the detector in the statuses named, with nothing owed by the
/// clock. Written straight into the store the handlers read, because `CREATION_FAILED`
/// and the rest are statuses this emulator's own calls never reach.
fn force_statuses(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
    workspace_status: &str,
    detector_status: &str,
) {
    let store = amp.store();
    let mut guard = store.lock();
    let workspace = guard
        .region_mut_for_create(region)
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the store holds workspace {workspace_id:?}"));
    workspace.status_code = workspace_status.to_string();
    workspace.transition_due = None;
    let detector = workspace
        .anomaly_detectors
        .get_mut(detector_id)
        .unwrap_or_else(|| panic!("workspace {workspace_id:?} holds detector {detector_id:?}"));
    detector.status_code = detector_status.to_string();
    detector.transition_due = None;
    detector.modified_at = seeded_at();
}

/// The detector the store of `region` holds, or `None` once it holds none.
fn stored(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
) -> Option<AnomalyDetector> {
    amp.store()
        .lock()
        .region(region)
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| workspace.anomaly_detectors.get(detector_id).cloned())
}

/// The status `DescribeAnomalyDetector` reports for a detector.
async fn reported_status(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    detector_id: &str,
) -> String {
    let described = describe_detector(amp, region, workspace_id, detector_id).await;
    text(&described.body["anomalyDetector"]["status"]["statusCode"])
}

/// Deleting an `ACTIVE` detector inside an `ACTIVE` workspace is accepted (the modelled
/// 202), moves the STORED record to `DELETING` with the removal owed on the clock, and
/// that clock then drops it — with or without an idempotency token.
#[tokio::test]
async fn deleting_an_active_detector_starts_the_removal_the_clock_owes() {
    let region = "eu-west-1";
    for client_token in [None, Some("delete-detector-token")] {
        let amp = Amp::new();
        let (workspace_id, detector_id) = created_pair(&amp, region, "deletable").await;
        force_statuses(
            &amp,
            region,
            &workspace_id,
            &detector_id,
            STATUS_ACTIVE,
            STATUS_ACTIVE,
        );

        let reply = delete_detector(&amp, region, &workspace_id, &detector_id, client_token).await;
        assert_eq!(
            reply.status, 202,
            "case {client_token:?}: DeleteAnomalyDetector is modelled 202, got {reply:?}"
        );
        let pending = stored(&amp, region, &workspace_id, &detector_id).unwrap_or_else(|| {
            panic!("case {client_token:?}: the record is held until the removal is due")
        });
        assert_eq!(
            pending.status_code, STATUS_DELETING,
            "case {client_token:?}: expected the status the call wrote, got {:?}",
            pending.status_code
        );
        assert!(
            pending.transition_due.is_some(),
            "case {client_token:?}: the delete must leave a removal owed, got {:?}",
            pending.transition_due
        );
        assert!(
            pending.modified_at > seeded_at(),
            "case {client_token:?}: the delete must move modifiedAt past {:?}, got {:?}",
            seeded_at(),
            pending.modified_at
        );

        wait_for_due_transition();
        let gone = describe_detector(&amp, region, &workspace_id, &detector_id).await;
        assert_eq!(
            gone.status, 404,
            "case {client_token:?}: a removed detector must answer 404, got {gone:?}"
        );
        assert_eq!(
            stored(&amp, region, &workspace_id, &detector_id).map(|record| record.status_code),
            None,
            "case {client_token:?}: the workspace must hold no detector once the removal is due"
        );
    }
}

/// The deletion is refused with the modelled `ConflictException` (409) unless the
/// detector AND its workspace are both `ACTIVE`, the refusal names the resource whose
/// status stands in the way, and it leaves the stored record exactly where it was. The
/// rows come from the model's own status catalogs, so a status this test never names by
/// hand is covered too.
#[tokio::test]
async fn a_delete_is_refused_unless_the_detector_and_its_workspace_are_active() {
    let region = "ap-south-1";

    // Every pair of statuses the two catalogs admit, minus the one pair the rule allows
    // and the pairs a deletion already under way is (the idempotent replay, not a
    // refusal).
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for workspace_status in WORKSPACE_STATUS_CODE_VALUES {
        for detector_status in ANOMALY_DETECTOR_STATUS_CODE_VALUES {
            if *detector_status == STATUS_DELETING {
                continue;
            }
            if *workspace_status == STATUS_ACTIVE && *detector_status == STATUS_ACTIVE {
                continue;
            }
            pairs.push((workspace_status, detector_status));
        }
    }
    assert!(
        pairs.len() > 1,
        "the model's status catalogs must yield refusable pairs, got {pairs:?}"
    );

    for (workspace_status, detector_status) in pairs {
        let case = format!("workspace {workspace_status} / detector {detector_status}");
        let amp = Amp::new();
        let (workspace_id, detector_id) = created_pair(&amp, region, "conflicted").await;
        force_statuses(
            &amp,
            region,
            &workspace_id,
            &detector_id,
            workspace_status,
            detector_status,
        );

        let reply = delete_detector(
            &amp,
            region,
            &workspace_id,
            &detector_id,
            Some("delete-detector-conflict"),
        )
        .await;
        assert_eq!(
            reply.status, 409,
            "case {case:?}: the delete must be refused with the modelled 409, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {reply:?}"
        );

        // The parent's status is what stands in the way whenever the workspace itself is
        // not usable; otherwise it is the detector's own.
        let (expected_id, expected_type, blocking_status) = if workspace_status != STATUS_ACTIVE {
            (&workspace_id, WORKSPACE_RESOURCE_TYPE, workspace_status)
        } else {
            (&detector_id, ANOMALY_DETECTOR_RESOURCE_TYPE, detector_status)
        };
        assert_eq!(
            &text(&reply.body["resourceId"]),
            expected_id,
            "case {case:?}: the refusal must name the resource whose status refuses it, \
             got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["resourceType"]),
            expected_type,
            "case {case:?}: the refusal must name that resource's type, got {reply:?}"
        );
        let message = text(&reply.body["message"]);
        assert!(
            message.contains(blocking_status) && message.contains(STATUS_ACTIVE),
            "case {case:?}: the refusal must say which status stands in the way and which \
             one is required, got {message:?}"
        );

        let record = stored(&amp, region, &workspace_id, &detector_id)
            .unwrap_or_else(|| panic!("case {case:?}: a refused delete removes nothing"));
        assert_eq!(
            record.status_code, detector_status,
            "case {case:?}: a refused delete leaves the status alone, got {:?}",
            record.status_code
        );
        assert_eq!(
            record.transition_due, None,
            "case {case:?}: a refused delete owes nothing, got {:?}",
            record.transition_due
        );
        assert_eq!(
            record.modified_at,
            seeded_at(),
            "case {case:?}: a refused delete leaves modifiedAt alone, expected {:?}, got {:?}",
            seeded_at(),
            record.modified_at
        );
    }
}

/// The same rule over the transient statuses the service's OWN calls produce: a create
/// still settling, the retraining `PutAnomalyDetector` starts, a workspace being renamed
/// and a workspace being deleted. Each is refused, the refusal discards nothing the
/// earlier call left owed — the transition still settles on its clock — and the delete
/// the same caller repeats once everything is `ACTIVE` again is accepted.
#[tokio::test]
async fn the_transient_statuses_the_service_produces_are_refused_then_deletable() {
    let region = "us-west-2";
    // What puts the pair out of the usable status, and the status the refusal must then
    // report for the resource that stands in the way.
    let cases = [
        ("detector still being created", STATUS_CREATING, false),
        ("detector settling a retraining", STATUS_UPDATING, false),
        ("workspace being renamed", STATUS_UPDATING, true),
    ];

    for (case, blocking_status, workspace_blocks) in cases {
        let amp = Amp::new();
        // The create leaves the detector `CREATING` inside an `ACTIVE` workspace; every
        // other row settles that first, so only the call it then makes puts the pair out
        // of the usable status.
        let (workspace_id, detector_id) = if blocking_status == STATUS_CREATING {
            seeded_pair(&amp, region, "transient").await
        } else {
            active_pair(&amp, region, "transient").await
        };
        if workspace_blocks {
            let renamed = amp
                .rename(region, &workspace_id, &json!({ "alias": "renamed" }))
                .await;
            assert_eq!(
                renamed.status, 204,
                "case {case:?}: the rename must be accepted, got {renamed:?}"
            );
        } else if blocking_status == STATUS_UPDATING {
            let retrained = amp
                .call(
                    "PUT",
                    &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
                    region,
                    &json!({ "configuration": configuration("avg(rate(errors_total[5m]))") })
                        .to_string(),
                )
                .await;
            assert_eq!(
                retrained.status, 202,
                "case {case:?}: the retraining must be accepted, got {retrained:?}"
            );
        }

        let refused = delete_detector(&amp, region, &workspace_id, &detector_id, None).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: the delete must be refused with the modelled 409, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refused:?}"
        );
        let message = text(&refused.body["message"]);
        assert!(
            message.contains(blocking_status) && message.contains(STATUS_ACTIVE),
            "case {case:?}: the refusal must report the status that stands in the way, \
             got {message:?}"
        );
        let (expected_id, expected_type) = if workspace_blocks {
            (&workspace_id, WORKSPACE_RESOURCE_TYPE)
        } else {
            (&detector_id, ANOMALY_DETECTOR_RESOURCE_TYPE)
        };
        assert_eq!(
            &text(&refused.body["resourceId"]),
            expected_id,
            "case {case:?}: the refusal must name the resource whose status refuses it, \
             got {refused:?}"
        );
        assert_eq!(
            text(&refused.body["resourceType"]),
            expected_type,
            "case {case:?}: the refusal must name that resource's type, got {refused:?}"
        );

        // The refusal discarded nothing: what the earlier call left owed still settles,
        // and the detector is then deletable.
        wait_for_due_transition();
        assert_eq!(
            reported_status(&amp, region, &workspace_id, &detector_id).await,
            STATUS_ACTIVE,
            "case {case:?}: the owed transition must still settle after a refused delete"
        );
        let accepted = delete_detector(&amp, region, &workspace_id, &detector_id, None).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: the delete must be accepted once the pair is {STATUS_ACTIVE}, \
             got {accepted:?}"
        );
    }
}

/// A workspace whose own deletion has started holds no deletable children: the child's
/// delete is refused naming the WORKSPACE, and the clock then takes the detector with its
/// workspace, after which the same call is the modelled 404.
#[tokio::test]
async fn a_detector_of_a_deleting_workspace_is_refused_then_gone() {
    let region = "us-east-2";
    let amp = Amp::new();
    let (workspace_id, detector_id) = active_pair(&amp, region, "doomed-host").await;

    let deleted_workspace = amp.delete(region, &workspace_id, None).await;
    assert_eq!(
        deleted_workspace.status, 202,
        "the workspace delete must be accepted, got {deleted_workspace:?}"
    );

    let refused = delete_detector(&amp, region, &workspace_id, &detector_id, None).await;
    assert_eq!(
        refused.status, 409,
        "a detector of a {STATUS_DELETING} workspace must be refused 409, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceId"]),
        workspace_id,
        "the refusal must name the workspace that stands in the way, got {refused:?}"
    );
    assert_eq!(
        text(&refused.body["resourceType"]),
        WORKSPACE_RESOURCE_TYPE,
        "the refusal must name the workspace's type, got {refused:?}"
    );
    assert_eq!(
        stored(&amp, region, &workspace_id, &detector_id).map(|record| record.status_code),
        Some(STATUS_ACTIVE.to_string()),
        "the refused delete must leave the detector where it was"
    );

    wait_for_due_transition();
    let gone = delete_detector(&amp, region, &workspace_id, &detector_id, None).await;
    assert_eq!(
        gone.status, 404,
        "once the workspace is gone its detector is not found, got {gone:?}"
    );
    assert_eq!(
        gone.error_code(),
        "ResourceNotFoundException",
        "unexpected error code, got {gone:?}"
    );
}

/// The operation is `@idempotent`: a delete sent while the removal of that same detector
/// is still owed is the SAME request, so it is accepted again — with the token the first
/// call carried, with none, or with another — and does NOT push the owed removal out.
#[tokio::test]
async fn a_delete_while_the_removal_is_owed_is_the_same_request() {
    let region = "eu-central-1";
    let token = "delete-detector-replay";
    let amp = Amp::new();
    let (workspace_id, detector_id) = active_pair(&amp, region, "replayed").await;

    let first = delete_detector(&amp, region, &workspace_id, &detector_id, Some(token)).await;
    assert_eq!(
        first.status, 202,
        "the first delete must be accepted, got {first:?}"
    );
    let owed = stored(&amp, region, &workspace_id, &detector_id)
        .and_then(|record| record.transition_due);
    assert!(
        owed.is_some(),
        "the delete must leave a removal owed, got {owed:?}"
    );
    let modified = stored(&amp, region, &workspace_id, &detector_id).map(|r| r.modified_at);

    for repeat_token in [Some(token), None, Some("delete-detector-other")] {
        let repeat =
            delete_detector(&amp, region, &workspace_id, &detector_id, repeat_token).await;
        assert_eq!(
            repeat.status, 202,
            "case {repeat_token:?}: a deletion already under way must still be accepted, \
             got {repeat:?}"
        );
        let record = stored(&amp, region, &workspace_id, &detector_id).unwrap_or_else(|| {
            panic!("case {repeat_token:?}: the record is held until the removal is due")
        });
        assert_eq!(
            record.transition_due, owed,
            "case {repeat_token:?}: the repeat must not move the instant the removal is \
             owed, expected {owed:?}, got {:?}",
            record.transition_due
        );
        assert_eq!(
            Some(record.modified_at),
            modified,
            "case {repeat_token:?}: the repeat changes nothing, so modifiedAt stays, \
             expected {modified:?}, got {:?}",
            record.modified_at
        );
        assert_eq!(
            record.status_code, STATUS_DELETING,
            "case {repeat_token:?}: the record stays in the status the first call wrote, \
             got {:?}",
            record.status_code
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAccessPoint`: the deletion an accepted call starts, the
//! `deleting` phase the clock owes the removal from, and the `AccessPointNotFound`
//! refusal every later read and delete of the dead id answers.
//!
//! The rule under test is asserted over a SET of inputs rather than on one call: the
//! table below varies what the rule generalises over — the lifecycle phase the access
//! point is in when the delete arrives, the form the request names it by (an id or an
//! access-point ARN), and how many deletes of the one live access point are issued — and
//! the ids nothing is stored for are a table of their own.
//!
//! The harness (`Emulator`, `Reply`, `settle`, `settle_deletion`) is the one every other
//! suite of this crate drives this service through.

use std::sync::Arc;

use serde_json::json;

use crate::access_point_lifecycle::ids_of;
use crate::harness::{settle, settle_deletion, Emulator};

/// One row of the deletion table: the state the access point is in when the delete
/// arrives, how the request names it, and how many deletes are issued against it while
/// it is still there.
struct Case {
    case: &'static str,
    /// Wait out the `creating` span before deleting, so the access point is `available`.
    settle_first: bool,
    /// Name the access point by its `AccessPointArn` rather than by its id: the
    /// `AccessPointId` member accepts either form.
    by_arn: bool,
    /// How many deletes of the one still-present access point to issue.
    deletes: usize,
}

fn deletion_table() -> Vec<Case> {
    vec![
        Case {
            case: "an available access point, named by its id",
            settle_first: true,
            by_arn: false,
            deletes: 1,
        },
        Case {
            // A delete is accepted whatever non-terminal phase the access point is in,
            // so one still inside its `creating` span is deleted too.
            case: "an access point still inside its creating span",
            settle_first: false,
            by_arn: false,
            deletes: 1,
        },
        Case {
            case: "an available access point, named by its ARN",
            settle_first: true,
            by_arn: true,
            deletes: 1,
        },
        Case {
            // Acceptance is conditional on the access point still being there, not on
            // being the first caller: a second delete inside the `deleting` span is
            // accepted as well and converges on the same single deletion.
            case: "two deletes inside the deleting span",
            settle_first: true,
            by_arn: false,
            deletes: 2,
        },
        Case {
            case: "eight deletes inside the deleting span",
            settle_first: true,
            by_arn: false,
            deletes: 8,
        },
    ]
}

/// Create one access point on a fresh available file system, and answer its file system
/// id, its access point id and its ARN.
async fn access_point(efs: &Emulator, token: &str) -> (String, String, String) {
    let file_system_id = efs.available_file_system(token).await;
    let created = efs
        .create_access_point(json!({
            "ClientToken": token,
            "FileSystemId": file_system_id.clone(),
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {token:?}: creating the access point: expected 200, got {} {:?}",
        created.status, created.body
    );
    (
        file_system_id,
        created.string("AccessPointId"),
        created.string("AccessPointArn"),
    )
}

#[tokio::test]
async fn an_accepted_delete_moves_the_access_point_to_deleting_then_removes_it() {
    for Case {
        case,
        settle_first,
        by_arn,
        deletes,
    } in deletion_table()
    {
        let efs = Emulator::new();
        let (file_system_id, access_point_id, access_point_arn) =
            access_point(&efs, "delete-access-point").await;
        if settle_first {
            settle().await;
        }

        // Every delete of the still-present access point is accepted with the modelled
        // 204 and an empty body.
        let addressed = if by_arn {
            // The ARN is the value of an `@httpLabel`, so its reserved characters are
            // percent-encoded exactly as an AWS SDK encodes them.
            access_point_arn.replace(':', "%3A").replace('/', "%2F")
        } else {
            access_point_id.clone()
        };
        for attempt in 1..=deletes {
            let deleted = efs.delete_access_point(&addressed).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?} delete {attempt}: expected 204, got {} {:?}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.body_len, 0,
                "case {case:?} delete {attempt}: a 204 carries no body, got {} byte(s)",
                deleted.body_len
            );
            if attempt > 1 {
                continue;
            }

            // The deletion is not complete when the call returns: inside its span the
            // access point is still reported, in the modelled `deleting` phase. Read
            // straight after the FIRST delete, so the read lands well inside the span
            // however long the remaining deletes of this row take.
            let deleting = efs
                .describe_access_points(&format!("AccessPointId={access_point_id}"))
                .await;
            assert_eq!(
                deleting.status, 200,
                "case {case:?}: a deleting access point is still described, got {} {:?}",
                deleting.status, deleting.body
            );
            let described = deleting.body["AccessPoints"][0].clone();
            assert_eq!(
                described["LifeCycleState"],
                json!("deleting"),
                "case {case:?}: expected LifeCycleState \"deleting\", got {:?}",
                described["LifeCycleState"]
            );
            assert_eq!(
                described["AccessPointId"],
                json!(access_point_id),
                "case {case:?}: the deleting description must keep its id, got {:?}",
                described["AccessPointId"]
            );
        }

        // Once the clock owes the removal, the record is gone: the id is not found by the
        // read, it is absent from its file system's list, and a further delete of it is
        // refused rather than accepted again.
        settle_deletion().await;
        let gone = efs
            .describe_access_points(&format!("AccessPointId={access_point_id}"))
            .await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: describing the deleted id: expected 404, got {} {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_type.as_deref(),
            Some("AccessPointNotFound"),
            "case {case:?}: describing the deleted id: unexpected error type, got {:?}",
            gone.error_type
        );
        let listed = efs
            .describe_access_points(&format!("FileSystemId={file_system_id}"))
            .await;
        assert!(
            ids_of(&listed).is_empty(),
            "case {case:?}: the file system must hold no access point, got {:?}",
            listed.body
        );
        let redeleted = efs.delete_access_point(&addressed).await;
        assert_eq!(
            redeleted.status, 404,
            "case {case:?}: redeleting the dead id: expected 404, got {} {:?}",
            redeleted.status, redeleted.body
        );
        assert_eq!(
            redeleted.error_type.as_deref(),
            Some("AccessPointNotFound"),
            "case {case:?}: redeleting the dead id: unexpected error type, got {:?}",
            redeleted.error_type
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_delete_racing_one_live_access_point_is_accepted() {
    let efs = Arc::new(Emulator::new());
    let (file_system_id, access_point_id, _) = access_point(&efs, "concurrent-delete").await;
    settle().await;

    // Eight deletes of the one live access point, issued from eight tasks on a
    // multi-thread runtime so they genuinely contend for the state lock: each is
    // accepted, none collides, and they converge on a single deletion.
    let racers: Vec<_> = (0..8)
        .map(|_| {
            let efs = Arc::clone(&efs);
            let access_point_id = access_point_id.clone();
            tokio::spawn(async move { efs.delete_access_point(&access_point_id).await })
        })
        .collect();
    for (racer, task) in racers.into_iter().enumerate() {
        let reply = task.await.expect("the racing delete joined");
        assert_eq!(
            reply.status, 204,
            "racer {racer}: expected 204, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, None,
            "racer {racer}: a racing delete must not collide, got {:?}",
            reply.error_type
        );
    }

    settle_deletion().await;
    let listed = efs
        .describe_access_points(&format!("FileSystemId={file_system_id}"))
        .await;
    assert!(
        ids_of(&listed).is_empty(),
        "eight racing deletes must converge on one deletion, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn delete_access_point_refuses_an_id_nothing_is_stored_for() {
    let efs = Emulator::new();
    let (file_system_id, access_point_id, _) = access_point(&efs, "absent-delete").await;
    settle().await;

    // Every form a stored `AccessPointId` may take, over the modelled id pattern's
    // `^fsap-[0-9a-f]{8,40}$` lower and upper lengths, names the same absent access
    // point, so each is the same 404.
    let absent = [
        ("the shortest bare id the pattern allows", "fsap-deadbeef".to_string()),
        (
            "the longest bare id the pattern allows",
            format!("fsap-{}", "a".repeat(40)),
        ),
        (
            "an ARN naming an access point nothing is stored for",
            "arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/fsap-deadbeef"
                .replace(':', "%3A")
                .replace('/', "%2F"),
        ),
    ];
    for (case, id) in absent {
        let refused = efs.delete_access_point(&id).await;
        assert_eq!(
            refused.status, 404,
            "{case}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("AccessPointNotFound"),
            "{case}: unexpected error type, got {:?}",
            refused.error_type
        );
    }

    // A refused delete touched nothing: the stored access point is still there, still
    // `available`.
    let listed = efs
        .describe_access_points(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        ids_of(&listed),
        vec![access_point_id],
        "a refused delete must leave the stored access points alone, got {:?}",
        listed.body
    );
    assert_eq!(
        listed.body["AccessPoints"][0]["LifeCycleState"],
        json!("available"),
        "a refused delete must not move another access point's lifecycle state, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_completed_deletion_frees_the_client_token_it_was_created_with() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("token-release").await;
    let create = json!({
        "ClientToken": "token-release",
        "FileSystemId": file_system_id.clone(),
    });
    let first = efs.create_access_point(create.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first create: expected 200, got {} {:?}",
        first.status, first.body
    );
    let first_id = first.string("AccessPointId");
    settle().await;

    // While the access point is still there — `available`, then `deleting` — the token
    // still names it, so a repeat of the create is the idempotency conflict.
    for case in ["available", "deleting"] {
        if case == "deleting" {
            let deleted = efs.delete_access_point(&first_id).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected 204, got {} {:?}",
                deleted.status, deleted.body
            );
        }
        let repeat = efs.create_access_point(create.clone()).await;
        assert_eq!(
            repeat.status, 409,
            "case {case:?}: reusing the token must conflict, got {} {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_type.as_deref(),
            Some("AccessPointAlreadyExists"),
            "case {case:?}: unexpected error type, got {:?}",
            repeat.error_type
        );
    }

    // Once the deletion has completed the token names nothing, so the very same call
    // mints a NEW access point with a distinct id.
    settle_deletion().await;
    let again = efs.create_access_point(create).await;
    assert_eq!(
        again.status, 200,
        "after the deletion completed: expected 200, got {} {:?}",
        again.status, again.body
    );
    let second_id = again.string("AccessPointId");
    assert_ne!(
        second_id, first_id,
        "the token must mint a distinct access point, got {second_id:?}"
    );
    settle().await;
    let listed = efs
        .describe_access_points(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        ids_of(&listed),
        vec![second_id],
        "exactly the one access point the second create minted must be stored, got {:?}",
        listed.body
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAccessPoint`, driven over HTTP through the same
//! `MultiServiceRouter` the production entrypoint serves.
//!
//! The rule under test: a delete of an access point is accepted (204, no body) for as
//! long as the record is PRESENT — whether it is still `creating`, `available`, or a
//! delete has already been accepted for it, in which case it is in `deleting` — and is
//! refused `AccessPointNotFound` (404) once the record is gone; however many deletes are
//! accepted for one access point, they converge on exactly ONE deletion at the instant
//! the first accepted call pinned, and the `ClientToken` the access point was made
//! idempotent by is released at exactly that instant, never before.
//!
//! Every assertion runs over a SET of inputs rather than on one recorded call: the race
//! table varies the number of concurrent deletes (the recorded case, 8, is one row) and
//! the lifecycle phase the access point is in when they are issued; the refusal table
//! varies the id's hex length at both bounds of the modelled `^fsap-[0-9a-f]{8,40}$`
//! pattern and whether the id was never minted or was minted and then deleted; and the
//! token table varies the `ClientToken` over its modelled `length 1..64`.
//!
//! The harness (`Emulator`, `Reply`, `settle`) is the one every suite of this crate
//! drives this service through.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{settle, Emulator, Reply};

/// Wait past the clock-owed removal of an access point whose delete was accepted. The
/// handler never sleeps for it — it stores the due instant — so only a test that wants
/// the settled state waits here.
async fn settle_deletion() {
    tokio::time::sleep(state::DELETING_FOR + Duration::from_millis(150)).await;
}

/// The access point ids one `DescribeAccessPoints` answer reports, in order.
fn ids_of(reply: &Reply) -> Vec<String> {
    reply
        .get("AccessPoints")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|description| {
            description["AccessPointId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `LifeCycleState` the one described access point reports.
fn life_cycle_state_of(reply: &Reply) -> String {
    reply
        .get("AccessPoints")
        .as_array()
        .and_then(|descriptions| descriptions.first().cloned())
        .map(|description| {
            description["LifeCycleState"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default()
}

/// Assert one `AccessPointNotFound` refusal: the modelled status, the error shape and the
/// request-derived sentence this service builds for a resolved-and-missing id.
fn assert_access_point_not_found(case: &str, reply: &Reply, named: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected 404, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("AccessPointNotFound"),
        "case {case:?}: expected error type AccessPointNotFound, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("Message"),
        format!("Access point '{named}' does not exist."),
        "case {case:?}: unexpected Message construction, got {:?}",
        reply.get("Message")
    );
}

/// Issue `racers` `DeleteAccessPoint` calls for one id CONCURRENTLY, on a multi-thread
/// runtime, and report what each answered.
async fn race_deletes(
    efs: &Arc<Emulator>,
    access_point_id: &str,
    racers: usize,
) -> Vec<(u16, Option<String>)> {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..racers {
        let efs = Arc::clone(efs);
        let id = access_point_id.to_string();
        tasks.spawn(async move {
            let reply = efs.delete_access_point(&id).await;
            (reply.status, reply.error_type)
        });
    }
    let mut answers = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        answers.push(joined.expect("a delete task that did not panic"));
    }
    answers
}

/// One row of the race table.
struct Race {
    case: &'static str,
    /// How many deletes of the SAME live access point are issued at once.
    racers: usize,
    /// Whether to wait out the clock-owed `creating` -> `available` transition first, so
    /// the rule is asserted on both lifecycle phases a record can be present in.
    settle_first: bool,
}

/// The recorded case is eight concurrent deletes of an `available` access point; the
/// other rows vary the only two things the rule generalises over — how many deletes race
/// and which present phase the record is in.
const RACES: &[Race] = &[
    Race {
        case: "a single delete of an available access point",
        racers: 1,
        settle_first: true,
    },
    Race {
        case: "two concurrent deletes",
        racers: 2,
        settle_first: true,
    },
    Race {
        case: "eight concurrent deletes",
        racers: 8,
        settle_first: true,
    },
    Race {
        case: "sixteen concurrent deletes",
        racers: 16,
        settle_first: true,
    },
    Race {
        case: "eight concurrent deletes while the access point is still creating",
        racers: 8,
        settle_first: false,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_deletes_of_a_live_access_point_are_all_accepted_and_converge_on_one_deletion()
{
    for race in RACES {
        let case = race.case;
        let efs = Arc::new(Emulator::new());
        let file_system_id = efs.available_file_system("delete-race").await;
        let token = format!("delete-race-{}", race.racers);
        let created = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
                "RootDirectory": { "Path": "/raced" },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAccessPoint expected 200, got {} {:?}",
            created.status, created.body
        );
        let access_point_id = created.string("AccessPointId");
        if race.settle_first {
            settle().await;
        }

        // EVERY racer is accepted: the record is still present when each of them runs,
        // so none of them collides with another.
        let answers = race_deletes(&efs, &access_point_id, race.racers).await;
        assert_eq!(
            answers,
            vec![(204u16, None); race.racers],
            "case {case:?}: expected every concurrent delete of a live access point to be \
             accepted with the modelled 204, got {answers:?}"
        );

        // The deletion the clock owes has not landed yet, so the access point is still
        // readable — in the `deleting` phase the accepted delete moved it to, which is
        // what makes the next racer's call land on a present record.
        let during = efs
            .describe_access_points(&format!("AccessPointId={access_point_id}"))
            .await;
        assert_eq!(
            during.status, 200,
            "case {case:?}: DescribeAccessPoints inside the deleting window expected 200, \
             got {} {:?}",
            during.status, during.body
        );
        // The modelled `LifeCycleState` value is pinned as a LITERAL here rather than
        // read back from the crate's own constant, so a mis-spelled constant cannot make
        // this pass while the serialized state diverges from the modelled enum.
        assert_eq!(
            life_cycle_state_of(&during),
            "deleting",
            "case {case:?}: an accepted delete moves the access point to \"deleting\", got \
             {:?}",
            during.body
        );

        settle_deletion().await;

        // CONVERGENCE: exactly one deletion happened — the access point is gone, and it
        // is the only thing that went.
        let after = efs
            .describe_access_points(&format!("AccessPointId={access_point_id}"))
            .await;
        assert_access_point_not_found(
            &format!("{case}: DescribeAccessPoints after the deletion"),
            &after,
            &access_point_id,
        );
        let remaining = efs
            .describe_access_points(&format!("FileSystemId={file_system_id}"))
            .await;
        assert_eq!(
            ids_of(&remaining),
            Vec::<String>::new(),
            "case {case:?}: the file system must hold no access point after the race, got \
             {:?}",
            remaining.body
        );

        // A FURTHER delete of the now-dead id is refused rather than accepted again:
        // acceptance is conditional on the record being present.
        let redelete = efs.delete_access_point(&access_point_id).await;
        assert_access_point_not_found(
            &format!("{case}: DeleteAccessPoint after the deletion"),
            &redelete,
            &access_point_id,
        );

        // The deletion released the `ClientToken` the access point was made idempotent
        // by, exactly once: the same token now mints ONE new, distinct access point.
        let recreated = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the token of a deleted access point creates again, expected 200, \
             got {} {:?}",
            recreated.status, recreated.body
        );
        let recreated_id = recreated.string("AccessPointId");
        assert_ne!(
            recreated_id, access_point_id,
            "case {case:?}: the reused token must mint a distinct id, got {recreated_id:?}"
        );
        let listed = efs
            .describe_access_points(&format!("FileSystemId={file_system_id}"))
            .await;
        assert_eq!(
            ids_of(&listed),
            vec![recreated_id],
            "case {case:?}: exactly the one re-created access point must remain, got {:?}",
            listed.body
        );
    }
}

/// `AccessPointId` values no access point holds, as `(what the request carries, the
/// access point the refusal must name)`: the shortest body the modelled
/// `^fsap-[0-9a-f]{8,40}$` pattern admits, one between the bounds, and the longest. The
/// member also accepts the ARN form, but this operation binds the id as a single
/// `@httpLabel`, which an ARN's slash cannot travel through as one path segment, so that
/// form is covered where it can be requested (`DescribeAccessPoints` in
/// `error_message_construction`).
const MISSING_ACCESS_POINTS: &[(&str, &str)] = &[
    ("fsap-00000000", "fsap-00000000"),
    ("fsap-0123456789abcdef", "fsap-0123456789abcdef"),
    (
        "fsap-cccccccccccccccccccccccccccccccccccccccc",
        "fsap-cccccccccccccccccccccccccccccccccccccccc",
    ),
];

#[tokio::test]
async fn delete_access_point_refuses_an_id_no_access_point_holds() {
    let efs = Emulator::new();

    for (requested, named) in MISSING_ACCESS_POINTS {
        let reply = efs.delete_access_point(requested).await;
        assert_access_point_not_found(
            &format!("DeleteAccessPoint({requested})"),
            &reply,
            named,
        );
    }
}

#[tokio::test]
async fn deleting_one_access_point_leaves_every_other_one_readable() {
    let efs = Emulator::new();
    let first_file_system = efs.available_file_system("delete-sibling-1").await;
    let second_file_system = efs.available_file_system("delete-sibling-2").await;

    // Three access points over two file systems, so the deletion is observed against
    // siblings on the same file system AND on another one.
    let mut ids = Vec::new();
    for (token, file_system_id) in [
        ("sibling-a", &first_file_system),
        ("sibling-b", &first_file_system),
        ("sibling-c", &second_file_system),
    ] {
        let created = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "creating {token}: expected 200, got {} {:?}",
            created.status, created.body
        );
        ids.push(created.string("AccessPointId"));
    }
    settle().await;

    let deleted = efs.delete_access_point(&ids[0]).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteAccessPoint({}): expected 204, got {} {:?}",
        ids[0], deleted.status, deleted.body
    );
    settle_deletion().await;

    assert_eq!(
        ids_of(&efs.describe_access_points("").await),
        vec![ids[1].clone(), ids[2].clone()],
        "only the deleted access point goes, in the unchanged creation order"
    );
    for id in &ids[1..] {
        let described = efs
            .describe_access_points(&format!("AccessPointId={id}"))
            .await;
        assert_eq!(
            described.status, 200,
            "DescribeAccessPoints({id}) after a sibling's deletion: expected 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            life_cycle_state_of(&described),
            "available",
            "case {id:?}: a sibling keeps its own lifecycle state, got {:?}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_deleted_access_points_client_token_is_released_only_once_its_record_is_gone() {
    // The token binding lives exactly as long as the record: inside the deleting window
    // the same `ClientToken` still conflicts with the access point being deleted, and
    // only once the record is gone does it create again.
    for token in [
        "t",
        "sim2real-probe-efs-ap-token-reuse",
        "token with spaces and .dots",
    ] {
        let efs = Emulator::new();
        let file_system_id = efs.available_file_system("delete-token").await;
        let created = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {token:?}: CreateAccessPoint expected 200, got {} {:?}",
            created.status, created.body
        );
        let access_point_id = created.string("AccessPointId");
        settle().await;

        let deleted = efs.delete_access_point(&access_point_id).await;
        assert_eq!(
            deleted.status, 204,
            "case {token:?}: DeleteAccessPoint expected 204, got {} {:?}",
            deleted.status, deleted.body
        );

        let during = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            during.status, 409,
            "case {token:?}: inside the deleting window the token still conflicts, expected \
             409, got {} {:?}",
            during.status, during.body
        );
        assert_eq!(
            during.string("AccessPointId"),
            access_point_id,
            "case {token:?}: the conflict names the access point being deleted, got {:?}",
            during.get("AccessPointId")
        );

        settle_deletion().await;
        let after = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await;
        assert_eq!(
            after.status, 200,
            "case {token:?}: once the record is gone the token creates again, expected 200, \
             got {} {:?}",
            after.status, after.body
        );
        assert_ne!(
            after.string("AccessPointId"),
            access_point_id,
            "case {token:?}: the released token mints a distinct access point, got {:?}",
            after.get("AccessPointId")
        );
    }
}

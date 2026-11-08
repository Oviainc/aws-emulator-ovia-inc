// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS backup-policy slice: `PutBackupPolicy` and
//! `DescribeBackupPolicy`.
//!
//! The rule these assert, over a SET of inputs rather than on one recorded call: a backup
//! policy is a resource a file system CARRIES rather than a status every file system has
//! — a file system none was ever set on answers the read with 404 `PolicyNotFound` and no
//! sentence — and a set is an ACCEPTANCE, so it reports the transient status EFS turns
//! backups on or off under and the clock settles that status at the one the request asked
//! for, observable by an independent read.
//!
//! The tables vary what the rule generalises over: whether the create asks for automatic
//! backups and how (the `Backup` member, the One Zone default), the status requested, the
//! ORDER the statuses are requested in, a repeated request, the form of the
//! `FileSystemId` (bare id or ARN) and the phase the file system is in.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::harness::{settle, Emulator, Reply};

/// The whole `PolicyNotFound` refusal, asserted the same way wherever it is reached: the
/// modelled status and error shape, and NO sentence — the member is present in the error
/// document as JSON `null`, which is how a client tells "this class carries no sentence"
/// from "the sentence is empty".
fn assert_policy_not_found(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("PolicyNotFound"),
        "case {case:?}: expected the modelled PolicyNotFound, got {:?}",
        reply.error_type
    );
    assert_eq!(
        reply.string("ErrorCode"),
        "PolicyNotFound",
        "case {case:?}: unexpected ErrorCode, got {:?}",
        reply.get("ErrorCode")
    );
    assert!(
        reply.get("Message").is_null(),
        "case {case:?}: the refusal carries no sentence, got {:?}",
        reply.get("Message")
    );
}

/// The `BackupPolicy.Status` one reply reports.
fn reported_status(reply: &Reply) -> String {
    reply.body["BackupPolicy"]["Status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_backup_policy_exists_only_once_a_create_or_a_put_sets_one() {
    // (case, CreateFileSystem body, the settled status the read must report, or `None`
    // when the file system carries no backup policy at all). `CreateFileSystem$Backup`:
    // "Specifies whether automatic backups are enabled on the file system that you are
    // creating ... Default is false. However, if you specify an AvailabilityZoneName, the
    // default is true."
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "no Backup member",
            json!({ "CreationToken": "backup-absent-1" }),
            None,
        ),
        (
            "Backup declared false",
            json!({ "CreationToken": "backup-absent-2", "Backup": false }),
            None,
        ),
        (
            "a One Zone file system that declares Backup false",
            json!({
                "CreationToken": "backup-absent-3",
                "AvailabilityZoneName": "us-east-1b",
                "Backup": false,
            }),
            None,
        ),
        (
            "Backup declared true",
            json!({ "CreationToken": "backup-set-1", "Backup": true }),
            Some(state::BACKUP_ENABLED),
        ),
        (
            "a One Zone file system, where Backup defaults to true",
            json!({
                "CreationToken": "backup-set-2",
                "AvailabilityZoneName": "us-east-1a",
            }),
            Some(state::BACKUP_ENABLED),
        ),
    ];

    let efs = Emulator::new();
    for (case, body, expected) in cases {
        let created = efs.create_file_system(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");

        // A create that asks for automatic backups starts the policy in the transient
        // status EFS reports while it turns them on; the span is never zero, so a caller
        // that reads straight away observes it.
        if expected.is_some() {
            let enabling = efs.describe_backup_policy(&file_system_id).await;
            assert_eq!(
                enabling.status, 200,
                "case {case:?}: DescribeBackupPolicy expected 200, got {} {:?}",
                enabling.status, enabling.body
            );
            assert_eq!(
                reported_status(&enabling),
                state::BACKUP_ENABLING,
                "case {case:?}: a create that asks for backups starts in the transient \
                 status, got {:?}",
                enabling.body
            );
        }

        settle().await;
        let described = efs.describe_backup_policy(&file_system_id).await;
        match expected {
            Some(status) => {
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: DescribeBackupPolicy expected 200, got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    reported_status(&described),
                    status,
                    "case {case:?}: unexpected settled status, got {:?}",
                    described.body
                );
            }
            None => assert_policy_not_found(case, &described),
        }
    }
}

/// One row of the table below: the case, whether the file system is addressed by its ARN,
/// and the requested statuses in order as (requested, reported by the acceptance, settled
/// at).
type StatusRow = (&'static str, bool, Vec<(&'static str, &'static str, &'static str)>);

#[tokio::test]
async fn put_backup_policy_drives_the_status_the_request_asks_for() {
    // (case, address the file system by its ARN, the requested statuses in order with the
    // status the acceptance reports and the one a later read converges on). A request
    // that CHANGES the policy is accepted into the transient status; one for the status
    // the policy already carries changes nothing, so it cannot push a settled policy back
    // into a transient phase.
    let cases: Vec<StatusRow> = vec![
        (
            "enable, disable, enable again, addressed by id",
            false,
            vec![
                ("ENABLED", "ENABLING", "ENABLED"),
                ("DISABLED", "DISABLING", "DISABLED"),
                ("ENABLED", "ENABLING", "ENABLED"),
            ],
        ),
        (
            "disable first, then enable, addressed by ARN",
            true,
            vec![
                ("DISABLED", "DISABLING", "DISABLED"),
                ("ENABLED", "ENABLING", "ENABLED"),
            ],
        ),
        (
            "a repeated request changes nothing",
            false,
            vec![
                ("ENABLED", "ENABLING", "ENABLED"),
                ("ENABLED", "ENABLED", "ENABLED"),
                ("DISABLED", "DISABLING", "DISABLED"),
                ("DISABLED", "DISABLED", "DISABLED"),
            ],
        ),
    ];

    let efs = Emulator::new();
    for (index, (case, by_arn, requests)) in cases.into_iter().enumerate() {
        let created = efs
            .create_file_system(json!({ "CreationToken": format!("backup-put-{index}") }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");
        // An `@httpLabel` is not greedy, so an SDK percent-encodes the separators of an
        // ARN it puts in one; both forms address the same stored file system.
        let addressed_by = if by_arn {
            created
                .string("FileSystemArn")
                .replace(':', "%3A")
                .replace('/', "%2F")
        } else {
            file_system_id.clone()
        };
        settle().await;

        // Before any set, the file system carries no policy at all.
        assert_policy_not_found(case, &efs.describe_backup_policy(&addressed_by).await);

        for (requested, accepted_as, settles_at) in requests {
            let row = format!("{case}: {requested}");
            let put = efs
                .put_backup_policy(
                    &addressed_by,
                    json!({ "BackupPolicy": { "Status": requested } }),
                )
                .await;
            assert_eq!(
                put.status, 200,
                "case {row:?}: PutBackupPolicy expected 200, got {} {:?}",
                put.status, put.body
            );
            assert_eq!(
                reported_status(&put),
                accepted_as,
                "case {row:?}: unexpected accepted status, got {:?}",
                put.body
            );

            // The read observes exactly what the acceptance reported: the write is
            // stored, not synthesized per call.
            let immediately = efs.describe_backup_policy(&file_system_id).await;
            assert_eq!(
                reported_status(&immediately),
                accepted_as,
                "case {row:?}: the read observes the accepted status, got {:?}",
                immediately.body
            );

            settle().await;
            let settled = efs.describe_backup_policy(&file_system_id).await;
            assert_eq!(
                settled.status, 200,
                "case {row:?}: DescribeBackupPolicy expected 200, got {} {:?}",
                settled.status, settled.body
            );
            assert_eq!(
                reported_status(&settled),
                settles_at,
                "case {row:?}: unexpected settled status, got {:?}",
                settled.body
            );
        }
    }
}

#[tokio::test]
async fn the_backup_policy_operations_refuse_what_they_cannot_serve() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("backup-refusals").await;

    // The two transient values of the modelled `Status` enum
    // (`smithy_query(query="shape",shape="Status")` -> ENABLED | ENABLING | DISABLED |
    // DISABLING) are the statuses EFS REPORTS while it is turning automatic backups on or
    // off, not statuses a caller can ask a policy to be set to.
    for status in [state::BACKUP_ENABLING, state::BACKUP_DISABLING] {
        let case = format!("a set asking for the transient status {status}");
        let refused = efs
            .put_backup_policy(&file_system_id, json!({ "BackupPolicy": { "Status": status } }))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("BadRequest"),
            "case {case:?}: expected the modelled BadRequest, got {:?}",
            refused.error_type
        );
        assert!(
            refused.string("Message").contains(status),
            "case {case:?}: the sentence names the status the request asked for, got {:?}",
            refused.get("Message")
        );
        // Effect-free: the refusal stored no policy, so the read still reports none.
        assert_policy_not_found(&case, &efs.describe_backup_policy(&file_system_id).await);
    }

    // An id no file system in this account holds: both entry points refuse with the one
    // request-derived sentence.
    let missing = "fs-00000000deadbeef";
    for (case, reply) in [
        ("the read of an unknown file system", efs.describe_backup_policy(missing).await),
        (
            "a set on an unknown file system",
            efs.put_backup_policy(missing, json!({ "BackupPolicy": { "Status": "ENABLED" } }))
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("FileSystemNotFound"),
            "case {case:?}: expected the modelled FileSystemNotFound, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.string("Message"),
            state::file_system_not_found_message(missing),
            "case {case:?}: unexpected Message, got {:?}",
            reply.get("Message")
        );
    }

    // `IncorrectFileSystemLifeCycleState`: "Returned if the file system's lifecycle state
    // is not available" — a file system still `creating` cannot take a set, and the
    // refusal stores nothing.
    let creating = efs
        .create_file_system(json!({ "CreationToken": "backup-while-creating" }))
        .await;
    let creating_id = creating.string("FileSystemId");
    let refused = efs
        .put_backup_policy(&creating_id, json!({ "BackupPolicy": { "Status": "ENABLED" } }))
        .await;
    assert_eq!(
        refused.status, 409,
        "case the set on a creating file system: expected HTTP 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "case the set on a creating file system: expected the modelled \
         IncorrectFileSystemLifeCycleState, got {:?}",
        refused.error_type
    );
    settle().await;
    assert_policy_not_found(
        "the read after a set refused for the lifecycle state",
        &efs.describe_backup_policy(&creating_id).await,
    );
}

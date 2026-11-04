// SPDX-License-Identifier: CC0-1.0

//! The connector operations that act on the REMOTE FILESYSTEM of the endpoint a
//! connector names: a move, a delete and a directory listing.
//!
//! Two rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. an accepted `StartRemoteMove`, `StartRemoteDelete` or `StartDirectoryListing` is
//!    CARRIED OUT against the endpoint the connector's `Url` names, and its record
//!    reports what that attempt did once the transfer's span has elapsed: a move leaves
//!    the source absent and the target holding the bytes, a delete leaves the path
//!    absent, a listing writes what the directory really holds to the output file it
//!    answered the name of, and a path the storage does not hold is `FAILED` with the
//!    code the storage itself answered
//!    ([`a_remote_move_relocates_the_object_and_reports_what_the_attempt_did`],
//!    [`a_remote_delete_removes_the_path_and_reports_what_the_attempt_did`],
//!    [`a_directory_listing_writes_what_the_remote_directory_holds`]);
//! 2. the same operation against an endpoint this emulator does NOT serve is left
//!    exactly as it was accepted — `QUEUED`, with nothing in storage changed — because
//!    nothing connected to it
//!    ([`a_remote_operation_on_an_endpoint_nothing_serves_changes_nothing`]).
//!
//! The emulated target (bucket, `ONLINE` server with a generated host key, a user with
//! a generated key pair, Secrets Manager and Amazon S3 mounted beside the transfer
//! service) is the shared [`crate::connector_gated_workflow::Target`], so the connector
//! these rules drive is brought into service exactly as that suite's are. The update
//! that brings one into service has its own suite,
//! [`crate::connector_plane`].

use serde_json::{json, Value};

use crate::connector_gated_workflow::{
    results, server_endpoint, Target, COMPLETED, FAILED, FILE_BODY, PAST_TRANSFER_SPAN, QUEUED,
    STATUS_OK,
};
use crate::harness::member_of;

/// The bytes a second object holds, so a move's target can be told from the file that
/// was already there.
const OTHER_BODY: &[u8] = b"other\n";

/// One target with a connector IN SERVICE against it: the secret is stored, the host
/// key it presents is trusted, and `TestConnection` answers `OK`.
pub(crate) async fn target_with_connector(case: &str) -> (Target, String) {
    let target = Target::with_home_at_bucket_root().await;
    let secret = target
        .store_secret(
            "aws/transfer/remote-ops",
            &target.user_name,
            &target.user_key.private,
        )
        .await;
    let connector_id = target
        .create_connector(
            case,
            json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
            None,
        )
        .await;
    let probe = target.test_connection(&connector_id).await;
    assert_eq!(
        probe.get("Status").as_str(),
        Some(STATUS_OK),
        "case {case:?}: the connector these rules drive must be in service, got {}",
        probe.body
    );
    (target, connector_id)
}

/// Accept one operation and answer the identifier it was recorded under.
pub(crate) async fn accept(
    target: &Target,
    case: &str,
    operation: &str,
    request: Value,
    id: &str,
) -> String {
    let answer = target.emulator.call(operation, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {operation} 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get(id)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {operation} to answer {id}, got {}", answer.body))
        .to_string()
}

/// The one result of an accepted remote operation, once its span has elapsed.
async fn settled_result(target: &Target, case: &str, connector_id: &str, id: &str) -> Value {
    let queued = results(target, connector_id, id).await;
    assert_eq!(
        member_of(&queued[0], "StatusCode").as_str(),
        Some(QUEUED),
        "case {case:?}: an accepted operation reads {QUEUED} before its span elapses, \
         got {}",
        queued[0]
    );
    tokio::time::sleep(PAST_TRANSFER_SPAN).await;
    let reported = results(target, connector_id, id).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the one path the operation addresses, got {reported:?}"
    );
    reported[0].clone()
}

/// Assert one settled result carries `status`, and — for a failure — a code and message
/// the storage answered.
fn assert_outcome(case: &str, result: &Value, status: &str) {
    assert_eq!(
        member_of(result, "StatusCode").as_str(),
        Some(status),
        "case {case:?}: expected {status}, got {result}"
    );
    if status == FAILED {
        for member in ["FailureCode", "FailureMessage"] {
            assert!(
                member_of(result, member)
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "case {case:?}: a failed attempt reports the {member} the storage \
                 answered, got {result}"
            );
        }
    } else {
        for member in ["FailureCode", "FailureMessage"] {
            assert!(
                member_of(result, member).is_null(),
                "case {case:?}: expected no {member} on an attempt that succeeded, got \
                 {result}"
            );
        }
    }
}

/// One row of the move table: the case, the objects to store first, the source key,
/// the target key and the status the record must report.
type MoveRow<'a> = (&'a str, Vec<(&'a str, &'a [u8])>, &'a str, &'a str, &'a str);

/// One row of the listing table: the case, the objects to store first, the directory
/// listed, the paths the output must name and the paths it must not.
type ListingRow<'a> = (&'a str, Vec<&'a str>, &'a str, Vec<&'a str>, Vec<&'a str>);

// ---------------------------------------------------------------------------
// Rule 1: what each accepted remote operation does.
// ---------------------------------------------------------------------------

/// RULE: a move accepted for a connector whose endpoint this emulator serves relocates
/// the object on that server — the source path is absent afterwards and the target holds
/// the bytes the source held — and the record reports `COMPLETED`; a source the storage
/// does not hold is `FAILED` with the code the storage answered.
///
/// The rows differ along what the rule generalises over: the depth of the source and
/// target paths, whether the move is a rename inside one directory or a relocation
/// between two, whether the target directory already holds another object, and whether
/// the source is there at all.
#[tokio::test]
async fn a_remote_move_relocates_the_object_and_reports_what_the_attempt_did() {
    // (case, the objects to store first, source key, target key, the status expected)
    let rows: Vec<MoveRow<'_>> = vec![
        (
            "a file at the home directory's root, moved into a directory",
            vec![("source.txt", FILE_BODY)],
            "source.txt",
            "archive/source.txt",
            COMPLETED,
        ),
        (
            "a rename inside one directory",
            vec![("testdir/testfile.txt", FILE_BODY)],
            "testdir/testfile.txt",
            "testdir/moved.txt",
            COMPLETED,
        ),
        (
            "a move between two nested directories, the target holding another object",
            vec![
                ("outbox/deep/report.csv", FILE_BODY),
                ("inbox/deep/other.csv", OTHER_BODY),
            ],
            "outbox/deep/report.csv",
            "inbox/deep/report.csv",
            COMPLETED,
        ),
        (
            "a source the storage does not hold",
            vec![],
            "never-written.txt",
            "archive/never-written.txt",
            FAILED,
        ),
    ];

    for (case, stored, source, target_key, status) in rows {
        let (target, connector_id) = target_with_connector(case).await;
        for (key, body) in &stored {
            // Each row's bodies are the suite's own constants, so the stored object can
            // be compared with what the move leaves at the target.
            let body: &'static [u8] = if *body == OTHER_BODY { OTHER_BODY } else { FILE_BODY };
            target.put_object(key, body).await;
        }
        let bucket = target.bucket.clone();

        let move_id = accept(
            &target,
            case,
            "StartRemoteMove",
            json!({
                "ConnectorId": &connector_id,
                "SourcePath": format!("/{bucket}/{source}"),
                "TargetPath": format!("/{bucket}/{target_key}"),
            }),
            "MoveId",
        )
        .await;

        let result = settled_result(&target, case, &connector_id, &move_id).await;
        assert_outcome(case, &result, status);
        assert_eq!(
            member_of(&result, "FilePath").as_str(),
            Some(format!("/{bucket}/{source}").as_str()),
            "case {case:?}: the result reports the path the move addressed, got {result}"
        );

        if status == COMPLETED {
            assert_eq!(
                target.object(target_key).await.as_deref(),
                Some(FILE_BODY),
                "case {case:?}: the target must hold the bytes the source held"
            );
            assert!(
                target.object(source).await.is_none(),
                "case {case:?}: the source must be absent after the move"
            );
        } else {
            assert!(
                target.object(target_key).await.is_none(),
                "case {case:?}: a move that failed must write nothing at the target"
            );
        }
    }
}

/// RULE: a delete accepted for such a connector removes the path on that server — it is
/// absent afterwards — and the record reports `COMPLETED`; a path the storage does not
/// hold is `FAILED` with the code the storage answered, and nothing else is removed.
///
/// The rows differ along the depth of the path, whether a neighbouring object shares its
/// directory, and whether the path is there at all.
#[tokio::test]
async fn a_remote_delete_removes_the_path_and_reports_what_the_attempt_did() {
    // (case, the objects to store first, the path deleted, the status expected)
    let rows: Vec<(&str, Vec<&str>, &str, &str)> = vec![
        (
            "a file at the home directory's root",
            vec!["doomed.txt"],
            "doomed.txt",
            COMPLETED,
        ),
        (
            "a file in a nested directory, beside one that stays",
            vec!["testdir/moved.txt", "testdir/kept.txt"],
            "testdir/moved.txt",
            COMPLETED,
        ),
        (
            "a path the storage does not hold",
            vec!["testdir/kept.txt"],
            "testdir/absent.txt",
            FAILED,
        ),
    ];

    for (case, stored, deleted, status) in rows {
        let (target, connector_id) = target_with_connector(case).await;
        for key in &stored {
            target.put_object(key, FILE_BODY).await;
        }
        let bucket = target.bucket.clone();

        let delete_id = accept(
            &target,
            case,
            "StartRemoteDelete",
            json!({
                "ConnectorId": &connector_id,
                "DeletePath": format!("/{bucket}/{deleted}"),
            }),
            "DeleteId",
        )
        .await;

        let result = settled_result(&target, case, &connector_id, &delete_id).await;
        assert_outcome(case, &result, status);
        assert!(
            target.object(deleted).await.is_none() || status == FAILED,
            "case {case:?}: the deleted path must be absent afterwards"
        );
        // Whatever the outcome, the neighbour the row stored is untouched.
        if stored.contains(&"testdir/kept.txt") {
            assert_eq!(
                target.object("testdir/kept.txt").await.as_deref(),
                Some(FILE_BODY),
                "case {case:?}: a delete must remove only the path it addressed"
            );
        }
    }
}

/// RULE: a directory listing accepted for such a connector reads one level of the remote
/// directory and writes what it holds to the output file the call answered the name of —
/// each file by its path, each directory below it by its own — so a listing taken after
/// a delete no longer names the removed file.
///
/// The rows differ along what the listed directory holds: files only, files beside a
/// sub-directory, and a directory that holds nothing.
#[tokio::test]
async fn a_directory_listing_writes_what_the_remote_directory_holds() {
    // (case, the objects to store first, the directory listed, the paths the output must
    //  name, the paths it must not)
    let rows: Vec<ListingRow<'_>> = vec![
        (
            "a directory holding two files",
            vec!["testdir/one.txt", "testdir/two.txt"],
            "testdir",
            vec!["testdir/one.txt", "testdir/two.txt"],
            vec![],
        ),
        (
            "a directory holding a file and a sub-directory, whose contents are not listed",
            vec!["outbox/report.csv", "outbox/nested/deep.csv"],
            "outbox",
            vec!["outbox/report.csv", "outbox/nested"],
            vec!["outbox/nested/deep.csv"],
        ),
        (
            "a directory the storage holds nothing under",
            vec!["elsewhere/kept.txt"],
            "empty-dir",
            vec![],
            vec!["elsewhere/kept.txt"],
        ),
    ];

    for (case, stored, listed, named, not_named) in rows {
        let (target, connector_id) = target_with_connector(case).await;
        for key in &stored {
            target.put_object(key, FILE_BODY).await;
        }
        let bucket = target.bucket.clone();

        let answer = target
            .emulator
            .call(
                "StartDirectoryListing",
                json!({
                    "ConnectorId": &connector_id,
                    "RemoteDirectoryPath": format!("/{bucket}/{listed}"),
                    "OutputDirectoryPath": format!("/{bucket}/listing-output"),
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected StartDirectoryListing 200, got {} with body {}",
            answer.status, answer.body
        );
        let listing_id = answer
            .get("ListingId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ListingId, got {}", answer.body))
            .to_string();
        let output_file_name = answer
            .get("OutputFileName")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an OutputFileName, got {}", answer.body)
            })
            .to_string();
        // "The naming convention for the output file is connector-ID-listing-ID.json."
        assert_eq!(
            output_file_name,
            format!("{connector_id}-{listing_id}.json"),
            "case {case:?}: expected the documented output file name"
        );

        let result = settled_result(&target, case, &connector_id, &listing_id).await;
        assert_outcome(case, &result, COMPLETED);

        let written = target
            .object(&format!("listing-output/{output_file_name}"))
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must be written to the output directory")
            });
        let written = String::from_utf8(written).expect("the listing output is UTF-8");
        let document: Value =
            serde_json::from_str(&written).expect("the listing output is a JSON document");
        let reported: Vec<String> = document
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .chain(
                document
                    .get("paths")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            )
            .map(|entry| {
                member_of(entry, "filePath")
                    .as_str()
                    .or_else(|| member_of(entry, "path").as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        for key in &named {
            assert!(
                reported.contains(&format!("/{bucket}/{key}")),
                "case {case:?}: expected the listing to name {key}, got {reported:?}"
            );
        }
        for key in &not_named {
            assert!(
                !reported.contains(&format!("/{bucket}/{key}")),
                "case {case:?}: expected the listing NOT to name {key}, got {reported:?}"
            );
        }
        assert_eq!(
            document.get("truncated"),
            Some(&json!(false)),
            "case {case:?}: a listing inside the bound is not truncated, got {written}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an endpoint this emulator does not serve.
// ---------------------------------------------------------------------------

/// RULE: an operation accepted for a connector whose `Url` names an endpoint this
/// emulator does not serve is left exactly as the acceptance put it — `QUEUED`, with
/// nothing in storage changed — because no session could be opened to act over.
///
/// The rows differ along the endpoint named (a partner host outside the emulator, a
/// well-formed server endpoint this emulator holds no server for) and the operation
/// asked for (a move and a delete).
#[tokio::test]
async fn a_remote_operation_on_an_endpoint_nothing_serves_changes_nothing() {
    let endpoints = [
        ("a partner host outside this emulator", "sftp://partner.example.com".to_string()),
        (
            "a server endpoint this emulator holds no server for",
            server_endpoint("s-00000000000000000"),
        ),
    ];

    for (case, url) in endpoints {
        let target = Target::with_home_at_bucket_root().await;
        let secret = target
            .store_secret("aws/transfer/unserved", &target.user_name, &target.user_key.private)
            .await;
        let connector_id = target
            .create_connector(
                case,
                json!({"UserSecretId": &secret, "TrustedHostKeys": [&target.host_key]}),
                Some(&url),
            )
            .await;
        target.put_object("testdir/kept.txt", FILE_BODY).await;
        let bucket = target.bucket.clone();

        let move_id = accept(
            &target,
            case,
            "StartRemoteMove",
            json!({
                "ConnectorId": &connector_id,
                "SourcePath": format!("/{bucket}/testdir/kept.txt"),
                "TargetPath": format!("/{bucket}/testdir/moved.txt"),
            }),
            "MoveId",
        )
        .await;
        let delete_id = accept(
            &target,
            case,
            "StartRemoteDelete",
            json!({
                "ConnectorId": &connector_id,
                "DeletePath": format!("/{bucket}/testdir/kept.txt"),
            }),
            "DeleteId",
        )
        .await;

        tokio::time::sleep(PAST_TRANSFER_SPAN).await;
        for (operation, id) in [("the move", &move_id), ("the delete", &delete_id)] {
            for result in results(&target, &connector_id, id).await {
                assert_eq!(
                    member_of(&result, "StatusCode").as_str(),
                    Some(QUEUED),
                    "case {case:?}: {operation} ran against nothing, so its path stays \
                     accepted, got {result}"
                );
            }
        }
        assert_eq!(
            target.object("testdir/kept.txt").await.as_deref(),
            Some(FILE_BODY),
            "case {case:?}: nothing may be moved or removed for an operation nothing \
             performed"
        );
        assert!(
            target.object("testdir/moved.txt").await.is_none(),
            "case {case:?}: nothing may be written for an operation nothing performed"
        );
    }
}



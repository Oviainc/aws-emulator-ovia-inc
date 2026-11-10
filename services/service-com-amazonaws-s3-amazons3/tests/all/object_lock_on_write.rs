// SPDX-License-Identifier: CC0-1.0

//! The Object Lock an object WRITE asks for, recorded on the version it writes.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle, object and Object Lock suites in this crate already own), so what is
//! pinned is the wire behaviour: the bound headers a read reports, the status a delete of the
//! locked version answers, and what the bucket holds afterwards.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. "The Object Lock mode that you want to apply to this object" with "the date and time
//!    when you want this object's Object Lock to expire"
//!    (`PutObjectRequest$ObjectLockMode` and `$ObjectLockRetainUntilDate`), and "specifies
//!    whether a legal hold will be applied to this object"
//!    (`$ObjectLockLegalHoldStatus`) — so the write stores that lock ON the version it
//!    records, a later read reports it, and a delete of that version is judged against it:
//!    "if you try to delete objects protected by … retention and the retention period has not
//!    expired, the operation fails" (Amazon S3 User Guide, *S3 Object Lock*), while
//!    `x-amz-bypass-governance-retention` reaches a GOVERNANCE-mode period.
//! 2. Every write that declares those members records the same lock from the same values: the
//!    one-shot `PutObject`, the `CopyObject` whose members are "the Object Lock mode that you
//!    want to apply to the copied object", and the `CreateMultipartUpload` whose initiation
//!    "specif[ies] the options you want on the finished object", which the completion stores.
//! 3. A lock the service cannot record is refused and nothing is written: a retention mode
//!    without the date it runs to (and a date without a mode) with 400 `InvalidRequest`,
//!    because "a retention runs in a mode to a date", and any of the three members addressed
//!    to a bucket with no Object Lock configuration with 400 `InvalidRequest` and the
//!    sentence S3 publishes for that condition.
//!
//! The rows differ along what the rule generalises over: which member the write names and how
//! many at once, the retention MODE, how far the retain-until date runs, the legal hold
//! status, and which operation carries the members.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::{
    ACCESS_DENIED_CODE, COMPLIANCE_RETENTION_MODE, GOVERNANCE_RETENTION_MODE,
    INVALID_REQUEST_CODE, LEGAL_HOLD_OFF_STATUS, LEGAL_HOLD_ON_STATUS,
    OBJECT_LOCK_ACCESS_DENIED_MESSAGE, OBJECT_LOCK_CONFIGURATION_MISSING_ON_WRITE_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_legal_hold::lock_enabled_bucket;

/// The bytes every write in this suite stores, so a read-back shows the lock did not disturb
/// the payload.
const PAYLOAD: &[u8] = b"locked-payload";

/// How far from now a retention period that is still RUNNING ends, in seconds — an hour,
/// which is long enough that no row's delete can race its expiry.
const ONE_HOUR: i64 = 60 * 60;

/// `HEAD /<bucket>/<key>?versionId=<id>` — `HeadObject` of one stored version.
async fn head_version(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    version_id: &str,
) -> Answer {
    send(
        router,
        "HEAD",
        &format!("/{name}/{key}?versionId={version_id}"),
        &[],
        "",
    )
    .await
}

/// `DELETE /<bucket>/<key>?versionId=<id>` — the PERMANENT delete of one stored version, the
/// request an Object Lock is enforced against, carrying the governance bypass when asked for.
async fn delete_version(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    version_id: &str,
    bypass: bool,
) -> Answer {
    let headers: Vec<(&str, &str)> = if bypass {
        vec![("x-amz-bypass-governance-retention", "true")]
    } else {
        Vec::new()
    };
    send_bytes(
        router,
        "DELETE",
        &format!("/{name}/{key}?versionId={version_id}"),
        &headers,
        b"",
    )
    .await
}

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`.
async fn copy_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(router, "PUT", &format!("/{name}/{key}"), &all, b"").await
}

/// An instant `seconds` from now, in the spelling the framework's timestamp binding reads a
/// value in — which is what the retain-until-date member carries.
fn timestamp_in(seconds: i64) -> String {
    let now = DateTime::from(std::time::SystemTime::now()).secs();
    DateTime::from_secs(now + seconds)
        .fmt(Format::DateTime)
        .expect("an instant renders as a timestamp")
}

/// Which write carries the Object Lock members a row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `PutObject` — the one-shot write.
    Put,
    /// `CopyObject` — the write whose bytes come from another object.
    Copy,
    /// `CreateMultipartUpload` followed by `CompleteMultipartUpload`: the initiation declares
    /// the members and the completion stores them on the assembled version.
    Multipart,
}

/// One row: the case name, the write that carries the members, the retention the write asks
/// for (mode and how far from now its date runs, which is NEGATIVE for a period that has
/// already lapsed), the legal hold status it asks for, and whether a delete of the version
/// without a bypass must be refused.
struct Row {
    case: &'static str,
    write: Write,
    retention: Option<(&'static str, i64)>,
    legal_hold: Option<&'static str>,
    delete_refused: bool,
}

/// Write `PAYLOAD` at `key` through `write`, carrying `headers`, and report the version the
/// write recorded it as.
async fn write_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    write: Write,
    headers: &[(&str, &str)],
    case: &str,
) -> Answer {
    match write {
        Write::Put => put_object(router, name, key, PAYLOAD, headers).await,
        Write::Copy => {
            let source = "sources/to-copy.bin";
            let stored = put_object(router, name, source, PAYLOAD, &[]).await;
            assert_eq!(
                stored.status, 200,
                "case {case:?}: the copy's source must be written first, got {}",
                stored.status
            );
            copy_object(router, name, key, &format!("{name}/{source}"), headers).await
        }
        Write::Multipart => {
            let initiated = send(router, "POST", &format!("/{name}/{key}?uploads"), headers, "")
                .await;
            if initiated.status != 200 {
                return initiated;
            }
            let upload_id = initiated
                .element("UploadId")
                .expect("an initiation must report an UploadId");
            let uploaded = send_bytes(
                router,
                "PUT",
                &format!(
                    "/{name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"
                ),
                &[],
                PAYLOAD,
            )
            .await;
            assert_eq!(
                uploaded.status,
                200,
                "case {case:?}: the part must be uploaded, got {} with body {:?}",
                uploaded.status,
                text(&uploaded)
            );
            let etag = uploaded
                .header("etag")
                .expect("a part upload reports the part's ETag");
            send(
                router,
                "POST",
                &format!("/{name}/{key}?uploadId={upload_id}"),
                &[],
                &format!(
                    "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber>\
                     <ETag>{etag}</ETag></Part></CompleteMultipartUpload>"
                ),
            )
            .await
        }
    }
}

#[tokio::test]
async fn a_write_that_names_an_object_lock_stores_it_on_the_version_it_writes() {
    let router = router();
    let name = "lock-on-write-bucket";
    lock_enabled_bucket(&router, name).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a governance retention still running, on a one-shot write",
            write: Write::Put,
            retention: Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)),
            legal_hold: None,
            delete_refused: true,
        },
        Row {
            case: "a compliance retention still running, on a one-shot write",
            write: Write::Put,
            retention: Some((COMPLIANCE_RETENTION_MODE, ONE_HOUR)),
            legal_hold: None,
            delete_refused: true,
        },
        Row {
            case: "a legal hold ON, on a one-shot write",
            write: Write::Put,
            retention: None,
            legal_hold: Some(LEGAL_HOLD_ON_STATUS),
            delete_refused: true,
        },
        Row {
            case: "a legal hold OFF, which protects nothing",
            write: Write::Put,
            retention: None,
            legal_hold: Some(LEGAL_HOLD_OFF_STATUS),
            delete_refused: false,
        },
        Row {
            case: "a retention and a legal hold at once",
            write: Write::Put,
            retention: Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)),
            legal_hold: Some(LEGAL_HOLD_ON_STATUS),
            delete_refused: true,
        },
        Row {
            case: "a governance retention on a copy",
            write: Write::Copy,
            retention: Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)),
            legal_hold: None,
            delete_refused: true,
        },
        Row {
            case: "a legal hold on a copy",
            write: Write::Copy,
            retention: None,
            legal_hold: Some(LEGAL_HOLD_ON_STATUS),
            delete_refused: true,
        },
        Row {
            case: "a compliance retention a multipart initiation named",
            write: Write::Multipart,
            retention: Some((COMPLIANCE_RETENTION_MODE, ONE_HOUR)),
            legal_hold: None,
            delete_refused: true,
        },
        Row {
            case: "a legal hold a multipart initiation named",
            write: Write::Multipart,
            retention: None,
            legal_hold: Some(LEGAL_HOLD_ON_STATUS),
            delete_refused: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let key = format!("locked/row-{index}.bin");
        let until = row
            .retention
            .map(|(_, offset)| timestamp_in(offset))
            .unwrap_or_default();
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some((mode, _)) = row.retention {
            headers.push(("x-amz-object-lock-mode", mode));
            headers.push(("x-amz-object-lock-retain-until-date", until.as_str()));
        }
        if let Some(status) = row.legal_hold {
            headers.push(("x-amz-object-lock-legal-hold", status));
        }

        let written = write_object(&router, name, &key, row.write, &headers, case).await;
        assert_eq!(
            written.status,
            200,
            "case {case:?}: the write must be served, got {} with body {:?}",
            written.status,
            text(&written)
        );
        let version_id = written
            .header("x-amz-version-id")
            .unwrap_or_else(|| panic!("case {case:?}: the write must mint a version id"));

        // What a read of that version reports about its lock: the mode and the date the
        // retention runs to, and the hold status — through `GetObject` and `HeadObject`
        // alike, because both report the same stored version.
        let read = get_object(&router, name, &key, &format!("?versionId={version_id}"), &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: the locked version must read back, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.as_ref(),
            PAYLOAD,
            "case {case:?}: the lock must leave the payload alone"
        );
        let head = head_version(&router, name, &key, &version_id).await;
        for (reader, answer) in [("GetObject", &read), ("HeadObject", &head)] {
            assert_eq!(
                answer.header("x-amz-object-lock-mode"),
                row.retention.map(|(mode, _)| mode.to_string()),
                "case {case:?}: {reader} must report the mode the write named, got {:?}",
                answer.header("x-amz-object-lock-mode")
            );
            assert_eq!(
                answer.header("x-amz-object-lock-legal-hold"),
                row.legal_hold.map(str::to_string),
                "case {case:?}: {reader} must report the hold the write named, got {:?}",
                answer.header("x-amz-object-lock-legal-hold")
            );
            let reported_until = answer.header("x-amz-object-lock-retain-until-date");
            assert_eq!(
                reported_until.is_some(),
                row.retention.is_some(),
                "case {case:?}: {reader} must report a retain-until date exactly when the \
                 write named a retention, got {reported_until:?}"
            );
        }

        // And what the lock DOES: a permanent delete of the protected version is refused
        // with the published 403 and the version stays, while the same delete of a version
        // nothing protects is carried out.
        let deleted = delete_version(&router, name, &key, &version_id, false).await;
        if row.delete_refused {
            assert_eq!(
                deleted.status,
                403,
                "case {case:?}: a delete of the protected version must be refused, got {} \
                 with body {:?}",
                deleted.status,
                text(&deleted)
            );
            assert_eq!(
                deleted.error_code(),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected the published code, got {:?}",
                deleted.error_code()
            );
            assert_eq!(
                deleted.error_message(),
                OBJECT_LOCK_ACCESS_DENIED_MESSAGE,
                "case {case:?}: expected the published sentence, got {:?}",
                deleted.error_message()
            );
            let survived =
                get_object(&router, name, &key, &format!("?versionId={version_id}"), &[]).await;
            assert_eq!(
                survived.status, 200,
                "case {case:?}: the refused delete must leave the version stored, got {}",
                survived.status
            );
        } else {
            assert_eq!(
                deleted.status,
                204,
                "case {case:?}: a delete of a version nothing protects must be carried out, \
                 got {} with body {:?}",
                deleted.status,
                text(&deleted)
            );
        }

        // The governance bypass reaches a GOVERNANCE period and nothing else: a compliance
        // period and a legal hold are judged independently of it.
        if row.delete_refused {
            let bypassed = delete_version(&router, name, &key, &version_id, true).await;
            let reachable = row.legal_hold != Some(LEGAL_HOLD_ON_STATUS)
                && row
                    .retention
                    .is_none_or(|(mode, _)| mode == GOVERNANCE_RETENTION_MODE);
            let expected = if reachable { 204 } else { 403 };
            assert_eq!(
                bypassed.status, expected,
                "case {case:?}: expected HTTP {expected} for the bypassed delete, got {} \
                 with body {:?}",
                bypassed.status,
                text(&bypassed)
            );
        }
    }
}

/// One row of the refusal table: the case name, the bucket the write is addressed to, the
/// Object Lock headers it carries, and the `<Message>` the refusal must carry (empty where
/// the row pins the code and status alone).
type RefusalRow<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>, &'a str);

#[tokio::test]
async fn a_lock_the_service_cannot_record_is_refused_and_nothing_is_written() {
    let router = router();
    let locked = "lock-refusal-bucket";
    let unlocked = "lock-refusal-plain-bucket";
    lock_enabled_bucket(&router, locked).await;
    bucket(&router, unlocked).await;
    let until = timestamp_in(ONE_HOUR);

    // (case, bucket, headers, the message the refusal must carry). Every row is the same code
    // and status — `InvalidRequest`, 400 — because each names a lock no version can carry:
    // half a retention, or a lock on a bucket that locks nothing.
    let rows: Vec<RefusalRow<'_>> = vec![
        (
            "a mode without the date it runs to",
            locked,
            vec![("x-amz-object-lock-mode", GOVERNANCE_RETENTION_MODE)],
            "",
        ),
        (
            "a date without the mode it runs in",
            locked,
            vec![("x-amz-object-lock-retain-until-date", until.as_str())],
            "",
        ),
        (
            "a retention on a bucket with no Object Lock configuration",
            unlocked,
            vec![
                ("x-amz-object-lock-mode", COMPLIANCE_RETENTION_MODE),
                ("x-amz-object-lock-retain-until-date", until.as_str()),
            ],
            OBJECT_LOCK_CONFIGURATION_MISSING_ON_WRITE_MESSAGE,
        ),
        (
            "a legal hold on a bucket with no Object Lock configuration",
            unlocked,
            vec![("x-amz-object-lock-legal-hold", LEGAL_HOLD_ON_STATUS)],
            OBJECT_LOCK_CONFIGURATION_MISSING_ON_WRITE_MESSAGE,
        ),
    ];

    for (index, (case, name, headers, message)) in rows.iter().enumerate() {
        let key = format!("refused/row-{index}.bin");
        for write in [Write::Put, Write::Copy, Write::Multipart] {
            let key = format!("{key}.{write:?}");
            let refused = write_object(&router, name, &key, write, headers, case).await;
            assert_eq!(
                refused.status,
                400,
                "case {case:?} through {write:?}: expected HTTP 400, got {} with body {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                INVALID_REQUEST_CODE,
                "case {case:?} through {write:?}: expected the published code, got {:?}",
                refused.error_code()
            );
            if !message.is_empty() {
                assert_eq!(
                    refused.error_message(),
                    *message,
                    "case {case:?} through {write:?}: expected the published sentence, got \
                     {:?}",
                    refused.error_message()
                );
            }
            // Nothing was written: the key holds nothing at all afterwards.
            let absent = get_object(&router, name, &key, "", &[]).await;
            assert_eq!(
                absent.status, 404,
                "case {case:?} through {write:?}: a refused write must store nothing, got \
                 {}",
                absent.status
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ENFORCEMENT half of S3 Object Lock: what a lock placed on a
//! stored version does to a delete of that version, and what a read reports about it.
//!
//! Every assertion drives a real HTTP request through the router — the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_retention`] and [`super::object_legal_hold`]
//! already own — so what is pinned is the wire behaviour (status, error code, error message,
//! bound headers, what the store holds afterwards) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * A request that would PERMANENTLY remove a stored object version is refused with HTTP
//!   403 `AccessDenied` while that version is protected — a legal hold is on it, or its
//!   retention period is still running — and the version stays stored. "If you try to
//!   delete objects protected by … retention and the retention period has not expired, the
//!   operation fails" (Amazon S3 User Guide, *S3 Object Lock*). The protection is the
//!   version's: a delete that only lays a delete marker takes nothing away and is not
//!   refused.
//! * `x-amz-bypass-governance-retention` reaches a GOVERNANCE-mode retention and nothing
//!   else: a compliance-mode period and a legal hold are judged independently of it, the
//!   hold because it "provides the same protection as a retention period" with no
//!   expiration and is lifted only by `PutObjectLegalHold`.
//! * That bypass is a PRIVILEGED override, not a self-service one: "to override or remove
//!   governance-mode retention settings you must have the `s3:BypassGovernanceRetention`
//!   permission", so a caller the bucket's policy denies that action is refused and the
//!   retained version stays.
//! * A read of a version reports its lock: `ObjectLockMode` "the Object Lock mode that's
//!   currently in place for this object", `ObjectLockRetainUntilDate` "the date and time
//!   when this object's Object Lock will expire" and `ObjectLockLegalHoldStatus`, which "is
//!   not returned if the specified version of this object has never had a legal hold
//!   applied" — through `GetObject` and `HeadObject` alike.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::{
    caller_principal_arn, ACCESS_DENIED_CODE, COMPLIANCE_RETENTION_MODE,
    GOVERNANCE_RETENTION_MODE, LEGAL_HOLD_OFF_STATUS, LEGAL_HOLD_ON_STATUS,
    OBJECT_LOCK_ACCESS_DENIED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{put_bucket_policy, router_with_engine, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{get_object, text};
use super::object_legal_hold::{legal_hold_payload, lock_enabled_bucket, put_object_legal_hold};
use super::object_retention::{
    given_lapsed_retention, now_seconds, object, put_object_retention, retention_payload, ONE_HOUR,
};
use super::object_versioning::head_object;

/// The lock a row places on the version it then tries to delete: the retention (its mode
/// and how far from now its `RetainUntilDate` is, which is NEGATIVE for a period that has
/// already lapsed) and the legal hold status, each present only where the row names one.
type Lock = (Option<(&'static str, i64)>, Option<&'static str>);

/// `DELETE /<bucket>/<key>?versionId=<id>` — `DeleteObject` of one stored version, carrying
/// `x-amz-bypass-governance-retention` (the bound header of
/// `DeleteObjectRequest$BypassGovernanceRetention`) when the row asks to bypass.
async fn delete_version(
    router: &MultiServiceRouter,
    bucket: &str,
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
        &format!("/{bucket}/{key}?versionId={version_id}"),
        &headers,
        b"",
    )
    .await
}

/// Write an object at `key` and place the lock `lock` names on the version it stored,
/// failing the test when the service did not accept either. Reports that version's id.
///
/// A retention whose period has already LAPSED is seeded through the store rather than
/// placed over the wire ([`given_lapsed_retention`] states why), which is why this takes the
/// engine beside the router.
async fn locked_version(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    bucket: &str,
    key: &str,
    case: &str,
    lock: Lock,
) -> String {
    let version_id = object(router, bucket, key)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the write into {bucket:?} must mint a version id"));
    let (retention, legal_hold) = lock;
    if let Some((mode, offset)) = retention {
        let until = now_seconds() + offset;
        if offset < 0 {
            given_lapsed_retention(engine, bucket, key, mode, until);
        } else {
            let placed = put_object_retention(
                router,
                bucket,
                key,
                "",
                &[],
                &retention_payload(Some(mode), Some(until)),
            )
            .await;
            assert_eq!(
                placed.status, 200,
                "case {case:?}: the setup retention must be placed, got {} with body {:?}",
                placed.status,
                text(&placed)
            );
        }
    }
    if let Some(status) = legal_hold {
        let held =
            put_object_legal_hold(router, bucket, key, "", &[], &legal_hold_payload(Some(status)))
                .await;
        assert_eq!(
            held.status, 200,
            "case {case:?}: the setup legal hold must be applied, got {} with body {:?}",
            held.status,
            text(&held)
        );
    }
    version_id
}

/// Whether the bucket still holds the version `version_id` of `key`: a read of it answers
/// 200 while it is stored and 404 once it is gone.
async fn version_is_stored(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    version_id: &str,
    case: &str,
) -> bool {
    let read = get_object(
        router,
        bucket,
        key,
        &format!("?versionId={version_id}"),
        &[],
    )
    .await;
    match read.status {
        200 => true,
        404 => false,
        status => panic!(
            "case {case:?}: a read of the version must answer 200 or 404, got {status} with \
             body {:?}",
            text(&read)
        ),
    }
}

/// One row of the enforcement table: the case name, the key the version is stored at, the
/// lock placed on it, whether the delete asks to bypass governance-mode restrictions, and
/// the status the delete must answer.
type DeleteRow = (&'static str, &'static str, Lock, bool, u16);

/// Every way a lock can relate to the delete of the version it protects.
///
/// The rows differ along what the enforcement rule generalises over: WHICH protection is on
/// the version (a retention, a legal hold, both, neither), which MODE the retention is in
/// (both values of `ObjectLockRetentionMode`), whether the period is still RUNNING or has
/// lapsed, which legal hold STATUS is applied (both values of
/// `ObjectLockLegalHoldStatus`), and whether the delete asks to BYPASS.
fn delete_rows() -> Vec<DeleteRow> {
    vec![
        (
            "a governance period still running, no bypass asked",
            "governance-retained",
            (Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)), None),
            false,
            403,
        ),
        (
            "a compliance period still running, no bypass asked",
            "compliance-retained",
            (Some((COMPLIANCE_RETENTION_MODE, 24 * ONE_HOUR)), None),
            false,
            403,
        ),
        (
            "a compliance period, which no bypass reaches",
            "compliance-bypass-attempt",
            (Some((COMPLIANCE_RETENTION_MODE, ONE_HOUR)), None),
            true,
            403,
        ),
        (
            "a governance period and a bypass the caller may perform",
            "governance-bypassed",
            (Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)), None),
            true,
            204,
        ),
        (
            "a legal hold on, no retention at all",
            "held",
            (None, Some(LEGAL_HOLD_ON_STATUS)),
            false,
            403,
        ),
        (
            "a legal hold on, which the bypass does not lift",
            "held-bypass-attempt",
            (None, Some(LEGAL_HOLD_ON_STATUS)),
            true,
            403,
        ),
        (
            "a legal hold on over a governance period, bypass asked",
            "held-and-governance-retained",
            (
                Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)),
                Some(LEGAL_HOLD_ON_STATUS),
            ),
            true,
            403,
        ),
        (
            "a legal hold off, which protects nothing",
            "hold-released",
            (None, Some(LEGAL_HOLD_OFF_STATUS)),
            false,
            204,
        ),
        (
            "a governance period that has already lapsed",
            "retention-lapsed",
            (Some((GOVERNANCE_RETENTION_MODE, -ONE_HOUR)), None),
            false,
            204,
        ),
        (
            "a period that lapsed, under a legal hold that has not",
            "held-and-retention-lapsed",
            (
                Some((COMPLIANCE_RETENTION_MODE, -24 * ONE_HOUR)),
                Some(LEGAL_HOLD_ON_STATUS),
            ),
            true,
            403,
        ),
        ("no lock at all", "unlocked", (None, None), false, 204),
    ]
}

#[tokio::test]
async fn object_lock_bars_the_permanent_delete_of_a_protected_version() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "object-lock-delete").await;

    for (case, key, lock, bypass, expected) in delete_rows() {
        let version_id =
            locked_version(&router, &engine, "object-lock-delete", key, case, lock).await;

        let deleted = delete_version(&router, "object-lock-delete", key, &version_id, bypass).await;
        assert_eq!(
            deleted.status, expected,
            "case {case:?}: expected HTTP {expected} from the version-scoped delete, got {} \
             with body {:?}",
            deleted.status,
            text(&deleted)
        );

        if expected == 403 {
            // One authorization refusal, with the sentence S3 words for a call the lock
            // bars — fixed text that names neither the object, the mode nor the date.
            assert_eq!(
                deleted.error_code(),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected the {ACCESS_DENIED_CODE} code, got {:?} from body {:?}",
                deleted.error_code(),
                text(&deleted)
            );
            assert_eq!(
                deleted.error_message(),
                OBJECT_LOCK_ACCESS_DENIED_MESSAGE,
                "case {case:?}: expected the object-lock refusal sentence, got {:?}",
                deleted.error_message()
            );
        }

        // What the store holds afterwards is the point of the rule: a refused delete took
        // nothing away, and an allowed one really removed the version.
        let still_there =
            version_is_stored(&router, "object-lock-delete", key, &version_id, case).await;
        assert_eq!(
            still_there,
            expected == 403,
            "case {case:?}: after a HTTP {} delete the version must be {}, and it is {}",
            deleted.status,
            if expected == 403 { "stored" } else { "gone" },
            if still_there { "stored" } else { "gone" }
        );
    }
}

#[tokio::test]
async fn a_delete_marker_is_laid_over_a_locked_version_without_removing_it() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "object-lock-marker").await;

    // A delete that names NO version takes nothing away on a versioning-enabled bucket —
    // "the operation inserts a delete marker, which becomes the current version of the
    // object" — so the lock on the version it hides bars nothing. Asserted for both
    // protections, since each would bar a permanent delete of the same version.
    for (case, key, lock) in [
        (
            "a compliance period still running",
            "retained-then-marked",
            (Some((COMPLIANCE_RETENTION_MODE, 24 * ONE_HOUR)), None),
        ),
        (
            "a legal hold on",
            "held-then-marked",
            (None, Some(LEGAL_HOLD_ON_STATUS)),
        ),
    ] {
        let version_id =
            locked_version(&router, &engine, "object-lock-marker", key, case, lock).await;

        let deleted = send_bytes(
            &router,
            "DELETE",
            &format!("/object-lock-marker/{key}"),
            &[],
            b"",
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: a delete naming no version lays a marker and is not barred, got \
             {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert_eq!(
            deleted.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the delete must report the marker it laid, got {:?}",
            deleted.header("x-amz-delete-marker")
        );
        assert!(
            version_is_stored(&router, "object-lock-marker", key, &version_id, case).await,
            "case {case:?}: the locked version must still be addressable by its id"
        );
    }
}

/// One row of the bypass-authorization table: the case name, the key its version is stored
/// at, the bucket policy in force (`None` for the bucket carrying none), and the status the
/// bypass delete must answer.
type BypassRow = (&'static str, &'static str, Option<String>, u16);

/// A policy document with one statement: `effect` on `action` for `principal`, over
/// `resource`.
fn policy(effect: &str, principal: &str, action: &str, resource: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": principal,
            "Action": action,
            "Resource": resource,
        }],
    })
    .to_string()
}

/// Every way a bucket policy can relate to the privileged override a bypass delete asks
/// for. The rows differ along the policy's EFFECT, the PRINCIPAL it names (every principal,
/// the caller itself), the ACTION it is about and the RESOURCE it covers — the four
/// elements that decide whether a statement is about this request at all.
fn bypass_rows() -> Vec<BypassRow> {
    let bypass_action = "s3:BypassGovernanceRetention";
    let every_object = "arn:aws:s3:::bypass-permission/*";
    vec![
        (
            "a bucket carrying no policy, so nothing denies the caller",
            "no-policy",
            None,
            204,
        ),
        (
            "an explicit deny of the bypass to every principal",
            "denied-to-everyone",
            Some(policy("Deny", "*", bypass_action, every_object)),
            403,
        ),
        (
            "an explicit deny of the bypass naming the caller",
            "denied-to-the-caller",
            Some(policy(
                "Deny",
                &caller_principal_arn(),
                bypass_action,
                every_object,
            )),
            403,
        ),
        (
            "a deny of the bypass on another key, which this delete is not about",
            "denied-elsewhere",
            Some(policy(
                "Deny",
                "*",
                bypass_action,
                "arn:aws:s3:::bypass-permission/some-other-key",
            )),
            204,
        ),
        (
            "a deny of a different action, which the bypass is not",
            "denied-another-action",
            // An action nothing in this row's sequence is authorized under: the write of the
            // version and the delete of it are each judged against the policy too, so a row
            // about the BYPASS names an action neither of them needs.
            Some(policy("Deny", "*", "s3:GetObjectTagging", every_object)),
            204,
        ),
        (
            "an allow of the bypass to the caller",
            "allowed-to-the-caller",
            Some(policy(
                "Allow",
                &caller_principal_arn(),
                bypass_action,
                every_object,
            )),
            204,
        ),
    ]
}

#[tokio::test]
async fn a_governance_bypass_is_refused_to_a_caller_denied_the_permission() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "bypass-permission").await;

    for (case, key, document, expected) in bypass_rows() {
        if let Some(document) = document.as_deref() {
            // One policy per bucket, so each row's put replaces the row before it.
            let put = put_bucket_policy(&router, "bypass-permission", document).await;
            assert_eq!(
                put.status, 204,
                "case {case:?}: the row's bucket policy must be accepted, got {} with body {:?}",
                put.status,
                text(&put)
            );
        }

        let lock = (Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)), None);
        let version_id =
            locked_version(&router, &engine, "bypass-permission", key, case, lock).await;

        let deleted = delete_version(&router, "bypass-permission", key, &version_id, true).await;
        assert_eq!(
            deleted.status, expected,
            "case {case:?}: expected HTTP {expected} from the bypass delete, got {} with body \
             {:?}",
            deleted.status,
            text(&deleted)
        );
        if expected == 403 {
            assert_eq!(
                deleted.error_code(),
                ACCESS_DENIED_CODE,
                "case {case:?}: expected the {ACCESS_DENIED_CODE} code, got {:?}",
                deleted.error_code()
            );
            // The caller was refused the OVERRIDE, not by the lock: that is an
            // authorization decision about the bypass action on this object, worded as S3
            // words one — the principal, the action and the resource the request named,
            // spelled out here rather than taken from the handler's own builder.
            let expected_message = format!(
                "User: {} is not authorized to perform: s3:BypassGovernanceRetention on \
                 resource: \"arn:aws:s3:::bypass-permission/{key}\" with an explicit deny in a \
                 resource-based policy",
                caller_principal_arn()
            );
            assert_eq!(
                deleted.error_message(),
                expected_message,
                "case {case:?}: expected the explicit-deny refusal sentence, got {:?}",
                deleted.error_message()
            );
        }
        let still_there =
            version_is_stored(&router, "bypass-permission", key, &version_id, case).await;
        assert_eq!(
            still_there,
            expected == 403,
            "case {case:?}: after a HTTP {} bypass delete the version is {}",
            deleted.status,
            if still_there { "stored" } else { "gone" }
        );
    }
}

/// One row of the reported-lock table: the case name, the key, the lock placed on the
/// version, and the three header values a read of it must report (`None` for a header the
/// response must not carry).
type ReportRow = (
    &'static str,
    &'static str,
    Lock,
    Option<&'static str>,
    Option<i64>,
    Option<&'static str>,
);

/// Every lock state a read can report. The rows differ along which protection the version
/// carries, the retention's mode, how far out (or how far past) its date is, and which
/// legal hold status was applied.
fn report_rows() -> Vec<ReportRow> {
    vec![
        (
            "a governance period a day out",
            "reported-governance",
            (Some((GOVERNANCE_RETENTION_MODE, 24 * ONE_HOUR)), None),
            Some(GOVERNANCE_RETENTION_MODE),
            Some(24 * ONE_HOUR),
            None,
        ),
        (
            "a compliance period a decade out, under a legal hold",
            "reported-compliance-and-hold",
            (
                Some((COMPLIANCE_RETENTION_MODE, 10 * 365 * 24 * ONE_HOUR)),
                Some(LEGAL_HOLD_ON_STATUS),
            ),
            Some(COMPLIANCE_RETENTION_MODE),
            Some(10 * 365 * 24 * ONE_HOUR),
            Some(LEGAL_HOLD_ON_STATUS),
        ),
        (
            "a legal hold released, and no retention",
            "reported-hold-off",
            (None, Some(LEGAL_HOLD_OFF_STATUS)),
            None,
            None,
            Some(LEGAL_HOLD_OFF_STATUS),
        ),
        (
            "a period that has lapsed, which is still the record on the version",
            "reported-lapsed",
            (Some((GOVERNANCE_RETENTION_MODE, -ONE_HOUR)), None),
            Some(GOVERNANCE_RETENTION_MODE),
            Some(-ONE_HOUR),
            None,
        ),
        (
            "a version nothing was ever placed on",
            "reported-unlocked",
            (None, None),
            None,
            None,
            None,
        ),
    ]
}

#[tokio::test]
async fn a_read_reports_the_object_lock_state_of_the_version() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "object-lock-reported").await;

    for (case, key, lock, mode, offset, hold) in report_rows() {
        // The instant the row's date is an offset from, taken before the setup so the
        // expected value is the one the setup placed.
        let placed_at = now_seconds();
        let version_id =
            locked_version(&router, &engine, "object-lock-reported", key, case, lock).await;

        for (reader, answer) in [
            (
                "GetObject",
                get_object(
                    &router,
                    "object-lock-reported",
                    key,
                    &format!("?versionId={version_id}"),
                    &[],
                )
                .await,
            ),
            (
                "HeadObject",
                head_object(
                    &router,
                    "object-lock-reported",
                    key,
                    &format!("?versionId={version_id}"),
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {reader} of the version must answer 200, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.header("x-amz-object-lock-mode").as_deref(),
                mode,
                "case {case:?}: {reader} must report the mode {mode:?}, got {:?}",
                answer.header("x-amz-object-lock-mode")
            );
            assert_eq!(
                answer.header("x-amz-object-lock-legal-hold").as_deref(),
                hold,
                "case {case:?}: {reader} must report the legal hold {hold:?}, got {:?}",
                answer.header("x-amz-object-lock-legal-hold")
            );
            let reported = answer.header("x-amz-object-lock-retain-until-date");
            match offset {
                None => assert_eq!(
                    reported, None,
                    "case {case:?}: {reader} must report no retain-until date, got {reported:?}"
                ),
                Some(offset) => {
                    // Compared as INSTANTS, read back through the `date-time` format the
                    // header is bound with (`GetObjectOutput$ObjectLockRetainUntilDate`),
                    // so the row pins the date rather than one rendering of it.
                    let rendered = reported.clone().unwrap_or_else(|| {
                        panic!("case {case:?}: {reader} must report the retain-until date")
                    });
                    let seconds = DateTime::from_str(&rendered, Format::DateTime)
                        .unwrap_or_else(|error| {
                            panic!(
                                "case {case:?}: {reader} must report an ISO8601 date-time, got \
                                 {rendered:?} ({error})"
                            )
                        })
                        .secs();
                    assert_eq!(
                        seconds,
                        placed_at + offset,
                        "case {case:?}: {reader} must echo the stored RetainUntilDate, got \
                         {seconds} from {rendered:?}"
                    );
                }
            }
        }
    }
}

/// The `Error` entries of a `DeleteObjects` result as `(Key, Code, Message)`, in response
/// order.
fn error_entries(answer: &Answer) -> Vec<(String, String, String)> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    body.split("<Error>")
        .skip(1)
        .filter_map(|chunk| {
            let chunk = chunk.split("</Error>").next()?;
            Some((
                element_of(chunk, "Key")?,
                element_of(chunk, "Code")?,
                element_of(chunk, "Message")?,
            ))
        })
        .collect()
}

/// The `Deleted` entries' keys, in response order.
fn deleted_keys(answer: &Answer) -> Vec<String> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    body.split("<Deleted>")
        .skip(1)
        .filter_map(|chunk| element_of(chunk.split("</Deleted>").next()?, "Key"))
        .collect()
}

/// One element's text inside a result entry.
fn element_of(chunk: &str, name: &str) -> Option<String> {
    chunk
        .split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(text, _)| text.to_string())
}

#[tokio::test]
async fn a_batch_delete_refuses_the_locked_identifiers_and_deletes_the_rest() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "object-lock-batch").await;

    // Each identifier of a multi-object delete performs what a single delete of it would,
    // so the lock refuses THAT identifier and the keys beside it still delete.
    let retained = locked_version(
        &router,
        &engine,
        "object-lock-batch",
        "batch-retained",
        "batch",
        (Some((GOVERNANCE_RETENTION_MODE, ONE_HOUR)), None),
    )
    .await;
    let held = locked_version(
        &router,
        &engine,
        "object-lock-batch",
        "batch-held",
        "batch",
        (None, Some(LEGAL_HOLD_ON_STATUS)),
    )
    .await;
    let unlocked = locked_version(
        &router,
        &engine,
        "object-lock-batch",
        "batch-unlocked",
        "batch",
        (None, None),
    )
    .await;

    let payload = format!(
        "<Delete xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Object><Key>batch-retained</Key><VersionId>{retained}</VersionId></Object>\
         <Object><Key>batch-held</Key><VersionId>{held}</VersionId></Object>\
         <Object><Key>batch-unlocked</Key><VersionId>{unlocked}</VersionId></Object>\
         </Delete>"
    );
    let answer = send(
        &router,
        "POST",
        "/object-lock-batch?delete",
        &[],
        &payload,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the batch delete itself must succeed, got {} with body {:?}",
        answer.status,
        text(&answer)
    );

    let expected_errors = vec![
        (
            "batch-retained".to_string(),
            ACCESS_DENIED_CODE.to_string(),
            OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string(),
        ),
        (
            "batch-held".to_string(),
            ACCESS_DENIED_CODE.to_string(),
            OBJECT_LOCK_ACCESS_DENIED_MESSAGE.to_string(),
        ),
    ];
    assert_eq!(
        error_entries(&answer),
        expected_errors,
        "case \"batch\": the locked identifiers must each be their own refusal, got {:?} from \
         body {:?}",
        error_entries(&answer),
        text(&answer)
    );
    assert_eq!(
        deleted_keys(&answer),
        vec!["batch-unlocked".to_string()],
        "case \"batch\": only the unlocked identifier may delete, got {:?}",
        deleted_keys(&answer)
    );

    for (key, version_id, stored) in [
        ("batch-retained", &retained, true),
        ("batch-held", &held, true),
        ("batch-unlocked", &unlocked, false),
    ] {
        let still_there =
            version_is_stored(&router, "object-lock-batch", key, version_id, "batch").await;
        assert_eq!(
            still_there, stored,
            "case \"batch\": the version of {key:?} must be {}, and it is {}",
            if stored { "stored" } else { "gone" },
            if still_there { "stored" } else { "gone" }
        );
    }
}

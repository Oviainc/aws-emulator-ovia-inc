// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an object version's OBJECT LOCK LEGAL HOLD: `PutObjectLegalHold`
//! applies it and `GetObjectLegalHold` reports it back.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_retention`] already own), so what is pinned is the wire behaviour —
//! status, payload, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * An object-level Object Lock call — a legal hold or a retention, read or written — is
//!   served only by a bucket that HAS an Object Lock configuration: the configuration's
//!   rule "will be applied by default to every new object placed in the specified bucket"
//!   (`PutObjectLockConfiguration`'s own documentation), so a bucket carrying none locks
//!   nothing and such a call is refused with HTTP 400 `InvalidRequest` ("Bucket is missing
//!   Object Lock Configuration") without writing anything. A bucket created with
//!   `x-amz-bucket-object-lock-enabled` ("specifies whether you want S3 Object Lock to be
//!   enabled for the new bucket", `CreateBucketRequest$ObjectLockEnabledForBucket`) has
//!   one — and with it the versioning state Object Lock works in.
//! * `PutObjectLegalHold` "applies a legal hold configuration to the specified object", so
//!   the `Status` the `LegalHold` container carried reads back unchanged through
//!   `GetObjectLegalHold`, and a second put REPLACES the status rather than merging into
//!   it. The hold belongs to the addressed object VERSION — "the version ID of the object
//!   that you want to place a legal hold on"
//!   (`PutObjectLegalHoldRequest$VersionId`) — so it follows neither another version of the
//!   same key nor another key, and it leaves the object's bytes alone.
//! * A call whose address this emulator does not hold answers which part is missing —
//!   `NoSuchBucket`, `NoSuchKey` or `NoSuchVersion`, each HTTP 404 — and writes nothing; a
//!   version no legal hold has ever been applied to answers the subresource-not-there 404
//!   rather than an empty 200, because a status "is not returned if the specified version
//!   of this object has never had a legal hold applied"
//!   (`HeadObjectOutput$ObjectLockLegalHoldStatus`).
//! * A container that names no status describes no configuration to apply, and so does one
//!   naming a status outside `ObjectLockLegalHoldStatus`: both are refused with HTTP 400.
//! * An `x-amz-expected-bucket-owner` that is not the account this emulator serves is "403
//!   Forbidden (access denied)" on both halves, and writes nothing.

use service_com_amazonaws_s3_amazons3::ops::types::OBJECT_LOCK_LEGAL_HOLD_STATUS_VALUES;
use service_com_amazonaws_s3_amazons3::state::{
    GOVERNANCE_RETENTION_MODE, LEGAL_HOLD_OFF_STATUS, LEGAL_HOLD_ON_STATUS,
    OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, text};
use super::object_retention::{
    get_object_retention, now_seconds, object, put_object_retention, retention_payload, ONE_HOUR,
};
use super::object_versioning::delete_object;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// The bytes [`object`] writes, so a read-back can show that locking a version left its
/// payload alone.
const PAYLOAD: &[u8] = b"payload";

/// A bucket with an S3 Object Lock configuration, which is what an object-level Object Lock
/// call needs: `CreateBucket` with `x-amz-bucket-object-lock-enabled`, the bound header of
/// `CreateBucketRequest$ObjectLockEnabledForBucket`.
///
/// `pub(crate)`: the retention suite ([`super::object_retention`]) holds its versions in
/// such a bucket too, and one helper keeps the two from drifting apart.
pub(crate) async fn lock_enabled_bucket(router: &MultiServiceRouter, name: &str) {
    let created = send(
        router,
        "PUT",
        &format!("/{name}"),
        &[("x-amz-bucket-object-lock-enabled", "true")],
        "",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the Object Lock enabled bucket {name:?} must be created first, got {} with body \
         {:?}",
        created.status,
        text(&created)
    );
}

/// `PUT /<bucket>/<key>?legal-hold` — `PutObjectLegalHold`, with whatever extra query
/// parameters and headers the case carries and `payload` as the `LegalHold` container.
///
/// `pub(crate)`, like [`lock_enabled_bucket`] above: the Object Lock ENFORCEMENT suite
/// ([`super::object_lock_enforcement`]) places the holds it then tries to delete under
/// through this one helper rather than forking a second copy of it.
pub(crate) async fn put_object_legal_hold(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?legal-hold{query}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>/<key>?legal-hold` — `GetObjectLegalHold`, with whatever extra query
/// parameters and headers the case carries.
async fn get_object_legal_hold(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}/{key}?legal-hold{query}"),
        headers,
        "",
    )
    .await
}

/// The `LegalHold` container a put carries: the status it names, and nothing else.
///
/// An empty container (no `Status` at all) names no configuration to apply, which is one of
/// the refusal rows below.
///
/// `pub(crate)` for the same reason as the request helper above: one spelling of this
/// container across the suites that place a legal hold.
pub(crate) fn legal_hold_payload(status: Option<&str>) -> String {
    let mut body = String::from("<LegalHold xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    if let Some(status) = status {
        body.push_str(&format!("<Status>{status}</Status>"));
    }
    body.push_str("</LegalHold>");
    body
}

/// A governance-mode `Retention` container running to a day from now — what the retention
/// rows of the bucket-configuration table carry, built by the retention suite's own payload
/// helper so the two suites cannot describe one container two ways.
fn a_days_governance_retention() -> String {
    retention_payload(
        Some(GOVERNANCE_RETENTION_MODE),
        Some(now_seconds() + 24 * ONE_HOUR),
    )
}

/// The legal hold status a `GetObjectLegalHold` reported, or `None` when the answer carried
/// no `Status` element at all.
fn reported_status(answer: &Answer) -> Option<String> {
    answer.element("Status")
}

/// One row of the round-trip table: the case name, the status the first put applies, and
/// the status the second one replaces it with.
type RoundTripRow<'a> = (&'a str, &'a str, &'a str);

/// Every way one put can relate to the status already on the version: each published status
/// applied first, then each published status put over it — so the table covers the whole
/// catalog in both positions, a replacement that changes the status and one that repeats it.
const ROUND_TRIP_TABLE: &[RoundTripRow] = &[
    ("a hold placed, then lifted", LEGAL_HOLD_ON_STATUS, LEGAL_HOLD_OFF_STATUS),
    ("a hold placed, then placed again", LEGAL_HOLD_ON_STATUS, LEGAL_HOLD_ON_STATUS),
    ("no hold, then one placed", LEGAL_HOLD_OFF_STATUS, LEGAL_HOLD_ON_STATUS),
    ("no hold, then none again", LEGAL_HOLD_OFF_STATUS, LEGAL_HOLD_OFF_STATUS),
];

#[tokio::test]
async fn a_legal_hold_round_trips_through_put_and_get_object_legal_hold() {
    let router = router();
    lock_enabled_bucket(&router, "object-legal-hold").await;

    // The rows span the whole published status catalog, so the table cannot quietly stop
    // covering a value the model names.
    let mut covered: Vec<&str> = ROUND_TRIP_TABLE
        .iter()
        .flat_map(|(_, first, second)| [*first, *second])
        .collect();
    covered.sort_unstable();
    covered.dedup();
    let mut published: Vec<&str> = OBJECT_LOCK_LEGAL_HOLD_STATUS_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        covered, published,
        "the table must cover every ObjectLockLegalHoldStatus the model publishes, got \
         {covered:?}"
    );

    for (case, first, second) in ROUND_TRIP_TABLE {
        let key = format!("held-{}-{}", first.to_lowercase(), second.to_lowercase());
        object(&router, "object-legal-hold", &key).await;

        let applied = put_object_legal_hold(
            &router,
            "object-legal-hold",
            &key,
            "",
            &[],
            &legal_hold_payload(Some(first)),
        )
        .await;
        assert_eq!(
            applied.status, 200,
            "case {case:?}: expected the modelled 200 from PutObjectLegalHold, got {} with \
             body {:?}",
            applied.status,
            text(&applied)
        );

        let read = get_object_legal_hold(&router, "object-legal-hold", &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetObjectLegalHold on the version just \
             locked, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            reported_status(&read).as_deref(),
            Some(*first),
            "case {case:?}: GetObjectLegalHold must report exactly the status applied, got \
             {:?}",
            reported_status(&read)
        );

        // A second put REPLACES the status: what the version carries afterwards is the one
        // this call named, whether that differs from the status before it or repeats it.
        let replaced = put_object_legal_hold(
            &router,
            "object-legal-hold",
            &key,
            "",
            &[],
            &legal_hold_payload(Some(second)),
        )
        .await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: a legal hold may be replaced, got {} with body {:?}",
            replaced.status,
            text(&replaced)
        );
        let reread = get_object_legal_hold(&router, "object-legal-hold", &key, "", &[]).await;
        assert_eq!(
            reported_status(&reread).as_deref(),
            Some(*second),
            "case {case:?}: the second put must replace the status, got {:?}",
            reported_status(&reread)
        );

        // Applying a legal hold locks a version; it does not touch the bytes it holds.
        let object_read = get_object(&router, "object-legal-hold", &key, "", &[]).await;
        assert_eq!(
            object_read.body.as_ref(),
            PAYLOAD,
            "case {case:?}: applying a legal hold must not touch the object's bytes, got {:?}",
            text(&object_read)
        );
    }
}

/// One row of the bucket-configuration table: the operation's name and the answer the call
/// produced.
type ObjectLockCall<'a> = (&'a str, Answer);

#[tokio::test]
async fn an_object_lock_call_needs_a_bucket_with_an_object_lock_configuration() {
    let router = router();
    // A bucket created the ordinary way carries NO Object Lock configuration; the one
    // beside it was created with Object Lock enabled.
    bucket(&router, "unlocked-bucket").await;
    object(&router, "unlocked-bucket", "held").await;
    lock_enabled_bucket(&router, "locked-bucket").await;
    object(&router, "locked-bucket", "held").await;

    let hold = legal_hold_payload(Some(LEGAL_HOLD_ON_STATUS));
    let retention = a_days_governance_retention();

    // Every object-level Object Lock call, read and written, over the bucket with no
    // configuration: each is a request S3 cannot carry out as given.
    let refused: [ObjectLockCall; 4] = [
        (
            "PutObjectLegalHold",
            put_object_legal_hold(&router, "unlocked-bucket", "held", "", &[], &hold).await,
        ),
        (
            "GetObjectLegalHold",
            get_object_legal_hold(&router, "unlocked-bucket", "held", "", &[]).await,
        ),
        (
            "PutObjectRetention",
            put_object_retention(&router, "unlocked-bucket", "held", "", &[], &retention).await,
        ),
        (
            "GetObjectRetention",
            get_object_retention(&router, "unlocked-bucket", "held", "", &[]).await,
        ),
    ];
    for (operation, answer) in &refused {
        assert_eq!(
            answer.status, 400,
            "case {operation}: a bucket with no Object Lock configuration cannot serve the \
             call, got {} with body {:?}",
            answer.status,
            text(answer)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequest",
            "case {operation}: expected the request-cannot-be-carried-out code, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            OBJECT_LOCK_CONFIGURATION_MISSING_MESSAGE,
            "case {operation}: expected the documented message, got {:?}",
            answer.error_message()
        );
    }

    // The refused writes left the object exactly as it was: its bytes are still there, and
    // moving the same calls to the bucket that HAS a configuration serves every one of them.
    let untouched = get_object(&router, "unlocked-bucket", "held", "", &[]).await;
    assert_eq!(
        untouched.body.as_ref(),
        PAYLOAD,
        "a refused Object Lock call must leave the object alone, got {:?}",
        text(&untouched)
    );

    let served: [ObjectLockCall; 4] = [
        (
            "PutObjectLegalHold",
            put_object_legal_hold(&router, "locked-bucket", "held", "", &[], &hold).await,
        ),
        (
            "GetObjectLegalHold",
            get_object_legal_hold(&router, "locked-bucket", "held", "", &[]).await,
        ),
        (
            "PutObjectRetention",
            put_object_retention(&router, "locked-bucket", "held", "", &[], &retention).await,
        ),
        (
            "GetObjectRetention",
            get_object_retention(&router, "locked-bucket", "held", "", &[]).await,
        ),
    ];
    for (operation, answer) in &served {
        assert_eq!(
            answer.status, 200,
            "case {operation}: a bucket with an Object Lock configuration serves the call, \
             got {} with body {:?}",
            answer.status,
            text(answer)
        );
    }
}

#[tokio::test]
async fn enabling_object_lock_on_a_create_enables_the_versioning_it_works_in() {
    let router = router();
    lock_enabled_bucket(&router, "locked-and-versioned").await;

    // "Object Lock works only in versioned buckets", so the create that enabled Object Lock
    // left the bucket versioning-enabled — which is what `GetBucketVersioning` reports and
    // what makes a stored version addressable by an id a legal hold can be placed on.
    let versioning = send(&router, "GET", "/locked-and-versioned?versioning", &[], "").await;
    assert_eq!(
        versioning.status, 200,
        "the versioning state of an Object Lock enabled bucket must be readable, got {} \
         with body {:?}",
        versioning.status,
        text(&versioning)
    );
    assert_eq!(
        versioning.element("Status").as_deref(),
        Some("Enabled"),
        "a bucket created with Object Lock enabled is versioning-enabled, got {:?}",
        text(&versioning)
    );
    assert!(
        object(&router, "locked-and-versioned", "versioned")
            .await
            .is_some(),
        "a write to a versioning-enabled bucket must mint a version id"
    );
}

#[tokio::test]
async fn a_legal_hold_belongs_to_one_object_version() {
    let router = router();
    lock_enabled_bucket(&router, "legal-hold-versions").await;

    // Two versions of one key, and a key beside them: each stored version is addressable on
    // its own, which is what a legal hold is addressed by.
    let first = object(&router, "legal-hold-versions", "versioned")
        .await
        .expect("the first write must mint a version id");
    let second = object(&router, "legal-hold-versions", "versioned")
        .await
        .expect("the second write must mint a version id");
    assert_ne!(
        first, second,
        "two writes of one key must mint two version ids, got {first:?} twice"
    );
    object(&router, "legal-hold-versions", "untouched").await;

    // Applied with no `versionId`, the hold goes on the CURRENT version: the one the write
    // just made current.
    let applied = put_object_legal_hold(
        &router,
        "legal-hold-versions",
        "versioned",
        "",
        &[],
        &legal_hold_payload(Some(LEGAL_HOLD_ON_STATUS)),
    )
    .await;
    assert_eq!(
        applied.status, 200,
        "the legal hold on the current version must be applied, got {} with body {:?}",
        applied.status,
        text(&applied)
    );

    // What each address reports: the current version and its id both carry the hold, and
    // nothing else does.
    for (case, query, expected) in [
        ("no versionId", String::new(), Some(LEGAL_HOLD_ON_STATUS)),
        (
            "the current version's id",
            format!("&versionId={second}"),
            Some(LEGAL_HOLD_ON_STATUS),
        ),
        ("the noncurrent version's id", format!("&versionId={first}"), None),
    ] {
        let read =
            get_object_legal_hold(&router, "legal-hold-versions", "versioned", &query, &[]).await;
        match expected {
            Some(status) => assert_eq!(
                reported_status(&read).as_deref(),
                Some(status),
                "case {case:?}: expected the status the hold placed, got {} with body {:?}",
                read.status,
                text(&read)
            ),
            None => assert_eq!(
                read.error_code(),
                "NoSuchObjectLockConfiguration",
                "case {case:?}: a version no hold was applied to has none to report, got {} \
                 with body {:?}",
                read.status,
                text(&read)
            ),
        }
    }

    // A hold on the NONCURRENT version is its own: lifting one there leaves the current
    // version's in place.
    let noncurrent = put_object_legal_hold(
        &router,
        "legal-hold-versions",
        "versioned",
        &format!("&versionId={first}"),
        &[],
        &legal_hold_payload(Some(LEGAL_HOLD_OFF_STATUS)),
    )
    .await;
    assert_eq!(
        noncurrent.status, 200,
        "a legal hold may be applied to a noncurrent version, got {} with body {:?}",
        noncurrent.status,
        text(&noncurrent)
    );
    let noncurrent_read = get_object_legal_hold(
        &router,
        "legal-hold-versions",
        "versioned",
        &format!("&versionId={first}"),
        &[],
    )
    .await;
    assert_eq!(
        reported_status(&noncurrent_read).as_deref(),
        Some(LEGAL_HOLD_OFF_STATUS),
        "the noncurrent version must carry the status applied to it, got {:?}",
        reported_status(&noncurrent_read)
    );
    let current_read =
        get_object_legal_hold(&router, "legal-hold-versions", "versioned", "", &[]).await;
    assert_eq!(
        reported_status(&current_read).as_deref(),
        Some(LEGAL_HOLD_ON_STATUS),
        "locking one version must leave another's hold untouched, got {:?}",
        reported_status(&current_read)
    );

    // And no other key was touched: the key beside it still has no hold at all.
    let untouched =
        get_object_legal_hold(&router, "legal-hold-versions", "untouched", "", &[]).await;
    assert_eq!(
        untouched.status, 404,
        "a legal hold belongs to one key, so the key beside it carries none, got {} with \
         body {:?}",
        untouched.status,
        text(&untouched)
    );
}

#[tokio::test]
async fn a_legal_hold_call_this_emulator_cannot_serve_answers_what_is_wrong() {
    let router = router();
    lock_enabled_bucket(&router, "held-legal-hold-bucket").await;
    object(&router, "held-legal-hold-bucket", "held").await;
    let payload = legal_hold_payload(Some(LEGAL_HOLD_ON_STATUS));

    // A version no hold has ever been applied to: the subresource is not there, so the read
    // answers 404 rather than an empty 200 or a status nobody set.
    let absent = get_object_legal_hold(&router, "held-legal-hold-bucket", "held", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a version with no legal hold applied has none to report, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchObjectLockConfiguration",
        "expected the subresource-not-there code, got {:?}",
        absent.error_code()
    );

    // A bucket this emulator does not hold: a different name, a prefix of the held one, an
    // extension of it, and one differing only in case (bucket names are case-sensitive).
    for missing in [
        "absent-bucket",
        "held-legal-hold",
        "held-legal-hold-bucket-2",
        "Held-Legal-Hold-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutObjectLegalHold",
                put_object_legal_hold(&router, missing, "held", "", &[], &payload).await,
            ),
            (
                "GetObjectLegalHold",
                get_object_legal_hold(&router, missing, "held", "", &[]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {missing:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {missing:?}/{operation}: expected the bucket to be named as the \
                 missing part, got {:?}",
                answer.error_code()
            );
        }
    }

    // A key the held bucket does not hold, in the shapes a client sends.
    for missing in ["absent-key", "hel", "held-2", "Held", "locked/held"] {
        for (operation, answer) in [
            (
                "PutObjectLegalHold",
                put_object_legal_hold(
                    &router,
                    "held-legal-hold-bucket",
                    missing,
                    "",
                    &[],
                    &payload,
                )
                .await,
            ),
            (
                "GetObjectLegalHold",
                get_object_legal_hold(&router, "held-legal-hold-bucket", missing, "", &[]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {missing:?}/{operation}: expected HTTP 404 for a key this bucket does \
                 not hold, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchKey",
                "case {missing:?}/{operation}: expected the key to be named as the missing \
                 part, got {:?}",
                answer.error_code()
            );
        }
    }

    // A `versionId` the bucket holds no version of that key for: the key itself is there,
    // so what is missing is the version. The ids below are the shapes a client sends — a
    // generated-looking one, one that is not a minted id at all, and the literal `null`
    // that names the version of a key written before versioning was enabled, which this
    // Object Lock enabled bucket has never held.
    for missing in ["3HL4kqtJlcpXroDTDmjVBH40Nrjfkd", "not-a-minted-id", "null"] {
        for (operation, answer) in [
            (
                "PutObjectLegalHold",
                put_object_legal_hold(
                    &router,
                    "held-legal-hold-bucket",
                    "held",
                    &format!("&versionId={missing}"),
                    &[],
                    &payload,
                )
                .await,
            ),
            (
                "GetObjectLegalHold",
                get_object_legal_hold(
                    &router,
                    "held-legal-hold-bucket",
                    "held",
                    &format!("&versionId={missing}"),
                    &[],
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {missing:?}/{operation}: expected HTTP 404 for a version this bucket \
                 does not hold, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchVersion",
                "case {missing:?}/{operation}: expected the version to be named as the \
                 missing part, got {:?}",
                answer.error_code()
            );
        }
    }

    // A delete that names no version inserts a delete marker, which becomes the current
    // version and has no object behind it to carry a hold: an object-level call addressed
    // to it is 405.
    object(&router, "held-legal-hold-bucket", "marked").await;
    let deleted = delete_object(&router, "held-legal-hold-bucket", "marked", "").await;
    assert_eq!(
        deleted.status, 204,
        "the setup delete must insert a delete marker, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    let marker = deleted
        .header("x-amz-version-id")
        .expect("the delete marker must report its version id");
    for (operation, answer) in [
        (
            "PutObjectLegalHold",
            put_object_legal_hold(
                &router,
                "held-legal-hold-bucket",
                "marked",
                &format!("&versionId={marker}"),
                &[],
                &payload,
            )
            .await,
        ),
        (
            "GetObjectLegalHold",
            get_object_legal_hold(
                &router,
                "held-legal-hold-bucket",
                "marked",
                &format!("&versionId={marker}"),
                &[],
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.status, 405,
            "case {operation}: a delete marker carries no legal hold, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
    }

    // A container that names no status describes no configuration to apply, and neither
    // does one naming a status outside the published catalog.
    for (case, body) in [
        ("an empty container", legal_hold_payload(None)),
        ("no container at all", String::new()),
        (
            "a status outside the published catalog",
            legal_hold_payload(Some("HOLD")),
        ),
    ] {
        let refused =
            put_object_legal_hold(&router, "held-legal-hold-bucket", "held", "", &[], &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a legal hold naming no status to be refused with HTTP \
             400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
    }

    // An `x-amz-expected-bucket-owner` that is not this emulator's account is "403
    // Forbidden (access denied)" for both halves.
    assert_ne!(
        NOT_THE_OWNER,
        engine::emulator_account_id(),
        "the test's non-owner account id must not be the account this emulator serves"
    );
    let owner = [("x-amz-expected-bucket-owner", NOT_THE_OWNER)];
    for (operation, answer) in [
        (
            "PutObjectLegalHold",
            put_object_legal_hold(
                &router,
                "held-legal-hold-bucket",
                "held",
                "",
                &owner,
                &payload,
            )
            .await,
        ),
        (
            "GetObjectLegalHold",
            get_object_legal_hold(&router, "held-legal-hold-bucket", "held", "", &owner).await,
        ),
    ] {
        assert_eq!(
            answer.status, 403,
            "case {operation}: an expected bucket owner that is not the owner is denied, got \
             {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {operation}: expected the documented code, got {:?}",
            answer.error_code()
        );
    }

    // None of the refusals above wrote anything: the held version still has no legal hold
    // applied to it at all.
    let still_absent =
        get_object_legal_hold(&router, "held-legal-hold-bucket", "held", "", &[]).await;
    assert_eq!(
        still_absent.error_code(),
        "NoSuchObjectLockConfiguration",
        "a refused call must not have locked the held version, got {} with body {:?}",
        still_absent.status,
        text(&still_absent)
    );
}


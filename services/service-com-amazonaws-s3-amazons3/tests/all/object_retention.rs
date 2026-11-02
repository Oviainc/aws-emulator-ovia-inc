// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an object version's OBJECT LOCK RETENTION: `PutObjectRetention`
//! places it and `GetObjectRetention` reports it back.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_versioning`] already own), so what is pinned is the wire behaviour —
//! status, payload, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutObjectRetention` "places an Object Retention configuration on an object", so the
//!   `Mode` and `RetainUntilDate` the request carried read back unchanged through
//!   `GetObjectRetention`, and a second put REPLACES the configuration rather than merging
//!   into it. The retention belongs to the addressed object VERSION — "the version ID for
//!   the object that you want to apply this Object Retention configuration to"
//!   (`PutObjectRetentionRequest$VersionId`) — so it follows neither another version of the
//!   same key nor another key.
//! * A call that would leave the version protected LESS than the retention in force on it
//!   protects it — ending the period earlier, taking the retention away, or putting
//!   `GOVERNANCE` where `COMPLIANCE` is in force — is refused with HTTP 403 `AccessDenied`,
//!   unless the retention in force is a governance one and the request asks to bypass it
//!   ("Bypassing a Governance Retention configuration requires the
//!   s3:BypassGovernanceRetention permission", and
//!   `PutObjectRetentionRequest$BypassGovernanceRetention` "indicates whether this action
//!   should bypass Governance-mode restrictions"). Lengthening the period, or replacing a
//!   retention whose date has passed, needs nothing.
//! * A call whose address this emulator does not hold answers which part is missing —
//!   `NoSuchBucket`, `NoSuchKey` or `NoSuchVersion`, each HTTP 404 — and writes nothing; a
//!   version with no retention associated with it answers the subresource-not-there 404
//!   rather than an empty 200.
//! * An `x-amz-expected-bucket-owner` that is not the account this emulator serves is "403
//!   Forbidden (access denied)" on both halves, and writes nothing.

use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::ops::types::OBJECT_LOCK_RETENTION_MODE_VALUES;
use service_com_amazonaws_s3_amazons3::state::{
    ComAmazonawsS3Amazons3State, ObjectRetention, ObjectRetentionWrite, COMPLIANCE_RETENTION_MODE,
    GOVERNANCE_RETENTION_MODE, OBJECT_LOCK_ACCESS_DENIED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, router_with_engine, send, Answer};
use super::object_and_tagging_roundtrip::{get_object, put_object, text};
use super::object_legal_hold::lock_enabled_bucket;

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has. Asserted
/// against the emulator's own id where it is used, so it cannot silently become the owner.
const NOT_THE_OWNER: &str = "123456789012";

/// One hour in seconds — the step the shortening and lengthening rows move a retention
/// period by.
///
/// `pub(crate)`, like the four helpers below it: the legal-hold suite
/// ([`super::object_legal_hold`]) drives the retention half of the Object Lock family
/// through these rather than forking a second copy of them.
pub(crate) const ONE_HOUR: i64 = 3_600;

/// `PUT /<bucket>/<key>?retention` — `PutObjectRetention`, with whatever extra query
/// parameters and headers the case carries and `payload` as the `Retention` container.
pub(crate) async fn put_object_retention(
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
        &format!("/{bucket}/{key}?retention{query}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>/<key>?retention` — `GetObjectRetention`, with whatever extra query
/// parameters and headers the case carries.
pub(crate) async fn get_object_retention(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}/{key}?retention{query}"),
        headers,
        "",
    )
    .await
}

/// The `Retention` container a put carries: the members it names, and nothing else.
///
/// An empty container (both members `None`) is a container that describes no period, which
/// is how a caller asks for the retention to be taken away.
pub(crate) fn retention_payload(mode: Option<&str>, retain_until_seconds: Option<i64>) -> String {
    let mut body = String::from("<Retention xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    if let Some(mode) = mode {
        body.push_str(&format!("<Mode>{mode}</Mode>"));
    }
    if let Some(seconds) = retain_until_seconds {
        body.push_str(&format!(
            "<RetainUntilDate>{}</RetainUntilDate>",
            iso8601(seconds)
        ));
    }
    body.push_str("</Retention>");
    body
}

/// `epoch_seconds` as the ISO8601 `date-time` the wire carries it in —
/// `ObjectLockRetention$RetainUntilDate` is a timestamp whose `timestampFormat` is
/// `date-time` (`smithy_query(query="shape",shape="ObjectLockRetention")`).
fn iso8601(epoch_seconds: i64) -> String {
    DateTime::from_secs(epoch_seconds)
        .fmt(Format::DateTime)
        .expect("the test's own date must render as an ISO8601 date-time")
}

/// The epoch seconds a reported `RetainUntilDate` names, read back through the same
/// `date-time` format it is carried in — so a row compares INSTANTS rather than two
/// renderings of one.
fn epoch_seconds(rendered: &str) -> i64 {
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!(
                "the reported RetainUntilDate {rendered:?} must be an ISO8601 date-time, \
                 got {error}"
            )
        })
        .secs()
}

/// Now, in epoch seconds: what every row's retention date is an offset from.
pub(crate) fn now_seconds() -> i64 {
    DateTime::from(SystemTime::now()).secs()
}

/// Seed a retention whose period has ALREADY LAPSED onto the current version of
/// `bucket`/`key`, through the same store the service reads.
///
/// Not over the wire, because no request can place one: a `RetainUntilDate` that is not in
/// the future describes a period that protects nothing, and S3 refuses it as a bad argument
/// — which is what the refusal table pins. The state it leaves behind is still reachable,
/// because any retention lapses once its own date goes by, so it is seeded here the way the
/// listing suites seed a key no request line can carry.
///
/// `pub(crate)`: the enforcement suite ([`super::object_lock_enforcement`]) needs the same
/// lapsed state.
pub(crate) fn given_lapsed_retention(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    bucket: &str,
    key: &str,
    mode: &str,
    retain_until_seconds: i64,
) {
    assert!(
        retain_until_seconds < now_seconds(),
        "a lapsed retention must run to an instant that has gone by, got \
         {retain_until_seconds}"
    );
    let retain_until = SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(u64::try_from(retain_until_seconds).expect(
            "the test's own lapsed instant must be after the epoch",
        ));
    let state = engine.state::<ComAmazonawsS3Amazons3State>();
    let written = state.lock().replace_object_retention(
        bucket,
        key,
        None,
        Some(ObjectRetention {
            mode: mode.to_string(),
            retain_until,
        }),
    );
    assert_eq!(
        written,
        Some(ObjectRetentionWrite::Applied),
        "the lapsed retention on {bucket}/{key} must be recorded first"
    );
}

/// What a `GetObjectRetention` reported: the `Mode` and the `RetainUntilDate` as epoch
/// seconds, or `None` when the answer carried no retention at all.
fn reported_retention(answer: &Answer) -> Option<(String, i64)> {
    let mode = answer.element("Mode")?;
    let date = answer.element("RetainUntilDate")?;
    Some((mode, epoch_seconds(&date)))
}

/// An object written at `key`, so a retention call has a version to act on. Reports the
/// version id the write minted, when the bucket's versioning state made it mint one.
pub(crate) async fn object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
) -> Option<String> {
    let written = put_object(router, bucket_name, key, b"payload", &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {bucket_name}/{key} must be written first, got {} with body {:?}",
        written.status,
        text(&written)
    );
    written.header("x-amz-version-id")
}

/// Place `mode` until `retain_until_seconds` on `bucket/key`, failing the test when the
/// service did not accept it — the setup every refusal row needs.
async fn given_retention(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    mode: &str,
    retain_until_seconds: i64,
) {
    let placed = put_object_retention(
        router,
        bucket_name,
        key,
        "",
        &[],
        &retention_payload(Some(mode), Some(retain_until_seconds)),
    )
    .await;
    assert_eq!(
        placed.status, 200,
        "case {mode:?}: the setup retention on {bucket_name}/{key} must be placed, got {} \
         with body {:?}",
        placed.status,
        text(&placed)
    );
}

/// One row of the round-trip table: the case name, the key the object is stored at, the
/// `Mode` the put carries, and how far from now its `RetainUntilDate` is, in seconds.
type RoundTripRow = (&'static str, &'static str, &'static str, i64);

/// The table of retentions every row of which must round-trip unchanged.
///
/// The rows differ along what the place-and-report rule generalises over: which mode the
/// retention is in (both values of `ObjectLockRetentionMode`, asserted below to be the
/// whole catalog the model publishes), how far out the period runs (a minute, a day, a
/// year, a decade), and the shape of the key it is placed on (a flat name, a prefixed one,
/// a deep path, one with a dot).
fn round_trip_rows() -> Vec<RoundTripRow> {
    vec![
        (
            "governance, a day out, flat key",
            "flat-key",
            GOVERNANCE_RETENTION_MODE,
            24 * ONE_HOUR,
        ),
        (
            "compliance, a year out, prefixed key",
            "locked/annual-report.pdf",
            COMPLIANCE_RETENTION_MODE,
            365 * 24 * ONE_HOUR,
        ),
        (
            "governance, a minute out",
            "briefly-locked",
            GOVERNANCE_RETENTION_MODE,
            60,
        ),
        (
            "compliance, a decade out, deep path",
            "a/deep/path/ledger.bin",
            COMPLIANCE_RETENTION_MODE,
            10 * 365 * 24 * ONE_HOUR,
        ),
    ]
}

#[tokio::test]
async fn a_retention_round_trips_through_put_and_get_object_retention() {
    let router = router();
    lock_enabled_bucket(&router, "object-retention").await;

    // The rows span the whole published mode catalog, so the table cannot quietly stop
    // covering a value the model declares.
    let mut modes: Vec<&str> = round_trip_rows().iter().map(|row| row.2).collect();
    modes.sort_unstable();
    modes.dedup();
    let mut published: Vec<&str> = OBJECT_LOCK_RETENTION_MODE_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        modes, published,
        "the table must cover every ObjectLockRetentionMode the model publishes, got {modes:?}"
    );

    for (case, key, mode, offset) in round_trip_rows() {
        object(&router, "object-retention", key).await;
        let retain_until = now_seconds() + offset;

        let placed = put_object_retention(
            &router,
            "object-retention",
            key,
            "",
            &[],
            &retention_payload(Some(mode), Some(retain_until)),
        )
        .await;
        assert_eq!(
            placed.status, 200,
            "case {case:?}: expected the modelled 200 from PutObjectRetention, got {} with \
             body {:?}",
            placed.status,
            text(&placed)
        );
        // Requester Pays is not emulated, so nobody was charged and nothing is reported.
        assert_eq!(
            placed.header("x-amz-request-charged"),
            None,
            "case {case:?}: no requester was charged, got {:?}",
            placed.header("x-amz-request-charged")
        );

        let read = get_object_retention(&router, "object-retention", key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetObjectRetention on the version just \
             locked, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            reported_retention(&read),
            Some((mode.to_string(), retain_until)),
            "case {case:?}: GetObjectRetention must report exactly the retention placed, \
             got {:?} from body {:?}",
            reported_retention(&read),
            text(&read)
        );

        // A second put REPLACES the configuration: lengthening the period leaves the
        // version carrying exactly the new date, and nothing of the old one.
        let lengthened = retain_until + ONE_HOUR;
        let second = put_object_retention(
            &router,
            "object-retention",
            key,
            "",
            &[],
            &retention_payload(Some(mode), Some(lengthened)),
        )
        .await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: lengthening a period needs no bypass, got {} with body {:?}",
            second.status,
            text(&second)
        );
        let reread = get_object_retention(&router, "object-retention", key, "", &[]).await;
        assert_eq!(
            reported_retention(&reread),
            Some((mode.to_string(), lengthened)),
            "case {case:?}: the replacing put must be what the version carries, got {:?}",
            reported_retention(&reread)
        );

        // Locking a version changes nothing else about it: its bytes read back unchanged.
        let object_read = get_object(&router, "object-retention", key, "", &[]).await;
        assert_eq!(
            object_read.body.as_ref(),
            b"payload",
            "case {case:?}: placing a retention must not touch the object's bytes, got {:?}",
            text(&object_read)
        );
    }
}

/// One row of the weakening table: the case name, the `Mode` the call asks for (`None` for
/// an empty container, which asks for no retention at all), its `RetainUntilDate` as an
/// offset from the one in force (`None` for a container that names no date), and the modes
/// in force that this call would leave the version protected LESS than.
type WeakeningRow = (
    &'static str,
    Option<&'static str>,
    Option<i64>,
    &'static [&'static str],
);

/// Every way one `PutObjectRetention` can relate to the retention already in force.
///
/// The rows differ along what the bypass rule generalises over: whether the call ends the
/// period earlier, at the same instant or later, whether it keeps the mode, strengthens it
/// or weakens it, and whether it asks for a retention at all. The last column is the
/// expectation itself — which modes in force the call protects the version less than —
/// stated per row rather than re-derived, so the rule has something to be wrong about.
fn weakening_rows() -> Vec<WeakeningRow> {
    vec![
        (
            "the same mode, an hour earlier: the period is shortened",
            Some("same"),
            Some(-ONE_HOUR),
            &[GOVERNANCE_RETENTION_MODE, COMPLIANCE_RETENTION_MODE],
        ),
        (
            "an empty container: the retention is taken away",
            None,
            None,
            &[GOVERNANCE_RETENTION_MODE, COMPLIANCE_RETENTION_MODE],
        ),
        (
            "governance over what is in force: the lock becomes liftable",
            Some(GOVERNANCE_RETENTION_MODE),
            Some(0),
            &[COMPLIANCE_RETENTION_MODE],
        ),
        (
            "compliance over what is in force: the lock becomes unliftable",
            Some(COMPLIANCE_RETENTION_MODE),
            Some(0),
            &[],
        ),
        (
            "the same mode and the same date: nothing changes",
            Some("same"),
            Some(0),
            &[],
        ),
        (
            "the same mode, an hour later: the period is lengthened",
            Some("same"),
            Some(ONE_HOUR),
            &[],
        ),
    ]
}

#[tokio::test]
async fn a_call_that_protects_a_version_less_needs_a_governance_bypass() {
    let router = router();
    lock_enabled_bucket(&router, "retention-bypass").await;

    for in_force in OBJECT_LOCK_RETENTION_MODE_VALUES {
        for (index, (case, asked_mode, offset, weakens)) in weakening_rows().iter().enumerate() {
            for bypass in [false, true] {
                let key = format!("{in_force}-{index}-bypass-{bypass}").to_lowercase();
                object(&router, "retention-bypass", &key).await;
                let retain_until = now_seconds() + 24 * ONE_HOUR;
                given_retention(&router, "retention-bypass", &key, in_force, retain_until).await;

                // What the call asks for: the mode the row names (`"same"` meaning the one
                // in force) and the date at the row's offset from the one in force.
                let asked_mode = asked_mode.map(|mode| {
                    if mode == "same" {
                        *in_force
                    } else {
                        mode
                    }
                });
                let asked_until = offset.map(|offset| retain_until + offset);
                let payload = retention_payload(asked_mode, asked_until);
                let headers: Vec<(&str, &str)> = if bypass {
                    vec![("x-amz-bypass-governance-retention", "true")]
                } else {
                    Vec::new()
                };

                // The rule: a call that protects the version less than the retention in
                // force is refused, UNLESS that retention is a governance one and the
                // request asks to bypass it. Anything else is served.
                let weakening = weakens.contains(in_force);
                let allowed =
                    !weakening || (bypass && *in_force == GOVERNANCE_RETENTION_MODE);
                let case = format!("{in_force} in force, {case}, bypass {bypass}");

                let answer = put_object_retention(
                    &router,
                    "retention-bypass",
                    &key,
                    "",
                    &headers,
                    &payload,
                )
                .await;
                if allowed {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the call to be served, got {} with body \
                         {:?}",
                        answer.status,
                        text(&answer)
                    );
                } else {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected the call to be refused with HTTP 403, got \
                         {} with body {:?}",
                        answer.status,
                        text(&answer)
                    );
                    assert_eq!(
                        answer.error_code(),
                        "AccessDenied",
                        "case {case:?}: expected the documented code, got {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.error_message(),
                        OBJECT_LOCK_ACCESS_DENIED_MESSAGE,
                        "case {case:?}: expected the fixed sentence S3 sends for an \
                         object-lock denial, got {:?}",
                        answer.error_message()
                    );
                }

                // And what the version carries afterwards: exactly what the call asked for
                // when it was served, and exactly what was in force when it was refused.
                let read = get_object_retention(&router, "retention-bypass", &key, "", &[]).await;
                let expected = if !allowed {
                    Some((in_force.to_string(), retain_until))
                } else {
                    match (asked_mode, asked_until) {
                        (Some(mode), Some(until)) => Some((mode.to_string(), until)),
                        _ => None,
                    }
                };
                assert_eq!(
                    reported_retention(&read),
                    expected,
                    "case {case:?}: expected the version to carry {expected:?} afterwards, \
                     got {:?} from a {} answer",
                    reported_retention(&read),
                    read.status
                );
                if expected.is_none() {
                    assert_eq!(
                        read.status, 404,
                        "case {case:?}: a version whose retention was removed carries none, \
                         got {} with body {:?}",
                        read.status,
                        text(&read)
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_retention_whose_date_has_passed_restricts_nothing() {
    let (router, engine) = router_with_engine();
    lock_enabled_bucket(&router, "expired-retention").await;

    // A retention whose date has already gone by is a period that is over, so it protects
    // the version no longer — in either mode, and whether the call that replaces it asks for
    // a shorter period or for none at all, with no bypass header anywhere.
    for mode in OBJECT_LOCK_RETENTION_MODE_VALUES {
        for (case, asked) in [
            ("replaced by a shorter period", Some(ONE_HOUR)),
            ("taken away entirely", None),
        ] {
            let key = format!("{mode}-{}", asked.unwrap_or_default()).to_lowercase();
            object(&router, "expired-retention", &key).await;
            let expired = now_seconds() - 24 * ONE_HOUR;
            given_lapsed_retention(&engine, "expired-retention", &key, mode, expired);

            let asked_until = asked.map(|offset| now_seconds() + offset);
            let answer = put_object_retention(
                &router,
                "expired-retention",
                &key,
                "",
                &[],
                &retention_payload(asked.map(|_| *mode), asked_until),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {mode}/{case:?}: an expired retention restricts nothing, so the call \
                 must be served, got {} with body {:?}",
                answer.status,
                text(&answer)
            );

            let read = get_object_retention(&router, "expired-retention", &key, "", &[]).await;
            let expected = asked_until.map(|until| (mode.to_string(), until));
            assert_eq!(
                reported_retention(&read),
                expected,
                "case {mode}/{case:?}: expected the version to carry {expected:?}, got {:?}",
                reported_retention(&read)
            );
        }
    }
}

/// One row of the version-read table: the case name, the `versionId` query the read
/// carries (empty for none), and the retention it must report.
type VersionReadRow = (String, String, Option<(String, i64)>);

#[tokio::test]
async fn a_retention_belongs_to_one_object_version() {
    let router = router();
    lock_enabled_bucket(&router, "retention-versions").await;

    // Two versions of one key, and a second key beside it: each write mints an id of its
    // own, which is what a retention is addressed by.
    let first = object(&router, "retention-versions", "versioned")
        .await
        .expect("a write into a versioning-enabled bucket mints a version id");
    let second = object(&router, "retention-versions", "versioned")
        .await
        .expect("a write into a versioning-enabled bucket mints a version id");
    assert_ne!(
        first, second,
        "the two writes must be two versions, got the same id {first:?} twice"
    );
    object(&router, "retention-versions", "untouched").await;

    // Placed with no `versionId`, the retention goes on the CURRENT version: the one the
    // second write made.
    let current_until = now_seconds() + 24 * ONE_HOUR;
    given_retention(
        &router,
        "retention-versions",
        "versioned",
        GOVERNANCE_RETENTION_MODE,
        current_until,
    )
    .await;

    // (case, the versionId the read names, what it must report)
    let reads: Vec<VersionReadRow> = vec![
        (
            "no versionId reads the current version".to_string(),
            String::new(),
            Some((GOVERNANCE_RETENTION_MODE.to_string(), current_until)),
        ),
        (
            "the current version by its own id".to_string(),
            format!("&versionId={second}"),
            Some((GOVERNANCE_RETENTION_MODE.to_string(), current_until)),
        ),
        (
            "the noncurrent version carries none of it".to_string(),
            format!("&versionId={first}"),
            None,
        ),
    ];
    for (case, query, expected) in &reads {
        let read = get_object_retention(&router, "retention-versions", "versioned", query, &[])
            .await;
        assert_eq!(
            &reported_retention(&read),
            expected,
            "case {case:?}: expected {expected:?}, got {:?} from a {} answer with body {:?}",
            reported_retention(&read),
            read.status,
            text(&read)
        );
    }

    // A retention on the NONCURRENT version is its own: placing a compliance one there
    // leaves the current version's governance one exactly as it was.
    let noncurrent_until = now_seconds() + 48 * ONE_HOUR;
    let placed = put_object_retention(
        &router,
        "retention-versions",
        "versioned",
        &format!("&versionId={first}"),
        &[],
        &retention_payload(Some(COMPLIANCE_RETENTION_MODE), Some(noncurrent_until)),
    )
    .await;
    assert_eq!(
        placed.status, 200,
        "a retention may be placed on a noncurrent version, got {} with body {:?}",
        placed.status,
        text(&placed)
    );
    let noncurrent = get_object_retention(
        &router,
        "retention-versions",
        "versioned",
        &format!("&versionId={first}"),
        &[],
    )
    .await;
    assert_eq!(
        reported_retention(&noncurrent),
        Some((COMPLIANCE_RETENTION_MODE.to_string(), noncurrent_until)),
        "the noncurrent version must carry the retention placed on it, got {:?}",
        reported_retention(&noncurrent)
    );
    let current = get_object_retention(&router, "retention-versions", "versioned", "", &[]).await;
    assert_eq!(
        reported_retention(&current),
        Some((GOVERNANCE_RETENTION_MODE.to_string(), current_until)),
        "locking one version must leave another's retention untouched, got {:?}",
        reported_retention(&current)
    );

    // And no other key was touched: the key beside it still carries no retention at all.
    let untouched =
        get_object_retention(&router, "retention-versions", "untouched", "", &[]).await;
    assert_eq!(
        untouched.status, 404,
        "a retention belongs to one key, so the key beside it carries none, got {} with \
         body {:?}",
        untouched.status,
        text(&untouched)
    );
}

#[tokio::test]
async fn a_retention_call_this_emulator_cannot_serve_answers_what_is_wrong() {
    let router = router();
    lock_enabled_bucket(&router, "held-retention-bucket").await;
    object(&router, "held-retention-bucket", "held").await;
    let retain_until = now_seconds() + 24 * ONE_HOUR;
    let payload = retention_payload(Some(GOVERNANCE_RETENTION_MODE), Some(retain_until));

    // A version with no retention associated with it: the subresource is not there, so the
    // read answers 404 rather than an empty 200.
    let absent = get_object_retention(&router, "held-retention-bucket", "held", "", &[]).await;
    assert_eq!(
        absent.status, 404,
        "an unlocked version has no retention to report, got {} with body {:?}",
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
        "held-retention",
        "held-retention-bucket-2",
        "Held-Retention-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutObjectRetention",
                put_object_retention(&router, missing, "held", "", &[], &payload).await,
            ),
            (
                "GetObjectRetention",
                get_object_retention(&router, missing, "held", "", &[]).await,
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
                "PutObjectRetention",
                put_object_retention(
                    &router,
                    "held-retention-bucket",
                    missing,
                    "",
                    &[],
                    &payload,
                )
                .await,
            ),
            (
                "GetObjectRetention",
                get_object_retention(&router, "held-retention-bucket", missing, "", &[]).await,
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
    // generated-looking one, one that is not a minted id at all, and the literal `null` of
    // a version written before versioning was enabled, which this Object Lock enabled
    // bucket (and so versioning-enabled from the moment it existed) has never held.
    for missing in ["3HL4kqtJlcpXroDTDmjVBH40Nrjfkd", "not-a-minted-id", "null"] {
        for (operation, answer) in [
            (
                "PutObjectRetention",
                put_object_retention(
                    &router,
                    "held-retention-bucket",
                    "held",
                    &format!("&versionId={missing}"),
                    &[],
                    &payload,
                )
                .await,
            ),
            (
                "GetObjectRetention",
                get_object_retention(
                    &router,
                    "held-retention-bucket",
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

    // A container naming one of the two members describes no period, so it is refused —
    // and so is one naming a mode no `ObjectLockRetentionMode` value spells.
    for (case, body) in [
        (
            "a mode with no date",
            retention_payload(Some(GOVERNANCE_RETENTION_MODE), None),
        ),
        ("a date with no mode", retention_payload(None, Some(retain_until))),
        (
            "a mode outside the published catalog",
            retention_payload(Some("ETERNAL"), Some(retain_until)),
        ),
    ] {
        let refused =
            put_object_retention(&router, "held-retention-bucket", "held", "", &[], &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the half-described retention to be refused with HTTP \
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
            "PutObjectRetention",
            put_object_retention(
                &router,
                "held-retention-bucket",
                "held",
                "",
                &owner,
                &payload,
            )
            .await,
        ),
        (
            "GetObjectRetention",
            get_object_retention(&router, "held-retention-bucket", "held", "", &owner).await,
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

    // None of the refusals above wrote anything: the held version still carries no
    // retention at all.
    let still_absent =
        get_object_retention(&router, "held-retention-bucket", "held", "", &[]).await;
    assert_eq!(
        still_absent.error_code(),
        "NoSuchObjectLockConfiguration",
        "a refused call must not have locked the held version, got {} with body {:?}",
        still_absent.status,
        text(&still_absent)
    );
}

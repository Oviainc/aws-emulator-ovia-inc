// SPDX-License-Identifier: CC0-1.0

//! S3 Express One Zone: creating a DIRECTORY bucket, taking a session on it, and appending
//! to an object in it.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one the
//! bucket-lifecycle and object suites in this crate already own), so what is pinned is the
//! wire behaviour — status, bound headers, error code, the stored bytes a later read serves.
//!
//! The rules under test, with their sources:
//!
//! * a `CreateBucket` whose `CreateBucketConfiguration$Bucket` names the `Directory` type is
//!   placed in the ONE zone its `Location` names — "for directory buckets, the name of the
//!   location is the Zone ID of the Availability Zone (AZ) or Local Zone (LZ) where the
//!   bucket will be created" (`LocationInfo`) — the bucket's name has to be a directory
//!   bucket's and name that same zone (`<base>--<zone-id>--x-s3`, Amazon S3 User Guide,
//!   *Directory bucket naming rules*), and the create answers the bucket's ARN, which "is
//!   only supported for S3 directory buckets" (`CreateBucketOutput$BucketArn`);
//! * `CreateSession` "creates a session that establishes temporary security credentials to
//!   support fast authentication and authorization for the Zonal endpoint API operations on
//!   directory buckets", whose credentials "are scoped to the bucket and expire after 5
//!   minutes", and whose one modelled error is `NoSuchBucket` (404) for a bucket that is not
//!   there;
//! * `PutObject`'s `x-amz-write-offset-bytes` "specifies the offset for appending data to
//!   existing objects in bytes. The offset must be equal to the size of the existing object
//!   being appended to. If no object exists, setting this header to 0 will create a new
//!   object" (`PutObjectRequest$WriteOffsetBytes`), it "is only supported for objects in the
//!   Amazon S3 Express One Zone storage class in directory buckets", an offset that is not
//!   the current size is the modelled `InvalidWriteOffset` (400, "the write offset value
//!   that you specified does not match the current object size"), and the write reports "the
//!   size of the object in bytes … only present if you append to an object"
//!   (`PutObjectOutput$Size`);
//! * a session is a credential this process keeps ACCEPTING until it expires, so the
//!   sessions it holds at once are bounded and an expired one is released — the documented
//!   5-minute interval is enforced, not only reported.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::{
    ComAmazonawsS3Amazons3State, MAX_VENDED_SESSIONS,
};
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The `CreateBucketConfiguration` a directory-bucket create carries: the zone it is placed
/// in and the kind of bucket it is, in the model's own element order.
fn directory_configuration(
    location_type: Option<&str>,
    zone_id: Option<&str>,
    data_redundancy: Option<&str>,
    bucket_type: Option<&str>,
) -> String {
    let element = |name: &str, value: Option<&str>| match value {
        Some(value) => format!("<{name}>{value}</{name}>"),
        None => String::new(),
    };
    let location = match (location_type, zone_id) {
        (None, None) => String::new(),
        _ => format!(
            "<Location>{}{}</Location>",
            element("Type", location_type),
            element("Name", zone_id)
        ),
    };
    let bucket = match (data_redundancy, bucket_type) {
        (None, None) => String::new(),
        _ => format!(
            "<Bucket>{}{}</Bucket>",
            element("DataRedundancy", data_redundancy),
            element("Type", bucket_type)
        ),
    };
    format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         {location}{bucket}</CreateBucketConfiguration>"
    )
}

/// `PUT /<bucket>` — `CreateBucket` carrying a directory-bucket configuration.
async fn create_directory_bucket(
    router: &MultiServiceRouter,
    name: &str,
    configuration: &str,
) -> Answer {
    send(router, "PUT", &format!("/{name}"), &[], configuration).await
}

/// `GET /<bucket>?session` — `CreateSession`.
async fn create_session(router: &MultiServiceRouter, name: &str) -> Answer {
    send(router, "GET", &format!("/{name}?session"), &[], "").await
}

/// The same `CreateSession`, signed for the S3 EXPRESS signing name instead of `s3` — the
/// scope a client that addresses a directory bucket's Zonal endpoint presents.
///
/// Sent with its own `Authorization` header rather than through [`send`], because the
/// credential scope is exactly what this case varies; neither the key id nor the signature
/// is a credential here (this suite's router verifies none, as the other suites' do).
async fn create_session_signed_for_s3_express(
    router: &MultiServiceRouter,
    name: &str,
) -> Answer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://{name}.s3express-use1-az4.us-east-1.amazonaws.com/{name}?session"
        ))
        .header("host", "s3express-use1-az4.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=emulator-test-key/20260101/us-east-1/s3express/aws4_request, \
             SignedHeaders=host, Signature=not-verified",
        )
        .body(bytes::Bytes::new())
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `PUT /<bucket>/<key>` carrying `x-amz-write-offset-bytes` — an APPENDING `PutObject`.
async fn append_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    offset: i64,
    chunk: &[u8],
) -> Answer {
    let offset = offset.to_string();
    put_object(
        router,
        bucket_name,
        key,
        chunk,
        &[("x-amz-write-offset-bytes", offset.as_str())],
    )
    .await
}

/// A directory bucket in one zone, created and asserted created, so a suite about what is
/// served ON one does not repeat the create table.
///
/// `pub(crate)`: the conditional-delete suite ([`super::object_conditional_deletes`]) needs
/// one too, because the modification-time and size conditions of a delete are "only
/// supported for directory buckets" (`DeleteObjectRequest$IfMatchSize`).
pub(crate) async fn directory_bucket(router: &MultiServiceRouter, name: &str, zone_id: &str) {
    let created = create_directory_bucket(
        router,
        name,
        &directory_configuration(
            Some("AvailabilityZone"),
            Some(zone_id),
            None,
            Some("Directory"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the directory bucket {name:?} must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
}

/// RULE: a create that names the `Directory` bucket type places the bucket in the zone its
/// `Location` names, and the name it asks for has to be a directory bucket's name for that
/// same zone.
///
/// The rows differ along what the rule generalises over: the location type (an Availability
/// Zone and a Local Zone), whether the redundancy is named or implied, and each way the
/// request can fail to describe one bucket in one zone.
#[tokio::test]
async fn a_directory_bucket_is_placed_in_the_zone_its_location_names() {
    let router = router();
    let account = engine::emulator_account_id();

    // (case, bucket name, configuration, status, the ARN the create must report when it
    // succeeds, the error code when it does not).
    let rows: [(&str, &str, String, u16, &str); 8] = [
        (
            "an Availability Zone bucket whose redundancy is implied",
            "appendable--use1-az4--x-s3",
            directory_configuration(
                Some("AvailabilityZone"),
                Some("use1-az4"),
                None,
                Some("Directory"),
            ),
            200,
            "",
        ),
        (
            "an Availability Zone bucket naming its redundancy",
            "explicit--use1-az5--x-s3",
            directory_configuration(
                Some("AvailabilityZone"),
                Some("use1-az5"),
                Some("SingleAvailabilityZone"),
                Some("Directory"),
            ),
            200,
            "",
        ),
        (
            // A Local Zone id carries the zone's own name before the AZ suffix, which the
            // `--`-separated bucket name still reads as one segment.
            "a Local Zone bucket",
            "zonal--usw2-lax1-az1--x-s3",
            directory_configuration(
                Some("LocalZone"),
                Some("usw2-lax1-az1"),
                Some("SingleLocalZone"),
                Some("Directory"),
            ),
            200,
            "",
        ),
        (
            "a name whose zone segment is another zone",
            "mismatched--use1-az4--x-s3",
            directory_configuration(
                Some("AvailabilityZone"),
                Some("use1-az5"),
                None,
                Some("Directory"),
            ),
            400,
            "InvalidBucketName",
        ),
        (
            "a name without the directory-bucket suffix",
            "no-suffix--use1-az4",
            directory_configuration(
                Some("AvailabilityZone"),
                Some("use1-az4"),
                None,
                Some("Directory"),
            ),
            400,
            "InvalidBucketName",
        ),
        (
            "a directory bucket that names no zone",
            "zoneless--use1-az4--x-s3",
            directory_configuration(None, None, None, Some("Directory")),
            400,
            "InvalidRequest",
        ),
        (
            "a zone named for a bucket that is not a directory bucket",
            "general-purpose-with-a-zone",
            directory_configuration(Some("AvailabilityZone"), Some("use1-az4"), None, None),
            400,
            "InvalidRequest",
        ),
        (
            "a redundancy that names the other kind of zone",
            "redundancy--use1-az4--x-s3",
            directory_configuration(
                Some("AvailabilityZone"),
                Some("use1-az4"),
                Some("SingleLocalZone"),
                Some("Directory"),
            ),
            400,
            "InvalidRequest",
        ),
    ];

    for (case, name, configuration, status, code) in rows {
        let created = create_directory_bucket(&router, name, &configuration).await;
        assert_eq!(
            created.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            created.status,
            text(&created)
        );
        if status != 200 {
            assert_eq!(
                created.error_code(),
                code,
                "case {case:?}: expected the {code} code, got {:?}",
                created.error_code()
            );
            // A refused create leaves no bucket behind, so a session on the name it asked
            // for reports the bucket is not there.
            let session = create_session(&router, name).await;
            assert_eq!(
                session.error_code(),
                "NoSuchBucket",
                "case {case:?}: a refused create must leave no bucket, got {:?} from \
                 CreateSession",
                session.error_code()
            );
            continue;
        }
        // "This parameter is only supported for S3 directory buckets"
        // (`CreateBucketOutput$BucketArn`): the ARN names the `s3express` namespace, the
        // endpoint's Region, this account and the bucket.
        let expected_arn = format!("arn:aws:s3express:us-east-1:{account}:bucket/{name}");
        assert_eq!(
            created.header("x-amz-bucket-arn"),
            Some(expected_arn.clone()),
            "case {case:?}: expected the bucket ARN {expected_arn:?}, got {:?}",
            created.header("x-amz-bucket-arn")
        );
    }

    // A general purpose bucket is unchanged by all of this: it carries no ARN on the create,
    // because the member is for directory buckets only.
    let general = send(&router, "PUT", "/general-purpose-bucket", &[], "").await;
    assert_eq!(
        general.status, 200,
        "case \"a general purpose bucket\": expected HTTP 200, got {} with body {:?}",
        general.status,
        text(&general)
    );
    assert_eq!(
        general.header("x-amz-bucket-arn"),
        None,
        "case \"a general purpose bucket\": expected no bucket ARN, got {:?}",
        general.header("x-amz-bucket-arn")
    );
}

/// RULE: `CreateSession` vends temporary credentials for a directory bucket that exists,
/// each its own credential, expiring after the documented 5 minutes; it answers
/// `NoSuchBucket` for a bucket that is not there, and refuses a bucket no Zonal endpoint
/// addresses.
#[tokio::test]
async fn a_session_vends_scoped_credentials_for_a_directory_bucket() {
    let router = router();
    let name = "session--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;
    bucket(&router, "session-general-purpose").await;

    let mut vended: Vec<String> = Vec::new();
    for attempt in ["the first session", "a second session on the same bucket"] {
        let session = create_session(&router, name).await;
        assert_eq!(
            session.status, 200,
            "case {attempt:?}: expected HTTP 200, got {} with body {:?}",
            session.status,
            text(&session)
        );
        let access_key_id = session.element("AccessKeyId").unwrap_or_default();
        let secret = session.element("SecretAccessKey").unwrap_or_default();
        let token = session.element("SessionToken").unwrap_or_default();
        for (member, value) in [
            ("AccessKeyId", &access_key_id),
            ("SecretAccessKey", &secret),
            ("SessionToken", &token),
        ] {
            assert!(
                !value.is_empty(),
                "case {attempt:?}: the session must carry a {member}, got {value:?} in {:?}",
                text(&session)
            );
        }
        // Each session is its OWN credential: "you must use IAM credentials again to make a
        // CreateSession API request that generates a NEW set of temporary credentials".
        assert!(
            !vended.contains(&access_key_id),
            "case {attempt:?}: every session must vend its own access key id, but \
             {access_key_id:?} was vended before"
        );
        vended.push(access_key_id);

        // "Temporary security credentials … expire after 5 minutes": the instant reported
        // is parsed and compared against the window that interval puts it in, rather than
        // against any literal.
        let expiration = session
            .element("Expiration")
            .unwrap_or_else(|| panic!("case {attempt:?}: the session must carry an Expiration"));
        let parsed = DateTime::from_str(&expiration, Format::DateTime).unwrap_or_else(|error| {
            panic!("case {attempt:?}: the Expiration must be a timestamp ({error}): {expiration}")
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock must be after the epoch")
            .as_secs() as i64;
        let remaining = parsed.secs() - now;
        assert!(
            (4 * 60..=5 * 60).contains(&remaining),
            "case {attempt:?}: the session must expire 5 minutes out, but {expiration} is \
             {remaining} seconds away"
        );
    }

    // The SAME call signed for the S3 Express signing name — the scope a client reaching a
    // directory bucket's Zonal endpoint presents — is this service's too, and is served
    // rather than reaching nobody.
    let zonal = create_session_signed_for_s3_express(&router, name).await;
    assert_eq!(
        zonal.status, 200,
        "case \"a session signed for s3express\": expected HTTP 200, got {} with body {:?}",
        zonal.status,
        text(&zonal)
    );
    assert!(
        zonal.element("AccessKeyId").is_some_and(|id| !id.is_empty()),
        "case \"a session signed for s3express\": expected session credentials, got {:?}",
        text(&zonal)
    );

    // A `?session` request against a bucket that HAS no session subresource is not a refused
    // session at all: S3 ignores a query parameter that names no subresource of the bucket it
    // was addressed to, so what is left is `GET /{Bucket}` — the bucket's own listing.
    let listed = create_session(&router, "session-general-purpose").await;
    assert_eq!(
        listed.status, 200,
        "case \"a general purpose bucket, which has no session subresource\": expected the \
         bucket's listing with HTTP 200, got {} with body {:?}",
        listed.status,
        text(&listed)
    );
    let body = text(&listed);
    for element in ["Name", "Prefix", "Marker", "MaxKeys", "IsTruncated"] {
        assert!(
            body.contains(&format!("<{element}>")),
            "case \"a general purpose bucket, which has no session subresource\": the listing \
             must carry <{element}>, got {body}"
        );
    }
    assert!(
        !body.contains("<Credentials>"),
        "case \"a general purpose bucket, which has no session subresource\": no session may \
         be vended, got {body}"
    );

    let rows: [(&str, &str, u16, &str); 1] = [(
        "a bucket this emulator does not hold",
        "absent--use1-az4--x-s3",
        404,
        "NoSuchBucket",
    )];
    for (case, bucket_name, status, code) in rows {
        let refused = create_session(&router, bucket_name).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the {code} code, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.error_message().is_empty(),
            "case {case:?}: the refusal must carry a message, got {:?}",
            text(&refused)
        );
    }
}

/// RULE: an appending write extends the object when its offset IS the object's current size,
/// and is refused `InvalidWriteOffset` when it is not — on a directory bucket only.
///
/// The rows differ along the chunk's size (empty, one byte, a long run) and along whether
/// the key already holds anything, so the rule is exercised over a sequence of appends
/// rather than one.
#[tokio::test]
async fn an_append_extends_the_object_only_at_its_current_size() {
    let router = router();
    let name = "appends--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;
    bucket(&router, "appends-general-purpose").await;

    let key = "logs/stream.bin";
    let chunks: [(&str, &[u8]); 4] = [
        ("the first chunk, into a key holding nothing", b"alpha"),
        ("one byte", b"b"),
        ("an empty chunk", b""),
        ("a long run", b"0123456789012345678901234567890123456789"),
    ];
    let mut expected: Vec<u8> = Vec::new();
    for (case, chunk) in chunks {
        let offset = expected.len() as i64;
        let appended = append_object(&router, name, key, offset, chunk).await;
        assert_eq!(
            appended.status, 200,
            "case {case:?}: expected the append at offset {offset} to be accepted, got {} \
             with body {:?}",
            appended.status,
            text(&appended)
        );
        expected.extend_from_slice(chunk);
        // "The size of the object in bytes. This will only be present if you append to an
        // object" (`PutObjectOutput$Size`) — the size the NEXT append's offset has to be.
        assert_eq!(
            appended.header("x-amz-object-size"),
            Some(expected.len().to_string()),
            "case {case:?}: expected the write to report {} bytes, got {:?}",
            expected.len(),
            appended.header("x-amz-object-size")
        );
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            expected.as_slice(),
            "case {case:?}: expected the appended object to read back as {} bytes, got {}",
            expected.len(),
            read.body.len()
        );
        // An object in a directory bucket is stored in the S3 Express One Zone class, which
        // is the class an append requires.
        assert_eq!(
            read.header("x-amz-storage-class"),
            Some("EXPRESS_ONEZONE".to_string()),
            "case {case:?}: expected the stored object's storage class, got {:?}",
            read.header("x-amz-storage-class")
        );
    }

    // Every offset that is NOT the object's current size is refused, and the object is left
    // exactly as it was.
    let size = expected.len() as i64;
    let offsets: [(&str, i64); 4] = [
        ("an offset of zero on a key that holds bytes", 0),
        ("an offset below the current size", size - 1),
        ("an offset above the current size", size + 1),
        ("an offset far past the current size", size + 4096),
    ];
    for (case, offset) in offsets {
        let refused = append_object(&router, name, key, offset, b"rejected").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidWriteOffset",
            "case {case:?}: expected the InvalidWriteOffset code, got {:?}",
            refused.error_code()
        );
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.body.as_ref(),
            expected.as_slice(),
            "case {case:?}: a refused append must store nothing, got {} bytes",
            read.body.len()
        );
    }

    // The same header on a GENERAL PURPOSE bucket names a parameter that is not valid for
    // the request: appending "is only supported for objects in the Amazon S3 Express One
    // Zone storage class in directory buckets".
    for (case, offset) in [("offset zero", 0), ("a non-zero offset", 7)] {
        let refused =
            append_object(&router, "appends-general-purpose", "stream.bin", offset, b"x").await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a general purpose bucket to refuse an append with HTTP \
             400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequest",
            "case {case:?}: expected the InvalidRequest code, got {:?}",
            refused.error_code()
        );
    }

    // A write that carries NO offset still replaces the whole object, whichever kind of
    // bucket it is in.
    let replaced = put_object(&router, name, key, b"replaced", &[]).await;
    assert_eq!(
        replaced.status, 200,
        "case \"a write with no offset\": expected HTTP 200, got {} with body {:?}",
        replaced.status,
        text(&replaced)
    );
    assert_eq!(
        replaced.header("x-amz-object-size"),
        None,
        "case \"a write with no offset\": the size is reported only for an append, got {:?}",
        replaced.header("x-amz-object-size")
    );
    let read = get_object(&router, name, key, "", &[]).await;
    assert_eq!(
        read.body.as_ref(),
        b"replaced",
        "case \"a write with no offset\": expected the object to be replaced, got {} bytes",
        read.body.len()
    );
}

/// One contention row: the case name and the two chunks the racing appends carry.
type RaceRow<'a> = (&'a str, &'a [u8], &'a [u8]);

/// RULE: two appends racing at the SAME offset resolve to exactly one accepted write and one
/// `InvalidWriteOffset`, and the object ends up exactly one chunk longer — the offset is read
/// and the appended bytes written under one guard, so no interleaving can lose a chunk or
/// accept both.
#[tokio::test]
async fn two_appends_at_one_offset_resolve_to_exactly_one_write() {
    let router = router();
    let name = "contention--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    let key = "contended.bin";
    let base = b"base-content";
    let seeded = append_object(&router, name, key, 0, base).await;
    assert_eq!(
        seeded.status, 200,
        "the seeding append must be accepted, got {} with body {:?}",
        seeded.status,
        text(&seeded)
    );

    let rows: [RaceRow<'_>; 2] = [
        ("equal chunks", b"one-chunk", b"one-chunk"),
        ("chunks of different sizes", b"a", b"bbbbbbbb"),
    ];
    for (case, left, right) in rows {
        let before = get_object(&router, name, key, "", &[]).await.body.len();
        let offset = before as i64;
        let (first, second) = tokio::join!(
            append_object(&router, name, key, offset, left),
            append_object(&router, name, key, offset, right)
        );
        let accepted = [&first, &second]
            .iter()
            .filter(|answer| answer.status == 200)
            .count();
        assert_eq!(
            accepted, 1,
            "case {case:?}: exactly one of two appends at offset {offset} may be accepted, \
             got {} and {}",
            first.status, second.status
        );
        let refused = if first.status == 200 { &second } else { &first };
        assert_eq!(
            refused.status, 400,
            "case {case:?}: the other append must be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            text(refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidWriteOffset",
            "case {case:?}: expected the InvalidWriteOffset code, got {:?}",
            refused.error_code()
        );
        // Exactly ONE chunk longer: whichever write won, the other added nothing.
        let winner = if first.status == 200 { left } else { right };
        let after = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            after.body.len(),
            before + winner.len(),
            "case {case:?}: the object must be exactly one chunk longer, expected {} bytes, \
             got {}",
            before + winner.len(),
            after.body.len()
        );
    }
}

/// RULE: a session is a credential this process keeps accepting until it expires, so the
/// number held at once is bounded and an expired one is RELEASED — the release is what the
/// documented 5-minute interval buys, so a caller that lets its sessions expire is never
/// stopped by the bound.
///
/// The store is seeded directly here, the way the account's bucket-quota suite seeds it, so
/// the bound is exercised without dispatching hundreds of requests; the engine is the SAME
/// one the router hands the service, so what is seeded is what the handler sees.
#[tokio::test]
async fn the_sessions_held_at_once_are_bounded_and_expired_ones_are_released() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let name = "quota--use1-az4--x-s3";
    directory_bucket(&router, name, "use1-az4").await;

    // The release reports exactly the sessions the clock has already expired, and keeps the
    // ones it has not.
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        let now = SystemTime::now();
        let rows: [(&str, &str, SystemTime, bool); 3] = [
            ("expired a minute ago", "session-expired", now - Duration::from_secs(60), true),
            ("expiring at this instant", "session-due", now, true),
            ("live for another minute", "session-live", now + Duration::from_secs(60), false),
        ];
        for (case, id, expires_at, _) in rows {
            assert!(
                store.record_vended_session(id, expires_at),
                "case {case:?}: an empty store must take the session {id:?}"
            );
        }
        let released = store.release_expired_sessions(now);
        for (case, id, _, expired) in rows {
            assert_eq!(
                released.contains(&id.to_string()),
                expired,
                "case {case:?}: expected released={expired} for {id:?}, got {released:?}"
            );
            assert_eq!(
                store.holds_vended_session(id),
                !expired,
                "case {case:?}: expected the store to hold {id:?} only while it is live"
            );
        }
    }

    // At the bound, a vend is refused rather than growing the store — and the credential it
    // would have returned is not left behind.
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        let live = SystemTime::now() + Duration::from_secs(300);
        for index in 0..MAX_VENDED_SESSIONS {
            store.record_vended_session(&format!("filler-{index}"), live);
        }
        assert!(
            !store.record_vended_session("one-too-many", live),
            "the store must refuse a session past {MAX_VENDED_SESSIONS}"
        );
    }
    let refused = create_session(&router, name).await;
    assert_eq!(
        refused.status, 400,
        "case \"a vend at the bound\": expected HTTP 400, got {} with body {:?}",
        refused.status,
        text(&refused)
    );
    assert!(
        refused.element("Credentials").is_none(),
        "case \"a vend at the bound\": a refused vend must return no credentials, got {:?}",
        text(&refused)
    );

    // Once those sessions have expired, the next vend releases them and succeeds: the bound
    // is on what is LIVE, which is what makes the documented lifetime enforced rather than
    // reported.
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        // The sessions the bound test left behind go the way the clock takes them: released
        // once their expiry has passed, which is what asking for the release at a later
        // instant means.
        store.release_expired_sessions(SystemTime::now() + Duration::from_secs(3_600));
        let past = SystemTime::now() - Duration::from_secs(1);
        for index in 0..MAX_VENDED_SESSIONS {
            store.record_vended_session(&format!("stale-{index}"), past);
        }
    }
    let vended = create_session(&router, name).await;
    assert_eq!(
        vended.status, 200,
        "case \"a vend once the held sessions have expired\": expected HTTP 200, got {} with \
         body {:?}",
        vended.status,
        text(&vended)
    );
    assert!(
        vended
            .element("AccessKeyId")
            .is_some_and(|id| !id.is_empty()),
        "case \"a vend once the held sessions have expired\": expected session credentials, \
         got {:?}",
        text(&vended)
    );
}

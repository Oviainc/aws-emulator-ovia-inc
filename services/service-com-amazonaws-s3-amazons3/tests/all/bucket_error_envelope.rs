// SPDX-License-Identifier: CC0-1.0

//! A bucket-scoped S3 error document names the bucket the request addressed.
//!
//! The rule under test: where a refusal's condition is a BUCKET's — the bucket itself, or
//! one of the bucket-level subresources a `?`-addressed operation reads and writes — the
//! `<Error>` document carries a `<BucketName>` whose text is the bucket the request
//! addressed, verbatim, beside the code and the message. A refusal whose condition is NOT a
//! bucket's (an object's key, the caller's authorization, a request this emulator has not
//! written) carries no such element, and neither does a served response.
//!
//! Why it matters: a client that fans one operation out over many buckets, or that logs and
//! retries per bucket, identifies the bucket that failed from that element alone — an SDK
//! error object surfaces it as `BucketName` and hands back `None` when it is absent, which
//! makes the refusal unattributable.
//!
//! The table spans what the rule generalises over:
//!
//! * the error CODE — the bucket not existing, the bucket not being empty, the name already
//!   being owned, and each bucket-level subresource that can be absent
//!   (`crate::scaffolded::BUCKET_SCOPED_ERROR_CODES` is the catalog, and
//!   [`every_bucket_scoped_code_is_covered`] holds this table to it);
//! * the operation and METHOD answering it (`GET`, `PUT`, `DELETE`);
//! * the STATUS (404 and 409);
//! * the bucket NAME's own form — the 3-character minimum and the 63-character maximum of
//!   the published naming rules, a dotted name, a hyphenated one and a digit-bearing one;
//!   and
//! * how the request ADDRESSED that bucket — in its path (path style) and in the first
//!   label of its host (virtual-hosted style).
//!
//! The element's text is a value the caller chose, so
//! [`a_bucket_name_in_the_envelope_is_xml_escaped`] also pins that it is written through
//! the codec that escapes XML metacharacters rather than pasted into the document.
//!
//! This suite is the WIRE half of the rule, and is deliberately blind to how a given
//! refusal came by its name: a handler that attaches it itself
//! (`crate::scaffolded::Scaffolded::naming_bucket`, whose own table is
//! [`super::refusal_resource_naming`] — the metadata-configuration and annotation families)
//! and one that leaves the serializer to name it from the request's address must answer the
//! same document, which is what [`every_bucket_scoped_code_is_covered_by_a_request`] holds
//! the whole catalog to.
//!
//! [`a_copy_refused_for_a_bucket_names_the_bucket_that_condition_is_about`] pins the case
//! where the two differ: a copy reads from one bucket and writes into another, so a refusal
//! about its SOURCE must name the source and not the bucket the request addressed.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_s3_amazons3::scaffolded::BUCKET_SCOPED_ERROR_CODES;

use super::bucket_lifecycle::{create_bucket, router, send};
use super::object_and_tagging_roundtrip::put_object;

/// The bucket name element S3's bucket-scoped error document carries.
const BUCKET_NAME_ELEMENT: &str = "BucketName";

/// A bucket whose name is the SHORTEST one the naming rules allow ("bucket names must be
/// between 3 (min) and 63 (max) characters long", Amazon S3 User Guide, *Bucket naming
/// rules*).
const SHORTEST_BUCKET: &str = "env";

/// A bucket whose name is the LONGEST those same rules allow — 63 characters, counted by
/// [`the_boundary_bucket_names_are_the_published_bounds`].
const LONGEST_BUCKET: &str =
    "envelope-bucket-with-the-longest-name-a-bucket-name-may-carry-x";

/// A bucket whose name carries dots, which the same rules allow ("bucket names can consist
/// only of lowercase letters, numbers, dots (.), and hyphens (-)").
const DOTTED_BUCKET: &str = "envelope.dotted.bucket";

/// A bucket whose name carries digits, from that same character set.
const DIGIT_BUCKET: &str = "envelope-bucket-2026";

/// The bucket every subresource row reads: created, then stripped of the two subresources a
/// bucket comes into being carrying.
const SUBRESOURCE_BUCKET: &str = "envelope-subresources";

/// The bucket that still holds an object, so a delete of it is refused for the contents.
const NON_EMPTY_BUCKET: &str = "envelope-not-empty";

/// The bucket a VIRTUAL-HOSTED-STYLE request addresses in the first label of its host.
const HOST_STYLE_BUCKET: &str = "envelope-host-style";

/// A Region other than `us-east-1`, so a second create of a bucket is refused as a conflict
/// rather than answering the documented legacy 200.
const CONFLICT_REGION: &str = "us-west-2";

/// One row: the request, and the refusal it must answer — with the bucket its envelope must
/// name, or `None` for a refusal whose condition is not a bucket's.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    status: u16,
    code: &'static str,
    bucket: Option<&'static str>,
}

/// Create `bucket`, failing the suite if the setup create does not succeed.
async fn created(router: &MultiServiceRouter, bucket: &str, region: Option<&str>) {
    let answer = create_bucket(router, bucket, region, None).await;
    assert_eq!(
        answer.status, 200,
        "case {bucket:?}: the setup create must succeed, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Withdraw `subresource` from `bucket`, failing the suite if the setup delete does not
/// succeed — the only way to a bucket that carries none of a subresource it is created with.
async fn withdrawn(router: &MultiServiceRouter, bucket: &str, subresource: &str) {
    let answer = send(
        router,
        "DELETE",
        &format!("/{bucket}?{subresource}"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "case {subresource:?}: the setup delete must succeed, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

#[test]
fn the_boundary_bucket_names_are_the_published_bounds() {
    // The two rows that stand on the published naming bounds are only on them if they are
    // the lengths they claim: 3 and 63 characters (Amazon S3 User Guide, *Bucket naming
    // rules*).
    assert_eq!(
        SHORTEST_BUCKET.chars().count(),
        3,
        "the shortest-name row must be 3 characters, got {:?}",
        SHORTEST_BUCKET
    );
    assert_eq!(
        LONGEST_BUCKET.chars().count(),
        63,
        "the longest-name row must be 63 characters, got {} in {:?}",
        LONGEST_BUCKET.chars().count(),
        LONGEST_BUCKET
    );
}

#[tokio::test]
async fn a_bucket_scoped_refusal_names_the_bucket_it_is_about() {
    let router = router();

    for bucket in [
        SHORTEST_BUCKET,
        LONGEST_BUCKET,
        DOTTED_BUCKET,
        DIGIT_BUCKET,
        SUBRESOURCE_BUCKET,
        NON_EMPTY_BUCKET,
        HOST_STYLE_BUCKET,
    ] {
        created(&router, bucket, Some(CONFLICT_REGION)).await;
    }

    // A bucket is created carrying ownership controls and a Block Public Access
    // configuration, so the absent-subresource rows for those two need them withdrawn.
    withdrawn(&router, SUBRESOURCE_BUCKET, "ownershipControls").await;
    withdrawn(&router, SUBRESOURCE_BUCKET, "publicAccessBlock").await;

    // The bucket a delete is refused on: it still holds an object.
    let stored = put_object(&router, NON_EMPTY_BUCKET, "held.txt", b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the setup write must succeed, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    // An object to read, so the object-scoped negative row refuses on the KEY rather than
    // on the bucket.
    let stored = put_object(&router, DIGIT_BUCKET, "present.txt", b"present", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the setup write must succeed, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    let rows = [
        // The bucket itself is not there — the same condition read through four operations
        // and three methods, and named by four differently shaped bucket names.
        Row {
            case: "GetBucketAbac on a bucket that does not exist",
            method: "GET",
            uri: "/envelope-absent?abac",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent"),
        },
        Row {
            case: "GetBucketPolicy on a bucket that does not exist",
            method: "GET",
            uri: "/envelope-absent-policy?policy",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent-policy"),
        },
        Row {
            case: "GetPublicAccessBlock on a bucket that does not exist",
            method: "GET",
            uri: "/env-absent?publicAccessBlock",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("env-absent"),
        },
        Row {
            case: "GetBucketOwnershipControls on a bucket that does not exist",
            method: "GET",
            uri: "/envelope.absent.dotted?ownershipControls",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope.absent.dotted"),
        },
        Row {
            case: "DeleteBucket on a bucket that does not exist",
            method: "DELETE",
            uri: "/envelope-absent-2026",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent-2026"),
        },
        Row {
            case: "GetBucketCors on a bucket that does not exist",
            method: "GET",
            uri: "/envelope-absent-cors?cors",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent-cors"),
        },
        Row {
            case: "GetBucketLifecycleConfiguration on a bucket that does not exist",
            method: "GET",
            uri: "/envelope-absent-lifecycle?lifecycle",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent-lifecycle"),
        },
        Row {
            case: "GetBucketWebsite on a bucket that does not exist",
            method: "GET",
            uri: "/envelope-absent-website?website",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("envelope-absent-website"),
        },
        Row {
            case: "GetBucketAbac on the shortest bucket name there can be, absent",
            method: "GET",
            uri: "/abs?abac",
            status: 404,
            code: "NoSuchBucket",
            bucket: Some("abs"),
        },
        // The bucket IS there and the subresource is not: one row per bucket-level
        // subresource that can be absent.
        Row {
            case: "GetBucketPolicy on a bucket with no policy",
            method: "GET",
            uri: "/envelope-subresources?policy",
            status: 404,
            code: "NoSuchBucketPolicy",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketCors on a bucket with no cors configuration",
            method: "GET",
            uri: "/envelope-subresources?cors",
            status: 404,
            code: "NoSuchCORSConfiguration",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketLifecycleConfiguration on a bucket with no lifecycle rules",
            method: "GET",
            uri: "/envelope-subresources?lifecycle",
            status: 404,
            code: "NoSuchLifecycleConfiguration",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketWebsite on a bucket with no website configuration",
            method: "GET",
            uri: "/envelope-subresources?website",
            status: 404,
            code: "NoSuchWebsiteConfiguration",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketTagging on a bucket that was never tagged",
            method: "GET",
            uri: "/envelope-subresources?tagging",
            status: 404,
            code: "NoSuchTagSet",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketReplication on a bucket with no replication configuration",
            method: "GET",
            uri: "/envelope-subresources?replication",
            status: 404,
            code: "ReplicationConfigurationNotFoundError",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetObjectLockConfiguration on a bucket with no Object Lock configuration",
            method: "GET",
            uri: "/envelope-subresources?object-lock",
            status: 404,
            code: "ObjectLockConfigurationNotFoundError",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetBucketOwnershipControls on a bucket whose controls were withdrawn",
            method: "GET",
            uri: "/envelope-subresources?ownershipControls",
            status: 404,
            code: "OwnershipControlsNotFoundError",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        Row {
            case: "GetPublicAccessBlock on a bucket whose configuration was withdrawn",
            method: "GET",
            uri: "/envelope-subresources?publicAccessBlock",
            status: 404,
            code: "NoSuchPublicAccessBlockConfiguration",
            bucket: Some(SUBRESOURCE_BUCKET),
        },
        // The two CONFLICTS, which are about the bucket as a whole.
        Row {
            case: "DeleteBucket on a bucket that still holds an object",
            method: "DELETE",
            uri: "/envelope-not-empty",
            status: 409,
            code: "BucketNotEmpty",
            bucket: Some(NON_EMPTY_BUCKET),
        },
        Row {
            case: "CreateBucket of a bucket the caller already owns",
            method: "PUT",
            uri: "/envelope-bucket-2026",
            status: 409,
            code: "BucketAlreadyOwnedByYou",
            bucket: Some(DIGIT_BUCKET),
        },
        Row {
            case: "CreateBucket of the longest-named bucket the caller already owns",
            method: "PUT",
            uri: "/envelope-bucket-with-the-longest-name-a-bucket-name-may-carry-x",
            status: 409,
            code: "BucketAlreadyOwnedByYou",
            bucket: Some(LONGEST_BUCKET),
        },
        Row {
            case: "CreateBucket of a dotted bucket the caller already owns",
            method: "PUT",
            uri: "/envelope.dotted.bucket",
            status: 409,
            code: "BucketAlreadyOwnedByYou",
            bucket: Some(DOTTED_BUCKET),
        },
        Row {
            case: "CreateBucket of the shortest-named bucket the caller already owns",
            method: "PUT",
            uri: "/env",
            status: 409,
            code: "BucketAlreadyOwnedByYou",
            bucket: Some(SHORTEST_BUCKET),
        },
        // Refusals whose condition is NOT a bucket's: no element at all.
        Row {
            case: "GetObject of a key the bucket does not hold",
            method: "GET",
            uri: "/envelope-bucket-2026/absent.txt",
            status: 404,
            code: "NoSuchKey",
            bucket: None,
        },
        Row {
            case: "a read refused for the expected bucket owner",
            method: "GET",
            uri: "/envelope-subresources?abac",
            status: 403,
            code: "AccessDenied",
            bucket: None,
        },
        Row {
            case: "an operation this emulator has not written",
            method: "GET",
            uri: "/envelope-subresources?metadataTable",
            status: 501,
            code: "NotImplemented",
            bucket: None,
        },
    ];

    for row in &rows {
        let Row {
            case,
            method,
            uri,
            status,
            code,
            bucket,
        } = row;
        // The one row that is about the CALLER rather than the bucket is provoked by naming
        // an account that does not own the bucket; every other row needs no header.
        let headers: Vec<(&str, &str)> = match *status == 403 {
            true => vec![("x-amz-expected-bucket-owner", "123456789012")],
            false => vec![],
        };
        // A create's body names the Region, so a second create of the same bucket is a
        // conflict rather than the documented legacy 200 of `us-east-1`.
        let body = match *method == "PUT" {
            true => format!(
                "<CreateBucketConfiguration \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <LocationConstraint>{CONFLICT_REGION}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            ),
            false => String::new(),
        };
        let answer = send(&router, method, uri, &headers, &body).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.element(BUCKET_NAME_ELEMENT).as_deref(),
            *bucket,
            "case {case:?}: expected <BucketName> {bucket:?}, got {:?} in body {:?}",
            answer.element(BUCKET_NAME_ELEMENT),
            String::from_utf8_lossy(&answer.body)
        );
        // The element names the bucket, and the rest of the envelope is unchanged: the code
        // and the message are still there, and the correlation elements still close it.
        if bucket.is_some() {
            assert!(
                !answer.error_message().trim().is_empty(),
                "case {case:?}: the envelope must still carry its message, got {:?}",
                answer.error_message()
            );
            for element in ["RequestId", "HostId"] {
                assert!(
                    answer.element(element).is_some_and(|id| !id.is_empty()),
                    "case {case:?}: the envelope must still carry a {element}, got {:?}",
                    answer.element(element)
                );
            }
        }
    }
}

#[tokio::test]
async fn a_virtual_hosted_style_request_is_named_by_the_bucket_in_its_host() {
    // The bucket is read from the request's ADDRESS, which a virtual-hosted-style request
    // carries in the first label of its host and leaves out of its path entirely — so the
    // envelope names the same bucket either way.
    let router = router();
    created(&router, HOST_STYLE_BUCKET, Some(CONFLICT_REGION)).await;

    let host = format!("{HOST_STYLE_BUCKET}.s3.{CONFLICT_REGION}.amazonaws.com");
    let answer = super::bucket_lifecycle::send_bytes_to_host(
        &router,
        "GET",
        &host,
        "/?cors",
        &[("content-type", "application/xml")],
        b"",
    )
    .await;

    assert_eq!(
        answer.status, 404,
        "a host-style read of an absent cors configuration must be a 404, got {} with \
         body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchCORSConfiguration",
        "expected the absent-subresource code, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.element(BUCKET_NAME_ELEMENT).as_deref(),
        Some(HOST_STYLE_BUCKET),
        "the envelope must name the bucket the HOST addressed, got {:?} in body {:?}",
        answer.element(BUCKET_NAME_ELEMENT),
        String::from_utf8_lossy(&answer.body)
    );
}

#[tokio::test]
async fn a_served_response_carries_no_bucket_name_element() {
    // The rule is about ERROR documents. A served read of the same subresource carries no
    // such element, so nothing is writing one unconditionally.
    let router = router();
    created(&router, SUBRESOURCE_BUCKET, Some(CONFLICT_REGION)).await;

    let configured = send(
        &router,
        "PUT",
        &format!("/{SUBRESOURCE_BUCKET}?cors"),
        &[],
        "<CORSConfiguration><CORSRule><AllowedMethod>GET</AllowedMethod>\
         <AllowedOrigin>https://example.com</AllowedOrigin></CORSRule></CORSConfiguration>",
    )
    .await;
    assert!(
        (200..300).contains(&configured.status),
        "the setup put of a cors configuration must succeed, got {} with body {:?}",
        configured.status,
        String::from_utf8_lossy(&configured.body)
    );

    let read = send(
        &router,
        "GET",
        &format!("/{SUBRESOURCE_BUCKET}?cors"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "the configured subresource must be served, got {} with body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.element(BUCKET_NAME_ELEMENT),
        None,
        "a served response must carry no <BucketName>, got {:?}",
        read.element(BUCKET_NAME_ELEMENT)
    );
}

#[tokio::test]
async fn a_bucket_name_in_the_envelope_is_xml_escaped() {
    // The element's text is a value the CALLER chose, so it is written through the codec
    // that escapes XML metacharacters rather than pasted into the document: a bucket name
    // carrying an ampersand must leave the envelope well-formed and read back unchanged.
    // An ampersand is the one metacharacter a URI path may legally carry, so it is what a
    // request can actually address a bucket with.
    let router = router();
    let answer = send(&router, "GET", "/envelope&escaped?cors", &[], "").await;

    assert_eq!(
        answer.status, 404,
        "a read of a bucket that does not exist must be a 404, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.element(BUCKET_NAME_ELEMENT).as_deref(),
        Some("envelope&escaped"),
        "the envelope must name the bucket the request addressed, got {:?}",
        answer.element(BUCKET_NAME_ELEMENT)
    );
    let body = String::from_utf8_lossy(&answer.body).to_string();
    assert!(
        body.contains("<BucketName>envelope&amp;escaped</BucketName>"),
        "the metacharacter must be escaped in the document, got {body:?}"
    );
    // And the document as a whole still parses, so nothing the caller chose broke it.
    let mut reader = quick_xml::Reader::from_str(&body);
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the error document must be well-formed: {error}"))
        {
            quick_xml::events::Event::Eof => break,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn a_copy_refused_for_a_bucket_names_the_bucket_that_condition_is_about() {
    // The element names the bucket the REFUSAL is about, which for a copy is not always the
    // bucket the request addresses: a copy reads from the bucket `x-amz-copy-source` names
    // and writes into the bucket in the path, and either can be the one that is not there.
    // Each row drives the two copy operations and pins which of the two names the envelope
    // carries.
    let router = router();
    created(&router, "envelope-copy-source", Some(CONFLICT_REGION)).await;
    created(&router, "envelope-copy-target", Some(CONFLICT_REGION)).await;
    let stored = put_object(&router, "envelope-copy-source", "held.txt", b"held", &[]).await;
    assert_eq!(
        stored.status, 200,
        "the setup write must succeed, got {} with body {:?}",
        stored.status,
        String::from_utf8_lossy(&stored.body)
    );

    // `CopyObject`: the source bucket is the one that is not there, so the envelope names
    // the SOURCE and not the destination the request addressed.
    let refused = send(
        &router,
        "PUT",
        "/envelope-copy-target/copied.txt",
        &[("x-amz-copy-source", "/envelope-copy-absent/held.txt")],
        "",
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (404, "NoSuchBucket"),
        "a copy from a bucket that does not exist must be refused, got {} / {} with body {:?}",
        refused.status,
        refused.error_code(),
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.element(BUCKET_NAME_ELEMENT).as_deref(),
        Some("envelope-copy-absent"),
        "the envelope must name the SOURCE bucket the copy could not read, got {:?} in {:?}",
        refused.element(BUCKET_NAME_ELEMENT),
        String::from_utf8_lossy(&refused.body)
    );

    // The mirror case on the same operation: the DESTINATION is the bucket that is not
    // there, and that is what the envelope names.
    let refused = send(
        &router,
        "PUT",
        "/envelope-copy-nowhere/copied.txt",
        &[("x-amz-copy-source", "/envelope-copy-source/held.txt")],
        "",
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (404, "NoSuchBucket"),
        "a copy into a bucket that does not exist must be refused, got {} / {} with body {:?}",
        refused.status,
        refused.error_code(),
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.element(BUCKET_NAME_ELEMENT).as_deref(),
        Some("envelope-copy-nowhere"),
        "the envelope must name the DESTINATION bucket the copy could not write, got {:?} \
         in {:?}",
        refused.element(BUCKET_NAME_ELEMENT),
        String::from_utf8_lossy(&refused.body)
    );

    // `UploadPartCopy` reads its source the same way, so the same rule holds for a part
    // copied into an upload in progress.
    let initiated = send(
        &router,
        "POST",
        "/envelope-copy-target/assembled.txt?uploads",
        &[],
        "",
    )
    .await;
    assert_eq!(
        initiated.status, 200,
        "the setup initiation must succeed, got {} with body {:?}",
        initiated.status,
        String::from_utf8_lossy(&initiated.body)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");
    let refused = send(
        &router,
        "PUT",
        &format!(
            "/envelope-copy-target/assembled.txt?partNumber=1&uploadId={upload_id}\
             &x-id=UploadPartCopy"
        ),
        &[("x-amz-copy-source", "/envelope-copy-absent/held.txt")],
        "",
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (404, "NoSuchBucket"),
        "a part copied from a bucket that does not exist must be refused, got {} / {} with \
         body {:?}",
        refused.status,
        refused.error_code(),
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.element(BUCKET_NAME_ELEMENT).as_deref(),
        Some("envelope-copy-absent"),
        "the envelope must name the SOURCE bucket the part copy could not read, got {:?} \
         in {:?}",
        refused.element(BUCKET_NAME_ELEMENT),
        String::from_utf8_lossy(&refused.body)
    );
}

#[tokio::test]
async fn every_bucket_scoped_code_is_covered_by_a_request() {
    // The catalog is the rule's scope, so every code in it must be a refusal this suite
    // actually provokes — otherwise a row could be added to the catalog that nothing drives.
    // `BucketAlreadyExists` is the one code no request to THIS emulator can provoke: it is
    // the name-taken-by-another-account conflict, and this emulator serves one account, so
    // a second create by the same caller is `BucketAlreadyOwnedByYou`.
    let covered = [
        "BucketAlreadyOwnedByYou",
        "BucketNotEmpty",
        "NoSuchBucket",
        "NoSuchBucketPolicy",
        "NoSuchCORSConfiguration",
        "NoSuchLifecycleConfiguration",
        "NoSuchPublicAccessBlockConfiguration",
        "NoSuchTagSet",
        "NoSuchWebsiteConfiguration",
        "ObjectLockConfigurationNotFoundError",
        "OwnershipControlsNotFoundError",
        "ReplicationConfigurationNotFoundError",
    ];
    let unprovokable = ["BucketAlreadyExists"];

    for code in BUCKET_SCOPED_ERROR_CODES {
        assert!(
            covered.contains(code) || unprovokable.contains(code),
            "case {code:?}: the catalog names a bucket-scoped code no row of this suite \
             drives; add a row for it"
        );
    }
    for code in covered {
        assert!(
            BUCKET_SCOPED_ERROR_CODES.contains(&code),
            "case {code:?}: this suite drives a code the catalog does not name; the \
             catalog is {BUCKET_SCOPED_ERROR_CODES:?}"
        );
    }
}

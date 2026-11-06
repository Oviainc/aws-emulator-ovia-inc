// SPDX-License-Identifier: CC0-1.0

//! A read whose SigV4 credential travels in the QUERY STRING — what a presigned URL is —
//! resolves the same object the header-signed API path resolves, and so does one that
//! addresses its bucket in the HOST rather than in the path.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a browser or a third party fetching a presigned URL sees: the status, the
//! bytes and the bound headers.
//!
//! The rules under test:
//!
//! 1. WHERE the credential travels is not part of addressing. A request carrying
//!    `X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`,
//!    `X-Amz-SignedHeaders` and `X-Amz-Signature` in its query and NO `Authorization` header
//!    names the same bucket and key a signed `GetObject` names, and is answered with the
//!    same object — so a presigned GET of a stored key serves its bytes with HTTP 200, and a
//!    presigned GET of a key the bucket does not hold is `NoSuchKey` (404) for the same
//!    reason a signed one is: the key is absent, not the route.
//! 2. WHERE the bucket travels is not part of addressing either: "Both the
//!    virtual-hosted-style requests and the path-style requests are supported" for a general
//!    purpose bucket (`GetObject`'s own documentation), so
//!    `https://bucket-name.s3.region-code.amazonaws.com/key-name` resolves the same object
//!    `https://s3.region-code.amazonaws.com/bucket-name/key-name` does. Every AWS SDK's URL
//!    presigner writes the first form for an endpoint addressed by name.
//!
//! The rows differ along what the rules generalise over: how the object came to be (a
//! one-shot write and a completed multipart upload), the FORM of the key (a plain name and
//! one with `/` separators), what the read asks for (the whole object and one part of it),
//! which operation the address resolves (a read of an object, a read of its metadata, a
//! listing of the bucket), and whether the key exists at all.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, Answer};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_partial_reads::{completed_multipart_object, seeded_payload};

/// The signing algorithm a presigned URL names in `X-Amz-Algorithm`.
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// A presigned URL's validity, in seconds — the `X-Amz-Expires` a client asks for.
const EXPIRES_SECS: u64 = 900;

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The query a presigned URL carries, percent-encoded the way a client's signer writes it.
///
/// The six fields botocore's `generate_presigned_url` puts on the wire. The signature value
/// is not verified by this harness (the router it drives has request authentication off, as
/// every suite in this crate does), because what is under test is the ROUTE a
/// query-authenticated read resolves, not the SigV4 comparison — which
/// [`super::signature_refusal_wording`] pins against a router with authentication on.
fn presigned_query() -> String {
    let scope = "emulator-test-key/20260101/us-east-1/s3/aws4_request".replace('/', "%2F");
    format!(
        "X-Amz-Algorithm={SIGV4_ALGORITHM}&X-Amz-Credential={scope}\
         &X-Amz-Date=20260101T000000Z&X-Amz-Expires={EXPIRES_SECS}\
         &X-Amz-SignedHeaders=host\
         &X-Amz-Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Fetch `uri` the way a client fetches a presigned URL: the presigned query appended, and
/// NO `Authorization` header at all.
async fn fetch_presigned(router: &MultiServiceRouter, uri: &str) -> Answer {
    let separator = if uri.contains('?') { '&' } else { '?' };
    send_unsigned(
        router,
        "GET",
        "s3.us-east-1.amazonaws.com",
        &format!("{uri}{separator}{}", presigned_query()),
    )
    .await
}

/// Dispatch one request to `host` carrying no `Authorization` header — the shape of both a
/// presigned fetch and an anonymous read.
async fn send_unsigned(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
) -> Answer {
    send_to_host(router, method, host, uri, false).await
}

/// Dispatch one request to `host` carrying this suite's `Authorization` header.
///
/// What the host-style rows below are about is the ADDRESS a request resolves, so they
/// present a credential: a request carrying none speaks for nobody and is served only what
/// the bucket grants to everyone, which is a different rule and is pinned by
/// [`super::anonymous_object_reads`].
async fn send_signed(router: &MultiServiceRouter, method: &str, host: &str, uri: &str) -> Answer {
    send_to_host(router, method, host, uri, true).await
}

/// The `Authorization` header the suites in this crate sign with: a SigV4 scope naming `s3`,
/// which is the routing signal a SIGNED request carries whatever its host is. Neither the
/// key id nor the signature is a credential — this harness's router has request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// Dispatch one request to `host`, signed or not.
async fn send_to_host(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    signed: bool,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host);
    if signed {
        builder = builder.header("authorization", AUTHORIZATION);
    }
    let request = builder
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

/// `length` bytes from this suite's own seed — see [`seeded_payload`].
fn sized_payload(length: usize) -> Vec<u8> {
    seeded_payload(0x1234_5678, length)
}

/// Complete a multipart upload of `parts` at `key`, answering the bytes the object holds
/// afterwards — through the one multipart harness this test crate owns
/// ([`completed_multipart_object`]).
async fn multipart_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    parts: &[Vec<u8>],
) -> Vec<u8> {
    completed_multipart_object(router, name, key, parts, &[]).await;
    parts.concat()
}

#[tokio::test]
async fn a_query_authenticated_read_serves_the_object_the_signed_path_serves() {
    let router = router();
    bucket(&router, "presigned-bucket").await;

    // A one-shot write, and a completed two-part upload — the two ways a key comes to hold
    // bytes — plus a key written under `/` separators, which a presigned URL carries in its
    // path exactly as a signed request does.
    let written = sized_payload(2_048);
    let nested = sized_payload(37);
    let stored = put_object(&router, "presigned-bucket", "written.bin", &written, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the write must succeed, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let stored_nested = put_object(&router, "presigned-bucket", "deep/dir/nested.bin", &nested, &[])
        .await;
    assert_eq!(
        stored_nested.status,
        200,
        "the nested write must succeed, got {} with body {:?}",
        stored_nested.status,
        text(&stored_nested)
    );
    let parts = vec![
        sized_payload(MIN_MULTIPART_PART_BYTES),
        sized_payload(1_024),
    ];
    let assembled = multipart_object(&router, "presigned-bucket", "assembled.bin", &parts).await;

    let rows: [(&str, &str, Vec<u8>); 3] = [
        ("a one-shot write", "/presigned-bucket/written.bin", written),
        (
            "a key with / separators",
            "/presigned-bucket/deep/dir/nested.bin",
            nested,
        ),
        (
            "a completed multipart upload",
            "/presigned-bucket/assembled.bin",
            assembled,
        ),
    ];
    for (case, uri, expected) in rows {
        let fetched = fetch_presigned(&router, uri).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: a presigned read of a stored key must serve it with HTTP 200, got \
             {} with body {:?}",
            fetched.status,
            text(&fetched)
        );
        assert_eq!(
            fetched.body.as_ref(),
            expected.as_slice(),
            "case {case:?}: a presigned read must serve the stored bytes; got {} bytes",
            fetched.body.len()
        );
        assert_eq!(
            fetched.header("content-length"),
            Some(expected.len().to_string()),
            "case {case:?}: the advertised length must be the served length, got {:?}",
            fetched.header("content-length")
        );
    }

    // The same read asking for ONE PART of the completed upload: a presigned URL carries
    // whatever the read asks for, so the part-scoped answer is the same one the signed path
    // gives.
    let part = fetch_presigned(&router, "/presigned-bucket/assembled.bin?partNumber=2").await;
    assert_eq!(
        part.status,
        206,
        "case \"a presigned part-scoped read\": expected HTTP 206, got {} with body {:?}",
        part.status,
        text(&part)
    );
    assert_eq!(
        part.body.as_ref(),
        parts[1].as_slice(),
        "case \"a presigned part-scoped read\": expected the second part's bytes, got {} bytes",
        part.body.len()
    );

    // The boundary: a presigned read of a key the bucket does not hold is refused for the
    // KEY, which is what tells "the object is not there" from "the request did not route".
    let absent = fetch_presigned(&router, "/presigned-bucket/absent.bin").await;
    assert_eq!(
        absent.status,
        404,
        "case \"an absent key\": expected HTTP 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "NoSuchKey",
        "case \"an absent key\": expected the NoSuchKey code, got {:?}",
        absent.error_code()
    );
}

#[tokio::test]
async fn a_bucket_addressed_in_the_host_resolves_the_same_object_the_path_does() {
    let router = router();
    bucket(&router, "host-style-bucket").await;

    let written = sized_payload(1_031);
    let nested = sized_payload(97);
    for (key, body) in [("written.bin", &written), ("deep/dir/nested.bin", &nested)] {
        let stored = put_object(&router, "host-style-bucket", key, body, &[]).await;
        assert_eq!(
            stored.status,
            200,
            "the write of {key:?} must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
    }
    let parts = vec![sized_payload(MIN_MULTIPART_PART_BYTES), sized_payload(512)];
    let assembled = multipart_object(&router, "host-style-bucket", "assembled.bin", &parts).await;

    // The hosts a virtual-hosted-style request carries: the Regional endpoint, the global
    // one, and the LOCAL endpoint an emulator is reached at — a client configured with a
    // local endpoint URL addresses a bucket by host exactly as it does against AWS.
    let hosts = [
        "host-style-bucket.s3.us-east-1.amazonaws.com",
        "host-style-bucket.s3.amazonaws.com",
        "host-style-bucket.localhost",
    ];
    for host in hosts {
        // A read of an OBJECT: the path is the key alone.
        for (key, expected) in [
            ("/written.bin", &written),
            ("/deep/dir/nested.bin", &nested),
            ("/assembled.bin", &assembled),
        ] {
            let fetched = send_signed(&router, "GET", host, key).await;
            assert_eq!(
                fetched.status,
                200,
                "case {host:?} {key:?}: a host-style read must serve the object with HTTP 200, \
                 got {} with body {:?}",
                fetched.status,
                text(&fetched)
            );
            assert_eq!(
                fetched.body.as_ref(),
                expected.as_slice(),
                "case {host:?} {key:?}: expected the stored bytes, got {} bytes",
                fetched.body.len()
            );
        }

        // A read of the object's METADATA, and a LISTING of the bucket the host names — the
        // same two addresses, resolving the operations their path-style forms resolve.
        let head = send_signed(&router, "HEAD", host, "/written.bin").await;
        assert_eq!(
            head.status,
            200,
            "case {host:?}: a host-style HEAD must answer HTTP 200, got {}",
            head.status
        );
        assert_eq!(
            head.header("content-length"),
            Some(written.len().to_string()),
            "case {host:?}: a host-style HEAD must report the object's size, got {:?}",
            head.header("content-length")
        );
        let listed = send_signed(&router, "GET", host, "/?list-type=2").await;
        assert_eq!(
            listed.status,
            200,
            "case {host:?}: a host-style listing must answer HTTP 200, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert!(
            text(&listed).contains("<Name>host-style-bucket</Name>"),
            "case {host:?}: the listing must be of the bucket the host names, got {:?}",
            text(&listed)
        );

        // A key the bucket does not hold is absent, not unroutable.
        let absent = send_signed(&router, "GET", host, "/absent.bin").await;
        assert_eq!(
            absent.error_code(),
            "NoSuchKey",
            "case {host:?}: expected the NoSuchKey code, got {:?} with status {}",
            absent.error_code(),
            absent.status
        );
    }

    // The boundary of the rule. A host that is NOT one of this service's virtual-hosted
    // addresses carries no bucket at all, so its first path segment is the bucket and an
    // endpoint whose own name nothing here knows resolves as it always did. A host that IS
    // one names the bucket in its first label whether or not this emulator holds it — the
    // address is read off the host before any store is consulted (the core's
    // `VirtualHostAddressing`) — so a request for a bucket that is not there answers about
    // THAT bucket rather than resolving from the path.
    let path_style = send_signed(
        &router,
        "GET",
        "s3.us-east-1.amazonaws.com",
        "/host-style-bucket/written.bin",
    )
    .await;
    assert_eq!(
        path_style.status,
        200,
        "case \"a path-style read\": expected HTTP 200, got {} with body {:?}",
        path_style.status,
        text(&path_style)
    );
    assert_eq!(
        path_style.body.as_ref(),
        written.as_slice(),
        "case \"a path-style read\": expected the stored bytes, got {} bytes",
        path_style.body.len()
    );
    // The boundary of the rule: in a virtual-hosted-style address "the bucket name is part
    // of the domain name", so the HOST names the bucket whatever the path spells and the
    // path is the key alone. A host whose first label is a bucket this emulator does not
    // hold is therefore a read OF THAT BUCKET — `NoSuchBucket` — and not a path-style read
    // of the bucket its first path segment happens to name.
    let absent_host_bucket = send_signed(
        &router,
        "GET",
        "absent-host-bucket.s3.us-east-1.amazonaws.com",
        "/host-style-bucket/written.bin",
    )
    .await;
    assert_eq!(
        absent_host_bucket.error_code(),
        "NoSuchBucket",
        "case \"a host label naming no bucket\": the read must be about the bucket the HOST \
         names, got {:?} with status {} and body {:?}",
        absent_host_bucket.error_code(),
        absent_host_bucket.status,
        text(&absent_host_bucket)
    );
    assert_eq!(
        absent_host_bucket.element("BucketName").as_deref(),
        Some("absent-host-bucket"),
        "case \"a host label naming no bucket\": the refusal must name the host's bucket, got \
         {:?}",
        text(&absent_host_bucket)
    );
    // The other half of that boundary: a host that is NOT one of this service's
    // virtual-hosted addresses — a local endpoint reached under its own name — carries no
    // bucket at all, so its first path segment is still the bucket, which is what leaves an
    // endpoint whose own name nothing here knows resolving as it always did.
    let local_endpoint = send_signed(
        &router,
        "GET",
        "absent-host-bucket.localhost",
        "/host-style-bucket/written.bin",
    )
    .await;
    assert_eq!(
        local_endpoint.body.as_ref(),
        written.as_slice(),
        "case \"an endpoint whose own name is not a bucket\": the read must resolve from the \
         path, got {} bytes with status {}",
        local_endpoint.body.len(),
        local_endpoint.status
    );
}

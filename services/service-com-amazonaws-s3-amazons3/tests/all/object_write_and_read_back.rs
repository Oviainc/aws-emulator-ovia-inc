// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the object write-and-read-back loop: what `PutObject` stores is
//! what `HeadObject` reports.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers) rather than any handler's internals. The
//! refusals — a key or bucket this emulator does not hold, and the members a HEAD cannot
//! answer — are in `head_object_read_back.rs` beside this suite.
//!
//! The rule under test, asserted over a SET of inputs rather than one request: a read
//! reports the stored object — its `ETag` (the quoted hex MD5 of the stored bytes,
//! `HeadObjectOutput$ETag`), its `Content-Length`, its `Last-Modified`, the headers the
//! write carried and its `x-amz-meta-*` user metadata — and a second write to one key
//! REPLACES the whole object ("You cannot use PutObject to only update a single piece of
//! metadata for an existing object").

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A bound response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// Dispatch one request with the given headers and raw body bytes.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `PUT /<bucket>` — `CreateBucket` in the default Region.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "PUT", &format!("/{bucket}"), &[], Vec::new()).await
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}"),
        headers,
        body.to_vec(),
    )
    .await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`.
async fn head_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send(
        router,
        "HEAD",
        &format!("/{bucket}/{key}"),
        &[],
        Vec::new(),
    )
    .await
}

/// One row of the read-back table: the case name, the key written, the bytes, and the hex
/// MD5 those bytes are documented to hash to (`None` where the row's digest is not a
/// published vector, so the row is checked against the ETag the write reported instead).
type ReadRow<'a> = (&'a str, &'a str, &'a [u8], Option<&'a str>);

/// The published MD5 digests this suite expects, NOT values read back out of this
/// emulator: the vectors of RFC 1321's own test suite (appendix A.5, "MD5 test suite"),
/// plus the `5eb63bbbe01eeed093cb22bb8f5acdc3` ETag for `hello world` the S3 bootstrap
/// issue cites. Each is the hex digest; S3's ETag is that digest in double quotes.
///
/// The rows differ along what the rule generalises over: the size of the body (empty, one
/// byte, 4 KiB), what it holds (text, digits, bytes that are not UTF-8 at all) and the
/// shape of the key (flat, nested).
const READ_ROWS: &[ReadRow<'static>] = &[
    (
        "the empty body, the low end of what a write may carry",
        "empty.bin",
        b"",
        Some("d41d8cd98f00b204e9800998ecf8427e"),
    ),
    (
        "a single byte",
        "one-byte.txt",
        b"a",
        Some("0cc175b9c0f1b6a831c399e269772661"),
    ),
    (
        "three bytes",
        "abc.txt",
        b"abc",
        Some("900150983cd24fb0d6963f7d28e17f72"),
    ),
    (
        "a body with a space in it",
        "message-digest.txt",
        b"message digest",
        Some("f96b697d7cb7938d525a2f31aaf161d0"),
    ),
    (
        "the lower-case alphabet, under a nested key",
        "nested/key/with/slashes.txt",
        b"abcdefghijklmnopqrstuvwxyz",
        Some("c3fcd3d76192e4007dfb496cca67e13b"),
    ),
    (
        "eighty digits",
        "digits.txt",
        b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
        Some("57edf4a22be3c955ac49da2e2107b67a"),
    ),
    (
        "the body the bootstrap issue cites",
        "a/hello.txt",
        b"hello world",
        Some("5eb63bbbe01eeed093cb22bb8f5acdc3"),
    ),
    (
        "bytes that are not text at all",
        "binary.zip",
        // A minimal ZIP end-of-central-directory record: the kind of body the plugin
        // upload a kafkaconnect client performs carries, and one that is not valid UTF-8.
        b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        None,
    ),
    (
        "a body of many identical bytes",
        "repeated.bin",
        &[0x61; 4096],
        None,
    ),
];

#[tokio::test]
async fn a_written_object_reads_back_with_the_etag_size_and_metadata_it_was_written_with() {
    let router = router();
    let bucket = "read-back-bucket";
    assert_eq!(
        create_bucket(&router, bucket).await.status,
        200,
        "the bucket the rows write into must be created first"
    );

    let mut etags: Vec<(&str, String)> = Vec::new();
    for (case, key, body, digest) in READ_ROWS {
        let written = put_object(
            &router,
            bucket,
            key,
            &[
                ("content-type", "application/octet-stream"),
                ("cache-control", "max-age=60"),
                ("content-language", "en-GB"),
                ("x-amz-meta-owner", "alice"),
                ("x-amz-meta-purpose", "round-trip"),
            ],
            body,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
        let etag = written
            .header("etag")
            .unwrap_or_else(|| panic!("case {case:?}: PutObject must report an ETag"));
        if let Some(digest) = digest {
            assert_eq!(
                etag,
                format!("\"{digest}\""),
                "case {case:?}: expected the quoted MD5 of the body, got {etag:?}"
            );
        }

        let read = head_object(&router, bucket, key).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from HeadObject on the key just written, \
             got {}",
            read.status
        );
        assert_eq!(
            read.header("etag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: the read must report the ETag the write did, got {:?}",
            read.header("etag")
        );
        assert_eq!(
            read.header("content-length").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: expected ContentLength {}, got {:?}",
            body.len(),
            read.header("content-length")
        );
        assert_eq!(
            read.header("content-type").as_deref(),
            Some("application/octet-stream"),
            "case {case:?}: the read must report the stored ContentType, got {:?}",
            read.header("content-type")
        );
        assert_eq!(
            read.header("cache-control").as_deref(),
            Some("max-age=60"),
            "case {case:?}: the read must report the stored CacheControl, got {:?}",
            read.header("cache-control")
        );
        assert_eq!(
            read.header("content-language").as_deref(),
            Some("en-GB"),
            "case {case:?}: the read must report the stored ContentLanguage, got {:?}",
            read.header("content-language")
        );
        assert_eq!(
            read.header("x-amz-meta-owner").as_deref(),
            Some("alice"),
            "case {case:?}: the read must report the stored user metadata, got {:?}",
            read.header("x-amz-meta-owner")
        );
        assert_eq!(
            read.header("x-amz-meta-purpose").as_deref(),
            Some("round-trip"),
            "case {case:?}: every stored metadata entry must read back, got {:?}",
            read.header("x-amz-meta-purpose")
        );
        // "Amazon S3 returns this header for all objects except for S3 Standard storage
        // class objects" (`HeadObjectOutput$StorageClass`): a write naming no class stores
        // STANDARD, which the read reports by carrying no storage-class header at all.
        assert_eq!(
            read.header("x-amz-storage-class"),
            None,
            "case {case:?}: a read of a STANDARD object must carry no storage class, got {:?}",
            read.header("x-amz-storage-class")
        );
        assert_eq!(
            read.header("accept-ranges").as_deref(),
            Some("bytes"),
            "case {case:?}: a stored object reports it can be read by range, got {:?}",
            read.header("accept-ranges")
        );
        assert!(
            read.header("last-modified").is_some(),
            "case {case:?}: the read must report when the object was written"
        );

        etags.push((case, etag));
    }

    // The ETag identifies the bytes: two rows carrying different bytes never share one.
    for (left_index, (left_case, left)) in etags.iter().enumerate() {
        for (right_case, right) in &etags[left_index + 1..] {
            assert_ne!(
                left, right,
                "cases {left_case:?} and {right_case:?} carry different bytes, so their \
                 ETags must differ, got {left:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_second_write_to_one_key_replaces_the_whole_object() {
    let router = router();
    let bucket = "overwrite-bucket";
    create_bucket(&router, bucket).await;
    let key = "replaced.txt";

    /// One row of the overwrite table: the case, the body, its content type, and the user
    /// metadata the write carries (if any).
    type OverwriteRow<'a> = (&'a str, &'a [u8], &'a str, Option<(&'a str, &'a str)>);
    let writes: [OverwriteRow<'_>; 4] = [
        (
            "the first write",
            b"abc",
            "text/plain",
            Some(("x-amz-meta-stage", "first")),
        ),
        (
            "a longer body, new metadata",
            b"abcdefghijklmnopqrstuvwxyz",
            "application/json",
            Some(("x-amz-meta-stage", "second")),
        ),
        (
            "a write carrying no metadata at all drops the metadata before it",
            b"a",
            "application/octet-stream",
            None,
        ),
        ("back to the empty body", b"", "text/plain", None),
    ];

    for (case, body, content_type, metadata) in writes {
        let mut headers: Vec<(&str, &str)> = vec![("content-type", content_type)];
        if let Some((name, value)) = metadata {
            headers.push((name, value));
        }
        let written = put_object(&router, bucket, key, &headers, body).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected HTTP 200 from the overwrite, got {}",
            written.status
        );

        let read = head_object(&router, bucket, key).await;
        assert_eq!(
            read.header("content-length").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: the read must report the LAST write's size, got {:?}",
            read.header("content-length")
        );
        assert_eq!(
            read.header("content-type").as_deref(),
            Some(content_type),
            "case {case:?}: the read must report the LAST write's ContentType, got {:?}",
            read.header("content-type")
        );
        assert_eq!(
            read.header("etag").as_deref(),
            written.header("etag").as_deref(),
            "case {case:?}: the stored ETag must be the one the overwrite reported, got {:?}",
            read.header("etag")
        );
        assert_eq!(
            read.header("x-amz-meta-stage"),
            metadata.map(|(_, value)| value.to_string()),
            "case {case:?}: a write replaces the whole object, metadata included, got {:?}",
            read.header("x-amz-meta-stage")
        );
    }
}

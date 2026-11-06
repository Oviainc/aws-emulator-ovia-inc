// SPDX-License-Identifier: CC0-1.0

//! What the two Amazon S3 ADDRESS FORMS mean to the router, across the one emulator.
//!
//! RULE: "both the virtual-hosted-style requests and the path-style requests are supported"
//! (`GetObject`'s own documentation). A path-style request carries the bucket as the first
//! segment of its path; a virtual-hosted-style request carries it as the first label of its
//! host — `https://bucket-name.s3.region-code.amazonaws.com/key-name` — leaving the path to be
//! the KEY alone. The router reduces the second form to the first before any service sees the
//! request, so one object is reached by either address, and a request that names the bucket in
//! BOTH places addresses the object whose key is that path rather than the bucket.
//!
//! Every cross-service suite that stages an S3 object depends on this: the CloudFront
//! origin-access-control suite beside this one writes its origin objects PATH-STYLE for exactly
//! this reason, while the origin fetch its edge makes is the virtual-hosted request an S3 origin
//! is reached by. No router suite stated the rule, so a setup call that mixed the two forms read
//! as a write to a bucket-named key instead of as the call it meant.
//!
//! The rows span what the rule generalises over: which form the request arrives in, whether the
//! path is a bucket, a bare key or a nested one, and whether the bucket named is one the
//! emulator holds.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The code S3 refuses a write to a bucket it does not hold with (Amazon S3 "Error responses").
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// What one call answered: the status, and the body — an object's bytes or an `<Error>`.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of `<name>…</name>` in the body, or the empty string.
    fn element(&self, name: &str) -> String {
        self.body
            .split_once(&format!("<{name}>"))
            .and_then(|(_before, rest)| rest.split_once(&format!("</{name}>")))
            .map_or(String::new(), |(text, _after)| text.to_string())
    }
}

/// The emulator for this suite: S3 mounted on one engine exactly as the router mounts it, with
/// request authentication off so each call carries a credential scope (which is what names the
/// Region and the service) without a computed signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// The Region's PATH-STYLE endpoint: the host whose own name carries no bucket.
fn path_style_host() -> String {
    format!("s3.{REGION}.amazonaws.com")
}

/// The VIRTUAL-HOSTED-STYLE host of `bucket`: the bucket as the first label of the endpoint.
fn host_style_host(bucket: &str) -> String {
    format!("{bucket}.s3.{REGION}.amazonaws.com")
}

/// One call of `method` to `host` at `path`, signed for S3 in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    body: &str,
) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// Rule, first half: a PATH-STYLE request names its bucket in the path, so `PUT /{bucket}` is
/// the create and `PUT /{bucket}/{key}` the object write — and the virtual-hosted address of
/// that same object, whose path is the key alone, reads the bytes it stored.
///
/// The rows differ along what that generalises over: the form of the key (a bare name, a nested
/// prefix, one carrying a dot) and the bytes written.
#[tokio::test]
async fn a_path_style_request_names_its_bucket_in_the_path() {
    let router = emulator();
    let bucket = "address-forms-path-style";
    let created = call(&router, "PUT", &path_style_host(), &format!("/{bucket}"), "").await;
    assert_eq!(
        created.status, 200,
        "case {bucket:?}: a path-style CreateBucket must succeed, got {} with {:?}",
        created.status, created.body
    );

    // (case, key, bytes)
    let rows: [(&str, &str, &str); 3] = [
        ("a bare key", "plain.txt", "the plain object"),
        ("a nested key", "reports/2026/held.csv", "the nested object"),
        ("a key carrying a dot segment", "a.b/c.d", "the dotted object"),
    ];
    for (row, key, bytes) in rows {
        let written = call(
            &router,
            "PUT",
            &path_style_host(),
            &format!("/{bucket}/{key}"),
            bytes,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {row:?}: a path-style PutObject must succeed, got {} with {:?}",
            written.status, written.body
        );
        let read = call(
            &router,
            "GET",
            &host_style_host(bucket),
            &format!("/{key}"),
            "",
        )
        .await;
        assert_eq!(
            (read.status, read.body.as_str()),
            (200, bytes),
            "case {row:?}: the virtual-hosted address of {key:?} must read the bytes the \
             path-style write stored, got {} with {:?}",
            read.status,
            read.body
        );
    }
}

/// Rule, second half: a VIRTUAL-HOSTED-STYLE request's path is the KEY, so the bucket named in
/// the host is never read out of the path as well — which is what makes a request naming the
/// bucket in both places a write to the object of that name.
///
/// The rows differ along what that generalises over: whether the bucket in the host is one the
/// emulator holds, and whether the path repeats the bucket's name or names a key of its own.
#[tokio::test]
async fn a_virtual_hosted_style_request_reads_its_path_as_the_key() {
    let router = emulator();
    let held = "address-forms-host-style";
    let created = call(&router, "PUT", &path_style_host(), &format!("/{held}"), "").await;
    assert_eq!(
        created.status, 200,
        "case {held:?}: a path-style CreateBucket must succeed, got {} with {:?}",
        created.status, created.body
    );

    // A bucket the emulator does NOT hold: the request is a write to an object OF it, so it is
    // refused for the bucket rather than creating one.
    let absent = "address-forms-absent";
    let refused = call(
        &router,
        "PUT",
        &host_style_host(absent),
        &format!("/{absent}"),
        "",
    )
    .await;
    assert_eq!(
        refused.status, 404,
        "case {absent:?}: a virtual-hosted write to a bucket nothing holds must be refused, \
         got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.element("Code"),
        NO_SUCH_BUCKET,
        "case {absent:?}: expected {NO_SUCH_BUCKET}, got {:?}",
        refused.body
    );
    assert_eq!(
        refused.element("BucketName"),
        absent,
        "case {absent:?}: the refusal must name the bucket it is about, got {:?}",
        refused.body
    );

    // The same request against a bucket the emulator HOLDS writes the object whose key is the
    // path — here the bucket's own name — and leaves the bucket itself alone.
    let written = call(
        &router,
        "PUT",
        &host_style_host(held),
        &format!("/{held}"),
        "the bucket-named object",
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {held:?}: a virtual-hosted write must store the object, got {} with {:?}",
        written.status, written.body
    );
    let read = call(
        &router,
        "GET",
        &path_style_host(),
        &format!("/{held}/{held}"),
        "",
    )
    .await;
    assert_eq!(
        (read.status, read.body.as_str()),
        (200, "the bucket-named object"),
        "case {held:?}: the path-style address of that key must read the same bytes, got {} \
         with {:?}",
        read.status,
        read.body
    );
}

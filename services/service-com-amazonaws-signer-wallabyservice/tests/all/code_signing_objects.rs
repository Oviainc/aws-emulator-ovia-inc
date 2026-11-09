// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 objects a code-signing job needs, for the suites that start one.
//!
//! `StartSigningJob` signs an object in Amazon S3 and writes the signed artifact
//! to another bucket ("You must create an Amazon S3 source bucket … Your S3
//! source bucket must be version enabled … You must create an S3 destination
//! bucket"), so a suite that starts a job mounts Amazon S3 beside signer on the
//! one shared engine and seeds those objects over S3's own front door. Everything
//! a job reads and writes then goes through the same router a client's calls do.
//!
//! Shared here rather than per suite so every suite seeds the same way and the
//! wire form of each S3 call is written once.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header in the `s3` scope for `region`: the credential
/// scope is how the router picks the service a request is for, so a request to
/// the object-storage front door carries one naming `s3`. Nothing is verified —
/// every suite turns request authentication off.
fn signed_for_s3(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/s3/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Mount Amazon S3 beside signer on `router`, over the engine both share — what
/// the assembled router does.
pub(crate) fn mount_object_storage(
    router: &mut MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) {
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(engine),
    ));
}

/// One request to the object-storage front door, path-style (the emulator serves
/// one endpoint per region, so the bucket travels in the path).
async fn s3(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, ::http::HeaderMap, Vec<u8>) {
    let host = format!("s3.{region}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", signed_for_s3(region))
        .header("content-type", "application/octet-stream")
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed S3 request");
    let (parts, answer) = router.dispatch(request).await.into_parts();
    (parts.status.as_u16(), parts.headers, answer.to_vec())
}

/// `PUT /<bucket>` — `CreateBucket` in `region`, idempotent for a suite that
/// seeds the same bucket more than once: Amazon S3 answers `409`
/// `BucketAlreadyOwnedByYou` for a bucket this account already holds, which
/// leaves the bucket there either way.
///
/// A region-specific endpoint other than `us-east-1`'s needs the region named in
/// a `CreateBucketConfiguration` ("The unspecified location constraint is
/// incompatible for the region specific endpoint this request was sent to"), so
/// the body carries one everywhere but there.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, region: &str, bucket: &str) {
    let configuration = if region == "us-east-1" {
        String::new()
    } else {
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{region}</LocationConstraint></CreateBucketConfiguration>"
        )
    };
    let (status, _, body) = s3(
        router,
        region,
        "PUT",
        &format!("/{bucket}"),
        configuration.as_bytes(),
    )
    .await;
    assert!(
        status == 200 || status == 409,
        "bucket {bucket:?} in {region}: expected CreateBucket 200 (or 409 for a bucket already \
         held), got {status} {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>?versioning` — `PutBucketVersioning` with `Status` `Enabled`,
/// which a code-signing source bucket has to have.
pub(crate) async fn enable_versioning(router: &MultiServiceRouter, region: &str, bucket: &str) {
    let (status, _, body) = s3(
        router,
        region,
        "PUT",
        &format!("/{bucket}?versioning"),
        b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    assert_eq!(
        status, 200,
        "bucket {bucket:?} in {region}: expected PutBucketVersioning 200, got {status} {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`, answering the version id the write
/// reported (the empty string for a bucket that is not version enabled).
pub(crate) async fn put_object(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    key: &str,
    body: &[u8],
) -> String {
    let (status, headers, answer) = s3(router, region, "PUT", &format!("/{bucket}/{key}"), body)
        .await;
    assert_eq!(
        status, 200,
        "object s3://{bucket}/{key}: expected PutObject 200, got {status} {:?}",
        String::from_utf8_lossy(&answer)
    );
    headers
        .get("x-amz-version-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// `HEAD /<bucket>/<key>` — `HeadObject`, answering its HTTP status: the
/// independent read that says whether an object a job reported actually exists.
pub(crate) async fn head_object(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    key: &str,
) -> u16 {
    let (status, _, _) = s3(router, region, "HEAD", &format!("/{bucket}/{key}"), b"").await;
    status
}

/// `GET /<bucket>?list-type=2&prefix=<prefix>` — `ListObjectsV2`, answering the
/// keys it reported: what says whether anything was left under a prefix.
pub(crate) async fn keys_under(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    prefix: &str,
) -> Vec<String> {
    let (status, _, body) = s3(
        router,
        region,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={prefix}"),
        b"",
    )
    .await;
    assert_eq!(
        status, 200,
        "bucket {bucket:?}: expected ListObjectsV2 200, got {status} {:?}",
        String::from_utf8_lossy(&body)
    );
    // The listing is XML; every object's key arrives in one `<Key>` element.
    let document = String::from_utf8_lossy(&body).to_string();
    document
        .split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split("</Key>").next().map(str::to_string))
        .collect()
}

/// A zip archive carrying `entries` — a valid code-signing source object.
pub(crate) fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;

    let mut writer = ::zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer
            .start_file(*name, ::zip::write::SimpleFileOptions::default())
            .expect("the archive entry starts");
        writer.write_all(body).expect("the archive entry is written");
    }
    writer
        .finish()
        .expect("the archive is finished")
        .into_inner()
}

/// The smallest valid source archive a suite that does not care about the
/// archive's contents needs.
pub(crate) fn valid_zip() -> Vec<u8> {
    zip_of(&[("index.js", b"exports.handler = async () => 1;")])
}

/// A version-enabled source bucket holding `key` as a valid archive, plus the
/// destination bucket — answering the version id of the source object.
pub(crate) async fn seed_signing_objects(
    router: &MultiServiceRouter,
    region: &str,
    source_bucket: &str,
    source_key: &str,
    destination_bucket: &str,
) -> String {
    create_bucket(router, region, source_bucket).await;
    enable_versioning(router, region, source_bucket).await;
    create_bucket(router, region, destination_bucket).await;
    put_object(router, region, source_bucket, source_key, &valid_zip()).await
}

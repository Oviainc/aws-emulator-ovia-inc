// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a deployment package named by an AMAZON S3 LOCATION: what a write
//! that names one is served or refused, and which bytes the package ends up being.
//!
//! The RULE asserted here, over a table of inputs for each of the three writes that store
//! such a package: **Lambda READS the package out of the Amazon S3 object the write names
//! ("For a .zip file archive, the code property specifies the location of the .zip file",
//! `CreateFunction`; "an Amazon S3 location", `LayerVersionContentInput`), so the object's
//! own bytes are the package — `CodeSize` and `CodeSha256` are measured from them — and a
//! write whose object Amazon S3 answers, about a bucket it holds, that it will not hand over
//! is refused `InvalidParameterValueException` (HTTP 400) reporting the code and the message
//! Amazon S3 answered, with nothing stored.** A bucket this emulator's Amazon S3 holds no
//! record of is evidence about that store rather than about the object — a bucket name lives
//! in a namespace that spans every account, and an artifact bucket is routinely provisioned
//! out of band — so it refuses nothing and holds no bytes.
//!
//! The rows differ along what the rule generalises over: whether the object is there, WHICH
//! part of the location is absent (the key, the bucket), the shape and the size of the key
//! and of the object, and which of the three writes names the location.
//!
//! Amazon S3 is mounted BESIDE Lambda on one engine, and every object read here is written
//! through S3's own `CreateBucket`/`PutObject` — so the digest asserted is the digest of the
//! bytes that bucket holds rather than one this test handed Lambda. The Lambda-side harness
//! (the `Answer` wrapper, `json_call` and `package_bytes`) is the one `function_lifecycle.rs`
//! publishes.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, Answer, REGION, ROLE,
};

/// The error code every refusal in this suite carries, and its status.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";
const REFUSAL_STATUS: u16 = 400;

/// The bucket this suite writes its packages to.
const BUCKET: &str = "deployment-packages";

/// A bucket no `CreateBucket` is made for, so this emulator's Amazon S3 holds no record of
/// it at all.
const UNHELD_BUCKET: &str = "packages-of-another-account";

/// A SigV4 credential scope naming Amazon S3 in the SAME Region the Lambda harness signs
/// for. The signature is never checked: the router has request authentication off.
fn s3_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Lambda and Amazon S3 on one engine, the shape `core/router/src/lib.rs` mounts them in:
/// the object a package names is read out of S3's own store, so it is written through S3's
/// own surface.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(engine),
    ));
    router
}

/// One path-style Amazon S3 request that must be served.
async fn s3_ok(router: &MultiServiceRouter, case: &str, method: &str, uri: &str, body: &[u8]) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host.clone())
        .header("authorization", s3_authorization())
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    assert!(
        (200..300).contains(&parts.status.as_u16()),
        "case {case:?}: S3 {method} {uri} must succeed, got {} {}",
        parts.status.as_u16(),
        String::from_utf8_lossy(&answered)
    );
}

/// Create [`BUCKET`] through Amazon S3's own `CreateBucket`, so the store holds the bucket
/// a row's location names whether or not that row writes an object into it.
async fn create_bucket(router: &MultiServiceRouter, case: &str) {
    s3_ok(router, case, "PUT", &format!("/{BUCKET}"), &[]).await;
}

/// Put `bytes` under `key` in [`BUCKET`], through Amazon S3's own `PutObject`.
async fn put_object(router: &MultiServiceRouter, case: &str, key: &str, bytes: &[u8]) {
    s3_ok(router, case, "PUT", &format!("/{BUCKET}/{key}"), bytes).await;
}

/// The base64 SHA-256 of `bytes`, computed here so the test never reads the digest it is
/// checking out of the answer it is checking.
fn digest_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// The sentence a write is refused with for an object Amazon S3 answered `code`/`message`
/// about, as AWS Lambda words it.
fn refusal(code: &str, message: &str) -> String {
    format!("Error occurred while GetObject. S3 Error Code: {code}. S3 Error Message: {message}")
}

/// The refusal for a key the bucket holds nothing under: Amazon S3's `NoSuchKey` and the
/// message it answers it with.
fn no_such_key_refusal() -> String {
    refusal("NoSuchKey", "The specified key does not exist.")
}

/// The `message` of a wire error, under either casing the JSON envelopes use.
fn message(answer: &Answer) -> Option<String> {
    answer.string("message").or_else(|| answer.string("Message"))
}

/// A member of a JSON document, by pointer.
fn member(document: &Value, pointer: &str) -> Option<Value> {
    document.pointer(pointer).cloned()
}

/// `GetFunction` on a function name.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `UpdateFunctionCode` on a function name.
async fn update_function_code(
    router: &MultiServiceRouter,
    name: &str,
    body: &Value,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/code");
    json_call(router, "PUT", &uri, body).await
}

/// `PublishLayerVersion` on a layer name.
async fn publish_layer_version(
    router: &MultiServiceRouter,
    name: &str,
    body: &Value,
) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `ListLayerVersions` on a layer name.
async fn list_layer_versions(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetLayerVersion` on a layer name and a version number.
async fn get_layer_version(router: &MultiServiceRouter, name: &str, version: i64) -> Answer {
    let uri = format!("/2018-10-31/layers/{name}/versions/{version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// One row of the create table: a function whose `Code` names an Amazon S3 location.
struct CreateRow {
    case: &'static str,
    function: &'static str,
    /// The key the location names, and the bytes to write under it first — `None` writes
    /// nothing, so the location names an object the bucket does not hold.
    key: &'static str,
    object: Option<usize>,
    /// The bucket the location names, when it is not the one this suite writes to.
    bucket: Option<&'static str>,
}

fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            case: "an object the bucket holds",
            function: "from-held-object",
            key: "worker.zip",
            object: Some(96),
            bucket: None,
        },
        CreateRow {
            case: "an object of another size, under a nested key",
            function: "from-nested-key",
            key: "builds/2026/01/worker-v2.zip",
            object: Some(4_096),
            bucket: None,
        },
        CreateRow {
            case: "an empty object, which is still an object the bucket holds",
            function: "from-empty-object",
            key: "empty.zip",
            object: Some(0),
            bucket: None,
        },
        CreateRow {
            case: "a key the bucket holds nothing under",
            function: "from-absent-key",
            key: "never-uploaded.zip",
            object: None,
            bucket: None,
        },
        CreateRow {
            case: "a key of another shape the bucket holds nothing under",
            function: "from-absent-nested-key",
            key: "builds/2026/01/never-uploaded.zip",
            object: None,
            bucket: None,
        },
        CreateRow {
            case: "a bucket this store holds no record of, which shows nothing",
            function: "from-unheld-bucket",
            key: "worker.zip",
            object: None,
            bucket: Some(UNHELD_BUCKET),
        },
    ]
}

/// RULE: `CreateFunction` reads the package out of the Amazon S3 object the `Code` names —
/// so the function's `CodeSize` and `CodeSha256` are the object's own — and is refused
/// `InvalidParameterValueException` with what Amazon S3 answered when that bucket holds no
/// such object, with no function created.
#[tokio::test]
async fn a_create_reads_the_package_out_of_the_object_its_code_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    create_bucket(&router, "the table's bucket").await;

    for row in create_rows() {
        let bytes = row.object.map(package_bytes);
        if let Some(bytes) = &bytes {
            put_object(&router, row.case, row.key, bytes).await;
        }
        let created = create_function(
            &router,
            &json!({
                "FunctionName": row.function,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {
                    "S3Bucket": row.bucket.unwrap_or(BUCKET),
                    "S3Key": row.key,
                },
            }),
        )
        .await;

        // A location the bucket holds no object under is refused, and nothing is created.
        if row.object.is_none() && row.bucket.is_none() {
            assert_eq!(
                created.status, REFUSAL_STATUS,
                "case {:?}: expected {REFUSAL_STATUS}, got {} {}",
                row.case,
                created.status,
                String::from_utf8_lossy(&created.body)
            );
            assert!(
                created.error_code().contains(INVALID_PARAMETER),
                "case {:?}: expected {INVALID_PARAMETER}, got {:?}",
                row.case,
                created.error_code()
            );
            let worded = message(&created);
            assert_eq!(
                worded.as_deref(),
                Some(no_such_key_refusal().as_str()),
                "case {:?}: expected {:?}, got {worded:?}",
                row.case,
                no_such_key_refusal()
            );
            let read = get_function(&router, row.function).await;
            assert_eq!(
                read.status, 404,
                "case {:?}: expected no function to have been created, got {} {}",
                row.case,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            continue;
        }

        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {}",
            row.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        // The size and the digest the function reports are the OBJECT's, computed here from
        // the bytes the bucket holds; a location nothing could be shown about holds no bytes,
        // so neither is reported for it.
        let expected_size = bytes
            .as_ref()
            .map_or(0, |bytes| i64::try_from(bytes.len()).unwrap_or(i64::MAX));
        let expected_digest = bytes.as_ref().map(|bytes| digest_of(bytes));
        for (surface, document) in [
            ("the create's own answer", created.json()),
            (
                "GetFunction",
                member(&get_function(&router, row.function).await.json(), "/Configuration")
                    .unwrap_or(Value::Null),
            ),
        ] {
            let size = member(&document, "/CodeSize").and_then(|size| size.as_i64());
            assert_eq!(
                size,
                Some(expected_size),
                "case {:?} on {surface}: expected CodeSize {expected_size}, got {size:?}",
                row.case
            );
            let reported = member(&document, "/CodeSha256")
                .and_then(|digest| digest.as_str().map(str::to_string));
            assert_eq!(
                reported, expected_digest,
                "case {:?} on {surface}: expected CodeSha256 {expected_digest:?}, got \
                 {reported:?}",
                row.case
            );
        }
    }
}

/// One row of the code-update table.
struct UpdateRow {
    case: &'static str,
    key: &'static str,
    /// The bytes to write under that key first, or `None` for a key the bucket holds
    /// nothing under.
    object: Option<usize>,
}

/// RULE: `UpdateFunctionCode` follows a new Amazon S3 location the same way — the object's
/// bytes become the function's package, and an object the bucket does not hold is refused
/// with the function's code left exactly as it was.
#[tokio::test]
async fn a_code_update_follows_the_new_location_the_same_way() {
    let rows = [
        UpdateRow {
            case: "an object the bucket holds",
            key: "updated.zip",
            object: Some(512),
        },
        UpdateRow {
            case: "an object of another size",
            key: "updated-again.zip",
            object: Some(1_024),
        },
        UpdateRow {
            case: "a key the bucket holds nothing under",
            key: "never-updated.zip",
            object: None,
        },
    ];

    for row in rows {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator(&engine);
        create_bucket(&router, row.case).await;

        // The function starts from an inline package, so the digest it carries before the
        // update is one this test knows and the update's effect is visible.
        let original = package_bytes(64);
        let created = create_function(
            &router,
            &json!({
                "FunctionName": "updated-from-s3",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {
                    "ZipFile": base64::engine::general_purpose::STANDARD.encode(&original),
                },
            }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: the function under test must be created, got {} {}",
            row.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The create answers the function `Pending`, and Lambda refuses a modification of a
        // function in that window; the transition is owed by the CLOCK, so the due instant is
        // moved into the past rather than waited out.
        make_activation_due(&engine);

        let bytes = row.object.map(package_bytes);
        if let Some(bytes) = &bytes {
            put_object(&router, row.case, row.key, bytes).await;
        }
        let updated = update_function_code(
            &router,
            "updated-from-s3",
            &json!({ "S3Bucket": BUCKET, "S3Key": row.key }),
        )
        .await;

        let expected_digest = match &bytes {
            Some(bytes) => {
                assert_eq!(
                    updated.status, 200,
                    "case {:?}: expected 200, got {} {}",
                    row.case,
                    updated.status,
                    String::from_utf8_lossy(&updated.body)
                );
                digest_of(bytes)
            }
            None => {
                assert_eq!(
                    updated.status, REFUSAL_STATUS,
                    "case {:?}: expected {REFUSAL_STATUS}, got {} {}",
                    row.case,
                    updated.status,
                    String::from_utf8_lossy(&updated.body)
                );
                let worded = message(&updated);
                assert_eq!(
                    worded.as_deref(),
                    Some(no_such_key_refusal().as_str()),
                    "case {:?}: expected {:?}, got {worded:?}",
                    row.case,
                    no_such_key_refusal()
                );
                // Nothing changed: the function still holds the package it was created with.
                digest_of(&original)
            }
        };

        let read = get_function(&router, "updated-from-s3").await;
        let reported = member(&read.json(), "/Configuration/CodeSha256")
            .and_then(|digest| digest.as_str().map(str::to_string));
        assert_eq!(
            reported.as_deref(),
            Some(expected_digest.as_str()),
            "case {:?}: expected CodeSha256 {expected_digest:?}, got {reported:?}",
            row.case
        );
    }
}

/// RULE: `PublishLayerVersion` follows the Amazon S3 location its `Content` names the same
/// way — the object's bytes are the archive it reports a size and a digest for, and an object
/// the bucket does not hold is refused with no version published.
#[tokio::test]
async fn a_layer_publish_follows_the_location_its_content_names() {
    // (case, the layer, the key, the bytes to write under it first)
    let rows: [(&str, &str, &str, Option<usize>); 3] = [
        ("an object the bucket holds", "held-archive", "layer.zip", Some(256)),
        (
            "an object of another size, under a nested key",
            "nested-archive",
            "layers/deps/helpers.zip",
            Some(2_048),
        ),
        (
            "a key the bucket holds nothing under",
            "absent-archive",
            "never-published.zip",
            None,
        ),
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    create_bucket(&router, "the table's bucket").await;

    for (case, layer, key, size) in rows {
        let bytes = size.map(package_bytes);
        if let Some(bytes) = &bytes {
            put_object(&router, case, key, bytes).await;
        }
        let published = publish_layer_version(
            &router,
            layer,
            &json!({ "Content": { "S3Bucket": BUCKET, "S3Key": key } }),
        )
        .await;

        let Some(bytes) = &bytes else {
            assert_eq!(
                published.status, REFUSAL_STATUS,
                "case {case:?}: expected {REFUSAL_STATUS}, got {} {}",
                published.status,
                String::from_utf8_lossy(&published.body)
            );
            assert!(
                published.error_code().contains(INVALID_PARAMETER),
                "case {case:?}: expected {INVALID_PARAMETER}, got {:?}",
                published.error_code()
            );
            let worded = message(&published);
            assert_eq!(
                worded.as_deref(),
                Some(no_such_key_refusal().as_str()),
                "case {case:?}: expected {:?}, got {worded:?}",
                no_such_key_refusal()
            );
            // Nothing was published: the layer holds no version at all.
            let listed = list_layer_versions(&router, layer).await;
            let versions = member(&listed.json(), "/LayerVersions")
                .and_then(|versions| versions.as_array().map(Vec::len))
                .unwrap_or_default();
            assert_eq!(
                versions, 0,
                "case {case:?}: expected no version to have been published, got {}",
                String::from_utf8_lossy(&listed.body)
            );
            continue;
        };

        assert_eq!(
            published.status, 201,
            "case {case:?}: expected 201, got {} {}",
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        let expected_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let expected_digest = digest_of(bytes);
        let read = get_layer_version(&router, layer, 1).await;
        for (surface, document) in [
            ("the publish's own answer", published.json()),
            ("GetLayerVersion", read.json()),
        ] {
            let reported_size = member(&document, "/Content/CodeSize").and_then(|size| size.as_i64());
            assert_eq!(
                reported_size,
                Some(expected_size),
                "case {case:?} on {surface}: expected CodeSize {expected_size}, got \
                 {reported_size:?}"
            );
            let reported = member(&document, "/Content/CodeSha256")
                .and_then(|digest| digest.as_str().map(str::to_string));
            assert_eq!(
                reported.as_deref(),
                Some(expected_digest.as_str()),
                "case {case:?} on {surface}: expected CodeSha256 {expected_digest:?}, got \
                 {reported:?}"
            );
        }
    }
}

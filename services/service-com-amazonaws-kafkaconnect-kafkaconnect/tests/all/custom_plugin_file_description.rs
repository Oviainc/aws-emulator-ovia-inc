// SPDX-License-Identifier: CC0-1.0

//! What a custom plugin REVISION reports about the artifact it was built from, end to end
//! over HTTP, with the Amazon S3 service mounted beside kafkaconnect on one engine — the
//! way the assembled emulator runs them, and the way the service takes a plugin's file on
//! when the plugin is created.
//!
//! The rule under test: a plugin revision describes the artifact the service took on, so
//! `latestRevision.fileDescription.fileMd5` is the lowercase hex MD5 digest of that file's
//! bytes and `latestRevision.fileDescription.fileSize` is how many bytes there are
//! (`CustomPluginFileDescription$fileMd5`, "The hex-encoded MD5 checksum of the custom
//! plugin file. You can use it to validate the file."; `$fileSize`, "The size in bytes of
//! the custom plugin file."). Both are properties of the BYTES, so they hold for any
//! artifact, they are the same whichever read reports the revision, and they do not move
//! once the revision exists.
//!
//! The table's rows differ along what the rule generalises over: the artifact's content
//! (text, binary bytes including a NUL, a ZIP header), its size (nothing, one byte, a short
//! payload, past a 64 KiB buffering boundary), where it is staged (a bare key, a nested
//! prefix) and both `contentType` values. Two rows also name a PUBLISHED digest for their
//! bytes (RFC 1321's own test suite), which anchors the algorithm and the encoding rather
//! than only re-deriving them.
//!
//! The tests after it pin the rest of the rule: the digest describes the bytes of the
//! version the plugin PINNED (not whatever the key holds later), the facts hold on every
//! read — `DescribeCustomPlugin` and `ListCustomPlugins` alike, before and after the plugin
//! settles — and with no object store to ask, where nothing about the file can be computed,
//! `fileDescription` is reported not at all rather than invented.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN carries. No
/// authenticator is registered, so the signature is never checked; the scope is what routes
/// the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The same scope naming `s3`, which is what routes a request to the object store.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in both scopes above.
const REGION: &str = "eu-west-1";

/// The bucket the artifacts below are staged in.
const BUCKET: &str = "kafkaconnect-artifact-digests";

/// A second bucket, with versioning enabled, so a row may pin an `objectVersion` whose
/// bytes differ from the ones the key currently holds.
const VERSIONED_BUCKET: &str = "kafkaconnect-artifact-digests-versioned";

/// The header Amazon S3 reports a stored version's id in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A response header's value, or `""` when the response carries none.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or_else(|error| {
                panic!(
                    "a JSON response body: {error}; got {:?}",
                    String::from_utf8_lossy(&self.body)
                )
            })
        }
    }
}

/// One emulator: kafkaconnect and Amazon S3 on a single shared engine behind the
/// production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One kafkaconnect request (`restJson1`, the routes in `OPERATIONS.md`).
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> Answer {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("host", format!("kafkaconnect.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    answer(emulator, request).await
}

/// One path-style request to the emulator's S3 front door: the bucket and the key live in
/// the URI and the body is the object's own bytes.
async fn s3(emulator: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    answer(emulator, request).await
}

/// Dispatch one request and read back its status, headers and body.
async fn answer(emulator: &MultiServiceRouter, request: http::Request<bytes::Bytes>) -> Answer {
    let (parts, body) = emulator.dispatch(request).await.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `PUT /<bucket>` — `CreateBucket`, with the `LocationConstraint` this suite's Region
/// needs.
async fn create_bucket(emulator: &MultiServiceRouter, bucket: &str) {
    let payload = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let answered = s3(emulator, "PUT", &format!("/{bucket}"), payload.as_bytes()).await;
    assert_eq!(
        answered.status,
        200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
}

/// `PUT /<bucket>?versioning` with `Status=Enabled`, so every write into it mints a version
/// id a request may pin.
async fn enable_versioning(emulator: &MultiServiceRouter, bucket: &str) {
    let payload = "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                   <Status>Enabled</Status></VersioningConfiguration>";
    let answered = s3(
        emulator,
        "PUT",
        &format!("/{bucket}?versioning"),
        payload.as_bytes(),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "the bucket {bucket:?} must take versioning, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`. Answers the version id S3 minted for the write, or
/// `""` when the bucket is not versioned.
async fn stage(emulator: &MultiServiceRouter, bucket: &str, key: &str, body: &[u8]) -> String {
    let answered = s3(emulator, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        answered.status,
        200,
        "the artifact {bucket}/{key} must be stored, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    answered.header(VERSION_ID_HEADER)
}

/// The `CreateCustomPlugin` body naming one artifact.
fn plugin_request(
    name: &str,
    content_type: &str,
    bucket: &str,
    file_key: &str,
    object_version: Option<&str>,
) -> Value {
    let mut location = json!({
        "s3Location": { "bucketArn": format!("arn:aws:s3:::{bucket}"), "fileKey": file_key }
    });
    if let Some(object_version) = object_version {
        location["s3Location"]["objectVersion"] = json!(object_version);
    }
    json!({ "name": name, "contentType": content_type, "location": location })
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The hex-encoded MD5 digest of `bytes`, computed in the test from the bytes it staged —
/// the value a client validating the artifact against its own copy would compute.
fn md5_hex(bytes: &[u8]) -> String {
    use md5::Digest as _;

    hex::encode(md5::Md5::digest(bytes))
}

/// `POST /v1/custom-plugins`, answering the ARN the create minted.
async fn create_plugin(emulator: &MultiServiceRouter, case: &str, body: Value) -> String {
    let answered = call(emulator, "POST", "/v1/custom-plugins", body).await;
    let created = answered.json();
    assert_eq!(
        answered.status, 200,
        "case {case:?}: the create must be accepted, got {} {created}",
        answered.status
    );
    let arn = text(&created, "customPluginArn");
    assert!(
        !arn.is_empty(),
        "case {case:?}: a created plugin carries its ARN, got {created}"
    );
    arn
}

/// `GET /v1/custom-plugins/{customPluginArn}` — the described plugin.
async fn describe_plugin(emulator: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let read = call(
        emulator,
        "GET",
        &format!("/v1/custom-plugins/{}", label(arn)),
        Value::Null,
    )
    .await;
    let described = read.json();
    assert_eq!(
        read.status, 200,
        "case {case:?}: DescribeCustomPlugin must answer for the stored plugin, got {} \
         {described}",
        read.status
    );
    described
}

/// Every `ListCustomPlugins` summary, keyed by the plugin's ARN.
async fn listed_by_arn(emulator: &MultiServiceRouter) -> std::collections::HashMap<String, Value> {
    let answered = call(emulator, "GET", "/v1/custom-plugins", Value::Null).await;
    let listed = answered.json();
    assert_eq!(
        answered.status, 200,
        "ListCustomPlugins must succeed, got {} {listed}",
        answered.status
    );
    listed["customPlugins"]
        .as_array()
        .unwrap_or_else(|| panic!("a customPlugins array in {listed}"))
        .iter()
        .map(|summary| (text(summary, "customPluginArn"), summary.clone()))
        .collect()
}

/// Assert that the revision in `revision` describes exactly `bytes`.
fn assert_describes(case: &str, where_from: &str, revision: &Value, bytes: &[u8]) {
    let file = &revision["fileDescription"];
    assert!(
        file.is_object(),
        "case {case:?}: {where_from} must report latestRevision.fileDescription, got {revision}"
    );
    assert_eq!(
        text(file, "fileMd5"),
        md5_hex(bytes),
        "case {case:?}: {where_from} must report the hex MD5 of the artifact's {} bytes, \
         got {file}",
        bytes.len()
    );
    assert_eq!(
        file["fileSize"].as_i64(),
        Some(bytes.len() as i64),
        "case {case:?}: {where_from} must report the artifact's size in bytes, got {file}"
    );
}

/// What a row stages as the plugin's artifact.
enum Artifact {
    /// Exactly these bytes.
    Literal(&'static [u8]),
    /// `count` copies of one byte — the rows that vary SIZE rather than content.
    Repeated(u8, usize),
}

impl Artifact {
    fn bytes(&self) -> Vec<u8> {
        match *self {
            Self::Literal(bytes) => bytes.to_vec(),
            Self::Repeated(byte, count) => vec![byte; count],
        }
    }
}

/// One row: an artifact, where it is staged, and the plugin created from it.
struct ArtifactCase {
    /// What the row varies, named in every assertion.
    case: &'static str,
    name: &'static str,
    /// `CustomPluginContentType`: JAR | ZIP (`smithy_query(query="shape",
    /// shape="com.amazonaws.kafkaconnect#CustomPluginContentType")`).
    content_type: &'static str,
    file_key: &'static str,
    artifact: Artifact,
    /// A digest published for these bytes, where one is: RFC 1321's own test suite
    /// ("MD5 test suite", section A.5). Asserted on TOP of the derived value, so the
    /// algorithm and the encoding are pinned by something outside this emulator.
    published_md5: Option<&'static str>,
}

const ARTIFACTS: &[ArtifactCase] = &[
    ArtifactCase {
        case: "an empty artifact (RFC 1321 test suite: the empty string)",
        name: "empty-artifact",
        content_type: "ZIP",
        file_key: "empty.zip",
        artifact: Artifact::Literal(b""),
        published_md5: Some("d41d8cd98f00b204e9800998ecf8427e"),
    },
    ArtifactCase {
        case: "the RFC 1321 test suite's \"abc\"",
        name: "rfc-1321-abc",
        content_type: "JAR",
        file_key: "vectors/abc.jar",
        artifact: Artifact::Literal(b"abc"),
        published_md5: Some("900150983cd24fb0d6963f7d28e17f72"),
    },
    ArtifactCase {
        case: "a single NUL byte",
        name: "one-nul-byte",
        content_type: "JAR",
        file_key: "one-byte.jar",
        artifact: Artifact::Literal(b"\x00"),
        published_md5: None,
    },
    ArtifactCase {
        case: "a ZIP artifact of binary bytes, under a nested prefix",
        name: "binary-zip",
        content_type: "ZIP",
        file_key: "connectors/debezium/plugin-1.0.0.zip",
        artifact: Artifact::Literal(b"PK\x03\x04\x00\x00plugin\x00\xff\xfe\x7f payload"),
        published_md5: None,
    },
    ArtifactCase {
        case: "a text artifact of a few hundred bytes",
        name: "short-text",
        content_type: "JAR",
        file_key: "text/plugin.jar",
        artifact: Artifact::Repeated(b'J', 300),
        published_md5: None,
    },
    ArtifactCase {
        case: "an artifact one byte past a 64 KiB boundary",
        name: "past-64-kib",
        content_type: "ZIP",
        file_key: "large/plugin.zip",
        artifact: Artifact::Repeated(0xAB, 64 * 1024 + 1),
        published_md5: None,
    },
];

#[tokio::test]
async fn a_revision_reports_the_md5_and_size_of_the_artifact_it_was_built_from() {
    let emulator = emulator();
    create_bucket(&emulator, BUCKET).await;

    let mut created: Vec<(&ArtifactCase, Vec<u8>, String)> = Vec::new();
    for case in ARTIFACTS {
        let bytes = case.artifact.bytes();
        stage(&emulator, BUCKET, case.file_key, &bytes).await;
        if let Some(published) = case.published_md5 {
            assert_eq!(
                md5_hex(&bytes),
                published,
                "case {:?}: the digest this suite compares against must be the published one",
                case.case
            );
        }
        let arn = create_plugin(
            &emulator,
            case.case,
            plugin_request(case.name, case.content_type, BUCKET, case.file_key, None),
        )
        .await;

        let described = describe_plugin(&emulator, case.case, &arn).await;
        assert_describes(
            case.case,
            "DescribeCustomPlugin",
            &described["latestRevision"],
            &bytes,
        );
        if let Some(published) = case.published_md5 {
            assert_eq!(
                text(&described["latestRevision"]["fileDescription"], "fileMd5"),
                published,
                "case {:?}: the reported digest must be the published MD5 of these bytes, \
                 got {described}",
                case.case
            );
        }
        created.push((case, bytes, arn));
    }

    // The same facts reach the summaries the list operation answers with: both reads report
    // one revision, so neither can describe a different file.
    let listed = listed_by_arn(&emulator).await;
    for (case, bytes, arn) in &created {
        let summary = listed
            .get(arn)
            .unwrap_or_else(|| panic!("case {:?}: ListCustomPlugins must report {arn}", case.case));
        assert_describes(
            case.case,
            "ListCustomPlugins",
            &summary["latestRevision"],
            bytes,
        );
    }

    // The facts do not move: the revision describes the bytes it was built from, so the
    // plugin settling out of CREATING changes neither of them.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    for (case, bytes, arn) in &created {
        let described = describe_plugin(&emulator, case.case, arn).await;
        assert_eq!(
            text(&described, "customPluginState"),
            "ACTIVE",
            "case {:?}: the plugin must have settled by now, got {described}",
            case.case
        );
        assert_describes(
            case.case,
            "DescribeCustomPlugin after the plugin settled",
            &described["latestRevision"],
            bytes,
        );
    }
}

/// The key every row of this test writes its versions to.
const VERSIONED_KEY: &str = "versioned/plugin.zip";

#[tokio::test]
async fn a_revision_describes_the_bytes_of_the_version_the_plugin_pinned() {
    let emulator = emulator();
    create_bucket(&emulator, VERSIONED_BUCKET).await;
    enable_versioning(&emulator, VERSIONED_BUCKET).await;

    // Three writes to ONE key, each with its own bytes and its own length, so neither the
    // digest nor the size of one version can be mistaken for another's.
    let versions: Vec<(Vec<u8>, String)> = {
        let bodies: Vec<Vec<u8>> = vec![
            b"PK\x03\x04 version one".to_vec(),
            vec![b'2'; 4096],
            b"PK\x03\x04 the third and current version of the artifact".to_vec(),
        ];
        let mut stored = Vec::new();
        for body in bodies {
            let version_id = stage(&emulator, VERSIONED_BUCKET, VERSIONED_KEY, &body).await;
            assert!(
                !version_id.is_empty(),
                "a write into a versioned bucket must report a version id in {VERSION_ID_HEADER}"
            );
            stored.push((body, version_id));
        }
        stored
    };

    // Each stored version, pinned by its own id: the revision describes THAT version's
    // bytes, not the ones the key currently holds.
    for (index, (bytes, version_id)) in versions.iter().enumerate() {
        let case = format!("objectVersion pinning stored version {index}");
        let arn = create_plugin(
            &emulator,
            &case,
            plugin_request(
                &format!("pinned-{index}"),
                "ZIP",
                VERSIONED_BUCKET,
                VERSIONED_KEY,
                Some(version_id),
            ),
        )
        .await;
        let described = describe_plugin(&emulator, &case, &arn).await;
        assert_describes(
            &case,
            "DescribeCustomPlugin",
            &described["latestRevision"],
            bytes,
        );
    }

    // With no version pinned the read is of the key's current version, which is the last
    // write.
    let (current, _) = versions.last().expect("three versions were staged");
    let case = "no objectVersion pinned: the key's current version";
    let arn = create_plugin(
        &emulator,
        case,
        plugin_request(
            "unpinned-current",
            "ZIP",
            VERSIONED_BUCKET,
            VERSIONED_KEY,
            None,
        ),
    )
    .await;
    let described = describe_plugin(&emulator, case, &arn).await;
    assert_describes(
        case,
        "DescribeCustomPlugin",
        &described["latestRevision"],
        current,
    );

    // A later write to the same key does not re-describe an existing revision: a revision
    // is the bytes it was created from.
    let replacement = b"PK\x03\x04 a fourth write, long after both plugins were created";
    stage(&emulator, VERSIONED_BUCKET, VERSIONED_KEY, replacement).await;
    let described = describe_plugin(&emulator, case, &arn).await;
    assert_describes(
        case,
        "DescribeCustomPlugin after the key was overwritten",
        &described["latestRevision"],
        current,
    );
}

#[tokio::test]
async fn with_no_object_store_to_ask_a_revision_reports_no_file_description() {
    // Both members are computed from the artifact's bytes. With no S3 mounted there are no
    // bytes to compute them from, so the revision reports no `fileDescription` at all
    // rather than a digest or a size this emulator never saw.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut emulator = MultiServiceRouter::new();
    emulator.register_service(EmulatedService::new(engine));

    let case = "no object store mounted";
    let arn = create_plugin(
        &emulator,
        case,
        plugin_request("no-store", "ZIP", BUCKET, "plugin.zip", None),
    )
    .await;
    let described = describe_plugin(&emulator, case, &arn).await;
    let revision = &described["latestRevision"];
    assert!(
        revision["fileDescription"].is_null(),
        "case {case:?}: with nothing to read, the revision must report no fileDescription, \
         got {revision}"
    );
    // The rest of the revision is still reported: only the facts about the FILE are
    // unknown.
    assert_eq!(
        revision["revision"].as_i64(),
        Some(1),
        "case {case:?}: the revision itself is still reported, got {revision}"
    );
}

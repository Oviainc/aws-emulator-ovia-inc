// SPDX-License-Identifier: CC0-1.0

//! `CreateCustomPlugin` resolving the custom plugin FILE before it stores a plugin, end to
//! end over HTTP, with the Amazon S3 service mounted beside kafkaconnect on one engine —
//! exactly the way the assembled emulator runs them, and the way the service takes a
//! plugin's artifact on when the plugin is created.
//!
//! The rule under test: a create that names a stored object in another service's store
//! resolves that object against that service before it mints anything, and when the store
//! answers that the object cannot be read — it is not there, or the read is not permitted —
//! the request is refused with this operation's modelled client error (`BadRequestException`,
//! HTTP 400, an `x-amzn-errortype` header and a `message` naming the offending parameter)
//! and NOTHING is persisted, so no later list or describe can report it. An object the store
//! does answer for is taken on: the plugin is minted and reads back.
//!
//! In this service that rule has exactly one place to hold: `CreateCustomPlugin`'s
//! `location.s3Location` (`CustomPluginLocation$s3Location`, "The S3 bucket Amazon Resource
//! Name (ARN), file key, and object version of the plugin file stored in Amazon S3") is the
//! only member of the whole surface that names an OBJECT this service has to read.
//!
//! Each test asserts the rule over a TABLE whose rows differ along what it generalises
//! over: which part of the address does not resolve (the bucket, the key, the version), how
//! the key is shaped (a bare name, a nested prefix, a key that only prefixes a stored one),
//! whether the request pins an `objectVersion`, whether the bucket is versioned, and both
//! `contentType` values.
//!
//! The last test pins the other half of the rule: a refusal has to be GROUNDED in what the
//! object store answered. With no S3 mounted to ask, nothing is known about the object, so
//! nothing is refused — which is why the suites beside this one may create plugins whose
//! buckets no S3 holds. The test after it pins the region the resolution is addressed in:
//! the caller's signing region only while that field is a Region LABEL, never arbitrary
//! text, because that value lands both in every minted ARN and in the authority of the
//! endpoint URL the in-process resolution is addressed to.

use std::collections::BTreeSet;
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

/// The bucket the readable plugin files below are staged in.
const BUCKET: &str = "kafkaconnect-plugin-artifacts";

/// A second bucket, with versioning enabled, so a request may pin an `objectVersion` that
/// really addresses stored bytes.
const VERSIONED_BUCKET: &str = "kafkaconnect-plugin-artifacts-versioned";

/// A bucket name no `CreateBucket` below ever creates.
const ABSENT_BUCKET: &str = "kafkaconnect-plugin-artifacts-missing";

/// The header Amazon S3 reports a stored version's id in.
const VERSION_ID_HEADER: &str = "x-amz-version-id";

/// The bytes every staged plugin file holds. A custom plugin's artifact is a JAR or a ZIP;
/// nothing here reads its content, so one short payload serves every row.
const ARTIFACT: &[u8] = b"PK\x03\x04 emulator custom plugin artifact";

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
/// production router — what `build_router` assembles, narrowed to the services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_s3_amazons3::EmulatedService::new(engine),
    );
    router
}

/// One kafkaconnect request (`restJson1`, the routes in `OPERATIONS.md`).
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> Answer {
    signed_call(emulator, method, path, body, AUTHORIZATION).await
}

/// One kafkaconnect request under a given `authorization` header, so a case can choose the
/// credential scope the request carries.
async fn signed_call(
    emulator: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Value,
    authorization: &str,
) -> Answer {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("host", format!("kafkaconnect.{REGION}.amazonaws.com"))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    answer(emulator, request).await
}

/// One path-style request to the emulator's S3 front door: the bucket and the key live in
/// the URI and the body is the object's own bytes, so this is a raw request rather than a
/// modelled call. The credential scope names `s3`, which is what the router claims on.
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

/// `PUT /<bucket>` — `CreateBucket`.
async fn create_bucket(emulator: &MultiServiceRouter, bucket: &str) {
    let answered = s3(emulator, "PUT", &format!("/{bucket}"), b"").await;
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
async fn stage(emulator: &MultiServiceRouter, bucket: &str, key: &str) -> String {
    let answered = s3(emulator, "PUT", &format!("/{bucket}/{key}"), ARTIFACT).await;
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

/// Every `name` `ListCustomPlugins` reports.
async fn listed_names(emulator: &MultiServiceRouter) -> BTreeSet<String> {
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
        .map(|summary| text(summary, "name"))
        .collect()
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way an AWS SDK
/// encodes a label: every character a path segment reserves (including the ARN's own `:` and
/// `/`) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// One row of the table: a plugin name, the address it gives the plugin file, and whether the
/// object store can answer for that address.
struct LocationCase {
    /// What the row varies, named in every assertion.
    case: &'static str,
    name: &'static str,
    /// `CustomPluginContentType`: JAR | ZIP (`smithy_query(query="shape",
    /// shape="com.amazonaws.kafkaconnect#CustomPluginContentType")`).
    content_type: &'static str,
    bucket: &'static str,
    file_key: &'static str,
    /// How the row pins a version of the object, if at all.
    version: VersionPin,
}

/// What a row puts in `objectVersion`.
enum VersionPin {
    /// No `objectVersion` at all: the object's current version is the one read.
    None,
    /// The version id the staged write minted — the version the plugin pins really exists.
    Staged,
    /// A version id of the right shape that no write ever minted.
    Absent,
}

/// A version id S3 never minted (the recorded form of an S3 version id, 32 characters of
/// the alphabet S3 draws them from), for the rows that pin a version that is not there.
const UNMINTED_VERSION: &str = "3HL4kqtJlcpXroDTDmjVBH40Nrjfkd42";

/// The keys staged in [`BUCKET`], and the one key staged in [`VERSIONED_BUCKET`]. The
/// refused rows below address around them: a key that is only a PREFIX of one of these, and
/// one that only EXTENDS one, must not resolve.
const STAGED_KEYS: &[&str] = &["plugin.zip", "connectors/debezium/plugin-1.0.0.jar"];

/// The key staged in the versioned bucket.
const VERSIONED_KEY: &str = "versioned/plugin.zip";

/// Rows whose address the object store answers for: the plugin is taken on.
const READABLE: &[LocationCase] = &[
    LocationCase {
        case: "a ZIP at the bucket root",
        name: "root-zip",
        content_type: "ZIP",
        bucket: BUCKET,
        file_key: "plugin.zip",
        version: VersionPin::None,
    },
    LocationCase {
        case: "a JAR under a nested prefix",
        name: "nested-jar",
        content_type: "JAR",
        bucket: BUCKET,
        file_key: "connectors/debezium/plugin-1.0.0.jar",
        version: VersionPin::None,
    },
    LocationCase {
        case: "an objectVersion the write minted",
        name: "pinned-version",
        content_type: "ZIP",
        bucket: VERSIONED_BUCKET,
        file_key: VERSIONED_KEY,
        version: VersionPin::Staged,
    },
    LocationCase {
        case: "a versioned object with no objectVersion pinned",
        name: "unpinned-version",
        content_type: "JAR",
        bucket: VERSIONED_BUCKET,
        file_key: VERSIONED_KEY,
        version: VersionPin::None,
    },
];

/// Rows whose address the object store reports it cannot answer for: nothing is taken on.
const UNREADABLE: &[LocationCase] = &[
    LocationCase {
        case: "a bucket the object store does not hold",
        name: "missing-bucket",
        content_type: "ZIP",
        bucket: ABSENT_BUCKET,
        file_key: "plugin.zip",
        version: VersionPin::None,
    },
    LocationCase {
        case: "a key nothing was ever written to",
        name: "missing-key",
        content_type: "ZIP",
        bucket: BUCKET,
        file_key: "never-uploaded.zip",
        version: VersionPin::None,
    },
    LocationCase {
        case: "a key that only extends a stored one",
        name: "extends-stored-key",
        content_type: "JAR",
        bucket: BUCKET,
        file_key: "plugin.zip.bak",
        version: VersionPin::None,
    },
    LocationCase {
        case: "a key that is only the prefix of a stored one",
        name: "prefixes-stored-key",
        content_type: "JAR",
        bucket: BUCKET,
        file_key: "connectors/debezium",
        version: VersionPin::None,
    },
    LocationCase {
        case: "an objectVersion no write minted, on a versioned object",
        name: "unminted-version",
        content_type: "ZIP",
        bucket: VERSIONED_BUCKET,
        file_key: VERSIONED_KEY,
        version: VersionPin::Absent,
    },
    LocationCase {
        case: "an objectVersion on an object whose bucket keeps no versions",
        name: "unversioned-pin",
        content_type: "ZIP",
        bucket: BUCKET,
        file_key: "plugin.zip",
        version: VersionPin::Absent,
    },
];

/// Stage every readable artifact and answer the version id the versioned write minted.
async fn given_staged_artifacts(emulator: &MultiServiceRouter) -> String {
    create_bucket(emulator, BUCKET).await;
    for key in STAGED_KEYS {
        stage(emulator, BUCKET, key).await;
    }
    create_bucket(emulator, VERSIONED_BUCKET).await;
    enable_versioning(emulator, VERSIONED_BUCKET).await;
    let version_id = stage(emulator, VERSIONED_BUCKET, VERSIONED_KEY).await;
    assert!(
        !version_id.is_empty(),
        "a write into a versioned bucket must report a version id in {VERSION_ID_HEADER}"
    );
    version_id
}

/// The request one row sends.
fn request_for(case: &LocationCase, staged_version: &str) -> Value {
    let object_version = match case.version {
        VersionPin::None => None,
        VersionPin::Staged => Some(staged_version),
        VersionPin::Absent => Some(UNMINTED_VERSION),
    };
    plugin_request(
        case.name,
        case.content_type,
        case.bucket,
        case.file_key,
        object_version,
    )
}

#[tokio::test]
async fn a_custom_plugin_file_the_object_store_answers_for_is_taken_on_and_reads_back() {
    let emulator = emulator();
    let staged_version = given_staged_artifacts(&emulator).await;

    for case in READABLE {
        let body = request_for(case, &staged_version);
        let answered = call(&emulator, "POST", "/v1/custom-plugins", body).await;
        let created = answered.json();
        assert_eq!(
            answered.status, 200,
            "case {:?}: a readable plugin file must be taken on, got {} {created}",
            case.case, answered.status
        );
        let arn = text(&created, "customPluginArn");
        assert!(
            !arn.is_empty(),
            "case {:?}: a created plugin carries its ARN, got {created}",
            case.case
        );

        // Round-trip: the stored plugin reads back through DescribeCustomPlugin with the
        // very location that was resolved.
        let read = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        let described = read.json();
        assert_eq!(
            read.status, 200,
            "case {:?}: DescribeCustomPlugin must answer for the stored plugin, got {} {described}",
            case.case, read.status
        );
        let s3_location = &described["latestRevision"]["location"]["s3Location"];
        assert_eq!(
            text(s3_location, "bucketArn"),
            format!("arn:aws:s3:::{}", case.bucket),
            "case {:?}: the described bucketArn must be the resolved one, got {s3_location}",
            case.case
        );
        assert_eq!(
            text(s3_location, "fileKey"),
            case.file_key,
            "case {:?}: the described fileKey must be the resolved one, got {s3_location}",
            case.case
        );
    }

    let names = listed_names(&emulator).await;
    let expected: BTreeSet<String> = READABLE
        .iter()
        .map(|case| case.name.to_string())
        .collect();
    assert_eq!(
        names, expected,
        "ListCustomPlugins must report exactly the plugins whose file resolved, got {names:?}"
    );
}

#[tokio::test]
async fn a_custom_plugin_file_the_object_store_cannot_read_is_refused_and_nothing_is_stored() {
    let emulator = emulator();
    let staged_version = given_staged_artifacts(&emulator).await;

    for case in UNREADABLE {
        let body = request_for(case, &staged_version);
        let answered = call(&emulator, "POST", "/v1/custom-plugins", body).await;
        let refused = answered.json();
        assert_eq!(
            answered.status, 400,
            "case {:?}: an unreadable plugin file must be refused with 400, got {} {refused}",
            case.case, answered.status
        );
        assert!(
            answered
                .header("x-amzn-errortype")
                .contains("BadRequestException"),
            "case {:?}: the refusal must name the modelled error type, got {:?}",
            case.case,
            answered.header("x-amzn-errortype")
        );
        let message = text(&refused, "message");
        assert!(
            message.contains("s3Location"),
            "case {:?}: the refusal must carry a message naming the offending parameter, \
             got {refused}",
            case.case
        );

        // Nothing was minted: neither the list nor a name-uniqueness check can see the
        // plugin, so a retry with a corrected location cannot collide with it.
        let names = listed_names(&emulator).await;
        assert!(
            !names.contains(case.name),
            "case {:?}: a refused create must leave the store untouched, got {names:?}",
            case.case
        );
    }

    assert!(
        listed_names(&emulator).await.is_empty(),
        "no refused create may leave a plugin behind"
    );

    // The same name, with an address the store answers for, is free to be created: the
    // refusals above took nothing.
    let retried = call(
        &emulator,
        "POST",
        "/v1/custom-plugins",
        plugin_request(
            UNREADABLE[0].name,
            "ZIP",
            BUCKET,
            STAGED_KEYS[0],
            None,
        ),
    )
    .await;
    assert_eq!(
        retried.status,
        200,
        "a retry with a corrected location must be accepted, got {} {}",
        retried.status,
        retried.json()
    );
}

#[tokio::test]
async fn with_no_object_store_to_ask_nothing_about_the_plugin_file_is_refused() {
    // A refusal has to be grounded in what the object store answered. Here there is no S3
    // mounted at all, so nothing is known about the address and the create is accepted —
    // the emulator never refuses on an assumption.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut emulator = MultiServiceRouter::new();
    emulator.register_service(EmulatedService::new(engine));

    for case in UNREADABLE {
        let body = request_for(case, UNMINTED_VERSION);
        let answered = call(&emulator, "POST", "/v1/custom-plugins", body).await;
        let created = answered.json();
        assert_eq!(
            answered.status, 200,
            "case {:?}: with no object store to ask, the create must be accepted, got {} \
             {created}",
            case.case, answered.status
        );
        // Accepted means STORED, not merely un-refused: the plugin reads back through
        // DescribeCustomPlugin under the ARN this call minted.
        let arn = text(&created, "customPluginArn");
        let read = call(
            &emulator,
            "GET",
            &format!("/v1/custom-plugins/{}", label(&arn)),
            Value::Null,
        )
        .await;
        let described = read.json();
        assert_eq!(
            read.status, 200,
            "case {:?}: the accepted plugin must read back, got {} {described}",
            case.case, read.status
        );
        assert_eq!(
            text(&described, "name"),
            case.name,
            "case {:?}: the described plugin must be the created one, got {described}",
            case.case
        );
    }
}

/// The default region every emulator falls back to (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The region the resolution is addressed in, and the region a minted ARN records, is the
/// caller's signing region only while that field is a Region LABEL — the grammar every AWS
/// model's ARN pattern states for it, `[a-z0-9-]{1,20}` (see `service_common::arn`'s
/// `region_of`). Anything else names no AWS endpoint, and must never reach an ARN or the
/// authority of the endpoint URL the in-process resolution is addressed to, so the service
/// falls back to its default region.
///
/// The rows span the published bound and one past it, and each character class that would
/// re-point or truncate a URL authority.
#[tokio::test]
async fn the_region_a_request_is_read_with_is_a_region_label_or_the_default() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut emulator = MultiServiceRouter::new();
    emulator.register_service(EmulatedService::new(engine));

    // (case, the region field of the credential scope, the region every minted ARN carries)
    let rows: &[(&str, &str, &str)] = &[
        ("a Region label", "eu-west-1", "eu-west-1"),
        (
            "a GovCloud label",
            "us-gov-west-1",
            "us-gov-west-1",
        ),
        (
            "a label exactly at the published bound",
            "us-east-1-us-east-1x",
            "us-east-1-us-east-1x",
        ),
        (
            "a label one character past the published bound",
            "us-east-1-us-east-1xy",
            DEFAULT_REGION,
        ),
        (
            "a field carrying a path separator",
            "us-east-1/evil.example.com",
            DEFAULT_REGION,
        ),
        (
            "a field carrying userinfo",
            "us-east-1@evil.example.com",
            DEFAULT_REGION,
        ),
        (
            "a field carrying a fragment",
            "us-east-1#evil.example.com",
            DEFAULT_REGION,
        ),
        ("a field in upper case", "US-EAST-1", DEFAULT_REGION),
    ];

    for (index, (case, scope_region, expected_region)) in rows.iter().enumerate() {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{scope_region}/kafkaconnect/\
             aws4_request, SignedHeaders=host, Signature=0000"
        );
        let name = format!("region-case-{index}");
        let body = plugin_request(&name, "ZIP", BUCKET, STAGED_KEYS[0], None);
        let answered = signed_call(
            &emulator,
            "POST",
            "/v1/custom-plugins",
            body,
            &authorization,
        )
        .await;
        let created = answered.json();
        assert_eq!(
            answered.status, 200,
            "case {case:?}: the create must be answered, got {} {created}",
            answered.status
        );
        let arn = text(&created, "customPluginArn");
        let expected_prefix = format!("arn:aws:kafkaconnect:{expected_region}:");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateFunctionCode`: the package it stores, the size and digest
//! it computes from it, the version it publishes when asked, the `RevisionId`
//! precondition it honours, the dry run that modifies nothing, and the refusals a
//! request the function's package type does not admit gets.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals. Every
//! update is read back through `GetFunction`, so nothing here trusts the update response
//! alone. The clock-driven half — the move of `LastUpdateStatus` from `InProgress` to
//! `Successful` — is exercised by moving the stored due instant into the past, so nothing
//! here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Neither the key id nor the
/// signature is a credential: this harness turns request authentication off below,
/// exactly as the emulator runs it unless started with authentication on, so nothing
/// here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A container image URI, for the `Image` package rows.
const IMAGE_URI: &str = "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Pinned off so the suite does not depend on how the emulator process was started;
    // these tests assert Lambda's behaviour, not the emulator's SigV4 check.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The response body as JSON. Panics with the body text when it is not JSON, which
    /// is the failure a caller needs to see.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The response body as text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`) of an error response.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map_or_else(|| self.text(), str::to_string)
    }
}

/// Dispatch one JSON request, the shape every operation here uses.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// `UpdateFunctionCode` on a function reference, with the given request body.
async fn update_function_code(
    router: &MultiServiceRouter,
    reference: &str,
    body: &Value,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/code");
    json_call(router, "PUT", &uri, body).await
}

/// `PublishVersion` on a function reference.
async fn publish_version(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetFunction` on a function name, optionally qualified by a version.
async fn get_function(router: &MultiServiceRouter, name: &str, version: Option<&str>) -> Answer {
    let uri = match version {
        Some(version) => format!("/2015-03-31/functions/{name}?Qualifier={version}"),
        None => format!("/2015-03-31/functions/{name}"),
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256` must
/// report, so the test never reads the value it is checking out of the response.
fn expected_code_sha256(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// Deployment package bytes of a given length. The content is irrelevant to the API —
/// only its length and its digest are reported — but it differs per length so two
/// packages never share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN a function of `name` must carry.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create a function with the given request body, asserting it was stored.
async fn create_function(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let answer = json_call(router, "POST", "/2015-03-31/functions", body).await;
    assert_eq!(
        answer.status,
        201,
        "case {case}: expected 201 from CreateFunction, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// Create a plain .zip function of `name` from `code`, and answer its unpublished
/// configuration.
async fn create_zip_function(router: &MultiServiceRouter, name: &str, code: &[u8]) -> Value {
    create_function(
        router,
        name,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(code) },
        }),
    )
    .await
}

/// The `RevisionId` of a configuration the API reported.
fn revision_id(case: &str, configuration: &Value) -> String {
    configuration["RevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a RevisionId, got {configuration:?}"))
        .to_string()
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what
/// waiting would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Move every stored function's update-settled instant into the past, so the next read
/// materialises the `InProgress` -> `Successful` move the clock owes. This is what
/// waiting would do, without waiting.
fn make_update_settled(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// One row of the update table: the function to create, the body to update its code
/// with, and what the stored configuration must therefore report.
struct UpdateCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// The `CreateFunction` body.
    create: Value,
    /// The `UpdateFunctionCode` body.
    update: Value,
    /// The version label the call must report.
    version: &'static str,
    /// The `CodeSize` the updated configuration must report.
    code_size: i64,
    /// The `CodeSha256` the updated configuration must report.
    code_sha256: Option<String>,
    /// The `Architectures` the updated configuration must report.
    architectures: Value,
    /// The `PackageType` the updated configuration must still report: this call cannot
    /// change it.
    package_type: &'static str,
    /// The `Code.RepositoryType` `GetFunction` must report for the new package.
    repository_type: &'static str,
    /// The `Code.ResolvedS3Object` `GetFunction` must report for the new package.
    resolved_s3_object: Option<Value>,
}

/// `UpdateFunctionCode` replaces the unpublished configuration's deployment package,
/// reports the size and digest computed from what is stored, mints a fresh revision, and
/// the result is readable afterwards exactly as it was reported.
///
/// The rows differ along what the rule generalises over: where the new package comes from
/// (inline bytes of two sizes, an S3 object in each storage mode, a container image),
/// whether the request also publishes a version, whether the function already had one,
/// and whether the request changes the architecture.
#[tokio::test]
async fn update_function_code_replaces_the_stored_package() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // Both container-image references the image row names — the one it is created on and
    // the one the update moves it to — are RESOLVED against the registry, so both are
    // pushed there first.
    crate::function_lifecycle::seed_image(&engine, IMAGE_URI).await;
    let updated_digest = crate::function_lifecycle::seed_image(
        &engine,
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:2026-01",
    )
    .await;
    // The digest the registry minted for the NEW image is what the updated function's
    // `CodeSha256` reports, since the image IS its deployment package.
    let updated_sha256 = updated_digest
        .split_once(':')
        .map(|(_, hexadecimal)| hexadecimal.to_string());

    let old = package_bytes(64);
    let small = package_bytes(128);
    let large = package_bytes(4_096);

    let cases = vec![
        // The plain case: new inline bytes replace the old ones on `$LATEST`.
        UpdateCase {
            name: "inline-small",
            create: json!({
                "FunctionName": "inline-small",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "ZipFile": zip_file(&small) }),
            version: "$LATEST",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // A different package length, so the reported size and digest are computed from
        // the bytes rather than carried over from the row above.
        UpdateCase {
            name: "inline-large",
            create: json!({
                "FunctionName": "inline-large",
                "Role": ROLE,
                "Runtime": "nodejs20.x",
                "Handler": "app.main",
                "MemorySize": 1024,
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "ZipFile": zip_file(&large) }),
            version: "$LATEST",
            code_size: i64::try_from(large.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&large)),
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // "Set to true to publish a new version of the function after updating the code.
        // This has the same effect as calling PublishVersion separately."
        UpdateCase {
            name: "inline-published",
            create: json!({
                "FunctionName": "inline-published",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "ZipFile": zip_file(&small), "Publish": true }),
            version: "1",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // A function that already holds version 1: a version number is never reused, so
        // the one this call publishes follows the highest published so far.
        UpdateCase {
            name: "second-version",
            create: json!({
                "FunctionName": "second-version",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Publish": true,
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "ZipFile": zip_file(&large), "Publish": true }),
            version: "2",
            code_size: i64::try_from(large.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&large)),
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // "The instruction set architecture that the function supports": the update
        // carries it, so the stored configuration reports the new one.
        UpdateCase {
            name: "to-arm64",
            create: json!({
                "FunctionName": "to-arm64",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "ZipFile": zip_file(&small), "Architectures": ["arm64"] }),
            version: "$LATEST",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            architectures: json!(["arm64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // An S3 object in the default `COPY` mode: this emulator holds no bytes for it,
        // so it reports no size it did not measure and no digest it did not compute, and
        // Lambda serves the package from a copy of its own rather than from the caller's
        // object.
        UpdateCase {
            name: "to-s3-copy",
            create: json!({
                "FunctionName": "to-s3-copy",
                "Role": ROLE,
                "Runtime": "ruby3.3",
                "Handler": "app.handler",
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({ "S3Bucket": "deployments", "S3Key": "fn.zip" }),
            version: "$LATEST",
            code_size: 0,
            code_sha256: None,
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: None,
        },
        // "REFERENCE – Lambda references the deployment package from the specified
        // Amazon S3 bucket", so in that mode the caller's object IS the resolved one.
        UpdateCase {
            name: "to-s3-reference",
            create: json!({
                "FunctionName": "to-s3-reference",
                "Role": ROLE,
                "Runtime": "java21",
                "Handler": "app::handler",
                "Code": { "ZipFile": zip_file(&old) },
            }),
            update: json!({
                "S3Bucket": "deployments",
                "S3Key": "fn.zip",
                "S3ObjectVersion": "v7",
                "S3ObjectStorageMode": "REFERENCE",
            }),
            version: "$LATEST",
            code_size: 0,
            code_sha256: None,
            architectures: json!(["x86_64"]),
            package_type: "Zip",
            repository_type: "S3",
            resolved_s3_object: Some(json!({
                "S3Bucket": "deployments",
                "S3Key": "fn.zip",
                "S3ObjectVersion": "v7",
            })),
        },
        // "If the function's package type is Image, then you must specify the code
        // package in ImageUri": the new image is the one the function resolves to, and its
        // `CodeSha256` is that image's own digest.
        UpdateCase {
            name: "image",
            create: json!({
                "FunctionName": "image",
                "Role": ROLE,
                "PackageType": "Image",
                "MemorySize": 2048,
                "Code": { "ImageUri": IMAGE_URI },
            }),
            update: json!({
                "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:2026-01",
            }),
            version: "$LATEST",
            code_size: 0,
            // The digest the REGISTRY minted for the new image, since the image IS the
            // deployment package.
            code_sha256: updated_sha256,
            architectures: json!(["x86_64"]),
            package_type: "Image",
            repository_type: "ECR",
            resolved_s3_object: None,
        },
    ];

    for case in &cases {
        let created = create_function(&router, case.name, &case.create).await;
        let created_revision = revision_id(case.name, &created);
        // "When the code is Creating, you can't invoke or modify the function": the
        // function finishes being provisioned before its code is updated.
        make_activation_due(&engine);

        let answer = update_function_code(&router, case.name, &case.update).await;
        assert_eq!(
            answer.status,
            200,
            "case {}: expected 200 from UpdateFunctionCode, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let updated = answer.json();

        assert_eq!(
            updated["Version"],
            json!(case.version),
            "case {}: expected Version {:?}, got {:?}",
            case.name,
            case.version,
            updated["Version"]
        );
        // `UpdateFunctionCode`'s subject is the function, so the ARN it reports is the
        // function's own unqualified one whether or not the call also published a version;
        // the published version is reported by `Version` alone, asserted above.
        let expected_arn = function_arn(case.name);
        assert_eq!(
            updated["FunctionArn"],
            json!(expected_arn),
            "case {}: expected FunctionArn {expected_arn:?}, got {:?}",
            case.name,
            updated["FunctionArn"]
        );
        assert_eq!(
            updated["CodeSize"],
            json!(case.code_size),
            "case {}: expected CodeSize {}, got {:?}",
            case.name,
            case.code_size,
            updated["CodeSize"]
        );
        assert_eq!(
            updated["CodeSha256"],
            case.code_sha256
                .clone()
                .map_or(Value::Null, Value::String),
            "case {}: expected CodeSha256 {:?}, got {:?}",
            case.name,
            case.code_sha256,
            updated["CodeSha256"]
        );
        assert_eq!(
            updated["Architectures"], case.architectures,
            "case {}: expected Architectures {:?}, got {:?}",
            case.name, case.architectures, updated["Architectures"]
        );
        assert_eq!(
            updated["PackageType"],
            json!(case.package_type),
            "case {}: expected PackageType {:?}, got {:?}",
            case.name,
            case.package_type,
            updated["PackageType"]
        );
        // "The latest updated revision of the function": a change to the code is a
        // change, so the revision the caller read before it is no longer current.
        let updated_revision = revision_id(case.name, &updated);
        assert_ne!(
            updated_revision, created_revision,
            "case {}: expected a fresh RevisionId, got the created one {created_revision:?}",
            case.name
        );
        // "The status of the last update that was performed on the function": the update
        // is accepted at once and applied behind the call, so the UNPUBLISHED version
        // reports it as in flight. A version this call published froze the updated
        // configuration and is not the thing being updated, so it reports the settled
        // status.
        let expected_update_status = if case.version == "$LATEST" {
            "InProgress"
        } else {
            "Successful"
        };
        assert_eq!(
            updated["LastUpdateStatus"],
            json!(expected_update_status),
            "case {}: expected LastUpdateStatus {expected_update_status} right after the \
             update, got {:?}",
            case.name,
            updated["LastUpdateStatus"]
        );

        // Read the result back, so nothing here trusts the update response alone.
        let fetched = get_function(&router, case.name, Some(case.version)).await;
        assert_eq!(
            fetched.status,
            200,
            "case {}: expected 200 from GetFunction, got {} {}",
            case.name,
            fetched.status,
            fetched.text()
        );
        let body = fetched.json();
        let configuration = &body["Configuration"];
        assert_eq!(
            configuration["CodeSize"],
            json!(case.code_size),
            "case {}: expected the stored CodeSize {}, got {:?}",
            case.name,
            case.code_size,
            configuration["CodeSize"]
        );
        assert_eq!(
            configuration["CodeSha256"],
            case.code_sha256
                .clone()
                .map_or(Value::Null, Value::String),
            "case {}: expected the stored CodeSha256 {:?}, got {:?}",
            case.name,
            case.code_sha256,
            configuration["CodeSha256"]
        );
        assert_eq!(
            configuration["RevisionId"],
            json!(updated_revision),
            "case {}: expected the stored RevisionId {updated_revision:?}, got {:?}",
            case.name,
            configuration["RevisionId"]
        );
        assert_eq!(
            configuration["Architectures"], case.architectures,
            "case {}: expected the stored Architectures {:?}, got {:?}",
            case.name, case.architectures, configuration["Architectures"]
        );
        let code = &body["Code"];
        assert_eq!(
            code["RepositoryType"],
            json!(case.repository_type),
            "case {}: expected RepositoryType {:?}, got {:?}",
            case.name,
            case.repository_type,
            code["RepositoryType"]
        );
        assert_eq!(
            code["ResolvedS3Object"],
            case.resolved_s3_object.clone().unwrap_or(Value::Null),
            "case {}: expected ResolvedS3Object {:?}, got {:?}",
            case.name,
            case.resolved_s3_object,
            code["ResolvedS3Object"]
        );
        if case.package_type == "Image" {
            assert_eq!(
                code["ImageUri"], case.update["ImageUri"],
                "case {}: expected the new ImageUri {:?}, got {:?}",
                case.name, case.update["ImageUri"], code["ImageUri"]
            );
        }
    }
}

/// "The function's code is locked when you publish a version. You can't modify the code
/// of a published version, only the unpublished version."
#[tokio::test]
async fn the_code_of_a_published_version_is_locked() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let first = package_bytes(96);
    let second = package_bytes(192);

    create_zip_function(&router, "locked", &first).await;
    make_activation_due(&engine);
    let published = publish_version(&router, "locked", &Value::Null).await;
    assert_eq!(
        published.status,
        201,
        "expected 201 from PublishVersion, got {} {}",
        published.status,
        published.text()
    );

    let updated = update_function_code(&router, "locked", &json!({ "ZipFile": zip_file(&second) }))
        .await;
    assert_eq!(
        updated.status,
        200,
        "expected 200 from UpdateFunctionCode on the unpublished version, got {} {}",
        updated.status,
        updated.text()
    );

    // The unpublished version carries the new package ...
    let latest = get_function(&router, "locked", None).await.json();
    assert_eq!(
        latest["Configuration"]["CodeSha256"],
        json!(expected_code_sha256(&second)),
        "case $LATEST: expected the new digest, got {:?}",
        latest["Configuration"]["CodeSha256"]
    );
    // ... and version 1 still carries the one it froze.
    let version_one = get_function(&router, "locked", Some("1")).await.json();
    assert_eq!(
        version_one["Configuration"]["CodeSha256"],
        json!(expected_code_sha256(&first)),
        "case version 1: expected the frozen digest, got {:?}",
        version_one["Configuration"]["CodeSha256"]
    );

    // And a request that names the published version is refused rather than served.
    let refused = update_function_code(&router, "locked:1", &json!({ "ZipFile": zip_file(&first) }))
        .await;
    assert_eq!(
        refused.status,
        400,
        "expected 400 for a published version, got {} {}",
        refused.status,
        refused.text()
    );
    assert!(
        refused
            .error_code()
            .contains("InvalidParameterValueException"),
        "expected InvalidParameterValueException for a published version, got {}",
        refused.error_code()
    );
}

/// "Set to true to validate the request parameters and access permissions without
/// modifying the function code."
#[tokio::test]
async fn a_dry_run_validates_without_modifying_the_code() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let stored = package_bytes(320);
    let candidate = package_bytes(640);
    let created = create_zip_function(&router, "dry-run", &stored).await;
    let created_revision = revision_id("dry-run", &created);
    make_activation_due(&engine);

    // A valid dry run is accepted and reports the package it VALIDATED — the pre-deploy
    // validation step reports the hash of the artifact it checked — while every other
    // member is the configuration the store still holds.
    let answer = update_function_code(
        &router,
        "dry-run",
        &json!({ "ZipFile": zip_file(&candidate), "DryRun": true, "Publish": true }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "expected 200 from a dry run, got {} {}",
        answer.status,
        answer.text()
    );
    let reported = answer.json();
    for (member, expected) in [
        ("CodeSha256", json!(expected_code_sha256(&candidate))),
        (
            "CodeSize",
            json!(i64::try_from(candidate.len()).unwrap_or_default()),
        ),
        ("RevisionId", json!(created_revision.clone())),
        ("Version", json!("$LATEST")),
    ] {
        assert_eq!(
            reported[member], expected,
            "case dry run: expected {member} {expected:?}, got {:?}",
            reported[member]
        );
    }

    // Nothing was persisted: neither the package nor a published version.
    let fetched = get_function(&router, "dry-run", None).await.json();
    assert_eq!(
        fetched["Configuration"]["CodeSha256"],
        json!(expected_code_sha256(&stored)),
        "case dry run: expected the stored digest to be unchanged, got {:?}",
        fetched["Configuration"]["CodeSha256"]
    );
    assert_eq!(
        fetched["Configuration"]["RevisionId"],
        json!(created_revision),
        "case dry run: expected the stored RevisionId to be unchanged, got {:?}",
        fetched["Configuration"]["RevisionId"]
    );
    let version_one = get_function(&router, "dry-run", Some("1")).await;
    assert_eq!(
        version_one.status,
        404,
        "case dry run: expected no published version, got {} {}",
        version_one.status,
        version_one.text()
    );

    // A dry run of a request that is NOT valid is refused exactly as the real one is:
    // this function's package is a .zip archive, so an image URI names no code for it.
    let refused = update_function_code(
        &router,
        "dry-run",
        &json!({ "ImageUri": IMAGE_URI, "DryRun": true }),
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "case dry run of an invalid request: expected 400, got {} {}",
        refused.status,
        refused.text()
    );
}

/// `LastUpdateStatus` moves from `InProgress` to `Successful` once the clock makes the
/// move due, and not before.
#[tokio::test]
async fn the_last_update_status_settles_once_the_clock_makes_it_due() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "settling", &package_bytes(48)).await;
    // The function finishes being provisioned before the update, so the only transition
    // left for the clock to owe is the one this suite is about.
    make_activation_due(&engine);
    let answer = update_function_code(
        &router,
        "settling",
        &json!({ "ZipFile": zip_file(&package_bytes(96)) }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "expected 200 from UpdateFunctionCode, got {} {}",
        answer.status,
        answer.text()
    );

    let in_progress = get_function(&router, "settling", None).await.json();
    assert_eq!(
        in_progress["Configuration"]["LastUpdateStatus"],
        json!("InProgress"),
        "expected LastUpdateStatus InProgress before the update is settled, got {:?}",
        in_progress["Configuration"]["LastUpdateStatus"]
    );

    make_update_settled(&engine);

    let settled = get_function(&router, "settling", None).await.json();
    assert_eq!(
        settled["Configuration"]["LastUpdateStatus"],
        json!("Successful"),
        "expected LastUpdateStatus Successful once the update is due to be settled, got {:?}",
        settled["Configuration"]["LastUpdateStatus"]
    );
    // A settled update changes no other reported fact about the function.
    assert_eq!(
        settled["Configuration"]["State"],
        json!("Active"),
        "expected State Active throughout, got {:?}",
        settled["Configuration"]["State"]
    );
}

/// What a precondition row sends, resolved against the created function.
enum PreconditionBody {
    /// `RevisionId` is the function's own.
    MatchingRevisionId,
    /// `RevisionId` is some other revision.
    ForeignRevisionId,
}

/// One row of the precondition table.
struct PreconditionCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// What the request carries as its `RevisionId`.
    body: PreconditionBody,
    /// The status the call must answer.
    status: u16,
    /// The wire error code the call must answer, for a refusal.
    error_code: Option<&'static str>,
}

/// "Update the function only if the revision ID matches the ID that's specified": a
/// mismatch is refused with the modelled 412 and changes nothing.
#[tokio::test]
async fn the_revision_id_precondition_gates_the_update() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let stored = package_bytes(72);
    let candidate = package_bytes(144);

    let cases = [
        PreconditionCase {
            name: "revision-matches",
            body: PreconditionBody::MatchingRevisionId,
            status: 200,
            error_code: None,
        },
        PreconditionCase {
            name: "revision-differs",
            body: PreconditionBody::ForeignRevisionId,
            status: 412,
            error_code: Some("PreconditionFailedException"),
        },
    ];

    for case in &cases {
        let created = create_zip_function(&router, case.name, &stored).await;
        let created_revision = revision_id(case.name, &created);
        make_activation_due(&engine);
        let revision = match case.body {
            PreconditionBody::MatchingRevisionId => created_revision.clone(),
            PreconditionBody::ForeignRevisionId => "11111111-2222-3333-4444-555555555555".to_string(),
        };

        let answer = update_function_code(
            &router,
            case.name,
            &json!({ "ZipFile": zip_file(&candidate), "RevisionId": revision }),
        )
        .await;
        assert_eq!(
            answer.status,
            case.status,
            "case {}: expected {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );

        let fetched = get_function(&router, case.name, None).await.json();
        let expected_digest = match case.error_code {
            // The accepted update stored the new package ...
            None => expected_code_sha256(&candidate),
            // ... and the refused one left the old one in place.
            Some(code) => {
                assert!(
                    answer.error_code().contains(code),
                    "case {}: expected {code}, got {}",
                    case.name,
                    answer.error_code()
                );
                expected_code_sha256(&stored)
            }
        };
        assert_eq!(
            fetched["Configuration"]["CodeSha256"],
            json!(expected_digest),
            "case {}: expected the stored digest {expected_digest:?}, got {:?}",
            case.name,
            fetched["Configuration"]["CodeSha256"]
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    /// The case name.
    name: &'static str,
    /// The function reference in the URI.
    reference: &'static str,
    /// The request body.
    body: Value,
    /// The status the call must answer.
    status: u16,
    /// The wire error code the call must answer.
    error_code: &'static str,
}

/// A request that names no code the function's package type admits is refused with the
/// modelled error, and the one member whose behaviour the model does not state is
/// answered with the scaffold's 501 rather than with invented behaviour.
#[tokio::test]
async fn update_function_code_refuses_a_request_it_cannot_serve() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // The image function's reference is resolved against the registry at create.
    crate::function_lifecycle::seed_image(&engine, IMAGE_URI).await;

    let code = package_bytes(160);
    create_zip_function(&router, "refusals", &code).await;
    create_function(
        &router,
        "image-refusals",
        &json!({
            "FunctionName": "image-refusals",
            "Role": ROLE,
            "PackageType": "Image",
            "Code": { "ImageUri": IMAGE_URI },
        }),
    )
    .await;
    make_activation_due(&engine);

    let cases = [
        RefusalCase {
            name: "unknown-function",
            reference: "absent",
            body: json!({ "ZipFile": zip_file(&code) }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "unknown-function-by-arn",
            reference: "arn:aws:lambda:us-east-1:000000000000:function:absent",
            body: json!({ "ZipFile": zip_file(&code) }),
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // A qualifier names a published version or an alias, and the code of neither can
        // be modified.
        RefusalCase {
            name: "qualified-by-alias",
            reference: "refusals:prod",
            body: json!({ "ZipFile": zip_file(&code) }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // "If the function's package type is Zip, then you must specify the deployment
        // package as a .zip file archive."
        RefusalCase {
            name: "no-code-named",
            reference: "refusals",
            body: json!({}),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "s3-bucket-without-key",
            reference: "refusals",
            body: json!({ "S3Bucket": "deployments" }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "inline-and-s3",
            reference: "refusals",
            body: json!({
                "ZipFile": zip_file(&code),
                "S3Bucket": "deployments",
                "S3Key": "fn.zip",
            }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // An S3-only member beside inline bytes names a second package the same way a
        // bucket and key do, so it is refused rather than silently dropped.
        RefusalCase {
            name: "inline-and-s3-object-version",
            reference: "refusals",
            body: json!({ "ZipFile": zip_file(&code), "S3ObjectVersion": "v1" }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "inline-and-s3-storage-mode",
            reference: "refusals",
            body: json!({ "ZipFile": zip_file(&code), "S3ObjectStorageMode": "REFERENCE" }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // "Do not use for a function defined with a .zip file archive."
        RefusalCase {
            name: "image-uri-on-zip-function",
            reference: "refusals",
            body: json!({ "ImageUri": IMAGE_URI }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // ... and the mirror of it: a .zip archive names no code for an image function.
        RefusalCase {
            name: "zip-on-image-function",
            reference: "image-refusals",
            body: json!({ "ZipFile": zip_file(&code) }),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "no-image-uri-on-image-function",
            reference: "image-refusals",
            body: json!({}),
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // `PublishTo` names a destination the model describes in one sentence and whose
        // label the `Version` output member cannot express, so it is answered as not
        // implemented rather than served as an ordinary update.
        RefusalCase {
            name: "publish-to",
            reference: "refusals",
            body: json!({ "ZipFile": zip_file(&code), "PublishTo": "LATEST_PUBLISHED" }),
            status: 501,
            error_code: "NotImplemented",
        },
    ];

    for case in &cases {
        let answer = update_function_code(&router, case.reference, &case.body).await;
        assert_eq!(
            answer.status,
            case.status,
            "case {}: expected {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.error_code),
            "case {}: expected {}, got {}",
            case.name,
            case.error_code,
            answer.error_code()
        );
    }

    // No refusal changed anything: both functions still report the package they were
    // created with, and neither holds a published version.
    let zip_function = get_function(&router, "refusals", None).await.json();
    assert_eq!(
        zip_function["Configuration"]["CodeSha256"],
        json!(expected_code_sha256(&code)),
        "expected the refused updates to leave the stored digest alone, got {:?}",
        zip_function["Configuration"]["CodeSha256"]
    );
    let image_function = get_function(&router, "image-refusals", None).await.json();
    assert_eq!(
        image_function["Code"]["ImageUri"],
        json!(IMAGE_URI),
        "expected the refused updates to leave the stored image alone, got {:?}",
        image_function["Code"]["ImageUri"]
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PublishVersion`: the snapshot it freezes, the version numbers
//! it hands out, the "nothing changed, nothing published" rule the model states, and the
//! preconditions and refusals it answers.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than about any handler's internals. Each
//! published version is read back through `GetFunction`, so nothing here trusts the
//! publishing response alone.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    mint_revision_id, ComAmazonawsLambdaAwsgirapiserviceState, FunctionVersionRecord,
    MAX_FUNCTION_VERSIONS,
};
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

/// `PublishVersion` on a function reference, with the given request body.
async fn publish_version(router: &MultiServiceRouter, reference: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{reference}/versions");
    json_call(router, "POST", &uri, body).await
}

/// `GetFunction` on a function name, qualified by `version`.
async fn get_function_version(
    router: &MultiServiceRouter,
    name: &str,
    version: &str,
) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}?Qualifier={version}");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// must report, so the test never reads the value it is checking out of the response.
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

/// A `Code.ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
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

/// One row of the publishing table: a function to create, the body to publish with, and
/// the snapshot the published version must therefore report.
struct PublishCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// The `CreateFunction` body.
    create: Value,
    /// The `PublishVersion` body.
    publish: Value,
    /// The version label the call must report.
    version: &'static str,
    /// The `CodeSize` the frozen configuration must report.
    code_size: i64,
    /// The `CodeSha256` the frozen configuration must report.
    code_sha256: Option<String>,
    /// The `Description` the frozen configuration must report. `Some("")` for a version
    /// of a function created without one: the member is part of every
    /// `FunctionConfiguration` AWS reports.
    description: Option<&'static str>,
    /// The `Description` the UNPUBLISHED configuration must still report, under the same
    /// rule.
    latest_description: Option<&'static str>,
    /// The `MemorySize` the frozen configuration must report.
    memory_size: i64,
    /// The `Timeout` the frozen configuration must report.
    timeout: i64,
    /// The `PackageType` the frozen configuration must report.
    package_type: &'static str,
}

/// `PublishVersion` freezes the function's current code and configuration as a numbered
/// version, and that version is readable afterwards exactly as it was reported.
///
/// The rows differ along what the rule generalises over: the package the code came from
/// (inline bytes, S3, a container image), the configuration values frozen, whether the
/// request overrides the description, and whether the function already had a published
/// version (so the next number follows the highest one published).
#[tokio::test]
async fn publish_version_freezes_the_current_code_and_configuration() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // The container-image row's reference is resolved against the registry at create, so
    // the image it names is pushed there first — and the digest the registry minted is what
    // that function's `CodeSha256` reports, since the image IS its deployment package.
    let image_digest = crate::function_lifecycle::seed_image(
        &engine,
        "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest",
    )
    .await;
    let image_sha256 = image_digest
        .split_once(':')
        .map(|(_, hexadecimal)| hexadecimal.to_string());

    let small = package_bytes(64);
    let large = package_bytes(4_096);

    let cases = vec![
        // The plain case: an inline .zip archive with every optional member defaulted.
        PublishCase {
            name: "inline-defaults",
            create: json!({
                "FunctionName": "inline-defaults",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&small) },
            }),
            publish: Value::Null,
            version: "1",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            description: Some(""),
            latest_description: Some(""),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip",
        },
        // A fully configured function: the snapshot is the configuration the function
        // holds, not a set of defaults.
        PublishCase {
            name: "configured",
            create: json!({
                "FunctionName": "configured",
                "Role": ROLE,
                "Runtime": "nodejs20.x",
                "Handler": "app.main",
                "Description": "the function",
                "Timeout": 900,
                "MemorySize": 1024,
                "Environment": { "Variables": { "STAGE": "prod" } },
                "Code": { "ZipFile": zip_file(&large) },
            }),
            publish: Value::Null,
            version: "1",
            code_size: i64::try_from(large.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&large)),
            description: Some("the function"),
            latest_description: Some("the function"),
            memory_size: 1024,
            timeout: 900,
            package_type: "Zip",
        },
        // "A description for the version to override the description in the function
        // configuration": the override belongs to the version, and the unpublished
        // configuration keeps its own description.
        PublishCase {
            name: "described",
            create: json!({
                "FunctionName": "described",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Description": "the function",
                "Code": { "ZipFile": zip_file(&small) },
            }),
            publish: json!({ "Description": "release 2026-01" }),
            version: "1",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            description: Some("release 2026-01"),
            latest_description: Some("the function"),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip",
        },
        // An S3-sourced archive: this emulator holds no bytes for it, so the snapshot
        // reports no size it did not measure and no digest it did not compute.
        PublishCase {
            name: "from-s3",
            create: json!({
                "FunctionName": "from-s3",
                "Role": ROLE,
                "Runtime": "ruby3.3",
                "Handler": "app.handler",
                "Code": { "S3Bucket": "deployments", "S3Key": "fn.zip" },
            }),
            publish: Value::Null,
            version: "1",
            code_size: 0,
            code_sha256: None,
            description: Some(""),
            latest_description: Some(""),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip",
        },
        // A container image package, which carries no runtime and no handler.
        PublishCase {
            name: "from-image",
            create: json!({
                "FunctionName": "from-image",
                "Role": ROLE,
                "PackageType": "Image",
                "MemorySize": 2048,
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
            publish: Value::Null,
            version: "1",
            code_size: 0,
            // An `Image` package's `CodeSha256` is the digest of the image the function is
            // frozen to — the one the REGISTRY minted — which the published version freezes
            // with the rest of its code.
            code_sha256: image_sha256,
            description: Some(""),
            latest_description: Some(""),
            memory_size: 2048,
            timeout: 3,
            package_type: "Image",
        },
        // A function created with `Publish: true` already holds version 1, so the next
        // version follows it rather than restarting the numbering.
        PublishCase {
            name: "already-published",
            create: json!({
                "FunctionName": "already-published",
                "Role": ROLE,
                "Runtime": "java21",
                "Handler": "Handler::run",
                "Publish": true,
                "Code": { "ZipFile": zip_file(&small) },
            }),
            publish: json!({ "Description": "the second version" }),
            version: "2",
            code_size: i64::try_from(small.len()).unwrap_or_default(),
            code_sha256: Some(expected_code_sha256(&small)),
            description: Some("the second version"),
            latest_description: Some(""),
            memory_size: 128,
            timeout: 3,
            package_type: "Zip",
        },
    ];

    for case in &cases {
        create_function(&router, case.name, &case.create).await;

        let answer = publish_version(&router, case.name, &case.publish).await;
        assert_eq!(
            answer.status,
            201,
            "case {}: expected 201 from PublishVersion, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let published = answer.json();

        let expected_arn = format!("{}:{}", function_arn(case.name), case.version);
        assert_eq!(
            published["Version"],
            json!(case.version),
            "case {}: expected Version {}, got {:?}",
            case.name,
            case.version,
            published["Version"]
        );
        assert_eq!(
            published["FunctionArn"],
            json!(expected_arn),
            "case {}: expected FunctionArn {expected_arn}, got {:?}",
            case.name,
            published["FunctionArn"]
        );
        assert_eq!(
            published["FunctionName"],
            json!(case.name),
            "case {}: expected FunctionName {}, got {:?}",
            case.name,
            case.name,
            published["FunctionName"]
        );
        assert_eq!(
            published["CodeSize"],
            json!(case.code_size),
            "case {}: expected CodeSize {}, got {:?}",
            case.name,
            case.code_size,
            published["CodeSize"]
        );
        assert_eq!(
            published["CodeSha256"].as_str(),
            case.code_sha256.as_deref(),
            "case {}: expected CodeSha256 {:?}, got {:?}",
            case.name,
            case.code_sha256,
            published["CodeSha256"]
        );
        assert_eq!(
            published["Description"].as_str(),
            case.description,
            "case {}: expected Description {:?}, got {:?}",
            case.name,
            case.description,
            published["Description"]
        );
        assert_eq!(
            published["MemorySize"],
            json!(case.memory_size),
            "case {}: expected MemorySize {}, got {:?}",
            case.name,
            case.memory_size,
            published["MemorySize"]
        );
        assert_eq!(
            published["Timeout"],
            json!(case.timeout),
            "case {}: expected Timeout {}, got {:?}",
            case.name,
            case.timeout,
            published["Timeout"]
        );
        assert_eq!(
            published["PackageType"],
            json!(case.package_type),
            "case {}: expected PackageType {}, got {:?}",
            case.name,
            case.package_type,
            published["PackageType"]
        );
        assert!(
            published["RevisionId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {}: expected a RevisionId on the published version, got {:?}",
            case.name,
            published["RevisionId"]
        );

        // Round trip: the version is readable afterwards, and reports the same frozen
        // snapshot.
        let read_back = get_function_version(&router, case.name, case.version).await;
        assert_eq!(
            read_back.status,
            200,
            "case {}: expected 200 reading version {} back, got {} {}",
            case.name,
            case.version,
            read_back.status,
            read_back.text()
        );
        let configuration = read_back.json()["Configuration"].clone();
        assert_eq!(
            configuration["Version"], published["Version"],
            "case {}: expected GetFunction to report Version {:?}, got {:?}",
            case.name, published["Version"], configuration["Version"]
        );
        assert_eq!(
            configuration["Description"], published["Description"],
            "case {}: expected GetFunction to report Description {:?}, got {:?}",
            case.name, published["Description"], configuration["Description"]
        );
        assert_eq!(
            configuration["CodeSha256"], published["CodeSha256"],
            "case {}: expected GetFunction to report CodeSha256 {:?}, got {:?}",
            case.name, published["CodeSha256"], configuration["CodeSha256"]
        );
        assert_eq!(
            configuration["MemorySize"], published["MemorySize"],
            "case {}: expected GetFunction to report MemorySize {:?}, got {:?}",
            case.name, published["MemorySize"], configuration["MemorySize"]
        );

        // The unpublished configuration is untouched by the publish: it keeps its own
        // description and still reports `$LATEST`.
        let latest = get_function_version(&router, case.name, "$LATEST").await;
        assert_eq!(
            latest.status,
            200,
            "case {}: expected 200 reading $LATEST back, got {} {}",
            case.name,
            latest.status,
            latest.text()
        );
        let latest_configuration = latest.json()["Configuration"].clone();
        assert_eq!(
            latest_configuration["Description"].as_str(),
            case.latest_description,
            "case {}: expected the unpublished Description {:?}, got {:?}",
            case.name,
            case.latest_description,
            latest_configuration["Description"]
        );
        assert_eq!(
            latest_configuration["Version"],
            json!("$LATEST"),
            "case {}: expected the unpublished version to stay $LATEST, got {:?}",
            case.name,
            latest_configuration["Version"]
        );
    }
}

/// How many entries a `FunctionVersion=ALL` listing holds for published versions of
/// `name` — the independent count of what publishing actually persisted.
async fn published_version_count(router: &MultiServiceRouter, name: &str) -> usize {
    let answer = json_call(
        router,
        "GET",
        "/2015-03-31/functions?FunctionVersion=ALL",
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "expected 200 from ListFunctions, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()["Functions"]
        .as_array()
        .map(|functions| {
            functions
                .iter()
                .filter(|function| {
                    function["FunctionName"].as_str() == Some(name)
                        && function["Version"].as_str() != Some("$LATEST")
                })
                .count()
        })
        .unwrap_or_default()
}

/// "Lambda doesn't publish a version if the function's configuration and code haven't
/// changed since the last version": a repeated publish reports the version that already
/// froze that configuration, and persists no second copy of it.
#[tokio::test]
async fn publishing_an_unchanged_function_reports_the_version_it_already_published() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "stable", &package_bytes(128)).await;

    // Each row is the body to publish with and the version it must report. The first
    // publish of a configuration mints a number; publishing the same configuration again
    // reports the same one, and a version whose frozen description differs is a
    // different snapshot, so it gets its own number. The rule is scoped to the LAST
    // version, so a configuration an older version froze has changed since that last one
    // and is published again under a new number.
    let rounds = [
        (Value::Null, "1", 1usize),
        (Value::Null, "1", 1),
        (json!({ "Description": "second" }), "2", 2),
        (json!({ "Description": "second" }), "2", 2),
        (json!({ "Description": "third" }), "3", 3),
        (json!({ "Description": "third" }), "3", 3),
        // Back to the snapshot version 2 froze: that is a change since the last version,
        // so it gets its own number rather than re-reporting version 2.
        (json!({ "Description": "second" }), "4", 4),
        (json!({ "Description": "second" }), "4", 4),
    ];

    for (round, (body, version, versions_held)) in rounds.iter().enumerate() {
        let answer = publish_version(&router, "stable", body).await;
        assert_eq!(
            answer.status,
            201,
            "round {round}: expected 201, got {} {}",
            answer.status,
            answer.text()
        );
        let published = answer.json();
        assert_eq!(
            published["Version"],
            json!(version),
            "round {round}: expected Version {version}, got {:?}",
            published["Version"]
        );
        let held = published_version_count(&router, "stable").await;
        assert_eq!(
            held, *versions_held,
            "round {round}: expected {versions_held} published version(s) held, got {held}"
        );
    }
}

/// One row of the precondition table.
struct PreconditionCase {
    /// The case name, which is also the function name.
    name: &'static str,
    /// Whether the `PublishVersion` body carries the function's own digest, a foreign
    /// one, the function's own revision id, or a foreign one.
    body: PreconditionBody,
    /// The status the call must answer.
    status: u16,
    /// The wire error code the call must answer, for a refusal.
    error_code: Option<&'static str>,
}

/// What a precondition row sends, resolved against the created function.
enum PreconditionBody {
    /// `CodeSha256` is the digest of the function's own package.
    MatchingCodeSha256,
    /// `CodeSha256` is the digest of some other bytes.
    ForeignCodeSha256,
    /// `RevisionId` is the function's own revision id.
    MatchingRevisionId,
    /// `RevisionId` is a revision id the function never had.
    ForeignRevisionId,
}

/// `CodeSha256` and `RevisionId` are preconditions: the version is published only when
/// the value supplied is the one the function holds, and each mismatch is refused with
/// the error its own member is modelled with.
#[tokio::test]
async fn publish_version_honours_the_code_sha256_and_revision_id_preconditions() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let code = package_bytes(256);
    let other_code = package_bytes(512);

    let cases = [
        PreconditionCase {
            name: "sha-matches",
            body: PreconditionBody::MatchingCodeSha256,
            status: 201,
            error_code: None,
        },
        PreconditionCase {
            name: "sha-differs",
            body: PreconditionBody::ForeignCodeSha256,
            status: 400,
            error_code: Some("InvalidParameterValueException"),
        },
        PreconditionCase {
            name: "revision-matches",
            body: PreconditionBody::MatchingRevisionId,
            status: 201,
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
        let created = create_zip_function(&router, case.name, &code).await;
        let revision_id = created["RevisionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: CreateFunction must report a RevisionId", case.name))
            .to_string();
        let body = match case.body {
            PreconditionBody::MatchingCodeSha256 => {
                json!({ "CodeSha256": expected_code_sha256(&code) })
            }
            PreconditionBody::ForeignCodeSha256 => {
                json!({ "CodeSha256": expected_code_sha256(&other_code) })
            }
            PreconditionBody::MatchingRevisionId => json!({ "RevisionId": revision_id }),
            PreconditionBody::ForeignRevisionId => {
                json!({ "RevisionId": "00000000-0000-4000-8000-000000000000" })
            }
        };

        let answer = publish_version(&router, case.name, &body).await;
        assert_eq!(
            answer.status,
            case.status,
            "case {}: expected {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        match case.error_code {
            Some(code) => assert!(
                answer.error_code().contains(code),
                "case {}: expected {code}, got {}",
                case.name,
                answer.error_code()
            ),
            None => {
                let published = answer.json();
                assert_eq!(
                    published["Version"],
                    json!("1"),
                    "case {}: expected Version 1, got {:?}",
                    case.name,
                    published["Version"]
                );
            }
        }
        // A refused publish persists nothing, and an accepted one persists exactly one
        // version.
        let expected_held = usize::from(case.status == 201);
        let held = published_version_count(&router, case.name).await;
        assert_eq!(
            held, expected_held,
            "case {}: expected {expected_held} published version(s) held, got {held}",
            case.name
        );
    }
}

/// Every documented `FunctionName` form names the one stored record, so publishing
/// through any of them publishes from the same unpublished configuration — and, the
/// configuration being unchanged, reports the same version.
#[tokio::test]
async fn every_documented_function_name_form_publishes_from_the_one_record() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "publish-me", &package_bytes(32)).await;

    // The four forms the model documents on `FunctionName`, plus the `$LATEST`
    // qualifier, which names the configuration this operation publishes from.
    let references = [
        "publish-me",
        "publish-me:$LATEST",
        "function:publish-me",
        "000000000000:function:publish-me",
        "arn:aws:lambda:us-east-1:000000000000:function:publish-me",
        "arn:aws:lambda:us-east-1:000000000000:function:publish-me:$LATEST",
    ];
    for reference in references {
        let answer = publish_version(&router, reference, &Value::Null).await;
        assert_eq!(
            answer.status,
            201,
            "case {reference}: expected 201, got {} {}",
            answer.status,
            answer.text()
        );
        let published = answer.json();
        assert_eq!(
            published["FunctionArn"],
            json!(format!("{}:1", function_arn("publish-me"))),
            "case {reference}: expected the one record's version 1 ARN, got {:?}",
            published["FunctionArn"]
        );
    }
    let held = published_version_count(&router, "publish-me").await;
    assert_eq!(
        held, 1,
        "expected the unchanged function to hold exactly 1 published version, got {held}"
    );
}

/// Publishing is bounded: a function that already holds the ceiling of published
/// versions is refused the modelled `CodeStorageExceededException` for a snapshot it
/// does not hold, while a snapshot it DOES hold is still reported — a bound on the store
/// the slice keeps, not on what it can answer.
///
/// The versions are written onto the stored record directly, because reaching the
/// ceiling through the wire would say nothing more than this does.
#[tokio::test]
async fn publishing_past_the_version_ceiling_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "crowded", &package_bytes(64)).await;

    // Fill the function to its ceiling. Each snapshot freezes its own description, so
    // none of them is the one the first publish below would freeze.
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let record = store
            .functions
            .get_mut("crowded")
            .expect("the created function must be stored");
        let created = record.config.last_modified.clone();
        // Each snapshot is written exactly as `publish_next_version` writes one: taken
        // FROM the revision the unpublished configuration is at, carrying a revision of
        // its own, and freezing the package the function holds.
        let source_revision_id = record.config.revision_id.clone();
        let code = record.code.clone();
        let ceiling = u64::try_from(MAX_FUNCTION_VERSIONS).unwrap_or_default();
        for number in 1..=ceiling {
            let mut config = record.config.clone();
            config.description = Some(format!("version {number}"));
            config.revision_id = mint_revision_id();
            record.versions.insert(
                number,
                FunctionVersionRecord {
                    version: number,
                    config,
                    source_revision_id: source_revision_id.clone(),
                    code: code.clone(),
                    created: created.clone(),
                },
            );
        }
        record.latest_version = ceiling;
    }

    let refused = publish_version(&router, "crowded", &json!({ "Description": "one more" })).await;
    assert_eq!(
        refused.status,
        400,
        "expected 400 past the version ceiling, got {} {}",
        refused.status,
        refused.text()
    );
    assert!(
        refused.error_code().contains("CodeStorageExceededException"),
        "expected CodeStorageExceededException past the version ceiling, got {}",
        refused.error_code()
    );

    // The snapshot the LAST version froze needs no room, so it is still reported.
    let last = MAX_FUNCTION_VERSIONS.to_string();
    let held = publish_version(
        &router,
        "crowded",
        &json!({ "Description": format!("version {last}") }),
    )
    .await;
    assert_eq!(
        held.status,
        201,
        "expected 201 republishing the snapshot the last version froze, got {} {}",
        held.status,
        held.text()
    );
    assert_eq!(
        held.json()["Version"],
        json!(last),
        "expected the version that already froze that snapshot ({last}), got {:?}",
        held.json()["Version"]
    );
}

/// One row of the refusal table: a request `PublishVersion` cannot serve, and the
/// answer it must give instead.
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

/// A request that names no publishable configuration is refused with the modelled
/// error, and the one member whose behaviour the model does not state is answered with
/// the scaffold's 501 rather than with invented behaviour.
#[tokio::test]
async fn publish_version_refuses_a_request_it_cannot_serve() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_zip_function(&router, "refusals", &package_bytes(48)).await;
    let first = publish_version(&router, "refusals", &Value::Null).await;
    assert_eq!(
        first.status,
        201,
        "expected 201 for the first publish, got {} {}",
        first.status,
        first.text()
    );

    let cases = [
        RefusalCase {
            name: "unknown-function",
            reference: "absent",
            body: Value::Null,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "unknown-function-by-arn",
            reference: "arn:aws:lambda:us-east-1:000000000000:function:absent",
            body: Value::Null,
            status: 404,
            error_code: "ResourceNotFoundException",
        },
        // "Creates a version from the current code and configuration of a function":
        // a published version is not a publishable configuration.
        RefusalCase {
            name: "qualified-by-version",
            reference: "refusals:1",
            body: Value::Null,
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "qualified-by-alias",
            reference: "refusals:prod",
            body: Value::Null,
            status: 400,
            error_code: "InvalidParameterValueException",
        },
        // `PublishTo` names a destination the model describes in one sentence and whose
        // label this operation's `Version` member cannot express, so it is answered as
        // not implemented rather than served as an ordinary publish.
        RefusalCase {
            name: "publish-to",
            reference: "refusals",
            body: json!({ "PublishTo": "LATEST_PUBLISHED" }),
            status: 501,
            error_code: "NotImplemented",
        },
    ];

    for case in &cases {
        let answer = publish_version(&router, case.reference, &case.body).await;
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

    // No refusal published anything: the one version from the accepted call is all the
    // function holds.
    let held = published_version_count(&router, "refusals").await;
    assert_eq!(
        held, 1,
        "expected the refused publishes to persist nothing, got {held} published version(s)"
    );
}

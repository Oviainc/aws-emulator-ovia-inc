// SPDX-License-Identifier: CC0-1.0

//! `GetFunctionConfiguration` end to end over HTTP.
//!
//! The rule under test: a read of a function's configuration answers the stored
//! `FunctionConfiguration` of the version the request resolves to — the same object
//! `GetFunction` reports under its `Configuration` member — for every documented form of
//! `FunctionName`, for a version named in the name or in the `Qualifier` parameter, and
//! for every package a function can be created from; a reference that names no function
//! or no such version is a 404, a name and a `Qualifier` that name two different
//! versions is a 400, and the read leaves what a later caller sees unchanged.
//!
//! Every row builds its function through the real `CreateFunction` over HTTP, so a
//! hardcoded answer fails. The clock-driven half — the `Pending` -> `Active` move the
//! model documents on `CreateFunction` — is exercised by moving the stored due instant
//! into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use sha2::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here. Nothing in it is verified: the harness turns request
/// authentication off, exactly as the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the `AUTHORIZATION` scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request and answer its status and body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
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
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answer = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, answer)
}

/// `GetFunctionConfiguration`
/// (`GET /2015-03-31/functions/{FunctionName}/configuration`).
async fn get_function_configuration(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> (u16, Value) {
    let uri = match qualifier {
        Some(qualifier) => {
            format!("{FUNCTIONS_URI}/{reference}/configuration?Qualifier={qualifier}")
        }
        None => format!("{FUNCTIONS_URI}/{reference}/configuration"),
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// `GetFunction` (`GET /2015-03-31/functions/{FunctionName}`), whose `Configuration`
/// member is the same `FunctionConfiguration` shape.
async fn get_function(
    router: &MultiServiceRouter,
    reference: &str,
    qualifier: Option<&str>,
) -> (u16, Value) {
    let uri = match qualifier {
        Some(qualifier) => format!("{FUNCTIONS_URI}/{reference}?Qualifier={qualifier}"),
        None => format!("{FUNCTIONS_URI}/{reference}"),
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// Deployment package bytes of a given length. Only the length and the digest of a
/// package are reported, and the content differs per length so two packages never share
/// a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// The base64 SHA-256 of `bytes` — the independent computation of what `CodeSha256`
/// must report, so the test never reads the value it is checking out of the response.
fn expected_code_sha256(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(bytes))
}

/// The unqualified ARN a function of `name` must carry.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
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

/// What the configuration of one created function must report. `Value::Null` means the
/// member must be absent: nothing is reported that was not stored.
struct Expected {
    runtime: Value,
    handler: Value,
    /// `Description` is a member of every `FunctionConfiguration`, so a function created
    /// without one reports it as the empty string rather than not at all.
    description: Value,
    code_size: i64,
    code_sha256: Value,
    timeout: i64,
    memory_size: i64,
    package_type: &'static str,
    architecture: &'static str,
    ephemeral_storage: i64,
    tracing_mode: &'static str,
    environment: Value,
}

/// One function to create, and the configuration every read of it must then report.
struct Fixture {
    name: &'static str,
    request: Value,
    expected: Expected,
}

/// The fixtures: a function per package form and per level of configuration, so the
/// table below reads back values that no default could have produced.
fn fixtures() -> Vec<Fixture> {
    let small = package_bytes(64);
    let large = package_bytes(5_000);
    vec![
        // An inline .zip archive with every optional member defaulted.
        Fixture {
            name: "defaults",
            request: json!({
                "FunctionName": "defaults",
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&small) },
            }),
            expected: Expected {
                runtime: json!("python3.12"),
                handler: json!("index.handler"),
                description: json!(""),
                code_size: i64::try_from(small.len()).unwrap_or_default(),
                code_sha256: json!(expected_code_sha256(&small)),
                timeout: 3,
                memory_size: 128,
                package_type: "Zip",
                architecture: "x86_64",
                ephemeral_storage: 512,
                tracing_mode: "PassThrough",
                environment: Value::Null,
            },
        },
        // Every optional member named, so none of the defaults can be what is reported.
        Fixture {
            name: "configured",
            request: json!({
                "FunctionName": "configured",
                "Role": ROLE,
                "Runtime": "nodejs20.x",
                "Handler": "app.main",
                "Description": "a configured function",
                "Timeout": 900,
                "MemorySize": 1024,
                "Architectures": ["arm64"],
                "EphemeralStorage": { "Size": 2048 },
                "TracingConfig": { "Mode": "Active" },
                "Environment": { "Variables": { "STAGE": "prod" } },
                "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&large) },
            }),
            expected: Expected {
                runtime: json!("nodejs20.x"),
                handler: json!("app.main"),
                description: json!("a configured function"),
                code_size: i64::try_from(large.len()).unwrap_or_default(),
                code_sha256: json!(expected_code_sha256(&large)),
                timeout: 900,
                memory_size: 1024,
                package_type: "Zip",
                architecture: "arm64",
                ephemeral_storage: 2048,
                tracing_mode: "Active",
                environment: json!({ "Variables": { "STAGE": "prod" } }),
            },
        },
        // `Publish` freezes version 1 "from its initial configuration", so this function
        // has a published version to qualify a read with as well as `$LATEST`.
        Fixture {
            name: "published",
            request: json!({
                "FunctionName": "published",
                "Role": ROLE,
                "Runtime": "java21",
                "Handler": "Handler::run",
                "MemorySize": 512,
                "Publish": true,
                "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&small) },
            }),
            expected: Expected {
                runtime: json!("java21"),
                handler: json!("Handler::run"),
                description: json!(""),
                code_size: i64::try_from(small.len()).unwrap_or_default(),
                code_sha256: json!(expected_code_sha256(&small)),
                timeout: 3,
                memory_size: 512,
                package_type: "Zip",
                architecture: "x86_64",
                ephemeral_storage: 512,
                tracing_mode: "PassThrough",
                environment: Value::Null,
            },
        },
        // An S3-sourced archive: this emulator holds no bytes for it, so it reports no
        // size it did not measure and no digest it did not compute.
        Fixture {
            name: "from-s3",
            request: json!({
                "FunctionName": "from-s3",
                "Role": ROLE,
                "Runtime": "ruby3.3",
                "Handler": "app.handler",
                "Code": { "S3Bucket": "deployments", "S3Key": "fn.zip" },
            }),
            expected: Expected {
                runtime: json!("ruby3.3"),
                handler: json!("app.handler"),
                description: json!(""),
                code_size: 0,
                code_sha256: Value::Null,
                timeout: 3,
                memory_size: 128,
                package_type: "Zip",
                architecture: "x86_64",
                ephemeral_storage: 512,
                tracing_mode: "PassThrough",
                environment: Value::Null,
            },
        },
        // A container image package: no runtime and no handler, per the model's own
        // "You do not need to specify the handler and runtime properties".
        Fixture {
            name: "from-image",
            request: json!({
                "FunctionName": "from-image",
                "Role": ROLE,
                "PackageType": "Image",
                "Code": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest" },
            }),
            expected: Expected {
                runtime: Value::Null,
                handler: Value::Null,
                description: json!(""),
                code_size: 0,
                // An `Image` package's `CodeSha256` is the digest of the image the
                // function is frozen to, written as the hexadecimal an OCI digest is
                // written in — computed here rather than read out of the answer.
                code_sha256: json!(sha2::Sha256::digest(
                    "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest".as_bytes()
                )
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()),
                timeout: 3,
                memory_size: 128,
                package_type: "Image",
                architecture: "x86_64",
                ephemeral_storage: 512,
                tracing_mode: "PassThrough",
                environment: Value::Null,
            },
        },
    ]
}

/// One row of the read table: how the function is referred to, and which function and
/// version that reference must resolve to.
struct ReadCase {
    label: &'static str,
    reference: String,
    qualifier: Option<&'static str>,
    function: &'static str,
    version: &'static str,
}

/// The reference forms the model documents for `FunctionName` ("Function name –
/// my-function (name-only), my-function:v1 (with alias). Function ARN – arn:...:function:
/// my-function. Partial ARN – 123456789012:function:my-function. You can append a version
/// number or alias to any of the formats."), crossed with the two places a version can be
/// named, over the fixtures above.
fn read_cases() -> Vec<ReadCase> {
    vec![
        ReadCase {
            label: "bare name",
            reference: "defaults".to_string(),
            qualifier: None,
            function: "defaults",
            version: "$LATEST",
        },
        ReadCase {
            label: "name with the $LATEST qualifier",
            reference: "defaults:$LATEST".to_string(),
            qualifier: None,
            function: "defaults",
            version: "$LATEST",
        },
        ReadCase {
            label: "name with $LATEST in the Qualifier parameter",
            reference: "defaults".to_string(),
            qualifier: Some("$LATEST"),
            function: "defaults",
            version: "$LATEST",
        },
        ReadCase {
            label: "the bare function: prefix",
            reference: "function:configured".to_string(),
            qualifier: None,
            function: "configured",
            version: "$LATEST",
        },
        ReadCase {
            label: "partial ARN",
            reference: format!("{ACCOUNT}:function:configured"),
            qualifier: None,
            function: "configured",
            version: "$LATEST",
        },
        ReadCase {
            label: "full ARN",
            reference: function_arn("configured"),
            qualifier: None,
            function: "configured",
            version: "$LATEST",
        },
        ReadCase {
            label: "full ARN carrying the version",
            reference: format!("{}:1", function_arn("published")),
            qualifier: None,
            function: "published",
            version: "1",
        },
        ReadCase {
            label: "a published version named in the name",
            reference: "published:1".to_string(),
            qualifier: None,
            function: "published",
            version: "1",
        },
        ReadCase {
            label: "a published version named in the Qualifier parameter",
            reference: "published".to_string(),
            qualifier: Some("1"),
            function: "published",
            version: "1",
        },
        ReadCase {
            label: "the unpublished version of a function that has a published one",
            reference: "published".to_string(),
            qualifier: None,
            function: "published",
            version: "$LATEST",
        },
        ReadCase {
            label: "an S3-sourced package",
            reference: "from-s3".to_string(),
            qualifier: None,
            function: "from-s3",
            version: "$LATEST",
        },
        ReadCase {
            label: "a container-image package",
            reference: "from-image".to_string(),
            qualifier: None,
            function: "from-image",
            version: "$LATEST",
        },
    ]
}

/// Assert one member of the answered configuration, naming the case and the value.
fn assert_member(label: &str, body: &Value, pointer: &str, expected: &Value) {
    let actual = body.pointer(pointer).cloned().unwrap_or(Value::Null);
    assert_eq!(
        &actual, expected,
        "case {label:?}: expected {pointer} {expected}, got {actual}"
    );
}

#[tokio::test]
async fn get_function_configuration_reports_the_stored_configuration_of_the_resolved_version() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let fixtures = fixtures();

    for fixture in &fixtures {
        let (status, body) = call(&router, "POST", FUNCTIONS_URI, &fixture.request).await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateFunction must succeed, got {status} with {body}",
            fixture.name
        );
    }
    // Every function has finished provisioning, so each one reports the settled state.
    make_activation_due(&engine);

    for case in read_cases() {
        let fixture = fixtures
            .iter()
            .find(|fixture| fixture.name == case.function)
            .unwrap_or_else(|| panic!("case {:?}: names no fixture", case.label));
        let expected = &fixture.expected;

        let (status, body) =
            get_function_configuration(&router, &case.reference, case.qualifier).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.label
        );

        let expected_arn = if case.version == "$LATEST" {
            function_arn(case.function)
        } else {
            format!("{}:{}", function_arn(case.function), case.version)
        };
        assert_member(case.label, &body, "/FunctionName", &json!(case.function));
        assert_member(case.label, &body, "/FunctionArn", &json!(expected_arn));
        assert_member(case.label, &body, "/Version", &json!(case.version));
        assert_member(case.label, &body, "/Role", &json!(ROLE));
        assert_member(case.label, &body, "/Runtime", &expected.runtime);
        assert_member(case.label, &body, "/Handler", &expected.handler);
        assert_member(case.label, &body, "/Description", &expected.description);
        assert_member(case.label, &body, "/CodeSize", &json!(expected.code_size));
        assert_member(case.label, &body, "/CodeSha256", &expected.code_sha256);
        assert_member(case.label, &body, "/Timeout", &json!(expected.timeout));
        assert_member(
            case.label,
            &body,
            "/MemorySize",
            &json!(expected.memory_size),
        );
        assert_member(
            case.label,
            &body,
            "/PackageType",
            &json!(expected.package_type),
        );
        assert_member(
            case.label,
            &body,
            "/Architectures",
            &json!([expected.architecture]),
        );
        assert_member(
            case.label,
            &body,
            "/EphemeralStorage/Size",
            &json!(expected.ephemeral_storage),
        );
        assert_member(
            case.label,
            &body,
            "/TracingConfig/Mode",
            &json!(expected.tracing_mode),
        );
        assert_member(case.label, &body, "/Environment", &expected.environment);

        // Provisioning is done, so the lifecycle members report the settled function the
        // model describes ("This is first set to Successful after function creation
        // completes"), with no reason left to report.
        assert_member(case.label, &body, "/State", &json!("Active"));
        assert_member(case.label, &body, "/LastUpdateStatus", &json!("Successful"));
        assert_member(case.label, &body, "/StateReason", &Value::Null);
        assert_member(case.label, &body, "/StateReasonCode", &Value::Null);

        // `RevisionId` and `LastModified` are stored per configuration, so they are
        // reported rather than invented: non-empty, and the same on a second read.
        for pointer in ["/RevisionId", "/LastModified"] {
            let value = body.pointer(pointer).and_then(Value::as_str);
            assert!(
                value.is_some_and(|value| !value.is_empty()),
                "case {:?}: expected a stored {pointer}, got {:?}",
                case.label,
                body.pointer(pointer)
            );
        }

        // The same `FunctionConfiguration` shape `GetFunction` reports for the same
        // reference: one operation reports the version-specific settings on their own,
        // the other alongside the code location and the tags.
        let (get_status, get_body) =
            get_function(&router, &case.reference, case.qualifier).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: GetFunction must succeed too, got {get_status} with {get_body}",
            case.label
        );
        let from_get_function = get_body
            .pointer("/Configuration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            body, from_get_function,
            "case {:?}: expected the configuration GetFunction reports {from_get_function}, \
             got {body}",
            case.label
        );

        // A read must not change what a later caller sees.
        let (repeat_status, repeat_body) =
            get_function_configuration(&router, &case.reference, case.qualifier).await;
        assert_eq!(
            (repeat_status, &repeat_body),
            (status, &body),
            "case {:?}: reading the configuration changed the answer: {repeat_status} with \
             {repeat_body}",
            case.label
        );
    }

    // Nor does any of those reads add, drop or rename a stored function.
    let held: Vec<String> = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .functions
        .keys()
        .cloned()
        .collect();
    let mut expected_names: Vec<String> =
        fixtures.iter().map(|fixture| fixture.name.to_string()).collect();
    expected_names.sort();
    assert_eq!(
        held, expected_names,
        "reading configurations changed the store, it now holds {held:?}"
    );
}

/// One row of the refusal table: a reference, and the status and error code it must be
/// answered with.
struct RefusalCase {
    label: &'static str,
    reference: String,
    qualifier: Option<&'static str>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn get_function_configuration_refuses_what_it_cannot_resolve() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let package = package_bytes(32);
    let (status, body) = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "resolve-me",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction must succeed, got {status} with {body}"
    );

    let cases = vec![
        RefusalCase {
            label: "a function that was never created",
            reference: "absent".to_string(),
            qualifier: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a full ARN naming a function that was never created",
            reference: function_arn("absent"),
            qualifier: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a version this function never published",
            reference: "resolve-me:9".to_string(),
            qualifier: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a version named in the Qualifier parameter that was never published",
            reference: "resolve-me".to_string(),
            qualifier: Some("9"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an alias that was never created",
            reference: "resolve-me".to_string(),
            qualifier: Some("prod"),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a name and a Qualifier naming two different versions",
            reference: "resolve-me:1".to_string(),
            qualifier: Some("$LATEST"),
            status: 400,
            code: "InvalidParameterValueException",
        },
    ];

    for case in cases {
        let (status, body) =
            get_function_configuration(&router, &case.reference, case.qualifier).await;
        assert_eq!(
            status, case.status,
            "case {:?}: expected HTTP {}, got {status} with {body}",
            case.label, case.status
        );
        let reported = body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            reported.contains(case.code),
            "case {:?}: expected {}, got {reported:?} in {body}",
            case.label,
            case.code
        );
    }

    // The version that WAS published still reads back, so none of the refusals above
    // removed anything.
    let (status, body) = get_function_configuration(&router, "resolve-me", Some("1")).await;
    assert_eq!(
        status, 200,
        "the published version must still read back, got {status} with {body}"
    );
}

#[tokio::test]
async fn get_function_configuration_reports_the_state_the_clock_has_made_due() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let package = package_bytes(128);
    let (status, body) = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "provisioning",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction must succeed, got {status} with {body}"
    );

    // Before the activation is due, the function is still being provisioned: the model's
    // `StateReasonCode` documents "When the code is Creating, you can't invoke or modify
    // the function", and `LastUpdateStatus` "is first set to Successful after function
    // creation completes".
    let (status, pending) = get_function_configuration(&router, "provisioning", None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 while provisioning, got {status} with {pending}"
    );
    assert_member("while provisioning", &pending, "/State", &json!("Pending"));
    assert_member(
        "while provisioning",
        &pending,
        "/StateReasonCode",
        &json!("Creating"),
    );
    assert_member(
        "while provisioning",
        &pending,
        "/LastUpdateStatus",
        &Value::Null,
    );

    // Reading it again does not advance anything: the transition is owed by the clock.
    let (_, again) = get_function_configuration(&router, "provisioning", None).await;
    assert_eq!(
        again, pending,
        "case {:?}: a read moved the function's state on its own, got {again}",
        "read twice while provisioning"
    );

    make_activation_due(&engine);
    let (status, active) = get_function_configuration(&router, "provisioning", None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 once active, got {status} with {active}"
    );
    assert_member("once active", &active, "/State", &json!("Active"));
    assert_member("once active", &active, "/StateReason", &Value::Null);
    assert_member("once active", &active, "/StateReasonCode", &Value::Null);
    assert_member(
        "once active",
        &active,
        "/LastUpdateStatus",
        &json!("Successful"),
    );
    // The configuration itself did not change with the lifecycle move: the same stored
    // revision is reported.
    assert_eq!(
        active.get("RevisionId"),
        pending.get("RevisionId"),
        "case {:?}: the activation minted a new revision: {active}",
        "once active"
    );
}

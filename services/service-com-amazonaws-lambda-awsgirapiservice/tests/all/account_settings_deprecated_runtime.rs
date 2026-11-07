// SPDX-License-Identifier: CC0-1.0

//! `GetAccountSettings` reports whether this account still holds a function on a runtime
//! Lambda no longer supports.
//!
//! The rule under test: the answer carries `HasFunctionWithDeprecatedRuntime` in every
//! response, as a boolean computed from the functions this service persists — true when
//! any of them is configured with a runtime identifier past its deprecation, false when
//! none is, including on an account holding no function at all. A function whose
//! deployment package is a container image names no runtime, so it is not one.
//!
//! The rows differ along what the rule generalises over: how many functions the account
//! holds, which runtime family each names (one row per family, picked from the runtime
//! identifiers the model's `Runtime` enum declares and the Lambda runtime-deprecation
//! policy lists as deprecated), whether the deprecated runtime is the only one or one
//! among supported ones, and whether the function names a runtime at all.
//!
//! Every row builds its account through the real `CreateFunction` over HTTP and reads the
//! flag back through `GetAccountSettings`, so a hardcoded answer fails. Creating a
//! function ON a deprecated runtime is what this emulator admits today — whether a create
//! is blocked "shortly after each runtime is deprecated" is a separate rule, filed
//! nowhere here — and it is the only way an account can come to hold such a function.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `GetAccountSettings` is bound (`GET /2016-08-19/account-settings`).
const ACCOUNT_SETTINGS_URI: &str = "/2016-08-19/account-settings";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The member under test.
const FLAG: &str = "HasFunctionWithDeprecatedRuntime";

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here. Nothing in it is verified: the harness turns request
/// authentication off, exactly as the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A container image URI, for the row whose function names no runtime.
const IMAGE_URI: &str = "000000000000.dkr.ecr.us-east-1.amazonaws.com/fn:latest";

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

/// Create a function on `runtime`, or a container-image function when it names none.
async fn create_function(router: &MultiServiceRouter, name: &str, runtime: Option<&str>) {
    let package: Vec<u8> = (0..32u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let body = match runtime {
        Some(runtime) => json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
        None => json!({
            "FunctionName": name,
            "Role": ROLE,
            "PackageType": "Image",
            "Code": { "ImageUri": IMAGE_URI },
        }),
    };
    let (status, answered) = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        status, 201,
        "CreateFunction {name:?} on {runtime:?} must succeed, got {status} with {answered}"
    );
}

/// The flag one `GetAccountSettings` answer carries, read out of the object so a MISSING
/// member fails rather than reading as false.
async fn reported_flag(case: &str, router: &MultiServiceRouter) -> bool {
    let (status, body) = call(router, "GET", ACCOUNT_SETTINGS_URI, &Value::Null).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    let member = body
        .as_object()
        .and_then(|object| object.get(FLAG))
        .unwrap_or_else(|| panic!("case {case:?}: the answer must carry {FLAG}, got {body}"));
    member
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: {FLAG} must be a boolean, got {member}"))
}

/// Bring the activation the clock owes every stored function forward, the way waiting out
/// the create would: a function still `Pending` refuses a configuration update, and what
/// is under test here is which runtimes the account reports rather than when a function
/// becomes updatable.
fn activate(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// One row: the functions to create (each with the runtime it names, or none for a
/// container image) and the flag the account must then report.
struct Case {
    name: &'static str,
    /// `(function name, runtime)`; a `None` runtime is a container-image function.
    functions: &'static [(&'static str, Option<&'static str>)],
    expected: bool,
}

/// The rows. The deprecated identifiers are entries of the Lambda runtime-deprecation
/// policy's deprecated-runtime list, one per language family, and the supported ones are
/// `Runtime` values that list does not hold.
const CASES: &[Case] = &[
    Case {
        name: "an account holding no function at all",
        functions: &[],
        expected: false,
    },
    Case {
        name: "one function on a supported runtime",
        functions: &[("settings-python312", Some("python3.12"))],
        expected: false,
    },
    Case {
        name: "several functions, all on supported runtimes",
        functions: &[
            ("settings-python311", Some("python3.11")),
            ("settings-nodejs20", Some("nodejs20.x")),
            ("settings-java21", Some("java21")),
            ("settings-provided-al2023", Some("provided.al2023")),
        ],
        expected: false,
    },
    Case {
        name: "one function on a deprecated python runtime",
        functions: &[("settings-python38", Some("python3.8"))],
        expected: true,
    },
    Case {
        name: "one function on a deprecated nodejs runtime",
        functions: &[("settings-nodejs18", Some("nodejs18.x"))],
        expected: true,
    },
    Case {
        name: "one function on a deprecated ruby runtime",
        functions: &[("settings-ruby27", Some("ruby2.7"))],
        expected: true,
    },
    Case {
        name: "one function on a deprecated dotnet runtime",
        functions: &[("settings-dotnetcore31", Some("dotnetcore3.1"))],
        expected: true,
    },
    Case {
        name: "one function on a deprecated go runtime",
        functions: &[("settings-go1x", Some("go1.x"))],
        expected: true,
    },
    Case {
        name: "one function on the deprecated Amazon Linux 1 custom runtime",
        functions: &[("settings-provided", Some("provided"))],
        expected: true,
    },
    Case {
        name: "one deprecated runtime among several supported ones",
        functions: &[
            ("settings-mixed-supported", Some("python3.13")),
            ("settings-mixed-deprecated", Some("java8")),
            ("settings-mixed-other", Some("nodejs22.x")),
        ],
        expected: true,
    },
    Case {
        name: "a container-image function, which names no runtime",
        functions: &[("settings-image", None)],
        expected: false,
    },
    Case {
        name: "a container-image function beside a deprecated one",
        functions: &[
            ("settings-image-pair", None),
            ("settings-image-pair-old", Some("python2.7")),
        ],
        expected: true,
    },
];

#[tokio::test]
async fn the_account_reports_whether_a_function_runs_a_deprecated_runtime() {
    for case in CASES {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let label = case.name;
        // A container-image row resolves its reference against the registry at create, so
        // the image it names is pushed there first.
        crate::function_lifecycle::seed_image(&engine, IMAGE_URI).await;

        // Before anything is created the flag is already reported, and false: an account
        // with no function has none on a deprecated runtime.
        let empty = reported_flag(label, &router).await;
        assert!(
            !empty,
            "case {label:?}: an empty account must report {FLAG} false, got {empty:?}"
        );

        for (name, runtime) in case.functions {
            create_function(&router, name, *runtime).await;
        }

        let actual = reported_flag(label, &router).await;
        assert_eq!(
            actual, case.expected,
            "case {label:?}: expected {FLAG} {}, got {actual:?}",
            case.expected
        );

        // A report must not change what a later caller sees.
        let again = reported_flag(label, &router).await;
        assert_eq!(
            again, case.expected,
            "case {label:?}: a second read must report {FLAG} {} unchanged, got {again:?}",
            case.expected
        );
    }
}

/// The flag follows the functions: a published version keeps the runtime it froze, so
/// moving the unpublished configuration onto a supported runtime does not clear it, and
/// deleting the function does.
#[tokio::test]
async fn the_flag_follows_every_configuration_the_account_still_holds() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "settings-upgraded", Some("python3.7")).await;
    activate(&engine);
    assert!(
        reported_flag("created on a deprecated runtime", &router).await,
        "a function created on a deprecated runtime must set {FLAG}"
    );

    let (published, body) = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/settings-upgraded/versions"),
        &json!({}),
    )
    .await;
    assert_eq!(
        published, 201,
        "PublishVersion must succeed, got {published} with {body}"
    );

    let (updated, body) = call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/settings-upgraded/configuration"),
        &json!({ "Runtime": "python3.13" }),
    )
    .await;
    assert_eq!(
        updated, 200,
        "UpdateFunctionConfiguration must succeed, got {updated} with {body}"
    );
    assert!(
        reported_flag("the published version still runs the old runtime", &router).await,
        "a published version keeps the runtime it froze, so {FLAG} must stay set"
    );

    let (deleted, body) = call(
        &router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/settings-upgraded"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted, 204,
        "DeleteFunction must succeed, got {deleted} with {body}"
    );
    assert!(
        !reported_flag("the function is gone", &router).await,
        "an account holding no function at all must report {FLAG} false"
    );
}

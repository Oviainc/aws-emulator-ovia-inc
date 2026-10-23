// SPDX-License-Identifier: CC0-1.0

//! `GetAccountSettings` reports the account's limits and usage, end to end over HTTP.
//!
//! The rule under test: the answer is computed from what this service persists.
//! `AccountUsage` counts the functions the store holds and the storage their deployment
//! packages use, and `AccountLimit$UnreservedConcurrentExecutions` is the account's
//! concurrency limit less the concurrency those functions reserve (the member is
//! modelled as "The maximum number of simultaneous function executions, minus the
//! capacity that's reserved for individual functions with PutFunctionConcurrency", and
//! `@range(min: 0)`). The quota members are the published Lambda defaults, so they are
//! the same on every row.
//!
//! Every row builds its account through the real `CreateFunction` over HTTP and reads it
//! back through `GetAccountSettings`, so a hardcoded answer fails. Reserved concurrency
//! is written onto the stored record directly because `PutFunctionConcurrency` is still
//! a stub (its own issue); the field is the one that handler will set.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, CONCURRENT_EXECUTIONS_LIMIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `GetAccountSettings` is bound (`GET /2016-08-19/account-settings`).
const ACCOUNT_SETTINGS_URI: &str = "/2016-08-19/account-settings";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here. Nothing in it is verified: the harness turns request
/// authentication off, exactly as the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// One function to build for a case: its name, the length of its deployment package, and
/// the concurrency reserved for it.
struct Function {
    name: &'static str,
    package_len: usize,
    reserved: Option<i32>,
}

/// One case: the account to build, and the usage and unreserved concurrency it must then
/// report.
struct Case {
    name: &'static str,
    functions: &'static [Function],
    expected_function_count: i64,
    expected_total_code_size: i64,
    expected_unreserved: i32,
}

const CASES: &[Case] = &[
    Case {
        name: "an empty account",
        functions: &[],
        expected_function_count: 0,
        expected_total_code_size: 0,
        expected_unreserved: CONCURRENT_EXECUTIONS_LIMIT,
    },
    Case {
        name: "one function, nothing reserved",
        functions: &[Function {
            name: "alpha",
            package_len: 1_024,
            reserved: None,
        }],
        expected_function_count: 1,
        expected_total_code_size: 1_024,
        expected_unreserved: CONCURRENT_EXECUTIONS_LIMIT,
    },
    Case {
        name: "three functions of different sizes, one reserving concurrency",
        functions: &[
            Function {
                name: "alpha",
                package_len: 1_024,
                reserved: None,
            },
            Function {
                name: "beta",
                package_len: 20_480,
                reserved: Some(100),
            },
            Function {
                name: "gamma",
                package_len: 7,
                reserved: None,
            },
        ],
        expected_function_count: 3,
        expected_total_code_size: 21_511,
        expected_unreserved: CONCURRENT_EXECUTIONS_LIMIT - 100,
    },
    Case {
        name: "two functions reserving the whole account limit between them",
        functions: &[
            Function {
                name: "delta",
                package_len: 10,
                reserved: Some(400),
            },
            Function {
                name: "epsilon",
                package_len: 20,
                reserved: Some(600),
            },
        ],
        expected_function_count: 2,
        expected_total_code_size: 30,
        expected_unreserved: 0,
    },
    Case {
        name: "more reserved than the account limit floors the unreserved capacity at zero",
        functions: &[Function {
            name: "zeta",
            package_len: 64,
            reserved: Some(CONCURRENT_EXECUTIONS_LIMIT + 500),
        }],
        expected_function_count: 1,
        expected_total_code_size: 64,
        expected_unreserved: 0,
    },
];

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Deployment package bytes of a given length. Only the length and the digest of a
/// package are reported, and the content differs per length so two packages never share
/// a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
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

/// `GetAccountSettings`.
async fn get_account_settings(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GET", ACCOUNT_SETTINGS_URI, &Value::Null).await
}

/// The integer at `pointer`, or a panic naming the case.
fn number(case: &str, body: &Value, pointer: &str) -> i64 {
    body.pointer(pointer)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} is missing or not an integer: {body}"))
}

#[tokio::test]
async fn account_settings_report_the_stored_functions_and_their_reserved_concurrency() {
    for case in CASES {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);

        for function in case.functions {
            let package = package_bytes(function.package_len);
            let (status, body) = call(
                &router,
                "POST",
                FUNCTIONS_URI,
                &json!({
                    "FunctionName": function.name,
                    "Role": ROLE,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package),
                    },
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {:?}: CreateFunction {:?} must succeed, got {status} with {body}",
                case.name, function.name
            );

            if let Some(reserved) = function.reserved {
                // Written straight onto the record `PutFunctionConcurrency` writes:
                // these rows reserve more than that operation's documented
                // minimum-unreserved rule admits, on purpose, because what is under
                // test here is how `GetAccountSettings` reports a store holding them
                // (including the `@range(min: 0)` floor) rather than how a reservation
                // is accepted. `tests/all/function_concurrency.rs` covers the
                // operation itself end to end.
                let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
                let mut store = state.lock();
                let stored = store
                    .functions
                    .get_mut(function.name)
                    .unwrap_or_else(|| panic!("case {:?}: CreateFunction stored no record for {:?}", case.name, function.name));
                stored.reserved_concurrent_executions = Some(reserved);
            }
        }

        let (status, body) = get_account_settings(&router).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            case.name
        );

        let actual_count = number(case.name, &body, "/AccountUsage/FunctionCount");
        assert_eq!(
            actual_count, case.expected_function_count,
            "case {:?}: expected FunctionCount {}, got {actual_count:?}",
            case.name, case.expected_function_count
        );
        let actual_usage = number(case.name, &body, "/AccountUsage/TotalCodeSize");
        assert_eq!(
            actual_usage, case.expected_total_code_size,
            "case {:?}: expected AccountUsage.TotalCodeSize {}, got {actual_usage:?}",
            case.name, case.expected_total_code_size
        );
        let actual_unreserved = number(
            case.name,
            &body,
            "/AccountLimit/UnreservedConcurrentExecutions",
        );
        assert_eq!(
            actual_unreserved,
            i64::from(case.expected_unreserved),
            "case {:?}: expected UnreservedConcurrentExecutions {}, got {actual_unreserved:?}",
            case.name,
            case.expected_unreserved
        );

        // The published Lambda quotas, identical on every row: they are the account's
        // limits, not a function of its usage. Written as the published byte values
        // (AWS Lambda quotas: 75 GB function and layer storage, 250 MB unzipped, 50 MB
        // zipped for a direct upload, 1,000 concurrent executions) rather than as the
        // crate's own constants, so a wrong constant fails here.
        for (pointer, expected) in [
            ("/AccountLimit/TotalCodeSize", 80_530_636_800i64),
            ("/AccountLimit/CodeSizeUnzipped", 262_144_000),
            ("/AccountLimit/CodeSizeZipped", 52_428_800),
            ("/AccountLimit/ConcurrentExecutions", 1_000),
        ] {
            let actual = number(case.name, &body, pointer);
            assert_eq!(
                actual, expected,
                "case {:?}: expected {pointer} {expected}, got {actual:?}",
                case.name
            );
        }

        // A report must not change what a later caller sees: the second call answers
        // exactly the same, and the store still holds the same functions.
        let (repeat_status, repeat_body) = get_account_settings(&router).await;
        assert_eq!(
            (repeat_status, &repeat_body),
            (status, &body),
            "case {:?}: reading the account settings changed the answer: \
             {repeat_status} with {repeat_body}",
            case.name
        );
        let held = engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
            .lock()
            .functions
            .len();
        assert_eq!(
            held,
            case.functions.len(),
            "case {:?}: reading the account settings changed the store, it now holds {held:?}",
            case.name
        );
    }
}

/// The usage a function contributes stops counting when the function is deleted, and the
/// clock-owed `Pending` -> `Active` move does not change what is reported: the usage
/// follows the records, not the calls.
#[tokio::test]
async fn account_usage_follows_the_functions_as_they_come_and_go() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let package = package_bytes(512);

    let (status, body) = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": "transient",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction must succeed, got {status} with {body}"
    );

    let (_, created) = get_account_settings(&router).await;
    assert_eq!(
        number("while pending", &created, "/AccountUsage/TotalCodeSize"),
        512,
        "case {:?}: expected the new function's 512 bytes to be in use, got {created}",
        "while pending"
    );

    // Bring the activation the clock owes forward, the way waiting would.
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for function in store.functions.values_mut() {
            function.active_due = past;
        }
    }
    let (_, active) = get_account_settings(&router).await;
    assert_eq!(
        active, created,
        "case {:?}: the account settings must not depend on a function's state, got {active}",
        "once active"
    );

    let (delete_status, delete_body) = call(
        &router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/transient"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        delete_status, 204,
        "DeleteFunction must succeed, got {delete_status} with {delete_body}"
    );

    let (_, deleted) = get_account_settings(&router).await;
    assert_eq!(
        number("after the delete", &deleted, "/AccountUsage/FunctionCount"),
        0,
        "case {:?}: expected no functions left, got {deleted}",
        "after the delete"
    );
    assert_eq!(
        number("after the delete", &deleted, "/AccountUsage/TotalCodeSize"),
        0,
        "case {:?}: expected no storage in use, got {deleted}",
        "after the delete"
    );
}

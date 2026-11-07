// SPDX-License-Identifier: CC0-1.0

//! End-to-end test for the INITIALIZATION TYPE a `nodejs*` handler reads out of its own
//! execution environment: `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) of a function with and without a ready
//! provisioned concurrency allocation of the qualifier the call addresses.
//!
//! The rule under test: "AWS_LAMBDA_INITIALIZATION_TYPE – The initialization type of the
//! execution environment" is `provisioned-concurrency` for an invocation served by an
//! environment Lambda keeps INITIALISED — one a ready allocation of the addressed qualifier
//! holds — and `on-demand` for every other invocation. The allocation becomes ready on the
//! clock, so what the variable reports moves with the allocation's own status rather than
//! with anything the caller says.
//!
//! The rows differ along what the rule generalises over: which qualifier the invocation
//! addresses (an alias, the version it resolves to, and the unqualified function) and
//! whether the allocation addressed is ready yet.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let document = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (parts.status.as_u16(), document)
}

/// A .zip deployment package holding one handler that reports its own initialization type.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer.start_file("index.js", options).expect("the entry");
        writer
            .write_all(
                b"exports.handler = async () => ({\n\
                      initType: process.env.AWS_LAMBDA_INITIALIZATION_TYPE,\n\
                  });\n",
            )
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The initialization type a handler reads is the one of the environment serving it: a ready
/// provisioned allocation of the addressed qualifier makes it `provisioned-concurrency`, and
/// everything else reads `on-demand`.
#[tokio::test]
async fn the_initialization_type_is_the_serving_environments_own() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let (created, _) = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": "init-type",
            "Role": ROLE,
            "Runtime": "nodejs22.x",
            "Handler": "index.handler",
            "Timeout": 10,
            "MemorySize": 256,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()) },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(created, 201, "the function under test must be created");
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    // Before any allocation exists, every invocation is served by an environment Lambda
    // started for it.
    let (status, body) = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/init-type/invocations"),
        b"{}",
    )
    .await;
    assert_eq!(status, 200, "the invocation must answer 200, got {body}");
    assert_eq!(
        body.get("initType").and_then(Value::as_str),
        Some("on-demand"),
        "case no allocation: expected on-demand, got {body}"
    );

    // A published version, an alias on it, and a provisioned concurrency allocation of that
    // alias — which is READY once the clock has made its warm-up due.
    let (published, version) = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/init-type/versions"),
        b"{}",
    )
    .await;
    assert_eq!(published, 201, "the version must publish, got {version}");
    let version = version
        .get("Version")
        .and_then(Value::as_str)
        .expect("the published version")
        .to_string();
    let (alias, _) = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/init-type/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": version }))
            .expect("the alias body serializes"),
    )
    .await;
    assert_eq!(alias, 201, "the alias must be created");
    let (allocated, allocation) = call(
        &router,
        "PUT",
        "/2019-09-30/functions/init-type/provisioned-concurrency?Qualifier=live",
        &serde_json::to_vec(&json!({ "ProvisionedConcurrentExecutions": 1 }))
            .expect("the allocation body serializes"),
    )
    .await;
    assert_eq!(
        allocated, 202,
        "the allocation must be accepted, got {allocation}"
    );
    // The allocation is ready once the warm-up the clock owes has passed; nothing here
    // sleeps on the emulator's behalf beyond that span.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let cases: &[(&str, String, &str)] = &[
        (
            "the alias the allocation holds",
            "init-type/invocations?Qualifier=live".to_string(),
            "provisioned-concurrency",
        ),
        (
            "the version the alias resolves to, which the allocation does not hold",
            format!("init-type/invocations?Qualifier={version}"),
            "on-demand",
        ),
        (
            "the unqualified function",
            "init-type/invocations".to_string(),
            "on-demand",
        ),
    ];
    for (case, uri, expected) in cases {
        let (status, body) = call(&router, "POST", &format!("{FUNCTIONS_URI}/{uri}"), b"{}").await;
        assert_eq!(
            status, 200,
            "case {case}: the invocation must answer 200, got {body}"
        );
        assert_eq!(
            body.get("initType").and_then(Value::as_str),
            Some(*expected),
            "case {case}: expected {expected}, got {body}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONCURRENCY an invocation takes: what `Invoke` does when the
//! function's reserved concurrency is full, and what the run is told about the environment
//! serving it.
//!
//! The rules under test:
//!
//! * "Lambda returns `TooManyRequestsException` if running the function would cause you to
//!   exceed a concurrency limit at either the account level […] or function level
//!   (`ReservedFunctionConcurrentInvocationLimitExceeded`)" (`Invoke`), where that
//!   function-level limit is what `PutFunctionConcurrency` reserves — "the maximum number of
//!   simultaneous executions for a function" — and "concurrency settings apply to the
//!   function as a whole, including all published versions and the unpublished version", so
//!   every qualifier of one function draws on ONE pool.
//! * An admitted invocation holds its capacity only while it is in flight: the next caller is
//!   admitted once a run has ended, however it ended.
//! * Provisioned concurrency "initializes a requested number of execution environments so
//!   that they are prepared to respond immediately to your function's invocations"
//!   (`PutProvisionedConcurrencyConfig`), and an invocation one of those environments serves
//!   reports `AWS_LAMBDA_INITIALIZATION_TYPE=provisioned-concurrency` to the runtime; every
//!   other invocation reports `on-demand`.
//!
//! The rows differ along what the rules generalise over: the qualifier the invocation names,
//! the reservation in force, how the invocation that holds the pool ended, and whether the
//! qualifier the call addressed has a ready allocation.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ACTIVATION_DELAY, PROVISIONED_CONCURRENCY_ALLOCATION_DELAY,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module every package below holds the handler in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `Reason` a throttle that hit the function's own reservation names, one of the model's
/// `ThrottleReason` values (`smithy_query(query="shape", shape="ThrottleReason")`).
const RESERVED_REASON: &str = "ReservedFunctionConcurrentInvocationLimitExceeded";

/// How long past a due instant this suite waits before calling. Every transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long the handler that HOLDS the pool runs for, while a second caller is admitted or
/// throttled beside it. Short, because the run slots this emulator holds are shared by every
/// invocation in the process.
const HELD_SECONDS: f64 = 0.4;

/// How long after the holding invocation starts the second caller arrives.
const PROBE_AFTER: Duration = Duration::from_millis(100);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn error_code(&self) -> String {
        self.header("x-amzn-errortype")
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    fn message(&self) -> String {
        let document = self.json();
        document
            .get("Message")
            .or_else(|| document.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
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
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a function whose code is `source` with the timeout `timeout`, publish version 1,
/// point the alias `live` at it, and wait out the provisioning window.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    timeout: u32,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Publish": true,
            "Timeout": timeout,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    let alias = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
            .expect("the alias body serializes"),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias on {name:?} must succeed, got {} {}",
        alias.status,
        alias.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `PutFunctionConcurrency` (`PUT /2017-10-31/functions/{FunctionName}/concurrency`).
async fn reserve(router: &MultiServiceRouter, name: &str, reserved: i64) {
    let stored = call(
        router,
        "PUT",
        &format!("/2017-10-31/functions/{name}/concurrency"),
        &serde_json::to_vec(&json!({ "ReservedConcurrentExecutions": reserved }))
            .expect("the concurrency body serializes"),
    )
    .await;
    assert_eq!(
        stored.status,
        200,
        "PutFunctionConcurrency {reserved} on {name:?} must succeed, got {} {}",
        stored.status,
        stored.text()
    );
}

/// `Invoke` on the function `reference` names, with the event `event`.
async fn invoke(router: &MultiServiceRouter, reference: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// A handler that returns at once.
const RETURNS: &str = "def lambda_handler(event, context):\n    return {'ok': True}\n";

/// A handler that holds its execution environment for as long as its event asks.
const SLEEPS: &str = "import time\n\
                      def lambda_handler(event, context):\n\
                      \x20   time.sleep(event['seconds'])\n\
                      \x20   return {'slept': event['seconds']}\n";

/// A handler that reports the kind of environment the runtime says is serving it.
const REPORTS_INITIALIZATION_TYPE: &str =
    "import os\n\
     def lambda_handler(event, context):\n\
     \x20   return {'init': os.environ['AWS_LAMBDA_INITIALIZATION_TYPE']}\n";

/// A reservation of zero admits NO invocation of the function, whichever qualifier the call
/// addresses — the reservation is one pool for the function as a whole — and removing it
/// admits them again.
#[tokio::test]
async fn a_reservation_of_zero_admits_no_invocation_of_any_qualifier() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-reserved-zero";
    create_function(&router, name, RETURNS, 5).await;
    reserve(&router, name, 0).await;
    let cases: &[(&str, String)] = &[
        ("the unpublished version", name.to_string()),
        ("a published version", format!("{name}:1")),
        ("an alias", format!("{name}:live")),
        (
            "the full function arn",
            format!("arn:aws:lambda:us-east-1:000000000000:function:{name}"),
        ),
    ];
    for (case, reference) in cases {
        let answered = invoke(&router, reference, &json!({})).await;
        assert_eq!(
            answered.status,
            429,
            "case {case}: expected the modelled throttle, got {} {}",
            answered.status,
            answered.text()
        );
        assert!(
            answered.error_code().contains("TooManyRequestsException"),
            "case {case}: expected TooManyRequestsException, got {}",
            answered.error_code()
        );
        assert_eq!(
            answered.message(),
            "Rate Exceeded.",
            "case {case}: expected the throttle's own sentence, got {}",
            answered.text()
        );
        assert_eq!(
            answered.json().get("Reason").and_then(Value::as_str),
            Some(RESERVED_REASON),
            "case {case}: expected the function-level reason, got {}",
            answered.text()
        );
    }
    // Removing the reservation gives the function the account's capacity back, so the same
    // invocation is served.
    let removed = call(
        &router,
        "DELETE",
        &format!("/2017-10-31/functions/{name}/concurrency"),
        &[],
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "DeleteFunctionConcurrency must succeed, got {} {}",
        removed.status,
        removed.text()
    );
    let served = invoke(&router, name, &json!({})).await;
    assert_eq!(
        served.status,
        200,
        "case the reservation removed: the invocation must be served, got {} {}",
        served.status,
        served.text()
    );
    assert_eq!(
        served.json(),
        json!({"ok": true}),
        "case the reservation removed: expected the handler's own value, got {}",
        served.text()
    );
}

/// While an invocation holds the function's pool, the next caller is admitted only if the
/// reservation has room for it; and the capacity an invocation held is released HOWEVER that
/// invocation ended, so a reservation of one serves invocation after invocation.
///
/// The two halves are one test because each of them has to hold a real run while asserting,
/// and the run slots this emulator holds are shared by every invocation in the process: the
/// halves run one after the other rather than competing for those slots.
#[tokio::test]
async fn the_pool_is_held_while_an_invocation_is_in_flight_and_released_when_it_ends() {
    let _stream = one_invocation_stream_at_a_time().await;
    let beside: &[(&str, i64, bool)] = &[
        ("a reservation of one, already in use", 1, true),
        ("a reservation of two, with one in use", 2, false),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-reserved-pool";
    create_function(&router, name, SLEEPS, 5).await;
    for (case, reserved, refused_on_the_reservation) in beside {
        reserve(&router, name, *reserved).await;
        let held_event = json!({ "seconds": HELD_SECONDS });
        let probe_event = json!({ "seconds": 0 });
        let holder = invoke(&router, name, &held_event);
        let probe = async {
            // The probe arrives while the holder's run is still in flight, which is what
            // makes the pool full for it.
            tokio::time::sleep(PROBE_AFTER).await;
            invoke(&router, &format!("{name}:live"), &probe_event).await
        };
        let (held, probed) = tokio::join!(holder, probe);
        assert_eq!(
            held.status,
            200,
            "case {case}: the invocation holding the pool must be served, got {} {}",
            held.status,
            held.text()
        );
        // What the reservation decides is whether the invocation beside it is refused ON
        // THAT reservation. The row that has room for it asserts the absence of that
        // refusal rather than a bare 200, because the emulator's own run-slot pool is
        // shared by every invocation in the process and answers the same status for a
        // different limit.
        let reason = probed.json().get("Reason").and_then(Value::as_str) == Some(RESERVED_REASON);
        assert_eq!(
            reason,
            *refused_on_the_reservation,
            "case {case}: expected refused-on-the-reservation to be \
             {refused_on_the_reservation}, got {} {}",
            probed.status,
            probed.text()
        );
        if *refused_on_the_reservation {
            assert_eq!(
                probed.status,
                429,
                "case {case}: the reservation refuses with the modelled throttle, got {} {}",
                probed.status,
                probed.text()
            );
        }
        // And once the holder is done, its capacity is back: the same probe is served.
        let after = invoke(&router, name, &json!({ "seconds": 0 })).await;
        assert_eq!(
            after.status,
            200,
            "case {case}: the pool must be free once the run ended, got {} {}",
            after.status,
            after.text()
        );
    }

    // And the release happens however the run ENDED — one row per way it can end — which a
    // reservation of one makes observable: the second invocation is admitted only if the
    // first gave its capacity back.
    let released: &[(&str, &str, u32, Value)] = &[
        ("a handler that returned", RETURNS, 5, json!({})),
        (
            "a handler that raised",
            "def lambda_handler(event, context):\n    raise ValueError('boom')\n",
            5,
            json!({}),
        ),
        ("a run the timeout ended", SLEEPS, 1, json!({ "seconds": 2 })),
    ];
    for (case, source, timeout, event) in released {
        let name = "probe-released";
        create_function(&router, name, source, *timeout).await;
        reserve(&router, name, 1).await;
        let first = invoke(&router, name, event).await;
        assert_eq!(
            first.status,
            200,
            "case {case}: the first invocation must be served, got {} {}",
            first.status,
            first.text()
        );
        let second = invoke(&router, &format!("{name}:live"), event).await;
        assert_eq!(
            second.status,
            200,
            "case {case}: the next invocation must be admitted once the first ended, got {} {}",
            second.status,
            second.text()
        );
        let deleted = call(&router, "DELETE", &format!("{FUNCTIONS_URI}/{name}"), &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}

/// A function URL request is an invocation of the function, so it draws on the same pool: a
/// reservation of zero leaves the endpoint nothing to serve with, and removing the
/// reservation lets the same request through.
#[tokio::test]
async fn a_function_url_request_is_admitted_against_the_same_pool() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-url-pool";
    create_function(&router, name, URL_RESPONDS, 5).await;
    // `AuthType=NONE` bypasses IAM authentication, and the function's resource-based policy
    // is what admits the anonymous caller.
    let created = call(
        &router,
        "POST",
        &format!("/2021-10-31/functions/{name}/url"),
        &serde_json::to_vec(&json!({ "AuthType": "NONE" }))
            .expect("the url body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunctionUrlConfig must succeed, got {} {}",
        created.status,
        created.text()
    );
    let granted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/policy"),
        &serde_json::to_vec(&json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        }))
        .expect("the policy body serializes"),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "AddPermission must succeed, got {} {}",
        granted.status,
        granted.text()
    );
    let url = created
        .json()
        .get("FunctionUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| panic!("the create answers a FunctionUrl, got {}", created.text()));
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .expect("the minted URL carries a host")
        .to_string();

    reserve(&router, name, 0).await;
    let throttled = get_url(&router, &host).await;
    assert_eq!(
        throttled.status,
        429,
        "case the reservation at zero: expected the throttle, got {} {}",
        throttled.status,
        throttled.text()
    );
    assert_eq!(
        throttled.message(),
        "Rate Exceeded.",
        "case the reservation at zero: expected the throttle's own sentence, got {}",
        throttled.text()
    );

    let removed = call(
        &router,
        "DELETE",
        &format!("/2017-10-31/functions/{name}/concurrency"),
        &[],
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "DeleteFunctionConcurrency must succeed, got {} {}",
        removed.status,
        removed.text()
    );
    let served = get_url(&router, &host).await;
    assert_eq!(
        served.status,
        200,
        "case the reservation removed: the URL must serve the handler's response, got {} {}",
        served.status,
        served.text()
    );
    assert_eq!(
        served.text(),
        "served",
        "case the reservation removed: expected the handler's own body, got {}",
        served.text()
    );
}

/// One GET of the function URL published at `host`, addressed host-style as a client does.
async fn get_url(router: &MultiServiceRouter, host: &str) -> Answer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://{host}/"))
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The handler the function-URL row runs: it describes the response the endpoint serves.
const URL_RESPONDS: &str = "def lambda_handler(event, context):\n    \
     return {'statusCode': 200, 'body': 'served'}\n";

/// An invocation one of the environments provisioned concurrency keeps initialised serves
/// reports that to the runtime; every other invocation reports an on-demand environment.
///
/// The rows span what the qualifier the call addressed holds: a ready allocation, an
/// allocation still being made, and no allocation at all.
#[tokio::test]
async fn an_invocation_reports_the_kind_of_environment_serving_it() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-initialization-type";
    create_function(&router, name, REPORTS_INITIALIZATION_TYPE, 5).await;

    // Before anything is provisioned, every invocation is served on demand.
    let on_demand = invoke(&router, &format!("{name}:1"), &json!({})).await;
    assert_eq!(
        on_demand.json().get("init").and_then(Value::as_str),
        Some("on-demand"),
        "case nothing provisioned: expected an on-demand environment, got {}",
        on_demand.text()
    );

    let provisioned = call(
        &router,
        "PUT",
        &format!("/2019-09-30/functions/{name}/provisioned-concurrency?Qualifier=1"),
        &serde_json::to_vec(&json!({ "ProvisionedConcurrentExecutions": 1 }))
            .expect("the provisioned body serializes"),
    )
    .await;
    assert_eq!(
        provisioned.status,
        202,
        "PutProvisionedConcurrencyConfig must be accepted, got {} {}",
        provisioned.status,
        provisioned.text()
    );
    // While the allocation is still being made there is no initialised environment to serve
    // an invocation, so it is served on demand.
    let allocating = invoke(&router, &format!("{name}:1"), &json!({})).await;
    assert_eq!(
        allocating.json().get("init").and_then(Value::as_str),
        Some("on-demand"),
        "case the allocation in progress: expected an on-demand environment, got {}",
        allocating.text()
    );

    tokio::time::sleep(PROVISIONED_CONCURRENCY_ALLOCATION_DELAY + SLACK).await;
    let cases: &[(&str, String, &str)] = &[
        (
            "the version the allocation was made on",
            format!("{name}:1"),
            "provisioned-concurrency",
        ),
        (
            "the unpublished version, which holds no allocation",
            name.to_string(),
            "on-demand",
        ),
        (
            "an alias, which holds its own allocation and has none",
            format!("{name}:live"),
            "on-demand",
        ),
    ];
    for (case, reference, expected) in cases {
        let answered = invoke(&router, reference, &json!({})).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.json().get("init").and_then(Value::as_str),
            Some(*expected),
            "case {case}: expected the {expected} environment, got {}",
            answered.text()
        );
    }
}

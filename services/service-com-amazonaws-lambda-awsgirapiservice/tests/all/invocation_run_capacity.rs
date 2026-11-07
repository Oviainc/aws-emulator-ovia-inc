// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CAPACITY an invocation runs on: what a caller that arrives
//! while other handlers are executing is answered.
//!
//! The rule under test: "Lambda returns `TooManyRequestsException` if running the function
//! would cause you to exceed a concurrency limit at either the account level
//! (`ConcurrentInvocationLimitExceeded`) or function level
//! (`ReservedFunctionConcurrentInvocationLimitExceeded`)" (`Invoke`) — so an invocation
//! WITHIN those two limits is served, whatever else this process is running, and every one
//! of a function's simultaneous callers is answered with its own run's result. How many
//! handlers this emulator executes at once is not one of those limits: it bounds THIS
//! PROCESS's memory and CPU, so it is sized to the host and it makes a caller WAIT for an
//! execution environment rather than refusing the call.
//!
//! The rows differ along what the rule generalises over: how many callers invoke at once
//! (fewer than the smallest pool of environments this emulator ever has, exactly that many,
//! and more than that) and whether their handlers hold those environments while the others
//! arrive.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    max_concurrent_runs, HOST_WAIT_CEILING, MAX_CONCURRENT_RUNS_CEILING, MIN_CONCURRENT_RUNS,
    PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

/// The module the handler of the package below lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// A handler that holds its execution environment for as long as its event asks and then
/// answers the caller that sent the event, so one row's answers can be told apart.
const HOLDS: &str = "import time\n\
                     def lambda_handler(event, context):\n\
                     \x20   time.sleep(event['seconds'])\n\
                     \x20   return {'caller': event['caller']}\n";

/// How long each handler holds its environment on the rows where the callers overlap.
/// Short, because the pool of environments is shared by every invocation in this process.
const HELD_SECONDS: f64 = 0.2;

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

    fn error_code(&self) -> String {
        self.headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
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

/// Create the function the rows invoke, with no reserved concurrency of its own, and wait
/// out the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 5,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HOLDS)),
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
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` on `name` with the event `event`.
async fn invoke(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// Every one of a function's simultaneous callers is served its own run — one row per
/// relation between the number of callers and the smallest pool of execution environments
/// this emulator ever has, because neither the function (it reserves nothing) nor the
/// account is at a concurrency limit on any of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_simultaneous_caller_within_the_concurrency_limits_is_served() {
    // The counts are stated against `MIN_CONCURRENT_RUNS` rather than the host's own pool,
    // so what this suite asks of the pool every other suite shares stays bounded whatever
    // machine it runs on — and the row above that floor queues on any host.
    let cases: &[(&str, usize, f64)] = &[
        ("one caller on its own", 1, 0.0),
        (
            "as many callers as the smallest pool of environments",
            MIN_CONCURRENT_RUNS,
            HELD_SECONDS,
        ),
        (
            "more callers than that pool holds environments",
            MIN_CONCURRENT_RUNS + 3,
            HELD_SECONDS,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = Arc::new(router_on(&engine));
    let name = "probe-run-capacity";
    create_function(&router, name).await;

    for (case, callers, held) in cases {
        let mut tasks = Vec::with_capacity(*callers);
        for caller in 0..*callers {
            let router = Arc::clone(&router);
            let event = json!({ "seconds": held, "caller": caller });
            tasks.push(tokio::spawn(async move {
                (caller, invoke(&router, name, &event).await)
            }));
        }
        for task in tasks {
            let (caller, answered) = task.await.expect("each caller answers");
            assert_eq!(
                answered.status,
                200,
                "case {case:?}, caller {caller} of {callers}: an invocation within the \
                 function's and the account's concurrency limits must be served, got {} {}",
                answered.status,
                answered.text()
            );
            assert!(
                !answered.error_code().contains("TooManyRequestsException"),
                "case {case:?}, caller {caller} of {callers}: no concurrency limit was \
                 reached, got {}",
                answered.error_code()
            );
            assert_eq!(
                answered.json(),
                json!({ "caller": caller }),
                "case {case:?}, caller {caller} of {callers}: expected this caller's own \
                 run, got {}",
                answered.text()
            );
        }
    }
}

/// The pool of execution environments is sized to the HOST, within the bounds this emulator
/// states for it, and a caller waits for one of them at least as long as it would wait for
/// the run itself — so a run ahead of it is certain to hand its environment back inside that
/// wait, and the pool refuses nothing a concurrency limit did not refuse.
#[test]
fn the_pool_of_environments_is_host_sized_and_waited_out() {
    let pool = max_concurrent_runs();
    // Source: `std::thread::available_parallelism`, the host's own count of the threads of
    // execution it offers.
    let host = std::thread::available_parallelism()
        .map_or(MIN_CONCURRENT_RUNS, std::num::NonZeroUsize::get);
    let cases: &[(&str, bool, String)] = &[
        (
            "never below the floor",
            pool >= MIN_CONCURRENT_RUNS,
            format!("{pool} environments against a floor of {MIN_CONCURRENT_RUNS}"),
        ),
        (
            "never above the ceiling",
            pool <= MAX_CONCURRENT_RUNS_CEILING,
            format!("{pool} environments against a ceiling of {MAX_CONCURRENT_RUNS_CEILING}"),
        ),
        (
            "one per thread of execution the host offers",
            pool >= host.min(MAX_CONCURRENT_RUNS_CEILING),
            format!("{pool} environments on a host offering {host}"),
        ),
        (
            "waited for as long as a run ahead can hold one",
            PERMIT_WAIT >= HOST_WAIT_CEILING,
            format!("a wait of {PERMIT_WAIT:?} against a run of up to {HOST_WAIT_CEILING:?}"),
        ),
    ];
    for (case, holds, actual) in cases {
        assert!(*holds, "case {case:?}: expected it to hold, got {actual}");
    }
}

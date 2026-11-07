// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CAPACITY an invocation runs on: what a caller that arrives
//! while other handlers are executing is answered.
//!
//! The rule under test: "Lambda returns `TooManyRequestsException` if running the function
//! would cause you to exceed a concurrency limit at either the account level
//! (`ConcurrentInvocationLimitExceeded`) or function level
//! (`ReservedFunctionConcurrentInvocationLimitExceeded`)" (`Invoke`) — so an invocation
//! WITHIN those two limits is served, whatever else this process is running, and every one
//! of a function's simultaneous callers is answered with its own run's result. What this
//! emulator bounds instead is the RESOURCE its runs consume: the engine memory they hold
//! together (`state::execution::MAX_CONCURRENT_RUN_MEMORY_MB`), of which each run holds the
//! `MemorySize` its own function asks for (`state::execution::run_memory_claim_mb`). That
//! envelope is no limit of the API: a caller that finds it held WAITS for its share rather
//! than being refused the call.
//!
//! The rows differ along what the rule generalises over: how many callers invoke at once
//! (fewer than the runs the envelope holds at once, exactly that many, and more than that),
//! whether their handlers hold that memory while the others arrive, and how much memory the
//! function asks for.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    run_memory_claim_mb, HOST_WAIT_CEILING, MAX_CONCURRENT_RUNS, MAX_CONCURRENT_RUN_MEMORY_MB,
    PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
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

/// The module the handler of every package below lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Timeout` the functions here carry, in seconds: well above what their handlers do, so
/// what a row asserts is the admission and the waiting rather than a run its own deadline
/// ended, and inside the caller's own ceiling ([`HOST_WAIT_CEILING`]).
const FUNCTION_TIMEOUT_SECONDS: u32 = 20;

/// How long each handler holds its environment on the rows where the callers merely overlap.
/// Short, because the memory the runs of this process share is shared by every invocation in
/// it.
const HELD_SECONDS: f64 = 0.2;

/// How long the handlers that are meant to hold ALL of that memory run for.
///
/// Long enough that a caller arriving beside them really has to wait for its share — with the
/// envelope full and more handlers queued behind them, the wait is several times this —
/// rather than finding memory free as it arrives, which is what those rows are about.
const HELD_WHILE_QUEUEING_SECONDS: f64 = 2.0;

/// How long after the holders start the caller beside them arrives, so it really does arrive
/// while they are running.
const PROBE_AFTER: Duration = Duration::from_millis(150);

/// The most simultaneous invocations any row of this suite fires, so one test of this target
/// cannot take the whole host for itself.
const BURST_CEILING: u32 = 20;

/// The smallest `MemorySize` the model allows, and this suite's default
/// (`smithy_query(query="shape", shape="MemorySize")`: `@range(min: 128, max: 32768)`).
const SMALLEST_MEMORY_SIZE_MB: u32 = 128;

/// A handler that holds its execution environment for as long as its event asks and then
/// answers the caller that sent the event, so one row's answers can be told apart.
const HOLDS: &str = "import time\n\
                     def lambda_handler(event, context):\n\
                     \x20   time.sleep(event['seconds'])\n\
                     \x20   return {'caller': event['caller']}\n";

/// A handler that returns a value computed from its own event, so each answer of a burst is
/// identifiable as that invocation's run and not another's.
const DOUBLES: &str = "def lambda_handler(event, context):\n    \
     return {'doubled': event['n'] * 2}\n";

/// A handler that holds its execution environment for as long as its event asks.
const SLEEPS: &str = "import time\n\
                      def lambda_handler(event, context):\n\
                      \x20   time.sleep(event['seconds'])\n\
                      \x20   return {'slept': event['seconds']}\n";

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

/// Create a function whose code is `source` and whose `MemorySize` is `memory_size` MB, with
/// no reserved concurrency of its own, and wait out the provisioning window the create
/// documents. Nothing is reserved on it, so what bounds its invocations is the account's
/// capacity.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    memory_size: u32,
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
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "MemorySize": memory_size,
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

/// `count` invocations of `name` that are all in flight at once, each carrying the event
/// `event_of` builds for its index, answered in index order.
///
/// A CLONE of the router per caller: they share the one engine and the one service instance,
/// so on this multi-thread runtime the invocations really do overlap.
async fn simultaneously(
    router: &MultiServiceRouter,
    name: &str,
    count: u32,
    event_of: impl Fn(u32) -> Value,
) -> Vec<Answer> {
    let mut callers = tokio::task::JoinSet::new();
    for index in 0..count {
        let router = router.clone();
        let name = name.to_string();
        let event = event_of(index);
        callers.spawn(async move { (index, invoke(&router, &name, &event).await) });
    }
    let mut answers: Vec<(u32, Answer)> = Vec::with_capacity(count as usize);
    while let Some(joined) = callers.join_next().await {
        answers.push(joined.expect("an invoking task must not panic"));
    }
    answers.sort_by_key(|(index, _)| *index);
    answers.into_iter().map(|(_, answer)| answer).collect()
}

/// How many runs of a function at the smallest `MemorySize` the substrate holds at once, as
/// this suite drives it: the runs the envelope holds, never more than this suite's own
/// ceiling.
fn runs_at_once() -> u32 {
    MAX_CONCURRENT_RUNS.min(BURST_CEILING)
}

/// Every one of a function's simultaneous callers is served its own run — one row per
/// relation between the number of callers and the runs the substrate holds at once, because
/// neither the function (it reserves nothing) nor the account is at a concurrency limit on
/// any of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_simultaneous_caller_within_the_concurrency_limits_is_served() {
    let _stream = one_invocation_stream_at_a_time().await;
    let at_once = runs_at_once();
    let cases: &[(&str, u32, f64)] = &[
        ("one caller on its own", 1, 0.0),
        (
            "as many callers as the runs the envelope holds at once",
            at_once,
            HELD_SECONDS,
        ),
        (
            "more callers than the envelope holds runs",
            at_once + 3,
            HELD_SECONDS,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-run-capacity";
    create_function(&router, name, HOLDS, SMALLEST_MEMORY_SIZE_MB).await;

    for (case, callers, held) in cases {
        let held = *held;
        let answers = simultaneously(&router, name, *callers, |caller| {
            json!({ "seconds": held, "caller": caller })
        })
        .await;
        for (caller, answered) in answers.iter().enumerate() {
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

/// Every invocation of a simultaneous burst is RUN, and each answer is its own run's value.
///
/// The rows differ in how many callers arrive at once and in how much memory the function
/// asks for: a burst of a function at the smallest `MemorySize` the model allows, and one of
/// a function at the largest per-run heap the substrate gives a run, where the same burst
/// fits in the envelope a quarter at a time and so has to wait for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_invocation_of_a_simultaneous_burst_is_run_and_answered() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let per_run_ceiling_mb = run_memory_claim_mb(i32::MAX);
    let memories: &[(&str, u32)] = &[
        ("the smallest MemorySize the model allows", SMALLEST_MEMORY_SIZE_MB),
        ("the largest per-run heap a run is given", per_run_ceiling_mb),
    ];
    for (memory_case, memory_size) in memories {
        let name = &format!("probe-burst-{memory_size}");
        create_function(&router, name, DOUBLES, *memory_size).await;
        for count in [1_u32, 4, 8] {
            let case =
                format!("{count} simultaneous invocations of a function with {memory_case}");
            let answers = simultaneously(&router, name, count, |index| json!({ "n": index })).await;
            for (index, answered) in answers.iter().enumerate() {
                assert_eq!(
                    answered.status,
                    200,
                    "case {case}: invocation {index} must be run and answered, got {} {}",
                    answered.status,
                    answered.text()
                );
                let doubled = u64::try_from(index).expect("the index fits a u64") * 2;
                assert_eq!(
                    answered.json(),
                    json!({ "doubled": doubled }),
                    "case {case}: invocation {index} must be answered with its own run's \
                     value, got {}",
                    answered.text()
                );
            }
        }
    }
}

/// An invocation that arrives while other handlers hold the substrate's run memory is still
/// run and answered with its handler's value: memory another handler is holding is not a
/// concurrency limit of the account or of the function.
///
/// The rows differ in how many handlers are holding that memory when the caller beside them
/// arrives — a few of them, as many as the envelope holds at once, and more than that, which
/// leaves that caller nothing to do but wait for a share and still be served on one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invocation_is_run_while_other_handlers_hold_every_share() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-held-memory";
    create_function(&router, name, SLEEPS, SMALLEST_MEMORY_SIZE_MB).await;
    let at_once = runs_at_once();
    let rows: &[(String, u32)] = &[
        ("four handlers holding the run memory".to_string(), 4),
        (
            format!("{at_once} handlers, as many as the envelope holds at once"),
            at_once,
        ),
        (
            format!("{} handlers, more than it holds at once", at_once + 2),
            at_once + 2,
        ),
    ];
    for (case, holders) in rows {
        let holding = simultaneously(&router, name, *holders, |_| {
            json!({ "seconds": HELD_WHILE_QUEUEING_SECONDS })
        });
        let probe = async {
            tokio::time::sleep(PROBE_AFTER).await;
            invoke(&router, name, &json!({ "seconds": 0 })).await
        };
        let (held, probed) = tokio::join!(holding, probe);
        for (index, answered) in held.iter().enumerate() {
            assert_eq!(
                answered.status,
                200,
                "case {case}: holder {index} must be run and answered, got {} {}",
                answered.status,
                answered.text()
            );
        }
        assert_eq!(
            probed.status,
            200,
            "case {case}: the invocation beside them must be run too, got {} {}",
            probed.status,
            probed.text()
        );
        assert_eq!(
            probed.json(),
            json!({ "slept": 0 }),
            "case {case}: the invocation beside them must be answered with its own handler's \
             value, got {}",
            probed.text()
        );
    }
}

/// What a run holds of the substrate's memory envelope is the `MemorySize` its own function
/// asks for, clamped to the range the model allows and to the per-run heap a run is given —
/// so what this process can hold is bounded in AGGREGATE, how many runs fit in it at once
/// follows from the smallest `MemorySize` there is, and a caller waits for a share at least
/// as long as a run ahead of it can hold one.
///
/// The rows are one per class of configured value, including both published bounds of
/// `MemorySize` (`smithy_query(query="shape", shape="MemorySize")`: `@range(min: 128, max:
/// 32768)`) and one past each, which is why a stored value is clamped before it is claimed.
#[test]
fn a_run_holds_the_memory_its_function_asks_for() {
    let ceiling = run_memory_claim_mb(i32::MAX);
    let cases: &[(&str, i32, u32)] = &[
        ("the model's smallest MemorySize", 128, 128),
        ("one below it, which is clamped up", 127, 128),
        ("a negative value, which is clamped up", -512, 128),
        ("a value between the bounds", 256, 256),
        ("the per-run heap ceiling itself", 512, 512),
        ("a value above that ceiling", 1_024, ceiling),
        ("the model's largest MemorySize", 32_768, ceiling),
        ("one above it, which is clamped down", 32_769, ceiling),
    ];
    for (case, configured, expected) in cases {
        let claimed = run_memory_claim_mb(*configured);
        assert_eq!(
            claimed, *expected,
            "case {case} ({configured} MB configured): expected a claim of {expected} MB, \
             got {claimed}"
        );
        assert!(
            claimed <= MAX_CONCURRENT_RUN_MEMORY_MB,
            "case {case}: no run may claim more than the whole envelope of \
             {MAX_CONCURRENT_RUN_MEMORY_MB} MB, got {claimed}"
        );
    }
    // The run threads are bounded by that same envelope: the smallest claim there is, is what
    // decides how many runs fit in it at once.
    let smallest = run_memory_claim_mb(i32::MIN);
    assert_eq!(
        MAX_CONCURRENT_RUNS * smallest,
        MAX_CONCURRENT_RUN_MEMORY_MB,
        "case the runs the envelope holds: {MAX_CONCURRENT_RUNS} runs of the smallest claim \
         ({smallest} MB) must be exactly the {MAX_CONCURRENT_RUN_MEMORY_MB} MB envelope, got {}",
        MAX_CONCURRENT_RUNS * smallest
    );
    assert!(
        ceiling <= MAX_CONCURRENT_RUN_MEMORY_MB / 2,
        "case room beside the largest run: a run holding the per-run ceiling of {ceiling} MB \
         must leave room for another in the {MAX_CONCURRENT_RUN_MEMORY_MB} MB envelope"
    );
    assert!(
        PERMIT_WAIT >= HOST_WAIT_CEILING,
        "case waited for as long as a run ahead can hold a share: a wait of {PERMIT_WAIT:?} \
         against a run of up to {HOST_WAIT_CEILING:?}"
    );
}

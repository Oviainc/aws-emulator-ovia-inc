// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the bound on how many durable executions one function holds.
//!
//! The rule under test: a function's durable executions are held in memory and nothing
//! ever removes one, so the slice is bounded — a start past
//! `MAX_DURABLE_EXECUTIONS` is refused and stores nothing, and refusing is what keeps
//! every execution that WAS started reportable: the ceiling never drops an entry, so
//! `ListDurableExecutionsByFunction` still reports the oldest execution after the
//! refusal.
//!
//! No modelled operation starts a durable execution yet (`Invoke` minting one is its own
//! open issue), so the executions are started through the service's own state, which is
//! where that handler registers them — exactly as `durable_execution_listing.rs` does.
//! What each start answered, and what the listing reports afterwards, is asserted over
//! real HTTP requests through the router.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, MAX_DURABLE_EXECUTIONS,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `ListDurableExecutionsByFunction` is bound
/// (`GET /2025-12-01/functions/{FunctionName}/durable-executions`).
const DURABLE_EXECUTIONS_URI: &str = "/2025-12-01/functions";

/// The router picks a service by the SigV4 credential scope's service field. Nothing in
/// it is verified: this harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, as the model's `Version` pattern spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The function the executions below belong to.
const FUNCTION: &str = "durable-bounded";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `DurableExecutionName` of every entry a listing answered, in answer order.
    fn names(&self) -> Vec<String> {
        match self.json().get("DurableExecutions") {
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| {
                    entry
                        .get("DurableExecutionName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect(),
            other => panic!(
                "expected a DurableExecutions array, got {other:?}: {}",
                self.text()
            ),
        }
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
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

/// `CreateFunction`, which must succeed, then the activation the clock owes made due.
async fn create_function(engine: &Arc<engine::InMemoryEngine>, router: &MultiServiceRouter) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Start one durable execution named `name`, through the state the `Invoke` handler
/// registers one in, answering the ARN it was addressed by when it was started at all.
fn start_execution(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    started: SystemTime,
) -> Option<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let record = store
        .functions
        .get_mut(FUNCTION)
        .unwrap_or_else(|| panic!("case {name}: the store must hold the function {FUNCTION}"));
    record.start_durable_execution(name, VERSION_LATEST, started)
}

/// How many executions the function holds.
fn stored_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(FUNCTION)
        .map_or(0, |record| record.durable_executions.len())
}

/// `ListDurableExecutionsByFunction` with the query string `query`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{DURABLE_EXECUTIONS_URI}/{FUNCTION}/durable-executions{query}"),
        &Value::Null,
    )
    .await
}

/// The name of the `index`-th execution, zero-padded so the names sort the way they were
/// started.
fn execution_name(index: usize) -> String {
    format!("execution-{index:05}")
}

#[tokio::test]
async fn a_function_holds_at_most_the_bounded_number_of_durable_executions() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&engine, &router).await;

    let base = SystemTime::now() - Duration::from_secs(600);

    // Up to the ceiling every start is accepted and answers the ARN it stored, in the
    // one form the model's `DurableExecutionArn` pattern admits
    // (`…/durable-execution/<name>/<id>`), with an id minted per execution rather than
    // taken from the request.
    for index in 0..MAX_DURABLE_EXECUTIONS {
        let name = execution_name(index);
        let offset = Duration::from_millis(u64::try_from(index).unwrap_or(0));
        let started = start_execution(&engine, &name, base + offset);
        let arn = started.unwrap_or_else(|| {
            panic!("case {name:?}: the start inside the ceiling must answer the execution's ARN")
        });
        let infix = format!("/durable-execution/{name}/");
        let id = arn.rsplit('/').next().unwrap_or_default();
        assert!(
            arn.contains(&infix) && !id.is_empty(),
            "case {name:?}: expected an ARN carrying {infix:?} and a minted id, got {arn:?}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        MAX_DURABLE_EXECUTIONS,
        "the function must hold every execution started inside the ceiling"
    );

    // Past the ceiling a start with a name nobody used is refused and stores nothing:
    // the slice is bounded rather than growing with every call.
    for name in ["one-past-the-ceiling", "another-past-the-ceiling"] {
        let refused = start_execution(&engine, name, base + Duration::from_secs(1));
        assert!(
            refused.is_none(),
            "case {name:?}: a start past the ceiling must be refused, got {refused:?}"
        );
        assert_eq!(
            stored_count(&engine),
            MAX_DURABLE_EXECUTIONS,
            "case {name:?}: a refused start must store nothing"
        );
        let listed = list(&router, &format!("?DurableExecutionName={name}")).await;
        assert_eq!(
            (listed.status, listed.names()),
            (200, Vec::new()),
            "case {name:?}: a refused start must leave nothing to report, got {} {}",
            listed.status,
            listed.text()
        );
    }

    // Refusing is not evicting: the oldest execution is still there to be reported, so
    // no accepted work was un-reported to make room.
    let oldest = execution_name(0);
    let listed = list(&router, &format!("?DurableExecutionName={oldest}")).await;
    assert_eq!(
        (listed.status, listed.names()),
        (200, vec![oldest.clone()]),
        "case {oldest:?}: the first execution started must still be reported, got {} {}",
        listed.status,
        listed.text()
    );

    // And a name already held is still refused for its own reason, with the record it
    // names left exactly as it was.
    let repeat = start_execution(&engine, &oldest, base + Duration::from_secs(2));
    assert!(
        repeat.is_none(),
        "case {oldest:?}: an execution name is unique within the function, so a second \
         start must not replace it, got {repeat:?}"
    );
    let after = list(&router, &format!("?DurableExecutionName={oldest}")).await;
    assert_eq!(
        after.json(),
        listed.json(),
        "case {oldest:?}: a refused start must leave the stored execution alone, got {}",
        after.text()
    );
}

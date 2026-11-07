// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the request id an INVOCATION's answer names: `Invoke` and
//! `InvokeWithResponseStream` over the router.
//!
//! The rule under test: every answer of this service names the request it answered in the
//! `x-amzn-requestid` response header — which is what an SDK surfaces as
//! `ResponseMetadata.RequestId` — and an answer that ran or accepted an INVOCATION names
//! THAT invocation's own id there, the same id the run reports as `context.aws_request_id`,
//! writes into its `START`/`END`/`REPORT` log lines and carries in the error document of a
//! handler that raised. An answer that started no invocation names a freshly minted id.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: the runtime the function runs (Python, node), the qualifier
//! the invocation names (the unpublished version, a published version, an alias), how the
//! handler ends (a return, a raise), whether a log tail was asked for, and which invoking
//! operation served the call.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
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

/// The header AWS reports a response's request id in, and an SDK reads
/// `ResponseMetadata.RequestId` from.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

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

    /// The id this answer names its request by.
    fn request_id(&self) -> String {
        self.header(REQUEST_ID_HEADER).unwrap_or_default()
    }

    /// The execution log of the run, decoded from the `X-Amz-Log-Result` tail.
    fn log_tail(&self) -> String {
        let encoded = self.header("x-amz-log-result").unwrap_or_default();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap_or_default();
        String::from_utf8_lossy(&decoded).to_string()
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form every AWS request id carries.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width
            || !group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return false;
        }
    }
    groups.next().is_none()
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Taken by every suite of this target that RUNS handlers for the rules of this issue, for as
/// long as it is running them.
///
/// This emulator serves at most `state::execution::MAX_CONCURRENT_RUNS` invocations at once
/// FOR THE WHOLE PROCESS, and this target runs its tests in parallel: a suite that keeps
/// several of those slots busy starves every other suite's invocations until its own runs
/// end. Holding this lock makes these suites one stream of invocations rather than ten, so
/// they cost the rest of the target one slot instead of all of them. It bounds nothing about
/// the behaviour under test — each suite drives exactly the same requests either way.
pub(crate) async fn one_invocation_stream_at_a_time() -> tokio::sync::MutexGuard<'static, ()> {
    static STREAM: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    STREAM.lock().await
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
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

/// Create a function whose code is `source`, publish version 1, point the alias `live` at
/// it, and wait out the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, name: &str, source: &str, runtime: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": handler_of(runtime),
            "Publish": true,
            "Timeout": 5,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(module(runtime, source)),
            },
        }))
        .expect("the create body serializes"),
        &[],
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
        &[],
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

/// The handler a function of `runtime` is configured with: the module this suite packages
/// for that runtime, and the entry point in it.
fn handler_of(runtime: &str) -> &'static str {
    if runtime.starts_with("nodejs") {
        "index.handler"
    } else {
        "lambda_function.lambda_handler"
    }
}

/// The deployment package holding `source` as the module `runtime`'s handler names, written
/// by the one writer of each language's package this target already has.
fn module(runtime: &str, source: &str) -> Vec<u8> {
    if runtime.starts_with("nodejs") {
        return crate::node_invoke_run::package(&[("index.mjs", source)]);
    }
    package(source)
}

/// `Invoke` on the function `reference` names.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    headers: &[(&str, &str)],
) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        b"{}",
        headers,
    )
    .await
}

/// One row: the case, the runtime, the handler it runs, the qualifier the invocation names
/// and whether it asks for a log tail.
struct Row {
    case: &'static str,
    runtime: &'static str,
    source: &'static str,
    qualifier: Option<&'static str>,
    tail: bool,
}

/// The handler every Python row runs: it reports the id the runtime gave this invocation.
const PYTHON_REPORTS_ITS_REQUEST_ID: &str =
    "def lambda_handler(event, context):\n    return {'request': context.aws_request_id}\n";

/// The same handler for a node function.
const NODE_REPORTS_ITS_REQUEST_ID: &str =
    "export const handler = async (event, context) => ({ request: context.awsRequestId });\n";

/// The response of an invocation names the invocation it ran: the id in the header is the
/// id the run itself reports and the id its log lines are written under.
///
/// The rows span the runtimes an invocation can run, the qualifier forms it can name and
/// both log types, because the id a response names is a property of the invocation and not
/// of any of those.
#[tokio::test]
async fn an_invocations_answer_names_the_invocation_it_ran() {
    let _stream = one_invocation_stream_at_a_time().await;
    let rows: &[Row] = &[
        Row {
            case: "a python handler on the unpublished version",
            runtime: "python3.12",
            source: PYTHON_REPORTS_ITS_REQUEST_ID,
            qualifier: None,
            tail: false,
        },
        Row {
            case: "a python handler on a published version",
            runtime: "python3.12",
            source: PYTHON_REPORTS_ITS_REQUEST_ID,
            qualifier: Some("1"),
            tail: false,
        },
        Row {
            case: "a python handler through an alias",
            runtime: "python3.13",
            source: PYTHON_REPORTS_ITS_REQUEST_ID,
            qualifier: Some("live"),
            tail: false,
        },
        Row {
            case: "a python handler asked for its log tail",
            runtime: "python3.12",
            source: PYTHON_REPORTS_ITS_REQUEST_ID,
            qualifier: None,
            tail: true,
        },
        Row {
            case: "a node handler on the unpublished version",
            runtime: "nodejs22.x",
            source: NODE_REPORTS_ITS_REQUEST_ID,
            qualifier: None,
            tail: false,
        },
        Row {
            case: "a node handler through an alias, with its log tail",
            runtime: "nodejs20.x",
            source: NODE_REPORTS_ITS_REQUEST_ID,
            qualifier: Some("live"),
            tail: true,
        },
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut named: Vec<String> = Vec::new();
    for row in rows {
        let name = "probe-request-id";
        create_function(&router, name, row.source, row.runtime).await;
        let reference = match row.qualifier {
            Some(qualifier) => format!("{name}:{qualifier}"),
            None => name.to_string(),
        };
        let headers: Vec<(&str, &str)> = if row.tail {
            vec![("x-amz-log-type", "Tail")]
        } else {
            Vec::new()
        };
        let answered = invoke(&router, &reference, &headers).await;
        assert_eq!(
            answered.status,
            200,
            "case {}: expected 200, got {} {}",
            row.case,
            answered.status,
            answered.text()
        );
        let stated = answered.request_id();
        assert!(
            is_request_id(&stated),
            "case {}: expected the answer to name its request in {REQUEST_ID_HEADER}, got \
             {stated:?}",
            row.case
        );
        let reported = answered
            .json()
            .get("request")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported,
            stated,
            "case {}: the run reports the invocation as {reported:?}, so the answer must name \
             that invocation, got {stated:?}",
            row.case
        );
        if row.tail {
            let log = answered.log_tail();
            for line in ["START RequestId", "END RequestId", "REPORT RequestId"] {
                let expected = format!("{line}: {stated}");
                assert!(
                    log.contains(&expected),
                    "case {}: expected the log line {expected:?}, got {log:?}",
                    row.case
                );
            }
        }
        assert!(
            !named.contains(&stated),
            "case {}: every invocation is named by an id of its own, got {stated:?} twice",
            row.case
        );
        named.push(stated);
        let deleted = call(
            &router,
            "DELETE",
            &format!("{FUNCTIONS_URI}/{name}"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {}: the function must be removable between rows",
            row.case
        );
    }
}

/// A handler that RAISED is answered with its own error document, which names the same
/// invocation the response header does — one row per way a run fails.
#[tokio::test]
async fn a_failed_runs_error_document_names_the_invocation_the_answer_names() {
    let _stream = one_invocation_stream_at_a_time().await;
    let cases: &[(&str, &str)] = &[
        (
            "a raised exception",
            "def lambda_handler(event, context):\n    raise ValueError('boom')\n",
        ),
        (
            "a failed computation",
            "def lambda_handler(event, context):\n    return 1 / 0\n",
        ),
        (
            "a handler the module does not define",
            "def other(event, context):\n    return 1\n",
        ),
        (
            "a module that does not import",
            "import no_such_module_at_all\n",
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    for (case, source) in cases {
        let name = "probe-raise-request-id";
        create_function(&router, name, source, "python3.12").await;
        let answered = invoke(&router, name, &[("x-amz-log-type", "Tail")]).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: an unhandled error is still a 200, got {} {}",
            answered.status,
            answered.text()
        );
        let stated = answered.request_id();
        let reported = answered
            .json()
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported, stated,
            "case {case}: the error document names the invocation as {reported:?}, so the \
             answer must name that invocation, got {stated:?}"
        );
        let log = answered.log_tail();
        let expected = format!("REPORT RequestId: {stated}");
        assert!(
            log.contains(&expected),
            "case {case}: expected the log line {expected:?}, got {log:?}"
        );
        let deleted = call(
            &router,
            "DELETE",
            &format!("{FUNCTIONS_URI}/{name}"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: the function must be removable between rows"
        );
    }
}

/// A response-streaming invocation is an invocation: its answer names the invocation it ran,
/// which the run reports inside the stream.
#[tokio::test]
async fn a_response_stream_invocations_answer_names_the_invocation_it_ran() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-stream-request-id";
    create_function(&router, name, PYTHON_REPORTS_ITS_REQUEST_ID, "python3.12").await;
    for case in ["the unpublished version", "an alias"] {
        let reference = if case == "an alias" {
            format!("{name}:live")
        } else {
            name.to_string()
        };
        let answered = call(
            &router,
            "POST",
            &format!("/2021-11-15/functions/{reference}/response-streaming-invocations"),
            b"{}",
            &[],
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        let stated = answered.request_id();
        assert!(
            is_request_id(&stated),
            "case {case}: expected the answer to name its request in {REQUEST_ID_HEADER}, got \
             {stated:?}"
        );
        let framed = answered.text();
        assert!(
            framed.contains(&stated),
            "case {case}: the run reports its own invocation in the stream, so the answer must \
             name that invocation; {stated:?} is not in {framed:?}"
        );
    }
}

/// An answer that ran NO function still names its request — a fresh id, because there is no
/// invocation for it to name: one row per kind of answer that invokes nothing.
#[tokio::test]
async fn an_answer_that_invoked_nothing_names_a_fresh_request_id() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-no-run";
    create_function(&router, name, PYTHON_REPORTS_ITS_REQUEST_ID, "python3.12").await;
    // The invocation the function DID run, whose id no other answer may name.
    let ran = invoke(&router, name, &[]).await;
    let invocation = ran.request_id();

    let dry_run = invoke(&router, name, &[("x-amz-invocation-type", "DryRun")]).await;
    let read = call(&router, "GET", &format!("{FUNCTIONS_URI}/{name}"), &[], &[]).await;
    let missing = invoke(&router, "probe-no-such-function", &[]).await;
    let cases: &[(&str, u16, &Answer)] = &[
        ("a dry run, which invokes nothing", 204, &dry_run),
        ("a control-plane read", 200, &read),
        ("a refusal of an unresolvable function", 404, &missing),
    ];
    for (case, status, answered) in cases {
        assert_eq!(
            answered.status, *status,
            "case {case}: expected {status}, got {} {}",
            answered.status,
            answered.text()
        );
        let stated = answered.request_id();
        assert!(
            is_request_id(&stated),
            "case {case}: every answer names its request in {REQUEST_ID_HEADER}, got {stated:?}"
        );
        assert_ne!(
            stated, invocation,
            "case {case}: an answer that ran no function must not name another call's \
             invocation"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the X-RAY TRACE CONTEXT a deployed stage propagates onto the
//! `AWS_PROXY` integration it serves a request through, and the SEGMENT a tracing stage
//! records for that request: API Gateway, AWS Lambda and AWS X-Ray on ONE engine, the
//! context read back out of the function's own environment and the segment read back where a
//! client reads it.
//!
//! The rule under test: **every hop of a traced request chain propagates the chain on the
//! call it makes onward, in X-Ray's own `X-Amzn-Trace-Id` header — continuing the `Root` the
//! caller supplied where it supplied one of the documented form, starting one of its own
//! where it supplied none or supplied text that is not a trace id, naming the span of its OWN
//! work as the `Parent`, and carrying the caller's sampling decision where it took one. A hop
//! that records work of its own stamps that header and submits a segment for the work; a hop
//! that records nothing passes the caller's header on as written and submits nothing.** For a
//! REST stage, what decides which of the two it is is one record: `Stage$tracingEnabled`,
//! "specifies whether active tracing with X-ray is enabled for the Stage".
//!
//! Three services have to agree for that to hold — API Gateway stamps the header on the
//! invocation and records the stage's segment, Lambda reports the context to the handler in
//! `_X_AMZN_TRACE_ID` and records the invocation's own segments, and X-Ray holds them — so
//! the suite drives real HTTP requests through all three.
//!
//! The rows differ along what the rule generalises over: what the caller supplied (a sampled
//! chain, an unsampled one, a chain with no decision, a chain with a parent span of its own,
//! a root that is not a trace id, no header at all) and whether the stage the request reached
//! is tracing. Each row asserts both halves: the context the HANDLER saw, echoed in the proxy
//! response the client was served, and the segments X-Ray holds for the chain that context
//! named.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The .zip deployment package writer the sibling proxy suites of this target already have:
// one writer of a deployment archive here, rather than a second copy of it.
use crate::proxy_binary_media_types::package;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The two stages the api is deployed to: one tracing, one left at the default.
const TRACED_STAGE: &str = "traced";
const UNTRACED_STAGE: &str = "untraced";

/// The resource path the one method of the api is bound at.
const RESOURCE_PATH: &str = "probe";

/// The function the method is integrated with.
const FUNCTION: &str = "trace-probe";

/// The role the created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";

/// The handler the deployment package declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for Lambda's clock-owed transition to `Active`, and how often it
/// looks. The transition is that service's own, so nothing here sleeps on its behalf.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The `origin` of the segment a REST API stage records, and of the two a Lambda invocation
/// records.
const STAGE_ORIGIN: &str = "AWS::ApiGateway::Stage";
const LAMBDA_ORIGIN: &str = "AWS::Lambda";
const FUNCTION_ORIGIN: &str = "AWS::Lambda::Function";

/// The address the execution environment of every invocation names the X-Ray daemon at, as
/// the Lambda Developer Guide gives the form of `AWS_XRAY_DAEMON_ADDRESS` (`IP_ADDRESS:PORT`)
/// and the daemon's own `2000/udp` port, and the strategy a tracer reads out of
/// `AWS_XRAY_CONTEXT_MISSING` on Lambda.
const DAEMON_ADDRESS: &str = "127.0.0.1:2000";
const CONTEXT_MISSING: &str = "LOG_ERROR";

/// The handler every row runs: it reports the trace context its own environment carries and
/// the one subsegment it sent to the X-Ray daemon, so the client's own response is evidence of
/// what reached the invocation.
const HANDLER_SOURCE: &str = r##"
import json
import os
import socket


def _written(datagram, destination):
    """Write `datagram` to `destination` (the daemon's own address by default), answering the
    error where that address refused it."""
    named = destination or os.environ["AWS_XRAY_DAEMON_ADDRESS"]
    host, _, port = named.rpartition(":")
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as writer:
        try:
            if datagram is not None:
                writer.sendto(datagram.encode("utf-8"), (host, int(port)))
        except OSError as error:
            return str(error)
    return None


def lambda_handler(event, context):
    raw = os.environ.get("_X_AMZN_TRACE_ID") or ""
    fields = {}
    for field in raw.split(";"):
        name, _, value = field.partition("=")
        if name:
            fields[name] = value
    sub_id = (event.get("queryStringParameters") or {}).get("sub_id")
    sent = None
    # A tracer honours the request's own sampling decision: a request that is not being
    # recorded has nothing sent for it.
    if sub_id and fields.get("Root") and fields.get("Sampled") == "1":
        document = {
            "trace_id": fields["Root"],
            "id": sub_id,
            "name": "probe-work",
            "type": "subsegment",
            "namespace": "aws",
            "start_time": 1600000000.0,
            "end_time": 1600000001.0,
        }
        if fields.get("Parent"):
            document["parent_id"] = fields["Parent"]
        _written(json.dumps({"format": "json", "version": 1}) + "\n" + json.dumps(document), None)
        sent = sub_id
    # The datagrams the request's own body names, each to the address that entry names: the
    # FRAMING table, driven through one request so one run delivers the whole of it.
    refused = []
    for entry in json.loads(event.get("body") or "{}").get("datagrams") or []:
        refused.append(_written(entry.get("datagram"), entry.get("destination")))
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps(
            {
                "raw_trace_header": raw,
                "root": fields.get("Root"),
                "parent": fields.get("Parent"),
                "sampled": fields.get("Sampled"),
                "sent": sent,
                "refused": refused,
                # The two X-Ray variables a managed runtime's environment names beside the
                # trace context: where an instrumented handler sends its segment documents,
                # and what a tracer does when it finds no context.
                "daemon": os.environ.get("AWS_XRAY_DAEMON_ADDRESS"),
                "context_missing": os.environ.get("AWS_XRAY_CONTEXT_MISSING"),
                # What the method's AUTHORIZER published about its own invocation, which API
                # Gateway carries into the event under `requestContext.authorizer`.
                "authorizer_trace": (
                    (event.get("requestContext") or {}).get("authorizer") or {}
                ).get("trace"),
            }
        ),
    }
"##;

/// The AUTHORIZER function the second test's method is guarded by: it allows the method it was
/// asked about and publishes its OWN trace context in the `context` map, which API Gateway
/// carries into the integration's event — so the chain the authorizer's invocation ran in is
/// observable in the response the client is served.
const AUTHORIZER_SOURCE: &str = r##"
import os


def lambda_handler(event, context):
    return {
        "principalId": "trace-probe-principal",
        "policyDocument": {
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Action": "execute-api:Invoke",
                    "Effect": "Allow",
                    "Resource": event["methodArn"],
                }
            ],
        },
        "context": {"trace": os.environ.get("_X_AMZN_TRACE_ID") or ""},
    }
"##;

/// What a dispatched request answered.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them: the invocation the stage makes and the segments both services record are served by
/// the very Lambda and X-Ray this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: a control-plane call carries a credential
    // scope so the router can name the target service, and the invoke of the published stage
    // URL carries none, as a client of one sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answered.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire, answering its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// Every segment document X-Ray holds for `trace_id`, read through `BatchGetTraces` — the
/// operation a client reads a trace back with (`POST /Traces`).
///
/// `pub(crate)` so the sibling `stage_trace_sampling_decision` suite reads a chain back the
/// same one way rather than keeping a second reader of the same response shape.
pub(crate) async fn documents_of(router: &MultiServiceRouter, trace_id: &str) -> Vec<Value> {
    let answered = dispatch(
        router,
        "POST",
        &format!("xray.{REGION}.amazonaws.com"),
        "/Traces",
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("xray")),
        ],
        Bytes::from(json!({ "TraceIds": [trace_id] }).to_string()),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "expected HTTP 200 from BatchGetTraces, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered
        .json()
        .get("Traces")
        .and_then(Value::as_array)
        .map(|traces| {
            traces
                .iter()
                .flat_map(|trace| {
                    trace
                        .get("Segments")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|segment| {
                    segment
                        .get("Document")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// The function every row's request reaches, created with `TracingConfig` `Active` and waited
/// out to `Active`, with the grant that admits an invocation from API Gateway.
async fn given_function(router: &MultiServiceRouter) -> String {
    created_function(router, FUNCTION, HANDLER_SOURCE).await
}

/// One `TracingConfig` `Active` function running `source`, with the grant that admits an
/// invocation from API Gateway, waited out to `Active`. Answers its ARN.
async fn created_function(router: &MultiServiceRouter, name: &str, source: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 30,
            "TracingConfig": { "Mode": "Active" },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected HTTP 201 from CreateFunction, got {status} with {created}"
    );
    let arn = member("CreateFunction", &created, "FunctionArn");
    let (status, granted) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        Some(json!({
            "StatementId": "apigw-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected HTTP 201 from AddPermission, got {status} with {granted}"
    );
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let (_status, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return arn;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "setup: Lambda never reported the function Active: {reported}"
        );
        tokio::time::sleep(LOOK).await;
    }
}

/// The api every row's request reaches: one `GET /{RESOURCE_PATH}` method behind an
/// `AWS_PROXY` integration onto `function_arn`, deployed to both stages, with tracing turned
/// on for [`TRACED_STAGE`] alone.
async fn given_api(router: &MultiServiceRouter, function_arn: &str) -> String {
    let created = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "trace-context-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource_id = member("CreateResource", &resource, "id");
    let method = format!("/restapis/{api}/resources/{resource_id}/methods/GET");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &method,
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("{method}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function_arn}/invocations"
            ),
        })),
        201,
    )
    .await;
    for stage in [TRACED_STAGE, UNTRACED_STAGE] {
        apigw(
            router,
            "CreateDeployment",
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;
    }
    // Only the one stage traces: the other is left exactly as the deployment created it.
    apigw(
        router,
        "UpdateStage",
        "PATCH",
        &format!("/restapis/{api}/stages/{TRACED_STAGE}"),
        Some(json!({
            "patchOperations": [
                { "op": "replace", "path": "/tracingEnabled", "value": "true" }
            ]
        })),
        200,
    )
    .await;
    let reported = apigw(
        router,
        "GetStage",
        "GET",
        &format!("/restapis/{api}/stages/{TRACED_STAGE}"),
        None,
        200,
    )
    .await;
    assert_eq!(
        reported.get("tracingEnabled"),
        Some(&json!(true)),
        "setup: expected the stage to report tracingEnabled, got {reported}"
    );
    api
}

/// One row: what the caller sends and where, and what must follow.
struct Row {
    case: &'static str,
    /// The stage the request is addressed to.
    stage: &'static str,
    /// The `X-Amzn-Trace-Id` the caller sends, or `None` for a caller that sends none.
    sent: Option<&'static str>,
    /// The chain the handler must see, or `None` where the hop starts one of its own.
    chain: Option<&'static str>,
    /// The sampling decision the handler must see.
    decision: &'static str,
    /// Whether the stage records a segment of its own for this request.
    stage_segment: bool,
    /// Whether Lambda records its own two segments for this request.
    lambda_segments: bool,
}

/// A chain of X-Ray's own form (`1-<8 hex>-<24 hex>`), and a span id of its own form, that a
/// caller supplies. One per row that supplies one, so no two rows share a chain.
const SAMPLED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272701";
const UNSAMPLED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272702";
const UNDECIDED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272703";
const PARENTED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272704";
const UNTRACED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272705";
/// The chain of the row that proves a stage which records nothing passes the caller's header
/// on AS WRITTEN: its `Parent` reaches Lambda untouched, so Lambda's own segment hangs from
/// the caller's span rather than from one this stage minted.
const PASSED_THROUGH_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272706";
const CALLER_SPAN: &str = "53995c3f42cd8ad8";

const ROWS: &[Row] = &[
    Row {
        case: "a sampled chain, on the tracing stage",
        stage: TRACED_STAGE,
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272701;Sampled=1"),
        chain: Some(SAMPLED_CHAIN),
        decision: "1",
        stage_segment: true,
        lambda_segments: true,
    },
    Row {
        case: "an unsampled chain, on the tracing stage",
        stage: TRACED_STAGE,
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272702;Sampled=0"),
        chain: Some(UNSAMPLED_CHAIN),
        decision: "0",
        stage_segment: false,
        lambda_segments: false,
    },
    Row {
        case: "a chain with no decision, on the tracing stage",
        stage: TRACED_STAGE,
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272703"),
        chain: Some(UNDECIDED_CHAIN),
        decision: "1",
        stage_segment: true,
        lambda_segments: true,
    },
    Row {
        case: "a chain with a parent span of its own, on the tracing stage",
        stage: TRACED_STAGE,
        sent: Some(
            "Root=1-5759e988-bd862e3fe1be46a994272704;Parent=53995c3f42cd8ad8;Sampled=1",
        ),
        chain: Some(PARENTED_CHAIN),
        decision: "1",
        stage_segment: true,
        lambda_segments: true,
    },
    Row {
        case: "a root that is not a trace id, on the tracing stage",
        stage: TRACED_STAGE,
        sent: Some("Root=not-a-trace-id;Sampled=1"),
        chain: None,
        decision: "1",
        stage_segment: true,
        lambda_segments: true,
    },
    Row {
        case: "no header at all, on the tracing stage",
        stage: TRACED_STAGE,
        sent: None,
        chain: None,
        decision: "1",
        stage_segment: true,
        lambda_segments: true,
    },
    Row {
        case: "a sampled chain, on the stage that is not tracing",
        stage: UNTRACED_STAGE,
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272705;Sampled=1"),
        chain: Some(UNTRACED_CHAIN),
        decision: "1",
        stage_segment: false,
        lambda_segments: true,
    },
    Row {
        case: "no header at all, on the stage that is not tracing",
        stage: UNTRACED_STAGE,
        sent: None,
        chain: None,
        decision: "1",
        stage_segment: false,
        lambda_segments: true,
    },
    Row {
        case: "a chain with a parent span of its own, on the stage that is not tracing",
        stage: UNTRACED_STAGE,
        sent: Some(
            "Root=1-5759e988-bd862e3fe1be46a994272706;Parent=53995c3f42cd8ad8;Sampled=1",
        ),
        chain: Some(PASSED_THROUGH_CHAIN),
        decision: "1",
        stage_segment: false,
        lambda_segments: true,
    },
];

#[tokio::test]
async fn a_deployed_stage_propagates_the_trace_context_onto_its_proxy_integration() {
    let router = emulator();
    let function_arn = given_function(&router).await;
    let api = given_api(&router, &function_arn).await;
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // The subsegment the handler sends to the daemon, under an id of this row's own, so
        // what landed is told from what another row's handler sent.
        let sub_id = format!("77aa33bb44cc55{index:02}");
        let mut headers: Vec<(&str, &str)> = vec![];
        if let Some(sent) = row.sent {
            headers.push(("x-amzn-trace-id", sent));
        }
        let served = dispatch(
            &router,
            "GET",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            &format!("/{}/{RESOURCE_PATH}?sub_id={sub_id}", row.stage),
            &headers,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: expected HTTP 200 from the stage, got {} with {:?}",
            served.status,
            served.text()
        );
        let reported = served.json();
        let raw = member(case, &reported, "raw_trace_header");
        let root = reported
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            ::service_common::xray_trace::is_trace_id(&root),
            "case {case:?}: expected the handler to see a trace id as its Root, got {raw:?}"
        );
        if let Some(continued) = row.chain {
            assert_eq!(
                root, continued,
                "case {case:?}: expected the caller's chain continued, got {raw:?}"
            );
        } else {
            assert!(
                ROWS.iter().all(|other| other.chain != Some(root.as_str())),
                "case {case:?}: expected a chain the hop started, got {raw:?}"
            );
        }
        assert_eq!(
            reported.get("sampled").and_then(Value::as_str),
            Some(row.decision),
            "case {case:?}: expected the decision {:?} to reach the handler, got {raw:?}",
            row.decision
        );
        // The two X-Ray variables the execution environment of every invocation names, beside
        // the context itself: the daemon an instrumented handler writes its segment documents
        // to, and the strategy a tracer reads when it finds no context.
        assert_eq!(
            reported.get("daemon").and_then(Value::as_str),
            Some(DAEMON_ADDRESS),
            "case {case:?}: expected the invocation's environment to name the daemon at \
             {DAEMON_ADDRESS:?}, got {reported}"
        );
        assert_eq!(
            reported.get("context_missing").and_then(Value::as_str),
            Some(CONTEXT_MISSING),
            "case {case:?}: expected the context-missing strategy {CONTEXT_MISSING:?}, got \
             {reported}"
        );
        let parent = reported
            .get("parent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if row.stage == TRACED_STAGE {
            // A tracing stage names its own call out as the parent, so the handler's own
            // work hangs from a span the stage minted rather than from the caller's.
            assert!(
                ::service_common::xray_trace::is_span_id(parent),
                "case {case:?}: expected a minted span id as the handler's Parent, got \
                 {raw:?}"
            );
            assert_ne!(
                parent, CALLER_SPAN,
                "case {case:?}: expected the stage's own span rather than the caller's, got \
                 {raw:?}"
            );
        }
        // What X-Ray holds for the chain the handler saw: the stage's segment where the
        // stage records one, Lambda's two where the invocation is recorded, and the
        // handler's own subsegment where it was sent and the chain is recorded.
        let held = documents_of(&router, &root).await;
        let origins: Vec<&str> = held
            .iter()
            .filter_map(|document| document.get("origin").and_then(Value::as_str))
            .collect();
        assert_eq!(
            origins.contains(&STAGE_ORIGIN),
            row.stage_segment,
            "case {case:?}: expected a {STAGE_ORIGIN} segment={}, got {held:?}",
            row.stage_segment
        );
        assert_eq!(
            origins.contains(&LAMBDA_ORIGIN),
            row.lambda_segments,
            "case {case:?}: expected an {LAMBDA_ORIGIN} segment={}, got {held:?}",
            row.lambda_segments
        );
        assert_eq!(
            origins.contains(&FUNCTION_ORIGIN),
            row.lambda_segments,
            "case {case:?}: expected an {FUNCTION_ORIGIN} segment={}, got {held:?}",
            row.lambda_segments
        );
        let ids: Vec<&str> = held
            .iter()
            .filter_map(|document| document.get("id").and_then(Value::as_str))
            .collect();
        assert_eq!(
            ids.contains(&sub_id.as_str()),
            row.lambda_segments,
            "case {case:?}: expected the handler's own subsegment {sub_id:?} to be held={}, \
             got {held:?}",
            row.lambda_segments
        );
        if !row.lambda_segments {
            continue;
        }
        // The tree is CLOSED: the function segment's parent is the Lambda segment, the
        // Lambda segment's parent is the stage's own call out where the stage recorded one,
        // and the handler's subsegment hangs from the function segment — which is the span
        // the handler saw as its own `Parent`.
        let by_origin = |origin: &str| {
            held.iter()
                .find(|document| document.get("origin").and_then(Value::as_str) == Some(origin))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let service = by_origin(LAMBDA_ORIGIN);
        let function = by_origin(FUNCTION_ORIGIN);
        assert_eq!(
            function.get("parent_id").and_then(Value::as_str),
            service.get("id").and_then(Value::as_str),
            "case {case:?}: expected the function segment to hang from the Lambda segment, \
             got {held:?}"
        );
        assert_eq!(
            function.get("id").and_then(Value::as_str),
            Some(parent),
            "case {case:?}: expected the function segment to be the span the handler saw as \
             its Parent, got {held:?}"
        );
        let subsegment = held
            .iter()
            .find(|document| document.get("id").and_then(Value::as_str) == Some(sub_id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            subsegment.get("parent_id").and_then(Value::as_str),
            Some(parent),
            "case {case:?}: expected the handler's subsegment to hang from the function \
             segment, got {held:?}"
        );
        if row.stage_segment {
            let stage = by_origin(STAGE_ORIGIN);
            assert_eq!(
                stage.get("name").and_then(Value::as_str),
                Some(format!("{api}/{}", row.stage).as_str()),
                "case {case:?}: expected the stage segment named for the api and stage, got \
                 {held:?}"
            );
            let call_out = stage
                .get("subsegments")
                .and_then(Value::as_array)
                .and_then(|subsegments| subsegments.first())
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                call_out.get("namespace").and_then(Value::as_str),
                Some("aws"),
                "case {case:?}: expected an aws-namespace call out on the stage segment, got \
                 {held:?}"
            );
            assert_eq!(
                call_out.get("aws").and_then(|aws| aws.get("function_name")),
                Some(&json!(FUNCTION)),
                "case {case:?}: expected the invoked function named on the call out, got \
                 {held:?}"
            );
            assert_eq!(
                service.get("parent_id").and_then(Value::as_str),
                call_out.get("id").and_then(Value::as_str),
                "case {case:?}: expected the Lambda segment to hang from the stage's call \
                 out, got {held:?}"
            );
            // The stage is the one parentless document of a chain it started, and a child of
            // the caller's own span where the caller named one.
            let expected_parent = row
                .sent
                .filter(|sent| sent.contains("Parent="))
                .map(|_| CALLER_SPAN);
            assert_eq!(
                stage.get("parent_id").and_then(Value::as_str),
                expected_parent,
                "case {case:?}: expected the stage segment's parent to be \
                 {expected_parent:?}, got {held:?}"
            );
        } else {
            // A stage that records nothing passes the caller's header on AS WRITTEN, so the
            // span the caller named reaches Lambda untouched and Lambda's own segment hangs
            // from it; a caller that named none leaves that segment the root of the trace.
            let expected_parent = row
                .sent
                .filter(|sent| sent.contains("Parent="))
                .map(|_| CALLER_SPAN);
            assert_eq!(
                service.get("parent_id").and_then(Value::as_str),
                expected_parent,
                "case {case:?}: expected the Lambda segment's parent to be \
                 {expected_parent:?} in a trace no stage recorded, got {held:?}"
            );
        }
    }
    // What the X-RAY DAEMON at the address the environment names does with what a handler
    // writes to it, driven through this same api and stage — the other half of the rule, and
    // asserted where the handler that writes those datagrams already runs.
    a_datagram_a_handler_writes_to_the_daemon_becomes_a_trace_x_ray_holds(&router, &api).await;
    // The second half of the rule — what an AUTHORIZER's own invocation carries — is asserted
    // on an emulator of its own, from this same test function, so this suite adds one
    // concurrent interpreter to its target rather than two.
    an_authorizers_invocation_runs_in_the_same_trace_as_the_integrations().await;
}

/// One segment document, as a tracer writes one: the four members `PutTraceSegments` documents
/// as required, and the `name` a row asserts it by.
fn segment(trace_id: &str, id: &str, name: &str) -> String {
    json!({
        "trace_id": trace_id,
        "id": id,
        "name": name,
        "start_time": 1_600_000_000.0,
        "end_time": 1_600_000_001.0,
    })
    .to_string()
}

/// The header line the X-Ray daemon's own framing begins with.
const DAEMON_HEADER: &str = r#"{"format":"json","version":1}"#;

/// What one row's datagram is to leave in X-Ray.
struct Datagram {
    case: &'static str,
    /// Where the handler addresses it: the daemon's own address when `None`.
    destination: Option<&'static str>,
    /// Its lines, joined with `\n`, where `"header"` stands for the daemon's own header line
    /// and `"#<n>"` for the row's `n`th segment document.
    lines: &'static [&'static str],
    /// Which of the row's documents X-Ray must hold afterwards, by `name`.
    stored: &'static [&'static str],
    /// Whether the address refused it.
    refused: bool,
}

/// The rows differ along the FRAMING of the datagram (one document, two documents, a header
/// that is not the daemon's, no header line at all, a body line that is not a document) and
/// along its DESTINATION (the daemon's own address, another host, and the daemon's own port on
/// another host).
const DATAGRAMS: &[Datagram] = &[
    Datagram {
        case: "a well-formed datagram carrying one document",
        destination: None,
        lines: &["header", "#0"],
        stored: &["one"],
        refused: false,
    },
    Datagram {
        case: "a well-formed datagram carrying two documents",
        destination: None,
        lines: &["header", "#0", "#1"],
        stored: &["one", "two"],
        refused: false,
    },
    Datagram {
        case: "a header line that is not the daemon's format",
        destination: None,
        lines: &[r#"{"format":"garbage","version":1}"#, "#0"],
        stored: &[],
        refused: false,
    },
    Datagram {
        case: "a datagram with no header line at all",
        destination: None,
        lines: &["#0"],
        stored: &[],
        refused: false,
    },
    Datagram {
        case: "a body line that is not a document, behind a well-formed header",
        destination: None,
        lines: &["header", "not-a-document", "#1"],
        stored: &["two"],
        refused: false,
    },
    Datagram {
        case: "a datagram addressed at another host",
        destination: Some("198.51.100.7:2000"),
        lines: &["header", "#0"],
        stored: &[],
        refused: true,
    },
    Datagram {
        case: "a datagram addressed at the daemon's port on another host",
        destination: Some("127.0.0.2:2000"),
        lines: &["header", "#0"],
        stored: &[],
        refused: true,
    },
];

/// What the daemon does with what a handler writes to it: the whole [`DATAGRAMS`] table, driven
/// through ONE request to the stage — each row names its own chain and its own span ids, so
/// what one row left in X-Ray is told from what another did.
async fn a_datagram_a_handler_writes_to_the_daemon_becomes_a_trace_x_ray_holds(
    router: &MultiServiceRouter,
    api: &str,
) {
    let traces: Vec<String> = (0..DATAGRAMS.len())
        .map(|row| format!("1-5759e988-bd862e3fe1be46a9942728{row:02}"))
        .collect();
    let entries: Vec<Value> = DATAGRAMS
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let documents = [
                segment(&traces[index], &format!("53995c3f42cd{index:02}00"), "one"),
                segment(&traces[index], &format!("53995c3f42cd{index:02}01"), "two"),
            ];
            let datagram = row
                .lines
                .iter()
                .map(|line| match *line {
                    "header" => DAEMON_HEADER.to_string(),
                    "#0" => documents[0].clone(),
                    "#1" => documents[1].clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<String>>()
                .join("\n");
            json!({ "datagram": datagram, "destination": row.destination })
        })
        .collect();
    let served = dispatch(
        router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{TRACED_STAGE}/{RESOURCE_PATH}"),
        &[("content-type", "application/json")],
        Bytes::from(json!({ "datagrams": entries }).to_string()),
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "the stage must serve the request that delivers the datagram table, got {} with {:?}",
        served.status,
        served.text()
    );
    let reported = served.json();
    for (index, row) in DATAGRAMS.iter().enumerate() {
        let case = row.case;
        let trace = &traces[index];
        assert_eq!(
            reported["refused"][index].is_string(),
            row.refused,
            "case {case:?}: expected the write to be refused={}, got {reported}",
            row.refused
        );
        let held = documents_of(router, trace).await;
        let mut actual: Vec<String> = held
            .iter()
            .filter_map(|document| document.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let mut expected: Vec<String> = row.stored.iter().map(|name| (*name).to_string()).collect();
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected X-Ray to hold the documents {expected:?} under {trace}, \
             got {held:?}"
        );
    }
}

/// The function the guarded method's authorizer runs, and the header the request carries its
/// `identitySource` in.
const AUTHORIZER_FUNCTION: &str = "trace-authorizer";
const IDENTITY_HEADER: &str = "authorization";

/// A `CUSTOM` AUTHORIZER's own invocation is the same request's work — over a TABLE whose rows
/// differ along what the caller supplied, which is what decides whether the chain is the
/// caller's or one the stage starts.
///
/// The rule this pins is the second half of (b): a hop decides the chain and the decision ONCE
/// per request, and then names a span per call it makes. So the authorizer's function and the
/// integration's function run in ONE trace, with the authorizer's work hanging from the stage's
/// own recorded segment rather than from the span of the stage's call out to the integration.
/// The authorizer publishes its own `_X_AMZN_TRACE_ID` in the `context` map it returns, which
/// API Gateway carries into the integration's event, so the chain it ran in is observable in
/// the response the client is served.
async fn an_authorizers_invocation_runs_in_the_same_trace_as_the_integrations() {
    let router = emulator();
    let function_arn = created_function(&router, FUNCTION, HANDLER_SOURCE).await;
    let authorizer_arn = created_function(&router, AUTHORIZER_FUNCTION, AUTHORIZER_SOURCE).await;
    let created = apigw(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "authorizer-trace-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let authorizer = apigw(
        &router,
        "CreateAuthorizer",
        "POST",
        &format!("/restapis/{api}/authorizers"),
        Some(json!({
            "name": "trace-authorizer",
            "type": "REQUEST",
            "authorizerUri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {authorizer_arn}/invocations"
            ),
            "identitySource": format!("method.request.header.{IDENTITY_HEADER}"),
            // Nothing is held for an identity, so every row's request really asks the
            // function rather than being answered from a result the stage kept.
            "authorizerResultTtlInSeconds": 0,
        })),
        201,
    )
    .await;
    let authorizer_id = member("CreateAuthorizer", &authorizer, "id");
    let resource = apigw(
        &router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource_id = member("CreateResource", &resource, "id");
    let method = format!("/restapis/{api}/resources/{resource_id}/methods/GET");
    apigw(
        &router,
        "PutMethod",
        "PUT",
        &method,
        Some(json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer_id })),
        201,
    )
    .await;
    apigw(
        &router,
        "PutIntegration",
        "PUT",
        &format!("{method}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function_arn}/invocations"
            ),
        })),
        201,
    )
    .await;
    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": TRACED_STAGE })),
        201,
    )
    .await;
    apigw(
        &router,
        "UpdateStage",
        "PATCH",
        &format!("/restapis/{api}/stages/{TRACED_STAGE}"),
        Some(json!({
            "patchOperations": [
                { "op": "replace", "path": "/tracingEnabled", "value": "true" }
            ]
        })),
        200,
    )
    .await;
    // (case, the tracing header the caller sends, the chain the request must run in)
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        (
            "a sampled chain the caller named",
            Some("Root=1-5759e988-bd862e3fe1be46a994272711;Sampled=1"),
            Some("1-5759e988-bd862e3fe1be46a994272711"),
        ),
        ("no header at all", None, None),
        (
            "a root that is not a trace id",
            Some("Root=not-a-trace-id;Sampled=1"),
            None,
        ),
    ];
    for (index, (case, sent, chain)) in rows.into_iter().enumerate() {
        let sub_id = format!("88bb44cc55dd66{index:02}");
        let mut headers: Vec<(&str, &str)> = vec![(IDENTITY_HEADER, "caller-token")];
        if let Some(sent) = sent {
            headers.push(("x-amzn-trace-id", sent));
        }
        let served = dispatch(
            &router,
            "GET",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            &format!("/{TRACED_STAGE}/{RESOURCE_PATH}?sub_id={sub_id}"),
            &headers,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            served.status,
            200,
            "case {case:?}: expected HTTP 200 from the guarded stage, got {} with {:?}",
            served.status,
            served.text()
        );
        let reported = served.json();
        let root = member(case, &reported, "root");
        if let Some(continued) = chain {
            assert_eq!(
                root, continued,
                "case {case:?}: expected the caller's chain continued, got {reported}"
            );
        }
        let authorizer_trace = member(case, &reported, "authorizer_trace");
        assert_eq!(
            ::service_common::xray_trace::trace_field(&authorizer_trace, "Root"),
            Some(root.as_str()),
            "case {case:?}: expected the authorizer's invocation in the same chain as the \
             integration's, got {reported}"
        );
        assert_eq!(
            ::service_common::xray_trace::trace_field(&authorizer_trace, "Sampled"),
            reported.get("sampled").and_then(Value::as_str),
            "case {case:?}: expected the authorizer's invocation to carry the same decision, \
             got {reported}"
        );
        // The authorizer's work hangs from the STAGE's own recorded segment, not from the
        // span of the stage's call out to the integration — so nothing in the trace names a
        // parent no document records.
        let held = documents_of(&router, &root).await;
        let segment = |origin: &str, name: &str| {
            held.iter()
                .find(|document| {
                    document.get("origin").and_then(Value::as_str) == Some(origin)
                        && document.get("name").and_then(Value::as_str) == Some(name)
                })
                .cloned()
                .unwrap_or(Value::Null)
        };
        let stage = held
            .iter()
            .find(|document| {
                document.get("origin").and_then(Value::as_str) == Some(STAGE_ORIGIN)
            })
            .cloned()
            .unwrap_or(Value::Null);
        let call_out = stage
            .get("subsegments")
            .and_then(Value::as_array)
            .and_then(|subsegments| subsegments.first())
            .cloned()
            .unwrap_or(Value::Null);
        let authorizer_service = segment(LAMBDA_ORIGIN, AUTHORIZER_FUNCTION);
        assert_eq!(
            authorizer_service.get("parent_id").and_then(Value::as_str),
            stage.get("id").and_then(Value::as_str),
            "case {case:?}: expected the authorizer's invocation to hang from the span the \
             stage's own segment records, got {held:?}"
        );
        assert_ne!(
            authorizer_service.get("parent_id").and_then(Value::as_str),
            call_out.get("id").and_then(Value::as_str),
            "case {case:?}: expected the authorizer's invocation to hang from the stage \
             itself rather than from its call out to the integration, got {held:?}"
        );
        // The integration's own invocation still hangs from that call out, so one request is
        // one tree with two branches.
        assert_eq!(
            segment(LAMBDA_ORIGIN, FUNCTION)
                .get("parent_id")
                .and_then(Value::as_str),
            call_out.get("id").and_then(Value::as_str),
            "case {case:?}: expected the integration's invocation to hang from the stage's \
             call out, got {held:?}"
        );
    }
}

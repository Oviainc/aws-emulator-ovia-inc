// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING pipe whose `Target` is a LAMBDA FUNCTION does, end to end over the wire
//! with pipes, Amazon SQS and Lambda mounted on ONE engine.
//!
//! The RULE asserted over the table below: how many calls a pipe's target stage makes for
//! one polled batch is decided by the target service's own API and never by the pipe — a
//! target whose API carries ONE payload per call receives one call per event of the batch,
//! and a target whose API takes a payload that can be the batch receives ONE call carrying
//! every event of it. A Lambda function's `Invoke` is the second kind, so a pipe hands its
//! function target one invocation per polled batch, with the batch as a JSON array of the
//! events the pipe would otherwise have delivered one by one — which is what makes the
//! source's `BatchSize` the count of invocations a backlog comes to — and the records it
//! polled leave the source queue once that invocation has taken them.
//!
//! Nothing in this suite invokes the function: the only data-plane calls a row makes are
//! the `SendMessage`s that put the records on the source queue. Every message it reads back
//! off the RESULT queue was sent by the handler's own `boto3` call out of the batch it was
//! handed, so a pipe that reports `RUNNING` and delivers nothing leaves that queue empty and
//! the row fails.
//!
//! The rows differ along what the rule generalises over: how many records are waiting, the
//! `BatchSize` they are polled under (one, a divisor of the count, the whole count and the
//! member's absence), whether the target carries an `InputTemplate`, which `InvocationType`
//! the target parameters name (`REQUEST_RESPONSE`, `FIRE_AND_FORGET` and the member's
//! absence) and whether the pipe's `FilterCriteria` drop one of the records.
//!
//! The member names and the invocation types are the model's own
//! (`smithy_query(query="shape", shape="PipeTargetParameters,PipeTargetLambdaFunctionParameters")`).

use std::collections::BTreeSet;
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{collect_at_least, given_queue, send, LOOK_AGAIN};
use crate::pipe_lifecycle::{call, Reply, REGION};

/// The role every pipe and function below names. Nothing in this suite turns on its
/// policies: no IAM store is mounted, so no stage is refused on the strength of one.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-lambda-target";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The runtime every function below runs, and the module and attribute its package exposes
/// its handler as.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long one invocation of the handler is given, as the function's own `Timeout`: well
/// past what a run needs, because what is under test is the delivery and not how fast an
/// interpreter gets through a handler.
const FUNCTION_TIMEOUT: i64 = 30;

/// How long a wait for work nothing asked for — the pipe settling, a batch polled, the
/// handler's own send landing — is given before the row is a failure. Well above the poll
/// interval, the state-settle span and one interpreted run of the handler, so a row fails
/// only when the work did not happen at all.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a row watches for an invocation it does not expect — a batch invoked twice,
/// because the records it took were not consumed. A pass that was going to invoke again
/// would have: it is several times the poll interval.
const QUIET: Duration = Duration::from_secs(2);

/// The environment variable the handler reads the result queue's url from, and the two
/// members it reports each invocation under. None of the three is read by the emulator:
/// they are the handler's own, passed to it and asserted here.
const RESULT_QUEUE_VARIABLE: &str = "RESULT_QUEUE";
/// See [`RESULT_QUEUE_VARIABLE`].
const SIZE_MEMBER: &str = "size";
/// See [`RESULT_QUEUE_VARIABLE`].
const EVENTS_MEMBER: &str = "events";

/// The `eventSource` an Amazon SQS record envelope reports.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// Pipes, Amazon SQS and Lambda on one engine, as `core/router/src/lib.rs` mounts them: the
/// records are sent where a client sends them, the pipe's target is a function Lambda holds,
/// and the SDK call the running handler makes is dispatched back into this same router — so
/// the queue this suite reads is the queue the handler wrote.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDPIPESTEST/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    (status, parsed)
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory
/// the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler every row runs: it reports the batch it was handed — how many events it held
/// and the events themselves — as ONE message on the result queue, through the SDK the
/// runtime supplies, and refuses a payload that is not the array a pipe hands a function
/// target.
///
/// So one message on the result queue is one invocation, and what it carries is what that
/// invocation was handed.
fn handler_source() -> String {
    format!(
        r#"
import json
import os

import boto3

RESULT_QUEUE = os.environ["{RESULT_QUEUE_VARIABLE}"]
CLIENT = boto3.client("sqs")


def lambda_handler(event, context):
    if not isinstance(event, list):
        raise ValueError("a batch is handed over as a list, got " + type(event).__name__)
    CLIENT.send_message(
        QueueUrl=RESULT_QUEUE,
        MessageBody=json.dumps({{"{SIZE_MEMBER}": len(event), "{EVENTS_MEMBER}": event}}),
    )
    return {{"{SIZE_MEMBER}": len(event)}}
"#
    )
}

/// Create the function `name` whose package runs [`handler_source`] against the result
/// queue at `result_url`, wait until it reports `Active` — a function a pipe can invoke —
/// and answer its ARN.
async fn given_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    result_url: &str,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        Some(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Environment": { "Variables": { RESULT_QUEUE_VARIABLE: result_url } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(&handler_source())),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    let arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {created}"))
        .to_string();
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            None,
        )
        .await;
        if status == 200 && read["State"] == json!("Active") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never became Active: {status} {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The markers one row sends: `<case>-0`, `<case>-1`, … — distinct per row and per record,
/// so an invocation can be attributed to the records it was handed.
fn markers(case: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{case}-{index}")).collect()
}

/// One row: the records waiting on the source, the `BatchSize` they are polled under, and
/// how the target is configured.
struct Row {
    case: &'static str,
    records: usize,
    batch_size: Option<i64>,
    /// `TargetParameters.InputTemplate`, when the row configures one.
    template: Option<&'static str>,
    /// `TargetParameters.LambdaFunctionParameters.InvocationType`, when the row names one.
    invocation_type: Option<&'static str>,
    /// `SourceParameters.FilterCriteria`'s one pattern, when the row carries one, and how
    /// many of the records it admits.
    filter: Option<&'static str>,
    admitted: usize,
}

/// The size of each batch a backlog of `records` already waiting is polled in, under a
/// `BatchSize` of `batch_size`: a poll takes as many as are waiting up to that ceiling, so
/// the batches are full until the last one.
fn batch_sizes(records: usize, batch_size: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let mut left = records;
    while left > 0 {
        let taken = left.min(batch_size);
        sizes.push(taken);
        left -= taken;
    }
    sizes
}

/// The `CreatePipe` body of one row.
fn pipe_body(row: &Row, source_arn: &str, target_arn: &str) -> Value {
    let mut queue_parameters = serde_json::Map::new();
    if let Some(batch_size) = row.batch_size {
        queue_parameters.insert("BatchSize".to_string(), json!(batch_size));
    }
    let mut source_parameters = json!({ "SqsQueueParameters": Value::Object(queue_parameters) });
    if let Some(pattern) = row.filter {
        source_parameters["FilterCriteria"] = json!({ "Filters": [{ "Pattern": pattern }] });
    }
    let mut target_parameters = serde_json::Map::new();
    if let Some(template) = row.template {
        target_parameters.insert("InputTemplate".to_string(), json!(template));
    }
    if let Some(invocation_type) = row.invocation_type {
        target_parameters.insert(
            "LambdaFunctionParameters".to_string(),
            json!({ "InvocationType": invocation_type }),
        );
    }
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": ROLE,
        "DesiredState": "RUNNING",
        "SourceParameters": source_parameters,
    });
    if !target_parameters.is_empty() {
        body["TargetParameters"] = Value::Object(target_parameters);
    }
    body
}

/// `CreatePipe`, answered as the lifecycle harness reads a reply.
async fn create_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

/// How many records the source queue still holds — the ones waiting on it and the ones a
/// consumer has taken and not deleted — waited out until it holds `want` of them.
async fn wait_for_held(router: &MultiServiceRouter, url: &str, want: i64, budget: Duration) -> i64 {
    let deadline = Instant::now() + budget;
    loop {
        let held = crate::pipe_data_plane::counted(
            router,
            url,
            &[
                "ApproximateNumberOfMessages",
                "ApproximateNumberOfMessagesNotVisible",
            ],
        )
        .await;
        if held == want || Instant::now() >= deadline {
            return held;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a pipe whose target is a Lambda function hands that function ONE invocation per
/// polled batch, carrying the batch as the array of events — so the source's `BatchSize`
/// decides how many invocations a backlog comes to, each invocation holds every event of
/// its own batch, and the records it took leave the source once the invocation has them.
#[tokio::test]
async fn a_lambda_function_target_is_invoked_once_per_polled_batch() {
    let rows = [
        Row {
            case: "one-record-default-batch-size",
            records: 1,
            batch_size: None,
            template: None,
            invocation_type: None,
            filter: None,
            admitted: 1,
        },
        Row {
            case: "three-records-one-invocation",
            records: 3,
            batch_size: Some(5),
            template: None,
            invocation_type: Some("REQUEST_RESPONSE"),
            filter: None,
            admitted: 3,
        },
        Row {
            case: "batch-size-one-invokes-per-record",
            records: 3,
            batch_size: Some(1),
            template: None,
            invocation_type: None,
            filter: None,
            admitted: 3,
        },
        Row {
            case: "five-records-batch-of-two",
            records: 5,
            batch_size: Some(2),
            template: None,
            invocation_type: None,
            filter: None,
            admitted: 5,
        },
        Row {
            case: "fire-and-forget-invocation",
            records: 2,
            batch_size: Some(10),
            template: None,
            invocation_type: Some("FIRE_AND_FORGET"),
            filter: None,
            admitted: 2,
        },
        Row {
            case: "an-input-template-shapes-each-event",
            records: 2,
            batch_size: Some(10),
            template: Some(r#"{"order": "<$.body>"}"#),
            invocation_type: None,
            filter: None,
            admitted: 2,
        },
        Row {
            case: "a-filter-admits-one-of-two-records",
            records: 2,
            batch_size: Some(10),
            template: None,
            invocation_type: None,
            // The body of the record the row admits, as an event pattern matches a body
            // that is not a JSON document: by its text.
            filter: Some(r#"{"body": ["a-filter-admits-one-of-two-records-0"]}"#),
            admitted: 1,
        },
    ];
    let router = emulator();
    for row in &rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (result_url, _) = given_queue(&router, &format!("{case}-result")).await;
        let target_arn = given_function(&router, case, case, &result_url).await;

        let sent = markers(case, row.records);
        send(&router, &source_url, &sent).await;

        let created = create_pipe(&router, case, pipe_body(row, &source_arn, &target_arn)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );

        // The batches the backlog is polled in, and therefore the invocations it comes to:
        // one per batch, whatever each batch holds.
        let batches = batch_sizes(
            row.records,
            row.batch_size.unwrap_or(10).try_into().unwrap_or(10),
        );
        let reported = collect_at_least(&router, &result_url, batches.len(), BUDGET).await;
        assert_eq!(
            reported.len(),
            batches.len(),
            "case {case:?}: expected one invocation per polled batch ({} of them, of sizes \
             {batches:?}), got {} ({reported:?})",
            batches.len(),
            reported.len()
        );
        // Nothing MORE is invoked: a batch whose records the pipe did not consume would be
        // polled and handed over again, which this watch would see.
        let again = crate::pipe_data_plane::collect_during(&router, &result_url, QUIET).await;
        assert!(
            again.is_empty(),
            "case {case:?}: expected each polled batch invoked exactly once, {} further \
             invocation(s) followed: {again:?}",
            again.len()
        );

        // What each invocation was handed: one array, holding the events of its own batch.
        let mut events: Vec<Value> = Vec::new();
        for report in &reported {
            let parsed: Value = serde_json::from_str(report).unwrap_or_else(|error| {
                panic!("case {case:?}: a report is not JSON ({error}): {report:?}")
            });
            let held = parsed[EVENTS_MEMBER].as_array().cloned().unwrap_or_default();
            assert_eq!(
                parsed[SIZE_MEMBER].as_u64().unwrap_or_default() as usize,
                held.len(),
                "case {case:?}: the handler reported a size its batch does not hold: {parsed}"
            );
            assert!(
                !held.is_empty() && held.len() <= batches[0],
                "case {case:?}: an invocation carried {} events, outside the 1..={} one \
                 batch of this pipe holds: {parsed}",
                held.len(),
                batches[0]
            );
            events.extend(held);
        }
        assert_eq!(
            events.len(),
            row.admitted,
            "case {case:?}: expected the invocations to carry every admitted record exactly \
             once ({} of them), got {} ({events:?})",
            row.admitted,
            events.len()
        );

        // Each event, as the pipe handed it over: the record envelope itself, or what the
        // target's input template renders from it.
        let mut bodies: BTreeSet<String> = BTreeSet::new();
        for event in &events {
            match row.template {
                None => {
                    assert_eq!(
                        event["eventSource"].as_str().unwrap_or_default(),
                        SQS_EVENT_SOURCE,
                        "case {case:?}: expected the Amazon SQS record envelope, got {event}"
                    );
                    assert_eq!(
                        event["eventSourceARN"].as_str().unwrap_or_default(),
                        source_arn,
                        "case {case:?}: expected the source ARN on the envelope, got {event}"
                    );
                    bodies.insert(event["body"].as_str().unwrap_or_default().to_string());
                }
                Some(_) => {
                    bodies.insert(event["order"].as_str().unwrap_or_default().to_string());
                }
            }
        }
        let expected: BTreeSet<String> = sent.iter().take(row.admitted).cloned().collect();
        assert_eq!(
            bodies, expected,
            "case {case:?}: expected the admitted records' own bodies, got {bodies:?}"
        );

        // Consumed: a batch an invocation took is deleted from the source, and a record the
        // filter dropped is consumed too, so the queue ends up holding nothing.
        let held = wait_for_held(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            held, 0,
            "case {case:?}: expected the source queue drained once the invocations took its \
             records, it still holds {held}"
        );
    }
}

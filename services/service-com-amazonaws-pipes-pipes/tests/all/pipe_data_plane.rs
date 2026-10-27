// SPDX-License-Identifier: CC0-1.0

//! The pipes DATA PLANE, end to end over the wire: what a pipe does with no client
//! action at all.
//!
//! Every call here goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — with SQS and Step Functions mounted on the SAME engine,
//! because a pipe's work is asserted where a client sees it: on the target queue, on the
//! source queue it drained, and in what the enrichment was handed.
//!
//! The three rules asserted, each over a TABLE whose rows differ along what the rule
//! generalises over (how many records, the batch size, the batching window, what the
//! enrichment returns, and the lifecycle state the pipe is in):
//!
//! 1. An Amazon SQS target receives ONE message per event handed to the target stage,
//!    never one message holding a JSON array — `SendMessage` takes one payload per call
//!    and `PipeTargetSqsQueueParameters` has no batch member.
//! 2. The records polled together are handed to the enrichment as ONE array, and the
//!    array the enrichment RETURNS — not the source batch — is what the target stage
//!    delivers: an aggregate collapses a batch into fewer deliveries than there were
//!    records, a fan-out produces more, and an empty answer produces none.
//! 3. A pipe consumes its source only while RUNNING: a `STOPPED` pipe's backlog stays
//!    visible and undeleted and its target receives nothing, and `StartPipe` resumes
//!    consumption of everything that accumulated.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{call, Reply, PAST_SETTLE, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-data-plane";

/// How long a wait for work the pipe has to do is given before it is a failure. Well
/// above the poll interval and the batching windows the rows configure, so a row fails
/// only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(20);

/// How long an assertion that NOTHING arrives watches for. A pipe that was going to
/// deliver would have: it is longer than the poll interval and than every batching
/// window below.
const QUIET: Duration = Duration::from_secs(2);

/// How long between two looks at a queue while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// An enrichment that collapses the whole batch it was handed into ONE event: how many
/// records it saw, and their text. `States.ArrayLength` and `States.JsonToString` are
/// documented Amazon States Language intrinsics, so the aggregate is computed by the
/// enrichment rather than by this test.
const AGGREGATE_DEFINITION: &str = r#"{
  "StartAt": "Aggregate",
  "States": {
    "Aggregate": {
      "Type": "Pass",
      "Parameters": {
        "batch_len.$": "States.ArrayLength($)",
        "batch.$": "States.JsonToString($)"
      },
      "End": true
    }
  }
}"#;

/// An enrichment that returns TWO events whatever it was handed, so the target stage's
/// fan-out is driven by the enrichment's answer and not by the source batch.
const FAN_OUT_DEFINITION: &str = r#"{
  "StartAt": "FanOut",
  "States": {
    "FanOut": {
      "Type": "Pass",
      "Result": [{"copy": 1}, {"copy": 2}],
      "End": true
    }
  }
}"#;

/// An enrichment that returns an EMPTY array, which suppresses the target invocation.
const SUPPRESS_DEFINITION: &str = r#"{
  "StartAt": "Suppress",
  "States": {
    "Suppress": {
      "Type": "Pass",
      "Result": [],
      "End": true
    }
  }
}"#;

/// Pipes, SQS and Step Functions on one engine: a pipe's source and target are queues
/// and its enrichment is an Express workflow, so all three are reached in-process.
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
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", operation, body).await
}

async fn sfn(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "states", "AWSStepFunctions", operation, body).await
}

/// A queue called `name`; answers its url and the ARN a pipe names it by.
async fn given_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// An Express state machine called `name` running `definition`; answers its ARN.
async fn given_express_workflow(
    router: &MultiServiceRouter,
    name: &str,
    definition: &str,
) -> String {
    let (status, created) = sfn(
        router,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition,
            "roleArn": ROLE,
            "type": "EXPRESS",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateStateMachine answered {status} {created}"
    );
    created["stateMachineArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no stateMachineArn in {created}"))
        .to_string()
}

/// Send `markers` to the queue at `url`, one message each.
async fn send(router: &MultiServiceRouter, url: &str, markers: &[String]) {
    for marker in markers {
        let (status, body) = sqs(
            router,
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": marker }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case send {marker}: SendMessage answered {status} {body}"
        );
    }
}

/// The markers one row sends: `<case>-0`, `<case>-1`, … — distinct per row and per
/// record, so a delivery can be attributed.
fn markers(case: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{case}-{index}")).collect()
}

/// Every message body waiting on the queue at `url`, collected until there are at least
/// `want` of them or `budget` has elapsed.
async fn collect_at_least(
    router: &MultiServiceRouter,
    url: &str,
    want: usize,
    budget: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + budget;
    let mut collected: Vec<String> = Vec::new();
    loop {
        collected.extend(receive_now(router, url).await);
        if collected.len() >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every message body that arrives at `url` during the whole of `budget`.
async fn collect_during(
    router: &MultiServiceRouter,
    url: &str,
    budget: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + budget;
    let mut collected: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        collected.extend(receive_now(router, url).await);
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    collected
}

/// One `ReceiveMessage` of up to ten messages, as bodies.
async fn receive_now(router: &MultiServiceRouter, url: &str) -> Vec<String> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `ApproximateNumberOfMessages` of the queue at `url`.
async fn visible(router: &MultiServiceRouter, url: &str) -> i64 {
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["ApproximateNumberOfMessages"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case attributes {url}: GetQueueAttributes answered {status} {body}"
    );
    body["Attributes"]["ApproximateNumberOfMessages"]
        .as_str()
        .unwrap_or_default()
        .parse()
        .unwrap_or(-1)
}

/// Wait until the queue at `url` reports `want` visible messages, answering what it
/// reported last.
async fn wait_for_visible(
    router: &MultiServiceRouter,
    url: &str,
    want: i64,
    budget: Duration,
) -> i64 {
    let deadline = Instant::now() + budget;
    loop {
        let seen = visible(router, url).await;
        if seen == want || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `CreatePipe`, answered as the lifecycle harness reads a reply.
async fn create_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// The `CreatePipe` body of one row: an SQS source and an SQS target, the row's batching
/// parameters, and its enrichment when it has one.
fn pipe_body(
    source_arn: &str,
    target_arn: &str,
    batch_size: Option<i64>,
    window: Option<i64>,
    enrichment: Option<&str>,
    desired_state: &str,
) -> Value {
    let mut queue_parameters = serde_json::Map::new();
    if let Some(batch_size) = batch_size {
        queue_parameters.insert("BatchSize".to_string(), json!(batch_size));
    }
    if let Some(window) = window {
        queue_parameters.insert(
            "MaximumBatchingWindowInSeconds".to_string(),
            json!(window),
        );
    }
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": ROLE,
        "DesiredState": desired_state,
        "SourceParameters": { "SqsQueueParameters": Value::Object(queue_parameters) },
    });
    if let Some(enrichment) = enrichment {
        body["Enrichment"] = json!(enrichment);
    }
    body
}

/// The record envelope a delivered body carries, or a panic naming what it carried
/// instead.
fn envelope(case: &str, body: &str) -> Value {
    let parsed: Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("case {case:?}: a delivered body is not JSON ({error}): {body:?}"));
    assert!(
        !parsed.is_array(),
        "case {case:?}: an SQS target receives ONE record per message, so no delivered \
         body may be a JSON array of records, got {body:?}"
    );
    parsed
}

/// One row of the target fan-out table.
struct FanOut {
    case: &'static str,
    records: usize,
    batch_size: Option<i64>,
    window: Option<i64>,
}

/// RULE: every record a pipe polls becomes its OWN target message. The batch size and
/// the batching window bound the POLL, not the target invocation, so N source records
/// become N single-record messages at an SQS target and never one message holding an
/// array.
///
/// The rows differ along what the rule generalises over: how many records are sent, the
/// batch size they are polled under (one, a divisor of the count, and the whole count)
/// and whether a batching window lets a batch span several receives.
#[tokio::test]
async fn an_sqs_target_receives_one_message_per_source_record() {
    let rows = [
        FanOut {
            case: "ten-records-one-batch",
            records: 10,
            batch_size: Some(10),
            window: Some(1),
        },
        FanOut {
            case: "three-records-no-batching",
            records: 3,
            batch_size: Some(1),
            window: None,
        },
        FanOut {
            case: "five-records-batch-of-three",
            records: 5,
            batch_size: Some(3),
            window: Some(1),
        },
        FanOut {
            case: "two-records-default-parameters",
            records: 2,
            batch_size: None,
            window: None,
        },
    ];
    let router = emulator();
    for row in rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let sent = markers(case, row.records);
        send(&router, &source_url, &sent).await;

        let created = create_pipe(
            &router,
            case,
            pipe_body(
                &source_arn,
                &target_arn,
                row.batch_size,
                row.window,
                None,
                "RUNNING",
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );

        let delivered = collect_at_least(&router, &target_url, row.records, BUDGET).await;
        assert_eq!(
            delivered.len(),
            row.records,
            "case {case:?}: expected one target message per source record \
             ({} of them), got {} ({delivered:?})",
            row.records,
            delivered.len()
        );
        let mut bodies: BTreeSet<String> = BTreeSet::new();
        for body in &delivered {
            let record = envelope(case, body);
            assert_eq!(
                record["eventSource"].as_str().unwrap_or_default(),
                "aws:sqs",
                "case {case:?}: expected the Amazon SQS record envelope, got {record}"
            );
            assert_eq!(
                record["eventSourceARN"].as_str().unwrap_or_default(),
                source_arn,
                "case {case:?}: expected the source ARN on the envelope, got {record}"
            );
            bodies.insert(record["body"].as_str().unwrap_or_default().to_string());
        }
        let expected: BTreeSet<String> = sent.iter().cloned().collect();
        assert_eq!(
            bodies, expected,
            "case {case:?}: expected every sent record delivered exactly once, got {bodies:?}"
        );
        let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected a delivered record to be deleted from the source, \
             {left} are still visible"
        );
    }
}

/// What a row's enrichment is and what it does to the batch.
enum Enriched {
    /// Collapses the batch into one aggregate event.
    Aggregate,
    /// Returns two events whatever it was handed.
    FanOut,
    /// Returns an empty array.
    Suppress,
}

/// One row of the enrichment table.
struct EnrichmentCase {
    case: &'static str,
    records: usize,
    batch_size: i64,
    window: Option<i64>,
    enrichment: Enriched,
    /// How many target messages the enrichment's answer drives.
    expected_deliveries: usize,
}

/// RULE: the records polled together are handed to the enrichment as ONE JSON array of
/// record envelopes, and the array the enrichment RETURNS — not the source batch — is
/// what the target stage delivers.
///
/// The rows differ along what that rule generalises over: how many records are polled
/// together, and whether the enrichment returns fewer events than it was handed (an
/// aggregate), more (a fan-out), or none at all (a suppressed target).
#[tokio::test]
async fn the_enrichment_sees_the_whole_batch_and_its_answer_drives_the_target() {
    let rows = [
        EnrichmentCase {
            case: "aggregate-of-six",
            records: 6,
            batch_size: 6,
            window: Some(2),
            enrichment: Enriched::Aggregate,
            // Fewer deliveries than records: how many is the enrichment's business, so
            // the row asserts the bound and the records' coverage below.
            expected_deliveries: 1,
        },
        EnrichmentCase {
            case: "aggregate-per-record",
            records: 3,
            batch_size: 1,
            window: None,
            enrichment: Enriched::Aggregate,
            expected_deliveries: 3,
        },
        EnrichmentCase {
            case: "fan-out-two-per-batch",
            records: 2,
            batch_size: 1,
            window: None,
            enrichment: Enriched::FanOut,
            expected_deliveries: 4,
        },
        EnrichmentCase {
            case: "empty-answer-suppresses-the-target",
            records: 3,
            batch_size: 1,
            window: None,
            enrichment: Enriched::Suppress,
            expected_deliveries: 0,
        },
    ];
    let router = emulator();
    for row in rows {
        let case = row.case;
        let definition = match row.enrichment {
            Enriched::Aggregate => AGGREGATE_DEFINITION,
            Enriched::FanOut => FAN_OUT_DEFINITION,
            Enriched::Suppress => SUPPRESS_DEFINITION,
        };
        let enrichment =
            given_express_workflow(&router, &format!("{case}-enrichment"), definition).await;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let sent = markers(case, row.records);
        send(&router, &source_url, &sent).await;

        let created = create_pipe(
            &router,
            case,
            pipe_body(
                &source_arn,
                &target_arn,
                Some(row.batch_size),
                row.window,
                Some(&enrichment),
                "RUNNING",
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );

        // The source is drained whatever the enrichment returned: a record the
        // enrichment consumed is delivered or deliberately dropped, never left behind.
        let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected the enrichment to consume every source record, \
             {left} are still visible"
        );

        let delivered = if row.expected_deliveries == 0 {
            collect_during(&router, &target_url, QUIET).await
        } else {
            let mut delivered =
                collect_at_least(&router, &target_url, row.expected_deliveries, BUDGET).await;
            // Nothing beyond what the enrichment's answer asked for.
            delivered.extend(collect_during(&router, &target_url, QUIET).await);
            delivered
        };
        assert_eq!(
            delivered.len(),
            row.expected_deliveries,
            "case {case:?}: expected {} target message(s) driven by the enrichment's \
             answer, got {} ({delivered:?})",
            row.expected_deliveries,
            delivered.len()
        );

        if matches!(row.enrichment, Enriched::Aggregate) {
            let mut counted = 0usize;
            let mut widest = 0usize;
            let mut covered: BTreeSet<String> = BTreeSet::new();
            for body in &delivered {
                let aggregate = envelope(case, body);
                let batch_len = aggregate["batch_len"].as_u64().unwrap_or_default() as usize;
                let batch = aggregate["batch"].as_str().unwrap_or_default().to_string();
                assert!(
                    batch_len >= 1,
                    "case {case:?}: an aggregate must report the batch it saw, got {aggregate}"
                );
                let records: Vec<Value> = serde_json::from_str(&batch).unwrap_or_default();
                assert_eq!(
                    records.len(),
                    batch_len,
                    "case {case:?}: the enrichment was handed ONE array of {batch_len} \
                     record envelopes, got {batch:?}"
                );
                for record in &records {
                    assert_eq!(
                        record["eventSource"].as_str().unwrap_or_default(),
                        "aws:sqs",
                        "case {case:?}: the enrichment is handed record envelopes, got {record}"
                    );
                    covered.insert(record["body"].as_str().unwrap_or_default().to_string());
                }
                counted += batch_len;
                widest = widest.max(batch_len);
            }
            assert_eq!(
                counted, row.records,
                "case {case:?}: expected the aggregates to account for all {} records, \
                 they account for {counted}",
                row.records
            );
            let expected: BTreeSet<String> = sent.iter().cloned().collect();
            assert_eq!(
                covered, expected,
                "case {case:?}: expected every sent record in an aggregate, got {covered:?}"
            );
            if row.batch_size > 1 {
                assert!(
                    widest > 1,
                    "case {case:?}: a batch size of {} must hand the enrichment several \
                     records at once, the widest aggregate saw {widest}",
                    row.batch_size
                );
                assert!(
                    delivered.len() < row.records,
                    "case {case:?}: an aggregating enrichment must drive FEWER target \
                     messages than there were source records, got {} for {}",
                    delivered.len(),
                    row.records
                );
            }
        }
    }
}

/// One row of the lifecycle table: the state a pipe is created in, and whether it
/// consumes its source there.
struct Lifecycle {
    case: &'static str,
    desired_state: &'static str,
    consumes: bool,
}

/// RULE: a pipe consumes its source only while it is RUNNING. A `STOPPED` pipe polls
/// nothing — its backlog stays visible and undeleted and its target receives nothing —
/// and `StartPipe` resumes consumption of everything that accumulated while it was
/// stopped.
///
/// The rows differ along the state the pipe is in, which is what the rule turns on: one
/// pipe created `RUNNING` (the differential twin, which proves the records would have
/// moved) and one created `STOPPED` on its own source and target. The stopped pipe is
/// then started, and stopped again with a fresh backlog, so both directions of the
/// boundary are asserted.
#[tokio::test]
async fn a_stopped_pipe_consumes_nothing_and_start_pipe_drains_its_backlog() {
    let rows = [
        Lifecycle {
            case: "running-twin",
            desired_state: "RUNNING",
            consumes: true,
        },
        Lifecycle {
            case: "stopped-pipe",
            desired_state: "STOPPED",
            consumes: false,
        },
    ];
    let router = emulator();
    let mut held: Option<(String, String)> = None;
    for row in rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let sent = markers(case, 5);

        let created = create_pipe(
            &router,
            case,
            pipe_body(
                &source_arn,
                &target_arn,
                Some(1),
                None,
                None,
                row.desired_state,
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        // Past the span the pipe takes to reach the state it was asked for, so the row
        // is asserted against a settled pipe.
        tokio::time::sleep(PAST_SETTLE).await;
        send(&router, &source_url, &sent).await;

        if row.consumes {
            let delivered = collect_at_least(&router, &target_url, sent.len(), BUDGET).await;
            let bodies: BTreeSet<String> = delivered
                .iter()
                .map(|body| {
                    envelope(case, body)["body"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect();
            let expected: BTreeSet<String> = sent.iter().cloned().collect();
            assert_eq!(
                bodies, expected,
                "case {case:?}: a RUNNING pipe delivers its source records, got {bodies:?}"
            );
        } else {
            let delivered = collect_during(&router, &target_url, QUIET).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a STOPPED pipe's target receives nothing, got {delivered:?}"
            );
            let still = visible(&router, &source_url).await;
            assert_eq!(
                still,
                i64::try_from(sent.len()).unwrap_or_default(),
                "case {case:?}: a STOPPED pipe leaves its backlog visible, {still} of {} \
                 are visible",
                sent.len()
            );
            held = Some((source_url.clone(), target_url.clone()));
        }
    }

    // `StartPipe` on the stopped pipe: the whole held backlog reaches the target.
    let (source_url, target_url) = held.expect("the stopped row recorded its queues");
    let started = call(&router, "POST", "/v1/pipes/stopped-pipe/start", None, REGION).await;
    assert_eq!(
        started.status, 200,
        "case start: StartPipe answered {} {}",
        started.status, started.body
    );
    let held_markers = markers("stopped-pipe", 5);
    let drained = collect_at_least(&router, &target_url, held_markers.len(), BUDGET).await;
    let bodies: BTreeSet<String> = drained
        .iter()
        .map(|body| {
            envelope("start", body)["body"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let expected: BTreeSet<String> = held_markers.iter().cloned().collect();
    assert_eq!(
        bodies, expected,
        "case start: StartPipe drains the backlog held while the pipe was STOPPED, \
         got {bodies:?}"
    );
    let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
    assert_eq!(
        left, 0,
        "case start: a drained backlog is deleted from the source, {left} are still visible"
    );

    // And back: a pipe stopped again consumes nothing more.
    let stopped = call(&router, "POST", "/v1/pipes/stopped-pipe/stop", None, REGION).await;
    assert_eq!(
        stopped.status, 200,
        "case stop: StopPipe answered {} {}",
        stopped.status, stopped.body
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let after = markers("after-stop", 3);
    send(&router, &source_url, &after).await;
    let delivered = collect_during(&router, &target_url, QUIET).await;
    assert!(
        delivered.is_empty(),
        "case stop: a pipe stopped again consumes nothing, got {delivered:?}"
    );
    let still = visible(&router, &source_url).await;
    assert_eq!(
        still,
        i64::try_from(after.len()).unwrap_or_default(),
        "case stop: the records sent after the stop stay visible, {still} of {} are",
        after.len()
    );
}

/// The enrichment template the rows below configure: an object built from two of the
/// record's own values, which is what proves the enrichment saw the render and not the
/// record.
const RECORD_TEMPLATE: &str = r#"{"marker": "<$.body>", "source": "<$.eventSource>"}"#;

/// RULE: a stage's `InputTemplate` is what that stage is handed INSTEAD of the event —
/// "nothing from the event itself is passed" — so an enrichment configured with one sees
/// the array of payloads the template rendered, not the record envelopes, and a target
/// configured with one receives the payload its own template rendered.
///
/// The rows differ along which stage carries a template and what it renders (an object
/// built from two of the record's values, the record's body alone, a value of the
/// enrichment's answer), with the no-template row as the differential twin. The
/// enrichment is the same documented aggregate used above, so what it reports is what it
/// was handed and not what this test assumed.
#[tokio::test]
async fn an_input_template_is_what_the_stage_it_belongs_to_is_handed() {
    // (case, whether the pipe has an enrichment, the enrichment's template, the target's
    // template)
    let rows: Vec<(&str, bool, Option<&str>, Option<&str>)> = vec![
        ("no-template", true, None, None),
        ("enrichment-template", true, Some(RECORD_TEMPLATE), None),
        ("target-template", false, None, Some("<$.body>")),
        (
            "both-templates",
            true,
            Some(RECORD_TEMPLATE),
            Some("<$.batch_len>"),
        ),
    ];
    let router = emulator();
    for (case, has_enrichment, enrichment_template, target_template) in rows {
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
        let sent = markers(case, 2);
        send(&router, &source_url, &sent).await;

        let mut body = json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 2 } },
        });
        if has_enrichment {
            let enrichment = given_express_workflow(
                &router,
                &format!("{case}-enrichment"),
                AGGREGATE_DEFINITION,
            )
            .await;
            body["Enrichment"] = json!(enrichment);
        }
        if let Some(template) = enrichment_template {
            body["EnrichmentParameters"] = json!({ "InputTemplate": template });
        }
        if let Some(template) = target_template {
            body["TargetParameters"] = json!({ "InputTemplate": template });
        }
        let created = create_pipe(&router, case, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );

        let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
        let first = delivered
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: nothing was delivered"));

        match target_template {
            // No target template: the target receives the enrichment's answer itself,
            // which reports the array the enrichment was handed.
            None => {
                let aggregate = envelope(case, first);
                let handed = aggregate["batch"].as_str().unwrap_or_default().to_string();
                let records: Vec<Value> = serde_json::from_str(&handed).unwrap_or_default();
                assert_eq!(
                    records.len(),
                    sent.len(),
                    "case {case:?}: the enrichment must be handed one payload per record, \
                     got {handed:?}"
                );
                for record in &records {
                    match enrichment_template {
                        // The template's render, and nothing else from the record.
                        Some(_) => {
                            assert!(
                                record["marker"].is_string()
                                    && record.get("messageId").is_none()
                                    && record.get("attributes").is_none(),
                                "case {case:?}: the enrichment must be handed what its \
                                 template renders and nothing from the record itself, \
                                 got {record}"
                            );
                            assert!(
                                sent.contains(
                                    &record["marker"].as_str().unwrap_or_default().to_string()
                                ),
                                "case {case:?}: a placeholder must carry the record's own \
                                 value, got {record}"
                            );
                            assert_eq!(
                                record["source"].as_str().unwrap_or_default(),
                                "aws:sqs",
                                "case {case:?}: the template's placeholders must carry the \
                                 record's values, got {record}"
                            );
                        }
                        // No template: the record envelope, as before.
                        None => assert_eq!(
                            record["eventSource"].as_str().unwrap_or_default(),
                            "aws:sqs",
                            "case {case:?}: with no template the enrichment is handed the \
                             record envelope, got {record}"
                        ),
                    }
                }
            }
            // A target template: the body is exactly what it renders for the event the
            // target stage was handed.
            Some("<$.body>") => assert!(
                sent.contains(first),
                "case {case:?}: expected the record's body alone, got {first:?}"
            ),
            // `<$.batch_len>` reads a value of the aggregate the enrichment returned.
            Some(_) => assert_eq!(
                first,
                &sent.len().to_string(),
                "case {case:?}: expected the aggregate's batch length rendered by the \
                 target template, got {first:?}"
            ),
        }

        let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected every delivered record deleted from the source, \
             {left} are still visible"
        );
    }
}

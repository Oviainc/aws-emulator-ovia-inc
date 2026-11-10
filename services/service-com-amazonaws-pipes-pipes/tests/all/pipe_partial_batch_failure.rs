// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING pipe does with the report its TARGET answers about the batch it was
//! handed, end to end over the wire with pipes, Amazon SQS and Lambda mounted on ONE
//! engine.
//!
//! The RULE asserted over the table below: a target whose call answers a document of its own
//! reports which records of the batch it did NOT process — a `batchItemFailures` list naming
//! each by the identifier its source names it by — and the pipe finishes the batch with its
//! source only for the records that report leaves processed, so exactly the records the
//! report names are handed over again once their checkout lapses, while a report that cannot
//! be read as one about the records of THAT batch (an identifier the batch does not hold,
//! or every record of it) leaves nothing of the batch processed and the whole of it is handed
//! over again.
//!
//! The rows differ along what the rule generalises over: how many records the batch holds,
//! which of them the report names (none, the first, the last, two of four, every one of
//! them), whether the report also names an identifier that is no record of the batch, and
//! the `BatchSize` the records are polled under.
//!
//! Nothing in this suite invokes the function and nothing receives from a source queue: the
//! only data-plane calls a row makes are the `SendMessage`s that put its records on its own
//! source queue. Every message read off the RESULT queue was sent by the handler's own
//! `boto3` call out of the batch it was handed, so one message there is one invocation and
//! what it carries is the records that invocation was handed — which is how a row counts the
//! deliveries of each record.
//!
//! The report's members are the ones AWS publishes for it (`batchItemFailures`,
//! `itemIdentifier`), and the identifier of an Amazon SQS record is its message id; the
//! delivery count the handler reads a record's first delivery off is Amazon SQS's own
//! `ApproximateReceiveCount` ("the number of times a message has been received across all
//! queues but not deleted", `ReceiveMessage$MessageSystemAttributeNames`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{collect_during, sqs, LOOK_AGAIN};
use crate::pipe_lambda_target::{
    emulator, given_function, RESULT_QUEUE_VARIABLE as RESULT_QUEUE,
};
use crate::pipe_lifecycle::{call, Reply, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies: no IAM
/// store is mounted, so no stage is refused on the strength of one.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-partial-batch-failure";

/// How long a wait for work nothing asked for — the pipe settling, a batch polled, the
/// handler's own send landing, a reported record coming back — is given before the row is a
/// failure. Well above the poll interval, the state-settle span, one interpreted run of the
/// handler and the source queue's own visibility timeout, so a row fails only when the work
/// did not happen at all.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a row watches for a delivery it does not expect — a record handed over again
/// that the target reported it had processed. Comfortably past the source queue's visibility
/// timeout, so a record that was going to come back would have.
const QUIET: Duration = Duration::from_secs(5);

/// The `VisibilityTimeout` each SOURCE queue is created with, in seconds: how long after a
/// poll the records that poll took are hidden from the queue's consumers, and therefore how
/// long a record the target reported waits before the pipe is handed it again. This is the
/// SUITE's choice — short, so a row does not spend the default thirty seconds waiting out a
/// checkout, and long enough that a batch is finished with its source well before the
/// records it holds come back.
const SOURCE_VISIBILITY: &str = "5";

/// The `VisibilityTimeout` the RESULT queue is created with, in seconds: long enough that a
/// report this suite has read can never come back and be counted twice, because a row counts
/// invocations by the reports it has received.
const RESULT_VISIBILITY: &str = "3600";

/// What a record's body carries to make the handler report it as a record it did not process.
const FAIL_MARKER: &str = "FAIL";

/// What a record's body carries to make the handler ALSO report an identifier that is no
/// record of the batch at all.
const UNKNOWN_MARKER: &str = "UNKNOWN";

/// The identifier the handler reports for such a record: a message id no queue minted, so it
/// names no record of any batch.
const UNKNOWN_IDENTIFIER: &str = "00000000-0000-4000-8000-000000000000";

/// The environment variables the handler reads those three from. None of them is read by the
/// emulator: they are the handler's own.
const FAIL_MARKER_VARIABLE: &str = "FAIL_MARKER";
/// See [`FAIL_MARKER_VARIABLE`].
const UNKNOWN_MARKER_VARIABLE: &str = "UNKNOWN_MARKER";
/// See [`FAIL_MARKER_VARIABLE`].
const UNKNOWN_IDENTIFIER_VARIABLE: &str = "UNKNOWN_IDENTIFIER";

/// The member the handler reports the bodies of the batch it was handed under.
const BODIES_MEMBER: &str = "bodies";

/// The handler every row runs: it reports the batch it was handed — the body of each record
/// of it — as ONE message on the result queue, through the SDK the runtime supplies, and
/// answers the report of the records it did not process, which it reads off those bodies.
///
/// A record is reported only on its FIRST delivery, which it knows from Amazon SQS's own
/// delivery count, so a record handed over again is processed and the row's source queue
/// drains instead of the pipe redelivering for ever.
fn handler_source() -> String {
    format!(
        r#"
import json
import os

import boto3

RESULT_QUEUE = os.environ["{RESULT_QUEUE}"]
FAIL_MARKER = os.environ["{FAIL_MARKER_VARIABLE}"]
UNKNOWN_MARKER = os.environ["{UNKNOWN_MARKER_VARIABLE}"]
UNKNOWN_IDENTIFIER = os.environ["{UNKNOWN_IDENTIFIER_VARIABLE}"]
CLIENT = boto3.client("sqs")


def lambda_handler(event, context):
    if not isinstance(event, list):
        raise ValueError("a batch is handed over as a list, got " + type(event).__name__)
    CLIENT.send_message(
        QueueUrl=RESULT_QUEUE,
        MessageBody=json.dumps({{"{BODIES_MEMBER}": [record["body"] for record in event]}}),
    )
    failures = []
    for record in event:
        if not record["messageId"]:
            raise ValueError("a record carries no messageId")
        if record["attributes"]["ApproximateReceiveCount"] != "1":
            continue
        if FAIL_MARKER in record["body"]:
            failures.append({{"itemIdentifier": record["messageId"]}})
        if UNKNOWN_MARKER in record["body"]:
            failures.append({{"itemIdentifier": UNKNOWN_IDENTIFIER}})
    return {{"batchItemFailures": failures}}
"#
    )
}

/// A queue called `name` whose `VisibilityTimeout` is `visibility` seconds; answers its url
/// and the ARN a pipe names it by.
async fn given_queue(
    router: &MultiServiceRouter,
    name: &str,
    visibility: &str,
) -> (String, String) {
    let (status, body) = sqs(
        router,
        "CreateQueue",
        &json!({
            "QueueName": name,
            "Attributes": { "VisibilityTimeout": visibility },
        }),
    )
    .await;
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

/// Send each of `bodies` to the queue at `url`, one message each.
async fn send(router: &MultiServiceRouter, case: &str, url: &str, bodies: &[String]) {
    for body in bodies {
        let (status, answered) = sqs(
            router,
            "SendMessage",
            &json!({ "QueueUrl": url, "MessageBody": body }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SendMessage of {body:?} answered {status} {answered}"
        );
    }
}

/// One row: the records waiting on the source, which of them the report names, and the
/// `BatchSize` they are polled under.
struct Row {
    case: &'static str,
    /// What each record of the row carries, as a suffix of its body: the empty string for a
    /// record the handler processes, [`FAIL_MARKER`] for one it reports, and
    /// [`UNKNOWN_MARKER`] for one whose report also names an identifier no record of the
    /// batch has.
    records: &'static [&'static str],
    /// `SourceParameters.SqsQueueParameters.BatchSize`, when the row names one.
    batch_size: Option<i64>,
    /// Which of the records must be handed over a SECOND time, by their index: the ones the
    /// report names, or every one of them for a report that leaves nothing processed.
    redelivered: &'static [usize],
}

impl Row {
    /// The body of each record this row sends: `<case>-<index>` for a record the handler
    /// processes, with the marker that makes the handler report it appended for the others.
    /// Distinct per row and per record, so a delivery can be attributed to the record it
    /// carried.
    fn bodies(&self) -> Vec<String> {
        self.records
            .iter()
            .enumerate()
            .map(|(index, marker)| {
                let case = self.case;
                if marker.is_empty() {
                    format!("{case}-{index}")
                } else {
                    format!("{case}-{index}-{marker}")
                }
            })
            .collect()
    }

    /// How many times each record of this row must be handed to the target.
    fn expected(&self) -> BTreeMap<String, usize> {
        self.bodies()
            .into_iter()
            .enumerate()
            .map(|(index, body)| {
                let times = if self.redelivered.contains(&index) { 2 } else { 1 };
                (body, times)
            })
            .collect()
    }
}

/// The `CreatePipe` body of one row.
fn pipe_body(row: &Row, source_arn: &str, target_arn: &str) -> Value {
    let mut queue_parameters = serde_json::Map::new();
    if let Some(batch_size) = row.batch_size {
        queue_parameters.insert("BatchSize".to_string(), json!(batch_size));
    }
    json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": ROLE,
        "DesiredState": "RUNNING",
        "SourceParameters": { "SqsQueueParameters": Value::Object(queue_parameters) },
    })
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

/// One `ReceiveMessage` of up to ten reports off the result queue, each read as the bodies of
/// the batch ONE invocation was handed.
async fn invocations(router: &MultiServiceRouter, url: &str) -> Vec<Vec<String>> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receiving the reports answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let text = message["Body"].as_str().unwrap_or_default();
            let report: Value = serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("a report is not JSON ({error}): {text:?}"));
            report[BODIES_MEMBER]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|body| body.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .collect()
}

/// The invocations the row `case` was answered, collected off the result queue until they
/// carry `want` deliveries in all or `budget` has elapsed.
///
/// Only the batches holding THIS row's records are kept, so a report another row's pipe wrote
/// is never counted here.
async fn delivered(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    want: usize,
    budget: Duration,
) -> Vec<Vec<String>> {
    let deadline = Instant::now() + budget;
    let mut collected: Vec<Vec<String>> = Vec::new();
    loop {
        for batch in invocations(router, url).await {
            if batch.iter().any(|body| body.starts_with(case)) {
                collected.push(batch);
            }
        }
        let carried: usize = collected.iter().map(Vec::len).sum();
        if carried >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// How many records the source queue still holds — the ones waiting on it and the ones a
/// consumer has taken and not deleted — waited out until it holds `want` of them.
async fn held(router: &MultiServiceRouter, url: &str, want: i64, budget: Duration) -> i64 {
    crate::pipe_data_plane::wait_for_counted(
        router,
        url,
        &[
            "ApproximateNumberOfMessages",
            "ApproximateNumberOfMessagesNotVisible",
        ],
        want,
        budget,
    )
    .await
}

/// RULE: the records of a batch a pipe is finished with its source for are the ones its
/// target reported it PROCESSED — every record of the batch when the target's answer names
/// none, every record but the ones a `batchItemFailures` names, and none of them when that
/// report names an identifier the batch does not hold or every record of it — so exactly the
/// records the report leaves unprocessed are handed over again and the rest are consumed.
#[tokio::test]
async fn only_the_records_the_target_reported_are_handed_over_again() {
    let rows = [
        Row {
            case: "no-record-reported",
            records: &["", "", ""],
            batch_size: Some(10),
            redelivered: &[],
        },
        Row {
            case: "one-of-three-reported",
            records: &["", FAIL_MARKER, ""],
            batch_size: Some(10),
            redelivered: &[1],
        },
        Row {
            case: "two-of-four-reported",
            records: &[FAIL_MARKER, "", "", FAIL_MARKER],
            batch_size: Some(10),
            redelivered: &[0, 3],
        },
        Row {
            case: "every-record-reported",
            records: &[FAIL_MARKER, FAIL_MARKER],
            batch_size: Some(10),
            redelivered: &[0, 1],
        },
        Row {
            case: "an-unrecognized-identifier",
            records: &["", UNKNOWN_MARKER, ""],
            batch_size: Some(10),
            redelivered: &[0, 1, 2],
        },
        Row {
            case: "one-record-per-batch",
            records: &[FAIL_MARKER, ""],
            batch_size: Some(1),
            redelivered: &[0],
        },
    ];
    let router = emulator();
    // ONE function and ONE result queue serve every row: the handler reads which records to
    // report off the record's own body, so a row differs only in what it sends and how it
    // configures its pipe.
    let (result_url, _) = given_queue(&router, "partial-batch-result", RESULT_VISIBILITY).await;
    let target_arn = given_function(
        &router,
        "partial-batch",
        "partial-batch-reporter",
        &handler_source(),
        json!({
            RESULT_QUEUE: result_url,
            FAIL_MARKER_VARIABLE: FAIL_MARKER,
            UNKNOWN_MARKER_VARIABLE: UNKNOWN_MARKER,
            UNKNOWN_IDENTIFIER_VARIABLE: UNKNOWN_IDENTIFIER,
        }),
    )
    .await;

    for row in &rows {
        let case = row.case;
        let (source_url, source_arn) =
            given_queue(&router, &format!("{case}-source"), SOURCE_VISIBILITY).await;
        let bodies = row.bodies();
        send(&router, case, &source_url, &bodies).await;

        let created = create_pipe(&router, case, pipe_body(row, &source_arn, &target_arn)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );

        let expected = row.expected();
        let want: usize = expected.values().sum();
        let batches = delivered(&router, case, &result_url, want, BUDGET).await;
        // Nothing MORE is handed over: a record the target reported processed, offered again,
        // would show up here.
        for batch in collect_during(&router, &result_url, QUIET).await {
            let report: Value = serde_json::from_str(&batch)
                .unwrap_or_else(|error| panic!("case {case:?}: a report is not JSON ({error})"));
            let carried: Vec<String> = report[BODIES_MEMBER]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|body| body.as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                !carried.iter().any(|body| body.starts_with(case)),
                "case {case:?}: expected no further delivery once the reported records had \
                 been handed over again, got {carried:?}"
            );
        }

        let mut counted: BTreeMap<String, usize> = BTreeMap::new();
        for batch in &batches {
            assert!(
                !batch.is_empty(),
                "case {case:?}: an invocation was handed an empty batch ({batches:?})"
            );
            for body in batch {
                *counted.entry(body.clone()).or_default() += 1;
            }
            let distinct: std::collections::BTreeSet<&String> = batch.iter().collect();
            assert_eq!(
                distinct.len(),
                batch.len(),
                "case {case:?}: one batch carried a record twice: {batch:?}"
            );
        }
        assert_eq!(
            counted, expected,
            "case {case:?}: expected each record handed over {expected:?} time(s), got \
             {counted:?} (the invocations were {batches:?})"
        );

        // Consumed: once the records the report named have been handed over again and
        // processed, the source queue holds nothing at all.
        let left = held(&router, &source_url, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected the source queue drained once every record had been \
             processed, it still holds {left}"
        );
    }
}

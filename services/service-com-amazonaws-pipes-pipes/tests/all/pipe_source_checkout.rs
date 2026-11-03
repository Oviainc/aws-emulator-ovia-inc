// SPDX-License-Identifier: CC0-1.0

//! What a pipe's Amazon SQS SOURCE looks like to the OTHER consumers of that queue while
//! the pipe is retrying a delivery it cannot make — end to end over the wire, with IAM,
//! CloudWatch, Amazon SQS and Step Functions mounted beside pipes on ONE engine (the
//! harness of `tests/all/pipe_metrics.rs` and `tests/all/pipe_data_plane.rs`, reused
//! rather than copied).
//!
//! The RULE asserted over the table below: a polled batch is a CHECKOUT of the source
//! queue, and a pipe is one consumer of that queue among others. The records a pipe's poll
//! took are invisible to every other consumer for the queue's own `VisibilityTimeout` —
//! `ApproximateNumberOfMessagesNotVisible` counts them and no other `ReceiveMessage`
//! returns them — and a delivery the pipe cannot make leaves them undeleted, so when that
//! span lapses they return to the visible set, their `ApproximateReceiveCount` raised
//! ("the number of times a message has been received across all queues but not deleted"),
//! and another consumer can receive them: the pipe paces its retries so the release is
//! something that consumer can see, rather than re-claiming the records the instant they
//! reappear. Nothing reaches the target in the meantime.
//!
//! The rows differ along what the rule generalises over: the `VisibilityTimeout` the queue
//! is configured with (which is the span of the checkout, and is never this emulator's own
//! number), how many records one batch checks out, and the `BatchSize` they are polled
//! under.

use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::{RUNNING, STOPPED};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    counted, sqs, wait_for_counted, LOOK_AGAIN, QUIET, VISIBLE_ATTRIBUTE,
};
use crate::pipe_execution_role::wait_for_state;
use crate::pipe_lifecycle::{call, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};

/// How long a wait for something the pipe or the queue owes is given before the row is a
/// failure. Well above the data plane's poll interval and the span a pipe takes to reach a
/// lifecycle state, so a row fails only when the work did not happen at all.
const BUDGET: Duration = Duration::from_secs(20);

/// The attribute that counts the records a consumer has taken and not deleted: "messages
/// that are in flight" (`GetQueueAttributes$AttributeNames`).
const IN_FLIGHT_ATTRIBUTE: &str = "ApproximateNumberOfMessagesNotVisible";

/// The message system attribute a released record is read by: "the number of times a
/// message has been received across all queues but not deleted"
/// (`ReceiveMessage$MessageSystemAttributeNames`, `ApproximateReceiveCount`).
const RECEIVE_COUNT_ATTRIBUTE: &str = "ApproximateReceiveCount";

/// How many times a row looks for a record that is still checked out, to show that no
/// other consumer can take it while the pipe holds it.
const RECEIVES_WHILE_HELD: usize = 3;

/// How long the records a pipe could not deliver must stay the queue's own once they have
/// come back: the span another consumer of that queue — a second worker, a monitoring
/// probe, a dead-letter policy — has to see the release and act on it. A pipe that
/// re-claimed them inside its own poll interval would leave no such span at all.
const OBSERVABLE_RELEASE: Duration = Duration::from_secs(1);

/// How often the consumer below looks at the source: the pace a client polls a queue at,
/// not that of a tight loop.
const CONSUMER_INTERVAL: Duration = Duration::from_millis(500);

/// One row of the checkout table.
struct Row {
    /// The case, which is also the pipe's and the queues' name.
    case: &'static str,
    /// The `VisibilityTimeout` the SOURCE queue is created with, in seconds, as the
    /// attribute carries it — the span of the checkout, which is the queue's own number
    /// and not this emulator's.
    visibility: &'static str,
    /// The bodies the row sends to the source, in send order.
    messages: &'static [&'static str],
    /// The `BatchSize` the pipe polls under.
    batch_size: i64,
}

/// RULE: a record a pipe polled is checked out of its source for the queue's own
/// `VisibilityTimeout` — counted in flight, invisible to every other consumer — and a
/// delivery the pipe's role forbids leaves it undeleted, so it comes back when that span
/// lapses with its `ApproximateReceiveCount` raised and another consumer receives it,
/// while the target queue stays empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_a_pipe_cannot_deliver_is_checked_out_then_released_to_the_queue() {
    let rows = [
        Row {
            case: "one-record-held-for-a-second",
            visibility: "1",
            messages: &["held-1"],
            batch_size: 1,
        },
        Row {
            case: "one-record-held-for-three-seconds",
            visibility: "3",
            messages: &["held-3"],
            batch_size: 1,
        },
        Row {
            case: "two-records-in-one-batch",
            visibility: "2",
            messages: &["held-batch-a", "held-batch-b"],
            batch_size: 2,
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// Keep the table honest: every row is driven by this one body.
async fn run_row(row: &Row) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) =
        given_queue_holding_for(&router, case, &format!("{case}-src"), row.visibility).await;
    let (target_url, target_arn) = given_queue_holding_for(&router, case, &format!("{case}-tgt"), "30").await;

    // The permissions an Amazon SQS pipe source needs, and nothing else: the pipe may
    // take the source's records, delete them and read the queue's own attributes, and it
    // may not send to the target — so every delivery it attempts is refused.
    let role = given_role_stating(
        &router,
        &format!("{case}-role"),
        SERVICE_PRINCIPAL,
        json!([{
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
            "Resource": source_arn,
        }]),
    )
    .await;

    // Created STOPPED, written to, then started: the records are on the queue before the
    // pipe polls at all, so one poll checks the whole batch out.
    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": role,
            "DesiredState": "STOPPED",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": row.batch_size } },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    let reported = wait_for_state(&router, case, STOPPED, BUDGET).await;
    assert_eq!(
        reported, STOPPED,
        "case {case:?}: expected the pipe to rest STOPPED before it is written to, it \
         reports {reported}"
    );

    for body in row.messages {
        send(&router, &source_url, case, body).await;
    }
    let started = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}/start"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartPipe answered {} {}",
        started.status, started.body
    );
    let reported = wait_for_state(&router, case, RUNNING, BUDGET).await;
    assert_eq!(
        reported, RUNNING,
        "case {case:?}: expected the pipe RUNNING, it reports {reported}"
    );

    // The checkout, as the queue reports it to anyone who asks: every record of the batch
    // is in flight, and none is visible.
    let count = i64::try_from(row.messages.len()).unwrap_or_default();
    let in_flight = wait_for_counted(
        &router,
        &source_url,
        &[IN_FLIGHT_ATTRIBUTE],
        count,
        BUDGET,
    )
    .await;
    assert_eq!(
        in_flight, count,
        "case {case:?}: expected the {count} record(s) the pipe polled to be in flight on \
         its source, it reports {in_flight}"
    );
    let visible = counted(&router, &source_url, &[VISIBLE_ATTRIBUTE]).await;
    assert_eq!(
        visible, 0,
        "case {case:?}: expected no record of the source visible while the pipe holds the \
         batch, it reports {visible}"
    );

    // And as every OTHER consumer of that queue sees it: a receive is an atomic checkout,
    // so none of them can take a record the pipe is holding.
    for attempt in 0..RECEIVES_WHILE_HELD {
        let held = receive_with_counts(&router, &source_url, case).await;
        assert!(
            held.is_empty(),
            "case {case:?}: receive {attempt} while the pipe holds the batch must return \
             nothing, got {held:?}"
        );
    }

    // The delivery the role forbids leaves the records undeleted, so the lapse of their
    // visibility timeout returns them to the QUEUE: they are visible again, and they stay
    // the queue's own for a span another consumer can act in rather than being re-claimed
    // the instant they reappear.
    let back = wait_for_counted(&router, &source_url, &[VISIBLE_ATTRIBUTE], count, BUDGET).await;
    assert_eq!(
        back, count,
        "case {case:?}: expected the {count} record(s) the pipe could not deliver to come \
         back to the visible set of its source, it reports {back}"
    );
    tokio::time::sleep(OBSERVABLE_RELEASE).await;
    let still = counted(&router, &source_url, &[VISIBLE_ATTRIBUTE]).await;
    assert_eq!(
        still, count,
        "case {case:?}: expected the released record(s) to still be the queue's own \
         {OBSERVABLE_RELEASE:?} later, it reports {still} visible"
    );

    // And this consumer — one that polls its queue at a client's pace, not in a tight
    // loop — takes them, with the delivery count they came back with.
    let released = collect_released(&router, &source_url, case, row.messages.len()).await;
    let mut bodies: Vec<String> = released.iter().map(|(body, _)| body.clone()).collect();
    bodies.sort();
    let mut expected: Vec<String> = row.messages.iter().map(|body| (*body).to_string()).collect();
    expected.sort();
    assert_eq!(
        bodies, expected,
        "case {case:?}: expected the released record(s) {expected:?} on the source, got \
         {released:?}"
    );
    for (body, received) in &released {
        assert!(
            *received > 1,
            "case {case:?}: expected the released record {body:?} to report an \
             {RECEIVE_COUNT_ATTRIBUTE} above 1, got {received}"
        );
    }

    // And the target the pipe could not reach holds nothing at all.
    let delivered = counted(&router, &target_url, &[VISIBLE_ATTRIBUTE, IN_FLIGHT_ATTRIBUTE]).await;
    assert_eq!(
        delivered, 0,
        "case {case:?}: expected the target {target_arn} to hold nothing, it holds \
         {delivered}"
    );
}

/// A queue called `name` whose `VisibilityTimeout` is `visibility` seconds — "the length
/// of time, in seconds, that a message received from a queue will be invisible to other
/// receiving components" (`CreateQueue$Attributes`); answers its url and the ARN a pipe
/// names it by.
async fn given_queue_holding_for(
    router: &MultiServiceRouter,
    case: &str,
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
        "case {case:?}: CreateQueue of {name} answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn", "VisibilityTimeout"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes of {name} answered {status} {body}"
    );
    assert_eq!(
        body["Attributes"]["VisibilityTimeout"],
        json!(visibility),
        "case {case:?}: expected {name} to carry the visibility timeout it was created \
         with, got {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Send one body to the queue at `url`.
async fn send(router: &MultiServiceRouter, url: &str, case: &str, body: &str) {
    let (status, answer) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage of {body:?} answered {status} {answer}"
    );
}

/// ONE `ReceiveMessage` as a consumer of the queue that is not the pipe: each message it
/// returned, with the delivery count that message reports.
async fn receive_with_counts(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
) -> Vec<(String, i64)> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageSystemAttributeNames": [RECEIVE_COUNT_ATTRIBUTE],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let received = message["Attributes"][RECEIVE_COUNT_ATTRIBUTE]
                .as_str()
                .unwrap_or_default()
                .parse()
                .unwrap_or(-1);
            (
                message["Body"].as_str().unwrap_or_default().to_string(),
                received,
            )
        })
        .collect()
}

/// Keep receiving from `url` until `want` records have come back or the budget runs out:
/// what another consumer of the queue reads once the pipe's checkout has lapsed.
async fn collect_released(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    want: usize,
) -> Vec<(String, i64)> {
    let deadline = Instant::now() + BUDGET;
    let mut collected: Vec<(String, i64)> = Vec::new();
    loop {
        collected.extend(receive_with_counts(router, url, case).await);
        if collected.len() >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(CONSUMER_INTERVAL).await;
    }
}

/// RULE: the same checkout holds for a pipe whose delivery SUCCEEDS — the records are in
/// flight while it moves them and deleted when it has, so no other consumer ever receives
/// them and the queue is left empty rather than holding records that come back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_a_pipe_delivers_is_deleted_rather_than_released() {
    let case = "delivered-records-leave-the-source";
    let router = emulator();
    let (source_url, source_arn) =
        given_queue_holding_for(&router, case, &format!("{case}-src"), "1").await;
    let (target_url, target_arn) =
        given_queue_holding_for(&router, case, &format!("{case}-tgt"), "30").await;
    let role = given_role_stating(
        &router,
        &format!("{case}-role"),
        SERVICE_PRINCIPAL,
        json!([
            {
                "Effect": "Allow",
                "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                "Resource": source_arn,
            },
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target_arn },
        ]),
    )
    .await;
    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": role,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    send(&router, &source_url, case, "moved-1").await;

    // The delivery, read where a client sees it.
    let deadline = Instant::now() + BUDGET;
    let mut delivered: Vec<(String, i64)> = Vec::new();
    while delivered.is_empty() && Instant::now() < deadline {
        delivered.extend(receive_with_counts(&router, &target_url, case).await);
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected the record on the target {target_arn}, got {delivered:?}"
    );

    // The source is finished with: the record was deleted, not left to come back, so
    // nothing of it is on the queue however long its visibility timeout has had to lapse.
    let held = wait_for_counted(
        &router,
        &source_url,
        &[VISIBLE_ATTRIBUTE, IN_FLIGHT_ATTRIBUTE],
        0,
        BUDGET,
    )
    .await;
    assert_eq!(
        held, 0,
        "case {case:?}: expected the delivered record deleted from the source, which holds \
         {held}"
    );
    tokio::time::sleep(QUIET).await;
    let back = receive_with_counts(&router, &source_url, case).await;
    assert!(
        back.is_empty(),
        "case {case:?}: a deleted record never comes back, got {back:?}"
    );
}

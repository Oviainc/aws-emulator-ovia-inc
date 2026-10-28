// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the whole dead-letter recovery cycle, driven over the wire
//! (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! Two rules are under test.
//!
//! The cycle: a queue whose `RedrivePolicy` names a dead-letter target hands a message
//! over at most `maxReceiveCount` times and then moves it there, and a message movement
//! task started over that dead-letter queue is sized at the depth it had when the task
//! took its messages, advances from `RUNNING` to `COMPLETED`, accounts for every message it
//! took in `ApproximateNumberOfMessagesMoved`, makes each one deliverable on the
//! destination queue and leaves the queue it moved them out of empty and delivering
//! nothing.
//!
//! The reporting: each member of a reported task is populated only in the condition the
//! model documents it for — `TaskHandle` "only ... for tasks that are in RUNNING status",
//! `DestinationArn` only when the start request specified one, and
//! `MaxNumberOfMessagesPerSecond` only when it fixed a rate, while `FailureReason` is
//! "only included if the task status is FAILED".
//!
//! Each table spans the class its rule generalises over rather than one example: the cycle
//! rows walk the number of poison messages against the delivery allowance that
//! dead-letters them and both forms of the movement rate and of the destination, and the
//! reporting rows walk every status a task this store records reaches (`RUNNING`,
//! `COMPLETED`, `CANCELLING`, `CANCELLED`) crossed with both forms of the destination and
//! of the rate.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::support::{
    attribute_count as count, call, create_ok, dead_letter_queue, emulator, expected_arn,
    messages, ok, send_all,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the state the clock owes — the redrive of a message whose
/// deliveries are spent, or a task's own schedule — before giving up. Far longer than any
/// row's own schedule, so a slow machine cannot fail a row the emulator is still working
/// through.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long a polling loop pauses between two attempts.
const PAUSE: Duration = Duration::from_millis(25);

/// The statuses of the status set a task reaches here: "Possible values are: RUNNING,
/// COMPLETED, CANCELLING, CANCELLED, and FAILED"
/// (`ListMessageMoveTasksResultEntry$Status`).
const RUNNING: &str = "RUNNING";
const COMPLETED: &str = "COMPLETED";
const CANCELLING: &str = "CANCELLING";
const CANCELLED: &str = "CANCELLED";

/// One row of the cycle table: the case, how many poison messages are sent, the
/// `maxReceiveCount` the source queue's `RedrivePolicy` allows, the
/// `MaxNumberOfMessagesPerSecond` the move task is started at, and whether the start names
/// the destination queue (rather than leaving the messages to be "redriven back to their
/// respective original source queues").
type CycleRow = (&'static str, usize, i64, Option<i64>, bool);

/// One row of the reporting table: the case, how many messages the dead-letter queue
/// holds, the rate the task is started at, and whether the start names the destination.
type ReportingRow = (&'static str, usize, Option<i64>, bool);

/// The most recent `ListMessageMoveTasks` entry for `source_arn`.
async fn task_entry(router: &MultiServiceRouter, source_arn: &str) -> Value {
    let reply = ok(
        router,
        "ListMessageMoveTasks",
        json!({"SourceArn": source_arn, "MaxResults": 10}),
    )
    .await;
    reply
        .get("Results")
        .and_then(Value::as_array)
        .and_then(|results| results.first().cloned())
        .unwrap_or_else(|| panic!("case the listing of {source_arn}: no task entry in {reply}"))
}

/// The most recent task's entry for `source_arn` once it reports a status other than
/// `RUNNING`.
async fn settled_entry(router: &MultiServiceRouter, case: &str, source_arn: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let entry = task_entry(router, source_arn).await;
        if entry["Status"].as_str() != Some(RUNNING) {
            return entry;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected the task to settle within {DEADLINE:?}, last saw {entry}"
        );
        tokio::time::sleep(PAUSE).await;
    }
}

/// One entry's `long` member. `ApproximateNumberOfMessagesMoved` carries a modelled default
/// of 0, which the wire may leave out, so an absent member reads as that default.
fn number(entry: &Value, member: &str) -> i64 {
    entry.get(member).and_then(Value::as_i64).unwrap_or(0)
}

/// Assert every member whose population the model makes conditional: the handle is
/// reported exactly while the task is `RUNNING`, the destination ARN exactly when the start
/// request specified one, the rate exactly when it fixed one, and a task that has not
/// failed carries no reason.
fn assert_conditional_members(
    case: &str,
    entry: &Value,
    started_handle: &str,
    destination_arn: Option<&str>,
    rate: Option<i64>,
) {
    let status = entry["Status"].as_str().unwrap_or_default().to_string();
    let reported_handle = entry.get("TaskHandle").and_then(Value::as_str);
    if status == RUNNING {
        assert_eq!(
            reported_handle,
            Some(started_handle),
            "case {case}: expected a RUNNING task to report the handle its start answered \
             with, got {entry}"
        );
    } else {
        assert!(
            reported_handle.is_none(),
            "case {case}: expected a task in {status} to report no handle, got {entry}"
        );
    }
    assert_eq!(
        entry.get("DestinationArn").and_then(Value::as_str),
        destination_arn,
        "case {case}: expected the destination the start specified, got {entry}"
    );
    assert_eq!(
        entry.get("MaxNumberOfMessagesPerSecond").and_then(Value::as_i64),
        rate,
        "case {case}: expected the rate the start fixed, got {entry}"
    );
    assert!(
        entry.get("FailureReason").is_none(),
        "case {case}: expected a task in {status} to report no failure reason, got {entry}"
    );
}

/// Start a move task over `source_arn`, with the destination and rate a row asks for.
async fn start_task(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    destination_arn: Option<&str>,
    rate: Option<i64>,
) -> String {
    let mut request = json!({"SourceArn": source_arn});
    if let Some(arn) = destination_arn {
        request["DestinationArn"] = json!(arn);
    }
    if let Some(rate) = rate {
        request["MaxNumberOfMessagesPerSecond"] = json!(rate);
    }
    let reply = ok(router, "StartMessageMoveTask", request).await;
    let handle = reply["TaskHandle"].as_str().unwrap_or_default().to_string();
    assert!(
        !handle.is_empty(),
        "case {case}: expected a TaskHandle, got {reply}"
    );
    handle
}

/// Receive from the queue at `url` and DELETE what it hands over, until `wanted` bodies
/// have come across or the deadline passes, answering the bodies in delivery order.
///
/// A row that reads a queue whose messages become visible again would otherwise count one
/// message twice; deleting each delivery is also what ends a redriven message's journey.
async fn drain_bodies(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    deadline: Instant,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::with_capacity(wanted);
    while collected.len() < wanted && Instant::now() < deadline {
        let reply = ok(
            router,
            "ReceiveMessage",
            json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
        )
        .await;
        for message in messages(&reply) {
            collected.push(message["Body"].as_str().unwrap_or_default().to_string());
            ok(
                router,
                "DeleteMessage",
                json!({
                    "QueueUrl": url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
            )
            .await;
        }
    }
    collected
}

/// Receive from the queue at `url` without taking anything: what it hands over on a peek.
async fn peek(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 1}),
    )
    .await;
    messages(&reply)
}

/// The whole recovery path: poison messages fall to the dead-letter queue once their
/// deliveries are spent, and a move task over that queue completes, accounts for every
/// message it took, redelivers each one on the destination and leaves the dead-letter queue
/// empty.
#[tokio::test]
async fn a_move_task_completes_and_drains_every_dead_letter_queue_it_is_started_over() {
    // (case, poison messages sent, maxReceiveCount, MaxNumberOfMessagesPerSecond, whether
    // the start names the destination). The rows walk the message count against the
    // delivery allowance that dead-letters them, crossed with both forms of the rate ("if
    // this field is left blank, the system will optimize the rate") and both forms of the
    // destination.
    let rows: &[CycleRow] = &[
        ("one poison message, one delivery allowed, redriven back", 1, 1, None, false),
        ("two poison messages, one delivery allowed, a named destination", 2, 1, Some(1), true),
        ("three poison messages, two deliveries allowed, redriven back", 3, 2, None, false),
        (
            "four poison messages, three deliveries allowed, at the maximum rate",
            4,
            3,
            Some(500),
            true,
        ),
    ];

    for (index, (case, held, max_receive_count, rate, name_destination)) in
        rows.iter().enumerate()
    {
        let router = emulator();
        let prefix = format!("cycle-{index}");
        let dead_letter_name = format!("{prefix}-dlq");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &json!({})).await;
        let dead_letter_arn = expected_arn(&dead_letter_name);
        // A visibility timeout of zero hands a message straight back to the next receive,
        // so a row spends its allowance without waiting one out.
        let source_url = create_ok(
            &router,
            &format!("{prefix}-source"),
            &json!({
                "VisibilityTimeout": "0",
                "RedrivePolicy": json!({
                    "deadLetterTargetArn": dead_letter_arn,
                    "maxReceiveCount": max_receive_count,
                })
                .to_string(),
            }),
        )
        .await;
        let (destination_url, destination_arn) = if *name_destination {
            let name = format!("{prefix}-destination");
            (
                create_ok(&router, &name, &json!({})).await,
                Some(expected_arn(&name)),
            )
        } else {
            (source_url.clone(), None)
        };

        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        send_all(&router, &source_url, &bodies).await;
        let held_count = i64::try_from(*held).expect("a small count");

        // Spend each message's allowance: the source queue hands it over
        // `maxReceiveCount` times and then moves it to the dead-letter queue.
        let deadline = Instant::now() + DEADLINE;
        while count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await < held_count {
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the {held_count} messages dead-lettered after \
                 {max_receive_count} deliveries each within {DEADLINE:?}"
            );
            peek(&router, &source_url).await;
        }
        assert_eq!(
            (
                count(&router, &source_url, "ApproximateNumberOfMessages").await,
                count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            ),
            (0, held_count),
            "case {case}: expected the source queue drained into the dead-letter queue"
        );

        let handle = start_task(
            &router,
            case,
            &dead_letter_arn,
            destination_arn.as_deref(),
            *rate,
        )
        .await;
        let entry = settled_entry(&router, case, &dead_letter_arn).await;
        assert_eq!(
            entry["Status"].as_str(),
            Some(COMPLETED),
            "case {case}: expected the task to settle at {COMPLETED}, got {entry}"
        );
        assert_conditional_members(
            case,
            &entry,
            &handle,
            destination_arn.as_deref(),
            *rate,
        );
        // "This number is obtained at the time of starting the message movement task"
        // (ListMessageMoveTasksResultEntry$ApproximateNumberOfMessagesToMove): the depth
        // the dead-letter queue had when the task took its messages.
        assert_eq!(
            number(&entry, "ApproximateNumberOfMessagesToMove"),
            held_count,
            "case {case}: expected the {held_count} messages the task was started over, got \
             {entry}"
        );
        assert!(
            number(&entry, "ApproximateNumberOfMessagesMoved") >= held_count,
            "case {case}: expected at least the {held_count} messages taken to have moved, \
             got {entry}"
        );

        // Every body is deliverable on the destination again, and the queue the task moved
        // them out of is left empty and delivering nothing.
        let mut collected = drain_bodies(
            &router,
            &destination_url,
            bodies.len(),
            Instant::now() + DEADLINE,
        )
        .await;
        collected.sort();
        let mut expected = bodies.clone();
        expected.sort();
        assert_eq!(
            collected, expected,
            "case {case}: expected every body deliverable on the destination queue, got \
             {collected:?}"
        );
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: expected the drained dead-letter queue to hold nothing"
        );
        let left = peek(&router, &dead_letter_url).await;
        assert!(
            left.is_empty(),
            "case {case}: expected the drained dead-letter queue to deliver nothing, got \
             {left:?}"
        );
    }
}

/// A reported task populates each conditional member only in the condition the model
/// documents it for, in the status it is in when the listing is read: while it runs, and
/// once it has completed.
#[tokio::test]
async fn a_reported_task_populates_each_member_only_where_it_is_documented() {
    // (case, messages the dead-letter queue holds, MaxNumberOfMessagesPerSecond, whether
    // the start names the destination). The rows walk the message count against both forms
    // of the rate and both forms of the destination.
    let rows: &[ReportingRow] = &[
        ("an empty queue, no destination named, no rate fixed", 0, None, false),
        ("one message, a destination named, no rate fixed", 1, None, true),
        ("four messages at one per second, no destination named", 4, Some(1), false),
        ("four messages at one per second, a destination named", 4, Some(1), true),
        ("two messages at the maximum rate, a destination named", 2, Some(500), true),
    ];

    for (index, (case, held, rate, name_destination)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("members-{index}");
        let (dead_letter_url, dead_letter_arn, _) = dead_letter_queue(&router, &prefix).await;
        let destination_arn = if *name_destination {
            let name = format!("{prefix}-destination");
            create_ok(&router, &name, &json!({})).await;
            Some(expected_arn(&name))
        } else {
            None
        };

        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        send_all(&router, &dead_letter_url, &bodies).await;
        let held_count = i64::try_from(*held).expect("a small count");

        let handle = start_task(
            &router,
            case,
            &dead_letter_arn,
            destination_arn.as_deref(),
            *rate,
        )
        .await;

        // Read while the task may still be running: whichever status it is in, the handle
        // follows that status and the other members follow the request.
        let running = task_entry(&router, &dead_letter_arn).await;
        assert!(
            [RUNNING, COMPLETED].contains(&running["Status"].as_str().unwrap_or_default()),
            "case {case}: expected {RUNNING} or {COMPLETED} straight after the start, got \
             {running}"
        );
        assert_conditional_members(
            case,
            &running,
            &handle,
            destination_arn.as_deref(),
            *rate,
        );
        assert_eq!(
            number(&running, "ApproximateNumberOfMessagesToMove"),
            held_count,
            "case {case}: expected the size the task was started over, got {running}"
        );

        let settled = settled_entry(&router, case, &dead_letter_arn).await;
        assert_eq!(
            settled["Status"].as_str(),
            Some(COMPLETED),
            "case {case}: expected the task to settle at {COMPLETED}, got {settled}"
        );
        assert_conditional_members(
            case,
            &settled,
            &handle,
            destination_arn.as_deref(),
            *rate,
        );
        assert_eq!(
            (
                number(&settled, "ApproximateNumberOfMessagesMoved"),
                number(&settled, "ApproximateNumberOfMessagesToMove"),
            ),
            (held_count, held_count),
            "case {case}: expected the completed task to account for every message it took, \
             got {settled}"
        );
    }
}

/// The same reporting rule under the cancelled statuses: a task a cancel stopped is past
/// `RUNNING`, so it reports no handle while it is `CANCELLING` and none once it is
/// `CANCELLED`, and the destination it was started with is reported throughout.
#[tokio::test]
async fn a_cancelled_task_reports_no_handle_in_either_cancelled_status() {
    // (case, whether the start names the destination). Enough messages at the slowest rate
    // the member admits that the task is still running for seconds, so the cancel lands
    // while it has messages left to move.
    let rows: &[(&str, bool)] = &[
        ("a cancelled task redriving messages back", false),
        ("a cancelled task moving messages to a named destination", true),
    ];

    for (index, (case, name_destination)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("cancelled-members-{index}");
        let (dead_letter_url, dead_letter_arn, _) = dead_letter_queue(&router, &prefix).await;
        let destination_arn = if *name_destination {
            let name = format!("{prefix}-destination");
            create_ok(&router, &name, &json!({})).await;
            Some(expected_arn(&name))
        } else {
            None
        };
        let bodies: Vec<String> = (0..8).map(|number| format!("{case} #{number}")).collect();
        send_all(&router, &dead_letter_url, &bodies).await;

        let rate = Some(1);
        let handle = start_task(
            &router,
            case,
            &dead_letter_arn,
            destination_arn.as_deref(),
            rate,
        )
        .await;
        let (status, parsed) = call(
            &router,
            "CancelMessageMoveTask",
            &json!({"TaskHandle": handle}),
        )
        .await;
        if status != 200 {
            // "A message movement can only be cancelled when the current status is
            // RUNNING": on a machine slow enough that the whole schedule elapsed first,
            // the completed task is what the rule has to hold for instead.
            let entry = settled_entry(&router, case, &dead_letter_arn).await;
            assert_conditional_members(
                case,
                &entry,
                &handle,
                destination_arn.as_deref(),
                rate,
            );
            continue;
        }

        let cancelling = task_entry(&router, &dead_letter_arn).await;
        let reported = cancelling["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            [CANCELLING, CANCELLED].contains(&reported.as_str()),
            "case {case}: expected {CANCELLING} or {CANCELLED} after the cancel, got \
             {cancelling}, with the cancel answering {parsed}"
        );
        assert_conditional_members(
            case,
            &cancelling,
            &handle,
            destination_arn.as_deref(),
            rate,
        );

        let deadline = Instant::now() + DEADLINE;
        let settled = loop {
            let entry = task_entry(&router, &dead_letter_arn).await;
            if entry["Status"].as_str() == Some(CANCELLED) {
                break entry;
            }
            assert!(
                Instant::now() < deadline,
                "case {case}: expected the task to settle at {CANCELLED} within {DEADLINE:?}, \
                 last saw {entry}"
            );
            tokio::time::sleep(PAUSE).await;
        };
        assert_conditional_members(
            case,
            &settled,
            &handle,
            destination_arn.as_deref(),
            rate,
        );
    }
}

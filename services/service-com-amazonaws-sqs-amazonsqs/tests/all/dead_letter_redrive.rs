// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for dead-letter redrive, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: a queue whose `RedrivePolicy` names a dead-letter target hands one
//! of its messages over at most `maxReceiveCount` times — "the number of times a message
//! is delivered to the source queue before being moved to the dead-letter queue" — and
//! then moves it to that target, where it is visible at once and keeps its id, its body
//! and its cross-queue receive count, so the source queue's
//! `ApproximateNumberOfMessages` falls as the target's rises. A message still in flight
//! with a consumer, one whose deliveries have not run out, and one in a queue carrying no
//! such policy all stay where they are. The dead-letter queue a redrive fills is then what
//! a message movement task drains back: that task settles at `COMPLETED`, reports the
//! number of messages it was started over and the number it has moved, and leaves the
//! dead-letter queue empty.
//!
//! Each table spans the class the rule generalises over rather than one example: the
//! redrive rows walk the delivery allowance (1, 2 and 3 deliveries) crossed with the
//! message count (one and several) and both spellings a JSON count takes (a number and a
//! string of digits), and the rows that must NOT redrive walk every way a message's
//! allowance can still be unspent (no policy at all, deliveries left, in flight with a
//! consumer).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::support::{
    attribute_count as count, create_ok, emulator, expected_arn, message_counts, messages, ok,
    send_all,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a loop waits for the state the clock and the emulator owe — the messages a
/// redrive moves, or a movement task's completion — before giving up. Far longer than any
/// row's own schedule, so a slow machine cannot fail a row the emulator is still working
/// through.
const DEADLINE: Duration = Duration::from_secs(20);

/// The `RedrivePolicy` attribute value driving a queue's messages to `target_arn` after
/// `max_receive_count` deliveries. The count is passed as the JSON value a row spells it
/// with, so the table can walk both a number and a string of digits — which is why this
/// suite builds the policy here rather than through `super::support::redrives_to`, whose
/// count is fixed and which creates the queue it wires at the same time.
fn redrive_policy(target_arn: &str, max_receive_count: &Value) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": max_receive_count}).to_string()
}

/// One received message's `(MessageId, Body, ApproximateReceiveCount)`, as a receive that
/// selected that system attribute reports them.
fn received(message: &Value) -> (String, String, i64) {
    (
        message["MessageId"].as_str().unwrap_or_default().to_string(),
        message["Body"].as_str().unwrap_or_default().to_string(),
        message
            .get("Attributes")
            .and_then(|attributes| attributes.get("ApproximateReceiveCount"))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(-1),
    )
}

/// Receive up to ten messages from the queue at `url`, reporting each one's receive count.
///
/// `super::support::receive_until` is the shared loop for collecting messages a queue is
/// owed, but it selects no system attribute and reports no receipt handle; this suite
/// asserts the receive count each delivery reports and deletes what it collects, so it
/// drives one receive at a time and keeps the reply.
async fn receive_batch(router: &MultiServiceRouter, url: &str, wait_seconds: i64) -> Vec<Value> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": wait_seconds,
            "MessageSystemAttributeNames": ["ApproximateReceiveCount"],
        }),
    )
    .await;
    messages(&reply)
}

/// A message is handed over as many times as its queue's `RedrivePolicy` allows and then
/// moved to the dead-letter queue that policy names, with its identity and its cross-queue
/// receive count intact.
#[tokio::test]
async fn a_message_is_dead_lettered_once_its_queue_has_delivered_it_as_often_as_allowed() {
    // (case, the JSON `maxReceiveCount` the policy carries, the deliveries it allows, how
    // many messages are sent). The rows walk the allowance against the message count, and
    // the last row spells the same count as a string of digits — either spelling says the
    // same thing about the queue, and both bound the deliveries the same way.
    let rows: &[(&str, Value, i64, usize)] = &[
        ("one delivery allowed, one message", json!(1), 1, 1),
        ("one delivery allowed, three messages", json!(1), 1, 3),
        ("two deliveries allowed, two messages", json!(2), 2, 2),
        ("three deliveries allowed, one message", json!(3), 3, 1),
        (
            "a count spelled as a string of digits",
            json!("2"),
            2,
            2,
        ),
    ];

    for (index, (case, count_value, allowed, held)) in rows.iter().enumerate() {
        let router = emulator();
        let dead_letter_name = format!("redrive-{index}-dlq");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &json!({})).await;
        // A visibility timeout of zero hands a message straight back to the next receive,
        // so the row drives the deliveries without waiting out a timeout.
        let source_url = create_ok(
            &router,
            &format!("redrive-{index}-source"),
            &json!({
                "VisibilityTimeout": "0",
                "RedrivePolicy": redrive_policy(&expected_arn(&dead_letter_name), count_value),
            }),
        )
        .await;

        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        let sent = send_all(&router, &source_url, &bodies).await;
        let held_count = i64::try_from(*held).expect("a small count");

        // Receive until the dead-letter queue holds every message, counting what each
        // delivery reported about itself on the way.
        let mut deliveries: BTreeMap<String, i64> =
            bodies.iter().map(|body| (body.clone(), 0)).collect();
        let deadline = Instant::now() + DEADLINE;
        while count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await < held_count
            && Instant::now() < deadline
        {
            for message in receive_batch(&router, &source_url, 0).await {
                let (_, body, reported) = received(&message);
                let delivered = deliveries
                    .get_mut(&body)
                    .unwrap_or_else(|| panic!("case {case}: a body nothing sent: {body:?}"));
                *delivered += 1;
                assert_eq!(
                    reported, *delivered,
                    "case {case}: expected delivery {delivered} of {body:?} to report that \
                     receive count, got {reported}"
                );
            }
        }

        for (body, delivered) in &deliveries {
            assert_eq!(
                delivered, allowed,
                "case {case}: expected {body:?} to be handed over {allowed} times before it \
                 was dead-lettered, got {delivered}"
            );
        }
        assert_eq!(
            message_counts(&router, &source_url).await,
            (0, 0, 0),
            "case {case}: expected the source queue to hold nothing once its messages were \
             dead-lettered"
        );
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            held_count,
            "case {case}: expected the dead-letter queue to hold the {held_count} messages"
        );

        // The dead-letter queue delivers them: the same ids and bodies, with the receive
        // count continuing from the deliveries the source queue made, because it counts
        // receives "across all queues".
        let mut delivered: Vec<(String, String, i64)> = receive_batch(&router, &dead_letter_url, 1)
            .await
            .iter()
            .map(received)
            .collect();
        delivered.sort();
        let mut expected: Vec<(String, String, i64)> = sent
            .iter()
            .cloned()
            .zip(bodies.iter().cloned())
            .map(|(id, body)| (id, body, allowed + 1))
            .collect();
        expected.sort();
        assert_eq!(
            delivered, expected,
            "case {case}: expected the dead-letter queue to deliver the messages it was sent, \
             got {delivered:?}"
        );
    }
}

/// One row of the table of messages whose delivery allowance is not spent: the case, the
/// JSON `maxReceiveCount` the queue's `RedrivePolicy` carries (absent for a queue with no
/// such policy), the queue's `VisibilityTimeout`, how many receives are made, and the
/// `(available, delayed, in flight)` counts the queue must report afterwards.
type UnspentRow = (&'static str, Option<Value>, &'static str, usize, (i64, i64, i64));

/// A message whose allowance is not spent — because its queue drives nothing to a
/// dead-letter queue, because it has deliveries left, or because it is in flight with a
/// consumer — stays in the queue holding it.
#[tokio::test]
async fn a_message_whose_allowance_is_unspent_stays_in_its_queue() {
    // (case, the JSON `maxReceiveCount` the policy carries — absent for a queue with no
    // RedrivePolicy at all, the queue's VisibilityTimeout, how many receives are made,
    // the (available, delayed, in flight) counts the queue must report afterwards).
    let rows: &[UnspentRow] = &[
        ("no RedrivePolicy at all, three deliveries", None, "0", 3, (1, 0, 0)),
        (
            "three deliveries allowed, two made",
            Some(json!(3)),
            "0",
            2,
            (1, 0, 0),
        ),
        (
            "one delivery allowed, the message still in flight",
            Some(json!(1)),
            "30",
            2,
            (0, 0, 1),
        ),
    ];

    for (index, (case, count_value, visibility_timeout, receives, expected_counts)) in
        rows.iter().enumerate()
    {
        let router = emulator();
        let dead_letter_name = format!("unspent-{index}-dlq");
        let dead_letter_url = create_ok(&router, &dead_letter_name, &json!({})).await;
        let mut attributes = json!({"VisibilityTimeout": visibility_timeout});
        if let Some(count_value) = count_value {
            attributes["RedrivePolicy"] =
                json!(redrive_policy(&expected_arn(&dead_letter_name), count_value));
        }
        let source_url =
            create_ok(&router, &format!("unspent-{index}-source"), &attributes).await;

        let bodies = vec![format!("{case} body")];
        send_all(&router, &source_url, &bodies).await;
        for _ in 0..*receives {
            receive_batch(&router, &source_url, 0).await;
        }

        assert_eq!(
            message_counts(&router, &source_url).await,
            *expected_counts,
            "case {case}: expected the message to stay in its own queue"
        );
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: expected the dead-letter queue to have been handed nothing"
        );
    }
}

/// One row of the source-naming table: the case, and whether the pair of queues is FIFO —
/// "the dead-letter queue of a FIFO queue must also be a FIFO queue", so the rule is
/// asserted over both queue types.
type NamingRow = (&'static str, bool);

/// A dead-lettered message NAMES the queue that gave up on it: a receive from the
/// dead-letter queue reports that queue's ARN under `DeadLetterQueueSourceArn`, a message in
/// the queue it was sent to reports no such attribute, and the stamp travels with the
/// message when a movement task drives it back out of the dead-letter queue.
#[tokio::test]
async fn a_dead_lettered_message_names_the_queue_it_was_driven_out_of() {
    const ATTRIBUTE: &str = "DeadLetterQueueSourceArn";
    let rows: &[NamingRow] = &[
        ("a standard pair", false),
        ("a FIFO pair", true),
    ];

    for (index, (case, fifo)) in rows.iter().enumerate() {
        let router = emulator();
        let suffix = if *fifo { ".fifo" } else { "" };
        let dead_letter_name = format!("naming-{index}-dlq{suffix}");
        let source_name = format!("naming-{index}-source{suffix}");
        let dead_letter_arn = expected_arn(&dead_letter_name);
        let source_arn = expected_arn(&source_name);
        let mut dead_letter_attributes = json!({"VisibilityTimeout": "0"});
        let mut source_attributes = json!({
            // A visibility timeout of zero hands a message straight back to the next
            // receive, so the row spends the allowance without waiting out a timeout.
            "VisibilityTimeout": "0",
            "RedrivePolicy": redrive_policy(&dead_letter_arn, &json!(1)),
        });
        if *fifo {
            dead_letter_attributes["FifoQueue"] = json!("true");
            source_attributes["FifoQueue"] = json!("true");
        }
        let dead_letter_url = create_ok(&router, &dead_letter_name, &dead_letter_attributes).await;
        let source_url = create_ok(&router, &source_name, &source_attributes).await;

        let body = format!("{case}: the poison message");
        let send = if *fifo {
            json!({
                "QueueUrl": source_url,
                "MessageBody": body,
                "MessageGroupId": "naming",
                "MessageDeduplicationId": format!("naming-{index}"),
            })
        } else {
            json!({"QueueUrl": source_url, "MessageBody": body})
        };
        ok(&router, "SendMessage", send).await;

        // In the queue it was sent to, nothing has dead-lettered it.
        let delivered = receive_naming(&router, &source_url).await;
        let (_, reported) = delivered
            .first()
            .unwrap_or_else(|| panic!("case {case}: the source queue delivered nothing"));
        assert!(
            reported.get(ATTRIBUTE).is_none(),
            "case {case}: a message in the queue it was sent to must report no {ATTRIBUTE}, got \
             {reported:?}"
        );

        // Spend what the policy allows, so the next receive drives it to the dead-letter
        // queue instead of handing it over.
        let deadline = Instant::now() + DEADLINE;
        while count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await < 1
            && Instant::now() < deadline
        {
            receive_naming(&router, &source_url).await;
        }
        let delivered = receive_naming(&router, &dead_letter_url).await;
        let (delivered_body, reported) = delivered
            .first()
            .unwrap_or_else(|| panic!("case {case}: the dead-letter queue delivered nothing"));
        assert_eq!(
            (delivered_body.as_str(), reported.get(ATTRIBUTE).map(String::as_str)),
            (body.as_str(), Some(source_arn.as_str())),
            "case {case}: the dead-lettered message must name the queue it was driven out of, \
             got {reported:?}"
        );

        // Driven back by a movement task, the message keeps what the redrive recorded: the
        // queue it was dead-lettered from is still the one it names.
        ok(
            &router,
            "StartMessageMoveTask",
            json!({"SourceArn": dead_letter_arn, "DestinationArn": source_arn}),
        )
        .await;
        let deadline = Instant::now() + DEADLINE;
        let mut driven_back: Vec<(String, BTreeMap<String, String>)> = Vec::new();
        while driven_back.is_empty() && Instant::now() < deadline {
            driven_back = receive_naming(&router, &source_url).await;
        }
        let (_, reported) = driven_back
            .first()
            .unwrap_or_else(|| panic!("case {case}: the move task drove nothing back"));
        assert_eq!(
            reported.get(ATTRIBUTE).map(String::as_str),
            Some(source_arn.as_str()),
            "case {case}: a message driven back out of the dead-letter queue must still name \
             the queue it was dead-lettered from, got {reported:?}"
        );
    }
}

/// Receive from the queue at `url`, selecting every message system attribute, and answer
/// each message's `(Body, attributes)`.
async fn receive_naming(
    router: &MultiServiceRouter,
    url: &str,
) -> Vec<(String, BTreeMap<String, String>)> {
    let reply = ok(
        router,
        "ReceiveMessage",
        json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "WaitTimeSeconds": 0,
            "MessageSystemAttributeNames": ["All"],
        }),
    )
    .await;
    messages(&reply)
        .iter()
        .map(|message| {
            (
                message["Body"].as_str().unwrap_or_default().to_string(),
                super::support::as_map(message.get("Attributes").unwrap_or(&Value::Null)),
            )
        })
        .collect()
}

/// The whole poison-message recovery path: messages dead-lettered by the source queue's
/// `RedrivePolicy` are driven back by a message movement task, which settles at
/// `COMPLETED` reporting the number of messages it was started over and the number it
/// moved, leaves the dead-letter queue empty and delivering nothing, and makes every body
/// receivable on the destination again.
#[tokio::test]
async fn a_move_task_over_a_filled_dead_letter_queue_completes_and_drains_it() {
    let router = emulator();
    let dead_letter_name = "cycle-dlq";
    let source_name = "cycle-source";
    let dead_letter_url = create_ok(&router, dead_letter_name, &json!({})).await;
    let dead_letter_arn = expected_arn(dead_letter_name);
    let source_arn = expected_arn(source_name);
    let source_url = create_ok(
        &router,
        source_name,
        &json!({
            "VisibilityTimeout": "0",
            "RedrivePolicy": redrive_policy(&dead_letter_arn, &json!(1)),
        }),
    )
    .await;

    let bodies: Vec<String> = (0..2).map(|number| format!("poison #{number}")).collect();
    send_all(&router, &source_url, &bodies).await;
    let held = i64::try_from(bodies.len()).expect("a small count");

    let deadline = Instant::now() + DEADLINE;
    while count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await < held
        && Instant::now() < deadline
    {
        receive_batch(&router, &source_url, 0).await;
    }
    assert_eq!(
        (
            count(&router, &source_url, "ApproximateNumberOfMessages").await,
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
        ),
        (0, held),
        "case the filled dead-letter queue: expected the source queue drained into it"
    );

    let started = ok(
        &router,
        "StartMessageMoveTask",
        json!({"SourceArn": dead_letter_arn, "DestinationArn": source_arn}),
    )
    .await;
    let handle = started["TaskHandle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !handle.is_empty(),
        "case the move task: expected a TaskHandle, got {started}"
    );

    // Poll the task until it settles, then read its progress off the listing.
    let deadline = Instant::now() + DEADLINE;
    let entry = loop {
        let listed = ok(
            &router,
            "ListMessageMoveTasks",
            json!({"SourceArn": dead_letter_arn}),
        )
        .await;
        let entry = listed
            .get("Results")
            .and_then(Value::as_array)
            .and_then(|results| results.first().cloned())
            .unwrap_or_else(|| panic!("case the move task: expected one entry, got {listed}"));
        if entry["Status"].as_str() == Some("COMPLETED") {
            break entry;
        }
        assert!(
            Instant::now() < deadline,
            "case the move task: expected it to complete within {DEADLINE:?}, last saw {entry}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        entry["DestinationArn"].as_str(),
        Some(source_arn.as_str()),
        "case the completed task: expected the destination the start named, got {entry}"
    );
    // "This number is obtained at the time of starting the message movement task"
    // (ListMessageMoveTasksResultEntry$ApproximateNumberOfMessagesToMove): the depth the
    // dead-letter queue had when the task took its messages.
    assert_eq!(
        entry["ApproximateNumberOfMessagesToMove"].as_i64(),
        Some(held),
        "case the completed task: expected the {held} messages it was started over, got {entry}"
    );
    let moved = entry["ApproximateNumberOfMessagesMoved"].as_i64().unwrap_or(0);
    assert!(
        moved >= held,
        "case the completed task: expected at least the {held} messages to have moved, got \
         {entry}"
    );

    // Every body is receivable on the destination again, and deleting each one is what
    // ends its journey: the dead-letter queue is left empty and delivering nothing.
    let deadline = Instant::now() + DEADLINE;
    let mut collected: Vec<String> = Vec::with_capacity(bodies.len());
    while collected.len() < bodies.len() && Instant::now() < deadline {
        for message in receive_batch(&router, &source_url, 1).await {
            let (_, body, _) = received(&message);
            collected.push(body);
            ok(
                &router,
                "DeleteMessage",
                json!({
                    "QueueUrl": source_url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
            )
            .await;
        }
    }
    collected.sort();
    let mut expected = bodies.clone();
    expected.sort();
    assert_eq!(
        collected, expected,
        "case the redriven messages: expected both bodies receivable on the destination, got \
         {collected:?}"
    );
    assert_eq!(
        count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
        0,
        "case the drained dead-letter queue: expected it to hold nothing"
    );
    let left = receive_batch(&router, &dead_letter_url, 1).await;
    assert!(
        left.is_empty(),
        "case the drained dead-letter queue: expected it to deliver nothing, got {left:?}"
    );
}

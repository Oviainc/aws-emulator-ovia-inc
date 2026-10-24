// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartMessageMoveTask`, driven over the wire (`POST /` with
//! `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness in
//! [`super::support`].
//!
//! The rule under test: a started message movement task takes the messages its source
//! dead-letter queue can hand out at that instant, delivers each of them into the
//! destination queue at the instant the task's rate schedules it, and answers with a
//! handle for the task; a source queue that no other queue drives messages to, a source
//! or destination this account has no queue for, a movement rate outside the documented
//! range, and a source queue whose task is still running are all refused, and refusing
//! moves nothing.
//!
//! Each table spans the class the rule generalises over rather than one example: the
//! delivery rows walk the message count (none, one, several) crossed with the rate forms
//! (left to the service, the published minimum, the published maximum) and both ways of
//! choosing a destination (named, or implied by the single dead-letter source queue), and
//! the refusal rows walk every way a `SourceArn`/`DestinationArn` can fail to name a
//! queue of this account plus each bound of the rate range and one value past it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::MESSAGE_MOVE_TASK_START_DELAY;

use super::support::{
    account, all_attributes, call, create_ok, emulator, error_code, expected_arn, messages, ok,
};
use smithy_server_runtime::MultiServiceRouter;

/// How long a delivery loop waits for the messages a task owes before giving up. Far
/// longer than any row's own schedule, so a slow machine cannot fail a row that the
/// emulator is still working through.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// The longest a receive waits for one message in the delivery loop, in seconds. Inside
/// the documented `ReceiveMessageRequest$WaitTimeSeconds` range ("an integer from 0 to
/// 20").
const RECEIVE_WAIT_SECONDS: u64 = 1;

/// An account id that is not this emulator's, for the rows that check a `SourceArn`
/// belonging to somebody else.
const OTHER_ACCOUNT: &str = "123456789012";

/// Create a queue called `name` and a queue called `<name>-source` whose `RedrivePolicy`
/// drives its messages to it, which is what makes the first queue a dead-letter queue.
/// Returns `(dead-letter queue url, dead-letter queue ARN, source queue url)`.
async fn dead_letter_queue(
    router: &MultiServiceRouter,
    name: &str,
) -> (String, String, String) {
    let dead_letter_url = create_ok(router, name, &json!({})).await;
    let dead_letter_arn = expected_arn(name);
    let source_url = redrives_to(router, &format!("{name}-source"), &dead_letter_arn).await;
    (dead_letter_url, dead_letter_arn, source_url)
}

/// Create a queue called `name` whose `RedrivePolicy` names `dead_letter_arn` as its
/// dead-letter target, and return its url.
async fn redrives_to(
    router: &MultiServiceRouter,
    name: &str,
    dead_letter_arn: &str,
) -> String {
    let policy = json!({"deadLetterTargetArn": dead_letter_arn, "maxReceiveCount": "1"});
    create_ok(
        router,
        name,
        &json!({"RedrivePolicy": policy.to_string()}),
    )
    .await
}

/// Send `bodies` to the queue at `url` and return the `MessageId` each send minted, in
/// send order.
async fn send_all(router: &MultiServiceRouter, url: &str, bodies: &[String]) -> Vec<String> {
    let mut ids = Vec::with_capacity(bodies.len());
    for body in bodies {
        let reply = ok(
            router,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": body}),
        )
        .await;
        ids.push(
            reply["MessageId"]
                .as_str()
                .unwrap_or_else(|| panic!("case send {body}: no MessageId in {reply}"))
                .to_string(),
        );
    }
    ids
}

/// One queue attribute as an integer, which is how every `ApproximateNumberOfMessages*`
/// attribute is reported.
async fn count(router: &MultiServiceRouter, url: &str, attribute: &str) -> i64 {
    let attributes: BTreeMap<String, String> = all_attributes(router, url).await;
    attributes
        .get(attribute)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {attribute} of {url}: not an integer in {attributes:?}"))
}

/// Receive from `url` until `wanted` messages have come over or the deadline passes, and
/// return `(message id, body)` in the order they were handed over.
async fn receive_until(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    deadline: Instant,
) -> Vec<(String, String)> {
    let mut delivered: Vec<(String, String)> = Vec::with_capacity(wanted);
    while delivered.len() < wanted && Instant::now() < deadline {
        let reply = ok(
            router,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "WaitTimeSeconds": RECEIVE_WAIT_SECONDS,
            }),
        )
        .await;
        for message in messages(&reply) {
            delivered.push((
                message["MessageId"].as_str().unwrap_or_default().to_string(),
                message["Body"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    delivered
}

/// A started task hands every message its dead-letter queue can give to the destination
/// queue, in the order the dead-letter queue held them, with the id and body each message
/// was sent with — and not before the task has had the chance to move them.
#[tokio::test]
async fn a_started_task_delivers_the_dead_letter_queues_messages_to_the_destination() {
    // (case, how many messages the dead-letter queue holds, MaxNumberOfMessagesPerSecond,
    // whether the request names the destination). The rate column holds the forms the
    // member documents: left blank ("the system will optimize the rate"), the smallest
    // rate that moves anything, and "the maximum value for messages per second is 500".
    let rows: &[(&str, usize, Option<i64>, bool)] = &[
        ("no messages, rate left to the service", 0, None, false),
        ("one message, rate left to the service", 1, None, false),
        ("three messages, rate left to the service", 3, None, true),
        ("two messages at the maximum rate of 500", 2, Some(500), true),
        ("two messages at the minimum rate of 1", 2, Some(1), false),
    ];

    for (index, (case, held, rate, name_destination)) in rows.iter().enumerate() {
        let router = emulator();
        let prefix = format!("move-{index}");
        let (dead_letter_url, dead_letter_arn, source_url) =
            dead_letter_queue(&router, &prefix).await;

        // Either the request names the destination, or it is left blank and the messages
        // go "back to their respective original source queues" — the one queue that
        // drives its messages to this dead-letter queue.
        let (destination_url, destination_arn) = if *name_destination {
            let name = format!("{prefix}-destination");
            let url = create_ok(&router, &name, &json!({})).await;
            (url, Some(expected_arn(&name)))
        } else {
            (source_url, None)
        };

        let bodies: Vec<String> = (0..*held).map(|number| format!("{case} #{number}")).collect();
        let sent = send_all(&router, &dead_letter_url, &bodies).await;
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            i64::try_from(*held).expect("a small count"),
            "case {case}: the dead-letter queue must hold what was sent to it"
        );

        let mut request = json!({"SourceArn": dead_letter_arn});
        if let Some(arn) = &destination_arn {
            request["DestinationArn"] = json!(arn);
        }
        if let Some(rate) = rate {
            request["MaxNumberOfMessagesPerSecond"] = json!(rate);
        }
        let started = Instant::now();
        let reply = ok(&router, "StartMessageMoveTask", request).await;
        let handle = reply["TaskHandle"].as_str().unwrap_or_default().to_string();
        assert!(
            !handle.is_empty(),
            "case {case}: expected a TaskHandle, got {reply}"
        );

        // The task is asynchronous: a consumer that polls the destination the moment the
        // start call answers has nothing to collect yet. Asserted only while the start
        // delay has genuinely not elapsed, so a slow machine cannot fail the row — and
        // whatever this receive does take counts toward the delivery below, so no message
        // is lost to it either.
        let immediate = messages(
            &ok(
                &router,
                "ReceiveMessage",
                json!({"QueueUrl": destination_url, "MaxNumberOfMessages": 10}),
            )
            .await,
        );
        if started.elapsed() < MESSAGE_MOVE_TASK_START_DELAY {
            assert!(
                immediate.is_empty(),
                "case {case}: expected nothing delivered yet, got {immediate:?}"
            );
        }

        let mut delivered: Vec<(String, String)> = immediate
            .iter()
            .map(|message| {
                (
                    message["MessageId"].as_str().unwrap_or_default().to_string(),
                    message["Body"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        delivered.extend(
            receive_until(
                &router,
                &destination_url,
                held.saturating_sub(delivered.len()),
                started + DELIVERY_DEADLINE,
            )
            .await,
        );
        let elapsed = started.elapsed();
        assert_eq!(
            delivered.iter().map(|(_, body)| body.clone()).collect::<Vec<String>>(),
            bodies,
            "case {case}: expected the bodies in the order the dead-letter queue held them, \
             got {delivered:?}"
        );
        assert_eq!(
            delivered.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
            sent,
            "case {case}: a moved message keeps the id it was sent with, got {delivered:?}"
        );

        // A fixed rate is a rate: the n-th message cannot be delivered before the start
        // delay plus (n - 1) of its slots have passed.
        let owed = MESSAGE_MOVE_TASK_START_DELAY
            + rate.map_or(Duration::ZERO, |rate| {
                Duration::from_nanos(
                    u64::try_from(held.saturating_sub(1)).expect("a small count")
                        * 1_000_000_000
                        / u64::try_from(rate).expect("a positive rate"),
                )
            });
        if *held > 0 {
            assert!(
                elapsed >= owed,
                "case {case}: expected the last message no sooner than {owed:?} after the \
                 start call, got {elapsed:?}"
            );
        }

        // The source queue gave the messages away, and the destination queue is the one
        // holding them now (each one in flight with the consumer that just took it).
        assert_eq!(
            count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
            0,
            "case {case}: the dead-letter queue must have handed its messages over"
        );
        assert_eq!(
            count(
                &router,
                &destination_url,
                "ApproximateNumberOfMessagesNotVisible"
            )
            .await,
            i64::try_from(*held).expect("a small count"),
            "case {case}: the destination queue must hold the moved messages"
        );
    }
}

/// Every way a start call can name no movable source queue, no destination queue, or a
/// rate outside the documented range is refused — and a refusal moves nothing.
#[tokio::test]
async fn a_start_naming_no_queue_or_a_rate_outside_its_range_is_refused() {
    let router = emulator();
    assert_ne!(
        OTHER_ACCOUNT,
        account(),
        "case the other-account row: its account id must not be this emulator's"
    );
    let (dead_letter_url, dead_letter_arn, _) = dead_letter_queue(&router, "refused").await;
    let plain_url = create_ok(&router, "refused-plain", &json!({})).await;
    let plain_arn = expected_arn("refused-plain");
    let bodies = vec!["stays put".to_string()];
    send_all(&router, &dead_letter_url, &bodies).await;
    send_all(&router, &plain_url, &bodies).await;

    // (case, request, expected status, expected error code). A value that names no queue
    // in any account is not an address this service can resolve at all, which is what
    // `InvalidAddress` ("The specified ID is invalid.") refuses; a well-formed ARN of
    // another account or service names a resource that does not exist here. The rate rows
    // walk the member's own published bound: "the maximum value for messages per second is
    // 500" (StartMessageMoveTaskRequest$MaxNumberOfMessagesPerSecond).
    let rows: &[(&str, Value, u16, &str)] = &[
        (
            "a SourceArn no queue of this account answers to",
            json!({"SourceArn": expected_arn("refused-absent")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another account",
            json!({"SourceArn": format!("arn:aws:sqs:us-east-1:{OTHER_ACCOUNT}:refused")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn naming another service",
            json!({"SourceArn": format!("arn:aws:sns:us-east-1:{}:refused", account())}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a SourceArn that is not an ARN at all",
            json!({"SourceArn": "refused"}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with a segment too few",
            json!({"SourceArn": format!("arn:aws:sqs:{}:refused", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn with an empty region segment",
            json!({"SourceArn": format!("arn:aws:sqs::{}:refused", account())}),
            404,
            "InvalidAddress",
        ),
        (
            "a SourceArn whose queue name is longer than a queue name may be",
            json!({
                "SourceArn": format!("arn:aws:sqs:us-east-1:{}:{}", account(), "q".repeat(81)),
            }),
            404,
            "InvalidAddress",
        ),
        (
            "a source queue no other queue drives messages to",
            json!({"SourceArn": plain_arn}),
            400,
            "UnsupportedOperation",
        ),
        (
            "a DestinationArn no queue of this account answers to",
            json!({
                "SourceArn": dead_letter_arn,
                "DestinationArn": expected_arn("refused-absent"),
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a DestinationArn that is not an ARN at all",
            json!({"SourceArn": dead_letter_arn, "DestinationArn": "refused-source"}),
            404,
            "InvalidAddress",
        ),
        (
            "a rate of zero, one below the smallest rate that moves anything",
            json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 0}),
            400,
            "ValidationException",
        ),
        (
            "a negative rate",
            json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": -1}),
            400,
            "ValidationException",
        ),
        (
            "a rate of 501, one past the published maximum of 500",
            json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 501}),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, parsed) = call(&router, "StartMessageMoveTask", request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (*expected_status, *expected_code),
            "case {case}: expected HTTP {expected_status} {expected_code}, got {status} with {parsed}"
        );
        assert!(
            parsed.get("TaskHandle").is_none(),
            "case {case}: a refusal starts no task, got {parsed}"
        );
    }

    for (case, url) in [
        ("the dead-letter queue", &dead_letter_url),
        ("the queue that is nobody's dead-letter target", &plain_url),
    ] {
        assert_eq!(
            count(&router, url, "ApproximateNumberOfMessages").await,
            1,
            "case {case}: a refused start moves nothing"
        );
    }
}

/// "Only one active message movement task is supported per queue at any given time": a
/// second start is refused while the first is still running, and accepted once it is not.
#[tokio::test]
async fn only_one_message_movement_task_runs_per_source_queue_at_a_time() {
    let router = emulator();
    let (dead_letter_url, dead_letter_arn, source_url) =
        dead_letter_queue(&router, "one-at-a-time").await;
    let bodies: Vec<String> = (0..2).map(|number| format!("message #{number}")).collect();
    send_all(&router, &dead_letter_url, &bodies).await;

    // One message per second, so the second of the two messages is still owed for a
    // whole second after the start call: the task is running for all of that time.
    let request = json!({"SourceArn": dead_letter_arn, "MaxNumberOfMessagesPerSecond": 1});
    let first = ok(&router, "StartMessageMoveTask", request.clone()).await;
    let handle = first["TaskHandle"].as_str().unwrap_or_default().to_string();
    assert!(
        !handle.is_empty(),
        "case the first start: expected a TaskHandle, got {first}"
    );

    let (status, parsed) = call(&router, "StartMessageMoveTask", &request).await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "UnsupportedOperation"),
        "case a second start while the first task runs: expected HTTP 400 \
         UnsupportedOperation, got {status} with {parsed}"
    );

    // Once the task has delivered everything it took, the source queue is free again.
    let delivered = receive_until(
        &router,
        &source_url,
        bodies.len(),
        Instant::now() + DELIVERY_DEADLINE,
    )
    .await;
    assert_eq!(
        delivered.len(),
        bodies.len(),
        "case the first task: expected every message delivered, got {delivered:?}"
    );
    let again = ok(&router, "StartMessageMoveTask", request).await;
    let second_handle = again["TaskHandle"].as_str().unwrap_or_default().to_string();
    assert!(
        !second_handle.is_empty() && second_handle != handle,
        "case a start after the first task finished: expected a new TaskHandle, got {again}"
    );
}

/// A task takes only what the dead-letter queue can hand out: a message still delayed,
/// and a message in flight with a consumer, stay where they are.
#[tokio::test]
async fn a_delayed_or_in_flight_message_stays_in_the_dead_letter_queue() {
    let router = emulator();
    let (dead_letter_url, dead_letter_arn, source_url) =
        dead_letter_queue(&router, "only-visible").await;

    // Sent first and taken first, so this one is in flight with a consumer (invisible for
    // the queue's 30 second default visibility timeout) when the task starts.
    send_all(&router, &dead_letter_url, &["in flight".to_string()]).await;
    let in_flight = messages(
        &ok(
            &router,
            "ReceiveMessage",
            json!({"QueueUrl": dead_letter_url, "MaxNumberOfMessages": 1}),
        )
        .await,
    );
    assert_eq!(
        in_flight.len(),
        1,
        "case the setup receive: expected the one message sent, got {in_flight:?}"
    );

    let movable = send_all(&router, &dead_letter_url, &["movable".to_string()]).await;
    // "An integer from 0 to 900 seconds" (DelaySeconds): delayed for the whole test, so
    // this message is not the queue's to give away either.
    ok(
        &router,
        "SendMessage",
        json!({"QueueUrl": dead_letter_url, "MessageBody": "delayed", "DelaySeconds": 900}),
    )
    .await;

    ok(
        &router,
        "StartMessageMoveTask",
        json!({"SourceArn": dead_letter_arn}),
    )
    .await;

    let delivered = receive_until(
        &router,
        &source_url,
        1,
        Instant::now() + DELIVERY_DEADLINE,
    )
    .await;
    assert_eq!(
        delivered
            .iter()
            .map(|(id, body)| (id.clone(), body.clone()))
            .collect::<Vec<(String, String)>>(),
        vec![(movable[0].clone(), "movable".to_string())],
        "case the destination queue: expected only the message the dead-letter queue could \
         hand out, got {delivered:?}"
    );

    // (case, attribute, what the dead-letter queue must still report) — the in-flight and
    // the delayed message are both still its own.
    for (case, attribute, expected) in [
        ("nothing left to hand out", "ApproximateNumberOfMessages", 0),
        (
            "the in-flight message stays",
            "ApproximateNumberOfMessagesNotVisible",
            1,
        ),
        (
            "the delayed message stays",
            "ApproximateNumberOfMessagesDelayed",
            1,
        ),
    ] {
        assert_eq!(
            count(&router, &dead_letter_url, attribute).await,
            expected,
            "case {case}: expected {attribute} to be {expected}"
        );
    }
}

/// "If this field is left blank, the messages will be redriven back to their respective
/// original source queues": with several queues driving messages to one dead-letter
/// queue, which queue a message came from is not determined, so the start call is refused
/// until it names a destination.
#[tokio::test]
async fn with_several_dead_letter_source_queues_the_destination_must_be_named() {
    let router = emulator();
    let (dead_letter_url, dead_letter_arn, first_source_url) =
        dead_letter_queue(&router, "two-sources").await;
    let second_source_url = redrives_to(&router, "two-sources-other", &dead_letter_arn).await;
    let bodies = vec!["ambiguous".to_string()];
    send_all(&router, &dead_letter_url, &bodies).await;

    let (status, parsed) = call(
        &router,
        "StartMessageMoveTask",
        &json!({"SourceArn": dead_letter_arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "UnsupportedOperation"),
        "case no DestinationArn with two dead-letter source queues: expected HTTP 400 \
         UnsupportedOperation, got {status} with {parsed}"
    );
    assert_eq!(
        count(&router, &dead_letter_url, "ApproximateNumberOfMessages").await,
        1,
        "case a refused start: the dead-letter queue keeps its message"
    );

    // Naming either of them is accepted, and the messages go to the one named.
    ok(
        &router,
        "StartMessageMoveTask",
        json!({
            "SourceArn": dead_letter_arn,
            "DestinationArn": expected_arn("two-sources-other"),
        }),
    )
    .await;
    let delivered = receive_until(
        &router,
        &second_source_url,
        1,
        Instant::now() + DELIVERY_DEADLINE,
    )
    .await;
    assert_eq!(
        delivered.iter().map(|(_, body)| body.clone()).collect::<Vec<String>>(),
        bodies,
        "case the named destination: expected the moved message, got {delivered:?}"
    );
    assert_eq!(
        count(&router, &first_source_url, "ApproximateNumberOfMessages").await,
        0,
        "case the source queue that was not named: it receives nothing"
    );
}

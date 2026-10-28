// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an ENABLED event source mapping on an Amazon SQS queue does to
//! that queue: `CreateEventSourceMapping` "creates a mapping between an event source and an
//! Lambda Event Function. Lambda reads items from the event source and invokes the
//! function", and `Enabled` is "when true, the event source mapping is active. When false,
//! Lambda pauses polling and invocation."
//!
//! The rule under test, in one sentence: while a mapping on a queue reports `Enabled`, that
//! queue's messages are DELIVERED to it — each delivery raising the message's
//! `ApproximateReceiveCount` and hiding it for the queue's `VisibilityTimeout` — and a
//! message a consumer never deletes is therefore delivered again and again until the source
//! queue's `RedrivePolicy` is exhausted, at which point Amazon SQS moves it to that
//! policy's `deadLetterTargetArn` with its body unchanged and the source queue drains.
//! Nothing here runs a deployment package, so no batch is ever processed — which is exactly
//! the poison-message case that redrive exists for.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with BOTH
//! services on ONE engine — the queues are created and read through Amazon SQS exactly as a
//! client creates and reads them, and the mapping through Lambda — so progress that is not
//! made against the stored queues fails here. The rows differ along what the rule
//! generalises over: how many deliveries the policy allows, how long the queue hides a
//! delivered message for, how many messages are waiting, how many one batch may hold, and
//! what the bodies are (plain text, JSON, non-ASCII text and a large body).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, call_sqs, create_mapping, create_queue, create_timed_function, get_mapping,
    queue_attribute, router_with_queues, MAPPINGS_URI,
};

/// The `State` a mapping reports once it is polling its event source.
const STATE_ENABLED: &str = "Enabled";

/// The `State` a mapping reports when its `Enabled` setting is false.
const STATE_DISABLED: &str = "Disabled";

/// How long a row waits for the queues to reach what the rule owes, before failing. Every
/// step of the chain is driven by the clock (the mapping settles, the poller delivers, the
/// visibility timeout lapses, the redrive moves the message), so a row waits rather than
/// assuming an instant.
const DEADLINE: Duration = Duration::from_secs(30);

/// How often a row re-reads the queues while it waits.
const POLL: Duration = Duration::from_millis(100);

/// Several of the one-second visibility windows these suites' queues hand a message over
/// for: long enough that a queue which was going to lose a message, or let one be moved,
/// has had the chance.
const VISIBILITY_WINDOWS: Duration = Duration::from_secs(3);

/// `SendMessage` of `body` to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// One `ReceiveMessage` on the queue `url` addresses, asking for the receive count: the
/// `(body, ApproximateReceiveCount)` of every message it handed over.
async fn receive(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    visibility_timeout: i64,
) -> Vec<(String, u32)> {
    let (status, answered) = call_sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "VisibilityTimeout": visibility_timeout,
            "MessageSystemAttributeNames": ["ApproximateReceiveCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ReceiveMessage to answer 200, got {status} {answered}"
    );
    answered["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let body = message["Body"].as_str().unwrap_or_default().to_string();
                    let count = message["Attributes"]["ApproximateReceiveCount"]
                        .as_str()
                        .and_then(|raw| raw.parse::<u32>().ok())
                        .unwrap_or(0);
                    (body, count)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `RedrivePolicy` attribute value naming `target_arn` and allowing `max_receive_count`
/// deliveries, as that attribute documents it: "the Amazon Resource Name (ARN) of the
/// dead-letter queue to which Amazon SQS moves messages after the value of maxReceiveCount
/// is exceeded".
fn redrive_policy(target_arn: &str, max_receive_count: i64) -> String {
    json!({ "deadLetterTargetArn": target_arn, "maxReceiveCount": max_receive_count }).to_string()
}

/// One row: the queue's `VisibilityTimeout`, how many deliveries its `RedrivePolicy` allows,
/// the mapping's `BatchSize` and the bodies waiting on the queue.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    max_receive_count: i64,
    batch_size: i64,
    bodies: Vec<String>,
}

/// The catalog: one delivery allowed and several, a queue that hides a delivered message for
/// one second and for two, one message and a batch of them, a batch ceiling above and below
/// what is waiting, and bodies that are plain text, JSON, non-ASCII text and large.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one message, one delivery allowed",
            visibility_timeout: 1,
            max_receive_count: 1,
            batch_size: 10,
            bodies: vec!["always-fails".to_string()],
        },
        Row {
            case: "two messages in one batch, two deliveries allowed",
            visibility_timeout: 1,
            max_receive_count: 2,
            batch_size: 10,
            bodies: vec!["first-poison".to_string(), "second-poison".to_string()],
        },
        Row {
            case: "a json body, three deliveries allowed, one message per batch",
            visibility_timeout: 2,
            max_receive_count: 3,
            batch_size: 1,
            bodies: vec![json!({ "order": 17, "items": ["a", "b"] }).to_string()],
        },
        Row {
            case: "three messages pulled one per batch",
            visibility_timeout: 1,
            max_receive_count: 1,
            batch_size: 1,
            bodies: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        },
        Row {
            case: "a non-ascii body and a large one",
            visibility_timeout: 1,
            max_receive_count: 2,
            batch_size: 10,
            bodies: vec!["ünïcödé ⚑ body".to_string(), "x".repeat(4_096)],
        },
    ]
}

#[tokio::test]
async fn a_batch_no_consumer_processes_is_redriven_to_the_dead_letter_queue() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-redrive-fn-{index}");
        // The dead-letter queue first: the source queue's policy names it, so it has to
        // exist before that policy is accepted.
        let (dlq_url, dlq_arn) =
            create_queue(&router, case, &format!("esm-redrive-dlq-{index}"), &json!({})).await;
        let (source_url, source_arn) = create_queue(
            &router,
            case,
            &format!("esm-redrive-source-{index}"),
            &json!({
                "VisibilityTimeout": row.visibility_timeout.to_string(),
                "RedrivePolicy": redrive_policy(&dlq_arn, row.max_receive_count),
            }),
        )
        .await;
        // A queue only maps onto a function it outlasts, so the function's `Timeout` is the
        // queue's own visibility timeout.
        create_timed_function(&router, case, &function, row.visibility_timeout).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": source_arn,
                "BatchSize": row.batch_size,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));

        for body in &row.bodies {
            send_message(&router, case, &source_url, body).await;
        }

        // The whole chain: the mapping settles into `Enabled`, its poller delivers each
        // message, nothing processes the batch, the visibility timeout lapses, the delivery
        // is made again, and once the policy's count is used up Amazon SQS moves the message
        // to the dead-letter queue. The row waits for the END of that chain.
        let mut seen: BTreeMap<String, u32> = BTreeMap::new();
        let deadline = Instant::now() + DEADLINE;
        let mut drained: Option<String> = None;
        while Instant::now() < deadline {
            // A long enough visibility timeout that a message counted here is not handed to
            // this loop a second time while it waits for the rest.
            for (body, count) in receive(&router, case, &dlq_url, 300).await {
                seen.entry(body).or_insert(count);
            }
            let visible = queue_attribute(
                &router,
                case,
                &source_url,
                "ApproximateNumberOfMessages",
            )
            .await;
            let in_flight = queue_attribute(
                &router,
                case,
                &source_url,
                "ApproximateNumberOfMessagesNotVisible",
            )
            .await;
            if seen.len() == row.bodies.len()
                && visible.as_deref() == Some("0")
                && in_flight.as_deref() == Some("0")
            {
                drained = visible;
                break;
            }
            tokio::time::sleep(POLL).await;
        }

        // Every body that was sent is in the dead-letter queue, byte for byte: "SQS moves
        // the message to the dead-letter queue" — the same message, not a copy of it.
        let bodies: Vec<String> = seen.keys().cloned().collect();
        let mut expected = row.bodies.clone();
        expected.sort();
        assert_eq!(
            bodies, expected,
            "case {case:?}: the dead-letter queue must hold exactly the bodies that were \
             sent, got {bodies:?}"
        );
        // "The number of times a message is delivered to the source queue before being
        // moved to the dead-letter queue": the source queue made exactly that many
        // deliveries, and the receive above is one more — `ApproximateReceiveCount` is "the
        // number of times a message has been received across all queues but not deleted".
        let expected_count = u32::try_from(row.max_receive_count).unwrap_or(1) + 1;
        for (body, count) in &seen {
            assert_eq!(
                *count, expected_count,
                "case {case:?}: the message {body:.32?} must have been delivered \
                 {} times by the source queue before the move, got a receive count of \
                 {count} in the dead-letter queue",
                row.max_receive_count
            );
        }
        // And the source queue drained: the messages left it as they reached the
        // dead-letter one.
        assert_eq!(
            drained.as_deref(),
            Some("0"),
            "case {case:?}: the source queue must report no messages left after the \
             redrive, got {drained:?}"
        );

        // The mapping polled its queue and says what became of the batch it delivered:
        // "the result of the event source mapping's last processing attempt".
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("State")),
            (200, Some(STATE_ENABLED.to_string())),
            "case {case:?}: the mapping must still report {STATE_ENABLED}, got {} {}",
            reading.status,
            reading.text()
        );
        let result = reading.string("LastProcessingResult").unwrap_or_default();
        assert!(
            !result.is_empty(),
            "case {case:?}: a mapping that delivered a batch must report the result of that \
             attempt, got {}",
            reading.text()
        );
    }
}

#[tokio::test]
async fn a_disabled_mapping_takes_nothing_out_of_its_queue() {
    // "When false, Lambda pauses polling and invocation": the message stays where it was
    // sent, is never delivered, and so is never redriven.
    let case = "a mapping created with Enabled false";
    let router = router_with_queues();
    let (dlq_url, dlq_arn) = create_queue(&router, case, "esm-disabled-dlq", &json!({})).await;
    let (source_url, source_arn) = create_queue(
        &router,
        case,
        "esm-disabled-source",
        &json!({
            "VisibilityTimeout": "1",
            "RedrivePolicy": redrive_policy(&dlq_arn, 1),
        }),
    )
    .await;
    create_timed_function(&router, case, "esm-disabled-fn", 1).await;

    let created = create_mapping(
        &router,
        &json!({
            "FunctionName": "esm-disabled-fn",
            "EventSourceArn": source_arn,
            "Enabled": false,
        }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    send_message(&router, case, &source_url, "never-delivered").await;

    // Long enough for several visibility windows of the source queue to pass, so a poller
    // that was going to deliver anything has had every chance to.
    tokio::time::sleep(VISIBILITY_WINDOWS).await;

    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        (
            reading.status,
            reading.string("State"),
            reading.string("LastProcessingResult"),
        ),
        (200, Some(STATE_DISABLED.to_string()), None),
        "case {case:?}: a paused mapping reports {STATE_DISABLED} and no processing \
         attempt, got {} {}",
        reading.status,
        reading.text()
    );
    let dead_lettered = receive(&router, case, &dlq_url, 1).await;
    assert!(
        dead_lettered.is_empty(),
        "case {case:?}: nothing was delivered, so nothing can have been redriven, got \
         {dead_lettered:?}"
    );
    let visible = queue_attribute(&router, case, &source_url, "ApproximateNumberOfMessages").await;
    assert_eq!(
        visible.as_deref(),
        Some("1"),
        "case {case:?}: the message must still be waiting on the source queue, got \
         {visible:?}"
    );
}

#[tokio::test]
async fn a_queue_with_no_redrive_policy_delivers_and_keeps_the_message() {
    // The other half of the rule: the deliveries are counted whether or not a policy bounds
    // them, and with no `deadLetterTargetArn` to move the message to there is no number of
    // deliveries to exceed, so the message stays on its own queue however many times it is
    // handed over.
    let case = "a queue carrying no redrive policy";
    let router = router_with_queues();
    let (source_url, source_arn) = create_queue(
        &router,
        case,
        "esm-no-redrive-source",
        &json!({ "VisibilityTimeout": "1" }),
    )
    .await;
    create_timed_function(&router, case, "esm-no-redrive-fn", 1).await;
    let created = create_mapping(
        &router,
        &json!({ "FunctionName": "esm-no-redrive-fn", "EventSourceArn": source_arn }),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    send_message(&router, case, &source_url, "redelivered-for-ever").await;

    // The message is in flight with the mapping's poller for the queue's visibility timeout
    // after each delivery, which is what proves the delivery happened at all. The wait is
    // for that observation, not for a span: a slow machine delays it instead of weakening it.
    let deadline = Instant::now() + DEADLINE;
    let mut in_flight = None;
    while Instant::now() < deadline {
        in_flight = queue_attribute(
            &router,
            case,
            &source_url,
            "ApproximateNumberOfMessagesNotVisible",
        )
        .await;
        if in_flight.as_deref() == Some("1") {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    assert_eq!(
        in_flight.as_deref(),
        Some("1"),
        "case {case:?}: the mapping's poller must take the message out of the queue, got \
         {in_flight:?}"
    );

    // Several visibility windows later the message is still THIS queue's: there is no
    // dead-letter target to move it to and no consumer deleted it, so the queue holds
    // exactly the one message it was sent whatever the deliveries did.
    tokio::time::sleep(VISIBILITY_WINDOWS).await;
    let visible = queue_attribute(&router, case, &source_url, "ApproximateNumberOfMessages").await;
    let not_visible = queue_attribute(
        &router,
        case,
        &source_url,
        "ApproximateNumberOfMessagesNotVisible",
    )
    .await;
    let held: u32 = [&visible, &not_visible]
        .iter()
        .filter_map(|reported| reported.as_deref().and_then(|raw| raw.parse::<u32>().ok()))
        .sum();
    assert_eq!(
        held, 1,
        "case {case:?}: the queue must still hold exactly the message it was sent, got \
         {visible:?} visible and {not_visible:?} in flight"
    );

    // A client cannot take a message out from under a running poller — the poller hands it
    // over again the instant its visibility lapses — so the mapping is PAUSED first:
    // "when false, Lambda pauses polling and invocation". Once it reports `Disabled`
    // nothing else is competing for the message.
    let paused = call(
        &router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ "Enabled": false }),
    )
    .await;
    assert_eq!(
        paused.status,
        202,
        "case {case:?}: expected UpdateEventSourceMapping to answer 202, got {} {}",
        paused.status,
        paused.text()
    );
    let deadline = Instant::now() + DEADLINE;
    let mut state = None;
    while Instant::now() < deadline {
        state = get_mapping(&router, &uuid).await.string("State");
        if state.as_deref() == Some(STATE_DISABLED) {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    assert_eq!(
        state.as_deref(),
        Some(STATE_DISABLED),
        "case {case:?}: the paused mapping must settle into {STATE_DISABLED}, got {state:?}"
    );

    // And the client that now takes it sees the deliveries counted: at least the one this
    // test watched the poller make, beside this receive. "ApproximateReceiveCount — the
    // number of times a message has been received across all queues but not deleted."
    let deadline = Instant::now() + DEADLINE;
    let mut handed: Vec<(String, u32)> = Vec::new();
    while Instant::now() < deadline {
        handed = receive(&router, case, &source_url, 30).await;
        if !handed.is_empty() {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    let Some((body, count)) = handed.first() else {
        panic!("case {case:?}: the message must still be on its own queue, got {handed:?}");
    };
    assert_eq!(
        body, "redelivered-for-ever",
        "case {case:?}: the body must be the one that was sent, got {body:?}"
    );
    assert!(
        *count >= 2,
        "case {case:?}: every delivery is counted, so a message nobody deleted must show the \
         poller's deliveries beside this receive, got a receive count of {count}"
    );
}

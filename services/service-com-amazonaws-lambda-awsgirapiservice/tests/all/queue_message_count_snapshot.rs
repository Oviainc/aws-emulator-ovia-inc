// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what ONE `GetQueueAttributes` answer says about the messages an
//! Amazon SQS queue holds while an event source mapping's poller is delivering them.
//!
//! The rule under test, in one sentence: a queue's three message counts are a SNAPSHOT of
//! one instant — `ApproximateNumberOfMessages` is "the approximate number of visible
//! messages in a queue", `ApproximateNumberOfMessagesNotVisible` "the approximate number of
//! messages that are in flight" and `ApproximateNumberOfMessagesDelayed` "the approximate
//! number of messages in the queue that are delayed and not available for reading
//! immediately" — so every message the queue holds is counted in exactly one of them and
//! their total, within ONE answer, is the number of messages the queue holds, however often
//! a consumer moves a message from one count to another.
//!
//! That total is what a reader can rely on and a total assembled from SEVERAL answers is
//! not: each answer reports its own instant, so a message that is in flight when one count
//! is read and visible again when the next is read is counted twice or not at all. An event
//! source mapping on the queue makes that happen continually — it hands the message over on
//! every pass and the queue hides it only for its `VisibilityTimeout` — which is why this
//! suite drives the counts under a running poller rather than over a queue nobody touches.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with
//! Amazon SQS and Lambda on ONE engine: the queue is created, loaded and read exactly as a
//! client does it, and the mapping is made through Lambda. Nothing here runs a deployment
//! package, so no batch is ever processed and no message is deleted — the queue holds every
//! message it was sent for the whole of a row, which is what makes the total a known number.
//! The rows differ along what the rule generalises over: how many messages are waiting, how
//! long the queue hides a delivered one for, how many one batch may hold, whether the
//! messages start delayed, and what the bodies are (plain text, JSON, non-ASCII text and a
//! large body).

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call_sqs, create_mapping, create_queue, create_timed_function, messages_held,
    one_answer_attributes, queue_message_counts, router_with_queues, DELAYED_MESSAGES,
    IN_FLIGHT_MESSAGES, VISIBLE_MESSAGES,
};

/// How long a row goes on sampling the counts before it fails for never having seen the
/// poller take a message. Every step before the first delivery is driven by the clock (the
/// mapping settles, the delay elapses, the poller wakes), so a row waits rather than
/// assuming an instant.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long a row waits between two samples. Short against the poll interval, so a sample
/// lands inside a visibility window as often as between two of them.
const BETWEEN_SAMPLES: Duration = Duration::from_millis(50);

/// How many answers a row reads before it is satisfied: enough that the poller has made
/// several deliveries under the one- and two-second visibility windows these queues hand a
/// message over for, so the counts have moved between samples.
const SAMPLES: usize = 40;

/// One row: the queue's `VisibilityTimeout` and `DelaySeconds`, the mapping's `BatchSize`,
/// and the bodies sent to the queue.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    delay_seconds: i64,
    batch_size: i64,
    bodies: Vec<String>,
}

/// The catalog: one message and several, a queue that hides a delivered message for one
/// second and for two, a batch ceiling above and below what is waiting, messages that start
/// delayed and messages that do not, and bodies that are plain text, JSON, non-ASCII text
/// and large.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one message, polled ten at a time",
            visibility_timeout: 1,
            delay_seconds: 0,
            batch_size: 10,
            bodies: vec!["only-one".to_string()],
        },
        Row {
            case: "three messages, polled one per batch",
            visibility_timeout: 1,
            delay_seconds: 0,
            batch_size: 1,
            bodies: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        },
        Row {
            case: "seven messages hidden for two seconds at a time",
            visibility_timeout: 2,
            delay_seconds: 0,
            batch_size: 3,
            bodies: (0..7).map(|index| format!("body-{index}")).collect(),
        },
        Row {
            case: "two messages that start delayed",
            visibility_timeout: 1,
            delay_seconds: 2,
            batch_size: 10,
            bodies: vec![
                json!({ "order": 17, "items": ["a", "b"] }).to_string(),
                "ünïcödé ⚑ body".to_string(),
            ],
        },
        Row {
            case: "a json body and a large one",
            visibility_timeout: 1,
            delay_seconds: 0,
            batch_size: 2,
            bodies: vec![json!({ "id": "x" }).to_string(), "x".repeat(4_096)],
        },
    ]
}

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

#[tokio::test]
async fn one_answer_counts_every_message_the_queue_holds_exactly_once() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("queue-counts-fn-{index}");
        let mut attributes = json!({ "VisibilityTimeout": row.visibility_timeout.to_string() });
        if row.delay_seconds > 0 {
            attributes["DelaySeconds"] = json!(row.delay_seconds.to_string());
        }
        let (url, arn) =
            create_queue(&router, case, &format!("queue-counts-{index}"), &attributes).await;
        // A queue only maps onto a function it outlasts, so the function's `Timeout` is the
        // queue's own visibility timeout.
        create_timed_function(&router, case, &function, row.visibility_timeout).await;
        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
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

        for body in &row.bodies {
            send_message(&router, case, &url, body).await;
        }
        let sent = u32::try_from(row.bodies.len()).unwrap_or(u32::MAX);

        // Every answer, from the instant the messages were sent until the row has taken its
        // samples, accounts for exactly the messages the queue holds — and no single count
        // claims more than that.
        let deadline = Instant::now() + DEADLINE;
        let mut samples = 0usize;
        let mut delivered = false;
        while Instant::now() < deadline {
            let counts = queue_message_counts(&router, case, &url).await;
            let (visible, in_flight, delayed) = counts;
            assert_eq!(
                messages_held(counts),
                sent,
                "case {case:?}: one answer must account for each of the {sent} messages the \
                 queue holds exactly once, got {counts:?} as (visible, in flight, delayed) \
                 on sample {samples}"
            );
            for (name, count) in [
                (VISIBLE_MESSAGES, visible),
                (IN_FLIGHT_MESSAGES, in_flight),
                (DELAYED_MESSAGES, delayed),
            ] {
                assert!(
                    count <= sent,
                    "case {case:?}: {name} cannot exceed the {sent} messages the queue \
                     holds, got {count} in {counts:?}"
                );
            }
            delivered = delivered || in_flight > 0;
            samples += 1;
            if samples >= SAMPLES && delivered {
                break;
            }
            tokio::time::sleep(BETWEEN_SAMPLES).await;
        }

        // The counts were moving while they were sampled: the mapping's poller took
        // messages out and the queue hid them, so the invariant above held over a queue
        // whose messages were changing hands rather than over a still one.
        assert!(
            delivered,
            "case {case:?}: the mapping's poller must have taken a message out of the \
             queue, so that the counts moved while they were read; none of the {samples} \
             answers reported one in flight"
        );
        assert!(
            samples >= SAMPLES,
            "case {case:?}: expected at least {SAMPLES} answers inside the deadline, got \
             {samples}"
        );

        // The same holds of the answer a caller asks for with `All` — "All — all values" —
        // which is the other way the three counts are reported together.
        let all = one_answer_attributes(&router, case, &url, &["All"]).await;
        let count = |name: &str| {
            all[name]
                .as_str()
                .and_then(|raw| raw.parse::<u32>().ok())
                .unwrap_or_else(|| panic!("case {case:?}: no {name} in {all}"))
        };
        let reported = (
            count(VISIBLE_MESSAGES),
            count(IN_FLIGHT_MESSAGES),
            count(DELAYED_MESSAGES),
        );
        assert_eq!(
            messages_held(reported),
            sent,
            "case {case:?}: the All answer must account for each of the {sent} messages the \
             queue holds exactly once, got {reported:?} as (visible, in flight, delayed)"
        );
    }
}

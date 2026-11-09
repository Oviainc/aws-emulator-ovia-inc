// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon SQS queue CONSUMES of the batch it handed its
//! function, asserted through the function's own run: the handler of a real deployment
//! package reads every record of the batch and decides, from that record's own body, which of
//! them to report as unprocessed — so what the queue holds afterwards is a consequence of the
//! records having reached the handler and of its report having been read.
//!
//! The RULE asserted over the table below, in one sentence: a mapping whose
//! `FunctionResponseTypes` asks for its function's report of which messages of a batch failed
//! deletes from the source queue exactly the messages of that batch the report does NOT name,
//! and leaves exactly the ones it names for redelivery — whatever `MessageGroupId` each
//! message carries, because the report names messages and not groups. ("(Kinesis, DynamoDB
//! Streams, Amazon MSK, self-managed Apache Kafka, and Amazon SQS) A list of current response
//! type enums applied to the event source mapping", whose one value — `smithy_query(query =
//! "shape", shape = "FunctionResponseType")` — is `ReportBatchItemFailures`.)
//!
//! The second suite drives the other half of the same data plane, the one a queue's own
//! `RedrivePolicy` governs: a message no attempt processes is delivered as many times as the
//! policy allows and then moved to the dead-letter queue it names, and once the function is
//! repaired a `StartMessageMoveTask` that moves it back has it processed and the dead-letter
//! queue drained.
//!
//! The rows differ along what the rule generalises over: whether the source queue is FIFO or
//! standard, how many message groups the batch spans, where in the batch (and in its group)
//! the message the handler will not process sits, how many messages are waiting and how many
//! one batch may hold.
//!
//! The harness — the router with Lambda and Amazon SQS on one engine, the queue fixture, the
//! function whose package runs and the waits for what the clock owes — is the shared one in
//! `event_source_mappings.rs`, so every suite over this surface drives the same wire.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    HOST_WAIT_CEILING, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_OK, REPORT_BATCH_ITEM_FAILURES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_function_active, await_mapping_state, call, call_sqs, create_mapping, create_queue,
    create_running_function, get_mapping, messages_held, queue_held, queue_message_counts,
    router_with_queues, send_message, LOOK_AGAIN, MAPPINGS_URI, STATE_DISABLED,
};
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` and `UpdateFunctionCode` are bound (`POST /2015-03-31/functions`,
/// `PUT /2015-03-31/functions/{FunctionName}/code`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a mapping reports once it polls its event source
/// (`EventSourceMappingConfiguration$State`).
const STATE_ENABLED: &str = "Enabled";

/// The `VisibilityTimeout` every source queue below hides a delivered message for, and the
/// `Timeout` of the function it maps onto: a queue only maps onto a function it outlasts, and
/// a short window makes a message the handler did not process come back promptly.
const VISIBILITY_TIMEOUT: i64 = 2;

/// The marker a message body carries to be one the handler reports as unprocessed. Nothing in
/// the emulator reads it: the handler reads it out of the record's own body.
const POISON: &str = "poison";

/// How long these suites watch the QUEUES for the state the clock owes them: a mapping's pass
/// polling, invoking and deleting, a message no attempt processed coming back and being moved
/// to the dead-letter queue, and a move task draining it. Every wait over a queue here is
/// given this much room, the longest of them deciding it; the waits for what the CONTROL
/// plane owes — a function going `Active`, a mapping settling into its state — keep the
/// shared harness's own budget, since none of them waits on a queue.
///
/// The bound is PATIENCE, not a weakened expectation: every assertion below is unchanged.
/// It is taken from what the EMULATOR itself allows ONE attempt rather than being picked
/// here — the wait for a share of the run capacity ([`PERMIT_WAIT`]) and the wait for the
/// run's own answer ([`HOST_WAIT_CEILING`]) — with room for the several passes a row waits
/// for, because a whole-workspace test run shares its cores with every other suite.
const WATCH_BUDGET: Duration = PERMIT_WAIT
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(Duration::from_secs(30));

/// The handler every row of the first suite runs, with the marker above bound to the name it
/// reads it by — so there is ONE spelling of it on both sides of the interpreter.
///
/// It asserts the shape of the envelope Lambda hands an SQS batch over as, and decides what to
/// report from each record's own body, so the report is a consequence of the record having
/// arrived.
fn reporting_handler() -> String {
    format!(
        r#"
import os

QUEUE_ARN = os.environ["QUEUE_ARN"]
POISON = "{POISON}"


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    failures = []
    for record in records:
        if record["eventSource"] != "aws:sqs":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != QUEUE_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        if not record["messageId"]:
            raise ValueError("a record carries no messageId")
        if not record["receiptHandle"]:
            raise ValueError("a record carries no receiptHandle")
        if POISON in record["body"]:
            failures.append({{"itemIdentifier": record["messageId"]}})
    return {{"batchItemFailures": failures}}
"#
    )
}

/// The handler the dead-letter suite's function starts out with: every message it is handed
/// raises, so no attempt processes anything and the source queue's `RedrivePolicy` is what
/// decides where the message ends up.
const RAISING_HANDLER: &str = r#"
def lambda_handler(event, context):
    raise RuntimeError("this consumer cannot process " + repr(event["Records"][0]["body"]))
"#;

/// The handler it is repaired with: every message it is handed is processed.
const PROCESSING_HANDLER: &str = r#"
def lambda_handler(event, context):
    return {"processed": [record["messageId"] for record in event["Records"]]}
"#;

/// Create the mapping and wait until it reports `Enabled`, answering its UUID.
async fn given_enabled_mapping(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let created = create_mapping(router, request).await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: CreateEventSourceMapping answered {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    await_mapping_state(router, case, &uuid, STATE_ENABLED).await;
    uuid
}

/// Every body the queue `url` addresses still holds, read through `ReceiveMessage` until it
/// hands over nothing more.
async fn drain(router: &MultiServiceRouter, case: &str, url: &str) -> BTreeSet<String> {
    let mut bodies: BTreeSet<String> = BTreeSet::new();
    let deadline = Instant::now() + WATCH_BUDGET;
    while Instant::now() < deadline {
        let (status, answered) = call_sqs(
            router,
            "ReceiveMessage",
            &json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                // Long enough that a message counted here is not handed over again while
                // this loop reads the rest.
                "VisibilityTimeout": 300,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ReceiveMessage answered {status} {answered}"
        );
        let handed = answered["Messages"].as_array().cloned().unwrap_or_default();
        if handed.is_empty() {
            // Nothing visible right now. The queue holds nothing else once what it reports is
            // accounted for by what this loop has taken.
            let counts = queue_message_counts(router, case, url).await;
            if usize::try_from(messages_held(counts)).unwrap_or(usize::MAX) <= bodies.len() {
                break;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
            continue;
        }
        for message in handed {
            bodies.insert(
                message["Body"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a message with no Body"))
                    .to_string(),
            );
        }
    }
    bodies
}

/// Pause the mapping `uuid` names and wait until it reports `Disabled`, so nothing competes
/// with this test for the queue's messages: "when false, Lambda pauses polling and
/// invocation".
async fn pause_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) {
    let paused = call(
        router,
        "PUT",
        &format!("{MAPPINGS_URI}/{uuid}"),
        &json!({ "Enabled": false }),
    )
    .await;
    assert_eq!(
        paused.status,
        202,
        "case {case:?}: UpdateEventSourceMapping answered {} {}",
        paused.status,
        paused.text()
    );
    await_mapping_state(router, case, uuid, STATE_DISABLED).await;
}

/// Wait until the queue `url` addresses holds `expected` messages AND the mapping has
/// reported `wanted` as a `LastProcessingResult`, answering what was last held and every
/// distinct result reported on the way.
///
/// Both conditions are waited for together, and the results are collected as they are
/// reported: a pass deletes the messages its attempt processed BEFORE it records what that
/// attempt came to, and the pass after it is handed only the messages the report left behind —
/// which is a different attempt reporting a different result. So an assertion about the
/// attempt that CONSUMED the batch reads the results seen while the queue converged, rather
/// than whatever the mapping happens to report once it has.
async fn held_and_results(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    uuid: &str,
    expected: u32,
    wanted: &str,
) -> (u32, Vec<String>) {
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut seen: Vec<String> = Vec::new();
    loop {
        if let Some(result) = get_mapping(router, uuid).await.string("LastProcessingResult") {
            if seen.last() != Some(&result) {
                seen.push(result);
            }
        }
        let held = queue_held(router, case, url).await;
        let settled = held == expected && seen.iter().any(|result| result == wanted);
        if settled || Instant::now() >= deadline {
            return (held, seen);
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One message of a row: its body, and the `MessageGroupId` it is sent under when the queue is
/// FIFO.
struct Sent {
    body: &'static str,
    group: Option<&'static str>,
}

/// One row of the first suite: the source queue's kind, the messages waiting on it, the
/// `BatchSize` they are read in, and the bodies the queue must be left holding — the ones the
/// handler reports as unprocessed, and nothing else.
struct Row {
    case: &'static str,
    fifo: bool,
    sent: &'static [Sent],
    batch_size: i64,
    left_behind: &'static [&'static str],
}

/// The queue name a row's source queue is created under: a FIFO queue's name ends in `.fifo`
/// ("If you create a FIFO queue, your queue name must end with the .fifo suffix").
fn queue_name(case: &str, fifo: bool) -> String {
    let base = format!("esm-report-{}", case.replace(' ', "-"));
    if fifo {
        format!("{base}.fifo")
    } else {
        base
    }
}

/// RULE: a mapping that reads its function's report of which messages of a batch failed
/// deletes exactly the messages that report does not name, and leaves exactly the ones it
/// does — whatever message group each message belongs to.
#[tokio::test]
async fn a_partial_batch_report_deletes_every_message_it_does_not_name() {
    let rows = [
        // The recorded shape: one FIFO message group whose FIRST message the handler will not
        // process, and two later messages of that SAME group that it does — those two are
        // deleted even though their group's earlier message is going back for another attempt,
        // because the report names messages.
        Row {
            case: "fifo one group earlier failure later successes",
            fifo: true,
            sent: &[
                Sent {
                    body: "poison-1",
                    group: Some("orders"),
                },
                Sent {
                    body: "good-2",
                    group: Some("orders"),
                },
                Sent {
                    body: "good-3",
                    group: Some("orders"),
                },
            ],
            batch_size: 10,
            left_behind: &["poison-1"],
        },
        // The failure in the MIDDLE of one FIFO group, so the message before it and the one
        // after it are both deleted.
        Row {
            case: "fifo one group middle failure",
            fifo: true,
            sent: &[
                Sent {
                    body: "good-1",
                    group: Some("orders"),
                },
                Sent {
                    body: "poison-2",
                    group: Some("orders"),
                },
                Sent {
                    body: "good-3",
                    group: Some("orders"),
                },
            ],
            batch_size: 10,
            left_behind: &["poison-2"],
        },
        // TWO groups, each with a failure and a success: a group's failure holds back nothing
        // of the other group and nothing of its own that the report left out.
        Row {
            case: "fifo two groups",
            fifo: true,
            sent: &[
                Sent {
                    body: "poison-a1",
                    group: Some("a"),
                },
                Sent {
                    body: "good-a2",
                    group: Some("a"),
                },
                Sent {
                    body: "good-b1",
                    group: Some("b"),
                },
                Sent {
                    body: "poison-b2",
                    group: Some("b"),
                },
            ],
            batch_size: 10,
            left_behind: &["poison-a1", "poison-b2"],
        },
        // A STANDARD queue, whose messages carry no group at all, read one per batch: the same
        // report leaves the same messages behind.
        Row {
            case: "standard one message per batch",
            fifo: false,
            sent: &[
                Sent {
                    body: "good-1",
                    group: None,
                },
                Sent {
                    body: "poison-2",
                    group: None,
                },
                Sent {
                    body: "good-3",
                    group: None,
                },
            ],
            batch_size: 1,
            left_behind: &["poison-2"],
        },
        // The whole batch processed: the report names nothing, so the queue drains.
        Row {
            case: "fifo nothing reported",
            fifo: true,
            sent: &[
                Sent {
                    body: "good-1",
                    group: Some("orders"),
                },
                Sent {
                    body: "good-2",
                    group: Some("orders"),
                },
            ],
            batch_size: 10,
            left_behind: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-report-fn-{}", case.replace(' ', "-"));
        let mut attributes = json!({ "VisibilityTimeout": VISIBILITY_TIMEOUT.to_string() });
        if row.fifo {
            // A FIFO queue is created as one, and its sends carry no deduplication id, so the
            // queue derives one from the body ("ContentBasedDeduplication — enables
            // content-based deduplication").
            attributes["FifoQueue"] = json!("true");
            attributes["ContentBasedDeduplication"] = json!("true");
        }
        let (url, arn) = create_queue(&router, case, &queue_name(case, row.fifo), &attributes).await;
        create_running_function(
            &router,
            case,
            &function,
            &reporting_handler(),
            VISIBILITY_TIMEOUT,
            &json!({ "QUEUE_ARN": arn }),
        )
        .await;
        let uuid = given_enabled_mapping(
            &router,
            case,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": row.batch_size,
                "FunctionResponseTypes": [REPORT_BATCH_ITEM_FAILURES],
                "Enabled": true,
            }),
        )
        .await;

        for message in row.sent {
            send_message(&router, case, &url, message.body, message.group).await;
        }

        // The queue comes to hold exactly as many messages as the handler reported: the rest
        // were deleted by the mapping, which is the observation the rule makes.
        let expected = u32::try_from(row.left_behind.len()).unwrap_or(u32::MAX);
        let (held, results) = held_and_results(
            &router,
            case,
            &url,
            &uuid,
            expected,
            LAST_PROCESSING_RESULT_OK,
        )
        .await;
        assert_eq!(
            held, expected,
            "case {case:?}: the queue must be left holding the {} message(s) the report named, \
             got {held} (the mapping reported {results:?})",
            row.left_behind.len()
        );

        // The attempt that consumed the batch reported itself as such. It is read from the
        // results seen WHILE the deletions happened, because a later pass is handed only the
        // messages the report left behind and reports what it made of those.
        assert!(
            results.iter().any(|result| result == LAST_PROCESSING_RESULT_OK),
            "case {case:?}: the mapping must report {LAST_PROCESSING_RESULT_OK} for the attempt \
             that consumed part of its batch, saw {results:?}"
        );

        // And the messages left are the ones the report NAMED, body for body — read after the
        // mapping is paused, so nothing hands them over from under this reader.
        pause_mapping(&router, case, &uuid).await;
        let remaining = drain(&router, case, &url).await;
        let wanted: BTreeSet<String> = row
            .left_behind
            .iter()
            .map(|body| (*body).to_string())
            .collect();
        assert_eq!(
            remaining, wanted,
            "case {case:?}: the queue must be left holding exactly the reported messages, got \
             {remaining:?}"
        );
    }
}

/// `StartMessageMoveTask` on the dead-letter queue `source_arn` names, moving its messages to
/// `destination_arn`: the task handle it answered.
async fn start_move_task(
    router: &MultiServiceRouter,
    case: &str,
    source_arn: &str,
    destination_arn: &str,
) -> String {
    let (status, started) = call_sqs(
        router,
        "StartMessageMoveTask",
        &json!({ "SourceArn": source_arn, "DestinationArn": destination_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartMessageMoveTask answered {status} {started}"
    );
    started["TaskHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TaskHandle in {started}"))
        .to_string()
}

/// Wait until the move task on `source_arn` reports `COMPLETED`, answering the entry
/// `ListMessageMoveTasks` reported it as.
async fn completed_move_task(router: &MultiServiceRouter, case: &str, source_arn: &str) -> Value {
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let (status, listed) = call_sqs(
            router,
            "ListMessageMoveTasks",
            &json!({ "SourceArn": source_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMessageMoveTasks answered {status} {listed}"
        );
        last = listed["Results"]
            .as_array()
            .and_then(|results| results.first().cloned())
            .unwrap_or(Value::Null);
        if last["Status"] == json!("COMPLETED") {
            return last;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: the move task must reach COMPLETED, last reported {last}")
}

/// RULE: a message no attempt processes is delivered as many times as the source queue's
/// `RedrivePolicy` allows and is then moved to the dead-letter queue that policy names; a move
/// task that returns it to the source queue has the repaired function process it, and the
/// dead-letter queue drains.
#[tokio::test]
async fn a_dead_lettered_message_is_reprocessed_once_it_is_moved_back() {
    let case = "a poison message redriven after the function is repaired";
    let router = router_with_queues();
    let function = "esm-redrive-run-fn";
    let (dlq_url, dlq_arn) = create_queue(&router, case, "esm-redrive-run-dlq", &json!({})).await;
    let (source_url, source_arn) = create_queue(
        &router,
        case,
        "esm-redrive-run-source",
        &json!({
            "VisibilityTimeout": VISIBILITY_TIMEOUT.to_string(),
            // "The number of times a message is delivered to the source queue before being
            // moved to the dead-letter queue."
            "RedrivePolicy": json!({
                "deadLetterTargetArn": dlq_arn,
                "maxReceiveCount": 1,
            })
            .to_string(),
        }),
    )
    .await;
    create_running_function(
        &router,
        case,
        function,
        RAISING_HANDLER,
        VISIBILITY_TIMEOUT,
        &json!({ "QUEUE_ARN": source_arn }),
    )
    .await;
    let uuid = given_enabled_mapping(
        &router,
        case,
        &json!({
            "FunctionName": function,
            "EventSourceArn": source_arn,
            "BatchSize": 1,
            "Enabled": true,
        }),
    )
    .await;

    send_message(&router, case, &source_url, "marker-1", None).await;

    // The handler raises, so the one delivery the policy allows is used up and the message is
    // moved to the dead-letter queue, leaving the source queue empty.
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut dead_lettered = 0;
    let mut counts = (u32::MAX, 0, 0);
    while Instant::now() < deadline {
        dead_lettered = queue_held(&router, case, &dlq_url).await;
        counts = queue_message_counts(&router, case, &source_url).await;
        if dead_lettered == 1 && messages_held(counts) == 0 {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        (dead_lettered, messages_held(counts)),
        (1, 0),
        "case {case:?}: the poison message must land on the dead-letter queue and leave the \
         source queue, got {counts:?} as (visible, in flight, delayed) on the source"
    );

    // The function is repaired: the same function, new code.
    let repaired = call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{function}/code"),
        &json!({
            "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(
                PROCESSING_HANDLER,
            )),
        }),
    )
    .await;
    assert_eq!(
        repaired.status,
        200,
        "case {case:?}: UpdateFunctionCode answered {} {}",
        repaired.status,
        repaired.text()
    );
    await_function_active(&router, case, function).await;

    // The operator's recovery: the dead-lettered message goes back to the queue the mapping
    // polls.
    let handle = start_move_task(&router, case, &dlq_arn, &source_arn).await;
    assert!(
        !handle.is_empty(),
        "case {case:?}: StartMessageMoveTask must answer a task handle"
    );
    let task = completed_move_task(&router, case, &dlq_arn).await;
    assert_eq!(
        task["SourceArn"],
        json!(dlq_arn),
        "case {case:?}: the task must report the dead-letter queue as its source, got {task}"
    );
    assert!(
        task["ApproximateNumberOfMessagesMoved"]
            .as_i64()
            .is_some_and(|moved| moved >= 1),
        "case {case:?}: the task must report the message it moved, got {task}"
    );

    // And the repaired consumer processes it: both queues drain, and the mapping reports the
    // attempt that consumed the message.
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut drained = (u32::MAX, u32::MAX);
    while Instant::now() < deadline {
        drained = (
            queue_held(&router, case, &source_url).await,
            queue_held(&router, case, &dlq_url).await,
        );
        if drained == (0, 0) {
            break;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    assert_eq!(
        drained,
        (0, 0),
        "case {case:?}: the redriven message must be processed and both queues drained, got \
         (source, dlq) = {drained:?}"
    );
    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        reading.string("LastProcessingResult").as_deref(),
        Some(LAST_PROCESSING_RESULT_OK),
        "case {case:?}: the mapping must report the attempt that processed the redriven message \
         as {LAST_PROCESSING_RESULT_OK}, got {}",
        reading.text()
    );
}

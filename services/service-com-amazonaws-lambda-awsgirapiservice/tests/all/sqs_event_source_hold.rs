// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon SQS queue keeps HIDDEN while it is still
//! working on a batch, asserted through the queue a slow handler's batch came off.
//!
//! The RULE asserted over the table below, in one sentence: the hold a mapping puts on the
//! messages it has taken out of a queue lasts for as long as it is really working on them, so
//! a message its function did not process is handed over again only after that attempt has
//! been decided — and the messages BEHIND such a message are consumed rather than waiting
//! for ever behind an attempt that keeps being made on the one in front of them. What an
//! attempt costs is not the function's `Timeout` alone: an invocation also waits for its
//! share of this emulator's run capacity, so the span the hold has to cover is not a function
//! of the configuration ("The parameter is applied to the messages that Amazon SQS returns in
//! the response", and a consumer still working on a message re-hides it rather than letting
//! the window lapse under it).
//!
//! Why a client cares: a consumer of a queue that reports one message of a batch as
//! unprocessed expects the rest of the queue to keep moving. A hold that lapses under a
//! running attempt makes the queue's own delivery order hand the same unprocessed message
//! over at every poll, and nothing behind it is ever delivered at all.
//!
//! The rows differ along what the rule generalises over: how much of its budget the handler
//! spends before it answers (nothing at all, and most of it — which is what makes one attempt
//! outlast one span of the hold), whether the source queue is FIFO or standard, how many
//! messages one batch may hold, and where the message the handler will not process sits in
//! the queue.
//!
//! The harness — the router with Lambda and Amazon SQS on one engine, the queue fixture, the
//! function whose package runs and the waits for what the clock owes — is the shared one in
//! `event_source_mappings.rs`, so every suite over this surface drives the same wire.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    HOST_WAIT_CEILING, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::REPORT_BATCH_ITEM_FAILURES;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    await_mapping_state, call, call_sqs, create_mapping, create_queue, create_running_function,
    queue_held, router_with_queues, send_message, LOOK_AGAIN, MAPPINGS_URI, STATE_DISABLED,
};

/// The `State` a mapping reports once it polls its event source
/// (`EventSourceMappingConfiguration$State`).
const STATE_ENABLED: &str = "Enabled";

/// The `VisibilityTimeout` every source queue below hides a delivered message for, and the
/// `Timeout` of the function it maps onto: a queue only maps onto a function it outlasts, so
/// these are the two spans the hold on a batch is computed from — and the handler of the slow
/// rows spends nearly all of one of them, which is what makes its attempt outlast that hold.
const VISIBILITY_TIMEOUT: i64 = 2;

/// How long the slow rows' handler spends before it answers, in seconds: nearly the whole
/// `Timeout` above, so the attempt plus the emulator's own work on it runs past one span of
/// the hold.
const SLOW_HANDLER_SECONDS: f64 = 1.5;

/// The marker a message body carries to be one the handler reports as unprocessed. Nothing in
/// the emulator reads it: the handler reads it out of the record's own body.
const POISON: &str = "poison";

/// How long these rows watch the QUEUE for what the mapping owes it: a pass polling,
/// invoking, deleting and coming round again for the messages the report left behind, each
/// pass costing the handler's own span.
///
/// The bound is PATIENCE, not a weakened expectation: every assertion below is unchanged. It
/// is taken from what the EMULATOR itself allows ONE attempt rather than being picked here —
/// the wait for a share of the run capacity ([`PERMIT_WAIT`]) and the wait for the run's own
/// answer ([`HOST_WAIT_CEILING`]) — with room for the several passes a row waits for, because
/// a whole-workspace test run shares its cores with every other suite.
const WATCH_BUDGET: Duration = PERMIT_WAIT
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(Duration::from_secs(30));

/// The handler every row runs: it spends `seconds` of its budget, then decides from each
/// record's own body which of them to report as unprocessed — so the report is a consequence
/// of the record having arrived, and the attempt really takes that long.
fn reporting_handler(seconds: f64) -> String {
    format!(
        r#"
import time

POISON = "{POISON}"
SPEND = {seconds}


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    if SPEND:
        time.sleep(SPEND)
    failures = [
        {{"itemIdentifier": record["messageId"]}}
        for record in records
        if POISON in record["body"]
    ]
    return {{"batchItemFailures": failures}}
"#
    )
}

/// One message of a row: its body, and the `MessageGroupId` it is sent under when the queue
/// is FIFO.
struct Sent {
    body: &'static str,
    group: Option<&'static str>,
}

/// One row: how long the handler takes, the kind of source queue, the messages waiting on it,
/// the `BatchSize` they are read in, and the bodies the queue must be left holding — the ones
/// the handler reports as unprocessed, and nothing else.
struct Row {
    case: &'static str,
    handler_seconds: f64,
    fifo: bool,
    sent: &'static [Sent],
    batch_size: i64,
    left_behind: &'static [&'static str],
}

/// The queue name a row's source queue is created under: a FIFO queue's name ends in `.fifo`
/// ("If you create a FIFO queue, your queue name must end with the .fifo suffix").
fn queue_name(case: &str, fifo: bool) -> String {
    let base = format!("esm-hold-{}", case.replace(' ', "-"));
    if fifo {
        format!("{base}.fifo")
    } else {
        base
    }
}

/// Create the mapping and wait until it reports `Enabled`, answering its UUID.
async fn given_enabled_mapping(
    router: &MultiServiceRouter,
    case: &str,
    request: &serde_json::Value,
) -> String {
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

/// Every body the queue `url` addresses still holds, read through `ReceiveMessage` until it
/// hands over nothing more. Read with the mapping PAUSED, so nothing hands a message over
/// from under this reader.
async fn remaining_bodies(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    expected: usize,
) -> BTreeSet<String> {
    let deadline = Instant::now() + WATCH_BUDGET;
    let mut bodies: BTreeSet<String> = BTreeSet::new();
    while Instant::now() < deadline && bodies.len() < expected {
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
        for message in answered["Messages"].as_array().cloned().unwrap_or_default() {
            bodies.insert(
                message["Body"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a message with no Body"))
                    .to_string(),
            );
        }
        if bodies.len() < expected {
            ::tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
    bodies
}

/// Wait until the queue `url` addresses holds `expected` messages, answering what it last
/// held.
async fn held_messages(router: &MultiServiceRouter, case: &str, url: &str, expected: u32) -> u32 {
    let deadline = Instant::now() + WATCH_BUDGET;
    loop {
        let held = queue_held(router, case, url).await;
        if held == expected || Instant::now() >= deadline {
            return held;
        }
        ::tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a mapping keeps the batch it is working on hidden for as long as the attempt takes,
/// so the messages behind one its function does not process are consumed and the queue is
/// left holding exactly the ones the report named — whatever the attempt costs.
#[tokio::test]
async fn a_slow_attempt_does_not_starve_the_messages_behind_its_batch() {
    let rows = [
        // The shape the hold decides: a STANDARD queue read ONE message per batch, whose
        // HEAD is the message the handler will not process, and a handler that spends nearly
        // its whole budget. The queue hands over its oldest visible message, so an attempt
        // that outlived its hold would be handed that same head message at every pass and
        // the two behind it would never be delivered at all.
        Row {
            case: "standard one per batch slow handler poison first",
            handler_seconds: SLOW_HANDLER_SECONDS,
            fifo: false,
            sent: &[
                Sent {
                    body: "poison-1",
                    group: None,
                },
                Sent {
                    body: "good-2",
                    group: None,
                },
                Sent {
                    body: "good-3",
                    group: None,
                },
            ],
            batch_size: 1,
            left_behind: &["poison-1"],
        },
        // The same queue and the same slow handler with the unprocessed message in the
        // MIDDLE: the message before it is consumed, and so is the one after it.
        Row {
            case: "standard one per batch slow handler poison middle",
            handler_seconds: SLOW_HANDLER_SECONDS,
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
        // A batch holding MORE than one message, still read off a standard queue by a slow
        // handler: the report names one of the two the batch held, so its partner leaves the
        // queue and the message behind them both is delivered on the next pass.
        Row {
            case: "standard two per batch slow handler",
            handler_seconds: SLOW_HANDLER_SECONDS,
            fifo: false,
            sent: &[
                Sent {
                    body: "poison-1",
                    group: None,
                },
                Sent {
                    body: "good-2",
                    group: None,
                },
                Sent {
                    body: "good-3",
                    group: None,
                },
            ],
            batch_size: 2,
            left_behind: &["poison-1"],
        },
        // A FIFO queue whose batch holds the head of one group and the message behind it,
        // read by a slow handler: the report names the head, so the message of its own group
        // the report left out leaves the queue, and the other group's message is delivered on
        // a later pass — which only happens if the hold on the batch in flight outlasts the
        // attempt that was being made on it.
        Row {
            case: "fifo slow handler two groups",
            handler_seconds: SLOW_HANDLER_SECONDS,
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
            ],
            batch_size: 2,
            left_behind: &["poison-a1"],
        },
        // And the same table row with a handler that spends NOTHING, which is the fast end of
        // the axis: the rule does not depend on what an attempt costs.
        Row {
            case: "standard one per batch prompt handler",
            handler_seconds: 0.0,
            fifo: false,
            sent: &[
                Sent {
                    body: "poison-1",
                    group: None,
                },
                Sent {
                    body: "good-2",
                    group: None,
                },
            ],
            batch_size: 1,
            left_behind: &["poison-1"],
        },
        // A slow handler that reports NOTHING: every message of every batch is processed, so
        // the queue drains however long each attempt took.
        Row {
            case: "standard slow handler nothing reported",
            handler_seconds: SLOW_HANDLER_SECONDS,
            fifo: false,
            sent: &[
                Sent {
                    body: "good-1",
                    group: None,
                },
                Sent {
                    body: "good-2",
                    group: None,
                },
            ],
            batch_size: 1,
            left_behind: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-hold-fn-{}", case.replace(' ', "-"));
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
            &reporting_handler(row.handler_seconds),
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

        // The queue comes to hold exactly as many messages as the handler reported: every
        // message the report left out was delivered and deleted, which is what the rule
        // observes — nothing waited behind the one the report named.
        let expected = u32::try_from(row.left_behind.len()).unwrap_or(u32::MAX);
        let held = held_messages(&router, case, &url, expected).await;
        assert_eq!(
            held, expected,
            "case {case:?}: the queue must be left holding the {} message(s) the report named, \
             got {held}",
            row.left_behind.len()
        );

        // And the messages left are the ones the report NAMED, body for body.
        pause_mapping(&router, case, &uuid).await;
        let remaining = remaining_bodies(&router, case, &url, row.left_behind.len()).await;
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

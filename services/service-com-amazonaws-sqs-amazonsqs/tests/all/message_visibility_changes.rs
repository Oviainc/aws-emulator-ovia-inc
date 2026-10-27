// SPDX-License-Identifier: CC0-1.0

//! `ChangeMessageVisibility` and `ChangeMessageVisibilityBatch`: the visibility timeout a
//! consumer gives ONE message it is holding, driven over the wire and observed through a
//! later receive.
//!
//! The rule under test: a visibility change applies to the message a receipt handle names
//! and is counted from the instant of the call — "the 10 seconds begin to count from the
//! time that you make the ChangeMessageVisibility call" — so the message is handed to
//! nobody else until that long has passed, whatever the queue's own `VisibilityTimeout`
//! says, and a timeout of zero hands it back at once. The change is about the message a
//! receipt handle NAMES: a handle the queue holds no message under is
//! `ReceiptHandleIsInvalid`, and a value outside the member's documented range is refused
//! before anything is changed; a handle whose visibility window has lapsed still names its
//! message, so the change is applied (see [`super::receipt_handle_lifetime`]).
//! The batch applies that same change once per entry and reports each entry under its own
//! outcome, while the whole batch is refused on the id and size rules every batch operation
//! of this service shares. An entry's own change cannot decide another entry's outcome:
//! several entries naming one message are all applied, and the LAST of them is the one
//! whose span the message keeps.
//!
//! The spans are documented durations measured in seconds at the shortest, so no row sleeps
//! for one: they move the in-flight deadline a stored message carries into the past through
//! the engine seam [`super::support::emulator_over_engine`] hands back, and everything they
//! then assert goes over the wire like any other request.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    queue_name_from_url, ComAmazonawsSqsAmazonsqsState, MAX_BATCH_ENTRIES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{call, create_ok, emulator_over_engine, error_code, messages, ok};

/// The `VisibilityTimeout` the queues below are created with, well inside the documented
/// range ("an integer from 0 to 43,200"), and long enough that no row's own wall-clock time
/// can make a received message visible again on its own.
const QUEUE_TIMEOUT: &str = "600";

/// The bounds `ChangeMessageVisibilityRequest$VisibilityTimeout` documents: "values range: 0
/// to 43200".
const MIN_TIMEOUT: i64 = 0;
const MAX_TIMEOUT: i64 = 43_200;

/// Move the in-flight deadline of every message the queue at `url` holds `by` into the past,
/// which is how a row reaches the end of a visibility timeout without sleeping for it.
fn age_visibility(engine: &engine::InMemoryEngine, url: &str, by: Duration) {
    let name = queue_name_from_url(url)
        .unwrap_or_else(|| panic!("case ageing {url}: the url names no queue"))
        .to_string();
    let store = engine.state::<ComAmazonawsSqsAmazonsqsState>();
    let mut store = store.lock();
    let queue = store
        .queues
        .get_mut(&name)
        .unwrap_or_else(|| panic!("case ageing {url}: no queue named {name}"));
    for message in &mut queue.messages {
        if let Some(until) = message.invisible_until {
            message.invisible_until = Some(until.checked_sub(by).unwrap_or(until));
        }
    }
}

/// Send one message to the queue at `url`, receive it, and answer its receipt handle — the
/// state every row starts from: one message IN FLIGHT under the queue's own timeout.
async fn in_flight_handle(router: &MultiServiceRouter, url: &str, case: &str) -> String {
    ok(
        router,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": format!("body of {case}")}),
    )
    .await;
    let received = ok(router, "ReceiveMessage", json!({"QueueUrl": url})).await;
    let handed = messages(&received);
    assert_eq!(
        handed.len(),
        1,
        "case {case:?}: the receive must hand over the one message sent, got {received}"
    );
    handed[0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a received message carries a handle, got {received}"))
        .to_string()
}

/// How many messages a receive is prepared to hand over right now.
async fn available_now(router: &MultiServiceRouter, url: &str) -> usize {
    let answer = ok(
        router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "VisibilityTimeout": 0}),
    )
    .await;
    messages(&answer).len()
}

/// One row of the visibility-change table: the timeout the call gives the message, whether
/// the message is available again straight after the call, and how far the row then has to
/// move the deadline back for it to be available.
struct ChangeRow {
    case: &'static str,
    timeout: i64,
    available_at_once: bool,
}

/// A visibility change decides when the message the handle names is handed over again: at
/// once for a timeout of zero, and not before the timeout the call named has passed.
#[tokio::test]
async fn a_changed_visibility_timeout_decides_when_the_message_comes_back() {
    let rows: &[ChangeRow] = &[
        ChangeRow {
            // "The minimum is 0 seconds": the consumer hands the message straight back.
            case: "the documented minimum, which hands the message back at once",
            timeout: MIN_TIMEOUT,
            available_at_once: true,
        },
        ChangeRow {
            case: "a timeout shorter than the queue's own",
            timeout: 30,
            available_at_once: false,
        },
        ChangeRow {
            case: "a timeout longer than the queue's own",
            timeout: 3_600,
            available_at_once: false,
        },
        ChangeRow {
            // "The maximum is 12 hours."
            case: "the documented maximum",
            timeout: MAX_TIMEOUT,
            available_at_once: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let (router, engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("visibility-change-{index}"),
            &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
        )
        .await;
        let handle = in_flight_handle(&router, &url, row.case).await;

        let changed = ok(
            &router,
            "ChangeMessageVisibility",
            json!({
                "QueueUrl": url,
                "ReceiptHandle": handle,
                "VisibilityTimeout": row.timeout,
            }),
        )
        .await;
        assert_eq!(
            changed,
            json!({}),
            "case {:?}: the modelled output has no members, got {changed}",
            row.case
        );

        let at_once = available_now(&router, &url).await;
        assert_eq!(
            at_once,
            usize::from(row.available_at_once),
            "case {:?}: expected {} message(s) available straight after the change, got {at_once}",
            row.case,
            usize::from(row.available_at_once)
        );
        if row.available_at_once {
            continue;
        }

        // The timeout is counted from the CALL, so moving the deadline back by exactly what
        // the call named is what makes the message available — and nothing shorter does.
        age_visibility(&engine, &url, Duration::from_secs(row.timeout as u64 / 2));
        let halfway = available_now(&router, &url).await;
        assert_eq!(
            halfway, 0,
            "case {:?}: the message must still be held halfway through the timeout, got \
             {halfway} available",
            row.case
        );
        age_visibility(&engine, &url, Duration::from_secs(row.timeout as u64));
        let lapsed = available_now(&router, &url).await;
        assert_eq!(
            lapsed, 1,
            "case {:?}: the message must come back once the timeout has passed, got {lapsed} \
             available",
            row.case
        );
    }
}

/// A visibility change the service cannot act on is refused with the error modelled for
/// that condition, and changes nothing.
#[tokio::test]
async fn a_visibility_change_is_refused_for_what_it_cannot_act_on() {
    let (router, _engine) = emulator_over_engine();
    let url = create_ok(
        &router,
        "visibility-refusals",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    let handle = in_flight_handle(&router, &url, "the refusals").await;

    // (case, the request, the expected status and code)
    let rows: &[(&str, Value, u16, &str)] = &[
        (
            "a handle the queue holds no message under",
            json!({
                "QueueUrl": url,
                "ReceiptHandle": "not-a-handle-this-queue-minted",
                "VisibilityTimeout": 30,
            }),
            404,
            "ReceiptHandleIsInvalid",
        ),
        (
            "a timeout below the documented minimum",
            json!({"QueueUrl": url, "ReceiptHandle": handle, "VisibilityTimeout": MIN_TIMEOUT - 1}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a timeout above the documented maximum",
            json!({"QueueUrl": url, "ReceiptHandle": handle, "VisibilityTimeout": MAX_TIMEOUT + 1}),
            400,
            "InvalidParameterValue",
        ),
        (
            "a queue this account does not hold",
            json!({
                "QueueUrl": format!("{url}-elsewhere"),
                "ReceiptHandle": handle,
                "VisibilityTimeout": 30,
            }),
            400,
            "QueueDoesNotExist",
        ),
    ];
    for (case, request, status, code) in rows {
        let (answered, body) = call(&router, "ChangeMessageVisibility", request).await;
        assert_eq!(
            (answered, error_code(&body).as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {answered} with {body}"
        );
    }
    // Every row above was refused, so the message is still in flight under the timeout its
    // receive gave it.
    let held = available_now(&router, &url).await;
    assert_eq!(
        held, 0,
        "case 'the refusals': a refused change must leave the message in flight, got {held} \
         available"
    );
}

/// The batch applies the same change once per entry and reports each entry under its own
/// outcome: the ones it made under `Successful`, and the ones it could not under `Failed`
/// with that entry's own code.
#[tokio::test]
async fn a_batch_reports_each_entry_under_its_own_outcome() {
    let (router, engine) = emulator_over_engine();
    let url = create_ok(
        &router,
        "visibility-batch",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    // Three messages in flight, so the batch has three handles of its own to act on.
    for index in 0..3 {
        ok(
            &router,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": format!("batched {index}")}),
        )
        .await;
    }
    let received = ok(
        &router,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
    )
    .await;
    let handed = messages(&received);
    assert_eq!(
        handed.len(),
        3,
        "case 'the batch': the receive must hand over all three, got {received}"
    );
    let handles: Vec<String> = handed
        .iter()
        .map(|message| {
            message["ReceiptHandle"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();

    let answered = ok(
        &router,
        "ChangeMessageVisibilityBatch",
        json!({
            "QueueUrl": url,
            "Entries": [
                // Named timeout: zero, so this one is handed back at once.
                {"Id": "back-now", "ReceiptHandle": handles[0], "VisibilityTimeout": 0},
                // No timeout at all, so the entry takes the QUEUE's own value and the
                // message stays in flight.
                {"Id": "queue_default", "ReceiptHandle": handles[1]},
                // An out-of-range value is that entry's own fault.
                {
                    "Id": "out-of-range",
                    "ReceiptHandle": handles[2],
                    "VisibilityTimeout": MAX_TIMEOUT + 1,
                },
                // A handle this queue holds nothing under is the other per-entry refusal.
                {"Id": "no-such-handle", "ReceiptHandle": "not-a-handle", "VisibilityTimeout": 30},
            ],
        }),
    )
    .await;

    let successful: Vec<String> = answered["Successful"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        successful,
        vec!["back-now".to_string(), "queue_default".to_string()],
        "case 'the batch': the entries it acted on must be reported under Successful, got \
         {answered}"
    );
    let failed: Vec<(String, String, bool)> = answered["Failed"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Id"].as_str().unwrap_or_default().to_string(),
                entry["Code"].as_str().unwrap_or_default().to_string(),
                entry["SenderFault"].as_bool().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        failed,
        vec![
            (
                "out-of-range".to_string(),
                "InvalidParameterValue".to_string(),
                true
            ),
            (
                "no-such-handle".to_string(),
                "ReceiptHandleIsInvalid".to_string(),
                true
            ),
        ],
        "case 'the batch': each entry it could not act on must carry its own code, got \
         {answered}"
    );

    // The entry that asked for zero is available now; the one that took the queue's value
    // is not, and comes back once that value has passed.
    assert_eq!(
        available_now(&router, &url).await,
        1,
        "case 'the batch': only the entry that asked for zero is back"
    );
    age_visibility(&engine, &url, Duration::from_secs(QUEUE_TIMEOUT.parse().unwrap_or(600)));
    assert_eq!(
        available_now(&router, &url).await,
        3,
        "case 'the batch': every message comes back once its timeout has passed"
    );

    // A handle whose visibility window has lapsed still names its message, so an entry
    // carrying one is applied the same way the single-message call is. The deadline is
    // moved into the past without receiving again, so the handle is still the one that
    // message is held under.
    let (lapsed_router, lapsed_engine) = emulator_over_engine();
    let lapsed_url = create_ok(
        &lapsed_router,
        "visibility-batch-lapsed",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    let lapsed_handle = in_flight_handle(&lapsed_router, &lapsed_url, "a lapsed entry").await;
    age_visibility(
        &lapsed_engine,
        &lapsed_url,
        Duration::from_secs(QUEUE_TIMEOUT.parse().unwrap_or(600)),
    );
    let lapsed = ok(
        &lapsed_router,
        "ChangeMessageVisibilityBatch",
        json!({
            "QueueUrl": lapsed_url,
            "Entries": [{"Id": "lapsed", "ReceiptHandle": lapsed_handle, "VisibilityTimeout": 30}],
        }),
    )
    .await;
    assert_eq!(
        (lapsed["Successful"][0]["Id"].clone(), lapsed["Failed"].clone()),
        (json!("lapsed"), json!([])),
        "case 'a lapsed entry': expected the entry to be applied, got {lapsed}"
    );
    let rehidden = available_now(&lapsed_router, &lapsed_url).await;
    assert_eq!(
        rehidden, 0,
        "case 'a lapsed entry': the applied entry hides the message again, got {rehidden} \
         available"
    );
}

/// The whole batch is refused — nothing changed — on the id and size rules every batch
/// operation of this service shares.
#[tokio::test]
async fn a_batch_that_is_not_one_is_refused_whole() {
    let (router, _engine) = emulator_over_engine();
    let url = create_ok(
        &router,
        "visibility-batch-refusals",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    let handle = in_flight_handle(&router, &url, "the whole-batch refusals").await;
    let entry = |id: &str| json!({"Id": id, "ReceiptHandle": handle, "VisibilityTimeout": 30});

    // (case, the entries, the expected code)
    let rows: &[(&str, Value, &str)] = &[
        ("a batch with no entry at all", json!([]), "EmptyBatchRequest"),
        (
            "two entries under one id",
            json!([entry("same"), entry("same")]),
            "BatchEntryIdsNotDistinct",
        ),
        (
            "an id with a character the member does not allow",
            json!([entry("not an id")]),
            "InvalidBatchEntryId",
        ),
        (
            // "Maximum number of entries per request are 10."
            "eleven entries",
            Value::Array((0..11).map(|index| entry(&format!("entry-{index}"))).collect()),
            "TooManyEntriesInBatchRequest",
        ),
    ];
    for (case, entries, code) in rows {
        let (status, body) = call(
            &router,
            "ChangeMessageVisibilityBatch",
            &json!({"QueueUrl": url, "Entries": entries}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *code),
            "case {case:?}: expected 400 {code}, got {status} with {body}"
        );
        // Refused whole: the message the entries name is still in flight.
        let held = available_now(&router, &url).await;
        assert_eq!(
            held, 0,
            "case {case:?}: a refused batch must change nothing, got {held} available"
        );
    }
}

/// The ids a reply reports under `member` (`Successful` or `Failed`), in the order they are
/// reported. A member no entry fell under is absent from the reply, which is the empty list
/// here.
fn reported_ids(answer: &Value, member: &str) -> Vec<String> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Each `Failed` entry as `(Id, Code)`, in the order the reply reports them.
fn reported_failures(answer: &Value) -> Vec<(String, String)> {
    answer
        .get("Failed")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["Id"].as_str().unwrap_or_default().to_string(),
                entry["Code"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// One row of the repeated-handle table: the spans the entries of ONE request ask for the
/// SAME message, in the order the request carries them. The LAST of them is the span the
/// message is then held for.
struct RepeatedRow {
    case: &'static str,
    spans: &'static [i64],
}

/// Every entry of one batch is judged against the message states the REQUEST arrived to, so
/// an entry's own change cannot decide a later entry's outcome: several entries naming one
/// in-flight message are all applied, and the message keeps the LAST entry's span.
///
/// The rows span what that generalises over: the order of a release and an extension, how
/// MANY entries name the one message (up to the ten a batch takes), and the spans
/// themselves, from the documented minimum to as much of the maximum as one receive leaves
/// to ask for.
#[tokio::test]
async fn every_entry_of_one_batch_is_judged_against_the_state_the_request_arrived_to() {
    let rows: &[RepeatedRow] = &[
        RepeatedRow {
            case: "a release, then an extension",
            spans: &[MIN_TIMEOUT, 300],
        },
        RepeatedRow {
            case: "an extension, then a release",
            spans: &[300, MIN_TIMEOUT],
        },
        RepeatedRow {
            case: "two releases",
            spans: &[MIN_TIMEOUT, MIN_TIMEOUT],
        },
        RepeatedRow {
            case: "two extensions, the shorter one first",
            spans: &[30, 600],
        },
        RepeatedRow {
            case: "two extensions, the longer one first",
            spans: &[600, 30],
        },
        RepeatedRow {
            case: "three entries ending in a release",
            spans: &[120, 600, MIN_TIMEOUT],
        },
        RepeatedRow {
            case: "three entries ending in an extension",
            spans: &[MIN_TIMEOUT, MIN_TIMEOUT, 1_800],
        },
        RepeatedRow {
            // A release leaves the whole span one receive may hide the message for still to
            // be asked for, less what the consumer has already held it.
            case: "a release, then nearly the longest span one receive may hide a message for",
            spans: &[MIN_TIMEOUT, MAX_TIMEOUT - 60],
        },
        RepeatedRow {
            // "Maximum number of entries per request are 10", all naming the one message.
            case: "the ten entries one batch takes, ending in an extension",
            spans: &[0, 30, 0, 30, 0, 30, 0, 30, 0, 900],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        assert!(
            row.spans.len() <= MAX_BATCH_ENTRIES,
            "case {:?}: a row may not ask for more entries than one batch takes",
            row.case
        );
        let (router, engine) = emulator_over_engine();
        let url = create_ok(
            &router,
            &format!("visibility-batch-repeated-{index}"),
            &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
        )
        .await;
        let handle = in_flight_handle(&router, &url, row.case).await;

        let ids: Vec<String> = (0..row.spans.len())
            .map(|entry| format!("entry-{entry}"))
            .collect();
        let entries: Vec<Value> = ids
            .iter()
            .zip(row.spans)
            .map(|(id, span)| {
                json!({"Id": id, "ReceiptHandle": handle, "VisibilityTimeout": span})
            })
            .collect();
        let answered = ok(
            &router,
            "ChangeMessageVisibilityBatch",
            json!({"QueueUrl": url, "Entries": entries}),
        )
        .await;

        assert_eq!(
            reported_ids(&answered, "Successful"),
            ids,
            "case {:?}: every entry names a message that was in flight when the request \
             arrived, so every one must be applied, got {answered}",
            row.case
        );
        assert_eq!(
            reported_failures(&answered),
            Vec::new(),
            "case {:?}: no entry of this batch can be refused, got {answered}",
            row.case
        );

        // The span the message is left under is the LAST entry's: zero hands it back at
        // once, and anything else holds it for that long from the call.
        let kept = row.spans.last().copied().unwrap_or(MIN_TIMEOUT);
        let expected = usize::from(kept == MIN_TIMEOUT);
        let at_once = available_now(&router, &url).await;
        assert_eq!(
            at_once, expected,
            "case {:?}: the message keeps the last entry's {kept} seconds, so {expected} \
             message(s) are available straight after the batch, got {at_once}",
            row.case
        );
        if kept == MIN_TIMEOUT {
            continue;
        }
        let kept_seconds = u64::try_from(kept).unwrap_or_default();
        age_visibility(&engine, &url, Duration::from_secs(kept_seconds / 2));
        let halfway = available_now(&router, &url).await;
        assert_eq!(
            halfway, 0,
            "case {:?}: halfway through the last entry's {kept} seconds the message is still \
             held, got {halfway} available",
            row.case
        );
        age_visibility(&engine, &url, Duration::from_secs(kept_seconds));
        let lapsed = available_now(&router, &url).await;
        assert_eq!(
            lapsed, 1,
            "case {:?}: the message comes back once the last entry's {kept} seconds have \
             passed, got {lapsed} available",
            row.case
        );
    }
}

/// The same judgment for a handle whose visibility window has lapsed: the handle still
/// names its message, so every entry naming it is applied — and the LAST entry's span is
/// the one the message keeps, exactly as it is for a message still hidden.
#[tokio::test]
async fn every_entry_naming_a_lapsed_handle_is_applied_and_the_last_one_wins() {
    let (router, engine) = emulator_over_engine();
    let url = create_ok(
        &router,
        "visibility-batch-lapsed-twice",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    let handle = in_flight_handle(&router, &url, "a lapsed handle named twice").await;
    age_visibility(
        &engine,
        &url,
        Duration::from_secs(QUEUE_TIMEOUT.parse().unwrap_or(600)),
    );

    let answered = ok(
        &router,
        "ChangeMessageVisibilityBatch",
        json!({
            "QueueUrl": url,
            "Entries": [
                {"Id": "extend", "ReceiptHandle": handle, "VisibilityTimeout": 30},
                {"Id": "release", "ReceiptHandle": handle, "VisibilityTimeout": MIN_TIMEOUT},
            ],
        }),
    )
    .await;

    assert_eq!(
        reported_failures(&answered),
        Vec::new(),
        "case 'a lapsed handle named twice': a lapsed handle still names its message, so no \
         entry can be refused, got {answered}"
    );
    assert_eq!(
        reported_ids(&answered, "Successful"),
        vec!["extend".to_string(), "release".to_string()],
        "case 'a lapsed handle named twice': every entry must be applied, got {answered}"
    );
    let available = available_now(&router, &url).await;
    assert_eq!(
        available, 1,
        "case 'a lapsed handle named twice': the last entry released the message, so it is \
         available again, got {available} available"
    );
}

/// The instant a change is counted from is the CALL's, not the receive's: a message changed
/// late in its timeout is held for the full new span from then.
#[tokio::test]
async fn the_new_timeout_is_counted_from_the_call() {
    let (router, engine) = emulator_over_engine();
    let url = create_ok(
        &router,
        "visibility-from-the-call",
        &json!({"VisibilityTimeout": QUEUE_TIMEOUT}),
    )
    .await;
    let handle = in_flight_handle(&router, &url, "counted from the call").await;

    // Most of the queue's own timeout has passed by the time the change is made.
    let nearly_all: u64 = QUEUE_TIMEOUT.parse::<u64>().unwrap_or(600) - 1;
    age_visibility(&engine, &url, Duration::from_secs(nearly_all));
    assert_eq!(
        available_now(&router, &url).await,
        0,
        "case 'counted from the call': the message is still in flight when the change is made"
    );
    ok(
        &router,
        "ChangeMessageVisibility",
        json!({"QueueUrl": url, "ReceiptHandle": handle, "VisibilityTimeout": 300}),
    )
    .await;

    // The span the receive gave it has long since passed, and the message is still held:
    // the new timeout started when the call was made.
    age_visibility(&engine, &url, Duration::from_secs(299));
    assert_eq!(
        available_now(&router, &url).await,
        0,
        "case 'counted from the call': the message must be held for the new span from the call"
    );
    age_visibility(&engine, &url, Duration::from_secs(2));
    assert_eq!(
        available_now(&router, &url).await,
        1,
        "case 'counted from the call': the message comes back once the new span has passed"
    );
    // Nothing above depends on the wall clock moving on its own.
    let _ = SystemTime::now();
}

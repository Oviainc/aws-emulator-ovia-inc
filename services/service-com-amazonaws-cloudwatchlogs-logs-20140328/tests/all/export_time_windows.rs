// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `[from, to]` window `CreateExportTask` is given, and the refusal
//! it answers when that window is not a time range.
//!
//! The rule under test: an export's window is a FILTER over the timestamps of the events the
//! log group holds — "Events with a timestamp earlier than this time are not exported" /
//! "later than this time are not exported" — and not a constraint against any resource's own
//! lifetime, so every window whose `from` does not exceed its `to` is accepted and stored as
//! it was sent, including a window that lies wholly in the past of the log group it exports
//! from (the normal case for events written with historical timestamps). The one window that
//! is no range at all is the one whose end precedes its start: it is refused
//! `InvalidParameterException` with the service's time-range text, whose two quoted slots
//! carry the request's own `from` and `to`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: where the window
//! sits relative to the log group's creation (wholly before it, ending one millisecond before
//! it, ending exactly at it, straddling it, wholly after it, reaching into the future), the
//! width of the window (an hour, a single instant, the epoch), and by how much an inverted
//! window is inverted (one millisecond, a minute, an hour). The request an issue happens to
//! quote is one row.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, now_ms, Logs};

/// The operations under test.
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";

/// The operation a row settles its own task with, so the next row's create is not refused the
/// account's one active slot.
const CANCEL: &str = "CancelExportTask";

/// The modelled code a window that is no range is refused under.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The bucket every case exports to.
const DESTINATION: &str = "export-window-bucket";

/// One hour in the milliseconds `CreateExportTask$from` / `$to` carry.
const HOUR_MS: i64 = 60 * 60 * 1_000;

/// The whole message CloudWatch Logs refuses a window that is not a time range with: fixed
/// text, with the request's own `from` and `to` in its two quoted slots.
fn time_range_message(from: i64, to: i64) -> String {
    format!(
        "Specified time range of '{from}' and '{to}' is not valid. Please make sure the values \
         are within the retention period of the log groups and from value is lesser than the to \
         value"
    )
}

/// A log group with one stream and three events whose timestamps are HOURS IN THE PAST, which
/// is the ingest an export of a past window reads, plus the instant the group reports being
/// created at.
async fn group_with_backdated_events(logs: &Logs, group: &str) -> i64 {
    let stream = "window-stream";
    group_with_stream(logs, group, stream, None).await;
    let base = base_ms();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": stream,
            "logEvents": batch(&[
                (base - 2 * HOUR_MS, "three hours ago"),
                (base - HOUR_MS, "two hours ago"),
                (base, "one hour ago"),
            ]),
        }),
    )
    .await;
    let reported = logs
        .describe_group(group)
        .await
        .unwrap_or_else(|| panic!("the log group {group} must exist"));
    reported["creationTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("a log group reports its creationTime, got {reported}"))
}

/// The one task `DescribeExportTasks` reports under `id`.
async fn described_task(logs: &Logs, id: &str) -> Value {
    let page = logs.ok(DESCRIBE, &json!({"taskId": id})).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"))
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the task {id} must be reported, got {page}"))
}

/// Every window that is a range is accepted, wherever it sits relative to the log group, and
/// the task stores the window as it was sent.
#[tokio::test]
async fn a_window_that_is_a_range_is_accepted_wherever_it_sits() {
    let logs = Logs::new();
    const GROUP: &str = "/export/window";
    let created_at = group_with_backdated_events(&logs, GROUP).await;
    let now = now_ms();

    // (case, from, to)
    let cases: Vec<(&str, i64, i64)> = vec![
        (
            "a window wholly in the past of the log group, around backdated events",
            now - 3 * HOUR_MS,
            now - HOUR_MS,
        ),
        (
            "a window ending one millisecond before the log group was created",
            0,
            created_at - 1,
        ),
        (
            "a window ending exactly when the log group was created",
            0,
            created_at,
        ),
        (
            "a window straddling the log group's creation",
            created_at - HOUR_MS / 2,
            created_at + HOUR_MS / 2,
        ),
        ("a window of a single instant in the past", now - HOUR_MS, now - HOUR_MS),
        ("a window at the epoch", 0, 0),
        ("a window reaching into the future", now, now + HOUR_MS),
    ];
    for (case, from, to) in cases {
        let answer = logs
            .ok(
                CREATE,
                &json!({
                    "logGroupName": GROUP,
                    "from": from,
                    "to": to,
                    "destination": DESTINATION,
                }),
            )
            .await;
        let id = answer["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: an accepted create answers a taskId, got {answer}"))
            .to_string();
        let reported = described_task(&logs, &id).await;
        assert_eq!(
            (reported["from"].as_i64(), reported["to"].as_i64()),
            (Some(from), Some(to)),
            "case {case:?}: the task stores the window it was sent, got {reported}"
        );
        assert_eq!(
            reported["logGroupName"].as_str(),
            Some(GROUP),
            "case {case:?}: the task exports from the log group the request named, got {reported}"
        );
        // "Each account can only have one active (RUNNING or PENDING) export task at a time",
        // so an accepted row settles its own task before the next one runs.
        logs.ok(CANCEL, &json!({"taskId": id})).await;
    }
}

/// A window whose end precedes its start is no range: it is refused in the service's own
/// time-range wording, carrying the values the request sent, and it starts nothing.
#[tokio::test]
async fn an_inverted_window_is_refused_in_the_services_own_wording() {
    let logs = Logs::new();
    const GROUP: &str = "/export/window-refusals";
    let created_at = group_with_backdated_events(&logs, GROUP).await;
    let now = now_ms();

    // (case, from, to) — each inverted by a different span, at a different place on the clock.
    let cases: Vec<(&str, i64, i64)> = vec![
        ("inverted by one millisecond", now, now - 1),
        ("inverted by a minute", now, now - 60 * 1_000),
        ("inverted by an hour", now, now - HOUR_MS),
        ("inverted wholly in the past of the log group", now - HOUR_MS, now - 3 * HOUR_MS),
        ("inverted across the log group's creation", created_at + HOUR_MS, created_at - HOUR_MS),
        ("inverted at the epoch", 1, 0),
    ];
    for (case, from, to) in cases {
        let (code, message) = logs
            .refusal(
                CREATE,
                &json!({
                    "logGroupName": GROUP,
                    "from": from,
                    "to": to,
                    "destination": DESTINATION,
                }),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with message {message:?}"
        );
        let expected = time_range_message(from, to);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        let page = logs.ok(DESCRIBE, &json!({})).await;
        let started = page["exportTasks"]
            .as_array()
            .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"));
        assert!(
            started.is_empty(),
            "case {case:?}: a refused create starts no task, got {page}"
        );
    }
}

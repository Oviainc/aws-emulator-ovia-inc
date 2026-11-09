// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the destination bucket holds around the account's one active
//! export: the data the ACCEPTED export delivers, and the nothing a create refused for that
//! slot leaves behind.
//!
//! The rule under test has two halves, and both are about the same instant — the first read
//! that reports a terminal status:
//!
//! * `COMPLETED` is the service's statement that the export delivered the log data, so by the
//!   time a task first reports it, the destination bucket holds the named log group's events
//!   whose timestamp falls in the task's window, under the task's own `destinationPrefix` —
//!   the whole-bucket listing taken right then is non-empty and every key in it starts with
//!   that prefix. A task that has not delivered its data has not completed: it is still
//!   `RUNNING`, which is also what keeps the account's one active-export slot held.
//! * "Each account can only have one active (RUNNING or PENDING) export task at a time": a
//!   create issued while that task is active is refused `LimitExceededException` and is
//!   therefore not an export at all — it records no task and writes no object, in the bucket
//!   it named or anywhere else.
//!
//! CloudWatch Logs and Amazon S3 are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so an object a case reads back is one the object store itself
//! holds, written by the export task over the in-process callback and not by anything here.
//!
//! Every case waits for the terminal status the way a client does (poll `DescribeExportTasks`
//! until it is terminal) and then reads the bucket with no further wait, so an export whose
//! objects land after its status settles cannot pass.
//!
//! The table spans what the rule generalises over: how many events the window selects and
//! across how many streams, what prefix the accepted export names (flat, nested, a trailing
//! slash, none at all), and what the refused create names (another prefix in the same bucket,
//! or another bucket entirely).

use serde_json::{json, Value};

use crate::export_task_objects::Emulator;
use crate::harness::{error_code, now_ms};

/// Milliseconds in one hour.
const HOUR: i64 = 60 * 60 * 1_000;

/// The modelled error code a create past the account's one active export is refused under.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The whole message that refusal carries: fixed text for the error class, naming nothing of
/// the request or of the task holding the slot.
const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

/// The `destinationPrefix` an export task gets when the create names none — "if you don't
/// specify a value, the default is exportedlogs" (`CreateExportTask$destinationPrefix`).
const DEFAULT_PREFIX: &str = "exportedlogs";

/// The statuses of the model's `ExportTaskStatusCode` enum an export task never leaves.
const TERMINAL_STATUSES: [&str; 3] = ["COMPLETED", "CANCELLED", "FAILED"];

/// The statuses that enum calls active — the two an export is under way in.
const ACTIVE_STATUSES: [&str; 2] = ["PENDING", "RUNNING"];

/// How often, and how many times, a case polls for the terminal status. The product is far
/// past the spans an export is under way for, so a settled export is never missed, and no
/// case waits for a fixed duration instead of for the status itself.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const POLL_ATTEMPTS: usize = 300;

/// One row of the table.
struct Row {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// The events the group holds, as `(stream, whole hours before the clock, message)`. The
    /// window every row exports is `[clock - 3h, clock)`, so which of these the export must
    /// carry follows from the hour each is stamped at ([`in_window`]) rather than from a
    /// second list that could disagree with it.
    events: &'static [(&'static str, i64, &'static str)],
    /// The `destinationPrefix` the accepted create names, or `None` to take the documented
    /// default.
    prefix: Option<&'static str>,
    /// The `destinationPrefix` the REFUSED create names.
    refused_prefix: &'static str,
    /// The bucket the refused create names, when it is not the accepted export's own. The
    /// case creates it, so what refuses the create is the account's slot and not the bucket.
    refused_bucket: Option<&'static str>,
}

/// Whether an event stamped `hours_before` whole hours before the clock falls in the window
/// every row exports, `[clock - 3h, clock)`: "events with a timestamp earlier than this time
/// are not exported" (`from`, inclusive) and "events with a timestamp later than this time
/// are not exported" (`to`, the start of the next window).
fn in_window(hours_before: i64) -> bool {
    (1..=3).contains(&hours_before)
}

/// The accepted export delivers the window's events under its own prefix before it reports a
/// terminal status, and the create the account's slot refuses while it does leaves no task and
/// no object behind.
#[tokio::test]
async fn an_accepted_export_delivers_its_data_while_a_second_create_is_refused() {
    let rows: [Row; 4] = [
        Row {
            case: "one event in one stream, under a flat prefix",
            events: &[("app/one", 1, "ONE-INSIDE")],
            prefix: Some("exp-first"),
            refused_prefix: "exp-second",
            refused_bucket: None,
        },
        Row {
            case: "events on both window bounds and outside it, across two streams",
            events: &[
                ("app/one", 4, "BEFORE-THE-WINDOW"),
                ("app/one", 3, "ON-THE-WINDOW-START"),
                ("app/one", 2, "INSIDE-THE-WINDOW"),
                ("sys/two", 1, "INSIDE-ANOTHER-STREAM"),
                ("app/one", 0, "ON-THE-WINDOW-END"),
            ],
            prefix: Some("deep/nested/exports"),
            refused_prefix: "deep/nested/second",
            refused_bucket: None,
        },
        Row {
            case: "no prefix named, and the refused create names another bucket",
            events: &[("app/one", 2, "DEFAULT-PREFIX-EVENT")],
            prefix: None,
            refused_prefix: "second",
            refused_bucket: Some("export-quota-other"),
        },
        Row {
            case: "a prefix that already ends in a slash",
            events: &[
                ("app/one", 5, "BEFORE-THE-WINDOW"),
                ("app/one", 2, "TRAILING-SLASH-EVENT"),
            ],
            prefix: Some("trailing/"),
            refused_prefix: "other",
            refused_bucket: None,
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        let case = row.case;
        let group = "export/quota";
        let bucket = "export-quota-destination";
        emulator.create_bucket(case, bucket).await;
        if let Some(other) = row.refused_bucket {
            emulator.create_bucket(case, other).await;
        }
        emulator
            .logs_ok(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;

        let clock = now_ms();
        let mut streams: Vec<&str> = row.events.iter().map(|(stream, _, _)| *stream).collect();
        streams.sort_unstable();
        streams.dedup();
        for stream in streams {
            emulator
                .logs_ok(
                    case,
                    "CreateLogStream",
                    &json!({"logGroupName": group, "logStreamName": stream}),
                )
                .await;
            let events: Vec<Value> = row
                .events
                .iter()
                .filter(|(planted, _, _)| *planted == stream)
                .map(|(_, hours_before, message)| {
                    json!({"timestamp": clock - hours_before * HOUR, "message": message})
                })
                .collect();
            emulator
                .logs_ok(
                    case,
                    "PutLogEvents",
                    &json!({
                        "logGroupName": group,
                        "logStreamName": stream,
                        "logEvents": Value::Array(events),
                    }),
                )
                .await;
        }

        // The accepted export: the window is `[clock - 3h, clock)`, which is what
        // [`in_window`] is stated against.
        let mut create = json!({
            "logGroupName": group,
            "from": clock - 3 * HOUR,
            "to": clock,
            "destination": bucket,
        });
        if let Some(prefix) = row.prefix {
            create["destinationPrefix"] = json!(prefix);
        }
        let started = emulator.logs_ok(case, "CreateExportTask", &create).await;
        let task_id = started["taskId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a started export reports a taskId"))
            .to_string();

        // The create answered while the export was still owed, so the task is observably
        // active — which is what makes the slot it holds observable too.
        let active = emulator.task_status(case, &task_id).await;
        assert!(
            ACTIVE_STATUSES.contains(&active.as_str()),
            "case {case:?}: a just-created export is one of {ACTIVE_STATUSES:?}, got {active}"
        );

        // The second create, while that task holds the account's slot.
        let mut refused = create.clone();
        refused["destinationPrefix"] = json!(row.refused_prefix);
        if let Some(other) = row.refused_bucket {
            refused["destination"] = json!(other);
        }
        let (status, body) = emulator.logs("CreateExportTask", &refused).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create while one is active is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            LIMIT_EXCEEDED,
            "case {case:?}: expected {LIMIT_EXCEEDED}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(LIMIT_EXCEEDED_MESSAGE),
            "case {case:?}: expected the message {LIMIT_EXCEEDED_MESSAGE:?}, got {body}"
        );

        // The refused create started nothing: the account holds the one task it accepted.
        let described = emulator
            .logs_ok(case, "DescribeExportTasks", &json!({}))
            .await;
        let reported: Vec<String> = described["exportTasks"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a page reports exportTasks, got {described}"))
            .iter()
            .map(|task| task["taskId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            vec![task_id.clone()],
            "case {case:?}: only the accepted create minted a task, got {described}"
        );

        // Waiting for the terminal status is all a client does before reading the data, so
        // nothing else is waited for here.
        let settled = await_terminal(&emulator, case, &task_id).await;
        assert_eq!(
            settled, "COMPLETED",
            "case {case:?}: the accepted export must complete, got {settled}"
        );

        // "If you don't specify a value, the default is exportedlogs", and the prefix is "the
        // start of the key for every object exported", so one trailing slash and no more
        // separates it from the rest of the key.
        let prefix = row.prefix.unwrap_or(DEFAULT_PREFIX);
        let key_prefix = format!("{}/", prefix.trim_end_matches('/'));
        let keys = emulator.all_keys(case, bucket).await;
        assert!(
            !keys.is_empty(),
            "case {case:?}: a completed export has written its data, got {keys:?}"
        );
        for key in &keys {
            assert!(
                key.starts_with(&key_prefix) && key.ends_with(".gz"),
                "case {case:?}: every object of the export is a gzip object under \
                 {key_prefix}, got {key}"
            );
        }

        // Nothing of the refused create: no object under the prefix it named, and none
        // anywhere in the bucket it named when that was another one.
        let refused_keys = emulator.keys_under(case, bucket, row.refused_prefix).await;
        assert_eq!(
            refused_keys,
            Vec::<String>::new(),
            "case {case:?}: the refused create wrote nothing under {}, got {refused_keys:?}",
            row.refused_prefix
        );
        if let Some(other) = row.refused_bucket {
            let other_keys = emulator.all_keys(case, other).await;
            assert_eq!(
                other_keys,
                Vec::<String>::new(),
                "case {case:?}: the refused create wrote nothing in {other}, got {other_keys:?}"
            );
        }

        // The data itself: the window's events are there and the others are not.
        let text = emulator.exported_text(case, bucket, prefix).await;
        for (_, hours_before, message) in row.events {
            assert_eq!(
                text.contains(message),
                in_window(*hours_before),
                "case {case:?}: {message} is stamped {hours_before}h before the window's end, \
                 so in_window={}, got the exported text {text:?}",
                in_window(*hours_before)
            );
        }
    }
}

/// The terminal status `task_id` settles at, polled for the way a client polls for it.
async fn await_terminal(emulator: &Emulator, case: &str, task_id: &str) -> String {
    for _ in 0..POLL_ATTEMPTS {
        let status = emulator.task_status(case, task_id).await;
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return status;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the export task must settle within \
         {POLL_ATTEMPTS:?} polls, and still reports an active status"
    )
}

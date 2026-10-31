// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `DescribeExportTasks` reports in `ExportTask$status`.
//!
//! The rule under test: an export task's reported status is the PAIR the model declares —
//! `code` ("the status code of the export task") and `message` ("the status message related
//! to the status code") — so a task that has settled reports the message of the terminal
//! status it settled at, whichever way it got there, and that message is fixed text for the
//! status class carrying nothing of the request or of the stored task; a task whose export is
//! still under way has reached no outcome yet and so reports a code alone.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: which terminal
//! status the task settles at and how it got there (the clock finished the export, a caller
//! cancelled it before it started, a caller cancelled it while it was exporting), which
//! optional members the task was created with, which non-terminal status it is read in, and
//! which filter (`taskId`, `statusCode`, neither) the status is read through. The cancelled
//! task an issue happens to mention is one row.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{mid_export, now_ms, past_completion, Logs};

/// The operations under test.
const CANCEL: &str = "CancelExportTask";
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";

/// The statuses of the model's `ExportTaskStatusCode` enum this service's export tasks reach.
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";
const CANCELLED: &str = "CANCELLED";
const COMPLETED: &str = "COMPLETED";

/// The whole status message each terminal class carries, as `ExportTaskStatus$message`.
const CANCELLED_BY_USER: &str = "Export task cancelled by user";
const COMPLETED_SUCCESSFULLY: &str = "Completed successfully";

/// The bucket every case exports to, and the log group every case exports from.
const BUCKET: &str = "export-bucket";
const GROUP: &str = "/export/status-messages";

/// How a case settles the task it created.
#[derive(Debug, Clone, Copy)]
enum Settle {
    /// Wait for the clock to finish the export.
    OnTheClock,
    /// Cancel the task, after waiting the given span first.
    Cancel(Option<Duration>),
}

/// The tasks `DescribeExportTasks` reports for `body`.
async fn described(logs: &Logs, body: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, body).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"))
        .clone()
}

/// The `status` object the service reports for the task under `id`, read through `filter`.
async fn status_through(logs: &Logs, case: &str, filter: &Value, id: &str) -> Value {
    described(logs, filter)
        .await
        .into_iter()
        .find(|task| task["taskId"].as_str() == Some(id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the task {id} must be reported for the filter {filter}")
        })["status"]
        .clone()
}

/// Start one export of the suite's log group, answering the `taskId` the create minted.
async fn start_export(logs: &Logs, optional: &Value) -> String {
    let now = now_ms();
    let mut body = json!({
        "logGroupName": GROUP,
        "from": now - 60 * 60 * 1_000,
        "to": now,
        "destination": BUCKET,
    });
    if let Some(members) = optional.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    let answer = logs.ok(CREATE, &body).await;
    answer["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExportTask answers a taskId, got {answer}"))
        .to_string()
}

/// Whichever terminal status an export task settles at, the status it reports is that code
/// AND the message that status carries — read through every filter the report takes, and the
/// same on a second read.
#[tokio::test]
async fn a_settled_export_task_reports_the_message_of_the_status_it_settled_at() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // (case, the optional members the create carried, how the task settles, the status it
    // settles at, the whole message that status carries)
    let cases: Vec<(&str, Value, Settle, &str, &str)> = vec![
        (
            "the clock finished a task created with no optional member",
            json!({}),
            Settle::OnTheClock,
            COMPLETED,
            COMPLETED_SUCCESSFULLY,
        ),
        (
            "the clock finished a named task exporting to a prefix",
            json!({"taskName": "nightly", "destinationPrefix": "exports/nightly"}),
            Settle::OnTheClock,
            COMPLETED,
            COMPLETED_SUCCESSFULLY,
        ),
        (
            "the clock finished a task exporting one stream prefix",
            json!({"logStreamNamePrefix": "app-"}),
            Settle::OnTheClock,
            COMPLETED,
            COMPLETED_SUCCESSFULLY,
        ),
        (
            "a caller cancelled a task that had not started exporting",
            json!({}),
            Settle::Cancel(None),
            CANCELLED,
            CANCELLED_BY_USER,
        ),
        (
            "a caller cancelled a named task that was exporting",
            json!({"taskName": "stopped", "destinationPrefix": "exports/stopped"}),
            Settle::Cancel(Some(mid_export())),
            CANCELLED,
            CANCELLED_BY_USER,
        ),
    ];
    for (case, optional, settle, expected_code, expected_message) in cases {
        // "Each account can only have one active (RUNNING or PENDING) export task at a time",
        // so every row settles its own task before the next row creates one.
        let id = start_export(&logs, &optional).await;
        match settle {
            Settle::OnTheClock => tokio::time::sleep(past_completion()).await,
            Settle::Cancel(wait) => {
                if let Some(wait) = wait {
                    tokio::time::sleep(wait).await;
                }
                logs.ok(CANCEL, &json!({"taskId": id})).await;
            }
        }

        let by_id = json!({"taskId": id});
        let status = status_through(&logs, case, &by_id, &id).await;
        assert_eq!(
            status,
            json!({"code": expected_code, "message": expected_message}),
            "case {case:?}: expected the status {{{expected_code}, {expected_message:?}}}, got \
             {status}"
        );

        // The message is fixed text for the status class: nothing of the request or of the
        // stored task is interpolated into it.
        let message = status["message"].as_str().unwrap_or_default().to_string();
        let mut carried: Vec<String> = vec![id.clone(), GROUP.to_string(), BUCKET.to_string()];
        if let Some(members) = optional.as_object() {
            carried.extend(members.values().filter_map(|value| {
                value.as_str().map(str::to_string)
            }));
        }
        for value in carried {
            assert!(
                !message.contains(&value),
                "case {case:?}: the status message carries nothing of the request or the stored \
                 task, got {value:?} in {message:?}"
            );
        }

        // Every filter the report takes answers the same status for this task, and so does a
        // second read: the status is what the store holds, not what the request asked for.
        let filters: Vec<(&str, Value)> = vec![
            ("no filter", json!({})),
            ("the task's id", by_id.clone()),
            (
                "the status the task settled at",
                json!({"statusCode": expected_code}),
            ),
            ("the id again", by_id.clone()),
        ];
        for (filter_case, filter) in filters {
            let again = status_through(&logs, case, &filter, &id).await;
            assert_eq!(
                again, status,
                "case {case:?} read through {filter_case}: expected the same status {status}, \
                 got {again}"
            );
        }
    }
}

/// An export task whose export is still under way has reached no outcome, so the status it
/// reports is the code alone: `ExportTaskStatus$message` is the message RELATED TO the status
/// code, and there is none for a status the task has not settled at.
#[tokio::test]
async fn an_export_task_still_under_way_reports_a_code_alone() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // (case, how long after the create the task is read, the status it is in then)
    let cases: Vec<(&str, Option<Duration>, &str)> = vec![
        ("a task that has not started exporting", None, PENDING),
        ("a task that is exporting", Some(mid_export()), RUNNING),
    ];
    for (case, wait, expected_code) in cases {
        let id = start_export(&logs, &json!({})).await;
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
        let status = status_through(&logs, case, &json!({"taskId": id}), &id).await;
        assert_eq!(
            status["code"].as_str(),
            Some(expected_code),
            "case {case:?}: expected the status code {expected_code}, got {status}"
        );
        assert!(
            status["message"].is_null(),
            "case {case:?}: expected no status message, got {status}"
        );

        // Settle the task, so the next row's create is not refused the account's one active
        // slot.
        logs.ok(CANCEL, &json!({"taskId": id})).await;
    }
}

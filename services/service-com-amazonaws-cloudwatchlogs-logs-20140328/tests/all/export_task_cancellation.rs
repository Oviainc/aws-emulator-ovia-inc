// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelExportTask`: stopping an export that is still under way, and
//! the two refusals a cancel that has nothing to stop is answered with.
//!
//! The rule under test: a cancel applies to the export task the `taskId` names and only
//! while that task is in one of the statuses an export is still under way in — "The task
//! must be in the PENDING or RUNNING state" — which it then leaves `CANCELLED`, discarding
//! the transitions the clock had armed for it and reporting the status message that status
//! carries ("the status message related to the status code"); a task in any other status is
//! refused `InvalidOperationException` with the fixed text its class carries, an id the
//! account holds no task under is refused `ResourceNotFoundException` naming the kind of
//! resource, and either refusal leaves the stored task exactly as it was, so a report after
//! it answers what it answered before.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: which non-terminal
//! status the task is in when the cancel arrives (not started yet, exporting), which optional
//! members the task carries, how the task came to be settled (finished on the clock, already
//! cancelled, failed), which status the reported status message belongs to, and which form an
//! id the account holds no task under takes — including each published bound of the modelled
//! `ExportTaskId` length and one past it. The settled task an issue happens to mention is one
//! row.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, ExportTaskRecord, EXPORT_TASK_STATUS_FAILED,
};

use crate::harness::{mid_export, now_ms, past_completion, Logs};

/// The operations under test.
const CANCEL: &str = "CancelExportTask";
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";

/// The modelled error codes the rule names.
const INVALID_OPERATION: &str = "InvalidOperationException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The statuses an export task is in while the export is still under way, as the model's
/// `ExportTaskStatusCode` enum spells them.
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";

/// The statuses a settled export task carries, from the same enum.
const CANCELLED: &str = "CANCELLED";
const COMPLETED: &str = "COMPLETED";

/// The whole message a cancel of a task that is no longer under way is refused with: fixed
/// text for the error class, carrying nothing of the request or of the stored task.
const ALREADY_FINISHED: &str = "The specified export task has already finished";

/// The whole status message a cancelled export task reports beside that status code: what
/// `ExportTaskStatus$message` ("the status message related to the status code") says about a
/// task a caller stopped.
const CANCELLED_BY_USER: &str = "Export task cancelled by user";

/// The whole status message an export task that finished reports beside that status code,
/// from the same member.
const COMPLETED_SUCCESSFULLY: &str = "Completed successfully";

/// The status message the seeded failed task is stood up carrying: nothing in this emulator
/// fails an export, so the reason a failed export would report is supplied by the seed and
/// read back to show the report answers from the stored record.
const SEEDED_FAILURE_REASON: &str = "Export task failed";

/// The whole message a cancel naming an id the account holds no task under is refused with:
/// the service's missing-resource sentence, naming the kind of resource that is not there.
const TASK_NOT_FOUND: &str = "The specified export task does not exist.";

/// The published bounds of the modelled `ExportTaskId` (`@length min 1 / max 512`), which a
/// row of unknown ids walks and then steps past.
const TASK_ID_MIN_LEN: usize = 1;
const TASK_ID_MAX_LEN: usize = 512;

/// The tasks `DescribeExportTasks` reports for `body`.
async fn described(logs: &Logs, body: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, body).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"))
        .clone()
}

/// The one task the service reports under `id`.
async fn reported(logs: &Logs, case: &str, id: &str) -> Value {
    described(logs, &json!({"taskId": id}))
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the task {id} must be reported"))
}

/// The status code the service reports for the task under `id`.
async fn status_of(logs: &Logs, case: &str, id: &str) -> String {
    let task = reported(logs, case, id).await;
    task["status"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a reported task carries a status code, got {task}"))
        .to_string()
}

/// Start one export of `group`, answering the `taskId` the create minted.
async fn start_export(logs: &Logs, group: &str, optional: &Value) -> String {
    let now = now_ms();
    let mut body = json!({
        "logGroupName": group,
        "from": now - 60 * 60 * 1_000,
        "to": now,
        "destination": "export-bucket",
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

/// A cancel that must be accepted.
async fn cancel(logs: &Logs, id: &str) {
    logs.ok(CANCEL, &json!({"taskId": id})).await;
}

/// A cancel of a task that is still under way stops it: the task is `CANCELLED` from then on,
/// the transitions the clock had armed for it never land, and the slot the account's one
/// active task holds is free again.
#[tokio::test]
async fn a_cancel_stops_an_export_that_is_still_under_way() {
    let logs = Logs::new();
    const GROUP: &str = "/export/cancellable";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // (case, how long after the create the cancel arrives, the status the task is in then,
    // the optional members the create carried)
    let cases: Vec<(&str, Option<Duration>, &str, Value)> = vec![
        (
            "a task that has not started exporting",
            None,
            PENDING,
            json!({}),
        ),
        (
            "a task that is exporting",
            Some(mid_export()),
            RUNNING,
            json!({"taskName": "nightly", "destinationPrefix": "exports/nightly"}),
        ),
        (
            "a task that is exporting one stream prefix",
            Some(mid_export()),
            RUNNING,
            json!({"logStreamNamePrefix": "app-"}),
        ),
    ];
    for (case, wait, expected_status, optional) in cases {
        let id = start_export(&logs, GROUP, &optional).await;
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
        let before = status_of(&logs, case, &id).await;
        assert_eq!(
            before, expected_status,
            "case {case:?}: expected the task to be {expected_status} when the cancel arrives, \
             got {before}"
        );

        cancel(&logs, &id).await;
        let task = reported(&logs, case, &id).await;
        assert_eq!(
            task["status"]["code"].as_str(),
            Some(CANCELLED),
            "case {case:?}: a cancelled task is {CANCELLED}, got {task}"
        );
        let completed_at = task["executionInfo"]["completionTime"].as_i64();
        assert!(
            completed_at.is_some_and(|finished| finished
                >= task["executionInfo"]["creationTime"].as_i64().unwrap_or(0)),
            "case {case:?}: a cancelled task reports when it stopped, got {task}"
        );
        // Everything the task was created with survives the cancel: a cancel settles the
        // export, it does not rewrite it.
        assert_eq!(
            task["taskName"].as_str(),
            optional["taskName"].as_str(),
            "case {case:?}: the cancelled task still reports the name it was created with, \
             got {task}"
        );

        // The transitions the clock had armed are discarded: past the instant the create's
        // completion was due the task is still cancelled, at the same instant it stopped.
        tokio::time::sleep(past_completion()).await;
        let settled = reported(&logs, case, &id).await;
        assert_eq!(
            settled["status"]["code"].as_str(),
            Some(CANCELLED),
            "case {case:?}: past the span the create armed the task is still {CANCELLED}, got \
             {settled}"
        );
        assert_eq!(
            settled["executionInfo"]["completionTime"].as_i64(),
            completed_at,
            "case {case:?}: the instant the task stopped does not move, got {settled}"
        );
    }

    // "Each account can only have one active (RUNNING or PENDING) export task at a time": a
    // cancelled task is not active, so the creates above were all accepted in turn — and one
    // more still is.
    let last = start_export(&logs, GROUP, &json!({})).await;
    assert_eq!(
        status_of(&logs, "a create after the cancels", &last).await,
        PENDING,
        "case a create after the cancels: the cancelled tasks hold no active slot"
    );
}

/// `ExportTaskStatus$message` is "the status message related to the status code": a task that
/// has SETTLED reports the message of the terminal status it settled at, whichever way it got
/// there, while a task whose export is still under way has reached no outcome and so carries
/// no message.
#[tokio::test]
async fn a_settled_task_reports_the_status_message_that_status_carries() {
    let logs = Logs::new();
    const GROUP: &str = "/export/status-message";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // One task per status an export task reaches, each driven there the way a client drives
    // it — the account holds one active task at a time, so each is settled before the next is
    // created — and the reported task is captured in the status it was read in.
    let pending = start_export(&logs, GROUP, &json!({})).await;
    let under_way = reported(&logs, "a task whose export is under way", &pending).await;
    cancel(&logs, &pending).await;
    let cancelled_while_pending =
        reported(&logs, "a task cancelled before it started exporting", &pending).await;

    let running = start_export(&logs, GROUP, &json!({})).await;
    tokio::time::sleep(mid_export()).await;
    let while_running = reported(&logs, "a task that is exporting", &running).await;
    cancel(&logs, &running).await;
    let cancelled_while_running =
        reported(&logs, "a task cancelled while it was exporting", &running).await;

    let finished = start_export(&logs, GROUP, &json!({})).await;
    tokio::time::sleep(past_completion()).await;
    let finished_on_the_clock = reported(&logs, "a task the clock finished", &finished).await;

    // A failed export is a status the model carries that no request of this service reaches,
    // so it is stood up in the store and read back over the wire like the rows above.
    let failed = seed_failed_task(&logs, GROUP);
    let failed_task = reported(&logs, "a task whose export failed", &failed).await;

    // (case, the task as it was reported, the statuses that reading admits, the message that
    // status carries — `None` where the export has not reached an outcome yet). A row admits
    // two statuses where the read races the clock the create armed: a task read right after
    // the create has not started exporting unless that instant has already passed, and either
    // way the export is still under way.
    let cases: Vec<(&str, &Value, Vec<&str>, Option<&str>)> = vec![
        (
            "a task whose export is under way",
            &under_way,
            vec![PENDING, RUNNING],
            None,
        ),
        (
            "a task that is exporting",
            &while_running,
            vec![RUNNING],
            None,
        ),
        (
            "a task the clock finished",
            &finished_on_the_clock,
            vec![COMPLETED],
            Some(COMPLETED_SUCCESSFULLY),
        ),
        (
            "a task whose export failed",
            &failed_task,
            vec![EXPORT_TASK_STATUS_FAILED],
            Some(SEEDED_FAILURE_REASON),
        ),
        (
            "a task cancelled before it started exporting",
            &cancelled_while_pending,
            vec![CANCELLED],
            Some(CANCELLED_BY_USER),
        ),
        (
            "a task cancelled while it was exporting",
            &cancelled_while_running,
            vec![CANCELLED],
            Some(CANCELLED_BY_USER),
        ),
    ];
    for (case, task, admitted, expected_message) in cases {
        let code = task["status"]["code"].as_str().unwrap_or_default();
        assert!(
            admitted.contains(&code),
            "case {case:?}: expected one of {admitted:?}, got {task}"
        );
        assert_eq!(
            task["status"]["message"].as_str(),
            expected_message,
            "case {case:?}: expected the status message {expected_message:?}, got {task}"
        );
    }
}

/// A cancel of a task that is no longer under way is refused with the fixed text of its error
/// class, whichever way the task came to be settled — and the task is left exactly as it was.
#[tokio::test]
async fn a_cancel_of_a_settled_task_is_refused_and_changes_nothing() {
    let logs = Logs::new();
    const GROUP: &str = "/export/settled";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;

    // The ways an export task comes to be settled. `FAILED` is a status the model's
    // `ExportTaskStatusCode` enum carries that no REQUEST of this service produces — nothing
    // fails an export — so that row is stood up in the store and then asserted over the wire
    // like the others.
    let finished = start_export(&logs, GROUP, &json!({})).await;
    tokio::time::sleep(past_completion()).await;
    let cancelled = start_export(&logs, GROUP, &json!({"taskName": "stopped"})).await;
    cancel(&logs, &cancelled).await;
    let failed = seed_failed_task(&logs, GROUP);

    // (case, the task's id, the status it is settled in)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a task the clock finished", finished.as_str(), COMPLETED),
        ("a task already cancelled", cancelled.as_str(), CANCELLED),
        (
            "a task whose export failed",
            failed.as_str(),
            EXPORT_TASK_STATUS_FAILED,
        ),
    ];
    for (case, id, expected_status) in cases {
        let before = reported(&logs, case, id).await;
        assert_eq!(
            before["status"]["code"].as_str(),
            Some(expected_status),
            "case {case:?}: expected the task to be {expected_status} before the cancel, got \
             {before}"
        );

        // Asking twice refuses identically: the refusal follows the stored status, not the
        // history of the request.
        for attempt in ["the first cancel", "the same cancel again"] {
            let (code, message) = logs.refusal(CANCEL, &json!({"taskId": id})).await;
            assert_eq!(
                code, INVALID_OPERATION,
                "case {case:?} ({attempt}): expected {INVALID_OPERATION}, got {code} {message:?}"
            );
            assert_eq!(
                message, ALREADY_FINISHED,
                "case {case:?} ({attempt}): expected {ALREADY_FINISHED:?}, got {message:?}"
            );
            for carried in [id, GROUP, "export-bucket", expected_status] {
                assert!(
                    !message.contains(carried),
                    "case {case:?} ({attempt}): the refusal carries nothing of the request or \
                     the stored task, got {carried:?} in {message:?}"
                );
            }
        }

        let after = reported(&logs, case, id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused cancel leaves the task untouched, got {after}"
        );
    }
}

/// An id the account holds no export task under names nothing to cancel, in every form such
/// an id takes — and the cancel changes nothing.
#[tokio::test]
async fn an_id_no_task_is_held_under_is_refused_as_a_missing_resource() {
    let logs = Logs::new();
    const GROUP: &str = "/export/unknown-ids";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    // One live task, so the refusals below are a lookup against the store rather than a
    // handler that refuses every cancel.
    let live = start_export(&logs, GROUP, &json!({})).await;
    let held = held_task_ids(&logs).await;

    // (case, the id the cancel names, the code it is refused under)
    let cases: Vec<(String, String, &str)> = vec![
        (
            "an id in the form a create mints that none minted".to_string(),
            "00000000-0000-4000-8000-ffffffffffff".to_string(),
            NOT_FOUND,
        ),
        (
            "an id in no form a create mints".to_string(),
            "not-a-task".to_string(),
            NOT_FOUND,
        ),
        (
            format!("an id of the modelled minimum {TASK_ID_MIN_LEN} character"),
            "x".repeat(TASK_ID_MIN_LEN),
            NOT_FOUND,
        ),
        (
            format!("an id of the modelled maximum {TASK_ID_MAX_LEN} characters"),
            "x".repeat(TASK_ID_MAX_LEN),
            NOT_FOUND,
        ),
        (
            format!("an id one character past the modelled maximum of {TASK_ID_MAX_LEN}"),
            "x".repeat(TASK_ID_MAX_LEN + 1),
            INVALID_PARAMETER,
        ),
    ];
    for (case, id, expected) in cases {
        let (code, message) = logs.refusal(CANCEL, &json!({"taskId": id})).await;
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code} {message:?}"
        );
        if expected == NOT_FOUND {
            assert_eq!(
                message, TASK_NOT_FOUND,
                "case {case:?}: expected {TASK_NOT_FOUND:?}, got {message:?}"
            );
        }
        assert_eq!(
            held_task_ids(&logs).await,
            held,
            "case {case:?}: a refused cancel leaves the account's tasks as they were"
        );
    }

    // Positive control: the task the store really holds is cancellable on the same request
    // the unknown ids were refused on.
    cancel(&logs, &live).await;
    assert_eq!(
        status_of(&logs, "the live task", &live).await,
        CANCELLED,
        "case the live task: the cancel that the unknown ids were refused for is accepted"
    );
}

/// The ids of every export task the account holds, which a refused cancel must not change.
/// The STATUSES of those tasks move on the clock whether or not anything calls, so it is
/// the set of tasks that is compared across a refusal and not the statuses they report.
async fn held_task_ids(logs: &Logs) -> Vec<String> {
    described(logs, &json!({}))
        .await
        .iter()
        .map(|task| task["taskId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Record, in the store, an export task of `group` whose export FAILED — a status the model
/// carries and no request of this service reaches, so a cancel of a failed task can be
/// asserted over the wire at all. Answers the id the task is held under.
fn seed_failed_task(logs: &Logs, group: &str) -> String {
    let id = "failed-export-task".to_string();
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let created = now_ms() - 60 * 1_000;
    let mut record = ExportTaskRecord::new(&id, group, created - 1_000, created, "export-bucket", created);
    record.status_code = EXPORT_TASK_STATUS_FAILED.to_string();
    record.status_message = Some(SEEDED_FAILURE_REASON.to_string());
    record.completion_time_ms = Some(created + 1_000);
    record.running_due_ms = None;
    record.completion_due_ms = None;
    store.export_tasks.insert(id.clone(), record);
    id
}

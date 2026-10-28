// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one export task an ACCOUNT can have under way at a time, and the
//! refusal a `CreateExportTask` that would take a second one is answered with.
//!
//! The rule under test: "Each account can only have one active (RUNNING or PENDING) export
//! task at a time", so while the account holds an active task EVERY further create is refused
//! `LimitExceededException` carrying the service's fixed limit sentence — nothing of the
//! request and nothing of the task holding the slot enters the refusal, and the refused create
//! starts nothing and leaves the holding task as it was. The slot belongs to the account and
//! not to a log group, a window, a bucket or a prefix, so a create whose window overlaps the
//! holding task's is refused on exactly the same terms as one whose window is disjoint from
//! it; and once the holding task has settled — on the clock or because a caller cancelled it —
//! the very same request is accepted.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! a handler does not actually compute cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: how the second
//! create's window relates to the holding task's (identical, overlapping at either end,
//! wholly inside, disjoint before and after), whether it names the same log group, bucket,
//! prefix or task name, which of the two under-way statuses the holding task is in when the
//! second create arrives, and how that task later settles.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    ComAmazonawsCloudwatchlogsLogs20140328State, EXPORT_TASK_COMPLETION_DELAY_MS,
    EXPORT_TASK_RUNNING_DELAY_MS,
};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";
const CANCEL: &str = "CancelExportTask";

/// The modelled error code a create past the account's one active task is refused under.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The whole message that refusal carries: fixed text for the error class, naming neither the
/// limit nor anything of the request or of the task holding the slot.
const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

/// The statuses an export is still under way in, as the model's `ExportTaskStatusCode` enum
/// spells them.
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";

/// The two log groups the rows export from, so a row can name the holding task's group or
/// another one.
const GROUP: &str = "/export/slot";
const OTHER_GROUP: &str = "/export/slot-other";

/// One hour, in the milliseconds `CreateExportTask$from` / `$to` carry.
const HOUR_MS: i64 = 60 * 60 * 1_000;

/// How long a case waits before treating a started task as finished: past both spans the
/// create armed, with room for the test's own scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(
        u64::try_from(2 * (EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_TASK_COMPLETION_DELAY_MS))
            .expect("a span of the clock is not negative"),
    )
}

/// A margin well beyond the spans an export is under way for, so a transition pinned this far
/// out cannot fall due while a case makes the handful of calls it has left.
const FAR_OUT_MS: i64 = 10 * (EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_TASK_COMPLETION_DELAY_MS);

/// Pin the instants the clock owes the task under `id`, so the status it is reported in while
/// the case runs is the case's choice (`RUNNING` when `running`, `PENDING` otherwise) and not a
/// race against the spans the create armed.
///
/// The record is reached in the store because no REQUEST of this service moves a task between
/// the two statuses an export is under way in — the clock does — and everything each case then
/// asserts goes over the wire like any client's call.
fn pin_under_way(logs: &Logs, id: &str, running: bool) {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let mut store = state.lock();
    let task = store
        .export_tasks
        .get_mut(id)
        .unwrap_or_else(|| panic!("the task {id} the create started is stored"));
    let now = now_ms();
    let running_due = if running { now - 1 } else { now + FAR_OUT_MS };
    task.running_due_ms = Some(running_due);
    task.completion_due_ms = Some(running_due + FAR_OUT_MS);
}

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

/// What a task reports of the export it WAS created for, which no later call changes: the
/// window, the bucket, the prefix and the name.
fn export_identity(task: &Value) -> Value {
    json!({
        "logGroupName": task["logGroupName"],
        "from": task["from"],
        "to": task["to"],
        "destination": task["destination"],
        "destinationPrefix": task["destinationPrefix"],
        "taskName": task["taskName"],
    })
}

/// The `taskId` a successful create answered.
fn task_id(answer: &Value) -> String {
    answer["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExportTask answers a taskId, got {answer}"))
        .to_string()
}

/// A create of `group` over `[from, to]`, carrying whatever optional members `optional` names.
fn create_body(group: &str, from: i64, to: i64, optional: &Value) -> Value {
    let mut body = json!({
        "logGroupName": group,
        "from": from,
        "to": to,
        "destination": "export-bucket",
    });
    if let Some(members) = optional.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// One row's second create: the request it sends while the account already holds a task.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The log group the second create exports.
    group: &'static str,
    /// The second create's window, as offsets in milliseconds from the holding task's `from`
    /// (whose own window is that instant plus one hour).
    from_offset: i64,
    to_offset: i64,
    /// The optional members the second create names.
    optional: Value,
}

/// How the task holding the account's slot comes to settle.
enum Settle {
    /// Nobody touches it: the clock finishes the export it armed.
    OnTheClock,
    /// A caller cancels it while it is still under way.
    Cancelled,
}

/// Every create made while the account holds an active task is refused the same way, whatever
/// window, group, bucket, prefix or task name it names — and starts nothing.
#[tokio::test]
async fn a_create_while_a_task_is_active_is_refused_whatever_it_names() {
    let logs = Logs::new();
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }

    let rows = vec![
        Row {
            case: "the holding task's own window, repeated",
            group: GROUP,
            from_offset: 0,
            to_offset: HOUR_MS,
            optional: json!({}),
        },
        Row {
            case: "a window overlapping the holding task's at its end",
            group: GROUP,
            from_offset: HOUR_MS / 2,
            to_offset: 2 * HOUR_MS,
            optional: json!({}),
        },
        Row {
            case: "a window overlapping the holding task's at its start",
            group: GROUP,
            from_offset: -HOUR_MS,
            to_offset: HOUR_MS / 2,
            optional: json!({}),
        },
        Row {
            case: "a window wholly inside the holding task's",
            group: GROUP,
            from_offset: HOUR_MS / 4,
            to_offset: HOUR_MS / 2,
            optional: json!({"taskName": "inner", "destinationPrefix": "exports/inner"}),
        },
        Row {
            case: "a window wholly before the holding task's",
            group: GROUP,
            from_offset: -3 * HOUR_MS,
            to_offset: -2 * HOUR_MS,
            optional: json!({}),
        },
        Row {
            case: "a window wholly after the holding task's",
            group: GROUP,
            from_offset: 2 * HOUR_MS,
            to_offset: 3 * HOUR_MS,
            optional: json!({}),
        },
        Row {
            case: "another log group over an overlapping window",
            group: OTHER_GROUP,
            from_offset: HOUR_MS / 2,
            to_offset: HOUR_MS,
            optional: json!({}),
        },
        Row {
            case: "another bucket, prefix, task name and stream prefix",
            group: OTHER_GROUP,
            from_offset: 0,
            to_offset: HOUR_MS,
            optional: json!({
                "destination": "another-export-bucket",
                "destinationPrefix": "exports/another",
                "taskName": "another",
                "logStreamNamePrefix": "app-",
            }),
        },
    ];

    // (the status the holding task is in when the second create arrives, whether the instant it
    // begins exporting is pinned in the past)
    let holding_statuses: [(&str, bool); 2] = [(PENDING, false), (RUNNING, true)];

    // Every create the account accepts leaves a task in the store, so what the report must
    // answer after a refusal is the tasks the accepted creates started and nothing more.
    let mut accepted = 0usize;
    for (holding_status, pin_running) in holding_statuses {
        for row in &rows {
            let from = now_ms() - HOUR_MS;
            let holder = task_id(
                &logs
                    .ok(CREATE, &create_body(GROUP, from, from + HOUR_MS, &json!({})))
                    .await,
            );
            accepted += 1;
            pin_under_way(&logs, &holder, pin_running);
            let case = format!("{} while the holding task is {holding_status}", row.case);
            let holding = reported(&logs, &case, &holder).await;
            assert_eq!(
                holding["status"]["code"].as_str(),
                Some(holding_status),
                "case {case:?}: the holding task must be {holding_status}, got {holding}"
            );

            let body = create_body(
                row.group,
                from + row.from_offset,
                from + row.to_offset,
                &row.optional,
            );
            let (code, message) = logs.refusal(CREATE, &body).await;
            assert_eq!(
                code, LIMIT_EXCEEDED,
                "case {case:?}: expected {LIMIT_EXCEEDED}, got {code} {message:?}"
            );
            assert_eq!(
                message, LIMIT_EXCEEDED_MESSAGE,
                "case {case:?}: expected the message {LIMIT_EXCEEDED_MESSAGE:?}, got {message:?}"
            );

            // The refused create started nothing, and what the task holding the slot was
            // created for is untouched by the refusal.
            let tasks = described(&logs, &json!({})).await;
            assert_eq!(
                tasks.len(),
                accepted,
                "case {case:?}: expected the {accepted} started task(s) and no more, got {tasks:?}"
            );
            let after = reported(&logs, &case, &holder).await;
            assert_eq!(
                export_identity(&after),
                export_identity(&holding),
                "case {case:?}: the refused create leaves the holding task's export as it was"
            );

            // Free the account's slot for the next row.
            logs.ok(CANCEL, &json!({"taskId": holder})).await;
        }
    }
}

/// The slot is the only thing the refusal is about: a request refused while a task is active is
/// ACCEPTED once that task has settled, whichever way it settled.
#[tokio::test]
async fn the_same_request_is_accepted_once_the_slot_is_free() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let from = now_ms() - HOUR_MS;
    // The window the second create names is the holding task's own, which is the request most
    // likely to be mistaken for a conflict of its own.
    let second = create_body(
        GROUP,
        from,
        from + HOUR_MS,
        &json!({"destinationPrefix": "exports/again"}),
    );

    // (case, how the task holding the slot settles)
    let cases: Vec<(&str, Settle)> = vec![
        ("the holding task finished on the clock", Settle::OnTheClock),
        ("the holding task was cancelled", Settle::Cancelled),
    ];
    for (case, settle) in cases {
        let holder = task_id(
            &logs
                .ok(CREATE, &create_body(GROUP, from, from + HOUR_MS, &json!({})))
                .await,
        );
        let code = logs.err(CREATE, &second).await;
        assert_eq!(
            code, LIMIT_EXCEEDED,
            "case {case:?}: expected {LIMIT_EXCEEDED} while the slot is held, got {code}"
        );

        match settle {
            Settle::OnTheClock => tokio::time::sleep(past_completion()).await,
            Settle::Cancelled => {
                logs.ok(CANCEL, &json!({"taskId": holder})).await;
            }
        }

        let started = task_id(&logs.ok(CREATE, &second).await);
        assert_ne!(
            started, holder,
            "case {case:?}: the accepted create mints an id of its own"
        );
        let task = reported(&logs, case, &started).await;
        assert_eq!(
            (task["from"].as_i64(), task["to"].as_i64()),
            (Some(from), Some(from + HOUR_MS)),
            "case {case:?}: the accepted task exports the window the request named, got {task}"
        );
        assert_eq!(
            task["destinationPrefix"].as_str(),
            Some("exports/again"),
            "case {case:?}: the accepted task carries the prefix the request named, got {task}"
        );
        assert_eq!(
            task["status"]["code"].as_str(),
            Some(PENDING),
            "case {case:?}: a create that was just accepted is {PENDING}, got {task}"
        );

        // Leave the account's slot free for the next row.
        logs.ok(CANCEL, &json!({"taskId": started})).await;
    }
}

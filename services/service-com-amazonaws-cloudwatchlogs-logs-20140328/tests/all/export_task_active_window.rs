// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the window an export task is ACTIVE for after `CreateExportTask`
//! accepted it.
//!
//! The rule under test: `CreateExportTask` "is an asynchronous call" that "initiates an export
//! task and responds with the ID of the task", so what it hands back is a task that has NOT
//! finished — it is minted `PENDING` and stays in one of the two statuses the operation calls
//! active ("Each account can only have one active (RUNNING or PENDING) export task at a time")
//! across the further calls a client makes after the create, reaching a terminal status only
//! once both the spans the create armed have elapsed. Everything a client can do with an
//! active task therefore has to be reachable with nothing but further API calls: reading the
//! task back in a non-terminal status, being refused a second create while the slot is held,
//! and cancelling the export that is under way.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — and NOTHING
//! here reaches into the store to pin an instant: the span under test is the one the handler
//! itself arms, so a task that settled inside a client's round-trip cannot pass.
//!
//! The tables span what the rule generalises over rather than one request: how many further
//! client calls separate the create from the call that must still find the task active (none,
//! one, and up to a few dozen), which members the create named, the shape of the window it
//! exports, which log group it reads, and which filter the task is read back through.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    EXPORT_TASK_COMPLETION_DELAY_MS, EXPORT_TASK_RUNNING_DELAY_MS,
};

use crate::harness::{mid_export, now_ms, past_completion, Logs};

/// The operations under test.
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";
const CANCEL: &str = "CancelExportTask";

/// An operation a client calls BETWEEN the create and the call that must still find the task
/// active, standing in for whatever work a real caller does in between.
const BETWEEN: &str = "DescribeLogGroups";

/// The statuses of the model's `ExportTaskStatusCode` enum this suite names.
const PENDING: &str = "PENDING";
const RUNNING: &str = "RUNNING";
const COMPLETED: &str = "COMPLETED";
const CANCELLED: &str = "CANCELLED";

/// The modelled error code a create past the account's one active task is refused under.
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The whole message that refusal carries: fixed text for the error class, naming neither the
/// task that holds the slot nor anything of the request.
const LIMIT_EXCEEDED_MESSAGE: &str = "Resource limit exceeded.";

/// The whole status message a cancelled export task reports.
const CANCELLED_BY_USER: &str = "Export task cancelled by user";

/// The log groups the rows export from, and the buckets they export to.
const GROUP: &str = "/export/active-window";
const OTHER_GROUP: &str = "/export/active-window-other";
const BUCKET: &str = "export-bucket";
const OTHER_BUCKET: &str = "other-export-bucket";

/// One hour in the milliseconds `CreateExportTask$from` / `$to` carry.
const HOUR_MS: i64 = 60 * 60 * 1_000;

/// The tasks `DescribeExportTasks` reports for `filter`.
async fn described(logs: &Logs, filter: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, filter).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"))
        .clone()
}

/// The task carrying `id`, read back through `filter`.
async fn reported_through(logs: &Logs, case: &str, filter: &Value, id: &str) -> Value {
    described(logs, filter)
        .await
        .into_iter()
        .find(|task| task["taskId"].as_str() == Some(id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the task {id} must be reported for the filter {filter}")
        })
}

/// The one task the service reports under `id`.
async fn reported(logs: &Logs, case: &str, id: &str) -> Value {
    reported_through(logs, case, &json!({"taskId": id}), id).await
}

/// A create of `group` over `[from, to]` into `bucket`, carrying whatever optional members
/// `optional` names.
fn create_body(group: &str, bucket: &str, from: i64, to: i64, optional: &Value) -> Value {
    let mut body = json!({
        "logGroupName": group,
        "from": from,
        "to": to,
        "destination": bucket,
    });
    if let Some(members) = optional.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Start one export, answering the `taskId` the create minted.
async fn start_export(logs: &Logs, body: &Value) -> String {
    let answer = logs.ok(CREATE, body).await;
    answer["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExportTask answers a taskId, got {answer}"))
        .to_string()
}

/// The `count` further calls a client makes between the create and the call that must still
/// find the task active.
async fn calls_in_between(logs: &Logs, count: usize) {
    for _ in 0..count {
        logs.ok(BETWEEN, &json!({})).await;
    }
}

/// The instant the task reports being created at.
fn created_at(case: &str, task: &Value) -> i64 {
    task["executionInfo"]["creationTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: a reported task carries a creationTime, got {task}"))
}

/// The status code the task reports.
fn status_code(case: &str, task: &Value) -> String {
    task["status"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a reported task carries a status code, got {task}"))
        .to_string()
}

/// Assert that `task`, read at `read_at_ms`, is still the ACTIVE task the create handed back:
/// the read landed inside the window the create armed, and the status it found is the
/// non-terminal one that window owes — `PENDING` until the instant the export begins moving
/// data, and `PENDING` or `RUNNING` after it.
fn assert_still_active(case: &str, task: &Value, read_at_ms: i64) {
    let created = created_at(case, task);
    let elapsed = read_at_ms - created;
    let active_for = EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_TASK_COMPLETION_DELAY_MS;
    assert!(
        elapsed < active_for,
        "case {case:?}: the window a created export task is active for must outlast a client's \
         following calls, but {elapsed} ms of the {active_for} ms window had already passed"
    );
    let code = status_code(case, task);
    let admitted: &[&str] = if elapsed < EXPORT_TASK_RUNNING_DELAY_MS {
        &[PENDING]
    } else {
        &[PENDING, RUNNING]
    };
    assert!(
        admitted.contains(&code.as_str()),
        "case {case:?}: {elapsed} ms after the create the task must be one of {admitted:?}, got \
         {task}"
    );
    assert!(
        task["executionInfo"]["completionTime"].is_null(),
        "case {case:?}: a task whose export has not finished reports no completion time, got \
         {task}"
    );
    assert!(
        task["status"]["message"].is_null(),
        "case {case:?}: a task that has reached no outcome carries no status message, got {task}"
    );
}

/// One row: a create, and how many further client calls follow it before the task is read.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The log group the export reads.
    group: &'static str,
    /// The bucket it exports to.
    bucket: &'static str,
    /// The window it exports, as offsets in milliseconds from the moment the row runs.
    from_offset: i64,
    to_offset: i64,
    /// The optional members the create names.
    optional: Value,
    /// How many further calls the client makes before reading the task back.
    in_between: usize,
}

/// The rows every table in this suite walks.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no optional member, read straight after the create",
            group: GROUP,
            bucket: BUCKET,
            from_offset: -HOUR_MS,
            to_offset: 0,
            optional: json!({}),
            in_between: 0,
        },
        Row {
            case: "a task name and a prefix, one call in between",
            group: GROUP,
            bucket: BUCKET,
            from_offset: -HOUR_MS,
            to_offset: 0,
            optional: json!({"taskName": "nightly", "destinationPrefix": "exports/nightly"}),
            in_between: 1,
        },
        Row {
            case: "a stream prefix, four calls in between",
            group: GROUP,
            bucket: BUCKET,
            from_offset: -2 * HOUR_MS,
            to_offset: -HOUR_MS,
            optional: json!({"logStreamNamePrefix": "app-"}),
            in_between: 4,
        },
        Row {
            case: "every optional member, thirty-two calls in between",
            group: OTHER_GROUP,
            bucket: OTHER_BUCKET,
            from_offset: -HOUR_MS,
            to_offset: HOUR_MS,
            optional: json!({
                "taskName": "everything",
                "destinationPrefix": "exports/everything",
                "logStreamNamePrefix": "app-",
            }),
            in_between: 32,
        },
        Row {
            case: "a window of a single instant, two calls in between",
            group: OTHER_GROUP,
            bucket: BUCKET,
            from_offset: -HOUR_MS,
            to_offset: -HOUR_MS,
            optional: json!({}),
            in_between: 2,
        },
        Row {
            case: "a window reaching into the future, eight calls in between",
            group: GROUP,
            bucket: OTHER_BUCKET,
            from_offset: 0,
            to_offset: HOUR_MS,
            optional: json!({"destinationPrefix": "exports/ahead"}),
            in_between: 8,
        },
    ]
}

/// The log groups every row exports from.
async fn groups(logs: &Logs) {
    for group in [GROUP, OTHER_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
}

/// An accepted create hands back a task that has NOT finished: whatever a client does next, the
/// task it reads back is still in a non-terminal status — through every filter the report takes.
#[tokio::test]
async fn a_created_task_is_still_active_after_a_clients_following_calls() {
    let logs = Logs::new();
    groups(&logs).await;

    for row in rows() {
        let now = now_ms();
        let body = create_body(
            row.group,
            row.bucket,
            now + row.from_offset,
            now + row.to_offset,
            &row.optional,
        );
        let id = start_export(&logs, &body).await;
        calls_in_between(&logs, row.in_between).await;

        // Every filter the report takes answers the SAME task, and each read is itself one more
        // call the active window has to outlast. The two non-terminal statuses between them
        // cover the whole of that window, so a status filter reports the task while it is in
        // that status and the task is reported through at least one of the two.
        let mut found_by_status = 0usize;
        for (filter_case, filter) in [
            ("no filter", json!({})),
            ("the task's id", json!({"taskId": id})),
            ("the status it is in", json!({"statusCode": PENDING})),
            ("the status it moves to", json!({"statusCode": RUNNING})),
        ] {
            let case = format!("{} read through {filter_case}", row.case);
            let by_status = filter_case.starts_with("the status");
            let reported = described(&logs, &filter)
                .await
                .into_iter()
                .find(|task| task["taskId"].as_str() == Some(id.as_str()));
            let read_at = now_ms();
            match reported {
                Some(task) => {
                    assert_still_active(&case, &task, read_at);
                    if by_status {
                        found_by_status += 1;
                    }
                }
                None => assert!(by_status, "case {case:?}: the task {id} must be reported"),
            }
        }
        assert!(
            found_by_status >= 1,
            "case {:?}: the task must be reported under one of the two non-terminal status \
             filters, got {found_by_status}",
            row.case
        );

        // The slot is the account's, so this row settles its own task before the next row runs.
        logs.ok(CANCEL, &json!({"taskId": id})).await;
    }
}

/// While the task the create handed back is active, a second create in the same account is
/// refused — reachable with nothing but further API calls, because the slot is still held by
/// the time a client can issue one.
#[tokio::test]
async fn a_second_create_is_refused_while_the_first_task_is_still_active() {
    let logs = Logs::new();
    groups(&logs).await;

    let mut started = 0usize;
    for row in rows() {
        let now = now_ms();
        let holder = start_export(
            &logs,
            &create_body(GROUP, BUCKET, now - HOUR_MS, now, &json!({})),
        )
        .await;
        started += 1;
        calls_in_between(&logs, row.in_between).await;

        // The holding task is still active when the second create is issued — which is what
        // makes the refusal below the account's quota and not an accident of the clock.
        let holding = reported(&logs, row.case, &holder).await;
        assert_still_active(row.case, &holding, now_ms());

        let second = create_body(
            row.group,
            row.bucket,
            now + row.from_offset,
            now + row.to_offset,
            &row.optional,
        );
        let (code, message) = logs.refusal(CREATE, &second).await;
        assert_eq!(
            code, LIMIT_EXCEEDED,
            "case {:?}: expected {LIMIT_EXCEEDED}, got {code} {message:?}",
            row.case
        );
        assert_eq!(
            message, LIMIT_EXCEEDED_MESSAGE,
            "case {:?}: expected the message {LIMIT_EXCEEDED_MESSAGE:?}, got {message:?}",
            row.case
        );
        assert_eq!(
            described(&logs, &json!({})).await.len(),
            started,
            "case {:?}: the refused create started nothing, so the account holds the {started} \
             task(s) its accepted creates started",
            row.case
        );

        logs.ok(CANCEL, &json!({"taskId": holder})).await;
    }
}

/// The export a create started is still under way when a cancel can reach it: "The task must be
/// in the PENDING or RUNNING state", and it is — so a cancel issued after the create's own
/// following calls stops a task that is running rather than being refused a settled one.
#[tokio::test]
async fn a_cancel_after_the_create_stops_a_task_that_is_still_under_way() {
    let logs = Logs::new();
    groups(&logs).await;

    for row in rows() {
        let now = now_ms();
        let id = start_export(
            &logs,
            &create_body(
                row.group,
                row.bucket,
                now + row.from_offset,
                now + row.to_offset,
                &row.optional,
            ),
        )
        .await;
        calls_in_between(&logs, row.in_between).await;

        let before = reported(&logs, row.case, &id).await;
        assert_still_active(row.case, &before, now_ms());

        logs.ok(CANCEL, &json!({"taskId": id})).await;
        let stopped = reported(&logs, row.case, &id).await;
        assert_eq!(
            status_code(row.case, &stopped),
            CANCELLED,
            "case {:?}: a cancel of a task that is under way stops it, got {stopped}",
            row.case
        );
        assert_eq!(
            stopped["status"]["message"].as_str(),
            Some(CANCELLED_BY_USER),
            "case {:?}: a cancelled task reports the message that status carries, got {stopped}",
            row.case
        );
    }
}

/// The spans the create armed are what settles the task, and nothing earlier: it is `PENDING`
/// until the instant it begins moving data, `RUNNING` from then until the instant it completes,
/// and `COMPLETED` at exactly that instant — and it holds the account's slot for the whole of
/// both spans.
#[tokio::test]
async fn a_task_settles_only_once_both_the_spans_the_create_armed_have_passed() {
    let logs = Logs::new();
    groups(&logs).await;
    let now = now_ms();
    let create = create_body(GROUP, BUCKET, now - HOUR_MS, now, &json!({}));

    let id = start_export(&logs, &create).await;
    let pending = reported(&logs, "straight after the create", &id).await;
    assert_eq!(
        status_code("straight after the create", &pending),
        PENDING,
        "case straight after the create: a task that has not started exporting is {PENDING}, got \
         {pending}"
    );
    let created = created_at("straight after the create", &pending);

    // Past the first span and inside the second: the export is moving data, and the slot is
    // still the holder's.
    tokio::time::sleep(mid_export()).await;
    let running = reported(&logs, "past the first span", &id).await;
    assert_eq!(
        status_code("past the first span", &running),
        RUNNING,
        "case past the first span: an export that has started moving data is {RUNNING}, got \
         {running}"
    );
    assert!(
        running["executionInfo"]["completionTime"].is_null(),
        "case past the first span: a task that has not finished reports no completion time, got \
         {running}"
    );
    let code = logs.err(CREATE, &create).await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case past the first span: a {RUNNING} task still holds the account's slot, got {code}"
    );

    // Past both spans: the task has finished, at the instant the create armed and not at the
    // instant something happened to look.
    tokio::time::sleep(past_completion()).await;
    let completed = reported(&logs, "past both spans", &id).await;
    assert_eq!(
        status_code("past both spans", &completed),
        COMPLETED,
        "case past both spans: the task has finished, got {completed}"
    );
    assert_eq!(
        completed["executionInfo"]["completionTime"].as_i64(),
        Some(created + EXPORT_TASK_RUNNING_DELAY_MS + EXPORT_TASK_COMPLETION_DELAY_MS),
        "case past both spans: the task finished at the instant both spans were due, got \
         {completed}"
    );

    // The slot the settled task held is free.
    let next = start_export(&logs, &create).await;
    assert_ne!(
        next, id,
        "case once the first task settled: the accepted create mints an id of its own"
    );
    let reread = reported(&logs, "the first task once another started", &id).await;
    assert_eq!(
        reread["status"], completed["status"],
        "case the first task once another started: a settled task's status does not move, got \
         {reread}"
    );
}

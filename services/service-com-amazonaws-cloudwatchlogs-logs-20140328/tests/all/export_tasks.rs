// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateExportTask` and `DescribeExportTasks`: starting an export of a
//! log group's events to an S3 bucket, and reading the started task back.
//!
//! The rule under test: `CreateExportTask` exports from a log group this account HOLDS — a
//! request naming one it does not hold is refused `ResourceNotFoundException` and starts
//! nothing — over a window that is a range and that does not end before the log group was
//! created (`InvalidParameterException` otherwise), while the account has no other active
//! task (`LimitExceededException` otherwise); it mints the task's id and stores the task,
//! which `DescribeExportTasks` then reports, `PENDING` at first and moving on the clock to
//! `RUNNING` and `COMPLETED` without anybody polling it.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: which end of the
//! window is out of order, whether the window precedes the log group, whether the request
//! names a task name, a destination prefix or a log-stream prefix, and which filter
//! (`taskId`, `statusCode`, neither) the report is read through. The recorded request an
//! issue happens to mention is one row.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The operations under test.
const CREATE: &str = "CreateExportTask";
const DESCRIBE: &str = "DescribeExportTasks";

/// The modelled error codes the rule names.
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_PARAMETER: &str = "InvalidParameterException";
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// The statuses an export task passes through, as the model's `ExportTaskStatusCode` enum
/// spells them.
const PENDING: &str = "PENDING";
const COMPLETED: &str = "COMPLETED";

/// The prefix a task gets when the request names none: "If you don't specify a value, the
/// default is exportedlogs" (`CreateExportTask$destinationPrefix`).
const DEFAULT_PREFIX: &str = "exportedlogs";

/// How long a case waits before treating a started task as finished: past both spans the
/// create armed, with room for the test's own scheduling.
fn past_completion() -> Duration {
    Duration::from_millis(900)
}

/// The tasks `DescribeExportTasks` reports for `body`.
async fn described(logs: &Logs, body: &Value) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, body).await;
    page["exportTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeExportTasks reports an exportTasks list, got {page}"))
        .clone()
}

/// The one reported task carrying `id`, or `None` when none does.
async fn described_task(logs: &Logs, body: &Value, id: &str) -> Option<Value> {
    described(logs, body)
        .await
        .into_iter()
        .find(|task| task["taskId"].as_str() == Some(id))
}

/// The `taskId` a successful create answered.
fn task_id(answer: &Value) -> String {
    answer["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateExportTask answers a taskId, got {answer}"))
        .to_string()
}

/// An accepted create stores the task the report then answers from, carrying every member
/// the request named and the documented default for every one it did not.
#[tokio::test]
async fn a_created_export_task_is_reported_with_what_the_request_named() {
    let logs = Logs::new();
    const GROUP: &str = "/export/accepted";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let now = now_ms();

    // (case, the optional members the request carries, the prefix the task must report)
    let cases: Vec<(&str, Value, &str)> = vec![
        ("no optional member", json!({}), DEFAULT_PREFIX),
        (
            "a task name and a destination prefix",
            json!({"taskName": "nightly", "destinationPrefix": "exports/nightly"}),
            "exports/nightly",
        ),
        (
            "a log stream prefix as well",
            json!({
                "taskName": "one-stream",
                "destinationPrefix": "exports/one-stream",
                "logStreamNamePrefix": "app-",
            }),
            "exports/one-stream",
        ),
    ];
    for (case, optional, expected_prefix) in cases {
        let mut body = json!({
            "logGroupName": GROUP,
            "from": now - 60 * 60 * 1_000,
            "to": now,
            "destination": "export-bucket",
        });
        if let Some(members) = optional.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let id = task_id(&logs.ok(CREATE, &body).await);
        let reported = described_task(&logs, &json!({}), &id)
            .await
            .unwrap_or_else(|| panic!("case {case}: the created task must be reported"));
        assert_eq!(
            reported["logGroupName"].as_str(),
            Some(GROUP),
            "case {case}: the task reports the log group it exports, got {reported}"
        );
        assert_eq!(
            reported["destination"].as_str(),
            Some("export-bucket"),
            "case {case}: the task reports the bucket it exports to, got {reported}"
        );
        assert_eq!(
            reported["destinationPrefix"].as_str(),
            Some(expected_prefix),
            "case {case}: expected the prefix {expected_prefix:?}, got {reported}"
        );
        assert_eq!(
            reported["taskName"].as_str(),
            optional["taskName"].as_str(),
            "case {case}: the task reports the name the request named, got {reported}"
        );
        assert_eq!(
            (reported["from"].as_i64(), reported["to"].as_i64()),
            (body["from"].as_i64(), body["to"].as_i64()),
            "case {case}: the task reports the window the request named, got {reported}"
        );
        assert!(
            reported["executionInfo"]["creationTime"]
                .as_i64()
                .is_some_and(|created| created >= now),
            "case {case}: the task reports when it was created, got {reported}"
        );
        // One active task at a time, so this one is waited out before the next row runs.
        tokio::time::sleep(past_completion()).await;
    }

    assert_eq!(
        described(&logs, &json!({})).await.len(),
        3,
        "the account holds exactly the tasks that were created"
    );
}

/// A create the rule refuses, in each form the refusal takes — and nothing is started.
#[tokio::test]
async fn a_create_the_rule_refuses_starts_nothing() {
    let logs = Logs::new();
    const GROUP: &str = "/export/refusals";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let group = logs
        .describe_group(GROUP)
        .await
        .expect("the log group must exist");
    let created_at = group["creationTime"]
        .as_i64()
        .unwrap_or_else(|| panic!("a log group reports its creationTime, got {group}"));
    let now = now_ms();

    // (case, the request, the modelled code it is refused under)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a log group name this account does not hold",
            json!({
                "logGroupName": "/export/no-such-group",
                "from": now - 1_000,
                "to": now,
                "destination": "export-bucket",
            }),
            NOT_FOUND,
        ),
        (
            "a window whose end is one millisecond before its start",
            json!({
                "logGroupName": GROUP,
                "from": now,
                "to": now - 1,
                "destination": "export-bucket",
            }),
            INVALID_PARAMETER,
        ),
        (
            "a window whose ends are swapped",
            json!({
                "logGroupName": GROUP,
                "from": now,
                "to": now - 60 * 60 * 1_000,
                "destination": "export-bucket",
            }),
            INVALID_PARAMETER,
        ),
        (
            "a window ending one millisecond before the log group was created",
            json!({
                "logGroupName": GROUP,
                "from": 0,
                "to": created_at - 1,
                "destination": "export-bucket",
            }),
            INVALID_PARAMETER,
        ),
    ];
    for (case, body, expected) in cases {
        let code = logs.err(CREATE, &body).await;
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code}"
        );
        assert!(
            described(&logs, &json!({})).await.is_empty(),
            "case {case}: a refused create starts no task"
        );
    }

    // The window ending exactly when the log group was created is the published boundary,
    // which is accepted: "You must specify a time that is not earlier than when this log
    // group was created."
    let id = task_id(
        &logs
            .ok(
                CREATE,
                &json!({
                    "logGroupName": GROUP,
                    "from": 0,
                    "to": created_at,
                    "destination": "export-bucket",
                }),
            )
            .await,
    );
    assert!(
        described_task(&logs, &json!({}), &id).await.is_some(),
        "case a window ending when the log group was created: the task must be started"
    );
}

/// "Each account can only have one active (RUNNING or PENDING) export task at a time", and
/// the statuses a task passes through are the ones the clock owes it.
#[tokio::test]
async fn one_active_task_at_a_time_and_the_statuses_move_on_the_clock() {
    let logs = Logs::new();
    const GROUP: &str = "/export/lifecycle";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    let now = now_ms();
    let create = json!({
        "logGroupName": GROUP,
        "from": now - 60 * 60 * 1_000,
        "to": now,
        "destination": "export-bucket",
    });

    let id = task_id(&logs.ok(CREATE, &create).await);
    let pending = described_task(&logs, &json!({}), &id)
        .await
        .expect("the created task must be reported");
    assert_eq!(
        pending["status"]["code"].as_str(),
        Some(PENDING),
        "case straight after the create: a task that has not started is {PENDING}, got {pending}"
    );
    assert!(
        pending["executionInfo"]["completionTime"].is_null(),
        "case straight after the create: a task that has not finished reports no completion \
         time, got {pending}"
    );

    // A second create while the first is active is refused, and adds nothing.
    let code = logs.err(CREATE, &create).await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case a second create while one is active: expected {LIMIT_EXCEEDED}, got {code}"
    );
    assert_eq!(
        described(&logs, &json!({})).await.len(),
        1,
        "case a second create while one is active: the refused create started nothing"
    );

    tokio::time::sleep(past_completion()).await;
    let completed = described_task(&logs, &json!({}), &id)
        .await
        .expect("the task must still be reported");
    assert_eq!(
        completed["status"]["code"].as_str(),
        Some(COMPLETED),
        "case past the spans the create armed: the task is {COMPLETED}, got {completed}"
    );
    assert!(
        completed["executionInfo"]["completionTime"]
            .as_i64()
            .is_some_and(|finished| finished >= now),
        "case past the spans the create armed: a finished task reports when it finished, got \
         {completed}"
    );

    // Once the clock has finished the first task, another create is accepted.
    let second = task_id(&logs.ok(CREATE, &create).await);
    assert_ne!(
        second, id,
        "case a create once the first task finished: every create mints a new id"
    );

    // (case, the filter, how many tasks it reports)
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter", json!({}), 2),
        ("the id of the first task", json!({"taskId": id}), 1),
        (
            "an id no create minted",
            json!({"taskId": "00000000-0000-4000-8000-ffffffffffff"}),
            0,
        ),
        ("the status the first task reached", json!({"statusCode": COMPLETED}), 1),
        ("the status the second task is in", json!({"statusCode": PENDING}), 1),
        ("a status no task is in", json!({"statusCode": "CANCELLED"}), 0),
    ];
    for (case, filter, expected) in cases {
        let reported = described(&logs, &filter).await;
        assert_eq!(
            reported.len(),
            expected,
            "case {case}: expected {expected} tasks, got {reported:?}"
        );
    }

    // Reporting is a read: calling it twice reports the same thing.
    let first_read = described(&logs, &json!({"taskId": id})).await;
    let second_read = described(&logs, &json!({"taskId": id})).await;
    assert_eq!(
        first_read, second_read,
        "case twice: reporting the tasks changes nothing a later caller sees"
    );
}

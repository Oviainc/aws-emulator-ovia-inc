// SPDX-License-Identifier: CC0-1.0

//! The Migration Hub migration-task lifecycle, driven end to end over the
//! `awsJson1_1` wire (`POST /`, `X-Amz-Target: AWSMigrationHub.<Operation>`).
//!
//! Every assertion here is about PERSISTED state: what a create/notify/put wrote
//! is what a later `DescribeMigrationTask` / `ListMigrationTasks` must report,
//! and what a delete removed must be gone. The recorded script in the issue is
//! ONE row of each table below.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhub_awsmigrationhub::state::{
    MAX_PROGRESS_UPDATE_STREAMS, MAX_TASKS_PER_STREAM, MAX_UPDATES_PER_TASK,
};
use service_com_amazonaws_migrationhub_awsmigrationhub::{
    ComAmazonawsMigrationhubAwsmigrationhubState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// Every value the `Status` enum admits, in the model's own order.
///
/// Source: `smithy_query(query="shape", shape="Status")` on
/// `smithy-model/com-amazonaws-migrationhub-awsmigrationhub.json`.
const STATUS_CATALOG: &[&str] = &["NOT_STARTED", "IN_PROGRESS", "FAILED", "COMPLETED"];

/// Every value the `ResourceAttributeType` enum admits, in the model's own order.
///
/// Source: `smithy_query(query="shape", shape="ResourceAttributeType")`.
const RESOURCE_ATTRIBUTE_TYPE_CATALOG: &[&str] = &[
    "IPV4_ADDRESS",
    "IPV6_ADDRESS",
    "MAC_ADDRESS",
    "FQDN",
    "VM_MANAGER_ID",
    "VM_MANAGED_OBJECT_REFERENCE",
    "VM_NAME",
    "VM_PATH",
    "BIOS_ID",
    "MOTHERBOARD_SERIAL_NUMBER",
];

/// The published bounds of `Task.ProgressPercent` (`range min 0, max 100`).
///
/// Source: `smithy_query(query="shape", shape="MigrationTaskSummary")` member
/// `ProgressPercent`, which targets the same `ProgressPercent` shape `Task` does.
const PROGRESS_PERCENT_BOUNDS: (i64, i64) = (0, 100);

/// One emulator: the shared engine plus a router carrying only this service.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn start() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests drive the wire, not the signer: no authenticator, so the
        // credential scope below is read for ROUTING only.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine.clone()));
        Self { engine, router }
    }

    /// Send one awsJson1_1 request and return `(status, parsed body)`.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("http://localhost/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSMigrationHub.{operation}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20261021/us-west-2/mgh/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `(status, body)` of a call that must succeed, with the body of a failure
    /// in the panic message.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} must succeed, got HTTP {status} body {parsed}"
        );
        parsed
    }

    /// The update history this service persisted for one task, newest LAST, as
    /// `(UpdateDateTime seconds, UpdateType, MigrationTaskState.Status)`.
    ///
    /// `ListMigrationTaskUpdates` is still scaffolded, so the append-only history
    /// is read from the store the handlers write.
    fn stored_history(&self, stream: &str, task: &str) -> Vec<(f64, String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsMigrationhubAwsmigrationhubState>();
        let guard = state.lock();
        let record = guard
            .task(stream, task)
            .unwrap_or_else(|| panic!("task {stream}/{task} must be stored"));
        record
            .updates
            .iter()
            .map(|update| {
                (
                    update
                        .update_date_time
                        .map(|when| when.as_secs_f64())
                        .unwrap_or_default(),
                    update.update_type.clone().unwrap_or_default(),
                    update
                        .migration_task_state
                        .as_ref()
                        .map(|state| state.status.clone())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// One row of the lifecycle table: a task, the state notified against it, and
/// the resource attributes put on it.
#[derive(Debug)]
struct LifecycleCase {
    stream: String,
    task: String,
    status: &'static str,
    progress_percent: Option<i64>,
    status_detail: Option<String>,
    attributes: Vec<(&'static str, String)>,
    update_date_time: f64,
}

fn notify_body(case: &LifecycleCase, next_update_seconds: i64) -> Value {
    let mut task = serde_json::Map::new();
    task.insert("Status".to_string(), json!(case.status));
    if let Some(progress) = case.progress_percent {
        task.insert("ProgressPercent".to_string(), json!(progress));
    }
    if let Some(detail) = &case.status_detail {
        task.insert("StatusDetail".to_string(), json!(detail));
    }
    json!({
        "ProgressUpdateStream": case.stream,
        "MigrationTaskName": case.task,
        "Task": Value::Object(task),
        "UpdateDateTime": case.update_date_time,
        "NextUpdateSeconds": next_update_seconds,
    })
}

fn attribute_list(attributes: &[(&'static str, String)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(kind, value)| json!({"Type": kind, "Value": value}))
            .collect(),
    )
}

fn lifecycle_cases() -> Vec<LifecycleCase> {
    vec![
        // The row the issue recorded.
        LifecycleCase {
            stream: "MyTool".to_string(),
            task: "task-1".to_string(),
            status: "IN_PROGRESS",
            progress_percent: Some(42),
            status_detail: Some("copying".to_string()),
            attributes: vec![("IPV4_ADDRESS", "10.0.0.7".to_string())],
            update_date_time: 1_761_084_000.0,
        },
        // A second task in the SAME stream, a terminal status, an empty detail,
        // and more than one attribute.
        LifecycleCase {
            stream: "MyTool".to_string(),
            task: "task-2".to_string(),
            status: "COMPLETED",
            progress_percent: Some(100),
            status_detail: Some(String::new()),
            attributes: vec![
                ("FQDN", "host.example.internal".to_string()),
                ("MAC_ADDRESS", "00:11:22:33:44:55".to_string()),
            ],
            update_date_time: 1_761_084_123.0,
        },
        // Names using the punctuation the patterns allow (a space, a `/` in the
        // task name), no detail at all, and the lower progress bound.
        LifecycleCase {
            stream: "second tool v2".to_string(),
            task: "db/mysql prod".to_string(),
            status: "NOT_STARTED",
            progress_percent: Some(PROGRESS_PERCENT_BOUNDS.0),
            status_detail: None,
            attributes: vec![
                ("VM_MANAGER_ID", "vm-manager-1".to_string()),
                ("VM_PATH", "/dc-1/host-3/vm-9".to_string()),
            ],
            update_date_time: 1_600_000_000.0,
        },
        // The modelled maxima: a 256-character task name, a 2500-character
        // detail, no progress at all, and a sub-second timestamp.
        LifecycleCase {
            stream: "l".repeat(50),
            task: "t".repeat(256),
            status: "FAILED",
            progress_percent: None,
            status_detail: Some("d".repeat(2500)),
            attributes: vec![("MOTHERBOARD_SERIAL_NUMBER", "MB-0001".to_string())],
            update_date_time: 1_700_000_000.5,
        },
        // Every modelled attribute type on one task.
        LifecycleCase {
            stream: "Z-tool".to_string(),
            task: "everything".to_string(),
            status: "COMPLETED",
            progress_percent: Some(7),
            status_detail: Some("all attribute types".to_string()),
            attributes: RESOURCE_ATTRIBUTE_TYPE_CATALOG
                .iter()
                .enumerate()
                .map(|(index, kind)| (*kind, format!("value-{index}")))
                .collect(),
            update_date_time: 1_712_345_678.0,
        },
    ]
}

#[tokio::test]
async fn migration_task_lifecycle_round_trips_through_the_store() {
    let emulator = Emulator::start();

    for case in lifecycle_cases() {
        emulator
            .ok(
                "CreateProgressUpdateStream",
                json!({"ProgressUpdateStreamName": case.stream}),
            )
            .await;
        emulator
            .ok(
                "ImportMigrationTask",
                json!({
                    "ProgressUpdateStream": case.stream,
                    "MigrationTaskName": case.task,
                }),
            )
            .await;

        // A task that has only been imported has NO status and NO update time:
        // nothing has been notified, so there is nothing to report.
        let imported = emulator
            .ok(
                "DescribeMigrationTask",
                json!({
                    "ProgressUpdateStream": case.stream,
                    "MigrationTaskName": case.task,
                }),
            )
            .await;
        let imported = &imported["MigrationTask"];
        assert_eq!(
            imported["ProgressUpdateStream"].as_str(),
            Some(case.stream.as_str()),
            "case {case:?}: imported task must report its stream, got {imported}"
        );
        assert_eq!(
            imported["MigrationTaskName"].as_str(),
            Some(case.task.as_str()),
            "case {case:?}: imported task must report its name, got {imported}"
        );
        assert!(
            imported["Task"].is_null(),
            "case {case:?}: an imported task has no Task yet, got {imported}"
        );
        assert!(
            imported["UpdateDateTime"].is_null(),
            "case {case:?}: an imported task has no UpdateDateTime yet, got {imported}"
        );
        assert_eq!(
            imported["ResourceAttributeList"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an imported task has no resource attributes, got {imported}"
        );

        emulator
            .ok("NotifyMigrationTaskState", notify_body(&case, 60))
            .await;
        emulator
            .ok(
                "PutResourceAttributes",
                json!({
                    "ProgressUpdateStream": case.stream,
                    "MigrationTaskName": case.task,
                    "ResourceAttributeList": attribute_list(&case.attributes),
                }),
            )
            .await;

        let described = emulator
            .ok(
                "DescribeMigrationTask",
                json!({
                    "ProgressUpdateStream": case.stream,
                    "MigrationTaskName": case.task,
                }),
            )
            .await;
        let task = &described["MigrationTask"];
        assert_eq!(
            task["Task"]["Status"].as_str(),
            Some(case.status),
            "case {case:?}: Status must be what was notified, got {task}"
        );
        assert_eq!(
            task["Task"]["ProgressPercent"].as_i64(),
            case.progress_percent,
            "case {case:?}: ProgressPercent must be what was notified, got {task}"
        );
        assert_eq!(
            task["Task"]["StatusDetail"].as_str(),
            case.status_detail.as_deref(),
            "case {case:?}: StatusDetail must be what was notified, got {}",
            task["Task"]["StatusDetail"]
        );
        let update_date_time = task["UpdateDateTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: UpdateDateTime must be present, got {task}"));
        assert!(
            (update_date_time - case.update_date_time).abs() < 1e-6,
            "case {case:?}: expected UpdateDateTime {}, got {update_date_time}",
            case.update_date_time
        );
        assert_eq!(
            task["ResourceAttributeList"],
            attribute_list(&case.attributes),
            "case {case:?}: ResourceAttributeList must be what was put, got {}",
            task["ResourceAttributeList"]
        );

        // A second notification REPLACES the latest state and APPENDS to the
        // history: the task has one current status and remembers every report.
        let later = LifecycleCase {
            status: STATUS_CATALOG
                .iter()
                .find(|candidate| **candidate != case.status)
                .copied()
                .expect("the status catalog has more than one value"),
            update_date_time: case.update_date_time + 30.0,
            progress_percent: Some(PROGRESS_PERCENT_BOUNDS.1),
            status_detail: Some("second report".to_string()),
            stream: case.stream.clone(),
            task: case.task.clone(),
            attributes: Vec::new(),
        };
        emulator
            .ok("NotifyMigrationTaskState", notify_body(&later, 0))
            .await;

        let described = emulator
            .ok(
                "DescribeMigrationTask",
                json!({
                    "ProgressUpdateStream": case.stream,
                    "MigrationTaskName": case.task,
                }),
            )
            .await;
        let task = &described["MigrationTask"];
        assert_eq!(
            task["Task"]["Status"].as_str(),
            Some(later.status),
            "case {case:?}: the latest notification must win, got {task}"
        );
        assert_eq!(
            task["ResourceAttributeList"],
            attribute_list(&case.attributes),
            "case {case:?}: notifying state must not disturb the attribute list, got {}",
            task["ResourceAttributeList"]
        );

        let history = emulator.stored_history(&case.stream, &case.task);
        assert_eq!(
            history.len(),
            2,
            "case {case:?}: both notifications must be appended, got {history:?}"
        );
        assert_eq!(
            history
                .iter()
                .map(|(_, _, status)| status.as_str())
                .collect::<Vec<_>>(),
            vec![case.status, later.status],
            "case {case:?}: history is newest-last, got {history:?}"
        );
        for (index, (when, update_type, _)) in history.iter().enumerate() {
            assert_eq!(
                update_type, "MIGRATION_TASK_STATE_UPDATED",
                "case {case:?}: entry {index} must carry the modelled update type, got {history:?}"
            );
            let expected = case.update_date_time + if index == 0 { 0.0 } else { 30.0 };
            assert!(
                (when - expected).abs() < 1e-6,
                "case {case:?}: entry {index} expected UpdateDateTime {expected}, got {when}"
            );
        }
    }
}

#[tokio::test]
async fn progress_percent_is_stored_on_its_bounds_and_refused_past_them() {
    let emulator = Emulator::start();
    let (stream, task) = ("bounds-tool", "bounds-task");
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": stream}),
        )
        .await;
    emulator
        .ok(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
        )
        .await;

    let notify = |percent: i64| {
        json!({
            "ProgressUpdateStream": stream,
            "MigrationTaskName": task,
            "Task": {"Status": "IN_PROGRESS", "ProgressPercent": percent},
            "UpdateDateTime": 1_761_084_000.0,
            "NextUpdateSeconds": 0,
        })
    };

    // Both published bounds are accepted and stored verbatim.
    for percent in [PROGRESS_PERCENT_BOUNDS.0, 50, PROGRESS_PERCENT_BOUNDS.1] {
        emulator.ok("NotifyMigrationTaskState", notify(percent)).await;
        let described = emulator
            .ok(
                "DescribeMigrationTask",
                json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
            )
            .await;
        assert_eq!(
            described["MigrationTask"]["Task"]["ProgressPercent"].as_i64(),
            Some(percent),
            "case ProgressPercent={percent}: must round-trip, got {described}"
        );
    }

    // One past each bound is refused, and the refusal leaves the stored value
    // alone — the last accepted notification is still what a reader sees.
    for percent in [PROGRESS_PERCENT_BOUNDS.0 - 1, PROGRESS_PERCENT_BOUNDS.1 + 1] {
        let (status, body) = emulator.call("NotifyMigrationTaskState", notify(percent)).await;
        assert_eq!(
            status, 400,
            "case ProgressPercent={percent}: must be refused, got HTTP {status} body {body}"
        );
        let described = emulator
            .ok(
                "DescribeMigrationTask",
                json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
            )
            .await;
        assert_eq!(
            described["MigrationTask"]["Task"]["ProgressPercent"].as_i64(),
            Some(PROGRESS_PERCENT_BOUNDS.1),
            "case ProgressPercent={percent}: a refused notification must not be stored, got {described}"
        );
    }
}

#[tokio::test]
async fn put_resource_attributes_replaces_the_stored_list_for_every_modelled_type() {
    let emulator = Emulator::start();
    let (stream, task) = ("attr-tool", "attr-task");
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": stream}),
        )
        .await;
    emulator
        .ok(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
        )
        .await;

    // The model documents the write as replacing: "subsequent calls to
    // PutResourceAttributes will override previously stored attributes". Each
    // single-entry put must therefore leave exactly its own entry behind.
    for kind in RESOURCE_ATTRIBUTE_TYPE_CATALOG {
        let attributes = vec![(*kind, format!("{kind}-value"))];
        emulator
            .ok(
                "PutResourceAttributes",
                json!({
                    "ProgressUpdateStream": stream,
                    "MigrationTaskName": task,
                    "ResourceAttributeList": attribute_list(&attributes),
                }),
            )
            .await;
        let described = emulator
            .ok(
                "DescribeMigrationTask",
                json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
            )
            .await;
        assert_eq!(
            described["MigrationTask"]["ResourceAttributeList"],
            attribute_list(&attributes),
            "case Type={kind}: the latest put is the whole stored list, got {}",
            described["MigrationTask"]["ResourceAttributeList"]
        );
    }
}

/// Walk `ListMigrationTasks` with `MaxResults = page_size`, returning every
/// `(stream, task)` it reported, in order.
async fn walk_pages(emulator: &Emulator, page_size: Option<i64>) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..32 {
        let mut request = serde_json::Map::new();
        if let Some(size) = page_size {
            request.insert("MaxResults".to_string(), json!(size));
        }
        if let Some(cursor) = &token {
            request.insert("NextToken".to_string(), json!(cursor));
        }
        let body = emulator
            .ok("ListMigrationTasks", Value::Object(request))
            .await;
        let items = body["MigrationTaskSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("page {page}: MigrationTaskSummaryList must be a list, got {body}"))
            .clone();
        if let Some(size) = page_size {
            assert!(
                items.len() as i64 <= size,
                "case MaxResults={size}: page {page} returned {} items, got {body}",
                items.len()
            );
        }
        for item in &items {
            seen.push((
                item["ProgressUpdateStream"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                item["MigrationTaskName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return seen,
        }
    }
    panic!("case MaxResults={page_size:?}: paging did not terminate after 32 pages");
}

#[tokio::test]
async fn list_migration_tasks_pages_every_persisted_task_exactly_once() {
    let emulator = Emulator::start();

    // Two streams, three tasks each: enough that a page boundary can fall inside
    // a stream AND between two streams.
    let seeded: Vec<(&str, &str)> = vec![
        ("alpha-tool", "a-task"),
        ("alpha-tool", "b-task"),
        ("alpha-tool", "c-task"),
        ("beta-tool", "x-task"),
        ("beta-tool", "y-task"),
        ("beta-tool", "z-task"),
    ];
    for (stream, task) in &seeded {
        emulator
            .ok(
                "CreateProgressUpdateStream",
                json!({"ProgressUpdateStreamName": stream}),
            )
            .await;
        emulator
            .ok(
                "ImportMigrationTask",
                json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
            )
            .await;
    }
    let expected: Vec<(String, String)> = seeded
        .iter()
        .map(|(stream, task)| (stream.to_string(), task.to_string()))
        .collect();

    // Every page size from 1 to one past the item count, plus "no MaxResults",
    // must enumerate exactly the same set in the same order.
    for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(6), Some(7)] {
        let seen = walk_pages(&emulator, page_size).await;
        assert_eq!(
            seen, expected,
            "case MaxResults={page_size:?}: every task exactly once, in key order"
        );
    }

    // Summary fields are derived from the stored task, so a notified task
    // reports its state and an imported-only task reports none of it.
    emulator
        .ok(
            "NotifyMigrationTaskState",
            json!({
                "ProgressUpdateStream": "beta-tool",
                "MigrationTaskName": "y-task",
                "Task": {"Status": "FAILED", "ProgressPercent": 13, "StatusDetail": "disk full"},
                "UpdateDateTime": 1_761_084_321.0,
                "NextUpdateSeconds": 0,
            }),
        )
        .await;
    let body = emulator.ok("ListMigrationTasks", json!({})).await;
    let summaries = body["MigrationTaskSummaryList"]
        .as_array()
        .expect("a list")
        .clone();
    for summary in &summaries {
        let name = summary["MigrationTaskName"].as_str().unwrap_or_default();
        if name == "y-task" {
            assert_eq!(
                (
                    summary["Status"].as_str(),
                    summary["ProgressPercent"].as_i64(),
                    summary["StatusDetail"].as_str()
                ),
                (Some("FAILED"), Some(13), Some("disk full")),
                "case {name}: the summary must mirror the stored task, got {summary}"
            );
            assert!(
                summary["UpdateDateTime"].as_f64().is_some(),
                "case {name}: a notified task reports its update time, got {summary}"
            );
        } else {
            assert!(
                summary["Status"].is_null() && summary["UpdateDateTime"].is_null(),
                "case {name}: an imported-only task reports no status, got {summary}"
            );
        }
    }

    // A token this service never issued is a bad input, not an empty page.
    for bogus in ["Zm9vfGJhcg", "////", "not+a+cursor="] {
        let (status, body) = emulator
            .call("ListMigrationTasks", json!({"NextToken": bogus}))
            .await;
        assert_eq!(
            status, 400,
            "case NextToken={bogus:?}: must be refused, got HTTP {status} body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case NextToken={bogus:?}: wrong error code, got {body}"
        );
    }

    // Deleting a stream removes its tasks from every reader.
    emulator
        .ok(
            "DeleteProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "alpha-tool"}),
        )
        .await;
    let remaining = walk_pages(&emulator, Some(2)).await;
    assert_eq!(
        remaining,
        expected
            .iter()
            .filter(|(stream, _)| stream != "alpha-tool")
            .cloned()
            .collect::<Vec<_>>(),
        "after deleting alpha-tool only beta-tool's tasks remain"
    );
    for task in ["a-task", "b-task", "c-task"] {
        let (status, body) = emulator
            .call(
                "DescribeMigrationTask",
                json!({"ProgressUpdateStream": "alpha-tool", "MigrationTaskName": task}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {task}: a deleted stream's tasks are gone, got HTTP {status} body {body}"
        );
    }

    // Deleting every stream leaves an empty list with no token at all.
    emulator
        .ok(
            "DeleteProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "beta-tool"}),
        )
        .await;
    let body = emulator
        .ok("ListMigrationTasks", json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        body["MigrationTaskSummaryList"].as_array().map(Vec::len),
        Some(0),
        "no streams left, so no tasks, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "the last page carries no NextToken, got {body}"
    );
}

#[tokio::test]
async fn resource_name_filter_selects_by_discovered_resource_only() {
    let emulator = Emulator::start();
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "filter-tool"}),
        )
        .await;
    emulator
        .ok(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": "filter-tool", "MigrationTaskName": "filter-task"}),
        )
        .await;
    emulator
        .ok(
            "PutResourceAttributes",
            json!({
                "ProgressUpdateStream": "filter-tool",
                "MigrationTaskName": "filter-task",
                "ResourceAttributeList": [{"Type": "IPV4_ADDRESS", "Value": "10.0.0.7"}],
            }),
        )
        .await;

    // `ResourceName` filters by DISCOVERED RESOURCE name, and the only way to
    // associate one (`AssociateDiscoveredResource`) is still scaffolded, so no
    // task matches any filter value — in particular a resource attribute's value
    // is NOT a discovered resource name.
    for resource_name in ["10.0.0.7", "filter-task", "d-server-0001"] {
        let body = emulator
            .ok("ListMigrationTasks", json!({"ResourceName": resource_name}))
            .await;
        assert_eq!(
            body["MigrationTaskSummaryList"].as_array().map(Vec::len),
            Some(0),
            "case ResourceName={resource_name:?}: nothing is associated, got {body}"
        );
    }

    // Without the filter the task is listed, so the empty pages above are the
    // filter at work and not an empty store.
    let body = emulator.ok("ListMigrationTasks", json!({})).await;
    assert_eq!(
        body["MigrationTaskSummaryList"].as_array().map(Vec::len),
        Some(1),
        "unfiltered, the imported task is listed, got {body}"
    );
}

#[tokio::test]
async fn unknown_streams_and_tasks_are_reported_not_created() {
    let emulator = Emulator::start();
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "known-tool"}),
        )
        .await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": "ghost-tool", "MigrationTaskName": "t"}),
        ),
        (
            "NotifyMigrationTaskState",
            json!({
                "ProgressUpdateStream": "ghost-tool",
                "MigrationTaskName": "t",
                "Task": {"Status": "IN_PROGRESS"},
                "UpdateDateTime": 1_761_084_000.0,
                "NextUpdateSeconds": 0,
            }),
        ),
        (
            "NotifyMigrationTaskState",
            json!({
                "ProgressUpdateStream": "known-tool",
                "MigrationTaskName": "ghost-task",
                "Task": {"Status": "IN_PROGRESS"},
                "UpdateDateTime": 1_761_084_000.0,
                "NextUpdateSeconds": 0,
            }),
        ),
        (
            "PutResourceAttributes",
            json!({
                "ProgressUpdateStream": "known-tool",
                "MigrationTaskName": "ghost-task",
                "ResourceAttributeList": [{"Type": "FQDN", "Value": "host"}],
            }),
        ),
        (
            "DescribeMigrationTask",
            json!({"ProgressUpdateStream": "known-tool", "MigrationTaskName": "ghost-task"}),
        ),
        (
            "DescribeMigrationTask",
            json!({"ProgressUpdateStream": "ghost-tool", "MigrationTaskName": "t"}),
        ),
        (
            "DeleteProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "ghost-tool"}),
        ),
    ];

    for (operation, body) in cases {
        let (status, parsed) = emulator.call(operation, body.clone()).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "ResourceNotFoundException"),
            "case {operation} {body}: expected 400 ResourceNotFoundException, got HTTP {status} body {parsed}"
        );
    }

    // None of those refusals created anything.
    let listed = emulator.ok("ListMigrationTasks", json!({})).await;
    assert_eq!(
        listed["MigrationTaskSummaryList"].as_array().map(Vec::len),
        Some(0),
        "a refusal must not register a task, got {listed}"
    );
}

#[tokio::test]
async fn dry_run_reports_authorization_and_changes_nothing() {
    let emulator = Emulator::start();

    // CreateProgressUpdateStream: the dry run must not create the stream, which
    // the follow-up import proves by failing to find it.
    let (status, body) = emulator
        .call(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "dry-tool", "DryRun": true}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "DryRunOperation"),
        "case CreateProgressUpdateStream dry run: got HTTP {status} body {body}"
    );
    let (status, body) = emulator
        .call(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": "dry-tool", "MigrationTaskName": "t"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case CreateProgressUpdateStream dry run: the stream must not exist, got HTTP {status} body {body}"
    );

    // Now a real stream and task, notified once, as the baseline every dry run
    // below must leave untouched.
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "dry-tool"}),
        )
        .await;
    emulator
        .ok(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": "dry-tool", "MigrationTaskName": "dry-task"}),
        )
        .await;
    emulator
        .ok(
            "NotifyMigrationTaskState",
            json!({
                "ProgressUpdateStream": "dry-tool",
                "MigrationTaskName": "dry-task",
                "Task": {"Status": "IN_PROGRESS", "ProgressPercent": 42, "StatusDetail": "copying"},
                "UpdateDateTime": 1_761_084_000.0,
                "NextUpdateSeconds": 60,
            }),
        )
        .await;
    emulator
        .ok(
            "PutResourceAttributes",
            json!({
                "ProgressUpdateStream": "dry-tool",
                "MigrationTaskName": "dry-task",
                "ResourceAttributeList": [{"Type": "IPV4_ADDRESS", "Value": "10.0.0.7"}],
            }),
        )
        .await;
    let baseline = emulator
        .ok(
            "DescribeMigrationTask",
            json!({"ProgressUpdateStream": "dry-tool", "MigrationTaskName": "dry-task"}),
        )
        .await;

    let dry_runs: Vec<(&str, Value)> = vec![
        (
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "dry-tool", "DryRun": true}),
        ),
        (
            "ImportMigrationTask",
            json!({
                "ProgressUpdateStream": "dry-tool",
                "MigrationTaskName": "second-task",
                "DryRun": true,
            }),
        ),
        (
            "NotifyMigrationTaskState",
            json!({
                "ProgressUpdateStream": "dry-tool",
                "MigrationTaskName": "dry-task",
                "Task": {"Status": "COMPLETED", "ProgressPercent": 100},
                "UpdateDateTime": 1_761_099_999.0,
                "NextUpdateSeconds": 0,
                "DryRun": true,
            }),
        ),
        (
            "PutResourceAttributes",
            json!({
                "ProgressUpdateStream": "dry-tool",
                "MigrationTaskName": "dry-task",
                "ResourceAttributeList": [{"Type": "BIOS_ID", "Value": "replaced"}],
                "DryRun": true,
            }),
        ),
        (
            "DeleteProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "dry-tool", "DryRun": true}),
        ),
    ];

    for (operation, body) in dry_runs {
        let (status, parsed) = emulator.call(operation, body.clone()).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "DryRunOperation"),
            "case {operation} {body}: expected 400 DryRunOperation, got HTTP {status} body {parsed}"
        );
        let after = emulator
            .ok(
                "DescribeMigrationTask",
                json!({"ProgressUpdateStream": "dry-tool", "MigrationTaskName": "dry-task"}),
            )
            .await;
        assert_eq!(
            after, baseline,
            "case {operation} {body}: a dry run must change nothing"
        );
        let listed = emulator.ok("ListMigrationTasks", json!({})).await;
        assert_eq!(
            listed["MigrationTaskSummaryList"].as_array().map(Vec::len),
            Some(1),
            "case {operation} {body}: a dry run must neither add nor remove tasks, got {listed}"
        );
        assert_eq!(
            emulator.stored_history("dry-tool", "dry-task").len(),
            1,
            "case {operation} {body}: a dry run must not append to the update history"
        );
    }
}

/// The per-task update history is the one store that grows per REQUEST, so it is
/// bounded: past `MAX_UPDATES_PER_TASK` the OLDEST entries are evicted while the
/// newest stay exact, and the task's current state is unaffected.
#[tokio::test]
async fn update_history_is_bounded_and_evicts_the_oldest() {
    let emulator = Emulator::start();
    let (stream, task) = ("bounded-tool", "bounded-task");
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": stream}),
        )
        .await;
    emulator
        .ok(
            "ImportMigrationTask",
            json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
        )
        .await;

    // One notification per second, starting at this instant, so each entry is
    // identifiable by its UpdateDateTime and "the oldest 5 were evicted" is an
    // assertion about WHICH entries remain, not only how many.
    const FIRST_UPDATE_SECS: i64 = 1_761_084_000;
    let overflow = 5usize;
    let sent = MAX_UPDATES_PER_TASK + overflow;
    for index in 0..sent {
        let when = FIRST_UPDATE_SECS + index as i64;
        emulator
            .ok(
                "NotifyMigrationTaskState",
                json!({
                    "ProgressUpdateStream": stream,
                    "MigrationTaskName": task,
                    "Task": {"Status": "IN_PROGRESS", "ProgressPercent": 1, "StatusDetail": "tick"},
                    "UpdateDateTime": when as f64,
                    "NextUpdateSeconds": 0,
                }),
            )
            .await;
    }

    let history = emulator.stored_history(stream, task);
    assert_eq!(
        history.len(),
        MAX_UPDATES_PER_TASK,
        "case {sent} notifications: the history is capped at {MAX_UPDATES_PER_TASK}, got {}",
        history.len()
    );
    assert_eq!(
        history.first().map(|entry| entry.0),
        Some((FIRST_UPDATE_SECS + overflow as i64) as f64),
        "case {sent} notifications: the oldest {overflow} entries are the evicted ones, got {:?}",
        history.first()
    );
    assert_eq!(
        history.last().map(|entry| entry.0),
        Some((FIRST_UPDATE_SECS + sent as i64 - 1) as f64),
        "case {sent} notifications: the newest entry is the last one sent, got {:?}",
        history.last()
    );

    // Eviction touches the history only: the task still reports the state its last
    // notification wrote.
    let described = emulator
        .ok(
            "DescribeMigrationTask",
            json!({"ProgressUpdateStream": stream, "MigrationTaskName": task}),
        )
        .await;
    assert_eq!(
        described["MigrationTask"]["UpdateDateTime"].as_f64(),
        Some((FIRST_UPDATE_SECS + sent as i64 - 1) as f64),
        "case {sent} notifications: the current state is the newest one, got {described}"
    );
}

/// Both caller-named registries are bounded too: at the bound a NEW name is
/// refused with a modelled error, while re-creating a name the store already
/// holds still succeeds (creation is idempotent on the name, so it adds nothing).
///
/// The stores are pre-filled directly so the test asserts the boundary rather
/// than spending a request per record.
#[tokio::test]
async fn caller_named_registries_refuse_new_records_at_their_bound() {
    let emulator = Emulator::start();

    // Table: (what is filled, the operation that adds a record, a new name, a name
    // already stored).
    let streams_at_bound = |emulator: &Emulator| {
        let state = emulator
            .engine
            .state::<ComAmazonawsMigrationhubAwsmigrationhubState>();
        let mut guard = state.lock();
        for index in 0..MAX_PROGRESS_UPDATE_STREAMS {
            guard
                .create_stream(&format!("filler-{index:05}"))
                .expect("pre-filling up to the bound succeeds");
        }
        assert_eq!(
            guard.progress_update_streams.len(),
            MAX_PROGRESS_UPDATE_STREAMS,
            "the stream registry must be pre-filled exactly to its bound"
        );
    };
    streams_at_bound(&emulator);

    let (status, body) = emulator
        .call(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "one-too-many"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (500, "ServiceUnavailableException"),
        "case a new stream at the bound of {MAX_PROGRESS_UPDATE_STREAMS}: got HTTP {status} body {body}"
    );
    // A stream the registry already holds is still created (idempotently).
    emulator
        .ok(
            "CreateProgressUpdateStream",
            json!({"ProgressUpdateStreamName": "filler-00000"}),
        )
        .await;

    // The per-stream task map has the same shape. Fill one stream to its bound.
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsMigrationhubAwsmigrationhubState>();
        let mut guard = state.lock();
        let stream = guard
            .stream_mut("filler-00000")
            .expect("the filler stream exists");
        for index in 0..MAX_TASKS_PER_STREAM {
            stream
                .import_task(&format!("task-{index:06}"))
                .expect("pre-filling up to the bound succeeds");
        }
        assert_eq!(
            stream.tasks.len(),
            MAX_TASKS_PER_STREAM,
            "the task map must be pre-filled exactly to its bound"
        );
    }

    let (status, body) = emulator
        .call(
            "ImportMigrationTask",
            json!({
                "ProgressUpdateStream": "filler-00000",
                "MigrationTaskName": "one-too-many",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (500, "ServiceUnavailableException"),
        "case a new task at the bound of {MAX_TASKS_PER_STREAM}: got HTTP {status} body {body}"
    );
    // A task already registered is still imported (idempotently), and remains
    // readable, so the bound refuses growth and nothing else.
    emulator
        .ok(
            "ImportMigrationTask",
            json!({
                "ProgressUpdateStream": "filler-00000",
                "MigrationTaskName": "task-000000",
            }),
        )
        .await;
    let described = emulator
        .ok(
            "DescribeMigrationTask",
            json!({
                "ProgressUpdateStream": "filler-00000",
                "MigrationTaskName": "task-000000",
            }),
        )
        .await;
    assert_eq!(
        described["MigrationTask"]["MigrationTaskName"].as_str(),
        Some("task-000000"),
        "case a stored task at the bound: it must still be readable, got {described}"
    );
}

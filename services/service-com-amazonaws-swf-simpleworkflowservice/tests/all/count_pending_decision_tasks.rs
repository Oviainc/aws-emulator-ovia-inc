// SPDX-License-Identifier: CC0-1.0

//! `CountPendingDecisionTasks`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the operation answers with the NUMBER of decision
//! tasks waiting in the named domain's task list — one per decision task scheduled on
//! that list by a run of that domain that is still OPEN and that nothing has taken out
//! of the list — so a list no decision task was ever scheduled in answers 0, a list
//! named in another domain contributes nothing, an unregistered domain is the modelled
//! `UnknownResourceFault`, and counting changes nothing a later caller sees.
//!
//! Every expected count is computed by the TEST from the seeded table, never hardcoded,
//! and the rows vary along what the rule generalises over: which task list is asked
//! about, how many runs share it, whether the list came from the request or from the
//! workflow type's registered default, whether its runs are open or closed, the domain
//! the list is asked about in, and how the count moves as runs start and close.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SimpleWorkflowService";

/// The region the `Authorization` credential scope below signs for.
const REGION: &str = "us-east-1";

/// The task list the seeded workflow type carries as its `defaultTaskList`, so a start
/// that names no `taskList` schedules its decision task there.
const DEFAULT_LIST: &str = "registered-default-list";

/// One emulator: a fresh engine behind a router that claims `swf` requests.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call one operation and return `(status, body)`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/swf/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("authorization", authorization)
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error}: {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// Call one operation, requiring a 200, and return the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// Call one operation, requiring a 400 with `expected_code`.
    async fn refused(&self, case: &str, operation: &str, body: &Value, expected_code: &str) {
        let (status, parsed) = self.call(operation, body).await;
        let code = error_code(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected {operation} to answer 400 {expected_code}, \
             got {status} {code} with {parsed:?}"
        );
    }

    /// `CountPendingDecisionTasks` over one `(domain, taskList)`, requiring a 200.
    async fn count(&self, case: &str, domain: &str, task_list: &str) -> Value {
        self.ok(
            case,
            "CountPendingDecisionTasks",
            &json!({ "domain": domain, "taskList": { "name": task_list } }),
        )
        .await
    }

    /// The `count` member of a successful answer.
    async fn count_of(&self, case: &str, domain: &str, task_list: &str) -> i64 {
        let body = self.count(case, domain, task_list).await;
        assert_eq!(
            body.get("truncated").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: the store holds far fewer tasks than this API truncates at, \
             so truncated must be false, got {body:?}"
        );
        body.get("count")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a count member in {body:?}"))
    }

    /// Start one run of the seeded workflow type, on `task_list` when it is given (and
    /// on the type's `defaultTaskList` when it is not). Returns the minted `runId`.
    async fn start(&self, domain: &str, workflow_id: &str, task_list: Option<&str>) -> String {
        let mut body = json!({
            "domain": domain,
            "workflowId": workflow_id,
            "workflowType": { "name": "alpha", "version": "1.0" },
        });
        if let Some(task_list) = task_list {
            body.as_object_mut()
                .expect("a JSON object")
                .insert("taskList".to_string(), json!({ "name": task_list }));
        }
        let started = self.ok(workflow_id, "StartWorkflowExecution", &body).await;
        started
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the start of {workflow_id:?} must mint a runId"))
            .to_string()
    }

    /// Terminate one run, closing it.
    async fn terminate(&self, domain: &str, workflow_id: &str) {
        self.ok(
            workflow_id,
            "TerminateWorkflowExecution",
            &json!({ "domain": domain, "workflowId": workflow_id }),
        )
        .await;
    }

    /// `DescribeWorkflowExecution` for one run.
    async fn describe(&self, domain: &str, workflow_id: &str, run_id: &str) -> Value {
        self.ok(
            workflow_id,
            "DescribeWorkflowExecution",
            &json!({
                "domain": domain,
                "execution": { "workflowId": workflow_id, "runId": run_id },
            }),
        )
        .await
    }
}

/// The `__type` discriminator of an awsJson error body, reduced to the shape name.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// One run this test seeds: its id, the task list its start names (`None` to inherit
/// the workflow type's `defaultTaskList`), and whether it is terminated afterwards (so
/// it is CLOSED and its decision task is no longer waiting anywhere).
#[derive(Debug)]
struct SeededRun {
    workflow_id: &'static str,
    requested_task_list: Option<&'static str>,
    terminated: bool,
}

impl SeededRun {
    /// The task list this run's decision task was scheduled on.
    fn task_list(&self) -> &'static str {
        self.requested_task_list.unwrap_or(DEFAULT_LIST)
    }
}

/// The seeded runs: two sharing one list, one alone on a second list, one that inherits
/// the registered default list, and one that is terminated after starting — on a list
/// no other run uses, so that list's only decision task belongs to a closed run.
const SEEDED: [SeededRun; 5] = [
    SeededRun {
        workflow_id: "wf-shared-1",
        requested_task_list: Some("deciders-shared"),
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-shared-2",
        requested_task_list: Some("deciders-shared"),
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-alone",
        requested_task_list: Some("deciders-alone"),
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-inherits-default",
        requested_task_list: None,
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-closed",
        requested_task_list: Some("deciders-closed"),
        terminated: true,
    },
];

/// The rule, applied by the TEST to the seeded table: one waiting decision task per
/// still-OPEN seeded run whose start scheduled it on exactly this list. This is the
/// oracle the handler's answer is checked against.
fn expected_count(task_list: &str) -> i64 {
    SEEDED
        .iter()
        .filter(|run| !run.terminated && run.task_list() == task_list)
        .count() as i64
}

/// Register the domain, the workflow type every seeded run uses, and (when `seed_runs`)
/// start the seeded runs, terminating the one that must end up CLOSED. Returns the
/// `runId` the service minted for each started `workflowId`.
async fn seed(emulator: &Emulator, domain: &str, seed_runs: bool) -> Vec<(&'static str, String)> {
    emulator
        .ok(
            "seed",
            "RegisterDomain",
            &json!({ "name": domain, "workflowExecutionRetentionPeriodInDays": "7" }),
        )
        .await;
    emulator
        .ok(
            "seed",
            "RegisterWorkflowType",
            &json!({
                "domain": domain,
                "name": "alpha",
                "version": "1.0",
                "defaultTaskList": { "name": DEFAULT_LIST },
                "defaultTaskStartToCloseTimeout": "60",
                "defaultExecutionStartToCloseTimeout": "300",
                "defaultChildPolicy": "TERMINATE",
            }),
        )
        .await;
    let mut run_ids = Vec::new();
    if seed_runs {
        for run in &SEEDED {
            let run_id = emulator
                .start(domain, run.workflow_id, run.requested_task_list)
                .await;
            if run.terminated {
                emulator.terminate(domain, run.workflow_id).await;
            }
            run_ids.push((run.workflow_id, run_id));
        }
    }
    run_ids
}

#[tokio::test]
async fn the_count_is_the_number_of_decision_tasks_waiting_in_the_named_task_list() {
    let emulator = Emulator::new();
    let run_ids = seed(&emulator, "pending", true).await;

    // Rows: every list the table uses — one shared by two runs, one used by a single
    // run, the registered default list a start inherited, and the list whose only run
    // was closed — plus a list nothing was ever scheduled in and one differing from a
    // used list only in case.
    let rows = [
        "deciders-shared",
        "deciders-alone",
        DEFAULT_LIST,
        "deciders-closed",
        "never-scheduled-in",
        "DECIDERS-SHARED",
    ];

    for task_list in rows {
        let actual = emulator.count_of(task_list, "pending", task_list).await;
        assert_eq!(
            actual,
            expected_count(task_list),
            "case {task_list:?}: expected one waiting decision task per open seeded run \
             scheduled on that list, got {actual}"
        );

        // A count is a pure read: asking twice answers the same.
        let again = emulator.count_of(task_list, "pending", task_list).await;
        assert_eq!(
            again, actual,
            "case {task_list:?}: counting twice must answer the same, got {again}"
        );
    }

    // Every waiting task belongs to exactly one list, so the lists the table uses
    // partition the open runs' decision tasks.
    let lists: BTreeSet<&'static str> = SEEDED.iter().map(SeededRun::task_list).collect();
    let mut total = 0;
    for task_list in &lists {
        total += emulator.count_of("partition", "pending", task_list).await;
    }
    let open_runs = SEEDED.iter().filter(|run| !run.terminated).count() as i64;
    assert_eq!(
        total, open_runs,
        "case \"partition\": the counts of every used list must add up to the {open_runs} \
         open seeded runs, got {total}"
    );

    // Counting moved nothing: every seeded run keeps the status it had, and its one
    // decision task — which no decider ever answered, not even on the runs that were
    // terminated — is still outstanding.
    for (run, (_, run_id)) in SEEDED.iter().zip(&run_ids) {
        let described = emulator.describe("pending", run.workflow_id, run_id).await;
        let expected_status = if run.terminated { "CLOSED" } else { "OPEN" };
        let status = described
            .pointer("/executionInfo/executionStatus")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        assert_eq!(
            status, expected_status,
            "case {:?}: counting must not move a run's status, got {described:?}",
            run.workflow_id
        );
        let open_decision_tasks = described
            .pointer("/openCounts/openDecisionTasks")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        assert_eq!(
            open_decision_tasks, 1,
            "case {:?}: counting must not consume a run's decision task, got {described:?}",
            run.workflow_id
        );
    }
}

#[tokio::test]
async fn the_count_follows_the_runs_that_start_and_close_on_the_list() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "moving", false).await;
    let list = "deciders-moving";

    // A list in a domain with no runs at all holds nothing.
    let empty = emulator.count_of("no runs yet", "moving", list).await;
    assert_eq!(
        empty, 0,
        "case \"no runs yet\": a list no decision task was ever scheduled in holds none, \
         got {empty}"
    );

    // Each start schedules one decision task on the list, so the count rises by one per
    // start; a start on another list leaves this one where it is.
    let mut run_ids = Vec::new();
    for ordinal in 1..=3 {
        let workflow_id = format!("wf-moving-{ordinal}");
        run_ids.push(emulator.start("moving", &workflow_id, Some(list)).await);
        let case = format!("after {ordinal} start(s)");
        let actual = emulator.count_of(&case, "moving", list).await;
        assert_eq!(
            actual, ordinal,
            "case {case:?}: each start schedules one decision task on {list:?}, got {actual}"
        );
    }
    emulator
        .start("moving", "wf-moving-elsewhere", Some("deciders-other"))
        .await;
    let actual = emulator.count_of("start on another list", "moving", list).await;
    assert_eq!(
        actual, 3,
        "case \"start on another list\": a task scheduled on {:?} is not in {list:?}, got {actual}",
        "deciders-other"
    );

    // Closing a run takes its waiting decision task out of the list, so the count falls
    // by one per close and reaches zero when every run on the list has closed.
    for ordinal in 1..=3 {
        emulator
            .terminate("moving", &format!("wf-moving-{ordinal}"))
            .await;
        let case = format!("after {ordinal} termination(s)");
        let actual = emulator.count_of(&case, "moving", list).await;
        assert_eq!(
            actual,
            3 - ordinal,
            "case {case:?}: a closed run's decision task is no longer waiting, got {actual}"
        );
    }

    // The run on the other list was untouched by all of that.
    let other = emulator.count_of("other list", "moving", "deciders-other").await;
    assert_eq!(
        other, 1,
        "case \"other list\": its one open run still has a decision task waiting, got {other}"
    );
}

#[tokio::test]
async fn a_task_list_is_scoped_to_its_own_domain() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "left", true).await;
    let _ = seed(&emulator, "right", false).await;

    // The same list name in a second domain starts empty, and one run there does not
    // change what the first domain reports.
    let list = "deciders-shared";
    let before = emulator.count_of("right before", "right", list).await;
    assert_eq!(
        before, 0,
        "case \"right before\": a same-named list in another domain holds nothing, got {before}"
    );
    emulator.start("right", "wf-right", Some(list)).await;
    for (case, domain, expected) in [
        ("right after", "right", 1),
        ("left after", "left", expected_count(list)),
    ] {
        let actual = emulator.count_of(case, domain, list).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: {domain:?} must count only its own runs on {list:?}, got {actual}"
        );
    }

    // A domain that was never registered addresses no task list: the modelled
    // UnknownResourceFault, and asking for it does not create it.
    for domain in ["never-registered", "LEFT", "left-2"] {
        emulator
            .refused(
                domain,
                "CountPendingDecisionTasks",
                &json!({ "domain": domain, "taskList": { "name": list } }),
                "UnknownResourceFault",
            )
            .await;
        emulator
            .refused(
                domain,
                "DescribeDomain",
                &json!({ "name": domain }),
                "UnknownResourceFault",
            )
            .await;
    }
}

#[tokio::test]
async fn a_task_list_name_sws_naming_rule_rejects_is_a_bad_request() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "names", false).await;

    // The naming rule stated on every user-supplied SWF name, `taskList.name` included:
    // "The specified string must not start or end with whitespace. It must not contain a
    // : (colon), / (slash), | (vertical bar), or any control characters
    // (\u0000-\u001f | \u007f-\u009f). Also, it must not be the literal string arn."
    let rejected = [
        ("leading whitespace", " deciders"),
        ("trailing whitespace", "deciders "),
        ("a colon", "deciders:1"),
        ("a slash", "deciders/1"),
        ("a vertical bar", "deciders|1"),
        ("a control character", "deciders\u{1}"),
        ("a high control character", "deciders\u{7f}"),
        ("the literal arn", "arn"),
    ];
    for (case, task_list) in rejected {
        emulator
            .refused(
                case,
                "CountPendingDecisionTasks",
                &json!({ "domain": "names", "taskList": { "name": task_list } }),
                "ValidationException",
            )
            .await;
    }

    // Names the rule allows are counted, not refused — including one holding characters
    // next to the rejected ones and one at the modelled 256-character maximum.
    let accepted = [
        ("plain", "deciders".to_string()),
        ("dots and dashes", "deciders.a-1_b".to_string()),
        ("inner whitespace", "deciders list".to_string()),
        ("arn with a suffix", "arn-ish".to_string()),
        ("the longest modelled name", "d".repeat(256)),
    ];
    for (case, task_list) in &accepted {
        let actual = emulator.count_of(case, "names", task_list).await;
        assert_eq!(
            actual, 0,
            "case {case:?}: no decision task was scheduled in {task_list:?}, got {actual}"
        );
    }
}

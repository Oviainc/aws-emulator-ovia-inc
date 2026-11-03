// SPDX-License-Identifier: CC0-1.0

//! `CountPendingActivityTasks`, end to end over the service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<Operation>`)
//! through the same `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: the operation answers with the NUMBER of activity
//! tasks waiting in the named domain's task list — one per activity task scheduled on
//! that list by a run of that domain that is still OPEN and that nothing has taken out
//! of the list — so a list no activity task was ever scheduled in answers 0, a list
//! named in another domain contributes nothing, a count past the most this API reports
//! comes back clipped with `truncated` set, a list named in a domain no record is stored
//! under likewise answers 0, and counting changes nothing a later caller sees.
//!
//! Every expected count is computed by the TEST from the seeded table — and
//! cross-checked against the same runs' histories read back over the wire — never
//! hardcoded. Rows vary along what the rule generalises over: how many tasks a run
//! schedules, how many runs share a list, how many lists one run schedules on, whether
//! its runs are open or closed, the domain the list is asked about in, the shape of the
//! list's name, and whether the count is below, at or past the reported maximum.
//!
//! The scheduling events a row seeds are appended to the run's stored history directly
//! because their only producer — `RespondDecisionTaskCompleted`'s `ScheduleActivityTask`
//! decision — is still a 501 scaffold tracked by its own gap issue. That is also why
//! this suite keeps the engine handle, rather than reusing a harness that drops it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::{
    ActivityTaskScheduledEventAttributes, ActivityType, TaskList,
};
use service_com_amazonaws_swf_simpleworkflowservice::state::{
    now, service_state, EventAttributes, MAX_REPORTED_EXECUTION_COUNT,
};
use service_com_amazonaws_swf_simpleworkflowservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SimpleWorkflowService";

/// The region the `Authorization` credential scope below signs for.
const REGION: &str = "us-east-1";

/// One emulator: a fresh engine behind a router that claims `swf` requests. The engine
/// handle is kept so a test can append the scheduling events the decision operation
/// will write once it exists.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
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

    /// `CountPendingActivityTasks` over one `(domain, taskList)`, as
    /// `(count, truncated)`.
    async fn count(&self, case: &str, domain: &str, task_list: &str) -> (i64, bool) {
        let body = self
            .ok(
                case,
                "CountPendingActivityTasks",
                &json!({ "domain": domain, "taskList": { "name": task_list } }),
            )
            .await;
        let count = body
            .get("count")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a count member in {body:?}"));
        let truncated = body
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| panic!("case {case:?}: expected a truncated member in {body:?}"));
        (count, truncated)
    }

    /// The `count` of a successful answer that must not be truncated.
    async fn count_of(&self, case: &str, domain: &str, task_list: &str) -> i64 {
        let (count, truncated) = self.count(case, domain, task_list).await;
        assert!(
            !truncated,
            "case {case:?}: {task_list:?} holds far fewer tasks than this API truncates at, \
             so truncated must be false, got count {count}"
        );
        count
    }

    /// Start one run of the seeded workflow type. Returns the minted `runId`.
    async fn start(&self, domain: &str, workflow_id: &str) -> String {
        let started = self
            .ok(
                workflow_id,
                "StartWorkflowExecution",
                &json!({
                    "domain": domain,
                    "workflowId": workflow_id,
                    "workflowType": { "name": "alpha", "version": "1.0" },
                }),
            )
            .await;
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

    /// Every event of one run's history, read back over the wire.
    async fn history(&self, domain: &str, workflow_id: &str, run_id: &str) -> Vec<Value> {
        let body = self
            .ok(
                workflow_id,
                "GetWorkflowExecutionHistory",
                &json!({
                    "domain": domain,
                    "execution": { "workflowId": workflow_id, "runId": run_id },
                }),
            )
            .await;
        body.get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("the history of {workflow_id:?} must carry events"))
    }

    /// Schedule `count` activity tasks of this run on `task_list`: append one
    /// `ActivityTaskScheduled` event per task to its stored history — the record
    /// `ScheduleActivityTask` will write — and count each as an open activity task of
    /// the run.
    fn schedule_activity_tasks(
        &self,
        domain: &str,
        workflow_id: &str,
        run_id: &str,
        task_list: &str,
        count: usize,
    ) {
        let state = service_state(&*self.engine).expect("the SWF store is reachable");
        let mut guard = state.lock();
        let execution = guard
            .domains
            .get_mut(domain)
            .unwrap_or_else(|| panic!("domain {domain:?} must be registered"))
            .executions
            .get_mut(&(workflow_id.to_string(), run_id.to_string()))
            .unwrap_or_else(|| panic!("run {workflow_id:?} must be started"));
        for index in 0..count {
            execution.append_event(
                now(),
                EventAttributes::ActivityTaskScheduled(Box::new(
                    ActivityTaskScheduledEventAttributes {
                        activity_type: ActivityType {
                            name: "ship".to_string(),
                            version: "1.0".to_string(),
                        },
                        activity_id: format!("{task_list}-{index}"),
                        task_list: TaskList {
                            name: task_list.to_string(),
                        },
                        ..Default::default()
                    },
                )),
            );
            execution.open_activity_tasks += 1;
        }
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

/// How many events of a history read back over the wire are activity tasks scheduled on
/// `task_list` — the same record the handler counts, as a client sees it.
fn scheduled_on(events: &[Value], task_list: &str) -> i64 {
    let matching = events
        .iter()
        .filter(|event| {
            event.get("eventType").and_then(Value::as_str) == Some("ActivityTaskScheduled")
                && event
                    .pointer("/activityTaskScheduledEventAttributes/taskList/name")
                    .and_then(Value::as_str)
                    == Some(task_list)
        })
        .count();
    i64::try_from(matching).expect("an event count fits an i64")
}

/// One run this test seeds: its id, how many activity tasks it schedules on which task
/// lists, and whether it is terminated afterwards (so it is CLOSED and its activity
/// tasks are no longer waiting anywhere).
#[derive(Debug)]
struct SeededRun {
    workflow_id: &'static str,
    /// `(task list, how many activity tasks this run schedules on it)`.
    scheduled: &'static [(&'static str, usize)],
    terminated: bool,
}

impl SeededRun {
    /// How many activity tasks this run scheduled on `task_list`.
    fn scheduled_on(&self, task_list: &str) -> usize {
        self.scheduled
            .iter()
            .filter(|(list, _)| *list == task_list)
            .map(|(_, count)| *count)
            .sum()
    }
}

/// The seeded runs: one scheduling on TWO lists, one sharing the first of them, one
/// scheduling nothing at all, one closed run whose list no open run uses, and one closed
/// run that had scheduled on the shared list.
const SEEDED: [SeededRun; 5] = [
    SeededRun {
        workflow_id: "wf-two-lists",
        scheduled: &[("activities-shared", 2), ("activities-alone", 1)],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-shares-the-list",
        scheduled: &[("activities-shared", 3)],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-schedules-none",
        scheduled: &[],
        terminated: false,
    },
    SeededRun {
        workflow_id: "wf-closed-alone",
        scheduled: &[("activities-closed", 4)],
        terminated: true,
    },
    SeededRun {
        workflow_id: "wf-closed-shared",
        scheduled: &[("activities-shared", 5)],
        terminated: true,
    },
];

/// The rule, applied by the TEST to the seeded table: one waiting activity task per task
/// a still-OPEN seeded run scheduled on exactly this list. This is the oracle the
/// handler's answer is checked against.
fn expected_count(task_list: &str) -> i64 {
    let total: usize = SEEDED
        .iter()
        .filter(|run| !run.terminated)
        .map(|run| run.scheduled_on(task_list))
        .sum();
    i64::try_from(total).expect("a seeded count fits an i64")
}

/// Register the domain and the workflow type every seeded run uses.
async fn register(emulator: &Emulator, domain: &str) {
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
                "defaultTaskList": { "name": "deciders" },
                "defaultTaskStartToCloseTimeout": "60",
                "defaultExecutionStartToCloseTimeout": "300",
                "defaultChildPolicy": "TERMINATE",
            }),
        )
        .await;
}

/// Register `domain`, start every seeded run in it, schedule its activity tasks and
/// terminate the ones the table marks closed. Returns each run's minted `runId`.
async fn seed(emulator: &Emulator, domain: &str) -> Vec<(&'static str, String)> {
    register(emulator, domain).await;
    let mut run_ids = Vec::new();
    for run in &SEEDED {
        let run_id = emulator.start(domain, run.workflow_id).await;
        for (task_list, count) in run.scheduled {
            emulator.schedule_activity_tasks(domain, run.workflow_id, &run_id, task_list, *count);
        }
        if run.terminated {
            emulator.terminate(domain, run.workflow_id).await;
        }
        run_ids.push((run.workflow_id, run_id));
    }
    run_ids
}

#[tokio::test]
async fn the_count_is_the_number_of_activity_tasks_waiting_in_the_named_task_list() {
    let emulator = Emulator::new();
    let run_ids = seed(&emulator, "pending").await;

    // Rows: every list the table uses — one shared by two open runs (and by a closed
    // one), one used by a single run, the list whose only run was closed — plus a list
    // nothing was ever scheduled in, the decision task list of the same runs (no
    // ACTIVITY task was scheduled there), and a list differing from a used one only in
    // case.
    let rows = [
        "activities-shared",
        "activities-alone",
        "activities-closed",
        "never-scheduled-in",
        "deciders",
        "ACTIVITIES-SHARED",
    ];

    for task_list in rows {
        let actual = emulator.count_of(task_list, "pending", task_list).await;
        assert_eq!(
            actual,
            expected_count(task_list),
            "case {task_list:?}: expected one waiting activity task per task an open seeded \
             run scheduled on that list, got {actual}"
        );

        // A count is a pure read: asking twice answers the same.
        let again = emulator.count_of(task_list, "pending", task_list).await;
        assert_eq!(
            again, actual,
            "case {task_list:?}: counting twice must answer the same, got {again}"
        );

        // And the same number is in the runs' histories as a client reads them: the
        // count comes from that one record, not from a second copy of it.
        let mut from_history = 0;
        for (run, (_, run_id)) in SEEDED.iter().zip(&run_ids) {
            let events = emulator.history("pending", run.workflow_id, run_id).await;
            let scheduled = scheduled_on(&events, task_list);
            assert_eq!(
                scheduled,
                i64::try_from(run.scheduled_on(task_list)).expect("a seeded count fits an i64"),
                "case {task_list:?}: the history of {:?} must record every activity task it \
                 scheduled on that list, got {scheduled}",
                run.workflow_id
            );
            if !run.terminated {
                from_history += scheduled;
            }
        }
        assert_eq!(
            actual, from_history,
            "case {task_list:?}: the count must equal the open runs' scheduled-and-waiting \
             events in their histories ({from_history}), got {actual}"
        );
    }

    // Counting moved nothing: every seeded run keeps the status it had and the open
    // activity tasks it had.
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
        let open_activity_tasks = described
            .pointer("/openCounts/openActivityTasks")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        // The counter reports the tasks of this run whose own status is OPEN, and closing
        // the run closed none of them: every task it scheduled and nobody closed is still
        // counted, terminated run or not.
        let scheduled: usize = run.scheduled.iter().map(|(_, count)| *count).sum();
        assert_eq!(
            open_activity_tasks,
            i64::try_from(scheduled).expect("a seeded count fits an i64"),
            "case {:?}: counting must not consume a run's activity tasks, got {described:?}",
            run.workflow_id
        );
    }
}

#[tokio::test]
async fn the_count_follows_the_tasks_scheduled_on_the_list_and_the_runs_that_close() {
    let emulator = Emulator::new();
    register(&emulator, "moving").await;
    let list = "activities-moving";

    // A list in a domain whose runs scheduled nothing holds nothing.
    let empty = emulator.count_of("no tasks yet", "moving", list).await;
    assert_eq!(
        empty, 0,
        "case \"no tasks yet\": a list no activity task was ever scheduled in holds none, \
         got {empty}"
    );

    // Each scheduled task adds one to the list, whichever run scheduled it; a task
    // scheduled on another list leaves this one where it is.
    let mut started = Vec::new();
    for ordinal in 1..=3 {
        let workflow_id = format!("wf-moving-{ordinal}");
        let run_id = emulator.start("moving", &workflow_id).await;
        emulator.schedule_activity_tasks("moving", &workflow_id, &run_id, list, ordinal);
        emulator.schedule_activity_tasks("moving", &workflow_id, &run_id, "activities-other", 1);
        started.push(workflow_id);

        let case = format!("after {ordinal} run(s)");
        let expected = i64::try_from((1..=ordinal).sum::<usize>()).expect("a sum fits an i64");
        let actual = emulator.count_of(&case, "moving", list).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: every task scheduled on {list:?} is waiting in it, got {actual}"
        );
        let other = emulator
            .count_of(&case, "moving", "activities-other")
            .await;
        assert_eq!(
            other,
            i64::try_from(ordinal).expect("an ordinal fits an i64"),
            "case {case:?}: the other list holds one task per run, got {other}"
        );
    }

    // Closing a run takes its waiting activity tasks out of the list, so the count falls
    // by that run's share and reaches zero once every run on the list has closed.
    let mut remaining: i64 = i64::try_from((1..=3).sum::<usize>()).expect("a sum fits an i64");
    for (index, workflow_id) in started.iter().enumerate() {
        emulator.terminate("moving", workflow_id).await;
        remaining -= i64::try_from(index + 1).expect("an ordinal fits an i64");
        let case = format!("after closing {workflow_id}");
        let actual = emulator.count_of(&case, "moving", list).await;
        assert_eq!(
            actual, remaining,
            "case {case:?}: a closed run's activity tasks are no longer waiting, got {actual}"
        );
    }
    assert_eq!(
        remaining, 0,
        "every seeded run was closed, so the list must have emptied"
    );
}

#[tokio::test]
async fn a_count_past_the_reported_maximum_comes_back_truncated() {
    // `truncated`'s own doc: "If set to true, indicates that the actual count was more
    // than the maximum supported by this API and the count returned is the truncated
    // value." Rows: one short of the maximum, exactly at it, and one past it.
    let emulator = Emulator::new();
    register(&emulator, "truncating").await;
    let reported_max = i64::try_from(MAX_REPORTED_EXECUTION_COUNT).expect("the cap fits an i64");

    for (case, scheduled, expected) in [
        (
            "one below the maximum",
            MAX_REPORTED_EXECUTION_COUNT - 1,
            (reported_max - 1, false),
        ),
        (
            "exactly the maximum",
            MAX_REPORTED_EXECUTION_COUNT,
            (reported_max, false),
        ),
        (
            "one past the maximum",
            MAX_REPORTED_EXECUTION_COUNT + 1,
            (reported_max, true),
        ),
    ] {
        let workflow_id = format!("wf-{scheduled}");
        let run_id = emulator.start("truncating", &workflow_id).await;
        let list = format!("activities-{scheduled}");
        emulator.schedule_activity_tasks("truncating", &workflow_id, &run_id, &list, scheduled);

        let actual = emulator.count(case, "truncating", &list).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: {scheduled} waiting tasks must be reported as {expected:?}, \
             got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_task_list_is_scoped_to_its_own_domain() {
    let emulator = Emulator::new();
    let _ = seed(&emulator, "left").await;
    register(&emulator, "right").await;

    // The same list name in a second domain starts empty, and tasks scheduled there do
    // not change what the first domain reports.
    let list = "activities-shared";
    let before = emulator.count_of("right before", "right", list).await;
    assert_eq!(
        before, 0,
        "case \"right before\": a same-named list in another domain holds nothing, got {before}"
    );
    let run_id = emulator.start("right", "wf-right").await;
    emulator.schedule_activity_tasks("right", "wf-right", &run_id, list, 2);
    for (case, domain, expected) in [
        ("right after", "right", 2),
        ("left after", "left", expected_count(list)),
    ] {
        let actual = emulator.count_of(case, domain, list).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: {domain:?} must count only its own runs on {list:?}, got {actual}"
        );
    }

    // A domain that was never registered holds no task waiting in any list: the
    // documented zero, and asking for it does not create the domain.
    for domain in ["never-registered", "LEFT", "left-2"] {
        let actual = emulator.count_of(domain, domain, list).await;
        assert_eq!(
            actual, 0,
            "case {domain:?}: a list in a domain no record is stored under holds nothing, \
             got {actual}"
        );
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
async fn a_task_list_name_swfs_naming_rule_rejects_is_a_bad_request() {
    let emulator = Emulator::new();
    register(&emulator, "names").await;

    // The naming rule stated on every user-supplied SWF name, `taskList.name` included:
    // "The specified string must not start or end with whitespace. It must not contain a
    // : (colon), / (slash), | (vertical bar), or any control characters
    // (\u0000-\u001f | \u007f-\u009f). Also, it must not be the literal string arn."
    let rejected = [
        ("leading whitespace", " activities"),
        ("trailing whitespace", "activities "),
        ("a colon", "activities:1"),
        ("a slash", "activities/1"),
        ("a vertical bar", "activities|1"),
        ("a control character", "activities\u{1}"),
        ("a high control character", "activities\u{7f}"),
        ("the literal arn", "arn"),
    ];
    for (case, task_list) in rejected {
        emulator
            .refused(
                case,
                "CountPendingActivityTasks",
                &json!({ "domain": "names", "taskList": { "name": task_list } }),
                "ValidationException",
            )
            .await;
    }

    // Names the rule allows are counted, not refused — including one holding characters
    // next to the rejected ones and one at the modelled 256-character maximum.
    let accepted = [
        ("plain", "activities".to_string()),
        ("dots and dashes", "activities.a-1_b".to_string()),
        ("inner whitespace", "activities list".to_string()),
        ("arn with a suffix", "arn-ish".to_string()),
        ("the longest modelled name", "a".repeat(256)),
    ];
    for (case, task_list) in &accepted {
        let actual = emulator.count_of(case, "names", task_list).await;
        assert_eq!(
            actual, 0,
            "case {case:?}: no activity task was scheduled in {task_list:?}, got {actual}"
        );
    }
}

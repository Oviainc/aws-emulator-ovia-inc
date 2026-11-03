// SPDX-License-Identifier: CC0-1.0

//! The decision-task LONG POLL and the per-task-list poller cap, end to end over the
//! service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: SimpleWorkflowService.<Operation>`) through the same
//! `MultiServiceRouter` the emulator mounts in production.
//!
//! The rule every case here asserts: `PollForDecisionTask` is a long poll — "the service
//! holds the HTTP connection open and responds as soon a task becomes available" — and an
//! open poll is counted against the task list it is waiting on, so a task list admits at
//! most `MAX_POLLERS_PER_TASK_LIST` simultaneous pollers (the Amazon SWF Developer
//! Guide's quota of 1,000 pollers per task list) and a poll offered past that is refused
//! promptly with 400 `LimitExceededFault` — never `OperationNotPermittedFault` — carrying
//! this condition's own fixed message ([`POLLER_LIMIT_EXCEEDED_MESSAGE`], whose wording
//! `poller_cap_limit_message` asserts over the whole class). The cap is
//! counted per task list, and the slot is given back the moment the poll holding it ends,
//! however it ends.
//!
//! The suite varies what the rule generalises over: whether the list has a task waiting,
//! which task list a poller is counted against, and how a poll ends (answered, or its
//! caller gone).
//!
//! `poll_for_decision_task.rs` covers which task a poll is handed and what it answers
//! with; this suite stands up its own harness because the poller count lives in this
//! service's state and reading it is what makes the cap assertions deterministic — no
//! sleep has to be guessed, and a cap that never fills fails instead of hanging.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::state::{
    service_state, ComAmazonawsSwfSimpleworkflowserviceState, MAX_POLLERS_PER_TASK_LIST,
    POLLER_LIMIT_EXCEEDED_MESSAGE,
};
use service_com_amazonaws_swf_simpleworkflowservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, merge, register_domain_body, string_at,
};

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SimpleWorkflowService";

/// The region the `Authorization` credential scope below signs for.
const REGION: &str = "us-east-1";

/// The workflow type every run here is started from.
const TYPE: (&str, &str) = ("demo-wf", "1.0");

/// The task list the seeded workflow type registers as its `defaultTaskList`.
const DEFAULT_LIST: &str = "registered-default-list";

/// How long a poll is given to prove it is NOT answering. A poll of an empty task list
/// holds the connection open for the documented 60 seconds, so any wait far below that
/// which produces no answer shows the hold.
const HOLD_OPEN_PROBE: Duration = Duration::from_millis(250);

/// How long an answer that must come promptly is given before the test fails.
const PROMPT: Duration = Duration::from_secs(5);

/// One emulator, with the engine kept beside the router so the test can read this
/// service's own poller counter.
///
/// `pub(crate)` so the sibling suite that asserts the over-cap refusal's wording drives
/// the service through this same harness rather than standing up one of its own.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation and return `(status, body)`.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
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
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed:?}"
        );
        parsed
    }

    /// How many decision-task pollers this service's state counts on `task_list`.
    fn open_pollers(&self, domain: &str, task_list: &str) -> usize {
        let state = self.state();
        let guard = state.lock();
        guard
            .domains
            .get(domain)
            .map_or(0, |domain| domain.open_decision_pollers(task_list))
    }

    /// This service's own store, reached the way its handlers reach it.
    pub(crate) fn state(
        &self,
    ) -> ::engine::ServiceState<ComAmazonawsSwfSimpleworkflowserviceState> {
        service_state(&*self.engine).expect("the SWF state is reachable")
    }
}

/// Register a domain and the workflow type every run here uses.
pub(crate) async fn seed(harness: &Harness, case: &str, domain: &str) {
    harness
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
    let mut body = json!({ "domain": domain, "name": TYPE.0, "version": TYPE.1 });
    merge(&mut body, full_defaults(DEFAULT_LIST, "TERMINATE"));
    harness.ok(case, "RegisterWorkflowType", &body).await;
}

/// Start one run of the seeded type, whose decision task waits in [`DEFAULT_LIST`].
async fn start_run(harness: &Harness, case: &str, domain: &str, workflow_id: &str) {
    harness
        .ok(
            case,
            "StartWorkflowExecution",
            &json!({
                "domain": domain,
                "workflowId": workflow_id,
                "workflowType": { "name": TYPE.0, "version": TYPE.1 },
            }),
        )
        .await;
}

/// A `PollForDecisionTask` request on one task list.
fn poll_body(domain: &str, task_list: &str) -> Value {
    json!({
        "domain": domain,
        "taskList": { "name": task_list },
        "identity": "decider-1",
    })
}

/// Open one poll in its own task, so the test can go on while it waits.
fn open_poll(
    harness: &Arc<Harness>,
    domain: &str,
    task_list: &str,
) -> tokio::task::JoinHandle<(u16, Value)> {
    let harness = Arc::clone(harness);
    let body = poll_body(domain, task_list);
    tokio::spawn(async move { harness.call("PollForDecisionTask", &body).await })
}

/// Wait until `task_list` counts exactly `expected` open pollers.
///
/// The counter is the admission record, so waiting on it is what makes the assertions
/// below deterministic rather than timing-dependent.
async fn await_pollers(harness: &Harness, domain: &str, task_list: &str, expected: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let open = harness.open_pollers(domain, task_list);
        if open == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {task_list} to hold {expected} open pollers, it held {open}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_poll_of_an_empty_task_list_holds_the_connection_open() {
    let harness = Arc::new(Harness::new());
    let domain = "poll-empty";
    seed(&harness, "empty list", domain).await;

    // No run was started, so no decision task was ever scheduled on this list.
    let polling = open_poll(&harness, domain, DEFAULT_LIST);
    tokio::time::sleep(HOLD_OPEN_PROBE).await;
    assert!(
        !polling.is_finished(),
        "a poll of an empty task list holds the connection open rather than answering; \
         it answered inside {HOLD_OPEN_PROBE:?}"
    );
    assert_eq!(
        harness.open_pollers(domain, DEFAULT_LIST),
        1,
        "the waiting poll is counted against its task list while it is open"
    );

    // A task scheduled while the poll waits is what the poll is waiting for: starting a
    // run hands it over, so the poll answers.
    start_run(&harness, "empty list", domain, "wf-1").await;
    let (status, task) = tokio::time::timeout(PROMPT, polling)
        .await
        .expect("the waiting poll answers once a task is scheduled")
        .expect("the polling task did not panic");
    assert_eq!(
        (status, string_at(&task, "/workflowExecution/workflowId")),
        (200, "wf-1".to_string()),
        "the waiting poll is handed the task the start scheduled, got {task:?}"
    );
    assert!(
        !string_at(&task, "/taskToken").is_empty(),
        "the answered poll carries a real task token, got {task:?}"
    );
    assert_eq!(
        harness.open_pollers(domain, DEFAULT_LIST),
        0,
        "a poll that has answered no longer holds a poller slot"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_list_admits_at_most_the_documented_number_of_simultaneous_pollers() {
    let harness = Arc::new(Harness::new());
    let domain = "poll-cap";
    let other_list = "second-list";
    seed(&harness, "poller cap", domain).await;

    // Fill one task list to its cap. No run was started, so none of these polls can be
    // answered: each holds its slot until it is dropped below.
    let mut open: Vec<tokio::task::JoinHandle<(u16, Value)>> = Vec::new();
    for _ in 0..MAX_POLLERS_PER_TASK_LIST {
        open.push(open_poll(&harness, domain, DEFAULT_LIST));
    }
    await_pollers(&harness, domain, DEFAULT_LIST, MAX_POLLERS_PER_TASK_LIST).await;

    // The next poll on that list is refused, promptly and with the quota's own fault.
    let (status, body) = tokio::time::timeout(
        PROMPT,
        harness.call("PollForDecisionTask", &poll_body(domain, DEFAULT_LIST)),
    )
    .await
    .expect("an over-cap poll is refused promptly rather than held open");
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededFault"),
        "a poll past the per-task-list cap is 400 LimitExceededFault (never \
         OperationNotPermittedFault), got {status} {code} with {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(POLLER_LIMIT_EXCEEDED_MESSAGE),
        "the over-cap refusal carries this condition's fixed message, got {body:?}"
    );

    // The cap is counted PER TASK LIST: another list of the same domain still admits a
    // poller, which is why its own counter moves.
    let elsewhere = open_poll(&harness, domain, other_list);
    await_pollers(&harness, domain, other_list, 1).await;
    assert_eq!(
        harness.open_pollers(domain, DEFAULT_LIST),
        MAX_POLLERS_PER_TASK_LIST,
        "a poller on another list is not counted against the full one"
    );

    // A slot is given back when the poll holding it ends, so the list admits a poller
    // again.
    open.pop().expect("the cap is greater than zero").abort();
    await_pollers(
        &harness,
        domain,
        DEFAULT_LIST,
        MAX_POLLERS_PER_TASK_LIST - 1,
    )
    .await;
    let readmitted = open_poll(&harness, domain, DEFAULT_LIST);
    await_pollers(&harness, domain, DEFAULT_LIST, MAX_POLLERS_PER_TASK_LIST).await;

    for handle in open {
        handle.abort();
    }
    readmitted.abort();
    elsewhere.abort();
}

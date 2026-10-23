// SPDX-License-Identifier: CC0-1.0

//! ADMISSION CONTROL on the command-execution data path: how many command
//! executions one DEVICE may have outstanding at once.
//!
//! The rule asserted here, over a set of devices, burst sizes and drain states
//! rather than one recorded burst:
//!
//! 1. a device admits at most `MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING`
//!    command executions that have not reached a terminal status; every further
//!    `StartCommandExecution` is refused with one of the two admission refusals
//!    the model declares for the operation, stores nothing, and each admitted
//!    execution stays individually retrievable while the device's own listing
//!    reports exactly the admitted set;
//! 2. a slot is RELEASED when the execution holding it reaches a terminal status
//!    — whichever terminal status that is, and including the `TIMED_OUT` the
//!    execution's own timer already made due — so exactly as many further starts
//!    are admitted as slots were released;
//! 3. a retry of an already-admitted `clientToken` is answered from the
//!    idempotency index and so is NOT refused at the limit: it creates no second
//!    execution and consumes no slot.
//!
//! Every request goes over the wire (restJson1 through the multi-service router),
//! so what is asserted is the HTTP behaviour a fleet-control client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::ops::types::JOB_EXECUTION_STATUS_VALUES;
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::state::{
    command_registry, now_epoch_seconds, service_state,
    MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING,
};
use service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router picks the service for a request.
const SIGV4: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/iot-jobs-data/aws4_request, SignedHeaders=host, Signature=0";

const COMMAND_ARN: &str = "arn:aws:iot:us-east-1:111122223333:command/reboot";

/// The device's published command-execution concurrency limit, under a short name.
const LIMIT: usize = MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING;

/// The two ADMISSION refusals `StartCommandExecution`'s model declares, as
/// (HTTP status, error code) — the set a refused start must answer with.
/// Source: `smithy_query(query="operation", operation="StartCommandExecution")` —
/// `ServiceQuotaExceededException` httpError 402, `ThrottlingException` httpError
/// 429 (the operation's other errors are not admission refusals).
const ADMISSION_REFUSALS: [(u16, &str); 2] = [
    (402, "ServiceQuotaExceededException"),
    (429, "ThrottlingException"),
];

/// The values of the model's `JobExecutionStatus` enum that END an execution:
/// every value of the generated catalog that is not one of the two the service
/// documents as pending. Taken from the catalog rather than hand-listed, so a
/// status the model adds is covered.
fn terminal_statuses() -> Vec<&'static str> {
    JOB_EXECUTION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !["QUEUED", "IN_PROGRESS"].contains(status))
        .collect()
}

/// One emulator holding one shared engine, driven over HTTP.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // An execution starts only for a command that EXISTS; commands are the
        // control plane's resource, so the shared index is seeded with the one
        // command these bursts start.
        command_registry(&*engine)
            .expect("the shared IoT command index")
            .lock()
            .register(COMMAND_ARN)
            .expect("register the command these tests start");
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://data.jobs.iot.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGV4)
            .header("content-type", "application/json")
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One `StartCommandExecution` for `thing`, optionally carrying a
    /// `clientToken` and an `executionTimeoutSeconds`.
    async fn start(
        &self,
        thing: &str,
        client_token: Option<&str>,
        timeout_seconds: Option<i64>,
    ) -> (u16, Value) {
        let mut body = json!({
            "targetArn": format!("arn:aws:iot:us-east-1:111122223333:thing/{thing}"),
            "commandArn": COMMAND_ARN,
        });
        if let Some(token) = client_token {
            body["clientToken"] = Value::from(token);
        }
        if let Some(seconds) = timeout_seconds {
            body["executionTimeoutSeconds"] = Value::from(seconds);
        }
        self.call("POST", "/command-executions", Some(body)).await
    }

    async fn pending(&self, thing: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs"), None).await
    }

    async fn describe(&self, thing: &str, execution_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/things/{thing}/jobs/{execution_id}"), None)
            .await
    }

    async fn update(&self, thing: &str, execution_id: &str, status: &str) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/things/{thing}/jobs/{execution_id}"),
            Some(json!({"status": status})),
        )
        .await
    }

    /// Backdate one execution's own command-execution timer so the deadline it was
    /// created with has already passed: the same record the clock moves to
    /// `TIMED_OUT` once `executionTimeoutSeconds` elapses, without a test that
    /// sleeps for it.
    fn expire_execution_timer(&self, thing: &str, execution_id: &str) {
        let store = service_state(&*self.engine).expect("the shared job-execution store");
        let mut store = store.lock();
        let index = store
            .execution_index(thing, execution_id, None)
            .unwrap_or_else(|| panic!("the execution {execution_id} of {thing} must be stored"));
        store
            .record_mut(thing, index)
            .expect("the index came from this store")
            .execution_timeout_expires_at = Some(now_epoch_seconds() - 1);
    }
}

fn execution_id(body: &Value) -> &str {
    body["executionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an executionId, got {body}"))
}

/// The wire error code of a modeled failure (restJson1 carries it in `__type`).
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("")
}

/// The ids the device's own pending listing reports.
fn queued_ids(list: &Value) -> BTreeSet<String> {
    list["queuedJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of queued summaries, got {list}"))
        .iter()
        .map(|summary| summary["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Start commands on `thing` until the next one is refused, answering the ids that
/// were admitted. Every refusal is checked against the modelled admission set.
async fn fill_to_the_limit(emulator: &Emulator, case: &str, thing: &str) -> Vec<String> {
    let mut admitted = Vec::new();
    for attempt in 0..=LIMIT {
        let (status, body) = emulator.start(thing, None, None).await;
        if status == 200 {
            admitted.push(execution_id(&body).to_string());
            continue;
        }
        assert!(
            ADMISSION_REFUSALS.contains(&(status, error_code(&body))),
            "case {case:?} (call {attempt}): a refusal must be one of {ADMISSION_REFUSALS:?}, got {status} {body}"
        );
        break;
    }
    assert_eq!(
        admitted.len(),
        LIMIT,
        "case {case:?}: expected {LIMIT} admitted before the first refusal, got {}",
        admitted.len()
    );
    admitted
}

/// RULE: a device admits at most the published number of command executions that
/// have not reached a terminal status (`MAX_CONCURRENT_COMMAND_EXECUTIONS_PER_THING`);
/// every start past that is refused with one of the operation's modelled admission
/// errors and stores nothing, while the admitted executions stay individually
/// retrievable and the device's listing reports exactly them.
///
/// The rows differ in burst size relative to the limit (well past it, exactly at
/// it, well under it, one past it), in the form of the device name, and in whether
/// the burst asks for an execution timeout — a device's accounting is its own, so
/// a second device starts from an empty count.
#[tokio::test]
async fn a_device_admits_only_the_published_number_of_concurrent_command_executions() {
    let emulator = Emulator::new();

    // (case, device, burst size, executionTimeoutSeconds asked for)
    let rows: [(&str, &str, usize, Option<i64>); 5] = [
        ("a burst half again past the limit", "laser-burst", LIMIT + LIMIT / 2, Some(3_600)),
        ("a burst exactly at the limit", "laser_at-limit", LIMIT, None),
        ("a burst well under the limit", "plant:laser:7", LIMIT / 3, None),
        ("a burst one past the limit", "laser-one-past", LIMIT + 1, None),
        ("a single start", "laser-single", 1, Some(1)),
    ];

    for (case, thing, burst, timeout_seconds) in rows {
        let mut admitted: Vec<String> = Vec::new();
        let mut refused = 0usize;
        for index in 0..burst {
            let (status, body) = emulator.start(thing, None, timeout_seconds).await;
            if status == 200 {
                admitted.push(execution_id(&body).to_string());
                continue;
            }
            refused += 1;
            assert!(
                ADMISSION_REFUSALS.contains(&(status, error_code(&body))),
                "case {case:?} (call {index}): a refusal must be one of {ADMISSION_REFUSALS:?}, got {status} {body}"
            );
        }

        let expected_admitted = burst.min(LIMIT);
        assert_eq!(
            admitted.len(),
            expected_admitted,
            "case {case:?}: expected {expected_admitted} of {burst} starts admitted, got {}",
            admitted.len()
        );
        assert_eq!(
            refused,
            burst - expected_admitted,
            "case {case:?}: expected {} refusals, got {refused}",
            burst - expected_admitted
        );

        // Every admitted call minted its OWN execution.
        let distinct: BTreeSet<&String> = admitted.iter().collect();
        assert_eq!(
            distinct.len(),
            admitted.len(),
            "case {case:?}: expected one execution id per admitted call, got {} distinct of {}",
            distinct.len(),
            admitted.len()
        );

        // The admitted executions are the ones the device holds — no phantom and
        // no missing admission, so a refused start stored nothing.
        let (status, listed) = emulator.pending(thing).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the device's listing, got {status} {listed}"
        );
        assert_eq!(
            queued_ids(&listed),
            distinct.into_iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: the device must report exactly the admitted executions, got {listed}"
        );

        // Each admitted execution is individually retrievable (first, middle and
        // last of the burst).
        for position in [0, admitted.len() / 2, admitted.len() - 1] {
            let id = &admitted[position];
            let (status, described) = emulator.describe(thing, id).await;
            assert_eq!(
                status, 200,
                "case {case:?} (admitted {position}): expected the execution readable, got {status} {described}"
            );
            assert_eq!(
                described["execution"]["status"], "QUEUED",
                "case {case:?} (admitted {position}): expected a non-terminal execution, got {described}"
            );
        }
    }
}

/// RULE: a device's concurrency slot is held only while the execution in it has
/// NOT reached a terminal status. Finishing an execution — in any terminal status
/// of the model's enum, including the `TIMED_OUT` its own timer already made due —
/// releases exactly one slot, so exactly as many further starts are admitted as
/// executions were finished.
///
/// The rows are every terminal value of `JobExecutionStatus`, so the rule is
/// asserted over the whole class rather than over the one status a client happens
/// to report.
#[tokio::test]
async fn a_slot_is_released_by_every_terminal_status_and_by_the_execution_timer() {
    let emulator = Emulator::new();

    let thing = "laser-drain";
    let admitted = fill_to_the_limit(&emulator, "the initial fill", thing).await;

    let terminal = terminal_statuses();
    assert!(
        terminal.len() >= 2,
        "the model must declare more than one terminal status, got {terminal:?}"
    );

    for (index, status) in terminal.iter().enumerate() {
        // At the limit, a fresh start is refused.
        let (code, refusal) = emulator.start(thing, None, None).await;
        assert!(
            ADMISSION_REFUSALS.contains(&(code, error_code(&refusal))),
            "case {status:?}: expected a modelled admission refusal at the limit, got {code} {refusal}"
        );

        // Finish ONE execution in this terminal status: one slot is released.
        let finished = &admitted[index];
        let (code, body) = emulator.update(thing, finished, status).await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected the update accepted, got {code} {body}"
        );

        let (code, body) = emulator.start(thing, None, None).await;
        assert_eq!(
            code, 200,
            "case {status:?}: expected the released slot admitted, got {code} {body}"
        );
        let replacement = execution_id(&body).to_string();

        // And only ONE: the device is at the limit again.
        let (code, refusal) = emulator.start(thing, None, None).await;
        assert!(
            ADMISSION_REFUSALS.contains(&(code, error_code(&refusal))),
            "case {status:?}: exactly one slot was released, so the next start must be refused, got {code} {refusal}"
        );

        // The device's listing holds the replacement and not the finished one.
        let (_, listed) = emulator.pending(thing).await;
        let queued = queued_ids(&listed);
        assert!(
            queued.contains(&replacement) && !queued.contains(finished),
            "case {status:?}: expected the replacement queued and the finished execution gone, got {listed}"
        );
        assert_eq!(
            queued.len(),
            LIMIT,
            "case {status:?}: expected {LIMIT} outstanding executions, got {}",
            queued.len()
        );
    }

    // The timer the execution was created with releases its slot too: an execution
    // whose `executionTimeoutSeconds` has elapsed is TIMED_OUT, hence terminal,
    // hence no longer outstanding — and admission reads that at the instant it is
    // asked, without anyone having called in between.
    let (code, refusal) = emulator.start(thing, None, None).await;
    assert!(
        ADMISSION_REFUSALS.contains(&(code, error_code(&refusal))),
        "case \"before the timer elapses\": expected a modelled admission refusal, got {code} {refusal}"
    );
    let (_, listed) = emulator.pending(thing).await;
    let timing_out = queued_ids(&listed)
        .into_iter()
        .next()
        .expect("the device holds outstanding executions");
    emulator.expire_execution_timer(thing, &timing_out);

    let (code, body) = emulator.start(thing, None, None).await;
    assert_eq!(
        code, 200,
        "case \"after the timer elapses\": expected the timed-out execution's slot admitted, got {code} {body}"
    );
    let (_, listed) = emulator.pending(thing).await;
    assert!(
        !queued_ids(&listed).contains(&timing_out),
        "case \"after the timer elapses\": the timed-out execution must no longer be outstanding, got {listed}"
    );
}

/// RULE: `clientToken` idempotency is answered from the index, so a retry of an
/// already-admitted token completes with the execution the first call created even
/// while the device is at its concurrency limit — it creates no second execution
/// and consumes no slot. A retry under a FRESH token is a new request and is
/// refused like any other start past the limit.
#[tokio::test]
async fn a_replay_of_an_admitted_client_token_is_answered_at_the_limit() {
    let emulator = Emulator::new();

    let thing = "laser-idempotent";
    let token = "client-token-1";
    let (status, body) = emulator.start(thing, Some(token), None).await;
    assert_eq!(status, 200, "the first start must be admitted, got {status} {body}");
    let first = execution_id(&body).to_string();

    // Fill the rest of the device's slots, then confirm it is at the limit.
    for index in 1..LIMIT {
        let (status, body) = emulator.start(thing, None, None).await;
        assert_eq!(
            status, 200,
            "case \"filling the device\" (call {index}): expected 200 below the limit, got {status} {body}"
        );
    }
    let (status, refusal) = emulator.start(thing, None, None).await;
    assert!(
        ADMISSION_REFUSALS.contains(&(status, error_code(&refusal))),
        "case \"a fresh start at the limit\": expected a modelled admission refusal, got {status} {refusal}"
    );
    let (status, refusal) = emulator.start(thing, Some("client-token-2"), None).await;
    assert!(
        ADMISSION_REFUSALS.contains(&(status, error_code(&refusal))),
        "case \"a fresh token at the limit\": expected a modelled admission refusal, got {status} {refusal}"
    );

    // The replay of the admitted token is NOT refused and mints nothing.
    let (status, replay) = emulator.start(thing, Some(token), None).await;
    assert_eq!(
        status, 200,
        "case \"a replayed token at the limit\": expected 200, got {status} {replay}"
    );
    assert_eq!(
        execution_id(&replay),
        first,
        "case \"a replayed token at the limit\": expected the first execution id, got {replay}"
    );
    let (_, listed) = emulator.pending(thing).await;
    assert_eq!(
        queued_ids(&listed).len(),
        LIMIT,
        "case \"a replayed token at the limit\": the replay must create nothing, got {listed}"
    );
}

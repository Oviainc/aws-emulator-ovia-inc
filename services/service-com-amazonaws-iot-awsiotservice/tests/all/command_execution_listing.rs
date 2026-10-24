// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListCommandExecutions`: the control plane lists the
//! COMMAND EXECUTIONS held in the SHARED job-execution store
//! (`service_common::iot_job_executions`) the IoT Jobs data plane writes
//! (`StartCommandExecution`) and the device then moves.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `POST /command-executions` signed for the `iot` signing name — so what is
//! asserted is what an AWS SDK client would see, not an internal function's return
//! value. The commands the executions run are created through `PUT /commands/{id}`, so
//! the namespace a filter selects on is the one a writer persisted.
//!
//! The rules under test:
//!
//! * every entry is the stored record projected onto the model's
//!   `CommandExecutionSummary` — `executionId`, `commandArn`, `targetArn`, `status`,
//!   `createdAt`, `startedAt` (the device's own start once it has reported one, and
//!   otherwise the instant the start call started the execution) and `completedAt`
//!   (only once it finished) — and a stored `QUEUED` execution is reported with the
//!   status this model names for one created and not yet taken up, `CREATED`;
//! * the listing is ordered by the instant the request's time filter selects
//!   (`startedTimeFilter` -> `startedAt`, `completedTimeFilter` -> `completedAt`,
//!   neither -> `createdAt`), descending unless `sortOrder` asks otherwise, with the
//!   store's enqueue order breaking a tie between two executions of the same second;
//! * each filter (`commandArn`, `targetArn`, `namespace`, `status`, a time window)
//!   selects exactly the executions it names and nothing else, a command created
//!   without a namespace being in the base one;
//! * `maxResults` / `nextToken` walk that one order, every execution exactly once, and
//!   a token of another listing is refused;
//! * the request pairs the operation documents as mutually exclusive, and a time no
//!   calendar holds, are refused `ValidationException` (400);
//! * listing is a READ: the second identical call answers exactly what the first did.
//!
//! Each table spans what its rule generalises over — which device and which command an
//! execution belongs to, which namespace that command was created in, every status an
//! execution can be read in, whether it has started or completed, which bound of which
//! time filter is given and in which of the forms the filter's documented format
//! admits, and the page size — rather than one recorded request.

use std::sync::Arc;

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{
    now_epoch_seconds, JobExecutionRecord, JobExecutionStore, STATUS_IN_PROGRESS, STATUS_QUEUED,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and [`REGION`]: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The `executionId`s of `commandExecutions`, in the order the service returned
    /// them.
    fn execution_ids(&self) -> Vec<String> {
        self.body
            .get("commandExecutions")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .get("executionId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One returned summary, by its `executionId`.
    fn summary(&self, execution_id: &str) -> Option<&Value> {
        self.body
            .get("commandExecutions")?
            .as_array()?
            .iter()
            .find(|entry| entry.get("executionId").and_then(Value::as_str) == Some(execution_id))
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it. The engine handle is kept so a
/// case can seed the SHARED job-execution store — the store the IoT Jobs data plane's
/// `StartCommandExecution` writes into — with the records a device-facing plane would
/// have left there.
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_command(&self, command_id: &str, body: &Value) -> Reply {
        self.call("PUT", &format!("/commands/{command_id}"), Some(body))
            .await
    }

    /// `ListCommandExecutions` with `request` as its body and `query` appended to the
    /// URI (where `maxResults` and `nextToken` are bound).
    async fn list_with_query(&self, request: &Value, query: &str) -> Reply {
        self.call("POST", &format!("/command-executions{query}"), Some(request))
            .await
    }

    async fn list(&self, request: &Value) -> Reply {
        self.list_with_query(request, "").await
    }

    /// Put one execution in the SHARED store, exactly as the data plane's
    /// `StartCommandExecution` does (a `QUEUED` record keyed by the minted
    /// `executionId`, carrying the `commandArn` that makes it a COMMAND execution),
    /// then move it to the state the row asks for — a status only the device-facing
    /// plane reaches, a start instant, an armed execution timeout.
    fn seed(&self, seed: &Seed, base: i64) {
        let mut record = JobExecutionRecord::queued(
            seed.execution_id.to_string(),
            seed.thing.to_string(),
            Some(r#"{"commandArn":"seeded","parameters":{}}"#.to_string()),
            base + seed.created_after_base,
        );
        record.command_arn = seed
            .command_id
            .map(|command_id| command_arn(command_id).to_string());
        record.status = seed.stored_status.to_string();
        record.started_at = seed.started_after_base.map(|offset| base + offset);
        record.last_updated_at = base + seed.last_updated_after_base;
        record.execution_timeout_expires_at =
            seed.timeout_after_base.map(|offset| base + offset);

        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        store
            .try_insert_execution(record)
            .unwrap_or_else(|refusal| panic!("seeding {}: {refusal}", seed.execution_id));
    }

    /// The status the SHARED store holds for an execution — what a device polling the
    /// Jobs data plane would see.
    fn stored_status(&self, thing: &str, execution_id: &str) -> String {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .resolve_record(thing, execution_id, None)
            .map(|record| record.status.clone())
            .unwrap_or_default()
    }
}

/// The ARN of a command of this account in [`REGION`].
fn command_arn(command_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:command/{command_id}")
}

/// The ARN of a thing of this account in [`REGION`] — the `targetArn` of an execution
/// running on it.
fn target_arn(thing: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing}")
}

/// An instant as the `yyyy-MM-dd'T'HH:mm:ss'Z'` form.
fn zulu(instant: i64) -> String {
    DateTime::from_secs(instant)
        .fmt(Format::DateTime)
        .expect("a seeded instant renders")
}

/// An instant as the MINUTE-precision form `TimeFilter`'s members document ("the
/// format yyyy-MM-dd'T'HH:mm"). Every instant a case builds one of these from is
/// minute-aligned, so no second is lost.
fn minute_form(instant: i64) -> String {
    zulu(instant)[..16].to_string()
}

/// An instant as a civil date-time carrying the `+02:00` UTC offset that names it: the
/// same instant, written the way a client whose clock is not UTC sends it.
fn offset_form(instant: i64) -> String {
    format!("{}+02:00", &zulu(instant + 2 * 3_600)[..19])
}

/// One seeded execution.
struct Seed {
    /// The id the data plane minted, and the record's key.
    execution_id: &'static str,
    thing: &'static str,
    /// The command the execution runs, or `None` for a JOB execution — a record the
    /// same store holds that this operation must not report.
    command_id: Option<&'static str>,
    /// `JobExecutionStatus` as the shared store holds it.
    stored_status: &'static str,
    created_after_base: i64,
    started_after_base: Option<i64>,
    last_updated_after_base: i64,
    /// An armed command-execution timeout, whose deadline the clock has already
    /// passed: the listing must report the `TIMED_OUT` the deadline decided.
    timeout_after_base: Option<i64>,
}

/// The command the `AWS-IoT` rows run.
const REBOOT: &str = "listing-reboot";
/// The command the `AWS-IoT-FleetWise` rows run.
const SIGNALS: &str = "listing-collect-signals";

/// The seeded executions, in the order the store accepted them — which is the enqueue
/// order that breaks a tie between two executions created in the same second.
///
/// The rows span which device and which command an execution belongs to, which
/// namespace that command was created in, every status an execution can be read in
/// (including the one the CLOCK decides and one this model cannot name), whether it
/// has started, and whether it has completed.
const SEEDS: &[Seed] = &[
    Seed {
        execution_id: "exec-created",
        thing: "lister-a",
        command_id: Some(REBOOT),
        stored_status: STATUS_QUEUED,
        created_after_base: 0,
        started_after_base: None,
        last_updated_after_base: 0,
        timeout_after_base: None,
    },
    Seed {
        // Created in the same SECOND as the row above, on another device: only the
        // store's enqueue order can tell the two apart.
        execution_id: "exec-in-progress",
        thing: "lister-b",
        command_id: Some(REBOOT),
        stored_status: STATUS_IN_PROGRESS,
        created_after_base: 0,
        started_after_base: Some(60),
        last_updated_after_base: 60,
        timeout_after_base: None,
    },
    Seed {
        execution_id: "exec-succeeded",
        thing: "lister-a",
        command_id: Some(SIGNALS),
        stored_status: "SUCCEEDED",
        created_after_base: 60,
        started_after_base: Some(120),
        last_updated_after_base: 180,
        timeout_after_base: None,
    },
    Seed {
        execution_id: "exec-failed",
        thing: "lister-b",
        command_id: Some(SIGNALS),
        stored_status: "FAILED",
        created_after_base: 120,
        started_after_base: Some(180),
        last_updated_after_base: 240,
        timeout_after_base: None,
    },
    Seed {
        // Rejected work is never started, and completed when it was rejected — at the
        // same instant as `exec-succeeded`, so the completion order has a tie too.
        execution_id: "exec-rejected",
        thing: "lister-a",
        command_id: Some(REBOOT),
        stored_status: "REJECTED",
        created_after_base: 180,
        started_after_base: None,
        last_updated_after_base: 180,
        timeout_after_base: None,
    },
    Seed {
        // Still `QUEUED` in the store with a deadline the clock has passed: the
        // listing must report the `TIMED_OUT` that deadline already decided, and
        // report the DEADLINE as when it completed.
        execution_id: "exec-timed-out",
        thing: "lister-b",
        command_id: Some(REBOOT),
        stored_status: STATUS_QUEUED,
        created_after_base: 240,
        started_after_base: None,
        last_updated_after_base: 240,
        timeout_after_base: Some(300),
    },
    Seed {
        // A stored status `CommandExecutionStatus` does not declare: there is no
        // command-execution status to report it as, so it is not reported.
        execution_id: "exec-canceled",
        thing: "lister-a",
        command_id: Some(REBOOT),
        stored_status: "CANCELED",
        created_after_base: 300,
        started_after_base: None,
        last_updated_after_base: 300,
        timeout_after_base: None,
    },
    Seed {
        // A JOB execution: the same store holds it, and this operation lists command
        // executions only.
        execution_id: "listing-job",
        thing: "lister-a",
        command_id: None,
        stored_status: STATUS_QUEUED,
        created_after_base: 360,
        started_after_base: None,
        last_updated_after_base: 360,
        timeout_after_base: None,
    },
];

/// Every command execution the seeds leave listable, newest-created first — the order
/// a request that names no filter and no `sortOrder` must answer with.
const NEWEST_FIRST: &[&str] = &[
    "exec-timed-out",
    "exec-rejected",
    "exec-failed",
    "exec-succeeded",
    "exec-in-progress",
    "exec-created",
];

/// An emulator holding the two commands and every seeded execution, plus the base
/// instant the seeds are offset from: two hours in the past, minute-aligned, so every
/// seeded instant is behind the clock (the armed deadline included) and every bound a
/// case writes in minute precision names the instant it means.
async fn seeded() -> (Emulator, i64) {
    let emulator = Emulator::new();
    let base = {
        let now = now_epoch_seconds() - 2 * 3_600;
        now - now.rem_euclid(60)
    };

    let created = emulator
        .create_command(REBOOT, &json!({ "namespace": "AWS-IoT" }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating {REBOOT} must answer 200, got {} with {:?}",
        created.status, created.body
    );
    let created = emulator
        .create_command(
            SIGNALS,
            &json!({
                "namespace": "AWS-IoT-FleetWise",
                "roleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/fleetwise-payload"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating {SIGNALS} must answer 200, got {} with {:?}",
        created.status, created.body
    );

    for seed in SEEDS {
        emulator.seed(seed, base);
    }
    (emulator, base)
}

/// RULE: every listed entry is the STORED record projected onto
/// `CommandExecutionSummary`, and a request that names no filter lists every command
/// execution the store holds, newest-created first.
#[tokio::test]
async fn every_entry_is_the_stored_execution_projected_newest_first() {
    let (emulator, base) = seeded().await;

    let listed = emulator.list(&json!({})).await;
    assert_eq!(
        listed.status, 200,
        "listing must answer 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.execution_ids(),
        NEWEST_FIRST,
        "the default listing must be every command execution newest-created first, \
         got {:?}",
        listed.body
    );

    // (case, executionId, command, device, reported status, createdAt, startedAt,
    //  completedAt) — one row per listable seed, so the projection is asserted for an
    //  execution that has not started, one in progress, one that succeeded, one that
    //  failed, one that was rejected without starting and one the clock timed out.
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        i64,
        Option<i64>,
        Option<i64>,
    );
    let rows: [Row; 6] = [
        (
            "created, never taken up",
            "exec-created",
            REBOOT,
            "lister-a",
            "CREATED",
            0,
            Some(0),
            None,
        ),
        (
            "taken up by its device",
            "exec-in-progress",
            REBOOT,
            "lister-b",
            "IN_PROGRESS",
            0,
            Some(60),
            None,
        ),
        (
            "succeeded",
            "exec-succeeded",
            SIGNALS,
            "lister-a",
            "SUCCEEDED",
            60,
            Some(120),
            Some(180),
        ),
        (
            "failed",
            "exec-failed",
            SIGNALS,
            "lister-b",
            "FAILED",
            120,
            Some(180),
            Some(240),
        ),
        (
            "rejected without being taken up",
            "exec-rejected",
            REBOOT,
            "lister-a",
            "REJECTED",
            180,
            Some(180),
            Some(180),
        ),
        (
            // The transition the CLOCK owed at the deadline, reported as of the
            // deadline and not of this read.
            "timed out by its own deadline",
            "exec-timed-out",
            REBOOT,
            "lister-b",
            "TIMED_OUT",
            240,
            Some(240),
            Some(300),
        ),
    ];

    for (case, execution_id, command, device, status, created, started, completed) in rows {
        let summary = listed
            .summary(execution_id)
            .unwrap_or_else(|| panic!("case {case:?}: {execution_id} must be listed"));
        assert_eq!(
            (
                summary.get("commandArn").and_then(Value::as_str),
                summary.get("targetArn").and_then(Value::as_str),
                summary.get("status").and_then(Value::as_str),
            ),
            (
                Some(command_arn(command).as_str()),
                Some(target_arn(device).as_str()),
                Some(status),
            ),
            "case {case:?}: expected {command}/{device}/{status}, got {summary:?}"
        );
        assert_eq!(
            summary.get("createdAt").and_then(Value::as_i64),
            Some(base + created),
            "case {case:?}: expected createdAt {}, got {summary:?}",
            base + created
        );
        assert_eq!(
            summary.get("startedAt").and_then(Value::as_i64),
            started.map(|offset| base + offset),
            "case {case:?}: expected startedAt {:?}, got {summary:?}",
            started.map(|offset| base + offset)
        );
        assert_eq!(
            summary.get("completedAt").and_then(Value::as_i64),
            completed.map(|offset| base + offset),
            "case {case:?}: expected completedAt {:?}, got {summary:?}",
            completed.map(|offset| base + offset)
        );
    }

    // Neither the record this model cannot name nor the JOB execution is a command
    // execution of this account.
    for hidden in ["exec-canceled", "listing-job"] {
        assert!(
            listed.summary(hidden).is_none(),
            "case {hidden:?}: must not be listed as a command execution, got {:?}",
            listed.body
        );
    }

    // A READ: the only write it may make is the transition the clock already owed, so
    // the second identical call answers exactly what the first did.
    let again = emulator.list(&json!({})).await;
    assert_eq!(
        again.body, listed.body,
        "listing twice must answer the same page, got {:?} then {:?}",
        listed.body, again.body
    );
    assert_eq!(
        emulator.stored_status("lister-b", "exec-timed-out"),
        "TIMED_OUT",
        "the materialised timeout must be the status a later reader of the shared \
         store sees, got {:?}",
        emulator.stored_status("lister-b", "exec-timed-out")
    );
    assert_eq!(
        emulator.stored_status("lister-a", "exec-created"),
        STATUS_QUEUED,
        "listing must not move an execution with no deadline, got {:?}",
        emulator.stored_status("lister-a", "exec-created")
    );
}

/// RULE: each filter selects exactly the executions it names, in the order the
/// selected instant and `sortOrder` decide.
#[tokio::test]
async fn each_filter_selects_the_executions_it_names() {
    let (emulator, base) = seeded().await;

    // (case, request, the executionIds expected in order). The rows differ along every
    // member the selection is built from: which command, which device, which namespace,
    // which status, which time filter and bound, and which sort order.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter at all", json!({}), NEWEST_FIRST.to_vec()),
        (
            "ascending is the reverse of the default order",
            json!({ "sortOrder": "ASCENDING" }),
            NEWEST_FIRST.iter().rev().copied().collect(),
        ),
        (
            "descending named explicitly is the default",
            json!({ "sortOrder": "DESCENDING" }),
            NEWEST_FIRST.to_vec(),
        ),
        (
            "one command",
            json!({ "commandArn": command_arn(REBOOT) }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-in-progress",
                "exec-created",
            ],
        ),
        (
            "the other command",
            json!({ "commandArn": command_arn(SIGNALS) }),
            vec!["exec-failed", "exec-succeeded"],
        ),
        (
            "a command nobody created",
            json!({ "commandArn": command_arn("listing-absent") }),
            vec![],
        ),
        (
            "one device",
            json!({ "targetArn": target_arn("lister-a") }),
            vec!["exec-rejected", "exec-succeeded", "exec-created"],
        ),
        (
            "the other device",
            json!({ "targetArn": target_arn("lister-b") }),
            vec!["exec-timed-out", "exec-failed", "exec-in-progress"],
        ),
        (
            "a device with no execution",
            json!({ "targetArn": target_arn("lister-z") }),
            vec![],
        ),
        (
            "the same device in another region",
            json!({ "targetArn": format!("arn:aws:iot:eu-west-1:{ACCOUNT_ID}:thing/lister-a") }),
            vec![],
        ),
        (
            "the AWS-IoT namespace",
            json!({ "namespace": "AWS-IoT" }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-in-progress",
                "exec-created",
            ],
        ),
        (
            "the AWS-IoT-FleetWise namespace",
            json!({ "namespace": "AWS-IoT-FleetWise" }),
            vec!["exec-failed", "exec-succeeded"],
        ),
        (
            "status CREATED",
            json!({ "status": "CREATED" }),
            vec!["exec-created"],
        ),
        (
            "status IN_PROGRESS",
            json!({ "status": "IN_PROGRESS" }),
            vec!["exec-in-progress"],
        ),
        (
            "status SUCCEEDED",
            json!({ "status": "SUCCEEDED" }),
            vec!["exec-succeeded"],
        ),
        (
            "status FAILED",
            json!({ "status": "FAILED" }),
            vec!["exec-failed"],
        ),
        (
            "status REJECTED",
            json!({ "status": "REJECTED" }),
            vec!["exec-rejected"],
        ),
        (
            "status TIMED_OUT",
            json!({ "status": "TIMED_OUT" }),
            vec!["exec-timed-out"],
        ),
        (
            "one device and one status together",
            json!({ "targetArn": target_arn("lister-a"), "status": "SUCCEEDED" }),
            vec!["exec-succeeded"],
        ),
        (
            // Ordered by `startedAt`, which every execution has: the device's own
            // start where it reported one, and the start call's instant otherwise.
            "a started-time filter with no bound",
            json!({ "startedTimeFilter": {} }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-failed",
                "exec-succeeded",
                "exec-in-progress",
                "exec-created",
            ],
        ),
        (
            "started after an instant, in the documented minute form",
            json!({ "startedTimeFilter": { "after": minute_form(base + 120) } }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-failed",
                "exec-succeeded",
            ],
        ),
        (
            "started after the same instant, written with seconds and a Z",
            json!({ "startedTimeFilter": { "after": zulu(base + 120) } }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-failed",
                "exec-succeeded",
            ],
        ),
        (
            "started after the same instant, written with a +02:00 offset",
            json!({ "startedTimeFilter": { "after": offset_form(base + 120) } }),
            vec![
                "exec-timed-out",
                "exec-rejected",
                "exec-failed",
                "exec-succeeded",
            ],
        ),
        (
            "started before an instant",
            json!({ "startedTimeFilter": { "before": minute_form(base + 120) } }),
            vec!["exec-succeeded", "exec-in-progress", "exec-created"],
        ),
        (
            "started inside a window",
            json!({
                "startedTimeFilter": {
                    "after": minute_form(base + 60),
                    "before": minute_form(base + 120),
                },
            }),
            vec!["exec-succeeded", "exec-in-progress"],
        ),
        (
            "started in a window nothing falls in",
            json!({
                "startedTimeFilter": {
                    "after": minute_form(base + 1_200),
                    "before": minute_form(base + 1_800),
                },
            }),
            vec![],
        ),
        (
            // Ordered by `completedAt`, so only the FINISHED executions are in it, and
            // the two that completed in the same second are ordered by enqueue order.
            "a completed-time filter with no bound",
            json!({ "completedTimeFilter": {} }),
            vec![
                "exec-timed-out",
                "exec-failed",
                "exec-rejected",
                "exec-succeeded",
            ],
        ),
        (
            "completed after an instant",
            json!({ "completedTimeFilter": { "after": minute_form(base + 240) } }),
            vec!["exec-timed-out", "exec-failed"],
        ),
        (
            "completed before an instant, ascending",
            json!({
                "completedTimeFilter": { "before": minute_form(base + 240) },
                "sortOrder": "ASCENDING",
            }),
            vec!["exec-succeeded", "exec-rejected", "exec-failed"],
        ),
        (
            "a completed-time filter and a status together",
            json!({
                "completedTimeFilter": { "after": minute_form(base + 180) },
                "status": "REJECTED",
            }),
            vec!["exec-rejected"],
        ),
    ];

    for (case, request, expected) in cases {
        let listed = emulator.list(&request).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.execution_ids(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: a page holding every match carries no nextToken, got {:?}",
            listed.body
        );
    }
}

/// RULE: `maxResults` and `nextToken` walk ONE order — every execution of the
/// selection exactly once, in the order an unpaged listing reports it — and a token
/// belongs to the listing that issued it.
///
/// A command created WITHOUT a namespace is in the base one, so the executions of a
/// bare command are the ones an `AWS-IoT` filter selects and the ones an
/// `AWS-IoT-FleetWise` filter does not: `CreateCommand$roleArn` is required with the
/// FleetWise namespace and refused with the other, so a command carrying neither a
/// namespace nor a role is one only the base namespace admits.
#[tokio::test]
async fn a_command_created_without_a_namespace_is_in_the_base_namespace() {
    let emulator = Emulator::new();
    let base = {
        let now = now_epoch_seconds() - 2 * 3_600;
        now - now.rem_euclid(60)
    };

    // (case, command id, the create request, the execution seeded for it, the
    // namespace filter that must select that execution, the one that must not)
    let rows: [(&str, &'static str, Value, &'static str, &str, &str); 2] = [
        (
            "no namespace at all",
            "listing-bare",
            json!({}),
            "exec-bare",
            "AWS-IoT",
            "AWS-IoT-FleetWise",
        ),
        (
            "the namespace named explicitly",
            "listing-named",
            json!({ "namespace": "AWS-IoT" }),
            "exec-named",
            "AWS-IoT",
            "AWS-IoT-FleetWise",
        ),
    ];

    for (case, command_id, request, execution_id, selects, excludes) in rows {
        let created = emulator.create_command(command_id, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating {command_id} must answer 200, got {} with {:?}",
            created.status, created.body
        );
        // The same seeding `StartCommandExecution` performs: a `QUEUED` record keyed
        // by the execution id and carrying the command's ARN.
        emulator.seed(
            &Seed {
                execution_id,
                thing: "lister-bare",
                command_id: Some(command_id),
                stored_status: STATUS_QUEUED,
                created_after_base: 0,
                started_after_base: None,
                last_updated_after_base: 0,
                timeout_after_base: None,
            },
            base,
        );

        let selected = emulator.list(&json!({ "namespace": selects })).await;
        assert!(
            selected.summary(execution_id).is_some(),
            "case {case:?}: the {selects} namespace must select {execution_id}, got {:?}",
            selected.body
        );
        let excluded = emulator.list(&json!({ "namespace": excludes })).await;
        assert!(
            excluded.summary(execution_id).is_none(),
            "case {case:?}: the {excludes} namespace must not select {execution_id}, \
             got {:?}",
            excluded.body
        );
    }
}

/// RULE: `maxResults` and `nextToken` walk ONE order — every execution of the
/// selection exactly once, in the order an unpaged listing reports it — and a token
/// belongs to the listing that issued it.
#[tokio::test]
async fn pagination_walks_the_selection_exactly_once() {
    let (emulator, _base) = seeded().await;

    // The rows differ in page size: one entry per page, a size that divides the
    // selection, one that does not, the whole selection in one page and a page larger
    // than the selection.
    for page_size in [1usize, 2, 4, 6, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let listed = emulator.list_with_query(&json!({}), &query).await;
            assert_eq!(
                listed.status, 200,
                "case page size {page_size}: expected 200, got {} with {:?}",
                listed.status, listed.body
            );
            let ids = listed.execution_ids();
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page holds at most {page_size}, got {}",
                ids.len()
            );
            walked.extend(ids);
            pages += 1;
            token = listed.next_token();
            if token.is_none() {
                break;
            }
            assert!(
                pages <= NEWEST_FIRST.len(),
                "case page size {page_size}: the walk must end, got {pages} pages so far \
                 with {walked:?}"
            );
        }
        assert_eq!(
            walked, NEWEST_FIRST,
            "case page size {page_size}: the walk must report every execution exactly \
             once in the unpaged order, got {walked:?}"
        );
        let expected_pages = NEWEST_FIRST.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page size {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A token is a position in ONE listing: the token an ascending listing issued
    // names no position of the descending one, and is refused rather than resuming it.
    let ascending = emulator
        .list_with_query(&json!({ "sortOrder": "ASCENDING" }), "?maxResults=2")
        .await;
    let token = ascending
        .next_token()
        .expect("a truncated ascending listing carries a token");
    let reused = emulator
        .list_with_query(&json!({}), &format!("?maxResults=2&nextToken={token}"))
        .await;
    assert_eq!(
        (reused.status, reused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "reusing another listing's token must be refused, got {} {:?} with {:?}",
        reused.status,
        reused.error_code,
        reused.body
    );
}

/// RULE: a request the operation does not admit is refused `ValidationException`
/// (HTTP 400) and lists nothing.
#[tokio::test]
async fn a_request_the_operation_does_not_admit_is_refused() {
    let (emulator, base) = seeded().await;

    // The rows cover both pairs the operation documents as mutually exclusive, every
    // shape of time bound no calendar holds, and a continuation token this listing
    // never issued.
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "both commandArn and targetArn",
            json!({
                "commandArn": command_arn(REBOOT),
                "targetArn": target_arn("lister-a"),
            }),
            String::new(),
        ),
        (
            "both time filters",
            json!({
                "startedTimeFilter": { "after": minute_form(base) },
                "completedTimeFilter": { "after": minute_form(base) },
            }),
            String::new(),
        ),
        (
            "a time bound that is not a date at all",
            json!({ "startedTimeFilter": { "after": "yesterday" } }),
            String::new(),
        ),
        (
            "a time bound with no time part",
            json!({ "startedTimeFilter": { "before": "2026-01-01" } }),
            String::new(),
        ),
        (
            "a time bound as epoch seconds",
            json!({ "completedTimeFilter": { "after": "1767225600" } }),
            String::new(),
        ),
        (
            "a time bound naming a day no month has",
            json!({ "completedTimeFilter": { "before": "2026-02-30T00:00" } }),
            String::new(),
        ),
        (
            "a nextToken that is not a token of any listing",
            json!({}),
            "?nextToken=not-a-token".to_string(),
        ),
    ];

    for (case, request, query) in cases {
        let refused = emulator.list_with_query(&request, &query).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
        assert!(
            refused.body.get("commandExecutions").is_none(),
            "case {case:?}: a refused request lists nothing, got {:?}",
            refused.body
        );
    }
}

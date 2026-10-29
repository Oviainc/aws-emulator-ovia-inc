// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the handle a durable `Invoke` answers and the stop that handle
//! addresses: "Stops a running durable execution. The execution transitions to STOPPED
//! status and cannot be resumed. Any in-progress operations are terminated."
//!
//! The rule under test: the `DurableExecutionArn` an accepted invocation of a durable
//! function answers IS the execution's handle, and it resolves to the same stored
//! execution in every operation modelled on it. `StopDurableExecution` on that handle ends
//! the execution at the instant of the call — status `STOPPED`, `EndTimestamp` and the
//! answered `StopTimestamp` both that instant, the `Error` the call gave kept as the reason
//! it ended, and an `ExecutionStopped` event appended to its history — and every read of
//! the execution reports that one transition. An execution that is already terminal is not
//! moved again and answers the instant it really ended at, so a repeat performs nothing a
//! second time. A handle naming no execution this emulator holds is refused.
//!
//! Every assertion is over real HTTP requests through the router — the invocation that
//! mints the handle, the stop, and the reads that check it — so a transition that was never
//! persisted fails here. The rows differ along what the rule generalises over: the error
//! object the stop carries (none, each member on its own, a full one with a multi-entry
//! stack trace), the qualifier the execution runs under ($LATEST, a published version, an
//! alias), the execution's name, the payload it started from, and whether the function's
//! `DurableConfig` sets an `ExecutionTimeout` at all.
//!
//! The clock-driven half (an execution whose `ExecutionTimeout` has already fallen due) is
//! exercised by moving the stored due instant into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, send, zip_file,
    Answer, ACCOUNT, REGION, ROLE,
};

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// Where `ListDurableExecutionsByFunction` is bound
/// (`GET /2025-12-01/functions/{FunctionName}/durable-executions`).
const DURABLE_EXECUTIONS_URI: &str = "/2025-12-01/functions";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The request header `Invoke$DurableExecutionName` is bound to.
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which the
/// durable functions here name in their `DurableConfig`.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// The `ExecutionStatus` values this suite asserts on, from
/// `smithy_query(query="shape", shape="ExecutionStatus")` (`RUNNING | SUCCEEDED | FAILED |
/// TIMED_OUT | STOPPED`).
const STATUS_RUNNING: &str = "RUNNING";
const STATUS_STOPPED: &str = "STOPPED";
const STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// The model's `EventType` values an execution stopped here records, from
/// `smithy_query(query="shape", shape="EventType")`.
const EVENT_EXECUTION_STARTED: &str = "ExecutionStarted";
const EVENT_EXECUTION_STOPPED: &str = "ExecutionStopped";
const EVENT_EXECUTION_TIMED_OUT: &str = "ExecutionTimedOut";

/// The unpublished version label, as the model's `Version` pattern spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The refusal sentence a `DurableExecutionArn` naming no execution gets, as this service
/// words every resource-shaped refusal.
fn not_found_sentence(arn: &str) -> String {
    format!("Durable execution not found: {arn}")
}

/// `Invoke` on `reference` (with `query` appended), naming `execution` as the durable
/// execution and carrying `payload`.
async fn invoke(
    router: &MultiServiceRouter,
    reference: &str,
    query: &str,
    execution: &str,
    payload: &[u8],
) -> Answer {
    send(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{reference}/invocations{query}"),
        "application/octet-stream",
        payload.to_vec(),
        &[
            ("x-amz-invocation-type", "Event".to_string()),
            (DURABLE_EXECUTION_NAME_HEADER, execution.to_string()),
        ],
    )
    .await
}

/// `StopDurableExecution` on the execution `arn` addresses, carrying `error` as the reason
/// (`POST /2025-12-01/durable-executions/{DurableExecutionArn}/stop`). The label is not
/// greedy, so the slashes the ARN carries are percent-encoded in the path, exactly as an
/// SDK encodes them. `Error` is this operation's `@httpPayload`, so it IS the request body.
async fn stop(router: &MultiServiceRouter, arn: &str, error: &Value) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/2025-12-01/durable-executions/{}/stop",
            arn.replace('/', "%2F")
        ),
        error,
    )
    .await
}

/// `GetDurableExecution` on the same handle, with `query` appended.
async fn get_execution(router: &MultiServiceRouter, arn: &str, query: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}{query}",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// `GetDurableExecutionHistory` on the same handle.
async fn get_history(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}/history",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// `ListDurableExecutionsByFunction` on the function `reference` names.
async fn list(router: &MultiServiceRouter, reference: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("{DURABLE_EXECUTIONS_URI}/{reference}/durable-executions"),
        &Value::Null,
    )
    .await
}

/// The events an answer reported, in the order it reported them.
fn events(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one execution a listing reported, or `Value::Null` when it reported none.
fn listed_one(answer: &Answer) -> Value {
    answer
        .json()
        .get("DurableExecutions")
        .and_then(Value::as_array)
        .and_then(|listed| listed.first().cloned())
        .unwrap_or(Value::Null)
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create the function `name`, publishing a version and an alias so every qualifier form
/// resolves, give it the `DurableConfig` that makes it a durable function (with
/// `timeout` as its `ExecutionTimeout` when it sets one), and bring it to `Active`.
async fn create_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    timeout: Option<i64>,
) {
    let mut durable = json!({ "KMSKeyArn": KMS_KEY });
    if let Some(seconds) = timeout {
        durable["ExecutionTimeout"] = json!(seconds);
    }
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "DurableConfig": durable,
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let alias = json_call(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{name}/aliases"),
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "case {name}: CreateAlias must answer 201, got {} {}",
        alias.status,
        String::from_utf8_lossy(&alias.body)
    );
    make_activation_due(engine);
}

/// One row of the stop table: how the function is configured and addressed, the execution
/// the invocation starts, and the `Error` the stop carries.
struct StopCase {
    case: &'static str,
    timeout: Option<i64>,
    query: &'static str,
    version: &'static str,
    execution: &'static str,
    payload: &'static [u8],
    /// The `ErrorObject` the stop sends as its payload. `Error` is this operation's
    /// `@httpPayload`, so it IS the request body.
    error: fn() -> Value,
}

/// The rows: a stop whose error object names nothing, one naming each `ErrorObject` member
/// on its own, and one naming a full object with a multi-entry stack trace — against an
/// execution at the unpublished version, at a published version, through an alias, and on a
/// durable function whose configuration sets no `ExecutionTimeout`.
fn stop_cases() -> Vec<StopCase> {
    vec![
        StopCase {
            case: "an error object naming nothing, at the unpublished version",
            timeout: Some(3_600),
            query: "",
            version: VERSION_LATEST,
            execution: "stopped-without-a-reason",
            payload: b"",
            error: || json!({}),
        },
        StopCase {
            case: "an error type alone, at a published version",
            timeout: Some(3_600),
            query: "?Qualifier=1",
            version: "1",
            execution: "stopped-by-type",
            payload: br#"{"wait_seconds":5}"#,
            error: || json!({ "ErrorType": "OperatorStop" }),
        },
        StopCase {
            case: "an error message alone, through an alias",
            timeout: Some(3_600),
            query: "?Qualifier=live",
            version: "1",
            execution: "stopped_by-message9",
            payload: b"[]",
            error: || json!({ "ErrorMessage": "stopped by the operator" }),
        },
        StopCase {
            case: "a full error object with a multi-entry stack trace",
            timeout: Some(1),
            query: "",
            version: VERSION_LATEST,
            execution: "stopped-with-everything",
            payload: br#"{"key":"value"}"#,
            error: || {
                json!({
                    "ErrorType": "States.Timeout",
                    "ErrorMessage": "the operator stopped this execution",
                    "ErrorData": "{\"attempt\":3}",
                    "StackTrace": ["index.handler line 1", "index.step line 7"],
                })
            },
        },
        StopCase {
            case: "a durable function whose configuration sets no ExecutionTimeout",
            timeout: None,
            query: "",
            version: VERSION_LATEST,
            execution: "stopped-without-a-timeout",
            payload: br#"{"wait_seconds":1}"#,
            error: || json!({ "ErrorType": "OperatorStop" }),
        },
    ]
}

#[tokio::test]
async fn a_stop_ends_the_execution_its_handle_addresses() {
    for case in stop_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "durable-stop";
        create_durable_function(&router, &engine, function, case.timeout).await;

        // The handle: the ARN the accepted invocation answers is the only thing the caller
        // has to address the execution with.
        let started = invoke(
            &router,
            function,
            case.query,
            case.execution,
            case.payload,
        )
        .await;
        assert_eq!(
            started.status,
            202,
            "case {:?}: the invocation must be accepted, got {} {}",
            case.case,
            started.status,
            String::from_utf8_lossy(&started.body)
        );
        let arn = started
            .header(DURABLE_EXECUTION_ARN_HEADER)
            .unwrap_or_default();
        let (prefix, id) = arn.rsplit_once('/').unwrap_or_default();
        assert_eq!(
            (
                prefix.to_string(),
                id.is_empty(),
                id.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || byte == b'-'
                    || byte == b'_'),
            ),
            (
                format!(
                    "{}:{}/durable-execution/{}",
                    function_arn(function),
                    case.version,
                    case.execution
                ),
                false,
                true,
            ),
            "case {:?}: the invocation must answer the execution's own ARN, got {arn:?}",
            case.case
        );
        let running = get_execution(&router, &arn, "").await;
        assert_eq!(
            (running.status, running.string("Status")),
            (200, Some(STATUS_RUNNING.to_string())),
            "case {:?}: the started execution must read back {STATUS_RUNNING}, got {} {}",
            case.case,
            running.status,
            String::from_utf8_lossy(&running.body)
        );
        assert_eq!(
            running.json()["EndTimestamp"],
            Value::Null,
            "case {:?}: a running execution has not ended, got {}",
            case.case,
            String::from_utf8_lossy(&running.body)
        );

        let error = (case.error)();
        let stopped = stop(&router, &arn, &error).await;
        assert_eq!(
            stopped.status,
            200,
            "case {:?}: the stop must answer 200, got {} {}",
            case.case,
            stopped.status,
            String::from_utf8_lossy(&stopped.body)
        );
        let stop_timestamp = stopped.json()["StopTimestamp"].clone();
        assert!(
            stop_timestamp.is_number(),
            "case {:?}: the stop must answer the instant it stopped the execution, got {}",
            case.case,
            String::from_utf8_lossy(&stopped.body)
        );

        // Every read of the execution reports that one transition, and the instant it
        // reports is the instant the stop answered.
        let read = get_execution(&router, &arn, "").await;
        assert_eq!(
            (read.string("Status"), read.json()["EndTimestamp"].clone()),
            (Some(STATUS_STOPPED.to_string()), stop_timestamp.clone()),
            "case {:?}: the read must report {STATUS_STOPPED} ending at the answered \
             StopTimestamp {stop_timestamp}, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json()["Error"],
            error,
            "case {:?}: the read must report the reason the stop gave, got {}",
            case.case,
            String::from_utf8_lossy(&read.body)
        );
        let listed = listed_one(&list(&router, function).await);
        assert_eq!(
            (
                listed.get("Status").cloned().unwrap_or(Value::Null),
                listed.get("EndTimestamp").cloned().unwrap_or(Value::Null),
            ),
            (json!(STATUS_STOPPED), stop_timestamp.clone()),
            "case {:?}: the listing must report the same stopped execution, got {listed}",
            case.case
        );

        // The history records what happened: the start the invocation made, then the stop
        // this call made, carrying the reason inside the envelope the model requires.
        let history = events(&get_history(&router, &arn).await);
        assert_eq!(
            history
                .iter()
                .map(|event| event["EventType"].clone())
                .collect::<Vec<Value>>(),
            vec![json!(EVENT_EXECUTION_STARTED), json!(EVENT_EXECUTION_STOPPED)],
            "case {:?}: expected the start and the stop, got {history:?}",
            case.case
        );
        let stop_event = &history[1];
        assert_eq!(
            (
                stop_event["EventId"].clone(),
                stop_event["EventTimestamp"].clone(),
                stop_event["ExecutionStoppedDetails"]["Error"]["Payload"].clone(),
                stop_event["ExecutionStoppedDetails"]["Error"]["Truncated"].clone(),
            ),
            (
                json!(2),
                stop_timestamp.clone(),
                error.clone(),
                json!(false),
            ),
            "case {:?}: the stop event must record the stop this call made, got {stop_event}",
            case.case
        );

        // "Set to false for a more compact response that includes only execution
        // metadata": the reason is execution data, so a compact read withholds it and still
        // reports the status and the instant.
        let compact = get_execution(&router, &arn, "?IncludeExecutionData=false").await;
        assert_eq!(
            (
                compact.string("Status"),
                compact.json()["EndTimestamp"].clone(),
                compact.json()["Error"].clone(),
            ),
            (
                Some(STATUS_STOPPED.to_string()),
                stop_timestamp.clone(),
                Value::Null,
            ),
            "case {:?}: a compact read must report the metadata and no reason, got {}",
            case.case,
            String::from_utf8_lossy(&compact.body)
        );

        // An execution that has ended "cannot be resumed", so it cannot be stopped a
        // second time: the repeat answers the instant it really ended at and performs
        // nothing, which the unchanged history is the record of.
        let repeat = stop(&router, &arn, &json!({ "ErrorType": "SecondStop" })).await;
        assert_eq!(
            (repeat.status, repeat.json()["StopTimestamp"].clone()),
            (200, stop_timestamp.clone()),
            "case {:?}: a repeat must answer the instant the execution ended, got {} {}",
            case.case,
            repeat.status,
            String::from_utf8_lossy(&repeat.body)
        );
        let after_repeat = get_execution(&router, &arn, "").await;
        assert_eq!(
            (
                after_repeat.string("Status"),
                after_repeat.json()["Error"].clone(),
                events(&get_history(&router, &arn).await).len(),
            ),
            (Some(STATUS_STOPPED.to_string()), error.clone(), 2),
            "case {:?}: a repeat must change nothing, got {}",
            case.case,
            String::from_utf8_lossy(&after_repeat.body)
        );
    }
}

/// Move every stored durable execution's timeout instant into the past, so the next read
/// materialises the `RUNNING` -> `TIMED_OUT` transition the clock owes. This is what
/// waiting out the `ExecutionTimeout` would do, without waiting.
fn make_timeout_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        for execution in function.durable_executions.values_mut() {
            execution.timeout_due = Some(past);
        }
    }
}

#[tokio::test]
async fn a_stop_leaves_an_execution_the_clock_already_ended_where_it_is() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-stop-timed-out";
    create_durable_function(&router, &engine, function, Some(3_600)).await;
    let started = invoke(&router, function, "", "timed-out-first", b"{}").await;
    let arn = started
        .header(DURABLE_EXECUTION_ARN_HEADER)
        .unwrap_or_default();
    assert_eq!(
        (started.status, arn.is_empty()),
        (202, false),
        "case \"timed out\": the invocation must answer a handle, got {} {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    make_timeout_due(&engine);

    // The timeout fell due before the stop arrived, so the execution is already terminal:
    // the stop reports the instant that timeout ended it at and does not re-end it.
    let timed_out = get_execution(&router, &arn, "").await;
    let ended = timed_out.json()["EndTimestamp"].clone();
    assert_eq!(
        (timed_out.string("Status"), ended.is_number()),
        (Some(STATUS_TIMED_OUT.to_string()), true),
        "case \"timed out\": the execution must have ended on its timeout, got {}",
        String::from_utf8_lossy(&timed_out.body)
    );
    let answer = stop(&router, &arn, &json!({ "ErrorType": "OperatorStop" })).await;
    assert_eq!(
        (answer.status, answer.json()["StopTimestamp"].clone()),
        (200, ended.clone()),
        "case \"timed out\": the stop must answer the instant the execution ended, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let after = get_execution(&router, &arn, "").await;
    assert_eq!(
        (
            after.string("Status"),
            after.json()["EndTimestamp"].clone(),
            after.json()["Error"].clone(),
        ),
        (Some(STATUS_TIMED_OUT.to_string()), ended, Value::Null),
        "case \"timed out\": the stop must leave the timed-out execution where it is, got {}",
        String::from_utf8_lossy(&after.body)
    );
    assert_eq!(
        events(&get_history(&router, &arn).await)
            .iter()
            .map(|event| event["EventType"].clone())
            .collect::<Vec<Value>>(),
        vec![
            json!(EVENT_EXECUTION_STARTED),
            json!(EVENT_EXECUTION_TIMED_OUT)
        ],
        "case \"timed out\": the stop must append no event of its own, got {}",
        String::from_utf8_lossy(&get_history(&router, &arn).await.body)
    );
}

/// One row of the refusal table: which part of a well-formed `DurableExecutionArn` names
/// nothing this emulator holds.
struct MissingCase {
    case: &'static str,
    /// Builds the ARN to stop out of the handle a real start answered.
    arn: fn(&str) -> String,
}

/// The rows: each component of the handle swapped for one that names nothing — the
/// function, the version the execution runs under, the execution's name, and the minted
/// execution id.
fn missing_cases() -> Vec<MissingCase> {
    vec![
        MissingCase {
            case: "a function this emulator does not hold",
            arn: |handle| handle.replace("durable-missing", "no-such-function"),
        },
        MissingCase {
            case: "a version the execution does not run under",
            arn: |handle| handle.replace(":$LATEST/", ":7/"),
        },
        MissingCase {
            case: "an execution name nothing started",
            arn: |handle| handle.replace("/live-execution/", "/no-such-execution/"),
        },
        MissingCase {
            case: "the right execution under another id",
            arn: |handle| {
                let prefix = handle
                    .rsplit_once('/')
                    .map_or(handle, |(head, _)| head);
                format!("{prefix}/0123456789abcdef0123456789abcdef")
            },
        },
    ]
}

#[tokio::test]
async fn a_stop_of_an_execution_nothing_started_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-missing";
    create_durable_function(&router, &engine, function, Some(3_600)).await;
    let started = invoke(&router, function, "", "live-execution", b"{}").await;
    let handle = started
        .header(DURABLE_EXECUTION_ARN_HEADER)
        .unwrap_or_default();
    assert!(
        !handle.is_empty(),
        "the invocation must answer a handle, got {}",
        String::from_utf8_lossy(&started.body)
    );

    for case in missing_cases() {
        let arn = (case.arn)(&handle);
        assert_ne!(
            arn, handle,
            "case {:?}: the row must name something other than the live execution",
            case.case
        );
        let answer = stop(&router, &arn, &json!({ "ErrorType": "OperatorStop" })).await;
        assert_eq!(
            answer.status,
            404,
            "case {:?}: expected 404 for {arn}, got {} {}",
            case.case,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {:?}: expected the modelled ResourceNotFoundException, got {:?}",
            case.case,
            answer.error_code()
        );
        assert_eq!(
            answer
                .string("message")
                .or_else(|| answer.string("Message")),
            Some(not_found_sentence(&arn)),
            "case {:?}: expected the refusal to name the execution asked about, got {}",
            case.case,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // And the live execution the rows were derived from is untouched by every refusal.
    let read = get_execution(&router, &handle, "").await;
    assert_eq!(
        (read.status, read.string("Status")),
        (200, Some(STATUS_RUNNING.to_string())),
        "a refused stop must leave the live execution running, got {} {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

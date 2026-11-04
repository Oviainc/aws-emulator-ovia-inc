// SPDX-License-Identifier: CC0-1.0

//! The IoT COMMAND-EXECUTION lifecycle across TWO services: the control plane creates
//! the command (`iot` `CreateCommand`) and later reads and deletes the execution (`iot`
//! `GetCommandExecution`, `DeleteCommandExecution`), while the device's own plane is
//! what starts it and reports its progress (`iot-jobs-data`
//! `StartCommandExecution`, `UpdateJobExecution`, `DescribeJobExecution`).
//!
//! This suite needs the WHOLE router: the execution is written by one crate and read by
//! another, and the point under test is that it is ONE execution — a single-service test
//! cannot see the hand-off at all.
//!
//! The rule under test: `StartCommandExecution` mints one command execution of the
//! command and device it names, and from then on the control plane's
//! `GetCommandExecution` computes its answer from that one record — `CREATED` until a
//! device takes it up, then the status the device reported, carrying the command's ARN,
//! the device's ARN, the parameters the start handed the device and the execution
//! timeout the start was given — while `DeleteCommandExecution` refuses the execution
//! until it reaches a terminal status and then removes it from the shared store, for the
//! device's plane as well as the control plane's.
//!
//! The rows differ along what the rule generalises over: whether the start carries
//! parameters and of which modelled types, whether it sets an execution timeout, and
//! which terminal status the device finishes with (including a rejection, which a device
//! may only make before it starts the work).
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

// The one percent-encoder this test crate has: RFC 3986's unreserved set left literal,
// everything else `%XX`. Its module says nothing new should write another one, and a
// `targetArn` in a query string needs exactly that encoding.
use crate::query_wire::form_encode;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A SigV4 credential scope naming `signing_name` in [`REGION`] — the signal the
/// router picks the owning service by.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One row of the lifecycle table.
struct CommandCase {
    case: &'static str,
    command_id: &'static str,
    thing_name: &'static str,
    /// The `parameters` the start carries, as the request body renders them; empty for
    /// a start that carries none.
    parameters: &'static str,
    /// `executionTimeoutSeconds`, when the row sets one
    /// (`CommandExecutionTimeoutInSeconds` is modelled `range 1..`).
    timeout_seconds: Option<i64>,
    /// Whether the device reports `IN_PROGRESS` before it finishes. A device may only
    /// REJECT work it has not started, so the rejecting row finishes straight away.
    reports_progress: bool,
    /// The terminal status the device finishes with.
    terminal_status: &'static str,
}

const COMMAND_CASES: &[CommandCase] = &[
    CommandCase {
        case: "no parameters, no timeout, the device succeeds",
        command_id: "handoff-reboot",
        thing_name: "cmd-laser-1",
        parameters: "",
        timeout_seconds: None,
        reports_progress: true,
        terminal_status: "SUCCEEDED",
    },
    CommandCase {
        case: "a parameter of every modelled type, a timeout, the device fails",
        command_id: "handoff_set-temperature",
        thing_name: "cmd-laser:line-2_A",
        // `CommandParameterValue` carries one member per parameter type (`S`, `B`, `I`,
        // `L`, `D`, `BIN`, `UL`); a blob travels base64 (`AAEC` is 0x00 0x01 0x02).
        parameters: r#"{
            "s": { "S": "on" },
            "b": { "B": false },
            "i": { "I": 3 },
            "l": { "L": 9000000000 },
            "d": { "D": 0.5 },
            "bin": { "BIN": "AAEC" },
            "ul": { "UL": "18446744073709551615" }
        }"#,
        timeout_seconds: Some(900),
        reports_progress: true,
        terminal_status: "FAILED",
    },
    CommandCase {
        case: "one parameter, a short timeout, the device rejects it",
        command_id: "handoff-eject",
        thing_name: "cmd-laser-3",
        parameters: r#"{ "mode": { "S": "off" } }"#,
        // Minutes rather than the modelled minimum of 1 second: this row drives the
        // device's own calls AFTER the read, and a timer that falls due in the middle
        // of the lifecycle would legitimately move the execution to `TIMED_OUT` before
        // the device could reject it. The published minimum is exercised where no later
        // call depends on it, in the owning crate's `command_execution_read` suite.
        timeout_seconds: Some(1_800),
        reports_progress: false,
        terminal_status: "REJECTED",
    },
];

/// The parameters a row carries, or `None` when it carries none.
fn parameters_of(case: &CommandCase) -> Option<Value> {
    (!case.parameters.is_empty()).then(|| {
        serde_json::from_str(case.parameters).unwrap_or_else(|why| {
            panic!("case {:?}: the row's parameters must be JSON: {why}", case.case)
        })
    })
}

#[test]
fn a_started_command_execution_is_the_one_the_control_plane_reads_and_deletes() {
    std::thread::Builder::new()
        .name("iot-command-execution-lifecycle".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let call = |method: &str, url: String, signing_name: &str, body: String| {
                let request = http::Request::builder()
                    .method(method)
                    .uri(url)
                    .header("authorization", authorization(signing_name))
                    .header("content-type", "application/json")
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let error_code = response
                    .headers()
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let parsed: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
                (status, error_code, rendered, parsed)
            };

            // The control plane, signed for `iot`.
            let control = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://iot.{REGION}.amazonaws.com{path}"),
                    "iot",
                    body,
                )
            };
            // The Jobs data plane the device talks to, signed for `iot-jobs-data`.
            let device = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://data.jobs.iot.{REGION}.amazonaws.com{path}"),
                    "iot-jobs-data",
                    body,
                )
            };

            for case in COMMAND_CASES {
                let label = case.case;
                let thing = case.thing_name;
                let target = format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing}");
                let command = format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:command/{}", case.command_id);
                let parameters = parameters_of(case);

                // The command the execution runs has to EXIST: the control plane creates
                // it, and the data plane resolves the `commandArn` against that record.
                let (status, _, rendered, _) = control(
                    "PUT",
                    format!("/commands/{}", case.command_id),
                    "{}".to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateCommand must answer 200, got {status} with \
                     {rendered}"
                );

                // The DEVICE plane starts the execution.
                let mut start = json!({ "targetArn": target, "commandArn": command });
                if let Some(parameters) = parameters.clone() {
                    start["parameters"] = parameters;
                }
                if let Some(seconds) = case.timeout_seconds {
                    start["executionTimeoutSeconds"] = json!(seconds);
                }
                let (status, _, rendered, parsed) = device(
                    "POST",
                    "/command-executions".to_string(),
                    start.to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: StartCommandExecution must answer 200, got {status} \
                     with {rendered}"
                );
                let execution_id = parsed["executionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !execution_id.is_empty()
                        && execution_id.len() <= 64
                        && execution_id
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                    "case {label:?}: the minted executionId must satisfy CommandExecutionId \
                     (len 1..64, ^[a-zA-Z0-9_-]+$), got {execution_id:?}"
                );

                let read = |execution_id: &str, target_arn: &str| {
                    control(
                        "GET",
                        format!(
                            "/command-executions/{execution_id}?targetArn={}",
                            form_encode(target_arn)
                        ),
                        String::new(),
                    )
                };
                let delete = |execution_id: &str, target_arn: &str| {
                    control(
                        "DELETE",
                        format!(
                            "/command-executions/{execution_id}?targetArn={}",
                            form_encode(target_arn)
                        ),
                        String::new(),
                    )
                };

                // The CONTROL plane reports the execution the DEVICE plane minted.
                let (status, _, rendered, reported) = read(&execution_id, &target);
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetCommandExecution of the started execution must \
                     answer 200, got {status} with {rendered}"
                );
                assert_eq!(
                    (
                        reported["executionId"].as_str(),
                        reported["commandArn"].as_str(),
                        reported["targetArn"].as_str(),
                        reported["status"].as_str(),
                    ),
                    (
                        Some(execution_id.as_str()),
                        Some(command.as_str()),
                        Some(target.as_str()),
                        Some("CREATED"),
                    ),
                    "case {label:?}: a started execution is CREATED and names its command \
                     and device, got {rendered}"
                );
                assert_eq!(
                    reported["executionTimeoutSeconds"].as_i64(),
                    case.timeout_seconds,
                    "case {label:?}: the reported timeout is the one the start was given, \
                     got {rendered}"
                );
                assert_eq!(
                    parameters.clone().unwrap_or(Value::Null),
                    reported
                        .get("parameters")
                        .cloned()
                        .unwrap_or(Value::Null),
                    "case {label:?}: the reported parameters are the ones the start handed \
                     the device, got {rendered}"
                );
                assert!(
                    reported["createdAt"].as_f64().is_some()
                        && reported["lastUpdatedAt"].as_f64().is_some()
                        && reported["timeToLive"].as_f64().is_some()
                        && reported["startedAt"].as_f64() == reported["createdAt"].as_f64()
                        && reported["completedAt"].is_null(),
                    "case {label:?}: a created execution is timestamped, started on its \
                     device by the start call and not completed, got {rendered}"
                );

                // The execution the control plane reports is the one the DEVICE polls.
                let (status, _, rendered, polled) =
                    device("GET", format!("/things/{thing}/jobs/{execution_id}"), String::new());
                assert_eq!(
                    (status, polled["execution"]["jobId"].as_str()),
                    (200, Some(execution_id.as_str())),
                    "case {label:?}: the device must see the very execution the control \
                     plane reported, got {status} with {rendered}"
                );

                // "Only command executions that enter a terminal state can be deleted":
                // while it is CREATED the delete is refused and changes nothing.
                let (status, error_code, rendered, refusal) = delete(&execution_id, &target);
                assert_eq!(
                    (status, error_code.as_str(), refusal["message"].as_str()),
                    (
                        400,
                        "ValidationException",
                        Some("Failed to delete command execution as it hasn't become terminal."),
                    ),
                    "case {label:?}: deleting a non-terminal execution must be refused, \
                     got {status} with {rendered}"
                );
                let (status, _, rendered, still_there) = read(&execution_id, &target);
                assert_eq!(
                    (status, still_there["status"].as_str()),
                    (200, Some("CREATED")),
                    "case {label:?}: a refused delete leaves the execution as it was, got \
                     {status} with {rendered}"
                );

                // The execution belongs to ITS device: another device's ARN holds nothing
                // under that id.
                let (status, error_code, rendered, _) = read(
                    &execution_id,
                    &format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing}-elsewhere"),
                );
                assert_eq!(
                    (status, error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {label:?}: an execution is not reported for another device, got \
                     {status} with {rendered}"
                );

                if case.reports_progress {
                    let (status, _, rendered, _) = device(
                        "POST",
                        format!("/things/{thing}/jobs/{execution_id}"),
                        json!({ "status": "IN_PROGRESS" }).to_string(),
                    );
                    assert_eq!(
                        status, 200,
                        "case {label:?}: the device taking the execution up must answer 200, \
                         got {status} with {rendered}"
                    );
                    let (status, _, rendered, in_progress) = read(&execution_id, &target);
                    assert_eq!(
                        (status, in_progress["status"].as_str()),
                        (200, Some("IN_PROGRESS")),
                        "case {label:?}: the control plane must report the status the device \
                         reported, got {status} with {rendered}"
                    );
                    assert!(
                        in_progress["startedAt"].as_f64().is_some()
                            && in_progress["completedAt"].is_null(),
                        "case {label:?}: an execution a device took up has started and not \
                         completed, got {rendered}"
                    );
                }

                // The device finishes the execution.
                let (status, _, rendered, _) = device(
                    "POST",
                    format!("/things/{thing}/jobs/{execution_id}"),
                    json!({ "status": case.terminal_status }).to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: the device finishing the execution must answer 200, got \
                     {status} with {rendered}"
                );
                let (status, _, rendered, finished) = read(&execution_id, &target);
                assert_eq!(
                    (status, finished["status"].as_str()),
                    (200, Some(case.terminal_status)),
                    "case {label:?}: the control plane must report the terminal status the \
                     device reported, got {status} with {rendered}"
                );
                assert!(
                    finished["completedAt"].as_f64().is_some(),
                    "case {label:?}: a finished execution reports when it completed, got \
                     {rendered}"
                );

                // Terminal, so the delete goes through — and the record is gone from the
                // SHARED store, which is what the device's own plane reads.
                let (status, _, rendered, _) = delete(&execution_id, &target);
                assert_eq!(
                    status, 200,
                    "case {label:?}: deleting a terminal execution must answer 200, got \
                     {status} with {rendered}"
                );
                let (status, error_code, rendered, _) = read(&execution_id, &target);
                assert_eq!(
                    (status, error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {label:?}: a deleted execution is no longer reported, got {status} \
                     with {rendered}"
                );
                let (status, error_code, rendered, _) =
                    device("GET", format!("/things/{thing}/jobs/{execution_id}"), String::new());
                assert_eq!(
                    (status, error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {label:?}: a deleted execution is gone for the device too, got \
                     {status} with {rendered}"
                );
                // Deleting it again removes nothing and is not an error the model declares.
                let (status, _, rendered, _) = delete(&execution_id, &target);
                assert_eq!(
                    status, 200,
                    "case {label:?}: a repeated delete removes nothing and answers 200, got \
                     {status} with {rendered}"
                );
            }
        })
        .expect("the lifecycle thread spawns")
        .join()
        .expect("the lifecycle thread finishes");
}

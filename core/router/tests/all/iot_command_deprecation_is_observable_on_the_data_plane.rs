// SPDX-License-Identifier: CC0-1.0

//! Whether a DEPRECATION the IoT control plane records is OBSERVABLE to a client of the
//! device-facing plane — not merely that the start is refused, but that the refusal says
//! which rule refused it.
//!
//! The rule under test: a lifecycle flag the control plane writes
//! (`iot:UpdateCommand$deprecated`) decides, at every later start, whether the command is
//! one `iot-jobs-data:StartCommandExecution` may mint an execution from, and a start it
//! refuses for that reason carries the modelled `ValidationException` (HTTP 400) whose
//! message is built FROM THE REQUEST — it names the command the supplied `commandArn`
//! named and states that that command has been deprecated — so a client can tell this
//! refusal apart from every other `ValidationException` of the same operation, while the
//! flag taken back makes the identical start serve again. A refused start mints nothing on
//! either plane.
//!
//! All of this operation's client-side refusals arrive as the same code and status, so the
//! message is the only discriminator: the suite therefore asserts the deprecation message
//! against the forms of the sibling refusals (an unknown command, an unusable target,
//! `parameters` against a static payload) and requires all four to differ.
//!
//! What `iot_command_executions_are_listed_across_planes.rs` already holds is that such a
//! start is refused with that code and status and mints nothing; what is asserted HERE is
//! the thing a client reads — the refusal's MESSAGE, over the forms a command id takes, and
//! its distinctness from the sibling refusals — together with the restore leg (the same
//! start served again, with a fresh execution, once the flag is taken back).
//!
//! This suite needs the WHOLE router: the flag is written by one service crate (`iot`,
//! `PATCH /commands/{commandId}` signed for `iot`) and read by another (`iot-jobs-data`,
//! `POST /command-executions` signed for `iot-jobs-data`), so a single-service test can
//! only seed the shared index itself — which is the half this one does not take on trust.
//!
//! The rows differ along what the rule generalises over: the form of the command id
//! (hyphens, underscores, digits, mixed case, the modelled 64-character maximum —
//! `CommandId` is `len 1..64, ^[a-zA-Z0-9_-]+$`), the command's payload kind (a literal
//! static payload or a template), and whether the start carries `parameters`. The message
//! depends on none of these except the command the request named.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A SigV4 credential scope naming `signing_name` in [`REGION`] — the signal the router
/// picks the owning service by.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The `executionId`s a `ListCommandExecutions` reply carries, sorted — a rule about WHICH
/// executions exist does not depend on the order they are listed in.
///
/// A summary that carries no `executionId` is a malformed listing rather than an execution
/// with an empty id, so it panics here instead of being normalised into a value a
/// comparison could accept.
fn listed_execution_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> = body
        .get("commandExecutions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry
                .get("executionId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("every listed command execution names its executionId, got {entry}")
                })
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// One command the control plane creates, deprecates, and un-deprecates, and the start the
/// device plane makes against it each time.
struct Row {
    case: &'static str,
    /// The command id, which is also what the data plane's refusal must name.
    command_id: &'static str,
    /// The device the start targets.
    thing: &'static str,
    /// Whether the command is created with a literal, STATIC payload (otherwise with a
    /// `payloadTemplate`, whose placeholders a start's parameters fill).
    static_payload: bool,
    /// Whether the start carries `parameters`. A static-payload command admits none, so
    /// only the templated rows carry them.
    parameters: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a hyphenated command id, static payload, no parameters",
        command_id: "deprecation-reboot-now",
        thing: "deprecation-laser-1",
        static_payload: true,
        parameters: false,
    },
    Row {
        case: "an underscored command id, templated payload, parameters",
        command_id: "deprecation_install_firmware_2",
        thing: "deprecation-laser-2",
        static_payload: false,
        parameters: true,
    },
    Row {
        case: "a numeric command id, templated payload, no parameters",
        command_id: "20261025",
        thing: "deprecation-laser-3",
        static_payload: false,
        parameters: false,
    },
    Row {
        case: "a mixed-case command id, static payload, no parameters",
        command_id: "Deprecation_Reboot-Laser7",
        thing: "deprecation-laser:line-4_A",
        static_payload: true,
        parameters: false,
    },
    Row {
        // `CommandId` is modelled `len 1..64`, so this row sits on the published maximum.
        case: "a 64-character command id, templated payload, parameters",
        command_id: "dccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        thing: "deprecation-laser-5",
        static_payload: false,
        parameters: true,
    },
];

/// The message the data plane must answer a start of a DEPRECATED command with: the
/// command the request's `commandArn` named, and the lifecycle state that refused it.
fn deprecated_refusal(command_id: &str) -> String {
    format!("The command {command_id} has been deprecated.")
}

#[test]
fn a_recorded_deprecation_refuses_a_start_by_naming_the_command_it_deprecated() {
    std::thread::Builder::new()
        .name("iot-command-deprecation-observable".to_string())
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
            // The device-facing Jobs plane, signed for `iot-jobs-data`.
            let device = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://data.jobs.iot.{REGION}.amazonaws.com{path}"),
                    "iot-jobs-data",
                    body,
                )
            };

            // Every refusal message the rows observed, so a template that ignored the
            // request would collapse them into fewer than one per command.
            let mut refusals: BTreeSet<String> = BTreeSet::new();

            for row in ROWS {
                let Row {
                    case,
                    command_id,
                    thing,
                    static_payload,
                    parameters,
                } = *row;
                let target_arn = format!("arn:aws:iot:{REGION}:000000000000:thing/{thing}");

                // The CONTROL plane creates the command, which is what decides the ARN the
                // data plane resolves.
                let create = if static_payload {
                    json!({
                        // `CommandPayload$content` is a blob, which restJson1 writes
                        // base64: `{"reboot": true}`.
                        "payload": {
                            "content": "eyJyZWJvb3QiOiB0cnVlfQ==",
                            "contentType": "application/json",
                        },
                    })
                } else {
                    json!({ "payloadTemplate": "{\"mode\":\"${mode}\"}" })
                };
                let (status, _, rendered, created) =
                    control("PUT", format!("/commands/{command_id}"), create.to_string());
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateCommand must answer 200, got {status} with {rendered}"
                );
                let command_arn = created["commandArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no commandArn in {rendered}"))
                    .to_string();

                let start_body = {
                    let mut request = json!({
                        "targetArn": target_arn,
                        "commandArn": command_arn,
                    });
                    if parameters {
                        request["parameters"] = json!({ "mode": { "S": "fast" } });
                    }
                    request.to_string()
                };
                let start = |body: String| device("POST", "/command-executions".to_string(), body);

                // Before any deprecation the start is served: the positive control, so a
                // later refusal is about the flag and not about the request.
                let (status, _, rendered, served) = start(start_body.clone());
                assert_eq!(
                    status, 200,
                    "case {case:?}: the start of a live command must be served, got {status} \
                     with {rendered}"
                );
                let first_execution_id = served["executionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !first_execution_id.is_empty(),
                    "case {case:?}: a served start mints an executionId, got {rendered}"
                );

                // Flip the flag both ways, and read the SAME start back each time: the
                // state the data plane answers from is the one the control plane last
                // recorded, at every call rather than once.
                let mut served_ids: Vec<String> = vec![first_execution_id.clone()];
                for deprecated in [true, false, true] {
                    let (status, _, rendered, updated) = control(
                        "PATCH",
                        format!("/commands/{command_id}"),
                        json!({ "deprecated": deprecated }).to_string(),
                    );
                    assert_eq!(
                        (status, updated["deprecated"].as_bool()),
                        (200, Some(deprecated)),
                        "case {case:?}: UpdateCommand must record deprecated={deprecated}, got \
                         {status} with {rendered}"
                    );
                    // A deprecated command still EXISTS and is still readable: the flag is
                    // about what may be started, not about the resource.
                    let (status, _, rendered, read) =
                        control("GET", format!("/commands/{command_id}"), String::new());
                    assert_eq!(
                        (status, read["deprecated"].as_bool()),
                        (200, Some(deprecated)),
                        "case {case:?}: the command must stay readable with deprecated\
                         ={deprecated}, got {status} with {rendered}"
                    );

                    let (status, error_code, rendered, answered) = start(start_body.clone());
                    if deprecated {
                        assert_eq!(
                            (status, error_code.as_str()),
                            (400, "ValidationException"),
                            "case {case:?}: a start of a deprecated command must be refused 400 \
                             ValidationException, got {status} with {rendered}"
                        );
                        let message = answered["message"].as_str().unwrap_or_default();
                        assert_eq!(
                            message,
                            deprecated_refusal(command_id),
                            "case {case:?}: the refusal must name the command the request named \
                             and state that it has been deprecated, got {rendered}"
                        );
                        assert!(
                            answered["executionId"].is_null(),
                            "case {case:?}: a refusal mints no executionId, got {rendered}"
                        );
                        refusals.insert(message.to_string());
                    } else {
                        assert_eq!(
                            status, 200,
                            "case {case:?}: taking the deprecation back must make the identical \
                             start serve again, got {status} with {rendered}"
                        );
                        let execution_id = answered["executionId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        assert!(
                            !execution_id.is_empty() && !served_ids.contains(&execution_id),
                            "case {case:?}: the restored start must mint a FRESH executionId, got \
                             {execution_id:?} after {served_ids:?}"
                        );
                        served_ids.push(execution_id);
                    }
                }

                // The device's queue holds exactly the executions the SERVED starts minted:
                // the refusals enqueued nothing on it.
                let (status, _, rendered, queue) =
                    device("GET", format!("/things/{thing}/jobs"), String::new());
                assert_eq!(
                    (status, queue["queuedJobs"].as_array().map(Vec::len)),
                    (200, Some(served_ids.len())),
                    "case {case:?}: expected the {} served start(s) queued and nothing from the \
                     refusals, got {status} with {rendered}",
                    served_ids.len()
                );

                // And so does the CONTROL plane's own listing of the command's executions.
                let (status, _, rendered, listed) = control(
                    "POST",
                    "/command-executions".to_string(),
                    json!({ "commandArn": command_arn }).to_string(),
                );
                let mut expected = served_ids.clone();
                expected.sort();
                assert_eq!(
                    (status, listed_execution_ids(&listed)),
                    (200, expected.clone()),
                    "case {case:?}: ListCommandExecutions must report exactly the served \
                     executions {expected:?}, got {status} with {rendered}"
                );
            }

            // `refusals` now holds every deprecation message the rows observed — one per
            // command, since each row asserted its own — which is what the sibling
            // refusals below are required to differ from.

            // The message is the only thing that tells this refusal apart from the other
            // `ValidationException`s of the same operation, so the forms must differ — and
            // each must be the one ITS own rule builds. The siblings are issued against a
            // command the control plane has NOT deprecated, created here for them, because
            // the deprecation is read FIRST: a start of a deprecated command is refused for
            // the deprecation whatever else the request carries, so a sibling issued against
            // one of the rows' commands could never reach the rule it is about.
            const SIBLING_COMMAND: &str = "deprecation-sibling-static";
            let (status, _, rendered, created) = control(
                "PUT",
                format!("/commands/{SIBLING_COMMAND}"),
                json!({
                    "payload": {
                        "content": "eyJyZWJvb3QiOiB0cnVlfQ==",
                        "contentType": "application/json",
                    },
                })
                .to_string(),
            );
            assert_eq!(
                status, 200,
                "case \"the siblings' command\": CreateCommand must answer 200, got {status} with \
                 {rendered}"
            );
            let live_command_arn = created["commandArn"]
                .as_str()
                .unwrap_or_else(|| panic!("no commandArn in {rendered}"))
                .to_string();
            let (status, _, rendered, read) =
                control("GET", format!("/commands/{SIBLING_COMMAND}"), String::new());
            assert_eq!(
                (status, read["deprecated"].as_bool()),
                (200, Some(false)),
                "case \"the siblings' command\": it must NOT be deprecated, or the siblings cannot \
                 reach their own rules, got {status} with {rendered}"
            );
            let live_target_arn = format!("arn:aws:iot:{REGION}:000000000000:thing/sibling-laser");
            const ABSENT_COMMAND_ID: &str = "no-such-command";

            // (case, request, the message the sibling's own rule builds)
            let siblings: [(&str, String, String); 3] = [
                (
                    "an unknown command",
                    json!({
                        "targetArn": live_target_arn,
                        "commandArn": format!(
                            "arn:aws:iot:{REGION}:000000000000:command/{ABSENT_COMMAND_ID}"
                        ),
                    })
                    .to_string(),
                    format!("The command {ABSENT_COMMAND_ID} was not found."),
                ),
                (
                    "a target that is not an ARN",
                    json!({ "targetArn": "not-an-arn", "commandArn": live_command_arn })
                        .to_string(),
                    "Invalid target arn not-an-arn not supported as a target".to_string(),
                ),
                (
                    "parameters against a static payload",
                    json!({
                        "targetArn": live_target_arn,
                        "commandArn": live_command_arn,
                        "parameters": { "mode": { "S": "fast" } },
                    })
                    .to_string(),
                    "The parameters option is unsupported for commands with static payloads."
                        .to_string(),
                ),
            ];
            // Seeded with every deprecation message the rows observed, so a sibling that
            // answered the deprecation form instead of its own is caught here as well as by
            // its own expected message.
            let mut distinct: BTreeSet<String> = refusals.clone();
            for (case, body, expected) in siblings {
                let (status, error_code, rendered, answered) =
                    device("POST", "/command-executions".to_string(), body);
                assert_eq!(
                    (status, error_code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected 400 ValidationException, got {status} with {rendered}"
                );
                let message = answered["message"].as_str().unwrap_or_default().to_string();
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the message this refusal's own rule builds, got \
                     {rendered}"
                );
                assert!(
                    distinct.insert(message.clone()),
                    "case {case:?}: every refusal of this operation must be distinguishable by \
                     its message, got {message:?} again among {distinct:?}"
                );
            }
        })
        .expect("the deprecation thread spawns")
        .join()
        .expect("the deprecation thread finishes");
}

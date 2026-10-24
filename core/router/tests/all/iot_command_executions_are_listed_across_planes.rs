// SPDX-License-Identifier: CC0-1.0

//! The CROSS-PLANE visibility of a command execution: `iot-jobs-data`
//! `StartCommandExecution` mints it on the device-facing plane, and from that moment the
//! IoT control plane's own readers answer about it — `GetCommandExecution` reports it and
//! `ListCommandExecutions` lists it — while what that plane may mint at all is decided by
//! the command the control plane created and still owns.
//!
//! Two rules, one per test:
//!
//! * a command execution the data plane mints is IMMEDIATELY one of the account's command
//!   executions on the control plane, resting at `CREATED` until a device takes it up, and
//!   `ListCommandExecutions` selects exactly the executions the request's filters name —
//!   projected from the SAME stored record the single-execution reader reports, so the two
//!   planes never disagree about what exists;
//! * a start is served only in the form the referenced COMMAND admits, as the control
//!   plane recorded it: a command it has DEPRECATED admits no new execution until the
//!   deprecation is taken back, and a command whose payload is STATIC admits none carrying
//!   `parameters`, each refusal 400 `ValidationException` that mints nothing.
//!
//! This suite needs the WHOLE router: the execution is written by one service crate
//! (`iot-jobs-data`, `POST /command-executions` signed for `iot-jobs-data`) and read by
//! another (`iot`, the same path signed for `iot`), so a single-service test cannot see
//! the hand-off — the owning crates' own suites seed the shared store themselves, which
//! is exactly the half this one does not take on trust.
//!
//! The rows differ along what the rules generalise over: which member filters the listing
//! (`commandArn`, `targetArn`, `namespace`, `status`, a time window, none at all), which
//! of the forms a `TimeFilter` bound admits the instant is written in, how many
//! executions a filter selects (all three, two, one, none), which way the listing is
//! ordered, the page size it is walked in, and — for the second rule — the command's
//! payload kind, its deprecation state and whether the start carries parameters.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_emulator_router::{build_router, InMemoryEngine};
use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};

// The one percent-encoder this test crate has: RFC 3986's unreserved set left literal,
// everything else `%XX`. Its module says nothing new should write another one, and a
// `targetArn` in a query string needs exactly that encoding.
use crate::query_wire::form_encode;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for — and therefore the region every ARN the
/// emulator mints or reads is scoped to.
const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// The command the `AWS-IoT` rows run.
const REBOOT: &str = "listing-handoff-reboot";

/// The command the `AWS-IoT-FleetWise` rows run.
const SIGNALS: &str = "listing-handoff-collect-signals";

/// The namespace a command created without one is in, and the one `roleArn` is refused
/// with (`CreateCommand$roleArn`).
const BASE_NAMESPACE: &str = "AWS-IoT";

/// The other `CommandNamespace` value, whose commands build their payload through an IAM
/// role (`CreateCommand$roleArn`: "The IAM role that you must provide when using the
/// AWS-IoT-FleetWise namespace").
const FLEETWISE_NAMESPACE: &str = "AWS-IoT-FleetWise";

/// A SigV4 credential scope naming `signing_name` in [`REGION`] — the signal the router
/// picks the owning service by, and the service reads its region from.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
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
        .expect("an instant renders")
}

/// An instant as the MINUTE-precision form `TimeFilter`'s members document ("The date and
/// time uses the format yyyy-MM-dd'T'HH:mm"). Truncating drops the seconds, so the bound
/// it names is at or before `instant` — which is what a client that writes the documented
/// form sends.
fn minute_form(instant: i64) -> String {
    zulu(instant)[..16].to_string()
}

/// An instant as a civil date-time carrying the `+02:00` UTC offset that names it: the
/// same instant, written the way a client whose clock is not UTC sends it.
fn offset_form(instant: i64) -> String {
    format!("{}+02:00", &zulu(instant + 2 * 3_600)[..19])
}

/// Now, as epoch seconds.
fn now_epoch_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is at or after the epoch")
            .as_secs(),
    )
    .expect("an epoch second fits an i64")
}

/// One execution the DEVICE plane starts.
struct StartCase {
    case: &'static str,
    thing: &'static str,
    command_id: &'static str,
    /// The `parameters` the start carries, as the request body renders them; empty for a
    /// start that carries none.
    parameters: &'static str,
    /// `executionTimeoutSeconds`, when the row sets one
    /// (`CommandExecutionTimeoutInSeconds` is modelled `range 1..`).
    timeout_seconds: Option<i64>,
}

/// The starts, in the order the data plane accepts them — which is the enqueue order the
/// listing's default (descending) order is the reverse of.
///
/// The rows span which device an execution belongs to, which command it runs (and so
/// which namespace that command was created in), whether it carries parameters, and
/// whether it arms an execution timeout. Every timeout here is far enough ahead that the
/// `TIMED_OUT` transition the clock owes cannot fall due inside the suite; that
/// transition has its own coverage in the owning crate's `command_execution_read` suite.
const START_CASES: &[StartCase] = &[
    StartCase {
        case: "no parameters, no timeout",
        thing: "listing-laser-1",
        command_id: REBOOT,
        parameters: "",
        timeout_seconds: None,
    },
    StartCase {
        case: "the same command on another device, with parameters and a timeout",
        thing: "listing-laser:line-2_A",
        command_id: REBOOT,
        parameters: r#"{ "mode": { "S": "fast" }, "retries": { "I": 2 } }"#,
        timeout_seconds: Some(1_800),
    },
    StartCase {
        case: "another command on the first device",
        thing: "listing-laser-1",
        command_id: SIGNALS,
        parameters: "",
        timeout_seconds: Some(3_600),
    },
];

/// One execution the data plane minted, as the rest of the suite refers to it.
#[derive(Debug, Clone)]
struct Started {
    case: &'static str,
    execution_id: String,
    thing: &'static str,
    command_id: &'static str,
}

/// The `executionId`s of a `ListCommandExecutions` reply, in the order it returned them.
fn execution_ids(body: &Value) -> Vec<String> {
    body.get("commandExecutions")
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

/// One summary of a reply, by the `executionId` it carries.
fn summary_of<'reply>(body: &'reply Value, execution_id: &str) -> Option<&'reply Value> {
    body.get("commandExecutions")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("executionId").and_then(Value::as_str) == Some(execution_id))
}

/// `ids`, sorted — how a row whose rule is about WHICH executions are selected rather
/// than in which order compares them.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
}

#[test]
fn a_started_command_execution_is_immediately_listed_by_the_control_plane() {
    std::thread::Builder::new()
        .name("iot-command-executions-across-planes".to_string())
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
            // The Jobs data plane the device talks to, signed for `iot-jobs-data`. Its
            // `POST /command-executions` is the SAME method and path the control plane's
            // listing is bound to, which is why the hand-off is only visible through the
            // router: the signing name is what tells the two apart.
            let device = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://data.jobs.iot.{REGION}.amazonaws.com{path}"),
                    "iot-jobs-data",
                    body,
                )
            };

            let list = |request: &Value, query: &str| {
                control(
                    "POST",
                    format!("/command-executions{query}"),
                    request.to_string(),
                )
            };
            let read = |execution_id: &str, target: &str| {
                control(
                    "GET",
                    format!(
                        "/command-executions/{execution_id}?targetArn={}",
                        form_encode(target)
                    ),
                    String::new(),
                )
            };

            // The commands the executions run have to EXIST: the data plane resolves a
            // start's `commandArn` against the control plane's own record, and the
            // listing's `namespace` filter is a property of that record.
            for (command_id, body) in [
                (REBOOT, json!({ "namespace": BASE_NAMESPACE })),
                (
                    SIGNALS,
                    json!({
                        "namespace": FLEETWISE_NAMESPACE,
                        // Required with this namespace and refused with the other
                        // (`CreateCommand$roleArn`).
                        "roleArn": format!("arn:aws:iam::{ACCOUNT_ID}:role/iot-fleetwise-signals"),
                    }),
                ),
            ] {
                let (status, _, rendered, _) =
                    control("PUT", format!("/commands/{command_id}"), body.to_string());
                assert_eq!(
                    status, 200,
                    "case {command_id:?}: CreateCommand must answer 200, got {status} with \
                     {rendered}"
                );
            }

            // An instant every start below is at or after, written in the forms a
            // `TimeFilter` bound admits.
            let before_any_start = now_epoch_seconds();

            let mut started: Vec<Started> = Vec::new();
            for case in START_CASES {
                let label = case.case;
                let target = target_arn(case.thing);
                let command = command_arn(case.command_id);

                let mut request = json!({ "targetArn": target, "commandArn": command });
                if !case.parameters.is_empty() {
                    request["parameters"] = serde_json::from_str(case.parameters)
                        .unwrap_or_else(|why| {
                            panic!("case {label:?}: the row's parameters must be JSON: {why}")
                        });
                }
                if let Some(seconds) = case.timeout_seconds {
                    request["executionTimeoutSeconds"] = json!(seconds);
                }

                let (status, _, rendered, parsed) = device(
                    "POST",
                    "/command-executions".to_string(),
                    request.to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: StartCommandExecution must answer 200, got {status} with \
                     {rendered}"
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
                started.push(Started {
                    case: label,
                    execution_id,
                    thing: case.thing,
                    command_id: case.command_id,
                });
            }

            // HALF ONE of the rule: the control plane reports each minted execution right
            // away, resting at `CREATED` because no device has taken it up.
            for execution in &started {
                let label = execution.case;
                let target = target_arn(execution.thing);
                let (status, error_code, rendered, reported) = read(&execution.execution_id, &target);
                assert_eq!(
                    (status, error_code.as_str()),
                    (200, ""),
                    "case {label:?}: GetCommandExecution of the just-started execution must \
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
                        Some(execution.execution_id.as_str()),
                        Some(command_arn(execution.command_id).as_str()),
                        Some(target.as_str()),
                        Some("CREATED"),
                    ),
                    "case {label:?}: a started execution is CREATED and names its command and \
                     device, got {rendered}"
                );

                // One record, two planes: the device polls the very execution the control
                // plane just reported.
                let (status, _, rendered, polled) = device(
                    "GET",
                    format!("/things/{}/jobs/{}", execution.thing, execution.execution_id),
                    String::new(),
                );
                assert_eq!(
                    (status, polled["execution"]["jobId"].as_str()),
                    (200, Some(execution.execution_id.as_str())),
                    "case {label:?}: the device must see the execution the control plane \
                     reported, got {status} with {rendered}"
                );
            }

            let id_of = |thing: &str, command_id: &str| -> Vec<String> {
                started
                    .iter()
                    .filter(|execution| {
                        (thing.is_empty() || execution.thing == thing)
                            && (command_id.is_empty() || execution.command_id == command_id)
                    })
                    .map(|execution| execution.execution_id.clone())
                    .collect()
            };
            let every_id = id_of("", "");

            // HALF TWO: each filter selects exactly the executions it names. The rows span
            // the filtering member, the form the instant is written in, and how many
            // executions the filter selects.
            let listing_cases: Vec<(&str, Value, Vec<String>)> = vec![
                (
                    "commandArn with a startedTimeFilter.after in the documented minute form",
                    json!({
                        "commandArn": command_arn(REBOOT),
                        "startedTimeFilter": { "after": minute_form(before_any_start) },
                    }),
                    id_of("", REBOOT),
                ),
                (
                    "the same window written to the second",
                    json!({
                        "commandArn": command_arn(REBOOT),
                        "startedTimeFilter": { "after": zulu(before_any_start) },
                    }),
                    id_of("", REBOOT),
                ),
                (
                    "the same instant written with a UTC offset",
                    json!({
                        "commandArn": command_arn(REBOOT),
                        "startedTimeFilter": { "after": offset_form(before_any_start) },
                    }),
                    id_of("", REBOOT),
                ),
                (
                    "targetArn selects one device, whatever command it ran",
                    json!({
                        "targetArn": target_arn(START_CASES[0].thing),
                        "startedTimeFilter": { "after": minute_form(before_any_start) },
                    }),
                    id_of(START_CASES[0].thing, ""),
                ),
                (
                    "no filter at all lists every command execution of the account",
                    json!({}),
                    every_id.clone(),
                ),
                (
                    "status CREATED, which every untaken execution rests at",
                    json!({ "status": "CREATED" }),
                    every_id.clone(),
                ),
                (
                    "status SUCCEEDED, which no untaken execution has reached",
                    json!({ "status": "SUCCEEDED" }),
                    Vec::new(),
                ),
                (
                    "namespace selects the commands created in it",
                    json!({ "namespace": FLEETWISE_NAMESPACE }),
                    id_of("", SIGNALS),
                ),
                (
                    "the other namespace selects the others",
                    json!({ "namespace": BASE_NAMESPACE }),
                    id_of("", REBOOT),
                ),
                (
                    "a window that closed an hour before any start selects nothing",
                    json!({
                        "startedTimeFilter": { "before": minute_form(before_any_start - 3_600) },
                    }),
                    Vec::new(),
                ),
                (
                    "a completed-time window selects nothing while nothing has completed",
                    json!({
                        "completedTimeFilter": { "after": minute_form(before_any_start - 3_600) },
                    }),
                    Vec::new(),
                ),
            ];

            for (label, request, expected) in &listing_cases {
                let (status, error_code, rendered, body) = list(request, "");
                assert_eq!(
                    (status, error_code.as_str()),
                    (200, ""),
                    "case {label:?}: ListCommandExecutions must answer 200, got {status} with \
                     {rendered}"
                );
                let actual = execution_ids(&body);
                assert_eq!(
                    sorted(&actual),
                    sorted(expected),
                    "case {label:?}: expected exactly {expected:?}, got {actual:?} from \
                     {rendered}"
                );
                assert!(
                    body.get("commandExecutions").is_some(),
                    "case {label:?}: a selection nothing matches is an EMPTY list, not an \
                     absent member, got {rendered}"
                );

                // Every entry is the SAME record the single-execution reader reports: the
                // two readers of the two planes' one store answer one value.
                for execution in started.iter().filter(|execution| {
                    expected.contains(&execution.execution_id)
                }) {
                    let summary = summary_of(&body, &execution.execution_id)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {label:?}: {} must be among {actual:?}",
                                execution.execution_id
                            )
                        })
                        .clone();
                    let (_, _, read_rendered, reported) =
                        read(&execution.execution_id, &target_arn(execution.thing));
                    for member in ["commandArn", "targetArn", "status", "createdAt", "startedAt"] {
                        assert_eq!(
                            summary.get(member),
                            reported.get(member),
                            "case {label:?}: the listed {member} of {} must be the one \
                             GetCommandExecution reports, got {summary} against {read_rendered}",
                            execution.execution_id
                        );
                    }
                    assert!(
                        summary.get("completedAt").is_none_or(Value::is_null),
                        "case {label:?}: an execution no device has finished reports no \
                         completedAt, got {summary}"
                    );
                }

                // Listing is a READ: the same request answers exactly the same thing.
                let (_, _, _, again) = list(request, "");
                assert_eq!(
                    again, body,
                    "case {label:?}: a second identical listing must answer what the first \
                     did, got {again} after {body}"
                );
            }

            // ORDER: one total order over the selection, descending by default
            // (`sortOrder`), and `ASCENDING` is its exact reverse.
            let unfiltered = json!({});
            let (_, _, descending_rendered, descending_body) = list(&unfiltered, "");
            let descending = execution_ids(&descending_body);
            let mut ascending_request = unfiltered.clone();
            ascending_request["sortOrder"] = json!("ASCENDING");
            let (_, _, ascending_rendered, ascending_body) = list(&ascending_request, "");
            let ascending = execution_ids(&ascending_body);
            assert_eq!(
                descending,
                ascending.iter().rev().cloned().collect::<Vec<String>>(),
                "case \"sortOrder\": the two orders must be exact reverses, got \
                 {descending_rendered} against {ascending_rendered}"
            );
            // The order the starts happened in is the order `ASCENDING` reports: each
            // execution's instant is when its start call created it, ties broken by the
            // store's enqueue order.
            assert_eq!(
                ascending, every_id,
                "case \"sortOrder\": ASCENDING lists the executions in the order they were \
                 started, got {ascending_rendered}"
            );

            // PAGING walks that one order, every execution exactly once.
            for page_size in [1_usize, 2, every_id.len(), every_id.len() + 1] {
                let mut walked: Vec<String> = Vec::new();
                let mut query = format!("?maxResults={page_size}");
                let mut pages = 0_usize;
                loop {
                    let (status, _, rendered, body) = list(&unfiltered, &query);
                    assert_eq!(
                        status, 200,
                        "case {page_size:?}: a page must answer 200, got {status} with {rendered}"
                    );
                    let page = execution_ids(&body);
                    assert!(
                        page.len() <= page_size,
                        "case {page_size:?}: a page holds at most maxResults entries, got \
                         {page:?}"
                    );
                    walked.extend(page);
                    pages += 1;
                    assert!(
                        pages <= every_id.len() + 1,
                        "case {page_size:?}: the walk must terminate, got {walked:?}"
                    );
                    match body.get("nextToken").and_then(Value::as_str) {
                        Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                        None => break,
                    }
                }
                assert_eq!(
                    walked, descending,
                    "case {page_size:?}: the pages must walk the whole listing in its order \
                     exactly once, got {walked:?}"
                );
            }

            // The pairs the operation documents as mutually exclusive are refused, and the
            // refusal leaves the listing exactly as it was.
            for (label, request) in [
                (
                    "commandArn and targetArn together",
                    json!({
                        "commandArn": command_arn(REBOOT),
                        "targetArn": target_arn(START_CASES[0].thing),
                    }),
                ),
                (
                    "both time filters together",
                    json!({
                        "startedTimeFilter": { "after": minute_form(before_any_start) },
                        "completedTimeFilter": { "after": minute_form(before_any_start) },
                    }),
                ),
                (
                    "a bound no calendar holds",
                    json!({ "startedTimeFilter": { "after": "the day before yesterday" } }),
                ),
            ] {
                let (status, error_code, rendered, _) = list(&request, "");
                assert_eq!(
                    (status, error_code.as_str()),
                    (400, "ValidationException"),
                    "case {label:?}: the request must be refused 400 ValidationException, got \
                     {status} with {rendered}"
                );
            }
            let (_, _, after_refusals, _) = list(&unfiltered, "");
            assert_eq!(
                after_refusals, descending_rendered,
                "a refused listing changes nothing, got {after_refusals} after \
                 {descending_rendered}"
            );
        })
        .expect("the cross-plane thread spawns")
        .join()
        .expect("the cross-plane thread finishes");
}

/// Which command a start may name, as the CONTROL plane decided it: the payload kind the
/// create fixed and the deprecation an update made.
struct AdmissionCase {
    case: &'static str,
    /// The device this row starts on — one per row, so what the row persisted (or did
    /// not) is readable on its own queue.
    thing: &'static str,
    /// Whether the command the row names is the static-payload one.
    static_payload: bool,
    /// Whether the command must be deprecated when the row runs.
    deprecated: bool,
    /// Whether the start carries `parameters`.
    parameters: bool,
    /// Whether the service serves the start.
    served: bool,
}

/// Every combination of the two facts the control plane records and the one choice the
/// start makes: a start is served unless the command is deprecated, or its payload is
/// static and the start supplies parameters.
const ADMISSION_CASES: &[AdmissionCase] = &[
    AdmissionCase {
        case: "a static-payload command, no parameters",
        thing: "admits-laser-1",
        static_payload: true,
        deprecated: false,
        parameters: false,
        served: true,
    },
    AdmissionCase {
        case: "a static-payload command, with parameters",
        thing: "admits-laser-2",
        static_payload: true,
        deprecated: false,
        parameters: true,
        served: false,
    },
    AdmissionCase {
        case: "a templated command, with parameters",
        thing: "admits-laser-3",
        static_payload: false,
        deprecated: false,
        parameters: true,
        served: true,
    },
    AdmissionCase {
        case: "a templated command, no parameters",
        thing: "admits-laser-4",
        static_payload: false,
        deprecated: false,
        parameters: false,
        served: true,
    },
    AdmissionCase {
        case: "the static-payload command once it is deprecated",
        thing: "admits-laser-5",
        static_payload: true,
        deprecated: true,
        parameters: false,
        served: false,
    },
    AdmissionCase {
        case: "the templated command once it is deprecated",
        thing: "admits-laser-6",
        static_payload: false,
        deprecated: true,
        parameters: true,
        served: false,
    },
    AdmissionCase {
        case: "the templated command once the deprecation is taken back",
        thing: "admits-laser-7",
        static_payload: false,
        deprecated: false,
        parameters: true,
        served: true,
    },
    AdmissionCase {
        case: "the static-payload command once the deprecation is taken back",
        thing: "admits-laser-8",
        static_payload: true,
        deprecated: false,
        parameters: false,
        served: true,
    },
];

#[test]
fn a_start_is_served_only_in_the_form_the_created_command_admits() {
    std::thread::Builder::new()
        .name("iot-command-execution-admission".to_string())
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
            let control = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://iot.{REGION}.amazonaws.com{path}"),
                    "iot",
                    body,
                )
            };
            let device = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://data.jobs.iot.{REGION}.amazonaws.com{path}"),
                    "iot-jobs-data",
                    body,
                )
            };

            // The STATIC command carries a literal payload; the other carries the
            // template whose placeholders a start's parameters fill.
            const STATIC_COMMAND: &str = "admits-static-reboot";
            const TEMPLATED_COMMAND: &str = "admits-templated-reboot";
            for (command_id, body) in [
                (
                    STATIC_COMMAND,
                    json!({
                        // `CommandPayload$content` is a blob, which restJson1 writes
                        // base64: `{"reboot": true}`.
                        "payload": {
                            "content": "eyJyZWJvb3QiOiB0cnVlfQ==",
                            "contentType": "application/json",
                        },
                    }),
                ),
                (
                    TEMPLATED_COMMAND,
                    json!({ "payloadTemplate": "{\"mode\":\"${mode}\"}" }),
                ),
            ] {
                let (status, _, rendered, _) =
                    control("PUT", format!("/commands/{command_id}"), body.to_string());
                assert_eq!(
                    status, 200,
                    "case {command_id:?}: CreateCommand must answer 200, got {status} with \
                     {rendered}"
                );
            }

            // The deprecation state each command is currently in, so a row only writes
            // it when it has to move.
            let mut deprecated: [bool; 2] = [false, false];
            let mut served_ids: Vec<(&str, String)> = Vec::new();

            for case in ADMISSION_CASES {
                let label = case.case;
                let command_id = if case.static_payload {
                    STATIC_COMMAND
                } else {
                    TEMPLATED_COMMAND
                };
                let slot = usize::from(!case.static_payload);
                if deprecated[slot] != case.deprecated {
                    let (status, _, rendered, updated) = control(
                        "PATCH",
                        format!("/commands/{command_id}"),
                        json!({ "deprecated": case.deprecated }).to_string(),
                    );
                    assert_eq!(
                        (status, updated["deprecated"].as_bool()),
                        (200, Some(case.deprecated)),
                        "case {label:?}: UpdateCommand must record the deprecation, got {status} \
                         with {rendered}"
                    );
                    // A deprecated command still EXISTS and is still readable: the flag
                    // is about what may be started, not about the resource.
                    let (status, _, rendered, read) =
                        control("GET", format!("/commands/{command_id}"), String::new());
                    assert_eq!(
                        (status, read["deprecated"].as_bool()),
                        (200, Some(case.deprecated)),
                        "case {label:?}: the command must still be readable with its flag, got \
                         {status} with {rendered}"
                    );
                    deprecated[slot] = case.deprecated;
                }

                let mut request = json!({
                    "targetArn": target_arn(case.thing),
                    "commandArn": command_arn(command_id),
                });
                if case.parameters {
                    request["parameters"] = json!({ "mode": { "S": "fast" } });
                }
                let (status, error_code, rendered, parsed) = device(
                    "POST",
                    "/command-executions".to_string(),
                    request.to_string(),
                );

                if case.served {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: the start must be served, got {status} with {rendered}"
                    );
                    let execution_id = parsed["executionId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        !execution_id.is_empty(),
                        "case {label:?}: a served start mints an executionId, got {rendered}"
                    );
                    // The served start is one the control plane reports, so what the two
                    // planes hold is one execution.
                    let (status, _, read_rendered, reported) = control(
                        "GET",
                        format!(
                            "/command-executions/{execution_id}?targetArn={}",
                            form_encode(&target_arn(case.thing))
                        ),
                        String::new(),
                    );
                    assert_eq!(
                        (status, reported["status"].as_str()),
                        (200, Some("CREATED")),
                        "case {label:?}: the served execution must be readable, got {status} \
                         with {read_rendered}"
                    );
                    served_ids.push((command_id, execution_id));
                } else {
                    assert_eq!(
                        (status, error_code.as_str()),
                        (400, "ValidationException"),
                        "case {label:?}: the start must be refused 400 ValidationException, got \
                         {status} with {rendered}"
                    );
                    assert!(
                        parsed["executionId"].is_null(),
                        "case {label:?}: a refusal mints no executionId, got {rendered}"
                    );
                    // A refusal persists NOTHING: the device's queue stays empty.
                    let (status, _, queue_rendered, queue) = device(
                        "GET",
                        format!("/things/{}/jobs", case.thing),
                        String::new(),
                    );
                    assert_eq!(
                        (status, queue["queuedJobs"].as_array().map(Vec::len)),
                        (200, Some(0)),
                        "case {label:?}: a refused start queues nothing on the device, got \
                         {status} with {queue_rendered}"
                    );
                }
            }

            // The account's command executions are exactly the ones the served starts
            // minted: the refusals left nothing behind, on either plane.
            for command_id in [STATIC_COMMAND, TEMPLATED_COMMAND] {
                let (status, _, rendered, body) = control(
                    "POST",
                    "/command-executions".to_string(),
                    json!({ "commandArn": command_arn(command_id) }).to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {command_id:?}: ListCommandExecutions must answer 200, got {status} \
                     with {rendered}"
                );
                let expected = sorted(
                    &served_ids
                        .iter()
                        .filter(|(served_for, _)| *served_for == command_id)
                        .map(|(_, execution_id)| execution_id.clone())
                        .collect::<Vec<String>>(),
                );
                let listed = sorted(&execution_ids(&body));
                assert_eq!(
                    listed, expected,
                    "case {command_id:?}: expected exactly the served executions {expected:?}, \
                     got {listed:?} from {rendered}"
                );
            }
        })
        .expect("the admission thread spawns")
        .join()
        .expect("the admission thread finishes");
}


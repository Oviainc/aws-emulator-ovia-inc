// SPDX-License-Identifier: CC0-1.0

//! The IoT job lifecycle across TWO services: the control plane creates a job that
//! TARGETS a device (`iot` `CreateJob`), and the device then polls, starts, reports
//! and finishes that job through the Jobs data plane (`iot-jobs-data`).
//!
//! This suite needs the WHOLE router: the job execution is created by one crate and
//! served by another, and the point under test is that it is ONE execution — a
//! single-service test cannot see the hand-off at all.
//!
//! The rule under test: an execution exists for a (thing, job) pair because a job
//! targeted that thing, and from then on every data-plane operation computes its
//! answer from that one record — `GetPendingJobExecutions` partitions it on its
//! status, `StartNextPendingJobExecution` moves it to `IN_PROGRESS` and hands back
//! the job document, `DescribeJobExecution` reports it until it finishes and then
//! refuses it 410 `TerminalStateException`, and `UpdateJobExecution` applies each
//! device-reported status, bumping the version every time, until a terminal one, after
//! which it refuses 409 `InvalidStateTransitionException`.
//!
//! Assembling the router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests are driven — on an explicitly sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

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

/// One row of the hand-off table: a job, the devices it targets and the document it
/// carries.
struct JobCase {
    case: &'static str,
    job_id: &'static str,
    /// The devices the job targets. The first is the one driven through the whole
    /// lifecycle; the others must see their own execution of the same job.
    things: &'static [&'static str],
    /// The `document` the job is created with, when it carries one.
    document: Option<&'static str>,
    /// `timeoutConfig.inProgressTimeoutInMinutes`, when the row carries one: once the
    /// device starts the execution, the data plane must report a remaining timeout.
    in_progress_timeout_minutes: Option<i64>,
    /// The statuses the device reports before finishing, in order.
    progress_statuses: &'static [&'static str],
    /// The terminal status the device finishes with.
    terminal_status: &'static str,
}

/// The rows differ in how many devices one job targets, in the form and presence of
/// the job document, in whether the job sets an in-progress timeout, in how many
/// progress updates the device sends, and in which terminal status it finishes with.
const JOB_CASES: &[JobCase] = &[
    JobCase {
        case: "one device, one progress update, succeeds",
        job_id: "handoff-reboot",
        things: &["handoff-laser-1"],
        document: Some(r#"{"operation":"reboot"}"#),
        in_progress_timeout_minutes: Some(10),
        progress_statuses: &["IN_PROGRESS"],
        terminal_status: "SUCCEEDED",
    },
    JobCase {
        case: "two devices, two progress updates, fails",
        job_id: "handoff-install",
        things: &["handoff-laser-2", "handoff-laser-3"],
        document: Some(r#"{"operation":"install","version":"3"}"#),
        in_progress_timeout_minutes: None,
        progress_statuses: &["IN_PROGRESS", "IN_PROGRESS"],
        terminal_status: "FAILED",
    },
    JobCase {
        case: "three devices, no document, rejected",
        job_id: "handoff-from-source",
        things: &["handoff-laser-4", "handoff-laser-5", "handoff-laser-6"],
        document: None,
        in_progress_timeout_minutes: Some(1),
        progress_statuses: &[],
        terminal_status: "REJECTED",
    },
];

/// The `jobId`s of one pending list, in the order the service returned them.
fn job_ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| {
            summary
                .get("jobId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[test]
fn a_job_that_targets_a_thing_is_the_execution_its_device_polls_for() {
    std::thread::Builder::new()
        .name("iot-job-execution-handoff".to_string())
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
            // The Jobs data plane, signed for `iot-jobs-data`.
            let device = |method: &str, path: String, body: String| {
                call(
                    method,
                    format!("https://data.jobs.iot.{REGION}.amazonaws.com{path}"),
                    "iot-jobs-data",
                    body,
                )
            };

            for case in JOB_CASES {
                let label = case.case;

                for thing in case.things {
                    let (status, _, rendered, _) =
                        control("POST", format!("/things/{thing}"), "{}".to_string());
                    assert_eq!(
                        status, 200,
                        "case {label:?}: registering {thing} must answer 200, got {status} \
                         with {rendered}"
                    );
                    // Before the job exists the device has nothing to do, which is an
                    // empty answer and not an error.
                    let (status, _, rendered, parsed) =
                        device("GET", format!("/things/{thing}/jobs"), String::new());
                    assert_eq!(
                        status, 200,
                        "case {label:?}: an untargeted device still reports, got {status} \
                         with {rendered}"
                    );
                    assert!(
                        job_ids(&parsed["queuedJobs"]).is_empty()
                            && job_ids(&parsed["inProgressJobs"]).is_empty(),
                        "case {label:?}: an untargeted device has no executions, got {rendered}"
                    );
                }

                // Create the job that targets every device of this row.
                let mut request = json!({
                    "targets": case
                        .things
                        .iter()
                        .map(|thing| format!(
                            "arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing}"
                        ))
                        .collect::<Vec<String>>(),
                });
                match case.document {
                    Some(document) => request["document"] = json!(document),
                    None => {
                        request["documentSource"] =
                            json!("https://s3.us-east-1.amazonaws.com/firmware/v1")
                    }
                }
                if let Some(minutes) = case.in_progress_timeout_minutes {
                    request["timeoutConfig"] = json!({"inProgressTimeoutInMinutes": minutes});
                }
                let (status, _, rendered, parsed) = control(
                    "PUT",
                    format!("/jobs/{}", case.job_id),
                    request.to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: creating the job must answer 200, got {status} with {rendered}"
                );
                assert_eq!(
                    parsed.get("jobId").and_then(Value::as_str),
                    Some(case.job_id),
                    "case {label:?}: expected the created jobId, got {rendered}"
                );

                // EVERY targeted device now sees the job queued for it.
                for thing in case.things {
                    let (status, _, rendered, parsed) =
                        device("GET", format!("/things/{thing}/jobs"), String::new());
                    assert_eq!(
                        status, 200,
                        "case {label:?}: {thing} must be able to poll, got {status} with {rendered}"
                    );
                    assert_eq!(
                        job_ids(&parsed["queuedJobs"]),
                        vec![case.job_id.to_string()],
                        "case {label:?}: expected the job queued for {thing}, got {rendered}"
                    );
                    let summary = &parsed["queuedJobs"][0];
                    assert_eq!(
                        (
                            summary["versionNumber"].as_i64(),
                            summary["executionNumber"].as_i64()
                        ),
                        (Some(1), Some(1)),
                        "case {label:?}: a created execution is version 1, execution 1, \
                         got {rendered}"
                    );
                    assert!(
                        summary["queuedAt"].as_i64().unwrap_or_default() > 0
                            && summary["startedAt"].is_null(),
                        "case {label:?}: expected a queue time and no start time for {thing}, \
                         got {rendered}"
                    );
                }

                // The first device of the row drives the whole lifecycle.
                let thing = case.things[0];

                let (status, _, rendered, parsed) = device(
                    "PUT",
                    format!("/things/{thing}/jobs/$next"),
                    json!({"statusDetails": {"stage": "download"}}).to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: $next must answer 200, got {status} with {rendered}"
                );
                let execution = &parsed["execution"];
                assert_eq!(
                    execution["jobId"].as_str(),
                    Some(case.job_id),
                    "case {label:?}: $next must hand back the targeted job, got {rendered}"
                );
                assert_eq!(
                    (
                        execution["status"].as_str(),
                        execution["versionNumber"].as_i64(),
                        execution["executionNumber"].as_i64()
                    ),
                    (Some("IN_PROGRESS"), Some(2), Some(1)),
                    "case {label:?}: starting it moves the status and the version, got {rendered}"
                );
                assert_eq!(
                    execution["jobDocument"].as_str(),
                    case.document,
                    "case {label:?}: $next must hand back the job's document, got {rendered}"
                );
                assert_eq!(
                    execution["approximateSecondsBeforeTimedOut"].is_null(),
                    case.in_progress_timeout_minutes.is_none(),
                    "case {label:?}: the in-progress timeout is reported exactly when the job \
                     set one, got {rendered}"
                );

                // It has moved from the queued partition to the in-progress one.
                let (_, _, rendered, parsed) =
                    device("GET", format!("/things/{thing}/jobs"), String::new());
                assert!(
                    job_ids(&parsed["queuedJobs"]).is_empty(),
                    "case {label:?}: a started execution leaves the queue, got {rendered}"
                );
                assert_eq!(
                    job_ids(&parsed["inProgressJobs"]),
                    vec![case.job_id.to_string()],
                    "case {label:?}: expected the execution in progress, got {rendered}"
                );
                assert_eq!(
                    parsed["inProgressJobs"][0]["versionNumber"].as_i64(),
                    Some(2),
                    "case {label:?}: the summary carries the bumped version, got {rendered}"
                );

                // And the device can read it back, document and all.
                let (status, _, rendered, parsed) = device(
                    "GET",
                    format!(
                        "/things/{thing}/jobs/{}?includeJobDocument=true",
                        case.job_id
                    ),
                    String::new(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: describing a live execution must answer 200, got {status} \
                     with {rendered}"
                );
                assert_eq!(
                    (
                        parsed["execution"]["status"].as_str(),
                        parsed["execution"]["versionNumber"].as_i64(),
                        parsed["execution"]["statusDetails"]["stage"].as_str(),
                        parsed["execution"]["jobDocument"].as_str(),
                    ),
                    (Some("IN_PROGRESS"), Some(2), Some("download"), case.document),
                    "case {label:?}: expected the started execution read back, got {rendered}"
                );

                // Each device-reported status is applied and bumps the version again.
                let mut version = 2_i64;
                for (index, status_value) in case.progress_statuses.iter().enumerate() {
                    version += 1;
                    let (status, _, rendered, parsed) = device(
                        "POST",
                        format!("/things/{thing}/jobs/{}", case.job_id),
                        json!({
                            "status": status_value,
                            "statusDetails": {"stage": format!("step-{index}")},
                            "expectedVersion": version - 1,
                            "includeJobExecutionState": true,
                        })
                        .to_string(),
                    );
                    assert_eq!(
                        status, 200,
                        "case {label:?}: update {index} must answer 200, got {status} \
                         with {rendered}"
                    );
                    assert_eq!(
                        (
                            parsed["executionState"]["status"].as_str(),
                            parsed["executionState"]["versionNumber"].as_i64(),
                            parsed["executionState"]["statusDetails"]["stage"].as_str(),
                        ),
                        (
                            Some(*status_value),
                            Some(version),
                            Some(format!("step-{index}").as_str())
                        ),
                        "case {label:?}: update {index} must echo the state it stored, \
                         got {rendered}"
                    );
                }

                // The terminal update returns the post-update state and the document.
                version += 1;
                let (status, _, rendered, parsed) = device(
                    "POST",
                    format!("/things/{thing}/jobs/{}", case.job_id),
                    json!({
                        "status": case.terminal_status,
                        "statusDetails": {"stage": "verify"},
                        "includeJobExecutionState": true,
                        "includeJobDocument": true,
                    })
                    .to_string(),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: finishing must answer 200, got {status} with {rendered}"
                );
                assert_eq!(
                    (
                        parsed["executionState"]["status"].as_str(),
                        parsed["executionState"]["versionNumber"].as_i64(),
                        parsed["executionState"]["statusDetails"]["stage"].as_str(),
                        parsed["jobDocument"].as_str(),
                    ),
                    (
                        Some(case.terminal_status),
                        Some(version),
                        Some("verify"),
                        case.document
                    ),
                    "case {label:?}: the terminal update must report the state it stored, \
                     got {rendered}"
                );

                // A finished execution is in neither pending list, is refused 410 by
                // the read and 409 by a further update — the two refusals a client
                // branches on.
                let (_, _, rendered, parsed) =
                    device("GET", format!("/things/{thing}/jobs"), String::new());
                assert!(
                    job_ids(&parsed["queuedJobs"]).is_empty()
                        && job_ids(&parsed["inProgressJobs"]).is_empty(),
                    "case {label:?}: a finished execution is in neither list, got {rendered}"
                );

                let (status, error_code, rendered, _) = device(
                    "GET",
                    format!("/things/{thing}/jobs/{}", case.job_id),
                    String::new(),
                );
                assert_eq!(
                    (status, error_code.as_str()),
                    (410, "TerminalStateException"),
                    "case {label:?}: a finished execution is refused as terminal, got {status} \
                     {error_code} with {rendered}"
                );

                let (status, error_code, rendered, _) = device(
                    "POST",
                    format!("/things/{thing}/jobs/{}", case.job_id),
                    json!({"status": "IN_PROGRESS"}).to_string(),
                );
                assert_eq!(
                    (status, error_code.as_str()),
                    (409, "InvalidStateTransitionException"),
                    "case {label:?}: updating a finished execution is a conflict, got {status} \
                     {error_code} with {rendered}"
                );

                // An execution the store never held is still 404, so "finished" and
                // "no such execution" stay distinguishable.
                let (status, error_code, rendered, _) = device(
                    "GET",
                    format!("/things/{thing}/jobs/never-created"),
                    String::new(),
                );
                assert_eq!(
                    (status, error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {label:?}: an execution that never existed is not found, got {status} \
                     {error_code} with {rendered}"
                );

                // The row's OTHER devices are untouched by all of that: their own
                // execution of the same job is still queued.
                for other in case.things.iter().skip(1) {
                    let (_, _, rendered, parsed) =
                        device("GET", format!("/things/{other}/jobs"), String::new());
                    assert_eq!(
                        job_ids(&parsed["queuedJobs"]),
                        vec![case.job_id.to_string()],
                        "case {label:?}: {other} must still hold its own queued execution, \
                         got {rendered}"
                    );
                }
            }
        })
        .expect("spawn the job-execution hand-off thread")
        .join()
        .expect("a job that targets a thing must reach that thing's device");
}

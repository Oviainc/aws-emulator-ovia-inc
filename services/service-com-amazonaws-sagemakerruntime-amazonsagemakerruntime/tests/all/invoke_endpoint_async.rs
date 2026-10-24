// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `InvokeEndpointAsync`, SageMaker's asynchronous inference
//! data plane.
//!
//! Every case goes over real HTTP through the same `MultiServiceRouter` the production
//! router assembles, with both SageMaker services mounted on ONE shared engine (see
//! [`crate::harness`]): the control plane that owns the endpoint and its
//! `AsyncInferenceConfig`, and this data plane that queues submissions against it.
//!
//! The assertions are RULES over TABLES of inputs, never one recorded call:
//!
//! * the two reported locations are computed from the endpoint's own
//!   `AsyncInferenceConfig$OutputConfig` paths plus the request's `S3OutputPathExtension`
//!   / `Filename` and the inference id, so the same submission always reports the same
//!   strings and a different request member moves them;
//! * `FailureLocation` is reported only where the endpoint configures a failure path;
//! * a submission is refused (`ValidationError`, HTTP 400) when the account hosts no such
//!   endpoint, when no deployment is behind it, when its configuration carries no
//!   `AsyncInferenceConfig`, and when it carries neither or both of `Body` and
//!   `InputLocation`;
//! * a header outside its modelled range is refused by the framework's constraint walk;
//! * an accepted submission is queued `InProgress` and leaves that status for the modelled
//!   failure state once its outcome falls due, because no inference container ran it;
//! * the queue of accepted submissions is bounded, dropping the oldest at the cap.

use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::{
    AsyncInvocationStatus, InvocationOutcome, ASYNC_OUTCOME_DUE_AFTER,
    MAX_QUEUED_ASYNC_INVOCATIONS, NO_INFERENCE_CONTAINER,
};

use crate::harness::{
    async_inference_config, async_invocations, async_queue, post, queued, settle_queue, Emulator,
    RequestCase, SubmissionCase, ACCOUNT, INPUT_LOCATION as INPUT,
};

/// Where the seeded deployments write their asynchronous results.
const OUTPUT_PATH: &str = "s3://aws-emulator-sagemaker/async/output";
const FAILURE_PATH: &str = "s3://aws-emulator-sagemaker/async/failure";

/// RULE: the reported `OutputLocation` and `FailureLocation` are computed from the
/// endpoint's configured asynchronous-inference paths plus the request's
/// `S3OutputPathExtension` / `Filename` and the inference id — so each of those members
/// moves the location, and nothing else does.
#[tokio::test]
async fn the_locations_are_computed_from_the_configured_paths_and_the_request() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "async-ep",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, Some(FAILURE_PATH)),
        )
        .await;

    // Rows differ along the members the rule generalises over: the inference id, the
    // appended path extension, and the response filename.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 6] = [
        ("req-1", None, None, "async/output/req-1.out"),
        ("req-2", None, None, "async/output/req-2.out"),
        ("req-3", Some("v2"), None, "async/output/v2/req-3.out"),
        ("req-4", Some("a/b"), None, "async/output/a/b/req-4.out"),
        ("req-5", None, Some("answer.json"), "async/output/answer.json"),
        (
            "req-6",
            Some("v2"),
            Some("answer.json"),
            "async/output/v2/answer.json",
        ),
    ];
    for (inference_id, extension, filename, expected_tail) in cases {
        let mut headers = vec![INPUT, ("x-amzn-sagemaker-inference-id", inference_id)];
        if let Some(extension) = extension {
            headers.push(("x-amzn-sagemaker-s3outputpathextension", extension));
        }
        if let Some(filename) = filename {
            headers.push(("x-amzn-sagemaker-filename", filename));
        }
        let call = post(&emulator, &async_invocations("async-ep"), &headers, b"").await;
        assert_eq!(
            call.status,
            202,
            "case {inference_id:?} {extension:?} {filename:?}: expected HTTP 202, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let reported = |header: &str| {
            call.headers
                .get(header)
                .map(|value| value.to_str().unwrap_or_default().to_string())
        };
        let output = reported("x-amzn-sagemaker-outputlocation");
        assert_eq!(
            output.as_deref(),
            Some(format!("s3://aws-emulator-sagemaker/{expected_tail}").as_str()),
            "case {inference_id:?} {extension:?} {filename:?}: the output location must be the \
             configured path plus the request's extension and filename, got {output:?}"
        );
        let failure = reported("x-amzn-sagemaker-failurelocation");
        assert_eq!(
            failure.as_deref(),
            Some(format!("{FAILURE_PATH}/{inference_id}-error.out").as_str()),
            "case {inference_id:?}: the failure location must be the configured failure path, \
             got {failure:?}"
        );
        assert_eq!(
            call.json()["InferenceId"].as_str(),
            Some(inference_id),
            "case {inference_id:?}: the response must report the submitted inference id, got {}",
            call.json()
        );
        // The record keeps the SAME strings the response reported.
        let record = queued(&emulator, inference_id)
            .unwrap_or_else(|| panic!("case {inference_id:?}: the submission must be queued"));
        assert_eq!(
            Some(record.output_location.as_str()),
            output.as_deref(),
            "case {inference_id:?}: the queued record must hold the reported output location, \
             got {:?}",
            record.output_location
        );
        assert_eq!(
            record.failure_location.as_deref(),
            failure.as_deref(),
            "case {inference_id:?}: the queued record must hold the reported failure location, \
             got {:?}",
            record.failure_location
        );
    }
}

/// RULE: the locations are a function of the request and the endpoint's configuration
/// alone, so submitting one inference id twice reports the same strings, and an endpoint
/// configured without a failure path reports no `FailureLocation` rather than one nothing
/// is configured to write.
#[tokio::test]
async fn one_submission_reports_stable_locations_and_an_unconfigured_path_is_omitted() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "with-failure",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, Some(FAILURE_PATH)),
        )
        .await;
    emulator
        .deploy(
            "no-failure",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    for endpoint in ["with-failure", "no-failure"] {
        let headers = [INPUT, ("x-amzn-sagemaker-inference-id", "steady")];
        let first = post(&emulator, &async_invocations(endpoint), &headers, b"").await;
        let again = post(&emulator, &async_invocations(endpoint), &headers, b"").await;
        for (label, call) in [("first", &first), ("resubmitted", &again)] {
            assert_eq!(
                call.status, 202,
                "case {endpoint} {label}: expected HTTP 202, got {}",
                call.status
            );
        }
        let location = |call: &crate::harness::Call, header: &str| {
            call.headers
                .get(header)
                .map(|value| value.to_str().unwrap_or_default().to_string())
        };
        assert_eq!(
            location(&first, "x-amzn-sagemaker-outputlocation"),
            location(&again, "x-amzn-sagemaker-outputlocation"),
            "case {endpoint}: resubmitting one inference id must report the same output location"
        );
        let failure = location(&first, "x-amzn-sagemaker-failurelocation");
        match endpoint {
            "with-failure" => assert_eq!(
                failure.as_deref(),
                Some(format!("{FAILURE_PATH}/steady-error.out").as_str()),
                "case {endpoint}: a configured failure path must be reported, got {failure:?}"
            ),
            _ => assert_eq!(
                failure, None,
                "case {endpoint}: no failure path is configured, so none may be reported, got \
                 {failure:?}"
            ),
        }
    }
}

/// RULE: a submission the service cannot queue is refused with the modelled
/// `ValidationError` (HTTP 400) — never accepted with a 202 that promises an output
/// location nothing could write — and the refusal is logged in this service's own state.
#[tokio::test]
async fn a_submission_that_cannot_be_queued_is_refused() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "ready",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, Some(FAILURE_PATH)),
        )
        .await;
    // Deployed, in service, but its configuration carries no AsyncInferenceConfig.
    emulator.deploy("realtime-only", &["AllTraffic"], json!({})).await;
    // Deployed and still provisioning: no deployment is behind it yet.
    emulator
        .deploy_without_waiting(
            "provisioning",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // Rows differ along WHY the submission cannot be queued.
    let cases: [SubmissionCase; 5] = [
        (
            "no such endpoint",
            "nosuchendpoint",
            vec![INPUT],
            b"",
            InvocationOutcome::EndpointNotFound,
        ),
        (
            "still provisioning",
            "provisioning",
            vec![INPUT],
            b"",
            InvocationOutcome::EndpointNotInService,
        ),
        (
            "no asynchronous configuration",
            "realtime-only",
            vec![INPUT],
            b"",
            InvocationOutcome::AsyncInferenceNotConfigured,
        ),
        (
            "neither Body nor InputLocation",
            "ready",
            vec![],
            b"",
            InvocationOutcome::NoInput,
        ),
        (
            "both Body and InputLocation",
            "ready",
            vec![INPUT],
            b"{\"instances\":[1,2,3]}",
            InvocationOutcome::InputAmbiguous,
        ),
    ];
    for (case, endpoint, headers, body, outcome) in cases {
        let before = async_queue(&emulator).len();
        let call = post(&emulator, &async_invocations(endpoint), &headers, body).await;
        assert_eq!(
            call.status,
            400,
            "case {case}: expected HTTP 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_code(),
            "ValidationError",
            "case {case}: expected ValidationError, got {:?}",
            call.error_code()
        );
        assert_eq!(
            async_queue(&emulator).len(),
            before,
            "case {case}: a refused submission must queue nothing"
        );
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case {case}: the refusal must be logged"));
        assert_eq!(
            logged.outcome,
            outcome,
            "case {case}: expected the {} outcome to be logged, got {}",
            outcome.label(),
            logged.outcome.label()
        );
    }
}

/// RULE: an inference id is the caller's to choose; where the caller supplies none the
/// service generates one ("Amazon SageMaker AI will generate an identifier for you if none
/// is specified"), reports it, and keys the queued record by it.
#[tokio::test]
async fn a_generated_inference_id_is_reported_and_keys_the_record() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "generated",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    let mut seen = Vec::new();
    for round in 0..3 {
        let call = post(
            &emulator,
            &async_invocations("generated"),
            &[INPUT],
            b"",
        )
        .await;
        assert_eq!(
            call.status, 202,
            "case round {round}: expected HTTP 202, got {}",
            call.status
        );
        let reported = call.json()["InferenceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !reported.is_empty(),
            "case round {round}: a generated inference id must be reported, got {}",
            call.json()
        );
        let record = queued(&emulator, &reported).unwrap_or_else(|| {
            panic!("case round {round}: the record must be keyed by the reported id {reported:?}")
        });
        assert!(
            record.output_location.contains(&reported),
            "case round {round}: the output location must be named for the inference id, got {:?}",
            record.output_location
        );
        assert!(
            !seen.contains(&reported),
            "case round {round}: a generated inference id must be unique, saw {reported:?} twice"
        );
        // The data-plane log keeps the id the RESPONSE reported, so the two views of one
        // submission can be read against each other.
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case round {round}: the submission must be logged"));
        assert_eq!(
            logged.inference_id.as_deref(),
            Some(reported.as_str()),
            "case round {round}: the log must carry the reported inference id, got {:?}",
            logged.inference_id
        );
        seen.push(reported);
    }
}

/// RULE: queueing a submission is work this service really does, so it is accepted
/// `InProgress`; performing the inference is not, because a model container is
/// customer-provided code this emulator does not run — so once the outcome falls due the
/// record leaves `InProgress` for the modelled failure state with a reason that says so,
/// and never for a `Completed` claiming an output object nothing wrote.
#[tokio::test]
async fn a_queued_submission_fails_once_its_outcome_is_due() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "queued",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, Some(FAILURE_PATH)),
        )
        .await;

    // Rows differ along the form of the input the submission carried.
    let cases: [RequestCase; 2] = [
        ("an S3 input location", vec![INPUT], b""),
        ("an inline body", vec![], b"{\"instances\":[1,2,3]}"),
    ];
    for (case, headers, body) in cases {
        let inference_id = format!("due-{}", case.replace(' ', "-"));
        let mut headers = headers;
        headers.push(("x-amzn-sagemaker-inference-id", &inference_id));
        let submitted = SystemTime::now();
        let call = post(&emulator, &async_invocations("queued"), &headers, body).await;
        assert_eq!(
            call.status, 202,
            "case {case}: expected HTTP 202, got {}",
            call.status
        );

        let record = queued(&emulator, &inference_id)
            .unwrap_or_else(|| panic!("case {case}: the submission must be queued"));
        assert_eq!(
            record.status,
            AsyncInvocationStatus::InProgress,
            "case {case}: a submission whose outcome is not yet due must read InProgress, got \
             {:?}",
            record.status
        );
        assert_eq!(
            record.outcome_due_at,
            record.submitted_at + ASYNC_OUTCOME_DUE_AFTER,
            "case {case}: the outcome must fall due exactly ASYNC_OUTCOME_DUE_AFTER the call \
             that queued it (submitted_at {:?}), got {:?}",
            record.submitted_at,
            record.outcome_due_at
        );
        assert!(
            record.submitted_at >= submitted,
            "case {case}: the record must be stamped at or after the call that queued it \
             ({submitted:?}), got {:?}",
            record.submitted_at
        );
        assert_eq!(
            record.inline_input_bytes,
            (!body.is_empty()).then_some(body.len()),
            "case {case}: only the inline body's SIZE is kept (the member is @sensitive), got {:?}",
            record.inline_input_bytes
        );

        // The outcome is owed by the CLOCK, not by a reader: settling at an instant past
        // the due one materialises exactly what was due then.
        settle_queue(&emulator, record.outcome_due_at + Duration::from_millis(1));
        let settled = queued(&emulator, &inference_id)
            .unwrap_or_else(|| panic!("case {case}: the record must still be queued"));
        assert_eq!(
            settled.status,
            AsyncInvocationStatus::Failed,
            "case {case}: a due submission nothing performed must read Failed, got {:?}",
            settled.status
        );
        assert_eq!(
            settled.failure_reason.as_deref(),
            Some(NO_INFERENCE_CONTAINER),
            "case {case}: the failure must say no container ran it, got {:?}",
            settled.failure_reason
        );
    }
}

/// RULE: the reported location is the endpoint's configured output path with the request's
/// `S3OutputPathExtension` and `Filename` spliced in LITERALLY, as an S3 key is — dots and
/// all, since nothing resolves the key — and the one value refused (HTTP 400, nothing
/// queued) is a `Filename` the member's own modelled pattern does not admit.
#[tokio::test]
async fn a_filename_the_member_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "escape",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // Rows differ along WHICH rule of `Filename`'s pattern
    // (`^(?!.*\..*\.)[a-zA-Z0-9][a-zA-Z0-9-_\.]*$`, smithy_query
    // operation=InvokeEndpointAsync) the value breaks: the character class, the
    // first-character class, and the negative lookahead that admits at most one dot. All
    // reach this service's own check, because the framework's regex-based constraint walk
    // cannot enforce that lookahead.
    let cases: [(&str, &str); 5] = [
        ("a path separator", "sub/answer.json"),
        ("a parent segment", ".."),
        ("a current segment", "."),
        ("two dots", "a.b.c"),
        ("a leading separator character", "-answer.json"),
    ];
    for (case, value) in cases {
        let before = async_queue(&emulator).len();
        let headers = [INPUT, ("x-amzn-sagemaker-filename", value)];
        let call = post(&emulator, &async_invocations("escape"), &headers, b"").await;
        assert_eq!(
            call.status,
            400,
            "case {case} ({value:?}): a filename the member's pattern does not admit must be \
             refused, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert!(
            call.message().contains("Filename"),
            "case {case} ({value:?}): the refusal must name the member, got {:?}",
            call.message()
        );
        assert_eq!(
            async_queue(&emulator).len(),
            before,
            "case {case} ({value:?}): a refused submission must queue nothing"
        );
        let logged = emulator
            .last_logged()
            .unwrap_or_else(|| panic!("case {case}: the refusal must be logged"));
        assert_eq!(
            logged.outcome,
            InvocationOutcome::OutputLocationInvalid,
            "case {case}: expected the OutputLocationInvalid outcome, got {}",
            logged.outcome.label()
        );
    }

    // Every value the members DO admit is spliced in verbatim — including an extension's
    // `.` and `..` segments, which `S3OutputPathExtension`'s pattern admits and which
    // nothing here resolves.
    let accepted = [
        (
            "an extension with a dotted segment",
            "x-amzn-sagemaker-s3outputpathextension",
            "v2.1",
            "async/output/v2.1/",
        ),
        (
            "an extension with a current segment",
            "x-amzn-sagemaker-s3outputpathextension",
            ".",
            "async/output/./",
        ),
        (
            "an extension with a parent segment",
            "x-amzn-sagemaker-s3outputpathextension",
            "v2/..",
            "async/output/v2/../",
        ),
        (
            "a nested extension",
            "x-amzn-sagemaker-s3outputpathextension",
            "v2/today",
            "async/output/v2/today/",
        ),
        (
            // The one normalisation: a leading or trailing `/` and an empty segment are
            // dropped, so the extension joins the configured path with exactly one
            // separator and the reported key carries no `//`.
            "an extension with redundant separators",
            "x-amzn-sagemaker-s3outputpathextension",
            "/v2//today/",
            "async/output/v2/today/",
        ),
        (
            "a filename with an extension",
            "x-amzn-sagemaker-filename",
            "answer.json",
            "async/output/answer.json",
        ),
        (
            "a filename with inner separators",
            "x-amzn-sagemaker-filename",
            "answer-1_final.json",
            "async/output/answer-1_final.json",
        ),
    ];
    for (case, header, value, expected_tail) in accepted {
        let call = post(
            &emulator,
            &async_invocations("escape"),
            &[INPUT, (header, value)],
            b"",
        )
        .await;
        assert_eq!(
            call.status,
            202,
            "case {case} ({value:?}): an ordinary name must be accepted, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        let reported = call
            .headers
            .get("x-amzn-sagemaker-outputlocation")
            .map(|value| value.to_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        assert!(
            reported.contains(expected_tail),
            "case {case} ({value:?}): the value must be spliced in literally, got {reported:?}"
        );
    }
}

/// RULE: a header outside the range the model publishes for it is refused before any
/// state is read — `RequestTTLSeconds` is `range 60..21600` and
/// `InvocationTimeoutSeconds` is `range 1..3600` (smithy_query
/// operation=InvokeEndpointAsync) — and a value inside the range is accepted and stored.
#[tokio::test]
async fn a_header_outside_its_modelled_range_is_refused() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "bounded",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // Each published bound, and one past it on both sides.
    let cases: [(&str, &str, i64, bool); 8] = [
        ("x-amzn-sagemaker-requestttlseconds", "ttl below minimum", 59, false),
        ("x-amzn-sagemaker-requestttlseconds", "ttl at minimum", 60, true),
        ("x-amzn-sagemaker-requestttlseconds", "ttl at maximum", 21600, true),
        ("x-amzn-sagemaker-requestttlseconds", "ttl past maximum", 21601, false),
        ("x-amzn-sagemaker-invocationtimeoutseconds", "timeout below minimum", 0, false),
        ("x-amzn-sagemaker-invocationtimeoutseconds", "timeout at minimum", 1, true),
        ("x-amzn-sagemaker-invocationtimeoutseconds", "timeout at maximum", 3600, true),
        ("x-amzn-sagemaker-invocationtimeoutseconds", "timeout past maximum", 3601, false),
    ];
    for (header, case, value, accepted) in cases {
        let rendered = value.to_string();
        let inference_id = format!("bound-{case}").replace(' ', "-");
        let headers = [
            INPUT,
            (header, rendered.as_str()),
            ("x-amzn-sagemaker-inference-id", inference_id.as_str()),
        ];
        let call = post(&emulator, &async_invocations("bounded"), &headers, b"").await;
        if accepted {
            assert_eq!(
                call.status,
                202,
                "case {case} ({value}): a value inside the modelled range must be accepted, got \
                 {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            let record = queued(&emulator, &inference_id)
                .unwrap_or_else(|| panic!("case {case}: the submission must be queued"));
            let stored = if header.ends_with("ttlseconds") {
                record.request_ttl_seconds
            } else {
                record.invocation_timeout_seconds
            };
            assert_eq!(
                stored,
                Some(i32::try_from(value).expect("the row's value fits the modelled integer")),
                "case {case}: the accepted value must be stored with the submission, got {stored:?}"
            );
        } else {
            assert_eq!(
                call.status, 400,
                "case {case} ({value}): a value outside the modelled range must be refused, got {}",
                call.status
            );
        }
    }
}

/// RULE: the endpoint an asynchronous submission names is the control plane's resource, so
/// the data plane's refusal names the endpoint and the account that hosts it.
#[tokio::test]
async fn a_refusal_names_the_endpoint_and_the_account() {
    let emulator = Emulator::new();
    for endpoint in ["ghost", "a", "ep-with-hyphens-1"] {
        let call = post(&emulator, &async_invocations(endpoint), &[INPUT], b"").await;
        assert!(
            call.message().contains(endpoint) && call.message().contains(ACCOUNT),
            "case {endpoint}: the refusal must name the endpoint and the account, got {:?}",
            call.message()
        );
    }
}

/// RULE: the queue of accepted submissions is BOUNDED
/// ([`MAX_QUEUED_ASYNC_INVOCATIONS`]) — at the cap the OLDEST submission is dropped, never
/// a newer one, so a client submitting in a loop cannot grow this service's state without
/// end and the submissions a live workflow just made stay readable.
#[tokio::test]
async fn the_async_queue_keeps_the_most_recent_submissions_within_its_bound() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "bounded",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // Rows: the submission below the cap, the one AT it, and three past it — the
    // transition the bound is about. Each carries its own `InferenceId`, so which
    // submissions survived is unambiguous.
    let overshoot = 3;
    for sequence in 1..=(MAX_QUEUED_ASYNC_INVOCATIONS + overshoot) {
        let inference_id = format!("queued-{sequence}");
        let call = post(
            &emulator,
            &async_invocations("bounded"),
            &[INPUT, ("x-amzn-sagemaker-inference-id", &inference_id)],
            b"",
        )
        .await;
        assert_eq!(
            call.status, 202,
            "case {inference_id}: expected HTTP 202, got {}",
            call.status
        );
        // Reading the whole queue on every one of >1000 iterations would dominate the
        // test, so the rows asserted are the ones the bound is about: the last below the
        // cap, the one at it, and every one past it.
        if sequence + overshoot <= MAX_QUEUED_ASYNC_INVOCATIONS {
            continue;
        }
        let queue = async_queue(&emulator);
        assert_eq!(
            queue.len(),
            sequence.min(MAX_QUEUED_ASYNC_INVOCATIONS),
            "case {inference_id}: the queue holds at most {MAX_QUEUED_ASYNC_INVOCATIONS} \
             submissions, got {}",
            queue.len()
        );
        assert_eq!(
            queue.last().map(|record| record.inference_id.clone()),
            Some(inference_id.clone()),
            "case {inference_id}: the newest submission is the last entry, got {:?}",
            queue.last().map(|record| record.inference_id.clone())
        );
        // Once past the cap, the oldest surviving submission is the one that many back,
        // and the evicted ones are no longer readable by their id.
        let oldest_survivor = sequence.saturating_sub(MAX_QUEUED_ASYNC_INVOCATIONS) + 1;
        assert_eq!(
            queue.first().map(|record| record.inference_id.clone()),
            Some(format!("queued-{oldest_survivor}")),
            "case {inference_id}: the oldest submission is dropped at the cap, got {:?}",
            queue.first().map(|record| record.inference_id.clone())
        );
        if oldest_survivor > 1 {
            let evicted = format!("queued-{}", oldest_survivor - 1);
            assert!(
                queued(&emulator, &evicted).is_none(),
                "case {inference_id}: the evicted submission {evicted} must no longer be \
                 readable"
            );
        }
    }
}

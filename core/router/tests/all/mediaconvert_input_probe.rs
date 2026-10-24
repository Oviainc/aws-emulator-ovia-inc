// SPDX-License-Identifier: CC0-1.0

//! MediaConvert classifies a failed job from the input probe it actually performed.
//!
//! The rule under test: a job that reaches the terminal `ERROR` status reports an
//! integer `Job.ErrorCode` classifying what failed together with an `ErrorMessage`
//! describing it for that job — and where the failure is that one of the job's own
//! `Settings.Inputs[].FileInput` locations could not be opened, the code is
//! MediaConvert's input-probe failure code 1404 and the message names the input URI that
//! failed. A job whose inputs could all be read is not classified that way: the code is
//! absent, because what failed was not the probe.
//!
//! The probe is a REAL read against S3 over the in-process callback, so this suite needs
//! TWO services on one engine — MediaConvert to submit the jobs and S3 to hold (or not
//! hold) their inputs — which is why it lives here rather than in either service's own
//! crate. The rows span what the rule generalises over: a bucket this account does not
//! hold, a bucket it holds without the key, a key it holds, an input that is not an S3
//! location at all, and several inputs where only a later one is unreadable.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The IAM role ARN shape MediaConvert's `Role` member documents.
const ROLE: &str = "arn:aws:iam::123456789012:role/MediaConvertRole";

/// The bucket this suite creates, which therefore exists.
const BUCKET: &str = "mediaconvert-probe-bucket";

/// A bucket this account never creates, so S3 answers `NoSuchBucket` for it.
const ABSENT_BUCKET: &str = "mediaconvert-probe-absent-bucket";

/// MediaConvert's input-probe failure code, as the issue's recorded job reports it.
const INPUT_PROBE_FAILURE_CODE: i64 = 1404;

/// Comfortably past the span MediaConvert's emulator holds a new job in `SUBMITTED`
/// (`JOB_SETTLES_AFTER`, 750 ms), so every job below has reached its terminal status.
const PAST_THE_SETTLE: Duration = Duration::from_millis(2_500);

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
/// The signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Send one request through the router and return the status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one S3 request path-style (`/<bucket>/<key>`), which is how the emulator
/// addresses buckets.
fn s3(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, String) {
    send(
        router,
        runtime,
        "s3",
        method,
        uri,
        "application/octet-stream",
        body.to_vec(),
    )
}

/// Drive one MediaConvert request, returning the status and the parsed JSON body.
fn mediaconvert(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let payload = body
        .map(|json| json.to_string().into_bytes())
        .unwrap_or_default();
    let (status, text) = send(
        router,
        runtime,
        "mediaconvert",
        method,
        uri,
        "application/json",
        payload,
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// A `CreateJob` body whose `Settings.Inputs` are exactly `inputs`.
fn job_body(inputs: &[&str]) -> serde_json::Value {
    let inputs: Vec<serde_json::Value> = inputs
        .iter()
        .map(|uri| serde_json::json!({ "fileInput": uri }))
        .collect();
    serde_json::json!({
        "role": ROLE,
        "settings": {
            "inputs": inputs,
            "outputGroups": [{
                "name": "File Group",
                "outputs": [{ "nameModifier": "_out" }],
                "outputGroupSettings": {
                    "type": "FILE_GROUP_SETTINGS",
                    "fileGroupSettings": { "destination": "s3://mediaconvert-probe-bucket/out/" }
                }
            }]
        }
    })
}

/// One row: the inputs a job submits and the input URI (if any) the probe must find
/// unreadable.
struct Case {
    case: &'static str,
    inputs: Vec<String>,
    /// `Some(uri)` when the job must be classified as an input-probe failure naming
    /// `uri`; `None` when its inputs were all readable (or not probeable), so no code is
    /// reported.
    unreadable: Option<String>,
}

fn cases() -> Vec<Case> {
    let good = format!("s3://{BUCKET}/in/good.mp4");
    let missing_key = format!("s3://{BUCKET}/in/missing.mp4");
    let missing_bucket = format!("s3://{ABSENT_BUCKET}/in/a.mp4");
    vec![
        Case {
            case: "a bucket this account does not hold",
            inputs: vec![missing_bucket.clone()],
            unreadable: Some(missing_bucket.clone()),
        },
        Case {
            case: "a bucket it holds, without the key",
            inputs: vec![missing_key.clone()],
            unreadable: Some(missing_key.clone()),
        },
        Case {
            case: "a key it holds",
            inputs: vec![good.clone()],
            unreadable: None,
        },
        Case {
            case: "two readable inputs",
            inputs: vec![good.clone(), good.clone()],
            unreadable: None,
        },
        Case {
            case: "a readable input followed by an unreadable one",
            inputs: vec![good.clone(), missing_bucket.clone()],
            unreadable: Some(missing_bucket.clone()),
        },
        Case {
            case: "an input that is not an S3 location",
            inputs: vec!["https://example.com/in/a.mp4".to_string()],
            unreadable: None,
        },
    ]
}

#[test]
fn a_failed_job_reports_the_input_probe_failure_code_only_for_an_input_s3_could_not_read() {
    std::thread::Builder::new()
        .name("mediaconvert-input-probe".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The bucket and the one object that exists, so the rows below differ in
            // whether S3 really holds what the job names.
            let (status, body) = s3(&router, &runtime, "PUT", &format!("/{BUCKET}"), b"");
            assert!(
                (200..300).contains(&status),
                "setup: CreateBucket must succeed, got {status} {body}"
            );
            let (status, body) = s3(
                &router,
                &runtime,
                "PUT",
                &format!("/{BUCKET}/in/good.mp4"),
                b"not really a movie",
            );
            assert!(
                (200..300).contains(&status),
                "setup: PutObject must succeed, got {status} {body}"
            );

            // Every job is submitted first, then read once the clock has had its span,
            // so the suite waits once rather than per row.
            let cases = cases();
            let mut ids = Vec::new();
            for case in &cases {
                let inputs: Vec<&str> = case.inputs.iter().map(String::as_str).collect();
                let (status, body) = mediaconvert(
                    &router,
                    &runtime,
                    "POST",
                    "/2017-08-29/jobs",
                    Some(job_body(&inputs)),
                );
                assert_eq!(
                    status, 201,
                    "case {:?}: CreateJob must succeed, got {status} {body}",
                    case.case
                );
                let id = body
                    .pointer("/job/id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    !id.is_empty(),
                    "case {:?}: CreateJob must mint an Id, got {body}",
                    case.case
                );
                ids.push(id);
            }

            // The due instant is stored, not slept for by a handler; this suite waits
            // past it on its own thread.
            std::thread::sleep(PAST_THE_SETTLE);

            for (case, id) in cases.iter().zip(&ids) {
                let (status, body) = mediaconvert(
                    &router,
                    &runtime,
                    "GET",
                    &format!("/2017-08-29/jobs/{id}"),
                    None,
                );
                assert_eq!(
                    status, 200,
                    "case {:?}: GetJob must succeed, got {status} {body}",
                    case.case
                );
                let job_status = body
                    .pointer("/job/status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert_eq!(
                    job_status, "ERROR",
                    "case {:?}: the job must have reached ERROR, got {body}",
                    case.case
                );
                let error_code = body.pointer("/job/errorCode").and_then(serde_json::Value::as_i64);
                let error_message = body
                    .pointer("/job/errorMessage")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match &case.unreadable {
                    Some(uri) => {
                        assert_eq!(
                            error_code,
                            Some(INPUT_PROBE_FAILURE_CODE),
                            "case {:?}: an input S3 could not read must carry ErrorCode \
                             {INPUT_PROBE_FAILURE_CODE}, got {error_code:?} in {body}",
                            case.case
                        );
                        assert!(
                            error_message.contains(uri.as_str()),
                            "case {:?}: the message must name the input that failed \
                             ({uri}), got {error_message:?}",
                            case.case
                        );
                    }
                    None => {
                        assert_eq!(
                            error_code, None,
                            "case {:?}: a job whose inputs were readable must carry no \
                             input-probe code, got {error_code:?} in {body}",
                            case.case
                        );
                        assert!(
                            !error_message.is_empty(),
                            "case {:?}: an ERROR job must still say why it failed, got \
                             {error_message:?}",
                            case.case
                        );
                    }
                }
            }
        })
        .expect("spawn the input-probe thread")
        .join()
        .expect("a failed job must be classified from the probe that was performed");
}

// SPDX-License-Identifier: CC0-1.0

//! The pieces the suites in this crate share, beside the one-emulator harness in
//! [`crate::endpoint_invocation`].
//!
//! The emulator itself (the shared engine with the SageMaker control plane and this data
//! plane mounted on the production `MultiServiceRouter`) is built there and REUSED here
//! rather than built a second time; what this module adds is what that suite does not
//! need: a request builder for the two other modelled routes
//! (`/endpoints/{EndpointName}/async-invocations` and `.../invocations-response-stream`),
//! the `AsyncInferenceConfig` body a deployment needs before it accepts an asynchronous
//! submission, and the read-back of this service's own asynchronous queue.

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::{
    AsyncInvocation, InvocationOutcome,
};
use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState as RuntimeState;

pub(crate) use crate::endpoint_invocation::{Call, Emulator, ACCOUNT, AUTHORIZATION};

/// The request headers one table row sends.
pub(crate) type Headers<'h> = Vec<(&'h str, &'h str)>;

/// One modelled route of this service, as a cross-route table names it: the operation's
/// name, the path builder for it, and the headers that route needs before it can get as
/// far as resolving the endpoint.
pub(crate) type Route = (&'static str, fn(&str) -> String, Headers<'static>);

/// One request row: a case label, the headers and the body.
pub(crate) type RequestCase<'c> = (&'c str, Headers<'c>, &'c [u8]);

/// One refusal row: a case label, the endpoint it names, the headers, and the outcome the
/// data plane must log for it.
pub(crate) type OutcomeCase<'c> = (&'c str, &'c str, Headers<'c>, InvocationOutcome);

/// One asynchronous-submission row: [`OutcomeCase`] with the body the row submits.
pub(crate) type SubmissionCase<'c> = (
    &'c str,
    &'c str,
    Headers<'c>,
    &'c [u8],
    InvocationOutcome,
);

/// The three modelled routes of this service, as an SDK client addresses them.
pub(crate) fn invocations(endpoint: &str) -> String {
    format!("/endpoints/{endpoint}/invocations")
}

pub(crate) fn async_invocations(endpoint: &str) -> String {
    format!("/endpoints/{endpoint}/async-invocations")
}

pub(crate) fn response_stream(endpoint: &str) -> String {
    format!("/endpoints/{endpoint}/invocations-response-stream")
}

/// An `InputLocation` naming an S3 object, as the member's pattern
/// (`^(https|s3)://([^/]+)/?(.*)$`) admits. The asynchronous route needs one (or an inline
/// body) before it gets as far as resolving the endpoint.
pub(crate) const INPUT_LOCATION: (&str, &str) =
    ("x-amzn-sagemaker-inputlocation", "s3://bucket/in/req.json");

/// The three modelled routes, each with the headers that route needs to reach the
/// resolution step — stated once, so a rule asserted "for every route" cannot drift
/// between the suites that assert it.
pub(crate) fn routes() -> Vec<Route> {
    vec![
        ("InvokeEndpoint", invocations as fn(&str) -> String, vec![]),
        (
            "InvokeEndpointAsync",
            async_invocations as fn(&str) -> String,
            vec![INPUT_LOCATION],
        ),
        (
            "InvokeEndpointWithResponseStream",
            response_stream as fn(&str) -> String,
            vec![],
        ),
    ]
}

/// [`routes`]'s headers for a row that carries `body`: the asynchronous route takes
/// EXACTLY ONE input source — the inline `Body` or `InputLocation` — so the location
/// header stands in only where the inline body is empty, and is dropped where the row
/// supplies a body of its own. Every other route carries its payload alone, so its
/// headers are unchanged.
pub(crate) fn headers_for<'h>(headers: &Headers<'h>, body: &[u8]) -> Headers<'h> {
    if body.is_empty() {
        return headers.clone();
    }
    headers
        .iter()
        .copied()
        .filter(|(name, _)| *name != INPUT_LOCATION.0)
        .collect()
}

/// `POST path` with `headers` and the raw `body`, over the same router an SDK client
/// reaches. The data plane's routes differ only in their last segment, so one builder
/// serves all three.
pub(crate) async fn post(
    emulator: &Emulator,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Call {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed test request");
    let response = emulator.router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Call {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The `AsyncInferenceConfig` an endpoint configuration carries so its endpoint accepts
/// asynchronous submissions, as `CreateEndpointConfig$AsyncInferenceConfig` models it:
/// `OutputConfig$S3OutputPath` is where a response payload would be written and
/// `OutputConfig$S3FailurePath` where a failure payload would.
pub(crate) fn async_inference_config(s3_output_path: &str, s3_failure_path: Option<&str>) -> Value {
    let mut output_config = json!({ "S3OutputPath": s3_output_path });
    if let Some(failure_path) = s3_failure_path {
        output_config["S3FailurePath"] = json!(failure_path);
    }
    json!({ "AsyncInferenceConfig": { "OutputConfig": output_config } })
}

/// This service's own asynchronous queue, as the next request would read it.
pub(crate) fn async_queue(emulator: &Emulator) -> Vec<AsyncInvocation> {
    emulator
        .engine
        .state::<RuntimeState>()
        .lock()
        .async_invocations()
        .cloned()
        .collect()
}

/// The queued submission made under `inference_id`, as the next request would read it.
pub(crate) fn queued(emulator: &Emulator, inference_id: &str) -> Option<AsyncInvocation> {
    emulator
        .engine
        .state::<RuntimeState>()
        .lock()
        .async_invocation(inference_id)
        .cloned()
}

/// Materialise every outcome the clock already owes the queue, as a later request would.
pub(crate) fn settle_queue(emulator: &Emulator, now: std::time::SystemTime) {
    emulator
        .engine
        .state::<RuntimeState>()
        .lock()
        .settle_async_invocations(now);
}

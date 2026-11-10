// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a RESPONSE-STREAMING invocation's event stream carries when
//! the handler fails after it has already written bytes.
//!
//! The rule under test: a streaming invocation's answer is the bytes the handler wrote, in
//! the order it wrote them, each write its own `PayloadChunk`, and the stream always ends
//! with the `InvokeComplete` event — which names the error of a handler that failed, so a
//! client can tell a complete response from one truncated by a failure.
//!
//! The rows differ along how the handler ends: returning normally, rejecting after its
//! writes, throwing synchronously before any write, and ending the stream itself.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_eventstream::frame::{DecodedFrame, MessageFrameDecoder};
use aws_smithy_types::event_stream::HeaderValue;
use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(100);

/// One decoded event-stream frame: the union member it carries and its payload.
#[derive(Debug)]
struct Frame {
    event_type: Option<String>,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload as JSON, when it is JSON at all.
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.payload).ok()
    }
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The frames of the event stream this answer's body is.
    fn frames(&self, case: &str) -> Vec<Frame> {
        let mut decoder = MessageFrameDecoder::new();
        let mut remaining: &[u8] = &self.body;
        let mut frames = Vec::new();
        while !remaining.is_empty() {
            match decoder.decode_frame(&mut remaining) {
                Ok(DecodedFrame::Complete(message)) => {
                    let event_type = message
                        .headers()
                        .iter()
                        .find(|header| header.name().as_str() == ":event-type")
                        .and_then(|header| match header.value() {
                            HeaderValue::String(value) => Some(value.as_str().to_string()),
                            _ => None,
                        });
                    frames.push(Frame {
                        event_type,
                        payload: message.payload().to_vec(),
                    });
                }
                Ok(DecodedFrame::Incomplete) => {
                    panic!("case {case:?}: the event stream ends mid-frame")
                }
                Err(error) => panic!(
                    "case {case:?}: the event stream is not decodable: {error}: {:?}",
                    self.text()
                ),
            }
        }
        frames
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A .zip deployment package holding `entries`.
fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, source) in entries {
            writer.start_file(*name, options).expect("the entry starts");
            writer
                .write_all(source.as_bytes())
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create a `nodejs20.x` function holding `source` as `index.js`, and wait out the
/// provisioning window the create documents.
async fn create_streaming_function(router: &MultiServiceRouter, name: &str, source: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "nodejs20.x",
            "Handler": "index.handler",
            "Timeout": 10,
            "MemorySize": 256,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("index.js", source)])),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `InvokeWithResponseStream` on `name`.
async fn invoke_with_response_stream(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/2021-11-15/functions/{name}/response-streaming-invocations"),
        b"{}",
        &[],
    )
    .await
}

/// A streaming handler that writes `records` as its own chunks and then ends the way
/// `ending` says.
fn streaming_source(ending: &str) -> String {
    format!(
        "exports.handler = awslambda.streamifyResponse(async (event, responseStream) => {{\n\
         \x20   for (let seq = 1; seq <= 3; seq += 1) {{\n\
         \x20       responseStream.write(JSON.stringify({{ seq: seq }}));\n\
         \x20       await new Promise((resolve) => setTimeout(resolve, 5));\n\
         \x20   }}\n\
         \x20   {ending}\n\
         }});\n"
    )
}

/// Every write a streaming handler made reaches the client as its own `PayloadChunk`, and
/// the stream ends with the `InvokeComplete` event in every case — carrying the error of a
/// handler that failed after its writes, and no error for one that returned.
#[tokio::test]
async fn a_streaming_invocation_always_ends_with_the_completion_event() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // The rows: how the handler ends, the chunk sequence the client must see, and the
    // `ErrorCode` the completion event must name.
    let cases: &[(&str, String, &[i64], Option<&str>)] = &[
        (
            "a handler that returns after its writes",
            streaming_source("return;"),
            &[1, 2, 3],
            None,
        ),
        (
            "a handler that ends the stream itself",
            streaming_source("responseStream.end();"),
            &[1, 2, 3],
            None,
        ),
        (
            "a handler that throws after its writes",
            streaming_source("throw new Error('after the first chunk');"),
            &[1, 2, 3],
            Some("Unhandled"),
        ),
        (
            "a handler that rejects after its writes",
            streaming_source("await Promise.reject(new TypeError('rejected'));"),
            &[1, 2, 3],
            Some("Unhandled"),
        ),
        (
            // The sibling half of the rule: nothing was begun for an error to have
            // truncated, so the caller is delivered no bytes and no error code, and the
            // failure is in the run's log alone.
            "a handler that fails before any write",
            "exports.handler = awslambda.streamifyResponse(async () => {\n\
             \x20   throw new Error('before any write');\n\
             });\n"
                .to_string(),
            &[],
            None,
        ),
    ];

    for (index, (label, source, sequence, error_code)) in cases.iter().enumerate() {
        let name = format!("stream-trailer-{index}");
        create_streaming_function(&router, &name, source).await;
        let answer = invoke_with_response_stream(&router, &name).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: a streaming invocation is answered 200, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.header("content-type").as_deref(),
            Some("application/vnd.amazon.eventstream"),
            "case {label:?}: the answer is an event stream, got {:?}",
            answer.header("content-type")
        );
        let frames = answer.frames(label);
        let chunks: Vec<i64> = frames
            .iter()
            .filter(|frame| frame.event_type.as_deref() == Some("PayloadChunk"))
            .filter_map(|frame| frame.json())
            .filter_map(|record| record.get("seq").and_then(Value::as_i64))
            .collect();
        assert_eq!(
            chunks, *sequence,
            "case {label:?}: expected one chunk per handler write {sequence:?}, got \
             {chunks:?} out of {frames:?}"
        );
        let complete: Vec<&Frame> = frames
            .iter()
            .filter(|frame| frame.event_type.as_deref() == Some("InvokeComplete"))
            .collect();
        assert_eq!(
            complete.len(),
            1,
            "case {label:?}: the stream ends with exactly one InvokeComplete, got {frames:?}"
        );
        assert_eq!(
            frames
                .last()
                .and_then(|frame| frame.event_type.clone())
                .as_deref(),
            Some("InvokeComplete"),
            "case {label:?}: the completion event is the LAST frame, got {frames:?}"
        );
        let reported = complete[0]
            .json()
            .and_then(|document| {
                document
                    .get("ErrorCode")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        match error_code {
            Some(expected) => assert_eq!(
                reported, *expected,
                "case {label:?}: the completion event names the handler's failure, got \
                 {:?}",
                complete[0].json()
            ),
            None => assert!(
                reported.is_empty(),
                "case {label:?}: a handler that returned names no error, got {:?}",
                complete[0].json()
            ),
        }
        if error_code.is_some() {
            let details = complete[0]
                .json()
                .and_then(|document| {
                    document
                        .get("ErrorDetails")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            assert!(
                details.contains("errorType"),
                "case {label:?}: the completion event carries the error document as its \
                 details, got {details:?}"
            );
        }
    }
}

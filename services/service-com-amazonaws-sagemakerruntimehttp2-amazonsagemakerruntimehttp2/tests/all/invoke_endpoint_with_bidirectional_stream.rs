// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InvokeEndpointWithBidirectionalStream`.
//!
//! Every test drives a real `MultiServiceRouter` over this service, so the whole
//! path is exercised: claiming, routing, the hand-written event-stream entry, the
//! `vnd.amazon.event-stream` framing in both directions, the endpoint registry,
//! and the transcript the store keeps.
//!
//! The assertions state RULES, each over a table of inputs:
//!
//! * `X-Amzn-Invoked-Production-Variant` is the variant selected out of the
//!   addressed endpoint's registry entry — the `TargetVariant` when it names one
//!   of that endpoint's production variants, otherwise that endpoint's default
//!   variant.
//! * every frame of the request stream is decoded in order, up to a `COMPLETE`
//!   frame, and recorded in the transcript with its event headers.
//! * the response frame sequence is the addressed endpoint's declared
//!   `ModelBehaviour` applied to the accumulated request bytes.
//! * a value outside a member's modelled `@length`/`@pattern` is refused with
//!   HTTP 400 `InputValidationError`; a value AT each published bound is served.
//! * an endpoint configured to fail answers the modelled `ModelError` (424) from
//!   the configured values, while a failure the model reports mid-stream is
//!   delivered as a member INSIDE the response stream, after the 200.

use aws_smithy_eventstream::frame::{write_message_to, DecodedFrame, MessageFrameDecoder};
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use service_com_amazonaws_sagemakerruntimehttp2_amazonsagemakerruntimehttp2 as sagemaker_runtime;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use sagemaker_runtime::ops::invoke_endpoint_with_bidirectional_stream::{
    MAX_REQUEST_FRAMES, MAX_REQUEST_PAYLOAD_BYTES, MAX_RESPONSE_FRAMES,
};
use sagemaker_runtime::state::{
    ComAmazonawsSagemakerruntimehttp2Amazonsagemakerruntimehttp2State as ServiceState, Endpoint,
    InvocationOutcome, ModelBehaviour, ResponseFrame, StreamFailureKind,
    MAX_RECORDED_FRAMES_PER_INVOCATION, MAX_RECORDED_HEADER_BYTES, MAX_RECORDED_INVOCATIONS,
    MAX_RECORDED_INVOCATION_BYTES,
};

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name, which is how the router resolves a routing key.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=deadbeef";

/// Event-stream header names the framing uses.
const MESSAGE_TYPE: &str = ":message-type";
const EVENT_TYPE: &str = ":event-type";
const EXCEPTION_TYPE: &str = ":exception-type";
const DATA_TYPE: &str = "DataType";
const COMPLETION_STATE: &str = "CompletionState";
const P: &str = "P";

/// A request-stream frame a test sends.
#[derive(Debug, Clone)]
struct Frame {
    bytes: Vec<u8>,
    data_type: Option<&'static str>,
    completion_state: Option<&'static str>,
    p: Option<&'static str>,
}

impl Frame {
    fn new(bytes: &[u8], data_type: &'static str, completion_state: &'static str) -> Self {
        Self {
            bytes: bytes.to_vec(),
            data_type: Some(data_type),
            completion_state: Some(completion_state),
            p: None,
        }
    }
}

/// One decoded response-stream message, flattened to what the tests assert on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decoded {
    message_type: Option<String>,
    event_type: Option<String>,
    exception_type: Option<String>,
    data_type: Option<String>,
    completion_state: Option<String>,
    p: Option<String>,
    payload: Vec<u8>,
}

fn new_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// A router mounting only this service, over `engine`.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(sagemaker_runtime::EmulatedService::new(Arc::clone(engine)));
    router
}

/// Put `endpoint` in the registry the handler reads.
fn register(engine: &Arc<engine::InMemoryEngine>, endpoint: Endpoint) {
    engine.state::<ServiceState>().lock().register_endpoint(endpoint);
}

/// An endpoint record, built through the state's own constructor so its default
/// variant is always one of its production variants.
fn endpoint(name: &str, variants: &[&str], behaviour: ModelBehaviour) -> Endpoint {
    ServiceState::endpoint(name, variants, behaviour)
        .unwrap_or_else(|| panic!("case {name:?}: an endpoint needs at least one variant"))
}

fn echo(chunk_size: usize, data_type: &str) -> ModelBehaviour {
    ModelBehaviour::Echo {
        chunk_size,
        data_type: data_type.to_string(),
    }
}

/// Frame `frames` as a `vnd.amazon.event-stream` request body.
fn encode_request_stream(frames: &[Frame]) -> Vec<u8> {
    let mut body = Vec::new();
    for frame in frames {
        let mut message = Message::new(frame.bytes.clone())
            .add_header(Header::new(MESSAGE_TYPE, HeaderValue::String("event".into())))
            .add_header(Header::new(
                EVENT_TYPE,
                HeaderValue::String("PayloadPart".into()),
            ));
        for (name, value) in [
            (DATA_TYPE, frame.data_type),
            (COMPLETION_STATE, frame.completion_state),
            (P, frame.p),
        ] {
            if let Some(value) = value {
                message =
                    message.add_header(Header::new(name, HeaderValue::String(value.into())));
            }
        }
        write_message_to(&message, &mut body).expect("a test frame must encode");
    }
    body
}

/// Decode a response body into its messages, in order.
fn decode_response_stream(case: &str, body: &[u8]) -> Vec<Decoded> {
    let mut decoder = MessageFrameDecoder::new();
    let mut remaining: &[u8] = body;
    let mut out = Vec::new();
    while !remaining.is_empty() {
        match decoder.decode_frame(&mut remaining) {
            Ok(DecodedFrame::Complete(message)) => {
                let header = |name: &str| {
                    message
                        .headers()
                        .iter()
                        .find(|h| h.name().as_str() == name)
                        .and_then(|h| match h.value() {
                            HeaderValue::String(v) => Some(v.as_str().to_string()),
                            _ => None,
                        })
                };
                out.push(Decoded {
                    message_type: header(MESSAGE_TYPE),
                    event_type: header(EVENT_TYPE),
                    exception_type: header(EXCEPTION_TYPE),
                    data_type: header(DATA_TYPE),
                    completion_state: header(COMPLETION_STATE),
                    p: header(P),
                    payload: message.payload().to_vec(),
                });
            }
            Ok(DecodedFrame::Incomplete) => {
                panic!("case {case:?}: the response stream ends mid-frame")
            }
            Err(e) => panic!("case {case:?}: the response stream is not decodable: {e}"),
        }
    }
    out
}

/// A buffered invocation request declaring the event-stream media type.
fn invoke_request(
    endpoint_name: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> http::Request<bytes::Bytes> {
    invoke_request_typed(
        endpoint_name,
        Some("application/vnd.amazon.eventstream"),
        headers,
        body,
    )
}

/// A buffered invocation request whose `Content-Type` is `content_type`, or which
/// declares none when that is `None`.
fn invoke_request_typed(
    endpoint_name: &str,
    content_type: Option<&str>,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!(
            "http://localhost/endpoints/{endpoint_name}/invocations-bidirectional-stream"
        ))
        .header("authorization", AUTHORIZATION);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder
        .body(bytes::Bytes::from(body))
        .expect("a test request must build")
}

/// The JSON body of an error response.
fn error_body(case: &str, response: &http::Response<bytes::Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or_else(|e| {
        panic!(
            "case {case:?}: expected a JSON error body, got {:?} ({e})",
            String::from_utf8_lossy(response.body())
        )
    })
}

/// The `PayloadPart` frames of a decoded response, in order.
fn payload_parts(frames: &[Decoded]) -> Vec<&Decoded> {
    frames
        .iter()
        .filter(|f| f.event_type.as_deref() == Some("PayloadPart"))
        .collect()
}

// ---------------------------------------------------------------------------
// RULE: the invoked production variant is selected out of the registry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invoked_production_variant_is_selected_from_the_endpoints_registry_entry() {
    // Rows differ along the variant list, which of them is the default (the
    // first), and whether the request names a `TargetVariant`.
    let cases: &[(&str, &[&str], Option<&str>, &str)] = &[
        ("single-variant-endpoint", &["AllTraffic"], None, "AllTraffic"),
        (
            "single-variant-endpoint",
            &["AllTraffic"],
            Some("AllTraffic"),
            "AllTraffic",
        ),
        ("default-is-the-first", &["Blue", "Green"], None, "Blue"),
        (
            "default-is-the-first",
            &["Blue", "Green"],
            Some("Green"),
            "Green",
        ),
        (
            "three-variants",
            &["one", "two", "three"],
            Some("three"),
            "three",
        ),
        ("three-variants", &["one", "two", "three"], None, "one"),
    ];

    for (name, variants, target_variant, expected) in cases {
        let case = format!("{name} variants={variants:?} target={target_variant:?}");
        let engine = new_engine();
        register(&engine, endpoint(name, variants, echo(16, "UTF8")));
        let router = router(&engine);

        let headers: Vec<(&str, &str)> = target_variant
            .map(|v| vec![("X-Amzn-SageMaker-Target-Variant", v)])
            .unwrap_or_default();
        let body = encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]);
        let response = router.dispatch(invoke_request(name, &headers, body)).await;

        let actual = response
            .headers()
            .get("x-amzn-invoked-production-variant")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            actual.as_deref(),
            Some(*expected),
            "case {case:?}: expected the invoked variant {expected:?}, got {actual:?}"
        );
        // And the transcript records the same selection.
        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        assert_eq!(
            recorded
                .last()
                .map(|i| i.invoked_production_variant.clone()),
            Some((*expected).to_string()),
            "case {case:?}: the transcript must record the selected variant, got {recorded:?}"
        );
    }
}

#[tokio::test]
async fn a_target_variant_the_endpoint_does_not_have_is_refused() {
    let cases: &[(&str, &[&str], &str)] = &[
        ("only-all-traffic", &["AllTraffic"], "Canary"),
        ("blue-green", &["Blue", "Green"], "Red"),
        // Variant names are case-sensitive, so a differently-cased name is a
        // variant this endpoint does not have.
        ("blue-green", &["Blue", "Green"], "blue"),
    ];

    for (name, variants, requested) in cases {
        let case = format!("{name} variants={variants:?} target={requested:?}");
        let engine = new_engine();
        register(&engine, endpoint(name, variants, echo(16, "UTF8")));
        let router = router(&engine);

        let body = encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]);
        let response = router
            .dispatch(invoke_request(
                name,
                &[("X-Amzn-SageMaker-Target-Variant", requested)],
                body,
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = error_body(&case, &response);
        assert_eq!(
            body["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every request frame is decoded in order and recorded.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_frames_are_decoded_in_order_and_recorded_in_the_transcript() {
    // Rows differ along frame count, payload size (including an empty frame),
    // the `DataType` value and whether a `P` header is present.
    let cases: Vec<(&str, Vec<Frame>)> = vec![
        (
            "one complete frame",
            vec![Frame::new(b"hello", "UTF8", "COMPLETE")],
        ),
        (
            "two frames",
            vec![
                Frame::new(b"hello ", "UTF8", "PARTIAL"),
                Frame::new(b"world", "UTF8", "COMPLETE"),
            ],
        ),
        (
            "four frames",
            vec![
                Frame::new(b"a", "UTF8", "PARTIAL"),
                Frame::new(b"bb", "UTF8", "PARTIAL"),
                Frame::new(b"ccc", "UTF8", "PARTIAL"),
                Frame::new(b"dddd", "UTF8", "COMPLETE"),
            ],
        ),
        (
            "binary bytes",
            vec![Frame::new(&[0x00, 0xff, 0x10, 0x80], "BINARY", "COMPLETE")],
        ),
        (
            "an empty payload frame",
            vec![
                Frame::new(b"", "UTF8", "PARTIAL"),
                Frame::new(b"tail", "UTF8", "COMPLETE"),
            ],
        ),
        (
            "a P event header",
            vec![Frame {
                bytes: b"carried".to_vec(),
                data_type: Some("UTF8"),
                completion_state: Some("COMPLETE"),
                p: Some("p-value"),
            }],
        ),
        (
            "no event headers at all",
            vec![Frame {
                bytes: b"headerless".to_vec(),
                data_type: None,
                completion_state: None,
                p: None,
            }],
        ),
    ];

    for (case, frames) in cases {
        let engine = new_engine();
        register(&engine, endpoint("transcript", &["AllTraffic"], echo(4, "UTF8")));
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                "transcript",
                &[],
                encode_request_stream(&frames),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: expected exactly one recorded invocation, got {recorded:?}"
        );
        let invocation = &recorded[0];
        assert_eq!(
            invocation.request_frames.len(),
            frames.len(),
            "case {case:?}: expected {} recorded request frames, got {:?}",
            frames.len(),
            invocation.request_frames
        );
        for (index, (sent, got)) in frames.iter().zip(&invocation.request_frames).enumerate() {
            assert_eq!(
                got.bytes, sent.bytes,
                "case {case:?} frame {index}: expected bytes {:?}, got {:?}",
                sent.bytes, got.bytes
            );
            assert_eq!(
                got.data_type.as_deref(),
                sent.data_type,
                "case {case:?} frame {index}: expected DataType {:?}, got {:?}",
                sent.data_type,
                got.data_type
            );
            assert_eq!(
                got.completion_state.as_deref(),
                sent.completion_state,
                "case {case:?} frame {index}: expected CompletionState {:?}, got {:?}",
                sent.completion_state,
                got.completion_state
            );
            assert_eq!(
                got.p.as_deref(),
                sent.p,
                "case {case:?} frame {index}: expected P {:?}, got {:?}",
                sent.p,
                got.p
            );
        }
        assert_eq!(
            invocation.terminal,
            InvocationOutcome::Completed,
            "case {case:?}: expected a completed invocation, got {:?}",
            invocation.terminal
        );
        // The frames the emulator PRODUCED are observable too, and are the ones
        // that reached the wire.
        let on_the_wire = payload_parts(&decode_response_stream(case, response.body()))
            .iter()
            .map(|f| f.payload.clone())
            .collect::<Vec<_>>();
        let in_the_transcript = invocation
            .response_frames
            .iter()
            .map(|f| f.bytes.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            on_the_wire, in_the_transcript,
            "case {case:?}: the transcript's response frames must be the ones sent, \
             got {in_the_transcript:?} vs {on_the_wire:?}"
        );
    }
}

#[tokio::test]
async fn bytes_after_a_complete_frame_are_refused_rather_than_discarded() {
    // A `COMPLETE` frame says the caller has nothing more to send, so anything
    // still in the body is not part of this stream. Discarding it would accept a
    // mis-framed stream and would leave the transcript describing only a prefix.
    // Rows differ along what follows the COMPLETE frame.
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "a further well-formed frame",
            encode_request_stream(&[
                Frame::new(b"kept", "UTF8", "PARTIAL"),
                Frame::new(b"last", "UTF8", "COMPLETE"),
                Frame::new(b"trailing", "UTF8", "PARTIAL"),
            ]),
        ),
        (
            "a truncated frame",
            {
                let mut body = encode_request_stream(&[Frame::new(b"last", "UTF8", "COMPLETE")]);
                body.extend_from_slice(&[0x00, 0x00, 0x00, 0x20, 0x00]);
                body
            },
        ),
        ("one stray byte", {
            let mut body = encode_request_stream(&[Frame::new(b"last", "UTF8", "COMPLETE")]);
            body.push(0x7f);
            body
        }),
    ];

    for (case, body) in cases {
        let engine = new_engine();
        register(&engine, endpoint("stops", &["AllTraffic"], echo(64, "UTF8")));
        let router = router(&engine);

        let response = router.dispatch(invoke_request("stops", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let parsed = error_body(case, &response);
        assert_eq!(
            parsed["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {parsed}"
        );
        assert!(
            engine.state::<ServiceState>().lock().invocations.is_empty(),
            "case {case:?}: a refused stream must not be recorded as an invocation"
        );
    }
}

#[tokio::test]
async fn a_stream_ending_at_its_complete_frame_is_consumed_whole() {
    let engine = new_engine();
    register(&engine, endpoint("stops", &["AllTraffic"], echo(64, "UTF8")));
    let router = router(&engine);

    let body = encode_request_stream(&[
        Frame::new(b"kept", "UTF8", "PARTIAL"),
        Frame::new(b"last", "UTF8", "COMPLETE"),
    ]);
    let response = router.dispatch(invoke_request("stops", &[], body)).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case \"stops\": expected 200, got {:?}",
        response.status()
    );

    let recorded = engine.state::<ServiceState>().lock().invocations.clone();
    let got: Vec<Vec<u8>> = recorded[0]
        .request_frames
        .iter()
        .map(|f| f.bytes.clone())
        .collect();
    assert_eq!(
        got,
        vec![b"kept".to_vec(), b"last".to_vec()],
        "case \"stops\": every frame up to and including the COMPLETE one must be \
         consumed, got {got:?}"
    );
    assert_eq!(
        recorded[0].request_frames_total, 2,
        "case \"stops\": the entry must report both frames, got {}",
        recorded[0].request_frames_total
    );
}

// ---------------------------------------------------------------------------
// RULE: the response stream is the endpoint's declared behaviour applied to the
// accumulated request bytes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_echo_behaviour_reframes_the_accumulated_request_bytes_by_the_configured_chunk_size() {
    // Rows differ along the configured chunk size, the configured DataType, and
    // the total number of request bytes (including zero).
    let cases: &[(usize, &str, &[&[u8]])] = &[
        (1, "UTF8", &[b"abc"]),
        (3, "UTF8", &[b"abcdef"]),
        (3, "UTF8", &[b"abcdefg"]),
        (8, "UTF8", &[b"hello ", b"world"]),
        (1024, "UTF8", &[b"short"]),
        (4, "BINARY", &[&[0u8, 1, 2, 3, 4, 5, 6]]),
        (5, "UTF8", &[b""]),
        (2, "UTF8", &[b"ab", b"", b"cde"]),
    ];

    for (chunk_size, data_type, payloads) in cases {
        let total: Vec<u8> = payloads.iter().flat_map(|p| p.iter().copied()).collect();
        let case = format!(
            "chunk_size={chunk_size} data_type={data_type} total_len={}",
            total.len()
        );
        let engine = new_engine();
        register(
            &engine,
            endpoint("echoes", &["AllTraffic"], echo(*chunk_size, data_type)),
        );
        let router = router(&engine);

        let frames: Vec<Frame> = payloads
            .iter()
            .enumerate()
            .map(|(index, payload)| {
                Frame::new(
                    payload,
                    data_type,
                    if index + 1 == payloads.len() {
                        "COMPLETE"
                    } else {
                        "PARTIAL"
                    },
                )
            })
            .collect();
        let response = router
            .dispatch(invoke_request("echoes", &[], encode_request_stream(&frames)))
            .await;

        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?}",
            response.status()
        );
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/vnd.amazon.eventstream"),
            "case {case:?}: a response event stream must declare its media type"
        );

        let decoded = decode_response_stream(&case, response.body());
        let parts = payload_parts(&decoded);
        let expected_count = total.len().div_ceil(*chunk_size).max(1);
        assert_eq!(
            parts.len(),
            expected_count,
            "case {case:?}: expected {expected_count} PayloadPart frames, got {:?}",
            decoded
        );

        let joined: Vec<u8> = parts.iter().flat_map(|f| f.payload.iter().copied()).collect();
        assert_eq!(
            joined, total,
            "case {case:?}: expected the response bytes {total:?}, got {joined:?}"
        );

        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            let expected_state = if last { "COMPLETE" } else { "PARTIAL" };
            assert_eq!(
                part.completion_state.as_deref(),
                Some(expected_state),
                "case {case:?} frame {index}: expected CompletionState {expected_state:?}, \
                 got {:?}",
                part.completion_state
            );
            assert_eq!(
                part.data_type.as_deref(),
                Some(*data_type),
                "case {case:?} frame {index}: expected the endpoint's DataType \
                 {data_type:?}, got {:?}",
                part.data_type
            );
            assert!(
                part.payload.len() <= *chunk_size,
                "case {case:?} frame {index}: a frame may not exceed the configured chunk \
                 size {chunk_size}, got {} bytes",
                part.payload.len()
            );
            assert_eq!(
                part.message_type.as_deref(),
                Some("event"),
                "case {case:?} frame {index}: a PayloadPart is an event, got {:?}",
                part.message_type
            );
        }
    }
}

#[tokio::test]
async fn the_scripted_behaviour_answers_with_the_frames_the_registry_holds() {
    // Rows differ along the scripted frame count and the bytes each frame
    // carries; none of it comes from the request.
    let scripts: Vec<(&str, Vec<ResponseFrame>)> = vec![
        (
            "one frame",
            vec![ResponseFrame::new(
                b"only".to_vec(),
                Some("UTF8".to_string()),
                Some("COMPLETE".to_string()),
                None,
            )],
        ),
        (
            "three frames",
            vec![
                ResponseFrame::new(
                    b"first ".to_vec(),
                    Some("UTF8".to_string()),
                    Some("PARTIAL".to_string()),
                    None,
                ),
                ResponseFrame::new(
                    b"second ".to_vec(),
                    Some("UTF8".to_string()),
                    Some("PARTIAL".to_string()),
                    Some("tag".to_string()),
                ),
                ResponseFrame::new(
                    b"third".to_vec(),
                    Some("UTF8".to_string()),
                    Some("COMPLETE".to_string()),
                    None,
                ),
            ],
        ),
        (
            "binary frame",
            vec![ResponseFrame::new(
                vec![0xde, 0xad, 0xbe, 0xef],
                Some("BINARY".to_string()),
                Some("COMPLETE".to_string()),
                None,
            )],
        ),
    ];

    for (case, frames) in scripts {
        let engine = new_engine();
        register(
            &engine,
            endpoint(
                "scripted",
                &["AllTraffic"],
                ModelBehaviour::Scripted {
                    frames: frames.clone(),
                },
            ),
        );
        let router = router(&engine);

        let body = encode_request_stream(&[Frame::new(b"request-bytes", "UTF8", "COMPLETE")]);
        let response = router.dispatch(invoke_request("scripted", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?}",
            response.status()
        );

        let decoded = decode_response_stream(case, response.body());
        let parts = payload_parts(&decoded);
        assert_eq!(
            parts.len(),
            frames.len(),
            "case {case:?}: expected {} frames, got {decoded:?}",
            frames.len()
        );
        for (index, (want, got)) in frames.iter().zip(parts.iter()).enumerate() {
            assert_eq!(
                got.payload, want.bytes,
                "case {case:?} frame {index}: expected the registry's bytes {:?}, got {:?}",
                want.bytes, got.payload
            );
            assert_eq!(
                got.data_type, want.data_type,
                "case {case:?} frame {index}: expected DataType {:?}, got {:?}",
                want.data_type, got.data_type
            );
            assert_eq!(
                got.completion_state, want.completion_state,
                "case {case:?} frame {index}: expected CompletionState {:?}, got {:?}",
                want.completion_state, got.completion_state
            );
            assert_eq!(
                got.p, want.p,
                "case {case:?} frame {index}: expected P {:?}, got {:?}",
                want.p, got.p
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: modelled constraints decide what is refused.
// ---------------------------------------------------------------------------

/// Every member the model constrains, with a value outside that constraint.
///
/// Source: `smithy_query closure InvokeEndpointWithBidirectionalStream` —
/// `EndpointName` (maxLen 63, `^[a-zA-Z0-9](-*[a-zA-Z0-9])*`), `TargetVariant`
/// (maxLen 63, same pattern), `ModelInvocationPath` (maxLen 100,
/// `^[A-Za-z0-9\-._]+(?:/[A-Za-z0-9\-._]+)*$`), `ModelQueryString` (maxLen 2048,
/// `^[a-zA-Z0-9][A-Za-z0-9_-]*=...`), `RequestPayloadPart$DataType`
/// (`^(UTF8)$|^(BINARY)$`) and `RequestPayloadPart$CompletionState`
/// (`^(PARTIAL)$|^(COMPLETE)$`).
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_with_input_validation_error() {
    let long_endpoint_name = "a".repeat(64);
    let long_target_variant = "v".repeat(64);
    let long_invocation_path = "p".repeat(101);
    let long_query_string = format!("k={}", "v".repeat(2047));

    enum Case {
        /// The `@httpLabel`.
        Label(String),
        /// An `@httpHeader` member.
        Header(&'static str, String),
        /// An `@eventHeader` member of a request frame.
        EventHeader(&'static str, &'static str),
    }

    let cases: Vec<(&str, Case)> = vec![
        // The `EndpointName`/`TargetVariant` pattern is anchored only at its
        // START (`^[a-zA-Z0-9](-*[a-zA-Z0-9])*`), and Smithy `@pattern` is a FIND
        // rather than an anchored match, so a violating value is one whose FIRST
        // character is outside `[a-zA-Z0-9]`. `bad_name` is accepted by this
        // pattern — see
        // `a_name_the_unanchored_pattern_accepts_is_not_refused_for_its_pattern`.
        (
            "EndpointName pattern: leading hyphen",
            Case::Label("-leading".to_string()),
        ),
        (
            "EndpointName pattern: leading underscore",
            Case::Label("_leading".to_string()),
        ),
        (
            "EndpointName maxLen 63, one past",
            Case::Label(long_endpoint_name),
        ),
        (
            "TargetVariant pattern: leading hyphen",
            Case::Header("X-Amzn-SageMaker-Target-Variant", "-leading".to_string()),
        ),
        (
            "TargetVariant pattern: leading underscore",
            Case::Header("X-Amzn-SageMaker-Target-Variant", "_not a variant".to_string()),
        ),
        (
            "TargetVariant maxLen 63, one past",
            Case::Header("X-Amzn-SageMaker-Target-Variant", long_target_variant),
        ),
        (
            "ModelInvocationPath pattern",
            Case::Header(
                "X-Amzn-SageMaker-Model-Invocation-Path",
                "bad path!".to_string(),
            ),
        ),
        (
            "ModelInvocationPath maxLen 100, one past",
            Case::Header(
                "X-Amzn-SageMaker-Model-Invocation-Path",
                long_invocation_path,
            ),
        ),
        (
            "ModelQueryString pattern",
            Case::Header(
                "X-Amzn-SageMaker-Model-Query-String",
                "not a query string".to_string(),
            ),
        ),
        (
            "ModelQueryString maxLen 2048, one past",
            Case::Header("X-Amzn-SageMaker-Model-Query-String", long_query_string),
        ),
        (
            "RequestPayloadPart$DataType pattern",
            Case::EventHeader(DATA_TYPE, "JSON"),
        ),
        (
            "RequestPayloadPart$DataType pattern: lower case",
            Case::EventHeader(DATA_TYPE, "utf8"),
        ),
        (
            "RequestPayloadPart$CompletionState pattern",
            Case::EventHeader(COMPLETION_STATE, "DONE"),
        ),
        (
            "RequestPayloadPart$CompletionState pattern: lower case",
            Case::EventHeader(COMPLETION_STATE, "complete"),
        ),
    ];

    for (case, spec) in cases {
        let engine = new_engine();
        let router = router(&engine);

        let (name, headers, frames) = match &spec {
            Case::Label(label) => (
                label.clone(),
                Vec::new(),
                vec![Frame::new(b"ping", "UTF8", "COMPLETE")],
            ),
            Case::Header(name, value) => (
                "valid-endpoint".to_string(),
                vec![(*name, value.as_str())],
                vec![Frame::new(b"ping", "UTF8", "COMPLETE")],
            ),
            Case::EventHeader(header, value) => {
                let mut frame = Frame::new(b"ping", "UTF8", "COMPLETE");
                if *header == DATA_TYPE {
                    frame.data_type = Some(value);
                } else {
                    frame.completion_state = Some(value);
                }
                ("valid-endpoint".to_string(), Vec::new(), vec![frame])
            }
        };

        // Register the endpoint the row addresses, under exactly the name it
        // addresses, carrying exactly the variant it asks for — so the ONLY thing
        // left that can refuse the request is the constraint under test. Without
        // this, an unregistered name or an absent variant would answer 400 for a
        // reason that has nothing to do with the modelled bound.
        let variants: Vec<&str> = match &spec {
            Case::Header(header, value) if *header == "X-Amzn-SageMaker-Target-Variant" => {
                vec!["AllTraffic", value.as_str()]
            }
            _ => vec!["AllTraffic"],
        };
        register(&engine, endpoint(&name, &variants, echo(8, "UTF8")));

        let response = router
            .dispatch(invoke_request(
                &name,
                &headers,
                encode_request_stream(&frames),
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = error_body(case, &response);
        assert_eq!(
            body["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {body}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: InputValidationError must carry a Message, got {body}"
        );
        assert!(
            body["ErrorCode"].as_str().is_some_and(|c| !c.is_empty()),
            "case {case:?}: InputValidationError must carry an ErrorCode, got {body}"
        );
        assert!(
            engine.state::<ServiceState>().lock().invocations.is_empty(),
            "case {case:?}: a refused request must not be recorded as an invocation"
        );
    }
}

#[tokio::test]
async fn a_value_at_a_published_length_bound_is_served() {
    // Each member's published `@length` maximum, exactly. The row one past each
    // bound is refused by the test above.
    let endpoint_name_at_max = "a".repeat(63);
    let target_variant_at_max = format!("v{}", "x".repeat(62));
    let invocation_path_at_max = "p".repeat(100);
    let query_string_at_max = format!("k={}", "v".repeat(2046));

    /// One row: the case name, the `EndpointName` it addresses, and the headers
    /// it sends. Named rather than a bare tuple so the table does not read as a
    /// "very complex type".
    struct Bound {
        case: &'static str,
        endpoint_name: String,
        headers: Vec<(&'static str, String)>,
    }

    let cases: Vec<Bound> = vec![
        Bound {
            case: "EndpointName at maxLen 63",
            endpoint_name: endpoint_name_at_max.clone(),
            headers: Vec::new(),
        },
        Bound {
            case: "TargetVariant at maxLen 63",
            endpoint_name: endpoint_name_at_max.clone(),
            headers: vec![(
                "X-Amzn-SageMaker-Target-Variant",
                target_variant_at_max.clone(),
            )],
        },
        Bound {
            case: "ModelInvocationPath at maxLen 100",
            endpoint_name: endpoint_name_at_max.clone(),
            headers: vec![(
                "X-Amzn-SageMaker-Model-Invocation-Path",
                invocation_path_at_max,
            )],
        },
        Bound {
            case: "ModelQueryString at maxLen 2048",
            endpoint_name: endpoint_name_at_max.clone(),
            headers: vec![("X-Amzn-SageMaker-Model-Query-String", query_string_at_max)],
        },
    ];

    for Bound {
        case,
        endpoint_name: name,
        headers,
    } in cases
    {
        let engine = new_engine();
        register(
            &engine,
            endpoint(
                &name,
                &["AllTraffic", target_variant_at_max.as_str()],
                echo(8, "UTF8"),
            ),
        );
        let router = router(&engine);

        let borrowed: Vec<(&str, &str)> =
            headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let response = router
            .dispatch(invoke_request(
                &name,
                &borrowed,
                encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]),
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a value at the published bound must be served, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

#[tokio::test]
async fn a_name_the_unanchored_pattern_accepts_is_not_refused_for_its_pattern() {
    // Smithy `@pattern` is a FIND, not an anchored match, and `EndpointName`'s
    // pattern (`^[a-zA-Z0-9](-*[a-zA-Z0-9])*`) is anchored only at its start. So
    // a name whose first character is in `[a-zA-Z0-9]` SATISFIES it however it
    // continues. These rows would be refused by a hand-written "names look like
    // this" check, and must not be: the bound enforced is the model's.
    let cases: &[&str] = &["bad_name", "has.a.dot", "a~b", "a1-b2", "A"];

    for name in cases {
        let engine = new_engine();
        register(&engine, endpoint(name, &["AllTraffic"], echo(8, "UTF8")));
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                name,
                &[],
                encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {name:?}: the model's pattern accepts this name, so it must be served; \
             got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

#[tokio::test]
async fn an_unregistered_endpoint_name_is_refused() {
    // Rows differ along what is registered beside the name addressed.
    let cases: &[(&str, &[&str])] = &[
        ("does-not-exist", &[]),
        ("does-not-exist", &["another-endpoint"]),
        ("DoesNotExist", &["doesnotexist"]),
    ];

    for (addressed, registered) in cases {
        let case = format!("addressed={addressed:?} registered={registered:?}");
        let engine = new_engine();
        // Start from an empty registry so only `registered` is present.
        engine.state::<ServiceState>().lock().endpoints.clear();
        for name in *registered {
            register(&engine, endpoint(name, &["AllTraffic"], echo(8, "UTF8")));
        }
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                addressed,
                &[],
                encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]),
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = error_body(&case, &response);
        assert_eq!(
            body["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {body}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: expected a populated Message, got {body}"
        );
        assert!(
            body["ErrorCode"].as_str().is_some_and(|c| !c.is_empty()),
            "case {case:?}: expected a populated ErrorCode, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_stream_that_carries_no_member_is_refused() {
    // Rows differ along how the stream fails to carry a `RequestStreamEvent`.
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("an empty body", Vec::new()),
        ("truncated framing", vec![0x00, 0x00, 0x00, 0x20, 0x00]),
        ("not event-stream bytes at all", b"{\"not\":\"a frame\"}".to_vec()),
    ];

    for (case, body) in cases {
        let engine = new_engine();
        register(&engine, endpoint("empty", &["AllTraffic"], echo(8, "UTF8")));
        let router = router(&engine);

        let response = router.dispatch(invoke_request("empty", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let parsed = error_body(case, &response);
        assert_eq!(
            parsed["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {parsed}"
        );
    }
}

#[tokio::test]
async fn a_frame_naming_no_member_of_the_request_union_is_refused() {
    let engine = new_engine();
    register(&engine, endpoint("unions", &["AllTraffic"], echo(8, "UTF8")));
    let router = router(&engine);

    // `RequestStreamEvent` has exactly one member, `PayloadPart`.
    let cases: &[(&str, Option<&str>)] = &[
        ("an unknown member", Some("SomethingElse")),
        ("no :event-type at all", None),
    ];

    for (case, event_type) in cases {
        let mut message = Message::new(b"ping".to_vec())
            .add_header(Header::new(MESSAGE_TYPE, HeaderValue::String("event".into())));
        if let Some(event_type) = event_type {
            message = message.add_header(Header::new(
                EVENT_TYPE,
                HeaderValue::String((*event_type).into()),
            ));
        }
        let mut body = Vec::new();
        write_message_to(&message, &mut body).expect("a test frame must encode");

        let response = router.dispatch(invoke_request("unions", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a configured failure answers from the configured state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_endpoint_configured_to_fail_answers_model_error_from_the_configured_values() {
    // Rows differ along every value the modelled `ModelError` carries.
    let cases: &[(i32, &str, &str, &str)] = &[
        (
            400,
            "the model rejected the payload",
            "arn:aws:logs:us-east-1:000000000000:log-group:/aws/sagemaker/Endpoints/failing:log-stream:AllTraffic/i-0",
            "MODEL_REJECTED",
        ),
        (
            500,
            "the model crashed",
            "arn:aws:logs:eu-west-1:000000000000:log-group:/aws/sagemaker/Endpoints/failing:log-stream:Canary/i-1",
            "MODEL_CRASHED",
        ),
        (
            503,
            "the model was not ready",
            "arn:aws:logs:ap-south-1:000000000000:log-group:/aws/sagemaker/Endpoints/failing:log-stream:AllTraffic/i-2",
            "MODEL_NOT_READY",
        ),
    ];

    for (original_status_code, original_message, log_stream_arn, error_code) in cases {
        let case = format!("original_status_code={original_status_code} error_code={error_code}");
        let engine = new_engine();
        register(
            &engine,
            endpoint(
                "failing",
                &["AllTraffic"],
                ModelBehaviour::FailWithModelError {
                    original_status_code: *original_status_code,
                    original_message: (*original_message).to_string(),
                    log_stream_arn: (*log_stream_arn).to_string(),
                    error_code: (*error_code).to_string(),
                },
            ),
        );
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                "failing",
                &[],
                encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]),
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            424,
            "case {case:?}: expected 424, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = error_body(&case, &response);
        assert_eq!(
            body["__type"].as_str(),
            Some("ModelError"),
            "case {case:?}: expected ModelError, got {body}"
        );
        assert_eq!(
            body["OriginalStatusCode"].as_i64(),
            Some(i64::from(*original_status_code)),
            "case {case:?}: expected OriginalStatusCode {original_status_code}, got {body}"
        );
        assert_eq!(
            body["OriginalMessage"].as_str(),
            Some(*original_message),
            "case {case:?}: expected OriginalMessage {original_message:?}, got {body}"
        );
        assert_eq!(
            body["LogStreamArn"].as_str(),
            Some(*log_stream_arn),
            "case {case:?}: expected LogStreamArn {log_stream_arn:?}, got {body}"
        );
        assert_eq!(
            body["ErrorCode"].as_str(),
            Some(*error_code),
            "case {case:?}: expected ErrorCode {error_code:?}, got {body}"
        );

        // The request frames still arrived, so the transcript records the refused
        // invocation.
        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        assert_eq!(
            recorded.len(),
            1,
            "case {case:?}: expected the refused invocation to be recorded, got {recorded:?}"
        );
        assert_eq!(
            recorded[0].terminal,
            InvocationOutcome::ModelError {
                original_status_code: *original_status_code,
                error_code: (*error_code).to_string(),
            },
            "case {case:?}: expected a ModelError outcome, got {:?}",
            recorded[0].terminal
        );
    }
}

#[tokio::test]
async fn a_mid_stream_failure_is_delivered_inside_the_response_stream_after_a_200() {
    // Rows differ along how many frames are flushed first and which member of
    // `ResponseStreamEvent` terminates the stream.
    let cases: Vec<(&str, usize, StreamFailureKind)> = vec![
        (
            "one frame then ModelStreamError",
            1,
            StreamFailureKind::ModelStreamError {
                message: "the model stopped mid-stream".to_string(),
                error_code: "MODEL_STREAM_HALTED".to_string(),
            },
        ),
        (
            "two frames then ModelStreamError",
            2,
            StreamFailureKind::ModelStreamError {
                message: "the model stopped after two frames".to_string(),
                error_code: "MODEL_STREAM_HALTED".to_string(),
            },
        ),
        (
            "no frames then InternalStreamFailure",
            0,
            StreamFailureKind::InternalStreamFailure {
                message: "an internal stream failure occurred".to_string(),
            },
        ),
        (
            "three frames then InternalStreamFailure",
            3,
            StreamFailureKind::InternalStreamFailure {
                message: "an internal stream failure occurred late".to_string(),
            },
        ),
    ];

    for (case, frames_before_failure, stream_error) in cases {
        let engine = new_engine();
        register(
            &engine,
            endpoint(
                "mid-stream",
                &["AllTraffic"],
                ModelBehaviour::FailMidStream {
                    chunk_size: 2,
                    data_type: "UTF8".to_string(),
                    frames_before_failure,
                    stream_error: stream_error.clone(),
                },
            ),
        );
        let router = router(&engine);

        // 8 bytes at chunk size 2 is 4 echo frames, so every row's
        // `frames_before_failure` is reachable.
        let response = router
            .dispatch(invoke_request(
                "mid-stream",
                &[],
                encode_request_stream(&[Frame::new(b"abcdefgh", "UTF8", "COMPLETE")]),
            ))
            .await;

        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a mid-stream failure must not change the status, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        let decoded = decode_response_stream(case, response.body());
        let parts = payload_parts(&decoded);
        assert_eq!(
            parts.len(),
            frames_before_failure,
            "case {case:?}: expected {frames_before_failure} leading PayloadPart frames, \
             got {decoded:?}"
        );
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(
                part.completion_state.as_deref(),
                Some("PARTIAL"),
                "case {case:?} frame {index}: a stream that never completes may not claim \
                 COMPLETE, got {:?}",
                part.completion_state
            );
        }

        let terminal = decoded
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: the response stream carried no frame"));
        assert_eq!(
            terminal.message_type.as_deref(),
            Some("exception"),
            "case {case:?}: the stream must end in an exception member, got {terminal:?}"
        );
        let (expected_member, expected_message, expected_code) = match &stream_error {
            StreamFailureKind::ModelStreamError {
                message,
                error_code,
            } => ("ModelStreamError", message.clone(), Some(error_code.clone())),
            StreamFailureKind::InternalStreamFailure { message } => {
                ("InternalStreamFailure", message.clone(), None)
            }
        };
        assert_eq!(
            terminal.exception_type.as_deref(),
            Some(expected_member),
            "case {case:?}: expected the {expected_member} member, got {terminal:?}"
        );
        let payload: serde_json::Value = serde_json::from_slice(&terminal.payload)
            .unwrap_or_else(|e| panic!("case {case:?}: the exception payload is not JSON ({e})"));
        assert_eq!(
            payload["Message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the configured Message {expected_message:?}, got {payload}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                payload["ErrorCode"].as_str(),
                Some(expected_code.as_str()),
                "case {case:?}: expected the configured ErrorCode {expected_code:?}, \
                 got {payload}"
            );
        }

        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        let expected_outcome = match &stream_error {
            StreamFailureKind::ModelStreamError {
                message,
                error_code,
            } => InvocationOutcome::ModelStreamError {
                message: message.clone(),
                error_code: error_code.clone(),
            },
            StreamFailureKind::InternalStreamFailure { message } => {
                InvocationOutcome::InternalStreamFailure {
                    message: message.clone(),
                }
            }
        };
        assert_eq!(
            recorded.last().map(|i| i.terminal.clone()),
            Some(expected_outcome.clone()),
            "case {case:?}: expected the outcome {expected_outcome:?}, got {:?}",
            recorded.last().map(|i| i.terminal.clone())
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the work and the memory one invocation can cause are bounded, and every
// bound is the declared constant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_stream_past_a_declared_bound_is_refused_and_one_at_it_is_served() {
    /// A row: the frames it sends, the endpoint `chunk_size` it addresses, and
    /// the status it must get.
    struct Row {
        case: &'static str,
        frames: Vec<Frame>,
        chunk_size: usize,
        expected_status: u16,
    }

    fn one_byte_frames(count: usize) -> Vec<Frame> {
        (0..count)
            .map(|index| {
                Frame::new(
                    b"x",
                    "UTF8",
                    if index + 1 == count { "COMPLETE" } else { "PARTIAL" },
                )
            })
            .collect()
    }

    fn one_frame_of(len: usize) -> Vec<Frame> {
        vec![Frame::new(&vec![b'x'; len], "UTF8", "COMPLETE")]
    }

    let rows: Vec<Row> = vec![
        // MAX_REQUEST_FRAMES: at the bound, and one past it.
        Row {
            case: "frame count at MAX_REQUEST_FRAMES",
            frames: one_byte_frames(MAX_REQUEST_FRAMES),
            chunk_size: MAX_REQUEST_FRAMES,
            expected_status: 200,
        },
        Row {
            case: "frame count one past MAX_REQUEST_FRAMES",
            frames: one_byte_frames(MAX_REQUEST_FRAMES + 1),
            chunk_size: MAX_REQUEST_FRAMES,
            expected_status: 400,
        },
        // MAX_REQUEST_PAYLOAD_BYTES: at the bound, and one past it. The chunk
        // size keeps the answer inside MAX_RESPONSE_FRAMES either way.
        Row {
            case: "payload at MAX_REQUEST_PAYLOAD_BYTES",
            frames: one_frame_of(MAX_REQUEST_PAYLOAD_BYTES),
            chunk_size: MAX_REQUEST_PAYLOAD_BYTES,
            expected_status: 200,
        },
        Row {
            case: "payload one past MAX_REQUEST_PAYLOAD_BYTES",
            frames: one_frame_of(MAX_REQUEST_PAYLOAD_BYTES + 1),
            chunk_size: MAX_REQUEST_PAYLOAD_BYTES,
            expected_status: 400,
        },
        // MAX_RESPONSE_FRAMES: a chunk size of 1 makes the emitted frame count
        // equal the payload length, so the amplification bound is reachable at
        // the bound and one past it. Past it the fault is the ENDPOINT's
        // `chunk_size` meeting a payload the caller was allowed to send, not the
        // request, so it is attributed as a 500 rather than a 400.
        Row {
            case: "emitted frames at MAX_RESPONSE_FRAMES",
            frames: one_frame_of(MAX_RESPONSE_FRAMES),
            chunk_size: 1,
            expected_status: 200,
        },
        Row {
            case: "emitted frames one past MAX_RESPONSE_FRAMES",
            frames: one_frame_of(MAX_RESPONSE_FRAMES + 1),
            chunk_size: 1,
            expected_status: 500,
        },
    ];

    for Row {
        case,
        frames,
        chunk_size,
        expected_status,
    } in rows
    {
        let engine = new_engine();
        register(
            &engine,
            endpoint("bounded", &["AllTraffic"], echo(chunk_size, "UTF8")),
        );
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                "bounded",
                &[],
                encode_request_stream(&frames),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            expected_status,
            "case {case:?}: expected {expected_status}, got {:?}",
            response.status()
        );
        // The fault is attributed to whoever chose the offending dimension: the
        // caller for the request-side caps, the endpoint's own configuration for
        // the amplification cap.
        if expected_status != 200 {
            let body = error_body(case, &response);
            let expected_type = if expected_status == 400 {
                "InputValidationError"
            } else {
                "InternalServerError"
            };
            assert_eq!(
                body["__type"].as_str(),
                Some(expected_type),
                "case {case:?}: expected {expected_type}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_transcript_retains_at_most_the_declared_number_of_invocations() {
    let engine = new_engine();
    register(
        &engine,
        endpoint("bounded-transcript", &["AllTraffic"], echo(64, "UTF8")),
    );
    let router = router(&engine);

    // A few calls past the cap, each carrying a distinguishable payload, so both
    // the retained count and WHICH entries were evicted are observable.
    let calls = MAX_RECORDED_INVOCATIONS + 5;
    for call in 0..calls {
        let payload = format!("call-{call:05}");
        let response = router
            .dispatch(invoke_request(
                "bounded-transcript",
                &[],
                encode_request_stream(&[Frame::new(payload.as_bytes(), "UTF8", "COMPLETE")]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {call}: expected 200, got {:?}",
            response.status()
        );
    }

    let recorded = engine.state::<ServiceState>().lock().invocations.clone();
    assert_eq!(
        recorded.len(),
        MAX_RECORDED_INVOCATIONS,
        "case \"bounded-transcript\": the transcript must retain at most \
         {MAX_RECORDED_INVOCATIONS} entries, got {}",
        recorded.len()
    );
    // The OLDEST entries were the ones evicted, so the window is the most recent.
    let first_retained = String::from_utf8_lossy(&recorded[0].request_frames[0].bytes).to_string();
    assert_eq!(
        first_retained,
        format!("call-{:05}", calls - MAX_RECORDED_INVOCATIONS),
        "case \"bounded-transcript\": the retained window must be the most recent calls, \
         its first entry is {first_retained:?}"
    );
    let last_retained = String::from_utf8_lossy(
        &recorded[MAX_RECORDED_INVOCATIONS - 1].request_frames[0].bytes,
    )
    .to_string();
    assert_eq!(
        last_retained,
        format!("call-{:05}", calls - 1),
        "case \"bounded-transcript\": the newest call must be retained, the last entry is \
         {last_retained:?}"
    );
}

#[tokio::test]
async fn a_recorded_frame_keeps_its_true_length_while_its_bytes_are_budgeted() {
    // Rows differ along how the payload sits relative to the per-entry byte
    // budget: well inside it, exactly at it, and past it.
    let cases: &[(&str, usize)] = &[
        ("well inside the budget", 1024),
        ("exactly at the budget", MAX_RECORDED_INVOCATION_BYTES),
        ("past the budget", MAX_RECORDED_INVOCATION_BYTES + 4096),
    ];

    for (case, len) in cases {
        let engine = new_engine();
        // A chunk size that keeps the answer inside MAX_RESPONSE_FRAMES.
        register(
            &engine,
            endpoint("budgeted", &["AllTraffic"], echo(4096, "UTF8")),
        );
        let router = router(&engine);

        let payload = vec![b'z'; *len];
        let response = router
            .dispatch(invoke_request(
                "budgeted",
                &[],
                encode_request_stream(&[Frame::new(&payload, "UTF8", "COMPLETE")]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?}",
            response.status()
        );

        // The WIRE is never truncated: the whole payload comes back.
        let decoded = decode_response_stream(case, response.body());
        let on_the_wire: usize = payload_parts(&decoded).iter().map(|f| f.payload.len()).sum();
        assert_eq!(
            on_the_wire, *len,
            "case {case:?}: the response must carry all {len} bytes, got {on_the_wire}"
        );

        // The TRANSCRIPT keeps the frame's true length and at most the budget.
        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        let frame = &recorded[0].request_frames[0];
        assert_eq!(
            frame.byte_len, *len,
            "case {case:?}: a recorded frame must report its true length {len}, got {}",
            frame.byte_len
        );
        let expected_retained = (*len).min(MAX_RECORDED_INVOCATION_BYTES);
        assert_eq!(
            frame.bytes.len(),
            expected_retained,
            "case {case:?}: a recorded frame must retain at most \
             {MAX_RECORDED_INVOCATION_BYTES} bytes (expected {expected_retained}), got {}",
            frame.bytes.len()
        );
        let response_frame = &recorded[0].response_frames[0];
        assert!(
            recorded[0]
                .response_frames
                .iter()
                .map(|f| f.bytes.len())
                .sum::<usize>()
                <= MAX_RECORDED_INVOCATION_BYTES,
            "case {case:?}: the recorded response frames must stay inside the budget, \
             first frame holds {} bytes",
            response_frame.bytes.len()
        );
    }
}

#[tokio::test]
async fn the_body_must_declare_the_event_stream_media_type() {
    // The operation's `@httpPayload` is a `@streaming` union, which travels only
    // as `application/vnd.amazon.eventstream`. Rows differ along what the request
    // declares: the media type, the media type with parameters, a different
    // casing, something else, and nothing at all.
    let cases: &[(&str, Option<&str>, u16)] = &[
        ("the media type", Some("application/vnd.amazon.eventstream"), 200),
        (
            "the media type with parameters",
            Some("application/vnd.amazon.eventstream; charset=utf-8"),
            200,
        ),
        (
            "the media type upper-cased",
            Some("Application/VND.amazon.eventstream"),
            200,
        ),
        ("application/json", Some("application/json"), 400),
        ("application/octet-stream", Some("application/octet-stream"), 400),
        ("no Content-Type at all", None, 400),
    ];

    for (case, content_type, expected_status) in cases {
        let engine = new_engine();
        register(&engine, endpoint("typed", &["AllTraffic"], echo(8, "UTF8")));
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request_typed(
                "typed",
                *content_type,
                &[],
                encode_request_stream(&[Frame::new(b"ping", "UTF8", "COMPLETE")]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            *expected_status,
            "case {case:?}: expected {expected_status}, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if *expected_status == 400 {
            let body = error_body(case, &response);
            assert_eq!(
                body["__type"].as_str(),
                Some("InputValidationError"),
                "case {case:?}: expected InputValidationError, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_frame_whose_prelude_or_event_header_is_malformed_is_refused() {
    // A frame's prelude headers must be present and string-valued, and so must
    // every `@eventHeader` the model names: reading a mistyped header as absent
    // would accept a frame AWS refuses and would change what the stream means.
    // Rows differ along which header is wrong and how.
    enum Bad {
        /// Omit `:message-type` entirely.
        NoMessageType,
        /// Send a header with a non-string value type.
        NotAString(&'static str, HeaderValue),
    }

    let cases: Vec<(&str, Bad)> = vec![
        ("no :message-type", Bad::NoMessageType),
        (
            ":message-type as an int32",
            Bad::NotAString(MESSAGE_TYPE, HeaderValue::Int32(1)),
        ),
        (
            ":event-type as a boolean",
            Bad::NotAString(EVENT_TYPE, HeaderValue::Bool(true)),
        ),
        (
            "DataType as an int64",
            Bad::NotAString(DATA_TYPE, HeaderValue::Int64(8)),
        ),
        (
            "CompletionState as a boolean",
            Bad::NotAString(COMPLETION_STATE, HeaderValue::Bool(false)),
        ),
        (
            "P as an int32",
            Bad::NotAString(P, HeaderValue::Int32(42)),
        ),
    ];

    for (case, bad) in cases {
        let engine = new_engine();
        register(&engine, endpoint("prelude", &["AllTraffic"], echo(8, "UTF8")));
        let router = router(&engine);

        // A well-formed frame, then exactly one header made wrong.
        let mut message = Message::new(b"ping".to_vec());
        let overridden = match &bad {
            Bad::NoMessageType => None,
            Bad::NotAString(name, _) => Some(*name),
        };
        for (name, value) in [
            (MESSAGE_TYPE, "event"),
            (EVENT_TYPE, "PayloadPart"),
            (DATA_TYPE, "UTF8"),
            (COMPLETION_STATE, "COMPLETE"),
            (P, "p-value"),
        ] {
            if Some(name) == overridden {
                continue;
            }
            if matches!(bad, Bad::NoMessageType) && name == MESSAGE_TYPE {
                continue;
            }
            message = message.add_header(Header::new(name, HeaderValue::String(value.into())));
        }
        if let Bad::NotAString(name, value) = &bad {
            message = message.add_header(Header::new(*name, value.clone()));
        }

        let mut body = Vec::new();
        write_message_to(&message, &mut body).expect("a test frame must encode");

        let response = router.dispatch(invoke_request("prelude", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected 400, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let parsed = error_body(case, &response);
        assert_eq!(
            parsed["__type"].as_str(),
            Some("InputValidationError"),
            "case {case:?}: expected InputValidationError, got {parsed}"
        );
    }
}

#[tokio::test]
async fn a_transcript_entry_bounds_its_frame_count_and_its_event_headers() {
    // Rows differ along how many frames the stream carries relative to the
    // per-entry frame cap, and how long the one event header the model constrains
    // neither in length nor by pattern (`P`) is relative to its cap.
    let cases: &[(&str, usize, usize)] = &[
        ("inside both caps", 4, 8),
        (
            "frames at the cap",
            MAX_RECORDED_FRAMES_PER_INVOCATION,
            MAX_RECORDED_HEADER_BYTES,
        ),
        (
            "frames past the cap, P past its cap",
            MAX_RECORDED_FRAMES_PER_INVOCATION + 16,
            MAX_RECORDED_HEADER_BYTES + 100,
        ),
    ];

    for (case, frame_count, p_len) in cases {
        let engine = new_engine();
        register(
            &engine,
            endpoint("retention", &["AllTraffic"], echo(4096, "UTF8")),
        );
        let router = router(&engine);

        let p_value = "p".repeat(*p_len);
        // Built as messages directly: the helper's `Frame` carries `&'static str`
        // event headers and cannot hold a generated `P` of this length.
        let mut body = Vec::new();
        for index in 0..*frame_count {
            let completion_state = if index + 1 == *frame_count {
                "COMPLETE"
            } else {
                "PARTIAL"
            };
            let message = Message::new(b"x".to_vec())
                .add_header(Header::new(MESSAGE_TYPE, HeaderValue::String("event".into())))
                .add_header(Header::new(
                    EVENT_TYPE,
                    HeaderValue::String("PayloadPart".into()),
                ))
                .add_header(Header::new(DATA_TYPE, HeaderValue::String("UTF8".into())))
                .add_header(Header::new(
                    COMPLETION_STATE,
                    HeaderValue::String(completion_state.into()),
                ))
                .add_header(Header::new(P, HeaderValue::String(p_value.clone().into())));
            write_message_to(&message, &mut body).expect("a test frame must encode");
        }

        let response = router.dispatch(invoke_request("retention", &[], body)).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        let recorded = engine.state::<ServiceState>().lock().invocations.clone();
        let invocation = &recorded[0];
        assert_eq!(
            invocation.request_frames_total, *frame_count,
            "case {case:?}: the entry must report all {frame_count} frames consumed, got {}",
            invocation.request_frames_total
        );
        let expected_retained = (*frame_count).min(MAX_RECORDED_FRAMES_PER_INVOCATION);
        assert_eq!(
            invocation.request_frames.len(),
            expected_retained,
            "case {case:?}: the entry must retain at most \
             {MAX_RECORDED_FRAMES_PER_INVOCATION} frames (expected {expected_retained}), got {}",
            invocation.request_frames.len()
        );
        for (index, frame) in invocation.request_frames.iter().enumerate() {
            let retained_p = frame.p.as_deref().unwrap_or_default();
            assert_eq!(
                retained_p.len(),
                (*p_len).min(MAX_RECORDED_HEADER_BYTES),
                "case {case:?} frame {index}: a retained event header must keep at most \
                 {MAX_RECORDED_HEADER_BYTES} bytes, got {}",
                retained_p.len()
            );
        }
        // The caller attribution the audit record also names.
        assert_eq!(
            invocation.caller_akid, "AKIDEXAMPLE",
            "case {case:?}: the entry must attribute the invocation to the presented \
             access-key id, got {:?}",
            invocation.caller_akid
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a state-changing invocation emits a structured audit record.
// ---------------------------------------------------------------------------

/// A `MakeWriter` that appends every rendered log line into a shared buffer, so
/// a test can read back the records the handler emitted.
#[derive(Clone)]
struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The one global capture buffer every audit record in this test binary is
/// rendered into.
///
/// It must be GLOBAL rather than per-test: `tracing` caches a callsite's interest
/// against the global dispatcher, so if any other test in this binary reaches the
/// handler's `tracing::info!` while no subscriber is installed, that callsite is
/// cached as disabled and a thread-local subscriber installed later would see
/// nothing.
static CAPTURED_RECORDS: std::sync::OnceLock<Arc<std::sync::Mutex<Vec<u8>>>> =
    std::sync::OnceLock::new();

/// Install the capturing subscriber once, and hand back the shared buffer.
fn captured_records() -> Arc<std::sync::Mutex<Vec<u8>>> {
    Arc::clone(CAPTURED_RECORDS.get_or_init(|| {
        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(CaptureWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        // Ignored if another test binary component already set one: either way a
        // subscriber is in force, which is what the callsite needs.
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    }))
}

/// Every captured record line, as text.
fn captured_text(buffer: &Arc<std::sync::Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(
        &buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .to_string()
}

#[tokio::test]
async fn every_served_invocation_emits_a_structured_audit_record() {
    // Rows differ along the outcome, so the record is pinned for a success, a
    // caller refusal and a model refusal alike — and the record must never carry
    // a payload byte or a header VALUE.
    let cases: &[(&str, &str, u16)] = &[
        ("served", "audited", 200),
        ("refused: unknown endpoint", "not-registered", 400),
        ("refused: model error", "audited-failing", 424),
    ];

    let buffer = captured_records();

    for (case, endpoint_name, expected_status) in cases {
        let engine = new_engine();
        register(&engine, endpoint("audited", &["AllTraffic"], echo(8, "UTF8")));
        register(
            &engine,
            endpoint(
                "audited-failing",
                &["AllTraffic"],
                ModelBehaviour::FailWithModelError {
                    original_status_code: 400,
                    original_message: "the model rejected the payload".to_string(),
                    log_stream_arn: "arn:aws:logs:us-east-1:000000000000:log-group:g:log-stream:s"
                        .to_string(),
                    error_code: "MODEL_REJECTED".to_string(),
                },
            ),
        );
        let router = router(&engine);

        let response = router
            .dispatch(invoke_request(
                endpoint_name,
                &[("X-Amzn-SageMaker-Model-Query-String", "secret=do-not-log")],
                encode_request_stream(&[Frame::new(b"audited-payload", "UTF8", "COMPLETE")]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            *expected_status,
            "case {case:?}: expected {expected_status}, got {:?}",
            response.status()
        );

        // The record for THIS invocation: the line naming the path it addressed.
        let path = format!("/endpoints/{endpoint_name}/invocations-bidirectional-stream");
        let all = captured_text(&buffer);
        let record = all
            .lines()
            .find(|line| line.contains(&path))
            .unwrap_or_else(|| {
                panic!("case {case:?}: no audit record naming {path:?}, captured: {all:?}")
            })
            .to_string();

        for required in [
            "served an InvokeEndpointWithBidirectionalStream invocation".to_string(),
            "operation=\"InvokeEndpointWithBidirectionalStream\"".to_string(),
            "caller_akid=AKIDEXAMPLE".to_string(),
            "service=\"sagemaker\"".to_string(),
            "method=POST".to_string(),
            format!("status={expected_status}"),
        ] {
            assert!(
                record.contains(&required),
                "case {case:?}: the audit record must carry {required:?}, got {record:?}"
            );
        }
        // Never the payload, and never a header VALUE.
        for forbidden in ["audited-payload", "do-not-log"] {
            assert!(
                !record.contains(forbidden),
                "case {case:?}: the audit record must not carry {forbidden:?}, got {record:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The seeded registry, and successive calls sharing one store.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_seeded_registry_serves_an_invocation_and_successive_calls_share_the_transcript() {
    let engine = new_engine();
    let router = router(&engine);

    // Nothing is registered by this test: `my-endpoint` is seeded at state
    // construction, because this service models no operation that could create
    // an endpoint.
    for call in 1..=3u32 {
        let response = router
            .dispatch(invoke_request(
                "my-endpoint",
                &[],
                encode_request_stream(&[
                    Frame::new(b"hello ", "UTF8", "PARTIAL"),
                    Frame::new(b"world", "UTF8", "COMPLETE"),
                ]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {call}: expected 200 from the seeded endpoint, got {:?} body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            response
                .headers()
                .get("x-amzn-invoked-production-variant")
                .and_then(|v| v.to_str().ok()),
            Some("AllTraffic"),
            "case {call}: the seeded endpoint's only variant must be the one reported"
        );

        let recorded = engine.state::<ServiceState>().lock().invocations.len();
        assert_eq!(
            recorded, call as usize,
            "case {call}: successive calls must share one transcript, got {recorded} entries"
        );
    }
}

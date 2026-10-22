// SPDX-License-Identifier: CC0-1.0

//! The streaming session slice, end to end over HTTP: `StartStreamTranscription`,
//! `StartMedicalStreamTranscription`, `StartMedicalScribeStream` and
//! `GetMedicalScribeStream`.
//!
//! Every case drives the real router, so it exercises the whole path a client
//! takes: route-key resolution, the `override_operation` seam that serves an
//! event-stream operation, `vnd.amazon.eventstream` framing in both directions, the
//! session store, and the modeled error envelopes.
//!
//! No case asserts a transcript, because this emulator recognises no speech:
//! [`assert_no_transcript`] pins what a result stream carries instead — the modeled
//! `ServiceUnavailableException` member of its `@streaming` union — so a future
//! change that starts inventing words fails here.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A credential scope naming this service's SigV4 signing name, which is what the
/// router resolves a request's target service from.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/transcribe/aws4_request, \
     SignedHeaders=host, Signature=0";

/// Well-formed `SessionId`s: 8-4-4-4-12 hex, the pattern the model requires.
const SESSION_A: &str = "11111111-2222-4333-8444-555555555555";
const SESSION_B: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const UNKNOWN_SESSION: &str = "99999999-8888-4777-8666-555555555555";
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/HealthScribeStreaming";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_transcribestreaming_transcribe::EmulatedService::new(engine),
    );
    // Signature verification belongs to the emulator, not to this service; these
    // cases are about the operations.
    router.set_request_authentication(false);
    router
}

fn request(
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://transcribestreaming.us-east-1.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    builder
        .body(bytes::Bytes::from(body))
        .expect("the request is well formed")
}

/// One `vnd.amazon.eventstream` frame, as an AWS SDK sends it.
fn frame(event_type: &str, content_type: &str, payload: Vec<u8>) -> Vec<u8> {
    let message = Message::new(payload)
        .add_header(Header::new(
            ":message-type",
            HeaderValue::String("event".into()),
        ))
        .add_header(Header::new(
            ":event-type",
            HeaderValue::String(event_type.to_string().into()),
        ))
        .add_header(Header::new(
            ":content-type",
            HeaderValue::String(content_type.to_string().into()),
        ));
    let mut out = Vec::new();
    write_message_to(&message, &mut out).expect("the frame encodes");
    out
}

/// An `AudioEvent`: `AudioChunk` is the event's `@eventPayload`, so the frame
/// payload is the raw audio.
fn audio_frame(chunk: &[u8]) -> Vec<u8> {
    frame("AudioEvent", "application/octet-stream", chunk.to_vec())
}

fn json_frame(event_type: &str, payload: &Value) -> Vec<u8> {
    frame(
        event_type,
        "application/json",
        serde_json::to_vec(payload).expect("the payload encodes"),
    )
}

/// `len` bytes of deterministic audio, distinct per `seed`.
fn audio(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u64 * 7 + u64::from(seed) * 31) % 251) as u8)
        .collect()
}

fn header_of(response: &http::Response<bytes::Bytes>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .map(|value| value.to_str().expect("a text header").to_string())
}

/// Decode an event-stream response body, asserting the frame headers an SDK
/// decoder requires. Each entry is `(message type, event or exception type,
/// payload)`.
fn decode_frames(case: &str, body: &[u8]) -> Vec<(String, String, Value)> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut events = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|e| {
            panic!("case {case}: the response body must decode as event-stream frames: {e}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|h| h.name().as_str() == name)
                .map(|h| match h.value() {
                    HeaderValue::String(s) => s.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            header(":content-type").as_deref(),
            Some("application/json"),
            "case {case}: an event payload is JSON, got {:?}",
            header(":content-type")
        );
        let message_type = header(":message-type")
            .unwrap_or_else(|| panic!("case {case}: a response frame carried no :message-type"));
        let kind = match message_type.as_str() {
            "event" => header(":event-type"),
            "exception" => header(":exception-type"),
            other => panic!("case {case}: unexpected :message-type {other:?}"),
        }
        .unwrap_or_else(|| panic!("case {case}: a response frame did not name its type"));
        let payload: Value = serde_json::from_slice(message.payload())
            .unwrap_or_else(|e| panic!("case {case}: the event payload must be JSON: {e}"));
        events.push((message_type, kind, payload));
    }
    events
}

/// Every `Start*` result stream must be exactly one in-band
/// `ServiceUnavailableException`: this emulator recognises no speech, and it does
/// not invent a transcript either.
fn assert_no_transcript(case: &str, body: &[u8]) {
    let frames = decode_frames(case, body);
    assert_eq!(
        frames.len(),
        1,
        "case {case}: the result stream carries exactly one event, got {frames:?}"
    );
    let (message_type, kind, payload) = &frames[0];
    assert_eq!(
        message_type, "exception",
        "case {case}: the one event is an in-band exception, got {message_type:?}"
    );
    assert_eq!(
        kind, "ServiceUnavailableException",
        "case {case}: expected the modeled ServiceUnavailableException member, got {kind:?}"
    );
    assert!(
        payload["Message"]
            .as_str()
            .is_some_and(|message| message.contains("no transcript")),
        "case {case}: the exception must say that no transcript is produced, got {payload}"
    );
}

fn body_json(case: &str, response: &http::Response<bytes::Bytes>) -> Value {
    serde_json::from_slice(response.body())
        .unwrap_or_else(|e| panic!("case {case}: the body must be JSON: {e}"))
}

/// The `PostStreamAnalyticsSettings` every `ConfigurationEvent` must carry.
fn analytics_settings() -> Value {
    json!({"ClinicalNoteGenerationSettings": {"OutputBucketName": "scribe-output"}})
}

// =============================================================================
// StartStreamTranscription
// =============================================================================

/// One row of the transcription table: a set of negotiated settings and the audio
/// streamed under them.
struct TranscriptionCase {
    name: &'static str,
    sample_rate: i32,
    encoding: &'static str,
    language: Option<&'static str>,
    /// `Some(n)` turns channel identification on with `NumberOfChannels = n`.
    channels: Option<i32>,
    chunks: Vec<Vec<u8>>,
}

impl TranscriptionCase {
    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![
            (
                "x-amzn-transcribe-sample-rate",
                self.sample_rate.to_string(),
            ),
            (
                "x-amzn-transcribe-media-encoding",
                self.encoding.to_string(),
            ),
        ];
        if let Some(language) = self.language {
            headers.push(("x-amzn-transcribe-language-code", language.to_string()));
        }
        if let Some(channels) = self.channels {
            headers.push((
                "x-amzn-transcribe-number-of-channels",
                channels.to_string(),
            ));
            headers.push((
                "x-amzn-transcribe-enable-channel-identification",
                "true".to_string(),
            ));
        }
        headers
    }

    fn body(&self) -> Vec<u8> {
        self.chunks
            .iter()
            .flat_map(|chunk| audio_frame(chunk))
            .collect()
    }
}

fn transcription_table() -> Vec<TranscriptionCase> {
    vec![
        TranscriptionCase {
            name: "one chunk at the lowest sample rate the model allows",
            sample_rate: 8_000,
            encoding: "pcm",
            language: Some("en-US"),
            channels: None,
            chunks: vec![audio(1, 16_000)],
        },
        TranscriptionCase {
            name: "three unevenly sized chunks",
            sample_rate: 16_000,
            encoding: "pcm",
            language: Some("en-US"),
            channels: None,
            chunks: vec![audio(2, 3_200), audio(3, 6_400), audio(4, 1_600)],
        },
        TranscriptionCase {
            name: "seven chunks at the highest sample rate the model allows, flac, no \
                   language code",
            sample_rate: 48_000,
            encoding: "flac",
            language: None,
            channels: None,
            chunks: (0..7u8).map(|seed| audio(5 + seed, 9_600)).collect(),
        },
        TranscriptionCase {
            name: "twelve chunks over two channels with channel identification",
            sample_rate: 16_000,
            encoding: "ogg-opus",
            language: Some("es-US"),
            channels: Some(2),
            chunks: (0..12u8).map(|seed| audio(20 + seed, 6_400)).collect(),
        },
    ]
}

#[tokio::test]
async fn start_stream_transcription_answers_from_the_session_it_stored() {
    // The rule across the dimensions it generalises over — sample rate, encoding,
    // channel count, and the number and size of the audio chunks. Every row asserts
    // the SAME invariants: a 200 whose modeled response headers are the negotiated
    // settings read back off the stored session, a `vnd.amazon.eventstream` body,
    // and a result stream that reports NO transcript (the emulator recognises no
    // speech) by carrying the modeled `ServiceUnavailableException` instead.
    let router = router();
    for case in &transcription_table() {
        let name = case.name;
        let response = router
            .dispatch(request(
                "POST",
                "/stream-transcription",
                &case.headers(),
                case.body(),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {name}: expected 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header_of(&response, "content-type").as_deref(),
            Some("application/vnd.amazon.eventstream"),
            "case {name}: the body is an event stream, got {:?}",
            header_of(&response, "content-type")
        );

        // The negotiated settings, read back off the stored session.
        let session_id = header_of(&response, "x-amzn-transcribe-session-id")
            .unwrap_or_else(|| panic!("case {name}: no session id header"));
        assert_eq!(
            session_id.len(),
            36,
            "case {name}: a generated SessionId is a 36-char UUID, got {session_id:?}"
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-sample-rate"),
            Some(case.sample_rate.to_string()),
            "case {name}: expected the negotiated sample rate, got {:?}",
            header_of(&response, "x-amzn-transcribe-sample-rate")
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-media-encoding").as_deref(),
            Some(case.encoding),
            "case {name}: expected the negotiated encoding, got {:?}",
            header_of(&response, "x-amzn-transcribe-media-encoding")
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-language-code").as_deref(),
            case.language,
            "case {name}: expected the negotiated language code, got {:?}",
            header_of(&response, "x-amzn-transcribe-language-code")
        );
        let request_id = header_of(&response, "x-amzn-request-id")
            .unwrap_or_else(|| panic!("case {name}: no request id header"));
        assert!(
            !request_id.is_empty(),
            "case {name}: the request id must not be empty, got {request_id:?}"
        );

        // The result stream itself: no transcript, and no invented one either.
        assert_no_transcript(name, response.body());
    }
}

#[tokio::test]
async fn a_request_past_this_emulator_s_capacity_is_refused_as_a_limit() {
    // Caller-chosen work is bounded BEFORE it is done, and the refusal is the error
    // the model names for a client that "has exceeded one of the Amazon Transcribe
    // limits": frames per request, audio per session, and streams running at once.
    use service_com_amazonaws_transcribestreaming_transcribe::state::{
        MAX_AUDIO_BYTES_PER_SESSION, MAX_CONCURRENT_STREAMS, MAX_EVENTS_PER_STREAM,
    };
    let router = router();

    let too_many: Vec<u8> = (0..=MAX_EVENTS_PER_STREAM)
        .flat_map(|index| audio_frame(&audio((index % 251) as u8, 64)))
        .collect();
    let response = router
        .dispatch(request(
            "POST",
            "/stream-transcription",
            &standard_headers("16000", "pcm"),
            too_many,
        ))
        .await;
    assert_eq!(
        response.status().as_u16(),
        429,
        "more than {MAX_EVENTS_PER_STREAM} frames must be refused as a limit, got {} \
         with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let body = body_json("too many frames", &response);
    assert_eq!(
        body["__type"].as_str(),
        Some("LimitExceededException"),
        "expected a LimitExceededException envelope, got {body}"
    );

    let too_much = audio_frame(&audio(9, MAX_AUDIO_BYTES_PER_SESSION as usize + 1));
    let response = router
        .dispatch(request(
            "POST",
            "/stream-transcription",
            &standard_headers("16000", "pcm"),
            too_much,
        ))
        .await;
    assert_eq!(
        response.status().as_u16(),
        429,
        "more than {MAX_AUDIO_BYTES_PER_SESSION} bytes of audio must be refused as a \
         limit, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );

    // Every HealthScribe stream below stays running (no END_OF_SESSION), so the
    // concurrency bound is what refuses the one past it.
    for index in 0..MAX_CONCURRENT_STREAMS {
        let session = format!("{index:08x}-0000-4000-8000-000000000000");
        let mut headers = scribe_headers("16000", "pcm", "en-US");
        headers.push(("x-amzn-transcribe-session-id", session.clone()));
        let response = router
            .dispatch(request(
                "POST",
                "/medical-scribe-stream",
                &headers,
                scribe_body(false),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "stream {index} of {MAX_CONCURRENT_STREAMS} must start, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
    let mut headers = scribe_headers("16000", "pcm", "en-US");
    headers.push((
        "x-amzn-transcribe-session-id",
        "ffffffff-0000-4000-8000-000000000000".to_string(),
    ));
    let response = router
        .dispatch(request(
            "POST",
            "/medical-scribe-stream",
            &headers,
            scribe_body(false),
        ))
        .await;
    assert_eq!(
        response.status().as_u16(),
        429,
        "the stream past the concurrency bound must be refused as a limit, got {} with \
         body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let body = body_json("concurrency", &response);
    assert_eq!(
        body["__type"].as_str(),
        Some("LimitExceededException"),
        "expected a LimitExceededException envelope, got {body}"
    );
}

#[tokio::test]
async fn the_audio_a_stream_received_is_accounted_for_without_being_transcribed() {
    // What the emulator does with the audio it is sent: it accounts for it (so the
    // capacity bounds and the session lifecycle are real) and reports NOTHING about
    // what was said. Streaming more audio, or re-streaming the same audio, never
    // turns into words.
    let router = router();
    let stream = |chunks: Vec<Vec<u8>>| {
        let body: Vec<u8> = chunks.iter().flat_map(|chunk| audio_frame(chunk)).collect();
        let router = &router;
        async move {
            let response = router
                .dispatch(request(
                    "POST",
                    "/stream-transcription",
                    &standard_headers("16000", "pcm"),
                    body,
                ))
                .await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "expected 200, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            assert_no_transcript("audio accounting", response.body());
            // Every session gets its own id and its own request id, whether or not
            // the audio was the same.
            (
                header_of(&response, "x-amzn-transcribe-session-id").expect("a session id"),
                header_of(&response, "x-amzn-request-id").expect("a request id"),
            )
        }
    };

    let first = audio(11, 3_200);
    let second = audio(12, 6_400);
    let (session_a, request_a) = stream(vec![first.clone()]).await;
    let (session_b, request_b) = stream(vec![first.clone(), second]).await;
    let (session_c, request_c) = stream(vec![first]).await;
    let sessions: BTreeSet<&String> = [&session_a, &session_b, &session_c].into_iter().collect();
    assert_eq!(
        sessions.len(),
        3,
        "a generated SessionId is minted per stream, got {sessions:?}"
    );
    let requests: BTreeSet<&String> = [&request_a, &request_b, &request_c].into_iter().collect();
    assert_eq!(
        requests.len(),
        3,
        "a RequestId is minted per request, got {requests:?}"
    );
}

// =============================================================================
// Refusals: BadRequestException
// =============================================================================

/// One row of the refusal table: the headers and body of a `Start*` request, and
/// the status the model gives it.
struct RefusalCase {
    name: &'static str,
    path: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    expected_status: u16,
}

fn standard_headers(rate: &str, encoding: &str) -> Vec<(&'static str, String)> {
    vec![
        ("x-amzn-transcribe-sample-rate", rate.to_string()),
        ("x-amzn-transcribe-media-encoding", encoding.to_string()),
        ("x-amzn-transcribe-language-code", "en-US".to_string()),
    ]
}

fn scribe_headers(rate: &str, encoding: &str, language: &str) -> Vec<(&'static str, String)> {
    vec![
        ("x-amzn-transcribe-sample-rate", rate.to_string()),
        ("x-amzn-transcribe-media-encoding", encoding.to_string()),
        ("x-amzn-transcribe-language-code", language.to_string()),
    ]
}

fn scribe_body(session_control: bool) -> Vec<u8> {
    let mut body = json_frame(
        "ConfigurationEvent",
        &json!({
            "ResourceAccessRoleArn": ROLE_ARN,
            "PostStreamAnalyticsSettings": analytics_settings(),
        }),
    );
    body.extend(audio_frame(&audio(40, 32_000)));
    if session_control {
        body.extend(json_frame(
            "SessionControlEvent",
            &json!({"Type": "END_OF_SESSION"}),
        ));
    }
    body
}

#[tokio::test]
async fn settings_outside_the_model_are_refused_as_bad_request() {
    // The published bounds of `MediaSampleRateHertz` are tested on each bound and one
    // past it, and the closed value sets on a value the set does not hold. A
    // `Start*` operation answers every one of them with the modeled
    // `BadRequestException`, HTTP 400 — never a 200 with an empty stream.
    let audio_body: Vec<u8> = audio_frame(&audio(30, 3_200));
    let cases = vec![
        RefusalCase {
            name: "sample rate one below the modeled minimum",
            path: "/stream-transcription",
            headers: standard_headers("7999", "pcm"),
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "sample rate at the modeled minimum",
            path: "/stream-transcription",
            headers: standard_headers("8000", "pcm"),
            body: audio_body.clone(),
            expected_status: 200,
        },
        RefusalCase {
            name: "sample rate at the modeled maximum",
            path: "/stream-transcription",
            headers: standard_headers("48000", "pcm"),
            body: audio_body.clone(),
            expected_status: 200,
        },
        RefusalCase {
            name: "sample rate one above the modeled maximum",
            path: "/stream-transcription",
            headers: standard_headers("48001", "pcm"),
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "sample rate that is not a number",
            path: "/stream-transcription",
            headers: standard_headers("sixteen-thousand", "pcm"),
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "required sample rate absent",
            path: "/stream-transcription",
            headers: vec![("x-amzn-transcribe-media-encoding", "pcm".to_string())],
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "required media encoding absent",
            path: "/stream-transcription",
            headers: vec![("x-amzn-transcribe-sample-rate", "16000".to_string())],
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "media encoding outside MediaEncoding",
            path: "/stream-transcription",
            headers: standard_headers("16000", "mp3"),
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "language code outside LanguageCode",
            path: "/stream-transcription",
            headers: vec![
                ("x-amzn-transcribe-sample-rate", "16000".to_string()),
                ("x-amzn-transcribe-media-encoding", "pcm".to_string()),
                ("x-amzn-transcribe-language-code", "xx-XX".to_string()),
            ],
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "session id outside the SessionId pattern",
            path: "/stream-transcription",
            headers: {
                let mut headers = standard_headers("16000", "pcm");
                headers.push(("x-amzn-transcribe-session-id", "not-a-session".to_string()));
                headers
            },
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "number of channels below the modeled minimum of 2",
            path: "/stream-transcription",
            headers: {
                let mut headers = standard_headers("16000", "pcm");
                headers.push(("x-amzn-transcribe-number-of-channels", "1".to_string()));
                headers
            },
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "the required AudioStream payload absent",
            path: "/stream-transcription",
            headers: standard_headers("16000", "pcm"),
            body: Vec::new(),
            expected_status: 400,
        },
        RefusalCase {
            name: "a frame naming no member of AudioStream",
            path: "/stream-transcription",
            headers: standard_headers("16000", "pcm"),
            body: json_frame("NotAnAudioEvent", &json!({})),
            expected_status: 400,
        },
        RefusalCase {
            name: "a body that is not event-stream framed",
            path: "/stream-transcription",
            headers: standard_headers("16000", "pcm"),
            body: b"this is not a frame".to_vec(),
            expected_status: 400,
        },
        RefusalCase {
            name: "medical stream without the required Specialty",
            path: "/medical-stream-transcription",
            headers: {
                let mut headers = standard_headers("16000", "pcm");
                headers.push(("x-amzn-transcribe-type", "CONVERSATION".to_string()));
                headers
            },
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "medical stream with a Specialty outside the value set",
            path: "/medical-stream-transcription",
            headers: {
                let mut headers = standard_headers("16000", "pcm");
                headers.push(("x-amzn-transcribe-specialty", "DENTISTRY".to_string()));
                headers.push(("x-amzn-transcribe-type", "CONVERSATION".to_string()));
                headers
            },
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe sample rate one below its own modeled minimum",
            path: "/medical-scribe-stream",
            headers: scribe_headers("15999", "pcm", "en-US"),
            body: scribe_body(true),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe sample rate at its own modeled minimum",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "pcm", "en-US"),
            body: scribe_body(true),
            expected_status: 200,
        },
        RefusalCase {
            name: "HealthScribe encoding outside MedicalScribeMediaEncoding",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "g711-alaw", "en-US"),
            body: scribe_body(true),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe language outside MedicalScribeLanguageCode",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "pcm", "en-GB"),
            body: scribe_body(true),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe stream whose first event is not a ConfigurationEvent",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "pcm", "en-US"),
            body: audio_body.clone(),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe ConfigurationEvent without the required role ARN",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "pcm", "en-US"),
            body: json_frame(
                "ConfigurationEvent",
                &json!({"PostStreamAnalyticsSettings": analytics_settings()}),
            ),
            expected_status: 400,
        },
        RefusalCase {
            name: "HealthScribe ConfigurationEvent with a role ARN outside its pattern",
            path: "/medical-scribe-stream",
            headers: scribe_headers("16000", "pcm", "en-US"),
            body: json_frame(
                "ConfigurationEvent",
                &json!({
                    "ResourceAccessRoleArn": "not-an-arn",
                    "PostStreamAnalyticsSettings": analytics_settings(),
                }),
            ),
            expected_status: 400,
        },
    ];

    let router = router();
    for case in cases {
        let name = case.name;
        let response = router
            .dispatch(request("POST", case.path, &case.headers, case.body))
            .await;
        assert_eq!(
            response.status().as_u16(),
            case.expected_status,
            "case {name}: expected {}, got {} with body {:?}",
            case.expected_status,
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if case.expected_status == 400 {
            let body = body_json(name, &response);
            assert_eq!(
                body["__type"].as_str(),
                Some("BadRequestException"),
                "case {name}: expected a BadRequestException envelope, got {body}"
            );
            assert_eq!(
                header_of(&response, "x-amzn-errortype").as_deref(),
                Some("BadRequestException"),
                "case {name}: expected the error type header, got {:?}",
                header_of(&response, "x-amzn-errortype")
            );
            assert!(
                body["Message"].as_str().is_some_and(|m| !m.is_empty()),
                "case {name}: a refusal must say what was wrong, got {body}"
            );
        }
    }
}

// =============================================================================
// StartMedicalScribeStream + GetMedicalScribeStream
// =============================================================================

#[tokio::test]
async fn a_health_scribe_session_is_readable_after_it_is_started() {
    let router = router();
    let configuration = json!({
        "ResourceAccessRoleArn": ROLE_ARN,
        "VocabularyName": "clinic-terms",
        "VocabularyFilterName": "clinic-filter",
        "VocabularyFilterMethod": "mask",
        "ChannelDefinitions": [
            {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
            {"ChannelId": 1, "ParticipantRole": "PATIENT"},
        ],
        "EncryptionSettings": {"KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab"},
        "PostStreamAnalyticsSettings": analytics_settings(),
        "MedicalScribeContext": {"PatientContext": {"Pronouns": "SHE_HER"}},
    });
    let mut body = json_frame("ConfigurationEvent", &configuration);
    for chunk in [audio(50, 32_000), audio(51, 16_000)] {
        body.extend(audio_frame(&chunk));
    }

    let mut headers = scribe_headers("16000", "pcm", "en-US");
    headers.push(("x-amzn-transcribe-session-id", SESSION_A.to_string()));
    let started = router
        .dispatch(request("POST", "/medical-scribe-stream", &headers, body))
        .await;
    assert_eq!(
        started.status().as_u16(),
        200,
        "starting a HealthScribe stream: expected 200, got {} with body {:?}",
        started.status(),
        String::from_utf8_lossy(started.body())
    );
    assert_eq!(
        header_of(&started, "x-amzn-transcribe-session-id").as_deref(),
        Some(SESSION_A),
        "the session id the caller chose is echoed from the stored session, got {:?}",
        header_of(&started, "x-amzn-transcribe-session-id")
    );
    // The result stream: no transcript segment, and no invented one either.
    assert_no_transcript("health scribe start", started.body());

    // Read the session back: every field below was persisted by the start call.
    let read = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_A}"),
            &[],
            Vec::new(),
        ))
        .await;
    assert_eq!(
        read.status().as_u16(),
        200,
        "reading the session back: expected 200, got {} with body {:?}",
        read.status(),
        String::from_utf8_lossy(read.body())
    );
    let details = body_json("health scribe read", &read)["MedicalScribeStreamDetails"].clone();
    assert_eq!(
        details["SessionId"].as_str(),
        Some(SESSION_A),
        "expected the session id that was started, got {details}"
    );
    assert_eq!(
        details["StreamStatus"].as_str(),
        Some("IN_PROGRESS"),
        "a stream with no END_OF_SESSION is still in progress, got {details}"
    );
    assert!(
        details["StreamCreatedAt"].as_f64().unwrap_or_default() > 0.0,
        "StreamCreatedAt must be set, got {details}"
    );
    assert!(
        details["StreamEndedAt"].is_null(),
        "a stream that has not ended has no StreamEndedAt, got {details}"
    );
    assert_eq!(
        details["LanguageCode"].as_str(),
        Some("en-US"),
        "expected the negotiated language code, got {details}"
    );
    assert_eq!(
        details["MediaSampleRateHertz"].as_i64(),
        Some(16_000),
        "expected the negotiated sample rate, got {details}"
    );
    assert_eq!(
        details["MediaEncoding"].as_str(),
        Some("pcm"),
        "expected the negotiated encoding, got {details}"
    );
    assert_eq!(
        details["ResourceAccessRoleArn"].as_str(),
        Some(ROLE_ARN),
        "expected the role ARN the ConfigurationEvent carried, got {details}"
    );
    assert_eq!(
        details["VocabularyName"].as_str(),
        Some("clinic-terms"),
        "expected the vocabulary the ConfigurationEvent carried, got {details}"
    );
    assert_eq!(
        details["VocabularyFilterMethod"].as_str(),
        Some("mask"),
        "expected the vocabulary filter method the ConfigurationEvent carried, got {details}"
    );
    assert_eq!(
        details["ChannelDefinitions"],
        configuration["ChannelDefinitions"],
        "expected the channel definitions the ConfigurationEvent carried, got {details}"
    );
    assert_eq!(
        details["EncryptionSettings"]["KmsKeyId"],
        configuration["EncryptionSettings"]["KmsKeyId"],
        "expected the encryption settings the ConfigurationEvent carried, got {details}"
    );
    assert_eq!(
        details["PostStreamAnalyticsSettings"],
        analytics_settings(),
        "expected the post-stream analytics settings the ConfigurationEvent carried, \
         got {details}"
    );
    assert_eq!(
        details["MedicalScribeContextProvided"].as_bool(),
        Some(true),
        "a ConfigurationEvent carrying a MedicalScribeContext reports it, got {details}"
    );
    assert!(
        details["PostStreamAnalyticsResult"].is_null(),
        "post-stream analytics only start once the session ends, got {details}"
    );
}

#[tokio::test]
async fn an_unknown_session_is_not_found() {
    let router = router();
    let response = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{UNKNOWN_SESSION}"),
            &[],
            Vec::new(),
        ))
        .await;
    assert_eq!(
        response.status().as_u16(),
        404,
        "a well-formed but unknown session id is not found: got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let body = body_json("unknown session", &response);
    assert_eq!(
        body["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "expected a ResourceNotFoundException envelope, got {body}"
    );
}

#[tokio::test]
async fn a_transcription_session_is_not_a_health_scribe_session() {
    // `GetMedicalScribeStream` reports HealthScribe sessions. A session id that names
    // a plain transcription stream is as absent to it as one never started.
    let router = router();
    let mut headers = standard_headers("16000", "pcm");
    headers.push(("x-amzn-transcribe-session-id", SESSION_B.to_string()));
    let started = router
        .dispatch(request(
            "POST",
            "/stream-transcription",
            &headers,
            audio_frame(&audio(60, 3_200)),
        ))
        .await;
    assert_eq!(
        started.status().as_u16(),
        200,
        "starting the transcription stream: got {} with body {:?}",
        started.status(),
        String::from_utf8_lossy(started.body())
    );
    let read = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_B}"),
            &[],
            Vec::new(),
        ))
        .await;
    assert_eq!(
        read.status().as_u16(),
        404,
        "a transcription session is not a HealthScribe session: got {} with body {:?}",
        read.status(),
        String::from_utf8_lossy(read.body())
    );
}

#[tokio::test]
async fn ending_a_session_completes_it_and_starts_its_post_stream_analytics() {
    let router = router();
    let mut headers = scribe_headers("16000", "pcm", "en-US");
    headers.push(("x-amzn-transcribe-session-id", SESSION_A.to_string()));
    let started = router
        .dispatch(request(
            "POST",
            "/medical-scribe-stream",
            &headers,
            scribe_body(true),
        ))
        .await;
    assert_eq!(
        started.status().as_u16(),
        200,
        "starting the stream: got {} with body {:?}",
        started.status(),
        String::from_utf8_lossy(started.body())
    );

    let read = |router: &MultiServiceRouter| {
        let request = request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_A}"),
            &[],
            Vec::new(),
        );
        let router = router.clone();
        async move {
            let response = router.dispatch(request).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "reading the session: got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            body_json("ended session", &response)["MedicalScribeStreamDetails"].clone()
        }
    };

    let details = read(&router).await;
    assert_eq!(
        details["StreamStatus"].as_str(),
        Some("COMPLETED"),
        "an END_OF_SESSION event completes the stream, got {details}"
    );
    assert!(
        details["StreamEndedAt"].as_f64().unwrap_or_default() > 0.0,
        "a completed stream reports when it ended, got {details}"
    );
    let generation = details["PostStreamAnalyticsResult"]["ClinicalNoteGenerationResult"].clone();
    assert_eq!(
        generation["Status"].as_str(),
        Some("IN_PROGRESS"),
        "post-stream analytics start as soon as the session ends, got {details}"
    );
    assert!(
        generation["ClinicalNoteOutputLocation"].is_null(),
        "there is no output location until the analytics complete, got {details}"
    );

    // The outcome the clock owes: due a fixed span after the session ended, and
    // reported by the next read once it is due. Clinical-note generation summarises
    // a transcript; this emulator produces none, so the analytics end in the model's
    // own FAILED status with a reason rather than claiming an output nothing wrote.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let details = read(&router).await;
    let generation = details["PostStreamAnalyticsResult"]["ClinicalNoteGenerationResult"].clone();
    assert_eq!(
        generation["Status"].as_str(),
        Some("FAILED"),
        "the analytics reach a terminal status once their span has elapsed, got {details}"
    );
    assert!(
        generation["FailureReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no speech recognition")),
        "a failed analytics result must say why, got {details}"
    );
    assert!(
        generation["ClinicalNoteOutputLocation"].is_null()
            && generation["TranscriptOutputLocation"].is_null(),
        "no output location may be reported for a note nothing wrote, got {details}"
    );
    assert_eq!(
        details["StreamStatus"].as_str(),
        Some("COMPLETED"),
        "settling the analytics must not change the stream status, got {details}"
    );
}

#[tokio::test]
async fn a_second_stream_on_a_live_session_terminates_the_first_and_conflicts() {
    let router = router();
    let mut headers = scribe_headers("16000", "pcm", "en-US");
    headers.push(("x-amzn-transcribe-session-id", SESSION_B.to_string()));
    // No END_OF_SESSION, so this stream is still running.
    let first = router
        .dispatch(request(
            "POST",
            "/medical-scribe-stream",
            &headers,
            scribe_body(false),
        ))
        .await;
    assert_eq!(
        first.status().as_u16(),
        200,
        "starting the first stream: got {} with body {:?}",
        first.status(),
        String::from_utf8_lossy(first.body())
    );
    let before = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_B}"),
            &[],
            Vec::new(),
        ))
        .await;
    let created_at =
        body_json("live session", &before)["MedicalScribeStreamDetails"]["StreamCreatedAt"].clone();

    let second = router
        .dispatch(request(
            "POST",
            "/medical-scribe-stream",
            &headers,
            scribe_body(false),
        ))
        .await;
    assert_eq!(
        second.status().as_u16(),
        409,
        "a second stream on a live session id conflicts: got {} with body {:?}",
        second.status(),
        String::from_utf8_lossy(second.body())
    );
    let body = body_json("conflict", &second);
    assert_eq!(
        body["__type"].as_str(),
        Some("ConflictException"),
        "expected a ConflictException envelope, got {body}"
    );

    let read = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_B}"),
            &[],
            Vec::new(),
        ))
        .await;
    let details = body_json("conflicted session", &read)["MedicalScribeStreamDetails"].clone();
    assert_eq!(
        details["StreamStatus"].as_str(),
        Some("FAILED"),
        "the stream that was running has been terminated, got {details}"
    );
    assert!(
        details["StreamEndedAt"].as_f64().unwrap_or_default() > 0.0,
        "a terminated stream reports when it ended, got {details}"
    );
    // The refused stream was NOT stored: the session the store holds is still the
    // one that was running, so the response and the state agree.
    assert_eq!(
        details["StreamCreatedAt"], created_at,
        "the refused stream must not have replaced the stored session, got {details}"
    );
}

// =============================================================================
// StartMedicalStreamTranscription
// =============================================================================

#[tokio::test]
async fn medical_stream_transcription_reports_its_specialty_and_type_from_the_session() {
    let router = router();
    // Each row is one (Specialty, Type) pair from the two value sets the model
    // declares, including values the issue did not cite.
    let cases = [
        ("PRIMARYCARE", "CONVERSATION"),
        ("CARDIOLOGY", "DICTATION"),
        ("RADIOLOGY", "CONVERSATION"),
        ("UROLOGY", "DICTATION"),
    ];
    for (specialty, stream_type) in cases {
        let mut headers = standard_headers("16000", "pcm");
        headers.push(("x-amzn-transcribe-specialty", specialty.to_string()));
        headers.push(("x-amzn-transcribe-type", stream_type.to_string()));
        headers.push((
            "x-amzn-transcribe-content-identification-type",
            "PHI".to_string(),
        ));
        headers.push(("x-amzn-transcribe-show-speaker-label", "true".to_string()));
        let response = router
            .dispatch(request(
                "POST",
                "/medical-stream-transcription",
                &headers,
                audio_frame(&audio(70, 32_000)),
            ))
            .await;
        let name = specialty;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {name}: expected 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-specialty").as_deref(),
            Some(specialty),
            "case {name}: the specialty is read back off the stored session, got {:?}",
            header_of(&response, "x-amzn-transcribe-specialty")
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-type").as_deref(),
            Some(stream_type),
            "case {name}: the type is read back off the stored session, got {:?}",
            header_of(&response, "x-amzn-transcribe-type")
        );
        assert_eq!(
            header_of(&response, "x-amzn-transcribe-content-identification-type").as_deref(),
            Some("PHI"),
            "case {name}: expected PHI content identification, got {:?}",
            header_of(&response, "x-amzn-transcribe-content-identification-type")
        );
        assert_no_transcript(name, response.body());
    }
}

#[tokio::test]
async fn call_analytics_streaming_is_not_served_by_this_slice() {
    // The fourth `Start*` operation is modeled but not implemented, and must keep
    // saying so rather than answering an empty 200.
    let router = router();
    let response = router
        .dispatch(request(
            "POST",
            "/call-analytics-stream-transcription",
            &standard_headers("16000", "pcm"),
            audio_frame(&audio(80, 3_200)),
        ))
        .await;
    assert_eq!(
        response.status().as_u16(),
        501,
        "an operation this slice does not serve must answer the framework's 501 \
         'modeled but not served' response, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    assert!(
        !response.body().is_empty(),
        "the refusal must carry a body naming it, got an empty body"
    );
}

#[tokio::test]
async fn one_api_s_stream_does_not_terminate_another_api_s_session() {
    // The conflict rule is keyed by the operation that opened the stream as well as
    // by the session id: a transcription stream that happens to reuse a running
    // HealthScribe session's id finds the id taken (409) but must NOT advance that
    // session's status — one API does not mutate another API's resource.
    let router = router();
    let mut scribe = scribe_headers("16000", "pcm", "en-US");
    scribe.push(("x-amzn-transcribe-session-id", SESSION_A.to_string()));
    let started = router
        .dispatch(request(
            "POST",
            "/medical-scribe-stream",
            &scribe,
            scribe_body(false),
        ))
        .await;
    assert_eq!(
        started.status().as_u16(),
        200,
        "starting the HealthScribe stream: got {} with body {:?}",
        started.status(),
        String::from_utf8_lossy(started.body())
    );

    let mut transcription = standard_headers("16000", "pcm");
    transcription.push(("x-amzn-transcribe-session-id", SESSION_A.to_string()));
    let collided = router
        .dispatch(request(
            "POST",
            "/stream-transcription",
            &transcription,
            audio_frame(&audio(90, 3_200)),
        ))
        .await;
    assert_eq!(
        collided.status().as_u16(),
        409,
        "a session id held by a running stream is refused: got {} with body {:?}",
        collided.status(),
        String::from_utf8_lossy(collided.body())
    );
    assert_eq!(
        body_json("cross-api collision", &collided)["__type"].as_str(),
        Some("ConflictException"),
        "expected a ConflictException envelope"
    );

    let read = router
        .dispatch(request(
            "GET",
            &format!("/medical-scribe-stream/{SESSION_A}"),
            &[],
            Vec::new(),
        ))
        .await;
    let details = body_json("untouched session", &read)["MedicalScribeStreamDetails"].clone();
    assert_eq!(
        details["StreamStatus"].as_str(),
        Some("IN_PROGRESS"),
        "the HealthScribe session must be untouched by another API's collision, \
         got {details}"
    );
    assert!(
        details["StreamEndedAt"].is_null(),
        "the untouched session must not have been ended, got {details}"
    );
}

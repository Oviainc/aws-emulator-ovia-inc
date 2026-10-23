// SPDX-License-Identifier: CC0-1.0

//! The WebSocket transport of the `Start*` streaming operations: the data-plane path
//! `/<modeled uri>-websocket`, where the operation's input is carried as query
//! parameters instead of `x-amzn-transcribe-*` headers.
//!
//! Two rules are pinned here, over the class of requests each one generalises over
//! rather than over one recorded request:
//!
//! 1. The WebSocket path of a streaming operation is SERVED BY THAT OPERATION. It is
//!    claimed by this service (whose signing name the upgrade carries in its
//!    query-signed `X-Amz-Credential`, not in an `Authorization` header), routed to
//!    the operation the path names, read onto the same modeled input shape the header
//!    transport fills, validated against the same model constraints, and answered
//!    with that operation's OWN modeled errors — never with the router's
//!    "no emulated service is registered" 404, which is an "this API does not exist"
//!    shape no Transcribe client can branch on.
//! 2. An input combination the model forbids ACROSS two members — a member whose
//!    documentation makes it conditional on another member being enabled — is
//!    refused with the modeled `BadRequestException` (400) on BOTH transports of
//!    every operation that binds the pair.
//!
//! The upgrade itself is refused, with the modeled `ServiceUnavailableException`
//! (503) whose message is `NO_WEBSOCKET_TRANSPORT`: this emulator's front door
//! answers one buffered request per call and cannot switch a connection to the
//! WebSocket protocol, so a `101 Switching Protocols` would promise a stream nothing
//! could carry. Every row below therefore asserts a MODELED envelope — 400 for a
//! request the model refuses, 503 for one it accepts — and never a routing 404.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope a query-signed (presigned) upgrade carries, naming
/// this service's signing name. The `/` separators arrive percent-encoded, exactly
/// as a query-signed request spells them.
const QUERY_CREDENTIAL: &str = "AKIDEXAMPLE%2F20260101%2Fus-east-1%2Ftranscribe%2Faws4_request";

/// The SigV4 query-signing parameters, which carry the signature rather than any
/// member of the operation's input: the emulator verifies them, and the operation
/// must not read them as settings.
const SIGV4_PARAMETERS: &str = "X-Amz-Algorithm=AWS4-HMAC-SHA256\
     &X-Amz-Date=20260101T000000Z\
     &X-Amz-Expires=300\
     &X-Amz-SignedHeaders=host\
     &X-Amz-Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A credential scope naming this service's signing name in an `Authorization`
/// header, for the HTTP/2 transport cases.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/transcribe/aws4_request, \
     SignedHeaders=host, Signature=0";

/// A well-formed `SessionId`: 8-4-4-4-12 hex, the pattern the model requires.
const SESSION_ID: &str = "11111111-2222-4333-8444-555555555555";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_transcribestreaming_transcribe::EmulatedService::new(engine),
    );
    // Signature verification belongs to the emulator, not to this service; these
    // cases are about the transport and the operation's own refusals.
    router.set_request_authentication(false);
    router
}

/// The WebSocket upgrade a client sends: `GET <path>?<parameters>&<SigV4>` with the
/// handshake headers and no `Authorization` header, so the request routes by its
/// query-signed credential scope.
fn upgrade(path: &str, parameters: &str) -> http::Request<bytes::Bytes> {
    let separator = if parameters.is_empty() { "" } else { "&" };
    http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://transcribestreaming.us-east-1.amazonaws.com{path}\
             ?{parameters}{separator}X-Amz-Credential={QUERY_CREDENTIAL}&{SIGV4_PARAMETERS}"
        ))
        .header("host", "transcribestreaming.us-east-1.amazonaws.com")
        .header("upgrade", "websocket")
        .header("connection", "Upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(bytes::Bytes::new())
        .expect("the upgrade request is well formed")
}

/// A request on the modeled HTTP/2 transport: the same input in
/// `x-amzn-transcribe-*` headers, with the audio in the body.
fn header_transport(
    path: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method("POST")
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

/// One `vnd.amazon.eventstream` `AudioEvent` frame, as an AWS SDK sends it:
/// `AudioChunk` is the event's `@eventPayload`, so the payload is the raw audio.
fn audio_frame(chunk: &[u8]) -> Vec<u8> {
    use aws_smithy_eventstream::frame::write_message_to;
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
    let message = Message::new(chunk.to_vec())
        .add_header(Header::new(
            ":message-type",
            HeaderValue::String("event".into()),
        ))
        .add_header(Header::new(
            ":event-type",
            HeaderValue::String("AudioEvent".into()),
        ))
        .add_header(Header::new(
            ":content-type",
            HeaderValue::String("application/octet-stream".into()),
        ));
    let mut out = Vec::new();
    write_message_to(&message, &mut out).expect("the frame encodes");
    out
}

fn body_json(case: &str, response: &http::Response<bytes::Bytes>) -> Value {
    serde_json::from_slice(response.body())
        .unwrap_or_else(|e| panic!("case {case}: the body must be JSON: {e}"))
}

fn header_of(response: &http::Response<bytes::Bytes>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .map(|value| value.to_str().expect("a text header").to_string())
}

/// Assert the response is the modeled error `shape` with the status the model gives
/// it, in this service's own envelope — and so is NOT a router-level refusal.
fn assert_modeled_error(
    case: &str,
    response: &http::Response<bytes::Bytes>,
    status: u16,
    shape: &str,
) {
    assert_eq!(
        response.status().as_u16(),
        status,
        "case {case}: expected {status}, got {:?} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let body = body_json(case, response);
    assert_eq!(
        body["__type"].as_str(),
        Some(shape),
        "case {case}: expected a {shape} envelope, got {body}"
    );
    assert_eq!(
        header_of(response, "x-amzn-errortype").as_deref(),
        Some(shape),
        "case {case}: expected the {shape} error-type header, got {:?}",
        header_of(response, "x-amzn-errortype")
    );
    assert!(
        body["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case}: a refusal must say what was wrong, got {body}"
    );
}

/// One upgrade case: the data-plane path, the query spelling of the operation's
/// input, and the modeled answer it must get.
struct Case {
    name: &'static str,
    path: &'static str,
    parameters: &'static str,
    status: u16,
    shape: &'static str,
}

/// `StartStreamTranscription`'s WebSocket path.
const STANDARD: &str = "/stream-transcription-websocket";
/// `StartMedicalStreamTranscription`'s WebSocket path.
const MEDICAL: &str = "/medical-stream-transcription-websocket";

/// The rows differ along what the two rules generalise over: which operation's
/// WebSocket path is addressed, which member the parameters name, whether a value is
/// inside or outside the member's modeled constraint (each published bound and one
/// past it), whether a required member is present at all, and whether a
/// conditional member's companion is enabled.
///
/// The accepted values are the model's own catalogues, and every row below that is
/// not in the issue's own report is marked — `media-encoding` is the full
/// `MediaEncoding` enum (smithy_query(query="shape", shape="MediaEncoding"):
/// pcm | ogg-opus | flac | g711-alaw | g711-ulaw | g729), `partial-results-stability`
/// the full `PartialResultsStability` enum (high | medium | low), the sample rate the
/// published `@range` 8000..48000, and `specialty` the `Specialty` enum.
const CASES: &[Case] = &[
    // ---- accepted input: the modeled 503, because the stream cannot be carried ----
    Case {
        name: "the minimal accepted upgrade",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "media-encoding=ogg-opus (a modeled value the issue does not cite)",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=ogg-opus&sample-rate=16000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "media-encoding=flac",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=flac&sample-rate=16000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "media-encoding=g711-alaw",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=g711-alaw&sample-rate=8000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "media-encoding=g711-ulaw",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=g711-ulaw&sample-rate=8000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "media-encoding=g729 (a modeled value the issue does not cite)",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=g729&sample-rate=8000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "sample-rate at the published minimum of 8000",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=8000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "sample-rate at the published maximum of 48000",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=48000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "language-code=zu-ZA (a modeled language the issue does not cite)",
        path: STANDARD,
        parameters: "language-code=zu-ZA&media-encoding=pcm&sample-rate=16000",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "two channels WITH channel identification",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-channel-identification=true&number-of-channels=2",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "partial-results-stability=high WITH stabilization enabled",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-partial-results-stabilization=true&partial-results-stability=high",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "partial-results-stability=medium WITH stabilization enabled",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-partial-results-stabilization=true&partial-results-stability=medium",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "partial-results-stability=low WITH stabilization enabled",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-partial-results-stabilization=true&partial-results-stability=low",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        // `language-options` is percent-encoded, as a query parameter carrying a
        // comma-separated list is: undecoded, `en-US%2Ces-US` breaks the member's
        // `@pattern` (`^[a-zA-Z-,]+$`) and this row would be a 400.
        name: "language identification with a percent-encoded language-options list",
        path: STANDARD,
        parameters: "media-encoding=pcm&sample-rate=16000&identify-language=true\
                     &language-options=en-US%2Ces-US&preferred-language=en-US\
                     &show-speaker-label=true&vocabulary-filter-name=profanity\
                     &vocabulary-filter-method=mask&session-resume-window=60",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    // ---- refused input: the modeled 400, on the same path ----
    Case {
        name: "media-encoding=mp3 is not a modeled MediaEncoding",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=mp3&sample-rate=16000",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "media-encoding absent, though the model requires it",
        path: STANDARD,
        parameters: "language-code=en-US&sample-rate=16000",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "sample-rate absent, though the model requires it",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "sample-rate=4000, far below the published minimum",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=4000",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "sample-rate=7999, one below the published minimum",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=7999",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "sample-rate=48001, one above the published maximum",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=48001",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "sample-rate that is not a number at all",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=sixteen-thousand",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "language-code=en-XX is not a modeled LanguageCode",
        path: STANDARD,
        parameters: "language-code=en-XX&media-encoding=pcm&sample-rate=16000",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "partial-results-stability=extreme is not a modeled level",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-partial-results-stabilization=true&partial-results-stability=extreme",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "vocabulary-filter-method=redact is not a modeled method",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &vocabulary-filter-name=profanity&vocabulary-filter-method=redact",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "session-id that is not the modeled 36-character UUID",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &session-id=not-a-session-id",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "number-of-channels=2 WITHOUT channel identification",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &number-of-channels=2",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "number-of-channels=1, below the published minimum of 2",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &enable-channel-identification=true&number-of-channels=1",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "partial-results-stability WITHOUT stabilization enabled",
        path: STANDARD,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &partial-results-stability=high",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "no parameters at all: both required members are absent",
        path: STANDARD,
        parameters: "",
        status: 400,
        shape: "BadRequestException",
    },
    // ---- the medical operation's WebSocket path: the same two rules ----
    Case {
        name: "the minimal accepted medical upgrade",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &specialty=PRIMARYCARE&type=CONVERSATION",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "specialty=UROLOGY, type=DICTATION (modeled values the issue does not cite)",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=ogg-opus&sample-rate=48000\
                     &specialty=UROLOGY&type=DICTATION&show-speaker-label=true",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "medical two channels WITH channel identification",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &specialty=CARDIOLOGY&type=CONVERSATION\
                     &enable-channel-identification=true&number-of-channels=2",
        status: 503,
        shape: "ServiceUnavailableException",
    },
    Case {
        name: "medical number-of-channels=2 WITHOUT channel identification",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &specialty=CARDIOLOGY&type=CONVERSATION&number-of-channels=2",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "medical specialty absent, though the model requires it",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &type=CONVERSATION",
        status: 400,
        shape: "BadRequestException",
    },
    Case {
        name: "medical specialty=DENTISTRY is not a modeled Specialty",
        path: MEDICAL,
        parameters: "language-code=en-US&media-encoding=pcm&sample-rate=16000\
                     &specialty=DENTISTRY&type=CONVERSATION",
        status: 400,
        shape: "BadRequestException",
    },
];

#[tokio::test]
async fn the_websocket_data_plane_is_served_by_the_operation_its_path_names() {
    let router = router();
    for case in CASES {
        let response = router.dispatch(upgrade(case.path, case.parameters)).await;
        assert_modeled_error(case.name, &response, case.status, case.shape);
        // The whole point of the rule: whatever the answer is, it is this
        // operation's, not the router's "this API does not exist".
        assert_ne!(
            header_of(&response, "x-amzn-errortype").as_deref(),
            Some("UnknownService"),
            "case {}: a claimed data-plane path must never answer UnknownService",
            case.name
        );
    }
}

#[tokio::test]
async fn an_accepted_upgrade_is_refused_as_a_transport_not_as_a_bad_request() {
    // The 503 says WHY the stream was not carried, so a client is not left guessing
    // whether its parameters were wrong.
    let router = router();
    let response = router
        .dispatch(upgrade(
            STANDARD,
            "language-code=en-US&media-encoding=pcm&sample-rate=16000",
        ))
        .await;
    let body = body_json("accepted upgrade", &response);
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains("WebSocket") && message.contains("101"),
        "the refusal must name the transport it cannot serve and the handshake it \
         cannot answer, got {message:?}"
    );
    assert!(
        message.contains("HTTP/2"),
        "the refusal must name the transport that IS served, got {message:?}"
    );
}

#[tokio::test]
async fn a_near_miss_websocket_path_is_not_claimed() {
    // Claiming is exact first-segment equality, as it is for the modeled paths: a
    // path that merely resembles the data plane's is not this service's, and the
    // router answers for it.
    let router = router();
    for path in [
        "/stream-transcriptions-websocket",
        "/stream-transcription-websockets",
        "/medical-scribe-stream-websocket",
    ] {
        let response = router
            .dispatch(upgrade(
                path,
                "language-code=en-US&media-encoding=pcm&sample-rate=16000",
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            404,
            "case {path}: a path this service does not publish must not be claimed, \
             got {:?} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

#[tokio::test]
async fn a_refused_upgrade_stores_no_session() {
    // No stream was carried, so no session exists: the session id the upgrade named
    // is still free, which the HTTP/2 transport proves by starting a stream on it
    // (a stored session would answer 409 ConflictException) and reading the stored
    // id back off the response.
    let router = router();
    let refused = router
        .dispatch(upgrade(
            STANDARD,
            &format!(
                "language-code=en-US&media-encoding=pcm&sample-rate=16000&session-id={SESSION_ID}"
            ),
        ))
        .await;
    assert_modeled_error(
        "session id claimed by an upgrade",
        &refused,
        503,
        "ServiceUnavailableException",
    );

    let started = router
        .dispatch(header_transport(
            "/stream-transcription",
            &[
                ("x-amzn-transcribe-sample-rate", "16000".to_string()),
                ("x-amzn-transcribe-media-encoding", "pcm".to_string()),
                ("x-amzn-transcribe-language-code", "en-US".to_string()),
                ("x-amzn-transcribe-session-id", SESSION_ID.to_string()),
            ],
            audio_frame(&[7u8; 3_200]),
        ))
        .await;
    assert_eq!(
        started.status().as_u16(),
        200,
        "the session id a refused upgrade named must still be free, got {:?} with \
         body {:?}",
        started.status(),
        String::from_utf8_lossy(started.body())
    );
    assert_eq!(
        header_of(&started, "x-amzn-transcribe-session-id").as_deref(),
        Some(SESSION_ID),
        "the stored session must be the one that was started, got {:?}",
        header_of(&started, "x-amzn-transcribe-session-id")
    );
}

/// One header-transport case: the headers, and whether the model accepts the
/// combination they spell.
struct HeaderCase {
    name: &'static str,
    path: &'static str,
    headers: &'static [(&'static str, &'static str)],
    status: u16,
}

/// The cross-member rules hold on the MODELED transport too — same operation, same
/// members, same refusal — so each pair is tested with the companion member absent
/// (refused) and present (accepted), on every operation that binds it.
const HEADER_CASES: &[HeaderCase] = &[
    HeaderCase {
        name: "channels without channel identification",
        path: "/stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-number-of-channels", "2"),
        ],
        status: 400,
    },
    HeaderCase {
        name: "channels with channel identification",
        path: "/stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-number-of-channels", "2"),
            ("x-amzn-transcribe-enable-channel-identification", "true"),
        ],
        status: 200,
    },
    HeaderCase {
        name: "stability without stabilization",
        path: "/stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-partial-results-stability", "low"),
        ],
        status: 400,
    },
    HeaderCase {
        name: "stability with stabilization",
        path: "/stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-partial-results-stability", "low"),
            (
                "x-amzn-transcribe-enable-partial-results-stabilization",
                "true",
            ),
        ],
        status: 200,
    },
    HeaderCase {
        name: "medical channels without channel identification",
        path: "/medical-stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-specialty", "PRIMARYCARE"),
            ("x-amzn-transcribe-type", "CONVERSATION"),
            ("x-amzn-transcribe-number-of-channels", "2"),
        ],
        status: 400,
    },
    HeaderCase {
        name: "medical channels with channel identification",
        path: "/medical-stream-transcription",
        headers: &[
            ("x-amzn-transcribe-sample-rate", "16000"),
            ("x-amzn-transcribe-media-encoding", "pcm"),
            ("x-amzn-transcribe-language-code", "en-US"),
            ("x-amzn-transcribe-specialty", "PRIMARYCARE"),
            ("x-amzn-transcribe-type", "CONVERSATION"),
            ("x-amzn-transcribe-number-of-channels", "2"),
            ("x-amzn-transcribe-enable-channel-identification", "true"),
        ],
        status: 200,
    },
];

#[tokio::test]
async fn the_cross_member_rules_hold_on_the_modeled_transport_too() {
    let router = router();
    for case in HEADER_CASES {
        let headers: Vec<(&str, String)> = case
            .headers
            .iter()
            .map(|(name, value)| (*name, (*value).to_string()))
            .collect();
        let response = router
            .dispatch(header_transport(
                case.path,
                &headers,
                audio_frame(&[3u8; 1_600]),
            ))
            .await;
        assert_eq!(
            response.status().as_u16(),
            case.status,
            "case {}: expected {}, got {:?} with body {:?}",
            case.name,
            case.status,
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if case.status == 400 {
            assert_modeled_error(case.name, &response, 400, "BadRequestException");
        }
    }
}

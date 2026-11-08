// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `StartMedicalScribeListeningSession`, driven over the same
//! router the emulator serves with, and read back through
//! `GetMedicalScribeListeningSession`.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs
//! spanning the class that rule covers: every value of each enumerated header rather
//! than one, each published bound of the sample rate and one past it, every member of
//! the input stream's union rather than one, and each link of the (`domainId`,
//! `subscriptionId`) scope rather than one.
//!
//! The enums, bounds and patterns asserted here are the model's own
//! (`smithy_query(query="operation", operation="StartMedicalScribeListeningSession")`,
//! `smithy_query(query="shape", shape="MedicalScribeInputStream,MedicalScribeOutputStream")`),
//! quoted beside each constant.

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::ops::start_medical_scribe_listening_session::{
    MODELED_PATH, NO_RECOGNISER,
};
use service_com_amazonaws_connecthealth_connecthealth::state::MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION;

use crate::domain_subscription_lifecycle::{error_code, Emulator};

/// The media type an event-stream body carries, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The frame headers an SDK decoder reads.
const MESSAGE_TYPE: &str = ":message-type";
const EVENT_TYPE: &str = ":event-type";
const EXCEPTION_TYPE: &str = ":exception-type";

/// `MedicalScribeLanguageCode`'s whole value set.
const LANGUAGE_CODES: &[&str] = &["en-US", "multi"];
/// `MedicalScribeMediaEncoding`'s whole value set.
const MEDIA_ENCODINGS: &[&str] = &["pcm", "flac"];
/// `MedicalScribeMediaSampleRateHertz`: `@range` 8000..48000.
const SAMPLE_RATE_RANGE: (i64, i64) = (8000, 48000);

/// The response headers the model binds the output's members to.
const HEADER_SESSION_ID: &str = "x-amzn-medscribe-session-id";
const HEADER_DOMAIN_ID: &str = "x-amzn-medscribe-domain-id";
const HEADER_SUBSCRIPTION_ID: &str = "x-amzn-medscribe-subscription-id";
const HEADER_LANGUAGE_CODE: &str = "x-amzn-medscribe-language-code";
const HEADER_SAMPLE_RATE: &str = "x-amzn-medscribe-sample-rate";
const HEADER_MEDIA_ENCODING: &str = "x-amzn-medscribe-media-encoding";
const HEADER_REQUEST_ID: &str = "x-amzn-request-id";

/// One `sessionId` of the modelled form: 36 characters matching
/// `[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}`.
fn session_id(seed: u64) -> String {
    format!("{seed:08x}-1a2b-4c3d-8e4f-{seed:012x}")
}

/// Frame one input event the way an SDK frames it: the union member's name in
/// `:event-type`, and the event structure as the JSON payload.
fn event_frame(event_type: &str, payload: &Value) -> Vec<u8> {
    raw_frame(
        "event",
        EVENT_TYPE,
        event_type,
        serde_json::to_vec(payload).expect("a serializable event"),
    )
}

/// One frame with the `:message-type`, the named header and the payload bytes given —
/// the frame writer the malformed-stream rows reach for.
fn raw_frame(
    message_type: &str,
    type_header: &'static str,
    event_type: &str,
    payload: Vec<u8>,
) -> Vec<u8> {
    let mut message = Message::new(payload).add_header(Header::new(
        MESSAGE_TYPE,
        HeaderValue::String(message_type.to_string().into()),
    ));
    if !event_type.is_empty() {
        message = message.add_header(Header::new(
            type_header,
            HeaderValue::String(event_type.to_string().into()),
        ));
    }
    let mut framed = Vec::new();
    write_message_to(&message, &mut framed).expect("an in-memory frame writer cannot fail");
    framed
}

/// A `configurationEvent` carrying `template` as its note template, the two channel
/// definitions the model's `@length 2..2` list requires, and an encounter context when
/// `context` is set.
fn configuration_event(template: &Value, channels: bool, context: Option<&str>) -> Vec<u8> {
    let mut event = json!({
        "postStreamActionSettings": {
            "outputS3Uri": "s3://clinic-notes/encounters/",
            "clinicalNoteGenerationSettings": { "noteTemplateSettings": template },
        }
    });
    if channels {
        event["channelDefinitions"] = json!([
            { "channelId": 0, "participantRole": "PATIENT" },
            { "channelId": 1, "participantRole": "CLINICIAN" },
        ]);
    }
    if let Some(context) = context {
        event["encounterContext"] = json!({ "unstructuredContext": context });
    }
    event_frame("configurationEvent", &event)
}

/// An `audioEvent`: `MedicalScribeAudioEvent$audioChunk` is an ordinary blob member, so
/// the chunk travels base64-encoded inside the JSON payload. `QUFB` is the three bytes
/// `AAA`.
fn audio_event() -> Vec<u8> {
    event_frame("audioEvent", &json!({ "audioChunk": "QUFB" }))
}

/// A `binaryAudioEvent`: its `audioChunk` is the event's `@eventPayload`, so the frame
/// payload IS the raw chunk.
fn binary_audio_event(chunk: &[u8]) -> Vec<u8> {
    raw_frame("event", EVENT_TYPE, "binaryAudioEvent", chunk.to_vec())
}

/// The `sessionControlEvent` that ends a session.
fn end_of_session() -> Vec<u8> {
    event_frame(
        "sessionControlEvent",
        &json!({ "type": "END_OF_SESSION" }),
    )
}

/// The request headers of a start: the six the model requires, plus whatever a row
/// overrides. An override with an empty value REMOVES that header, which is how a row
/// asks for a request that omits a required member.
fn start_headers<'h>(
    session: &'h str,
    domain: &'h str,
    subscription: &'h str,
    language: &'h str,
    sample_rate: &'h str,
    encoding: &'h str,
) -> Vec<(&'h str, &'h str)> {
    vec![
        ("content-type", EVENT_STREAM),
        (HEADER_SESSION_ID, session),
        (HEADER_DOMAIN_ID, domain),
        (HEADER_SUBSCRIPTION_ID, subscription),
        (HEADER_LANGUAGE_CODE, language),
        (HEADER_SAMPLE_RATE, sample_rate),
        (HEADER_MEDIA_ENCODING, encoding),
    ]
}

/// One decoded response frame.
#[derive(Debug, Clone)]
struct Frame {
    message_type: String,
    event_type: String,
    payload: Vec<u8>,
}

/// Decode a response body into its frames.
fn decode_frames(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let read = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default()
        };
        let message_type = read(MESSAGE_TYPE);
        let event_type = if message_type == "exception" {
            read(EXCEPTION_TYPE)
        } else {
            read(EVENT_TYPE)
        };
        frames.push(Frame {
            message_type,
            event_type,
            payload: message.payload().to_vec(),
        });
    }
    frames
}

/// A response's header, as a string.
fn header(case: &str, response: &http::Response<Bytes>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no {name} header"))
        .to_str()
        .unwrap_or_else(|_| panic!("case {case:?}: the {name} header is not text"))
        .to_string()
}

/// The error document of a refused start.
fn refusal_body(case: &str, response: &http::Response<Bytes>) -> Value {
    serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: a refusal carries a JSON error document ({error}), got {:?}",
            String::from_utf8_lossy(response.body())
        )
    })
}

/// The wire error code of a refusal, read off the body the way the sibling suites read
/// it.
fn refusal_code(body: &Value) -> String {
    error_code(body)
}

/// A live domain and subscription to start sessions on.
async fn scope(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator.create_domain(&json!({ "name": name })).await;
    let domain_id = created
        .get("domainId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateDomain must report a domainId, got {created:?}"))
        .to_string();
    let subscription_id = emulator.create_subscription(&domain_id).await;
    (domain_id, subscription_id)
}

/// `GetMedicalScribeListeningSession`'s details member for one scope.
async fn read_session(
    emulator: &Emulator,
    domain_id: &str,
    subscription_id: &str,
    session: &str,
) -> (u16, Value) {
    let (status, body) = emulator
        .send(
            "GET",
            &format!(
                "/medical-scribe-stream/domain/{domain_id}/subscription/{subscription_id}\
                 /session/{session}"
            ),
            None,
        )
        .await;
    (status, body)
}

/// One row of the start table.
struct StartRow {
    case: &'static str,
    language: &'static str,
    sample_rate: i64,
    encoding: &'static str,
    /// The events the row frames into the request body.
    events: Vec<Vec<u8>>,
    /// What the row's configuration event asked for, `None` when it sent none.
    template_member: Option<&'static str>,
    template_type: Option<&'static str>,
    channels: bool,
    context: bool,
    /// Whether the row's stream carried audio, so a transcript was asked for.
    audio: bool,
    /// Whether the row ended its session.
    ended: bool,
}

/// RULE: a started listening session is PERSISTED under exactly the (`domainId`,
/// `subscriptionId`) pair the request named, its response headers are read back out of
/// that record, and `GetMedicalScribeListeningSession` reports what the stream
/// configured.
#[tokio::test]
async fn a_started_session_is_persisted_and_read_back_through_its_scope() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;

    let managed = json!({ "managedTemplate": { "templateType": "GIRPP" } });
    // `CustomTemplateBase` admits a different set from `ManagedNoteTemplate`
    // (`smithy_query(query="shape", shape="CustomTemplateBase,ManagedNoteTemplate")`), so
    // the custom row names a value from that set and two section instructions, inside the
    // modelled `@length 1..20` list.
    let custom = json!({
        "customTemplate": {
            "templateType": "BIRP",
            "templateInstructions": [
                { "sectionHeader": "Behavior", "sectionInstruction": "Summarise the behaviour" },
                { "sectionHeader": "Plan", "sectionInstruction": "List the next steps" },
            ],
        }
    });

    let rows = [
        StartRow {
            case: "managed template, lowest modelled sample rate, ended by the client",
            language: LANGUAGE_CODES[0],
            sample_rate: SAMPLE_RATE_RANGE.0,
            encoding: MEDIA_ENCODINGS[0],
            events: vec![configuration_event(&managed, true, Some("Annual visit")), end_of_session()],
            template_member: Some("managedTemplate"),
            template_type: Some("GIRPP"),
            channels: true,
            context: true,
            audio: false,
            ended: true,
        },
        StartRow {
            case: "custom template, highest modelled sample rate, audio still flowing",
            language: LANGUAGE_CODES[1],
            sample_rate: SAMPLE_RATE_RANGE.1,
            encoding: MEDIA_ENCODINGS[1],
            events: vec![configuration_event(&custom, false, None), audio_event()],
            template_member: Some("customTemplate"),
            template_type: Some("BIRP"),
            channels: false,
            context: false,
            audio: true,
            ended: false,
        },
        StartRow {
            case: "raw binary audio only, nothing configured",
            language: LANGUAGE_CODES[0],
            sample_rate: 16_000,
            encoding: MEDIA_ENCODINGS[0],
            events: vec![binary_audio_event(&[1, 2, 3, 4, 5]), end_of_session()],
            template_member: None,
            template_type: None,
            channels: false,
            context: false,
            audio: true,
            ended: true,
        },
        StartRow {
            case: "an opened stream that carries no event at all",
            language: LANGUAGE_CODES[1],
            sample_rate: 44_100,
            encoding: MEDIA_ENCODINGS[1],
            events: Vec::new(),
            template_member: None,
            template_type: None,
            channels: false,
            context: false,
            audio: false,
            ended: false,
        },
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let session = session_id(index as u64 + 1);
        let rate = row.sample_rate.to_string();
        let headers = start_headers(
            &session,
            &domain_id,
            &subscription_id,
            row.language,
            &rate,
            row.encoding,
        );
        let body: Vec<u8> = row.events.iter().flatten().copied().collect();
        let response = emulator
            .send_bytes("POST", MODELED_PATH, &headers, body)
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a start on a live scope is served, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        // The response headers are the output members the model binds, each read back out
        // of the stored record.
        assert_eq!(
            header(case, &response, HEADER_SESSION_ID),
            session,
            "case {case:?}: the response reports the session that was started"
        );
        assert_eq!(
            header(case, &response, HEADER_DOMAIN_ID),
            domain_id,
            "case {case:?}: the response reports the domain the session belongs to"
        );
        assert_eq!(
            header(case, &response, HEADER_SUBSCRIPTION_ID),
            subscription_id,
            "case {case:?}: the response reports the subscription the session belongs to"
        );
        assert_eq!(
            header(case, &response, HEADER_LANGUAGE_CODE),
            row.language,
            "case {case:?}: the response reports the language the session negotiated"
        );
        assert_eq!(
            header(case, &response, HEADER_SAMPLE_RATE),
            rate,
            "case {case:?}: the response reports the sample rate the session negotiated"
        );
        assert_eq!(
            header(case, &response, HEADER_MEDIA_ENCODING),
            row.encoding,
            "case {case:?}: the response reports the encoding the session negotiated"
        );
        assert_eq!(
            header(case, &response, "content-type"),
            EVENT_STREAM,
            "case {case:?}: the body is an event stream"
        );

        // `RequestId` is `@length 1..256` and identifies the CALL, so every call gets its
        // own.
        let request_id = header(case, &response, HEADER_REQUEST_ID);
        assert!(
            (1..=256).contains(&request_id.len()),
            "case {case:?}: the request id is within its modelled bound, got {request_id:?}"
        );
        assert!(
            !request_ids.contains(&request_id),
            "case {case:?}: every call gets its own request id, but {request_id:?} repeated"
        );
        request_ids.push(request_id);

        // The response stream: no transcript is invented, and a stream that carried audio
        // is told so through the modelled in-band member.
        let frames = decode_frames(case, response.body());
        if row.audio {
            assert_eq!(
                frames.len(),
                1,
                "case {case:?}: a stream that carried audio answers with one in-band event, \
                 got {frames:?}"
            );
            let frame = &frames[0];
            assert_eq!(
                (frame.message_type.as_str(), frame.event_type.as_str()),
                ("exception", "internalFailureException"),
                "case {case:?}: the in-band event is the modelled failure member, got {frame:?}"
            );
            let reported: Value = serde_json::from_slice(&frame.payload).unwrap_or_else(|error| {
                panic!("case {case:?}: the in-band payload must be JSON ({error})")
            });
            assert_eq!(
                reported.get("message").and_then(Value::as_str),
                Some(NO_RECOGNISER),
                "case {case:?}: the in-band failure says why no transcript was produced, got \
                 {reported:?}"
            );
        } else {
            assert!(
                frames.is_empty(),
                "case {case:?}: a stream that carried no audio answers with no event, got \
                 {frames:?}"
            );
        }

        // The ROUND TRIP: the reader reports the record the start persisted.
        let (status, body) = read_session(&emulator, &domain_id, &subscription_id, &session).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the started session is readable, got {body:?}"
        );
        let details = body
            .get("medicalScribeListeningSessionDetails")
            .unwrap_or_else(|| panic!("case {case:?}: the reader reports details, got {body:?}"));
        assert_eq!(
            details.get("sessionId").and_then(Value::as_str),
            Some(session.as_str()),
            "case {case:?}: the stored session carries its own id, got {details:?}"
        );
        assert_eq!(
            details.get("languageCode").and_then(Value::as_str),
            Some(row.language),
            "case {case:?}: the stored session carries the negotiated language, got {details:?}"
        );
        assert_eq!(
            details.get("mediaSampleRateHertz").and_then(Value::as_i64),
            Some(row.sample_rate),
            "case {case:?}: the stored session carries the negotiated sample rate, got \
             {details:?}"
        );
        assert_eq!(
            details.get("mediaEncoding").and_then(Value::as_str),
            Some(row.encoding),
            "case {case:?}: the stored session carries the negotiated encoding, got {details:?}"
        );
        let expected_status = if row.ended {
            "COMPLETED"
        } else {
            "IN_PROGRESS"
        };
        assert_eq!(
            details.get("streamStatus").and_then(Value::as_str),
            Some(expected_status),
            "case {case:?}: END_OF_SESSION is what completes a stream, got {details:?}"
        );
        assert_eq!(
            details.get("streamEndTime").is_some(),
            row.ended,
            "case {case:?}: only an ended stream carries an end instant, got {details:?}"
        );
        assert!(
            details.get("streamCreationTime").is_some(),
            "case {case:?}: every session carries its creation instant, got {details:?}"
        );
        assert_eq!(
            details.get("encounterContextProvided").and_then(Value::as_bool),
            Some(row.context),
            "case {case:?}: the flag follows whether the stream sent an encounter context, \
             got {details:?}"
        );
        let channels = details
            .get("channelDefinitions")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            channels,
            if row.channels { 2 } else { 0 },
            "case {case:?}: the stored channel definitions are the ones the stream sent, got \
             {details:?}"
        );
        match (row.template_member, row.template_type) {
            (Some(member), Some(template_type)) => {
                let settings = details
                    .get("postStreamActionSettings")
                    .unwrap_or_else(|| panic!("case {case:?}: configured settings are reported, got {details:?}"));
                assert_eq!(
                    settings.get("outputS3Uri").and_then(Value::as_str),
                    Some("s3://clinic-notes/encounters/"),
                    "case {case:?}: the configured output location is stored, got {settings:?}"
                );
                let reported = settings
                    .get("clinicalNoteGenerationSettings")
                    .and_then(|notes| notes.get("noteTemplateSettings"))
                    .and_then(|template| template.get(member))
                    .and_then(|template| template.get("templateType"))
                    .and_then(Value::as_str);
                assert_eq!(
                    reported,
                    Some(template_type),
                    "case {case:?}: the configured note template is stored under {member}, got \
                     {settings:?}"
                );
            }
            _ => assert!(
                details.get("postStreamActionSettings").is_none(),
                "case {case:?}: a session nothing configured reports no settings, got {details:?}"
            ),
        }
        // Nothing ran, so nothing is reported about the post-stream artifacts.
        assert!(
            details.get("postStreamActionResult").is_none(),
            "case {case:?}: no post-stream result is invented, got {details:?}"
        );
    }
}

/// One row of the refusal table: the headers to override, and the modelled failure.
struct RefusalRow {
    case: &'static str,
    /// `(header, value)`; an empty value omits the header.
    overrides: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
}

/// RULE: a start whose header-bound members break the model's own rules — an absent
/// required member, a value outside an enumerated set, a value outside the declared
/// range or pattern — is the modelled `ValidationException`, and one naming a scope this
/// Region does not hold live is the modelled `ResourceNotFoundException`.
#[tokio::test]
async fn a_start_outside_the_model_or_outside_a_live_scope_is_refused() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;
    let other_scope = scope(&emulator, "other-clinic").await;

    let mut rows = vec![
        RefusalRow {
            case: "no sessionId header at all",
            overrides: vec![(HEADER_SESSION_ID, String::new())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "no languageCode header at all",
            overrides: vec![(HEADER_LANGUAGE_CODE, String::new())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "no sample rate header at all",
            overrides: vec![(HEADER_SAMPLE_RATE, String::new())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "no mediaEncoding header at all",
            overrides: vec![(HEADER_MEDIA_ENCODING, String::new())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a sessionId outside the modelled UUID pattern",
            overrides: vec![(HEADER_SESSION_ID, "not-a-uuid-at-all-not-a-uuid-at-all1".to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a languageCode outside the modelled enum",
            overrides: vec![(HEADER_LANGUAGE_CODE, "fr-FR".to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a mediaEncoding outside the modelled enum",
            overrides: vec![(HEADER_MEDIA_ENCODING, "ogg-opus".to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a sample rate one below the modelled minimum",
            overrides: vec![(HEADER_SAMPLE_RATE, (SAMPLE_RATE_RANGE.0 - 1).to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a sample rate one above the modelled maximum",
            overrides: vec![(HEADER_SAMPLE_RATE, (SAMPLE_RATE_RANGE.1 + 1).to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a domainId outside the modelled pattern",
            overrides: vec![(HEADER_DOMAIN_ID, "DOM-0123456789abcdef".to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a subscriptionId outside the modelled pattern",
            overrides: vec![(HEADER_SUBSCRIPTION_ID, "sub-tooshort".to_string())],
            status: 400,
            code: "ValidationException",
        },
        RefusalRow {
            case: "a well-formed domainId no domain of this Region carries",
            overrides: vec![(HEADER_DOMAIN_ID, "dom-0123456789abcdef0".to_string())],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "a well-formed subscriptionId this domain does not carry",
            overrides: vec![(
                HEADER_SUBSCRIPTION_ID,
                "sub-000000000000000000000".to_string(),
            )],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "a subscription of ANOTHER domain",
            overrides: vec![(HEADER_SUBSCRIPTION_ID, other_scope.1.clone())],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalRow {
            case: "a domain of ANOTHER domain's subscription",
            overrides: vec![(HEADER_DOMAIN_ID, other_scope.0.clone())],
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    for (index, row) in rows.iter_mut().enumerate() {
        let case = row.case;
        let session = session_id(100 + index as u64);
        let rate = 16_000.to_string();
        let mut headers = start_headers(
            &session,
            &domain_id,
            &subscription_id,
            LANGUAGE_CODES[0],
            &rate,
            MEDIA_ENCODINGS[0],
        );
        for (name, value) in &row.overrides {
            headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
            if !value.is_empty() {
                headers.push((name, value.as_str()));
            }
        }
        let response = emulator
            .send_bytes(
                "POST",
                MODELED_PATH,
                &headers,
                configuration_event(
                    &json!({ "managedTemplate": { "templateType": "DAP" } }),
                    false,
                    None,
                ),
            )
            .await;
        assert_eq!(
            response.status().as_u16(),
            row.status,
            "case {case:?}: expected HTTP {}, got {} {:?}",
            row.status,
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = refusal_body(case, &response);
        assert_eq!(
            refusal_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body:?}",
            row.code
        );
        // This service reports an exception's detail text in its `Message` member, and a
        // refused stream is no exception to that.
        assert!(
            body.get("Message").and_then(Value::as_str).is_some(),
            "case {case:?}: the refusal states why, got {body:?}"
        );
        // A refused start writes nothing: the session it named is readable nowhere.
        let (status, read) =
            read_session(&emulator, &domain_id, &subscription_id, &session).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused start persists no session, got {read:?}"
        );
    }
}

/// One row of the malformed-stream table.
struct StreamRow {
    case: &'static str,
    body: Vec<u8>,
}

/// RULE: every event of the input stream is read and checked against the model's rules
/// for the member its `:event-type` names, so a frame this service cannot place, or a
/// payload the model refuses, is the modelled `ValidationException` and stores nothing.
#[tokio::test]
async fn an_input_stream_the_model_refuses_stores_nothing() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;

    let rows = vec![
        StreamRow {
            case: "a body that is not an event-stream message stream at all",
            body: b"not a frame".to_vec(),
        },
        StreamRow {
            case: "a frame a client may not send: :message-type exception",
            body: raw_frame("exception", EXCEPTION_TYPE, "validationException", b"{}".to_vec()),
        },
        StreamRow {
            case: "a frame naming no event",
            body: raw_frame("event", EVENT_TYPE, "", b"{}".to_vec()),
        },
        StreamRow {
            case: "an event type that is no member of MedicalScribeInputStream",
            body: event_frame("transcriptEvent", &json!({})),
        },
        StreamRow {
            case: "a configuration event without the settings the model requires",
            body: event_frame("configurationEvent", &json!({})),
        },
        StreamRow {
            case: "a configuration event whose channel definitions are not the modelled pair",
            body: event_frame(
                "configurationEvent",
                &json!({
                    "postStreamActionSettings": {
                        "outputS3Uri": "s3://clinic-notes/encounters/",
                        "clinicalNoteGenerationSettings": {
                            "noteTemplateSettings": { "managedTemplate": { "templateType": "DAP" } }
                        },
                    },
                    "channelDefinitions": [{ "channelId": 0, "participantRole": "PATIENT" }],
                }),
            ),
        },
        StreamRow {
            case: "a configuration event whose output location is no S3 URI",
            body: event_frame(
                "configurationEvent",
                &json!({
                    "postStreamActionSettings": {
                        "outputS3Uri": "https://clinic-notes.example/encounters/",
                        "clinicalNoteGenerationSettings": {
                            "noteTemplateSettings": { "managedTemplate": { "templateType": "DAP" } }
                        },
                    }
                }),
            ),
        },
        StreamRow {
            case: "a configuration event naming a managed template outside the modelled enum",
            body: event_frame(
                "configurationEvent",
                &json!({
                    "postStreamActionSettings": {
                        "outputS3Uri": "s3://clinic-notes/encounters/",
                        "clinicalNoteGenerationSettings": {
                            "noteTemplateSettings": {
                                "managedTemplate": { "templateType": "NOT_A_TEMPLATE" }
                            }
                        },
                    }
                }),
            ),
        },
        StreamRow {
            case: "a session control event outside the modelled enum",
            body: event_frame("sessionControlEvent", &json!({ "type": "PAUSE" })),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let session = session_id(200 + index as u64);
        let rate = 16_000.to_string();
        let headers = start_headers(
            &session,
            &domain_id,
            &subscription_id,
            LANGUAGE_CODES[0],
            &rate,
            MEDIA_ENCODINGS[0],
        );
        let response = emulator
            .send_bytes("POST", MODELED_PATH, &headers, row.body.clone())
            .await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: an input stream the model refuses is a 400, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let body = refusal_body(case, &response);
        assert_eq!(
            refusal_code(&body),
            "ValidationException",
            "case {case:?}: the modelled refusal is ValidationException, got {body:?}"
        );
        let (status, read) =
            read_session(&emulator, &domain_id, &subscription_id, &session).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused stream persists no session, got {read:?}"
        );
    }
}

/// RULE: the `sessionId` is the caller's, so starting a stream under an id a
/// subscription already holds RESUMES that session — one record, keeping the instant its
/// stream was first created — while a fresh id adds one; and a session is reachable only
/// through the scope it was started in.
#[tokio::test]
async fn a_repeated_session_id_resumes_one_session_and_a_scope_never_leaks() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;
    let sibling = emulator.create_subscription(&domain_id).await;
    let session = session_id(7);

    let rate = 16_000.to_string();
    let headers = start_headers(
        &session,
        &domain_id,
        &subscription_id,
        LANGUAGE_CODES[0],
        &rate,
        MEDIA_ENCODINGS[0],
    );
    let first = emulator
        .send_bytes(
            "POST",
            MODELED_PATH,
            &headers,
            configuration_event(
                &json!({ "managedTemplate": { "templateType": "DAP" } }),
                true,
                Some("First visit"),
            ),
        )
        .await;
    assert_eq!(
        first.status().as_u16(),
        200,
        "the first start is served, got {:?}",
        String::from_utf8_lossy(first.body())
    );
    let (_, opened) = read_session(&emulator, &domain_id, &subscription_id, &session).await;
    let opened = opened
        .get("medicalScribeListeningSessionDetails")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        opened.get("streamStatus").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "a stream nobody ended is still running, got {opened:?}"
    );
    let created_at = opened.get("streamCreationTime").cloned();

    // The same id again, with a different configuration and an END_OF_SESSION: one
    // record, carried forward.
    let resumed = emulator
        .send_bytes(
            "POST",
            MODELED_PATH,
            &headers,
            [
                configuration_event(
                    &json!({ "managedTemplate": { "templateType": "BIRP" } }),
                    false,
                    None,
                ),
                end_of_session(),
            ]
            .concat(),
        )
        .await;
    assert_eq!(
        resumed.status().as_u16(),
        200,
        "resuming a session the caller named is served, got {:?}",
        String::from_utf8_lossy(resumed.body())
    );
    let (_, read) = read_session(&emulator, &domain_id, &subscription_id, &session).await;
    let details = read
        .get("medicalScribeListeningSessionDetails")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        details.get("streamCreationTime").cloned(),
        created_at,
        "a resumed session keeps the instant its stream was created, got {details:?}"
    );
    assert_eq!(
        details.get("streamStatus").and_then(Value::as_str),
        Some("COMPLETED"),
        "the resuming stream's END_OF_SESSION completes the session, got {details:?}"
    );
    assert_eq!(
        details
            .get("postStreamActionSettings")
            .and_then(|settings| settings.get("clinicalNoteGenerationSettings"))
            .and_then(|notes| notes.get("noteTemplateSettings"))
            .and_then(|template| template.get("managedTemplate"))
            .and_then(|template| template.get("templateType"))
            .and_then(Value::as_str),
        Some("BIRP"),
        "the resuming stream's configuration is what the session reports, got {details:?}"
    );
    assert_eq!(
        details
            .get("channelDefinitions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "the resuming stream sent no channel definitions, got {details:?}"
    );

    // The same session id under a SIBLING subscription is a different session, and
    // neither is readable through the other's scope.
    let sibling_headers = start_headers(
        &session,
        &domain_id,
        &sibling,
        LANGUAGE_CODES[1],
        &rate,
        MEDIA_ENCODINGS[1],
    );
    let on_sibling = emulator
        .send_bytes("POST", MODELED_PATH, &sibling_headers, Vec::new())
        .await;
    assert_eq!(
        on_sibling.status().as_u16(),
        200,
        "a session of the same id on another subscription is its own, got {:?}",
        String::from_utf8_lossy(on_sibling.body())
    );
    let (_, sibling_read) = read_session(&emulator, &domain_id, &sibling, &session).await;
    let sibling_details = sibling_read
        .get("medicalScribeListeningSessionDetails")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        sibling_details.get("languageCode").and_then(Value::as_str),
        Some(LANGUAGE_CODES[1]),
        "each scope reports its own session, got {sibling_details:?}"
    );
    assert_eq!(
        sibling_details.get("streamStatus").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "the sibling's session was never ended, got {sibling_details:?}"
    );
    assert_eq!(
        details.get("streamStatus").and_then(Value::as_str),
        Some("COMPLETED"),
        "and the first scope's session is unchanged by it, got {details:?}"
    );
}

/// RULE: the sessions one subscription holds are bounded, and a start past that bound is
/// the modelled `ServiceQuotaExceededException` that stores nothing — while a start that
/// resumes a session the subscription already holds consumes none of it.
#[tokio::test]
async fn the_sessions_one_subscription_holds_are_bounded() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;
    let rate = 16_000.to_string();

    for index in 0..MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION {
        let session = session_id(1_000 + index as u64);
        let headers = start_headers(
            &session,
            &domain_id,
            &subscription_id,
            LANGUAGE_CODES[0],
            &rate,
            MEDIA_ENCODINGS[0],
        );
        let response = emulator
            .send_bytes("POST", MODELED_PATH, &headers, Vec::new())
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "session {index} is inside the bound of {MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION}, got \
             {:?}",
            String::from_utf8_lossy(response.body())
        );
    }

    // One past the bound, with an id the subscription does not hold.
    let past = session_id(9_999);
    let headers = start_headers(
        &past,
        &domain_id,
        &subscription_id,
        LANGUAGE_CODES[0],
        &rate,
        MEDIA_ENCODINGS[0],
    );
    let response = emulator
        .send_bytes("POST", MODELED_PATH, &headers, Vec::new())
        .await;
    assert_eq!(
        response.status().as_u16(),
        402,
        "a start past the bound is the modelled quota failure, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let body = refusal_body("past the bound", &response);
    assert_eq!(
        refusal_code(&body),
        "ServiceQuotaExceededException",
        "a start past the bound is the modelled quota failure, got {body:?}"
    );
    let (status, read) = read_session(&emulator, &domain_id, &subscription_id, &past).await;
    assert_eq!(
        status, 404,
        "a refused start stores nothing, got {read:?}"
    );

    // A session the subscription already holds is still startable: resuming adds no
    // record, so it cannot be at capacity.
    let held = session_id(1_000);
    let headers = start_headers(
        &held,
        &domain_id,
        &subscription_id,
        LANGUAGE_CODES[0],
        &rate,
        MEDIA_ENCODINGS[0],
    );
    let response = emulator
        .send_bytes("POST", MODELED_PATH, &headers, end_of_session())
        .await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "resuming a held session consumes no quota, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let (_, read) = read_session(&emulator, &domain_id, &subscription_id, &held).await;
    assert_eq!(
        read.get("medicalScribeListeningSessionDetails")
            .and_then(|details| details.get("streamStatus"))
            .and_then(Value::as_str),
        Some("COMPLETED"),
        "and the resumed session records what that stream did, got {read:?}"
    );
}

/// RULE: a session is scoped to the Region its domain was created in, exactly as every
/// other resource of this service is: an endpoint of another Region holds none of it.
#[tokio::test]
async fn a_session_is_scoped_to_the_region_its_domain_lives_in() {
    let emulator = Emulator::new("us-east-1");
    let (domain_id, subscription_id) = scope(&emulator, "clinic").await;
    let session = session_id(42);
    let rate = 16_000.to_string();
    let headers = start_headers(
        &session,
        &domain_id,
        &subscription_id,
        LANGUAGE_CODES[0],
        &rate,
        MEDIA_ENCODINGS[0],
    );

    let elsewhere = emulator
        .send_bytes_in("eu-west-1", "POST", MODELED_PATH, &headers, Vec::new())
        .await;
    assert_eq!(
        elsewhere.status().as_u16(),
        404,
        "another Region's endpoint holds no domain of this one, got {} {:?}",
        elsewhere.status(),
        String::from_utf8_lossy(elsewhere.body())
    );
    let body = refusal_body("another Region", &elsewhere);
    assert_eq!(
        refusal_code(&body),
        "ResourceNotFoundException",
        "another Region's endpoint answers the modelled 404, got {body:?}"
    );

    let served = emulator
        .send_bytes("POST", MODELED_PATH, &headers, Vec::new())
        .await;
    assert_eq!(
        served.status().as_u16(),
        200,
        "and the domain's own Region serves it, got {:?}",
        String::from_utf8_lossy(served.body())
    );
}

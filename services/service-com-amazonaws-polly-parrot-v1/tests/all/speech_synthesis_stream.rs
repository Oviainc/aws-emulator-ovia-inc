// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartSpeechSynthesisStream`, driven over HTTP through the
//! same router the emulator serves with — so what is asserted is the wire behaviour
//! a client sees: the status, the `vnd.amazon.eventstream` frames and their headers,
//! and the error document of a refusal.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs
//! spanning the class that rule covers: every audio format rather than one, several
//! action-event sequences rather than one, each header rule rather than the one an
//! issue happens to quote. The invariant most of these rest on is a CROSS-OPERATION
//! one: a stream answers with the audio `SynthesizeSpeech` answers for the same
//! text, voice, engine and format — so neither operation can drift from the other.

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_polly_parrot_v1::ops::start_speech_synthesis_stream::{
    MAX_ACTION_EVENTS, MAX_STREAM_CHARACTERS, MAX_STREAM_SECONDS, STREAMING_ENGINE,
};
use service_com_amazonaws_polly_parrot_v1::ops::synthesize_speech::plan::MAX_UTTERANCE_MS;

use crate::lexicons_and_speech::{error_code, header, json as json_body, pls, Polly};

/// The URI the model binds the operation to.
const STREAM_PATH: &str = "/v1/synthesisStream";

/// The media type an event-stream body carries, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The region the test's credential scope names, and therefore the region whose
/// lexicon store a streamed request resolves `LexiconNames` in.
const REGION: &str = "us-east-1";

/// A voice the catalog records as supporting the one engine this operation accepts.
const STREAMING_VOICE: &str = "Joanna";

/// `:message-type`, `:event-type` and `:content-type`: the three frame headers an
/// SDK decoder reads.
const MESSAGE_TYPE: &str = ":message-type";
const EVENT_TYPE: &str = ":event-type";
const CONTENT_TYPE: &str = ":content-type";

/// One decoded frame of a response stream.
#[derive(Debug, Clone)]
struct Frame {
    event_type: String,
    content_type: String,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload read as the JSON object a non-`@eventPayload` event
    /// carries.
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.payload).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the {} payload must be JSON ({error}), got {:?}",
                self.event_type,
                String::from_utf8_lossy(&self.payload)
            )
        })
    }
}

/// Frame one input action event the way an SDK frames it: the union member's name in
/// `:event-type`, and the event structure as the JSON payload.
fn action_frame(event_type: &str, payload: Value) -> Vec<u8> {
    let message = Message::new(serde_json::to_vec(&payload).expect("a serializable event"))
        .add_header(Header::new(
            MESSAGE_TYPE,
            HeaderValue::String("event".into()),
        ))
        .add_header(Header::new(
            EVENT_TYPE,
            HeaderValue::String(event_type.to_string().into()),
        ))
        .add_header(Header::new(
            CONTENT_TYPE,
            HeaderValue::String("application/json".into()),
        ));
    let mut framed = Vec::new();
    write_message_to(&message, &mut framed).expect("an in-memory frame writer cannot fail");
    framed
}

/// A `TextEvent` carrying `text`, optionally with a `TextType` and a forced flush.
fn text_event(text: &str, text_type: Option<&str>, force: Option<bool>) -> Vec<u8> {
    let mut event = json!({ "Text": text });
    if let Some(text_type) = text_type {
        event["TextType"] = json!(text_type);
    }
    if let Some(force) = force {
        event["FlushStreamConfiguration"] = json!({ "Force": force });
    }
    action_frame("TextEvent", event)
}

/// The `CloseStreamEvent` that ends an input stream. It models no member, so its
/// payload is the empty object.
fn close_stream_event() -> Vec<u8> {
    action_frame("CloseStreamEvent", json!({}))
}

/// The request headers of a stream: the three required members plus whatever a row
/// overrides or adds.
fn stream_headers<'h>(overrides: &[(&'h str, &'h str)]) -> Vec<(&'h str, &'h str)> {
    let mut headers: Vec<(&str, &str)> = vec![
        ("content-type", EVENT_STREAM),
        ("x-amzn-Engine", STREAMING_ENGINE),
        ("x-amzn-OutputFormat", "pcm"),
        ("x-amzn-VoiceId", STREAMING_VOICE),
    ];
    for (name, value) in overrides {
        // An override REPLACES the default for that header; an empty value removes
        // it, which is how a row asks for a request that omits a member.
        headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        if !value.is_empty() {
            headers.push((*name, *value));
        }
    }
    headers
}

/// One `StartSpeechSynthesisStream` call: the headers a row chose, and the action
/// events it framed, concatenated as one event-stream body.
async fn stream(
    polly: &Polly,
    headers: &[(&str, &str)],
    events: &[Vec<u8>],
) -> http::Response<Bytes> {
    stream_in(polly, REGION, headers, events).await
}

/// The same call signed for `region`, for the rule about the Region a lexicon lives
/// in.
async fn stream_in(
    polly: &Polly,
    region: &str,
    headers: &[(&str, &str)],
    events: &[Vec<u8>],
) -> http::Response<Bytes> {
    let body: Vec<u8> = events.iter().flatten().copied().collect();
    polly
        .call_bytes_in_region(region, "POST", STREAM_PATH, headers, body)
        .await
}

/// Decode a response body into its frames, asserting the frame headers an SDK
/// decoder requires of every one.
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
        };
        assert_eq!(
            read(MESSAGE_TYPE).as_deref(),
            Some("event"),
            "case {case:?}: a response frame must be an event, got {:?}",
            read(MESSAGE_TYPE)
        );
        let event_type = read(EVENT_TYPE)
            .unwrap_or_else(|| panic!("case {case:?}: a response frame named no {EVENT_TYPE}"));
        frames.push(Frame {
            event_type,
            content_type: read(CONTENT_TYPE).unwrap_or_default(),
            payload: message.payload().to_vec(),
        });
    }
    frames
}

/// The audio `SynthesizeSpeech` answers for `text` in `output_format`, with the same
/// voice and engine a stream uses — the bytes a stream of that text must carry.
async fn synthesized(
    polly: &Polly,
    case: &str,
    output_format: &str,
    sample_rate: Option<&str>,
    text: &str,
    text_type: Option<&str>,
) -> Vec<u8> {
    let mut request = json!({
        "Engine": STREAMING_ENGINE,
        "OutputFormat": output_format,
        "VoiceId": STREAMING_VOICE,
        "Text": text,
    });
    if let Some(sample_rate) = sample_rate {
        request["SampleRate"] = json!(sample_rate);
    }
    if let Some(text_type) = text_type {
        request["TextType"] = json!(text_type);
    }
    let response = polly.synthesize(&request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: SynthesizeSpeech must serve the reference audio, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    response.body().to_vec()
}

/// The `AudioEvent` frames of a response, in order.
fn audio_chunks(frames: &[Frame]) -> Vec<&Frame> {
    frames
        .iter()
        .filter(|frame| frame.event_type == "AudioEvent")
        .collect()
}

/// The `StreamClosedEvent` a served stream ends with.
fn stream_closed(case: &str, frames: &[Frame]) -> Value {
    let last = frames
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: a served stream carries at least one frame"));
    assert_eq!(
        last.event_type, "StreamClosedEvent",
        "case {case:?}: a served stream must end with StreamClosedEvent, got {:?}",
        last.event_type
    );
    last.json(case)
}

/// The `reason`, `fields` and `message` a refusal's error document carries.
fn refusal(case: &str, response: &http::Response<Bytes>) -> Value {
    assert_eq!(
        response.status().as_u16(),
        400,
        "case {case:?}: a refused stream is a 400, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(
        error_code(response),
        "ValidationException",
        "case {case:?}: the modelled refusal is ValidationException, got {:?}",
        error_code(response)
    );
    json_body(response)
}

/// One row of the format table: the format, the rate it is asked for at, and the
/// text events of the stream.
struct FormatRow {
    case: &'static str,
    output_format: &'static str,
    sample_rate: Option<&'static str>,
    texts: &'static [&'static str],
}

/// RULE: a stream answers the audio of the text it was sent, in the encoding the
/// request settled, and closes with the characters it synthesized — the same audio
/// `SynthesizeSpeech` answers for that text.
///
/// The table spans every `OutputFormat` that carries audio (the one that does not is
/// its own test below), both the default sample rate and an explicitly named one,
/// and one, two and three text events.
#[tokio::test]
async fn each_audio_format_streams_the_audio_of_its_text_and_closes_with_its_characters() {
    // Every audio value of the model's `OutputFormat`
    // (smithy_query(query="shape", shape="OutputFormat")), at the rates
    // `SynthesizeSpeechInput$SampleRate` documents for each.
    let rows = [
        FormatRow {
            case: "pcm at its default rate, one text event",
            output_format: "pcm",
            sample_rate: None,
            texts: &["Hello from the stream."],
        },
        FormatRow {
            case: "pcm at the other rate its format documents",
            output_format: "pcm",
            sample_rate: Some("8000"),
            texts: &["Hello from the stream."],
        },
        FormatRow {
            case: "mp3 at its default rate, two text events",
            output_format: "mp3",
            sample_rate: None,
            texts: &["One sentence. ", "And a second one."],
        },
        FormatRow {
            case: "mp3 at a named rate",
            output_format: "mp3",
            sample_rate: Some("44100"),
            texts: &["A longer passage, spoken at the highest rate mp3 documents."],
        },
        FormatRow {
            case: "ogg_opus at its one documented rate",
            output_format: "ogg_opus",
            sample_rate: Some("48000"),
            texts: &["Opus carries this one."],
        },
        FormatRow {
            case: "mulaw, three text events including non-ascii",
            output_format: "mulaw",
            sample_rate: None,
            texts: &["Übersetze: ", "木星 is the largest planet", " — richtig?"],
        },
        FormatRow {
            case: "alaw at its one documented rate",
            output_format: "alaw",
            sample_rate: Some("8000"),
            texts: &["A-law carries this one."],
        },
    ];

    for row in rows {
        let polly = Polly::new();
        let case = row.case;
        let mut overrides: Vec<(&str, &str)> = vec![("x-amzn-OutputFormat", row.output_format)];
        if let Some(rate) = row.sample_rate {
            overrides.push(("x-amzn-SampleRate", rate));
        }
        let headers = stream_headers(&overrides);
        let mut events: Vec<Vec<u8>> = row
            .texts
            .iter()
            .map(|text| text_event(text, None, None))
            .collect();
        events.push(close_stream_event());

        let response = stream(&polly, &headers, &events).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a well-formed stream is served, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "content-type"),
            EVENT_STREAM,
            "case {case:?}: the response body is an event stream, got {:?}",
            header(&response, "content-type")
        );

        let frames = decode_frames(case, response.body());
        let chunks = audio_chunks(&frames);
        assert_eq!(
            chunks.len(),
            1,
            "case {case:?}: a stream that forces no flush writes its audio as one \
             AudioEvent, got {} ({:?})",
            chunks.len(),
            frames.iter().map(|f| &f.event_type).collect::<Vec<_>>()
        );
        assert_eq!(
            chunks[0].content_type, "application/octet-stream",
            "case {case:?}: AudioChunk is the event's @eventPayload, so the frame declares \
             binary content, got {:?}",
            chunks[0].content_type
        );

        let whole_text: String = row.texts.concat();
        let reference = synthesized(
            &polly,
            case,
            row.output_format,
            row.sample_rate,
            &whole_text,
            None,
        )
        .await;
        assert_eq!(
            chunks[0].payload,
            reference,
            "case {case:?}: the streamed audio must be the audio SynthesizeSpeech answers \
             for the same text ({} bytes streamed, {} bytes for one request)",
            chunks[0].payload.len(),
            reference.len()
        );

        let closed = stream_closed(case, &frames);
        let expected_characters = whole_text.chars().count() as i64;
        assert_eq!(
            closed.get("RequestCharacters").and_then(Value::as_i64),
            Some(expected_characters),
            "case {case:?}: StreamClosedEvent reports the characters synthesized, expected \
             {expected_characters}, got {closed}"
        );
    }
}

/// One row of the flush table: the text events, each with the flush it forces, and
/// the text groups the audio events must cover.
struct FlushRow {
    case: &'static str,
    /// `(text, force)` per `TextEvent`.
    events: &'static [(&'static str, Option<bool>)],
    /// The text each `AudioEvent` must carry the audio of, in order.
    groups: &'static [&'static str],
}

/// RULE: the audio of the text buffered so far is written as its own `AudioEvent` at
/// each flush point — a `TextEvent` that forces a flush, and the end of the input
/// stream — so the frame boundaries follow the flushes the caller asked for and
/// nothing else.
#[tokio::test]
async fn forcing_a_flush_writes_the_buffered_audio_as_its_own_event() {
    let rows = [
        FlushRow {
            case: "no flush forced: one event for the whole text",
            events: &[("Alpha ", None), ("beta ", None), ("gamma.", None)],
            groups: &["Alpha beta gamma."],
        },
        FlushRow {
            case: "flush forced on the first of two",
            events: &[("Alpha ", Some(true)), ("beta.", None)],
            groups: &["Alpha ", "beta."],
        },
        FlushRow {
            case: "flush forced on every event",
            events: &[
                ("Alpha ", Some(true)),
                ("beta ", Some(true)),
                ("gamma.", Some(true)),
            ],
            groups: &["Alpha ", "beta ", "gamma."],
        },
        FlushRow {
            case: "a flush configuration that does not force one is not a flush point",
            events: &[("Alpha ", Some(false)), ("beta.", Some(false))],
            groups: &["Alpha beta."],
        },
        FlushRow {
            case: "a forced flush on the last event is not written twice",
            events: &[("Alpha ", None), ("beta.", Some(true))],
            groups: &["Alpha beta."],
        },
    ];

    for row in rows {
        let polly = Polly::new();
        let case = row.case;
        let headers = stream_headers(&[]);
        let mut events: Vec<Vec<u8>> = row
            .events
            .iter()
            .map(|(text, force)| text_event(text, None, *force))
            .collect();
        events.push(close_stream_event());

        let response = stream(&polly, &headers, &events).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a well-formed stream is served, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let frames = decode_frames(case, response.body());
        let chunks = audio_chunks(&frames);
        assert_eq!(
            chunks.len(),
            row.groups.len(),
            "case {case:?}: expected one AudioEvent per flush point ({}), got {}",
            row.groups.len(),
            chunks.len()
        );
        for (chunk, group) in chunks.iter().zip(row.groups) {
            let reference = synthesized(&polly, case, "pcm", None, group, None).await;
            assert_eq!(
                chunk.payload,
                reference,
                "case {case:?}: the audio event for {group:?} must be the audio of that text \
                 ({} bytes streamed, {} bytes for one request)",
                chunk.payload.len(),
                reference.len()
            );
        }
        let closed = stream_closed(case, &frames);
        let total: usize = row
            .events
            .iter()
            .map(|(text, _)| text.chars().count())
            .sum();
        assert_eq!(
            closed.get("RequestCharacters").and_then(Value::as_i64),
            Some(total as i64),
            "case {case:?}: the closing event reports every character synthesized, expected \
             {total}, got {closed}"
        );
    }
}

/// One row of the header table: the headers it overrides, and the member it must be
/// refused on (`None` for a row that must be served).
struct HeaderRow {
    case: &'static str,
    overrides: &'static [(&'static str, &'static str)],
    refused_field: Option<&'static str>,
}

/// RULE: the synthesis parameters a stream is opened with are accepted only where
/// the operation's own rules admit them — only the one engine it documents, only the
/// formats that carry audio, only a sample rate the chosen format enumerates, only a
/// voice that supports that engine and only a language that voice speaks — and a
/// value outside them is a `ValidationException` naming the member that failed.
#[tokio::test]
async fn a_synthesis_parameter_the_operation_does_not_admit_is_refused_on_its_member() {
    let rows = [
        HeaderRow {
            case: "the engine the operation documents",
            overrides: &[],
            refused_field: None,
        },
        // Every other value of the model's `Engine` enum
        // (smithy_query(query="shape", shape="Engine")).
        HeaderRow {
            case: "the standard engine",
            overrides: &[("x-amzn-Engine", "standard")],
            refused_field: Some("engine"),
        },
        HeaderRow {
            case: "the neural engine",
            overrides: &[("x-amzn-Engine", "neural")],
            refused_field: Some("engine"),
        },
        HeaderRow {
            case: "the long-form engine",
            overrides: &[("x-amzn-Engine", "long-form")],
            refused_field: Some("engine"),
        },
        HeaderRow {
            case: "the speech-mark output format, which this operation does not support",
            overrides: &[("x-amzn-OutputFormat", "json")],
            refused_field: Some("outputFormat"),
        },
        HeaderRow {
            case: "a sample rate pcm documents",
            overrides: &[("x-amzn-SampleRate", "16000")],
            refused_field: None,
        },
        HeaderRow {
            case: "a sample rate mp3 documents but pcm does not",
            overrides: &[("x-amzn-SampleRate", "22050")],
            refused_field: Some("sampleRate"),
        },
        HeaderRow {
            case: "a sample rate no format documents",
            overrides: &[("x-amzn-SampleRate", "11025")],
            refused_field: Some("sampleRate"),
        },
        HeaderRow {
            case: "a rate spelled with leading zeros is not the documented spelling",
            overrides: &[("x-amzn-SampleRate", "016000")],
            refused_field: Some("sampleRate"),
        },
        HeaderRow {
            case: "another voice the catalog records as supporting the engine",
            overrides: &[("x-amzn-VoiceId", "Matthew")],
            refused_field: None,
        },
        HeaderRow {
            case: "a voice that supports only the standard and neural engines",
            overrides: &[("x-amzn-VoiceId", "Ivy")],
            refused_field: Some("voiceId"),
        },
        HeaderRow {
            case: "a voice that supports only the long-form and neural engines",
            overrides: &[("x-amzn-VoiceId", "Gregory")],
            refused_field: Some("voiceId"),
        },
        HeaderRow {
            case: "the voice's own language",
            overrides: &[("x-amzn-LanguageCode", "en-US")],
            refused_field: None,
        },
        HeaderRow {
            case: "a bilingual voice asked for its additional language",
            overrides: &[("x-amzn-VoiceId", "Kajal"), ("x-amzn-LanguageCode", "en-IN")],
            refused_field: None,
        },
        HeaderRow {
            case: "a bilingual voice asked for its own language",
            overrides: &[("x-amzn-VoiceId", "Kajal"), ("x-amzn-LanguageCode", "hi-IN")],
            refused_field: None,
        },
        HeaderRow {
            // `LanguageCode` only selects a bilingual voice's language, so a language
            // the voice does not speak is streamed with that voice rather than refused
            // (see `crate::language_code_and_voice`).
            case: "a language the voice does not speak",
            overrides: &[("x-amzn-LanguageCode", "de-DE")],
            refused_field: None,
        },
        HeaderRow {
            case: "a lexicon no Region of this emulator stores",
            overrides: &[("x-amzn-LexiconNames", "Absent")],
            refused_field: Some("lexiconNames"),
        },
    ];

    for row in rows {
        let polly = Polly::new();
        let case = row.case;
        let headers = stream_headers(row.overrides);
        let events = [text_event("Parameter rules.", None, None), close_stream_event()];
        let response = stream(&polly, &headers, &events).await;

        let Some(field) = row.refused_field else {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: these parameters are admitted, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            continue;
        };
        let document = refusal(case, &response);
        assert_eq!(
            document.get("reason").and_then(Value::as_str),
            Some("fieldValidationFailed"),
            "case {case:?}: a parameter outside the rules is a field validation failure, got \
             {document}"
        );
        let named = document
            .get("fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(
            named,
            vec![field.to_string()],
            "case {case:?}: the refusal must name the member that failed, got {document}"
        );
        let message = document
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(field),
            "case {case:?}: the refusal's message must name the member that failed, got \
             {message:?}"
        );
    }
}

/// RULE: a required synthesis parameter the request omits is refused the way the
/// service refuses every absent required member, as a `ValidationException`.
#[tokio::test]
async fn a_required_synthesis_parameter_the_request_omits_is_refused() {
    // The three `@required` members of the input
    // (smithy_query(query="operation", operation="StartSpeechSynthesisStream")),
    // each with the envelope path the refusal names it by.
    let rows = [
        ("x-amzn-Engine", "engine"),
        ("x-amzn-OutputFormat", "outputFormat"),
        ("x-amzn-VoiceId", "voiceId"),
    ];
    for (omitted_header, member) in rows {
        let polly = Polly::new();
        let case = omitted_header;
        // An empty override value removes the header the row is about.
        let headers = stream_headers(&[(omitted_header, "")]);
        let events = [text_event("A required member.", None, None), close_stream_event()];
        let response = stream(&polly, &headers, &events).await;
        let document = refusal(case, &response);
        let message = document
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(member) && message.contains("must not be null"),
            "case {case:?}: an absent required member is refused naming it, got {message:?}"
        );
    }
}

/// One row of the action-event table: the frames of the input stream, and the reason
/// class the refusal must carry (`None` for a stream that must be served).
struct ActionRow {
    case: &'static str,
    frames: Vec<Vec<u8>>,
    refused_reason: Option<&'static str>,
}

/// RULE: the input stream carries only the members of its own action union, each a
/// readable event, and nothing after the event that closes it — anything else is a
/// `ValidationException` whose reason is the invalid-inbound-event class.
#[tokio::test]
async fn the_input_stream_admits_only_its_own_action_events() {
    /// A frame whose `:message-type` is not the one a client may send.
    fn exception_frame() -> Vec<u8> {
        let message = Message::new(b"{}".to_vec())
            .add_header(Header::new(
                MESSAGE_TYPE,
                HeaderValue::String("exception".into()),
            ))
            .add_header(Header::new(
                ":exception-type",
                HeaderValue::String("ValidationException".into()),
            ));
        let mut framed = Vec::new();
        write_message_to(&message, &mut framed).expect("an in-memory frame writer cannot fail");
        framed
    }
    /// A frame that names no event at all.
    fn unnamed_frame() -> Vec<u8> {
        let message = Message::new(b"{}".to_vec()).add_header(Header::new(
            MESSAGE_TYPE,
            HeaderValue::String("event".into()),
        ));
        let mut framed = Vec::new();
        write_message_to(&message, &mut framed).expect("an in-memory frame writer cannot fail");
        framed
    }

    let rows = vec![
        ActionRow {
            case: "both members of the action union, in order",
            frames: vec![text_event("Served.", None, None), close_stream_event()],
            refused_reason: None,
        },
        ActionRow {
            case: "a text event with no close event: the body ends the stream",
            frames: vec![text_event("Served.", None, None)],
            refused_reason: None,
        },
        ActionRow {
            case: "an event the action union does not declare",
            frames: vec![action_frame("AudioEvent", json!({})), close_stream_event()],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "an event whose name the union does not carry at all",
            frames: vec![action_frame("TextEvents", json!({ "Text": "Plural." }))],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "an exception frame, which only a server may send",
            frames: vec![exception_frame()],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a frame naming no event",
            frames: vec![unnamed_frame()],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a text event carrying no Text",
            frames: vec![action_frame("TextEvent", json!({ "TextType": "text" }))],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a text event whose TextType is not one the model declares",
            frames: vec![action_frame(
                "TextEvent",
                json!({ "Text": "Plain.", "TextType": "markdown" }),
            )],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a text event after the close event",
            frames: vec![
                text_event("First.", None, None),
                close_stream_event(),
                text_event("After the close.", None, None),
            ],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a body that is not an event stream at all",
            frames: vec![b"not a frame".to_vec()],
            refused_reason: Some("invalidInboundEvent"),
        },
        ActionRow {
            case: "a truncated frame",
            frames: vec![text_event("Truncated.", None, None)
                .into_iter()
                .take(12)
                .collect()],
            refused_reason: Some("invalidInboundEvent"),
        },
    ];

    for row in rows {
        let polly = Polly::new();
        let case = row.case;
        let headers = stream_headers(&[]);
        let response = stream(&polly, &headers, &row.frames).await;
        match row.refused_reason {
            None => assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: this input stream is served, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            ),
            Some(reason) => {
                let document = refusal(case, &response);
                assert_eq!(
                    document.get("reason").and_then(Value::as_str),
                    Some(reason),
                    "case {case:?}: expected the {reason} class, got {document}"
                );
            }
        }
    }
}

/// RULE: a `TextEvent` whose `TextType` is `ssml` is read as an XML document against
/// the SSML vocabulary Amazon Polly publishes, and bills only its text content — so
/// a document outside that vocabulary is refused as an invalid inbound event, and
/// one inside it synthesizes the audio of its text content alone.
#[tokio::test]
async fn ssml_text_is_read_as_a_document_and_bills_only_its_text_content() {
    // `(case, text, the characters it bills, or None where the document is refused)`
    let rows: [(&str, &str, Option<usize>); 6] = [
        ("plain content", "<speak>Hello there</speak>", Some(11)),
        (
            "markup around the content bills none of itself",
            "<speak><prosody rate=\"slow\">Hello</prosody> there</speak>",
            Some(11),
        ),
        (
            "an entity reference bills as the one character it stands for",
            "<speak>A &amp; B</speak>",
            Some(5),
        ),
        ("an element left unclosed", "<speak>Hello", None),
        (
            "a tag outside the published vocabulary",
            "<speak><marquee>Hello</marquee></speak>",
            None,
        ),
        (
            "a document element that is not speak",
            "<say>Hello</say>",
            None,
        ),
    ];

    for (case, text, billed) in rows {
        let polly = Polly::new();
        let headers = stream_headers(&[]);
        let events = [text_event(text, Some("ssml"), None), close_stream_event()];
        let response = stream(&polly, &headers, &events).await;

        let Some(billed) = billed else {
            let document = refusal(case, &response);
            assert_eq!(
                document.get("reason").and_then(Value::as_str),
                Some("invalidInboundEvent"),
                "case {case:?}: SSML the service does not accept is an invalid inbound event, \
                 got {document}"
            );
            continue;
        };
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: SSML inside the published vocabulary is served, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let frames = decode_frames(case, response.body());
        let closed = stream_closed(case, &frames);
        assert_eq!(
            closed.get("RequestCharacters").and_then(Value::as_i64),
            Some(billed as i64),
            "case {case:?}: SSML tags are not billed characters, expected {billed}, got {closed}"
        );
        let chunks = audio_chunks(&frames);
        assert_eq!(
            chunks.len(),
            1,
            "case {case:?}: one flush point, so one AudioEvent, got {}",
            chunks.len()
        );
        let reference = synthesized(&polly, case, "pcm", None, text, Some("ssml")).await;
        assert_eq!(
            chunks[0].payload,
            reference,
            "case {case:?}: the streamed audio must be the audio SynthesizeSpeech answers for \
             the same document ({} bytes streamed, {} bytes for one request)",
            chunks[0].payload.len(),
            reference.len()
        );
    }
}

/// RULE: every name in `LexiconNames` is resolved in the lexicon store of the Region
/// the request was signed for, so a lexicon stored in that Region is accepted and
/// the same name in another Region is not.
#[tokio::test]
async fn a_lexicon_is_resolved_in_the_region_the_stream_was_signed_for() {
    let polly = Polly::new();
    let stored = polly.put_lexicon("StreamOne", &pls("ipa", "en-US", 1)).await;
    assert_eq!(
        stored.status().as_u16(),
        200,
        "the fixture lexicon must be stored, got {} {:?}",
        stored.status(),
        String::from_utf8_lossy(stored.body())
    );
    let also_stored = polly.put_lexicon("StreamTwo", &pls("ipa", "en-US", 2)).await;
    assert_eq!(
        also_stored.status().as_u16(),
        200,
        "the second fixture lexicon must be stored, got {}",
        also_stored.status()
    );

    // `(case, the LexiconNames header, the region it is signed for, served?)`
    let rows = [
        ("one stored name", "StreamOne", REGION, true),
        ("two stored names", "StreamOne,StreamTwo", REGION, true),
        ("one stored and one absent", "StreamOne,Absent", REGION, false),
        ("a stored name in another Region", "StreamOne", "eu-west-1", false),
    ];
    for (case, names, region, served) in rows {
        let headers = stream_headers(&[("x-amzn-LexiconNames", names)]);
        let events = [text_event("Lexicons.", None, None), close_stream_event()];
        let response = stream_in(&polly, region, &headers, &events).await;
        if served {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: a lexicon the Region stores is accepted, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
        } else {
            let document = refusal(case, &response);
            assert_eq!(
                document.get("reason").and_then(Value::as_str),
                Some("fieldValidationFailed"),
                "case {case:?}: a lexicon the Region does not store is a field failure, got \
                 {document}"
            );
        }
    }
}

/// RULE: a stream that carries no action event is not a failure — `ActionStream` is
/// not a required member — so it closes with no audio and no characters
/// synthesized.
#[tokio::test]
async fn a_stream_with_no_text_closes_with_no_audio_and_no_characters() {
    // `(case, the frames of the input stream)`
    let rows: [(&str, Vec<Vec<u8>>); 3] = [
        ("an empty body", vec![]),
        ("the close event alone", vec![close_stream_event()]),
        (
            "a text event carrying the empty string",
            vec![text_event("", None, None), close_stream_event()],
        ),
    ];
    for (case, frames) in rows {
        let polly = Polly::new();
        let headers = stream_headers(&[]);
        let response = stream(&polly, &headers, &frames).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: a stream with no text is served, got {} {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let decoded = decode_frames(case, response.body());
        assert!(
            audio_chunks(&decoded).is_empty(),
            "case {case:?}: no text was sent, so no audio is returned, got {:?}",
            decoded.iter().map(|f| &f.event_type).collect::<Vec<_>>()
        );
        let closed = stream_closed(case, &decoded);
        assert_eq!(
            closed.get("RequestCharacters").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: nothing was synthesized, got {closed}"
        );
    }
}

/// RULE: a stream asking for more work than this emulator holds — more action events
/// or more characters than it synthesizes in one stream — is refused rather than
/// served, with the reason class the model keeps for a refusal that is neither a
/// field nor an event.
#[tokio::test]
async fn a_stream_past_this_emulators_capacity_is_refused() {
    let polly = Polly::new();
    let headers = stream_headers(&[]);

    // One character per event, so the count is what trips rather than the text.
    let mut too_many: Vec<Vec<u8>> = (0..=MAX_ACTION_EVENTS)
        .map(|_| text_event("x", None, None))
        .collect();
    let within = too_many[..MAX_ACTION_EVENTS].to_vec();
    too_many.push(close_stream_event());

    let response = stream(&polly, &headers, &within).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case \"the most events accepted\": a stream at the bound is served, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );

    let response = stream(&polly, &headers, &too_many).await;
    let document = refusal("one event past the bound", &response);
    assert_eq!(
        document.get("reason").and_then(Value::as_str),
        Some("other"),
        "case \"one event past the bound\": expected the catch-all reason, got {document}"
    );

    // The same bound on characters, in one event and then spread over several.
    let one_too_long = "x".repeat(MAX_STREAM_CHARACTERS + 1);
    let at_the_bound = "x".repeat(MAX_STREAM_CHARACTERS);
    let rows: [(&str, Vec<Vec<u8>>, bool); 3] = [
        (
            "the most characters synthesized in one stream",
            vec![text_event(&at_the_bound, None, None), close_stream_event()],
            true,
        ),
        (
            "one character past the bound, in one event",
            vec![text_event(&one_too_long, None, None), close_stream_event()],
            false,
        ),
        (
            "one character past the bound, spread over two events",
            vec![
                text_event(&at_the_bound, None, None),
                text_event("x", None, None),
                close_stream_event(),
            ],
            false,
        ),
    ];
    for (case, frames, served) in rows {
        let response = stream(&polly, &headers, &frames).await;
        if served {
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?}: a stream at the bound is served, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
        } else {
            let document = refusal(case, &response);
            assert_eq!(
                document.get("reason").and_then(Value::as_str),
                Some("other"),
                "case {case:?}: expected the catch-all reason, got {document}"
            );
        }
    }

    // The session's AUDIO bound is the other half, and the two bounds have to leave room
    // for each other: a flush refuses a run whose worst case (one whole utterance) would
    // pass what the session may still produce, so a session must be able to carry several
    // whole utterances or an ordinary multi-event stream would be refused for its last
    // word. This pins that relationship, which is what a change to either constant would
    // break.
    assert!(
        (MAX_STREAM_SECONDS as f32) * 1000.0 >= 3.0 * MAX_UTTERANCE_MS,
        "the session bound ({MAX_STREAM_SECONDS} s) must carry several whole utterances \
         of {MAX_UTTERANCE_MS} ms each"
    );
    // And an ordinary stream — several events, well inside both bounds — is served.
    let ordinary = stream(
        &polly,
        &headers,
        &[
            text_event("One sentence of a few words. ", None, Some(true)),
            text_event("A second one, flushed on its own. ", None, Some(true)),
            text_event("And a third.", None, None),
            close_stream_event(),
        ],
    )
    .await;
    assert_eq!(
        ordinary.status().as_u16(),
        200,
        "an ordinary multi-event stream is well inside both bounds, got {} {:?}",
        ordinary.status(),
        String::from_utf8_lossy(ordinary.body())
    );
    assert_eq!(
        audio_chunks(&decode_frames("ordinary", ordinary.body())).len(),
        3,
        "each flush point still writes its own AudioEvent"
    );
}

/// RULE: every `OutputFormat` that carries audio is streamed, and the audio a stream
/// answers for a text is the audio `SynthesizeSpeech` answers for it — so the two
/// operations agree about every format, `ogg_vorbis` included.
#[tokio::test]
async fn the_ogg_vorbis_format_is_streamed_like_the_single_request() {
    let polly = Polly::new();
    let headers = stream_headers(&[("x-amzn-OutputFormat", "ogg_vorbis")]);
    const TEXT: &str = "Vorbis carries this one.";
    let events = [text_event(TEXT, None, None), close_stream_event()];
    let response = stream(&polly, &headers, &events).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "a vorbis stream is served, got {} {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let frames = decode_frames("ogg_vorbis", response.body());
    let chunks = audio_chunks(&frames);
    assert_eq!(
        chunks.len(),
        1,
        "one flush point, so one AudioEvent, got {}",
        chunks.len()
    );
    assert_eq!(
        &chunks[0].payload[..4],
        b"OggS",
        "the chunk must be an Ogg stream"
    );
    let reference = synthesized(&polly, "ogg_vorbis", "ogg_vorbis", None, TEXT, None).await;
    assert_eq!(
        chunks[0].payload,
        reference,
        "the streamed audio must be the audio SynthesizeSpeech answers for the same text \
         ({} bytes streamed, {} bytes for one request)",
        chunks[0].payload.len(),
        reference.len()
    );

    // A stream that carries no action event has nothing to say, so it closes with no
    // audio rather than with an empty one.
    let empty = stream(&polly, &headers, &[]).await;
    assert_eq!(
        empty.status().as_u16(),
        200,
        "a stream with no action event is still served, got {} {:?}",
        empty.status(),
        String::from_utf8_lossy(empty.body())
    );
    assert!(
        audio_chunks(&decode_frames("no events", empty.body())).is_empty(),
        "a stream with no text writes no AudioEvent"
    );
}

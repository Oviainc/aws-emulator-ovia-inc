// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN a `json` synthesis request answers with a speech-mark
//! stream and when it is refused.
//!
//! RULE: `OutputFormat: json` IS the speech-mark stream, so the mark types the request
//! NAMES decide which marks it carries: naming none asks for no marks, so the answer is a
//! 200 carrying an empty stream for every `TextType`, and naming a type is answered with
//! exactly that type's marks — for plain text and for `ssml` alike.
//!
//! The tests check that over a TABLE whose rows differ along `TextType` (absent, `text`,
//! `ssml`), how the mark list is spelled (member absent, present but empty), the shape of
//! the input (plain text, an SSML document of text only, one with an empty element, one
//! with nested elements, one whose markup carries attribute text) and which mark types
//! are named (none, `sentence`, `word`, `viseme`, `ssml`, several at once) — so the answer
//! is pinned to follow the request rather than the `TextType` alone.

use crate::lexicons_and_speech::{header, Polly};

/// The media type `json` output carries, which is also what a refusal must NOT answer
/// with.
const MARK_STREAM_MEDIA_TYPE: &str = "application/x-json-stream";

/// One row of the empty-stream table: the case name, the `Text`, its `TextType` (absent
/// when `None`), its `SpeechMarkTypes` (absent when `None`) and the billed character
/// count the row expects.
type EmptyStreamRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static [&'static str]>,
    u64,
);

/// A `json` request's body, with `TextType` and `SpeechMarkTypes` present only when the
/// row names them — an absent member is a different input from an empty one.
fn request(text: &str, text_type: Option<&str>, mark_types: Option<&[&str]>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "Text": text,
        "VoiceId": "Joanna",
        "OutputFormat": "json",
    });
    if let Some(text_type) = text_type {
        body["TextType"] = serde_json::json!(text_type);
    }
    if let Some(mark_types) = mark_types {
        body["SpeechMarkTypes"] = serde_json::json!(mark_types);
    }
    body
}

/// RULE: a `json` request that names no mark type asks for no marks, so it is answered
/// with the speech-mark media type and an EMPTY stream — whatever `TextType` the input
/// is, because no offsets into the text are computed for a stream that carries nothing.
#[tokio::test]
async fn naming_no_mark_type_is_an_empty_stream_for_every_text_type() {
    let polly = Polly::new();
    // Every SSML row's markup is from Amazon Polly's supported-SSML-tags vocabulary, as
    // the crate records it in `ops::synthesize_speech::ssml::SUPPORTED_ELEMENTS`:
    // `speak`, `break`, `prosody`, `s`, `emphasis`, `sub`, `mark`.
    let table: [EmptyStreamRow; 10] = [
        // (case, Text, TextType, SpeechMarkTypes, billed characters)
        ("text default, marks absent", "Hi there. Go!", None, None, 13),
        (
            "text default, marks empty",
            "Hi there. Go!",
            None,
            Some(&[]),
            13,
        ),
        (
            "text type text, marks absent",
            "Hi there. Go!",
            Some("text"),
            None,
            13,
        ),
        (
            "text type text, marks empty",
            "Hi there. Go!",
            Some("text"),
            Some(&[]),
            13,
        ),
        (
            "ssml text only, marks absent",
            "<speak>Hi there. Go!</speak>",
            Some("ssml"),
            None,
            13,
        ),
        (
            "ssml text only, marks empty",
            "<speak>Hi there. Go!</speak>",
            Some("ssml"),
            Some(&[]),
            13,
        ),
        (
            "ssml with an empty element, marks absent",
            "<speak>one <break time=\"1s\"/>two</speak>",
            Some("ssml"),
            None,
            7,
        ),
        (
            "ssml with nested elements, marks empty",
            "<speak><prosody rate=\"slow\"><s>Hi there.</s></prosody> \
             <emphasis level=\"strong\">Go!</emphasis></speak>",
            Some("ssml"),
            Some(&[]),
            13,
        ),
        (
            "ssml with a mark element, marks absent",
            "<speak>one <mark name=\"here\"/> two</speak>",
            Some("ssml"),
            None,
            8,
        ),
        (
            "ssml with attribute text, marks empty",
            "<speak><sub alias=\"aluminium\">Al</sub></speak>",
            Some("ssml"),
            Some(&[]),
            2,
        ),
    ];
    for (case, text, text_type, mark_types, billed) in table {
        let response = polly
            .synthesize(&request(text, text_type, mark_types))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {} with body {:?}",
            response.status().as_u16(),
            String::from_utf8_lossy(response.body())
        );
        let media_type = header(&response, "content-type");
        assert_eq!(
            media_type, MARK_STREAM_MEDIA_TYPE,
            "case {case:?}: expected the speech-mark media type, got {media_type:?}"
        );
        assert!(
            response.body().is_empty(),
            "case {case:?}: expected an empty mark stream, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        // The rest of the response is still computed from the request: the billed
        // character count is the text's own, tags excluded.
        let reported = header(&response, "x-amzn-requestcharacters");
        assert_eq!(
            reported,
            billed.to_string(),
            "case {case:?}: expected {billed} billed characters, got {reported:?}"
        );
    }
}

/// RULE: naming a mark type is answered with exactly that type's marks, for plain text
/// and for `ssml` input alike — the stream carries every type the request named and no
/// other.
#[tokio::test]
async fn naming_a_mark_type_is_answered_for_every_text_type() {
    let polly = Polly::new();
    let table: [(&str, &str, &str, &[&str]); 10] = [
        // (case, Text, TextType, SpeechMarkTypes)
        ("word over text", "Hi there. Go!", "text", &["word"]),
        ("sentence over text", "Hi there. Go!", "text", &["sentence"]),
        (
            "both over text",
            "Hi there. Go!",
            "text",
            &["sentence", "word"],
        ),
        ("viseme over text", "Hi there. Go!", "text", &["viseme"]),
        (
            "word over ssml",
            "<speak>Hi there. Go!</speak>",
            "ssml",
            &["word"],
        ),
        (
            "sentence over ssml",
            "<speak>Hi there. Go!</speak>",
            "ssml",
            &["sentence"],
        ),
        (
            "both over ssml",
            "<speak>Hi there. Go!</speak>",
            "ssml",
            &["sentence", "word"],
        ),
        (
            "viseme over ssml",
            "<speak>Hi there. Go!</speak>",
            "ssml",
            &["viseme"],
        ),
        (
            "ssml marks over ssml",
            "<speak>one <mark name=\"here\"/> two</speak>",
            "ssml",
            &["ssml"],
        ),
        (
            "every type at once",
            "<speak>one <mark name=\"here\"/> two</speak>",
            "ssml",
            &["sentence", "word", "viseme", "ssml"],
        ),
    ];
    for (case, text, text_type, mark_types) in table {
        let response = polly
            .synthesize(&request(text, Some(text_type), Some(mark_types)))
            .await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: expected 200, got {} with body {:?}",
            response.status().as_u16(),
            String::from_utf8_lossy(response.body())
        );
        let body = String::from_utf8_lossy(response.body()).to_string();
        let types: Vec<String> = body
            .lines()
            .map(|line| {
                let mark: serde_json::Value =
                    serde_json::from_str(line).expect("each line is one JSON object");
                mark["type"].as_str().unwrap_or_default().to_string()
            })
            .collect();
        assert!(
            !types.is_empty(),
            "case {case:?}: expected marks, got an empty stream"
        );
        for mark_type in &types {
            assert!(
                mark_types.contains(&mark_type.as_str()),
                "case {case:?}: expected only {mark_types:?}, got {mark_type:?}"
            );
        }
        for requested in mark_types {
            assert!(
                types.iter().any(|mark_type| mark_type == requested),
                "case {case:?}: expected a {requested:?} mark, got {types:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the INVARIANCE behind the XML document rule: what XML admits
//! outside a document element is not part of the document, so it cannot change what
//! the document says — and what XML does not admit there is refused whatever else the
//! request asks for.
//!
//! RULE: padding an `ssml` `Text` with XML's own whitespace (space, tab, carriage
//! return, line feed — XML 1.0's `S` production) leaves the response identical to the
//! unpadded document's, member for member and byte for byte, in every `OutputFormat`;
//! padding it with character data XML does not admit there is HTTP 400
//! `InvalidSsmlException`, in every `OutputFormat` and with every other member of the
//! request unchanged.
//!
//! The tables span the formats the operation serves and both kinds of padding, so the
//! rule is checked where it holds rather than on the one request an issue quotes.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// The document each row pads. Its text content is what every row must still bill.
const DOCUMENT: &str = "<speak>hello there</speak>";

/// The audio formats `SynthesizeSpeech` serves, so the invariance is checked on streams
/// whose framing and byte length differ.
const FORMATS: [&str; 5] = ["mp3", "pcm", "ogg_opus", "mulaw", "alaw"];

/// One `SynthesizeSpeech` call for `text` in `format`.
async fn synthesize(polly: &Polly, text: &str, format: &str) -> http::Response<Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": text,
            "TextType": "ssml",
            "VoiceId": "Joanna",
            "OutputFormat": format,
        }))
        .await
}

/// RULE: whitespace around the document element is not the document, so the response is
/// the unpadded document's — same status, same `Content-Type`, same
/// `RequestCharacters`, and a byte-identical stream — in every format.
#[tokio::test]
async fn whitespace_around_the_document_element_changes_nothing_about_the_response() {
    let polly = Polly::new();
    // Each member of XML's whitespace set, and a combination of them on both sides.
    let paddings: [(&str, &str, &str); 6] = [
        ("space", " ", " "),
        ("tab", "\t", "\t"),
        ("carriage return", "\r", "\r"),
        ("line feed", "\n", "\n"),
        ("a run of every whitespace character", " \t\r\n", "\n\r\t "),
        ("whitespace on one side only", "\n  ", ""),
    ];
    for format in FORMATS {
        let plain = synthesize(&polly, DOCUMENT, format).await;
        assert_eq!(
            plain.status().as_u16(),
            200,
            "format {format:?}: the unpadded document must be synthesized, got {:?}",
            String::from_utf8_lossy(plain.body())
        );
        for (case, before, after) in paddings {
            let text = format!("{before}{DOCUMENT}{after}");
            let padded = synthesize(&polly, &text, format).await;
            assert_eq!(
                (
                    padded.status().as_u16(),
                    header(&padded, "content-type"),
                    header(&padded, "x-amzn-requestcharacters"),
                    padded.body().len(),
                ),
                (
                    plain.status().as_u16(),
                    header(&plain, "content-type"),
                    header(&plain, "x-amzn-requestcharacters"),
                    plain.body().len(),
                ),
                "case {case:?} in {format:?} ({text:?}): the padding must not change the \
                 response, got {:?}",
                String::from_utf8_lossy(padded.body())
            );
            assert!(
                padded.body() == plain.body(),
                "case {case:?} in {format:?}: the stream must be byte-identical to the \
                 unpadded document's ({} bytes against {})",
                padded.body().len(),
                plain.body().len()
            );
        }
    }
}

/// RULE (the other side of it): character data XML does not admit outside a document
/// element refuses the request whatever else it asks for — every format, and a request
/// that also names an engine, a sample rate and a lexicon.
#[tokio::test]
async fn character_data_outside_the_document_element_refuses_every_request() {
    let polly = Polly::new();
    // The ways a text can carry character data outside the document element, as rows
    // that differ in spelling and in side.
    let texts: [(&str, String); 5] = [
        ("text before it", format!("junk{DOCUMENT}")),
        ("text after it", format!("{DOCUMENT}junk")),
        ("a reference before it", format!("&amp;{DOCUMENT}")),
        ("a CDATA section after it", format!("{DOCUMENT}<![CDATA[x]]>")),
        (
            "a no-break space before it, which XML does not admit either",
            format!("\u{a0}{DOCUMENT}"),
        ),
    ];
    for (case, text) in &texts {
        for format in FORMATS {
            let response = synthesize(&polly, text, format).await;
            let body = json(&response);
            assert_eq!(
                (
                    response.status().as_u16(),
                    error_code(&response),
                    body.get("message").and_then(|m| m.as_str())
                ),
                (
                    400,
                    "InvalidSsmlException".to_string(),
                    Some(errors::INVALID_SSML)
                ),
                "case {case:?} in {format:?} ({text:?}): expected the SSML refusal, \
                 got {body:?}"
            );
        }
    }

    // The same refusal when the request asks for something else as well: the document
    // rule is about the text, not about the rest of the request.
    for (case, text) in &texts {
        let response = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "TextType": "ssml",
                "VoiceId": "Matthew",
                "Engine": "neural",
                "OutputFormat": "pcm",
                "SampleRate": "16000",
                "LexiconNames": ["NoSuchLexicon"],
            }))
            .await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidSsmlException".to_string()),
            "case {case:?} with a fuller request: expected the SSML refusal, got {:?}",
            json(&response)
        );
    }
}

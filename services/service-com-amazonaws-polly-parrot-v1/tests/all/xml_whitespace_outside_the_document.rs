// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH characters XML admits outside a document element, at
//! both of the service's XML readers: `SynthesizeSpeech`'s `ssml` `Text` and
//! `PutLexicon`'s PLS `Content`.
//!
//! RULE: the only character data a well-formed XML document may carry outside its
//! document element is XML's own whitespace — space, tab, carriage return and line
//! feed, XML 1.0's `S` production — so a text padded with any OTHER character is not
//! one XML document and is refused with the document error of the operation that read
//! it (`SynthesizeSpeech` -> `InvalidSsmlException`, `PutLexicon` ->
//! `InvalidLexiconException`), while a text padded with those four, in any combination,
//! is read and contributes nothing to what the document says.
//!
//! The tables span the class the rule generalises over: every member of XML's
//! whitespace set on the accepting side, and on the refusing side the characters
//! Unicode calls whitespace but XML does not (a no-break space, a vertical tab, a form
//! feed, a next line, a line and a paragraph separator, an ideographic space) plus an
//! ordinary character for contrast — each placed before and after the document element.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{PLS_NAMESPACE, PLS_VERSION};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// XML 1.0's `S` production, the whitespace a document may carry outside its document
/// element: `#x20 | #x9 | #xD | #xA`.
const XML_WHITESPACE: [(&str, &str); 4] = [
    ("space", " "),
    ("tab", "\t"),
    ("carriage return", "\r"),
    ("line feed", "\n"),
];

/// Characters OUTSIDE that production. Every one but the last is whitespace to Unicode
/// (`char::is_whitespace`) and character data to XML, which is the distinction the rule
/// turns on; the last is an ordinary character, the control case.
const NOT_XML_WHITESPACE: [(&str, &str); 8] = [
    ("no-break space", "\u{a0}"),
    ("vertical tab", "\u{b}"),
    ("form feed", "\u{c}"),
    ("next line", "\u{85}"),
    ("line separator", "\u{2028}"),
    ("paragraph separator", "\u{2029}"),
    ("ideographic space", "\u{3000}"),
    ("an ordinary character", "x"),
];

/// The SSML document every row pads: two characters of text content, so a row that
/// read the padding as content would be visible in the billed count.
const SSML: &str = "<speak>hi</speak>";

/// One `SynthesizeSpeech` call with `text` as SSML.
async fn synthesize_ssml(polly: &Polly, text: &str) -> http::Response<Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": text,
            "TextType": "ssml",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        }))
        .await
}

/// A valid PLS lexicon with one lexeme: the document every PLS row pads.
fn lexicon() -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
         <alias>World Wide Web Consortium</alias></lexeme></lexicon>"
    )
}

/// Each padding placed before the document element, after it, and on both sides.
fn placements(document: &str, padding: &str) -> Vec<(&'static str, String)> {
    vec![
        ("before the document element", format!("{padding}{document}")),
        ("after the document element", format!("{document}{padding}")),
        (
            "on both sides of the document element",
            format!("{padding}{document}{padding}"),
        ),
    ]
}

/// RULE (SSML, the refusal half): a character XML does not admit outside a document
/// element is character data there, so an `ssml` `Text` padded with one is HTTP 400
/// `InvalidSsmlException` — whichever side it sits on, and however much Unicode
/// considers it whitespace.
#[tokio::test]
async fn ssml_padded_with_a_character_xml_does_not_admit_is_refused() {
    let polly = Polly::new();
    for (what, padding) in NOT_XML_WHITESPACE {
        for (placement, text) in placements(SSML, padding) {
            let case = format!("{what} {placement}");
            let response = synthesize_ssml(&polly, &text).await;
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
                "case {case:?} ({text:?}): expected the SSML refusal, got {body:?}"
            );
        }
    }
}

/// RULE (SSML, the acceptance half): each of XML's four whitespace characters IS
/// admitted outside the document element, and none of them is the document's text
/// content, so every such document is synthesized and bills only what sits between its
/// tags.
#[tokio::test]
async fn ssml_padded_with_xml_whitespace_is_synthesized_and_bills_only_its_content() {
    let polly = Polly::new();
    // Each character on its own, and the whole set at once, so the rule is checked over
    // its members and over a combination of them.
    let paddings: Vec<(String, String)> = XML_WHITESPACE
        .iter()
        .map(|(what, padding)| ((*what).to_string(), (*padding).to_string()))
        .chain(std::iter::once((
            "every whitespace character at once".to_string(),
            XML_WHITESPACE.iter().map(|(_, p)| *p).collect::<String>(),
        )))
        .collect();
    for (what, padding) in paddings {
        for (placement, text) in placements(SSML, &padding) {
            let case = format!("{what} {placement}");
            let response = synthesize_ssml(&polly, &text).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case:?} ({text:?}): expected the document to be synthesized, got {:?}",
                String::from_utf8_lossy(response.body())
            );
            let billed = header(&response, "x-amzn-requestcharacters");
            assert_eq!(
                billed, "2",
                "case {case:?} ({text:?}): only the text content bills, got {billed:?}"
            );
        }
    }
}

/// RULE (PLS, the same rule at the other reader): a `Content` padded with a character
/// XML does not admit outside a document element is HTTP 400 `InvalidLexiconException`
/// and stores nothing, while one padded with XML's own whitespace is stored and reads
/// back verbatim.
#[tokio::test]
async fn pls_content_is_read_exactly_where_xml_admits_its_padding() {
    let polly = Polly::new();
    let document = lexicon();

    for (what, padding) in NOT_XML_WHITESPACE {
        for (placement, content) in placements(&document, padding) {
            let case = format!("{what} {placement}");
            let response = polly.put_lexicon("PaddedWithData", &content).await;
            let body = json(&response);
            assert_eq!(
                (
                    response.status().as_u16(),
                    error_code(&response),
                    body.get("message").and_then(|m| m.as_str())
                ),
                (
                    400,
                    "InvalidLexiconException".to_string(),
                    Some(errors::INVALID_LEXICON)
                ),
                "case {case:?} ({content:?}): expected the invalid-document refusal, \
                 got {body:?}"
            );
            let read_back = polly.get_lexicon("PaddedWithData").await;
            assert_eq!(
                (read_back.status().as_u16(), error_code(&read_back)),
                (404, "LexiconNotFoundException".to_string()),
                "case {case:?}: a refused put must store nothing, got {:?}",
                json(&read_back)
            );
        }
    }

    for (index, (what, padding)) in XML_WHITESPACE.iter().enumerate() {
        let name = format!("PaddedWithSpace{index:02}");
        let content = format!("{padding}{document}{padding}");
        let put = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {what:?} ({content:?}): expected the lexicon to be stored, got {:?}",
            json(&put)
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            (
                got["Lexicon"]["Content"].as_str(),
                got["LexiconAttributes"]["LanguageCode"].as_str(),
                got["LexiconAttributes"]["LexemesCount"].as_u64(),
            ),
            (Some(content.as_str()), Some("en-US"), Some(1)),
            "case {what:?}: the padded lexicon must read back as it was put, got {got:?}"
        );
    }
}

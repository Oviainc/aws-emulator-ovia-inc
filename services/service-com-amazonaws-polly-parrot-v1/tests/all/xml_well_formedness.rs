// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WHOLE-DOCUMENT reading of the two XML documents this
//! service accepts: `PutLexicon`'s PLS `Content` and `SynthesizeSpeech`'s `ssml`
//! `Text`.
//!
//! RULE: a request carrying an XML document is accepted only when the whole content is
//! ONE well-formed XML document — exactly one document element, every element closed,
//! and nothing but whitespace, comments and processing instructions outside that
//! document element — so character data or a second root element before or after it is
//! refused with the document error of the operation that read it (`PutLexicon` ->
//! `InvalidLexiconException`, `SynthesizeSpeech` -> `InvalidSsmlException`) and nothing
//! of it is stored, billed or synthesized.
//!
//! The tests check that over TABLES whose rows differ along WHAT sits outside the
//! document element (text, a single character, a CDATA section, an entity reference, a
//! numeric character reference, a second root element of the same name, a second root
//! element of another vocabulary), WHERE it sits (before, after, between), and what XML
//! does admit there (whitespace, a comment, a processing instruction, the XML
//! declaration) — plus the invariant that padding a valid document with whitespace
//! changes nothing about how it reads. The one document an issue happens to quote is a
//! single row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{PLS_NAMESPACE, PLS_VERSION};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// One `<lexeme>`, so no row is refused merely for carrying no lexeme.
const LEXEME: &str = "<lexeme><grapheme>W3C</grapheme>\
                      <alias>World Wide Web Consortium</alias></lexeme>";

/// The language every row's document element declares.
const LANGUAGE: &str = "en-US";

/// A valid PLS lexicon: the document each row puts something outside of.
fn lexicon() -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"{LANGUAGE}\">{LEXEME}</lexicon>"
    )
}

/// The things that are NOT whitespace, a comment or a processing instruction, so XML
/// admits none of them outside a document element. Each is placed before and after the
/// lexicon by the table below.
fn character_data_outside() -> Vec<(&'static str, String)> {
    vec![
        ("a word of text", "garbage".to_string()),
        ("a single character", "x".to_string()),
        ("a CDATA section", "<![CDATA[hello]]>".to_string()),
        ("an entity reference", "&amp;".to_string()),
        ("a numeric character reference", "&#120;".to_string()),
        ("a second root element of another vocabulary", "<speak>hello</speak>".to_string()),
        ("a second EMPTY root element", "<speak/>".to_string()),
        ("a second root element of the same name", lexicon()),
    ]
}

/// The PLS documents that are not ONE well-formed XML document.
fn documents_that_are_not_one_xml_document() -> Vec<(String, String)> {
    let document = lexicon();
    let mut rows: Vec<(String, String)> = Vec::new();
    for (what, outside) in character_data_outside() {
        rows.push((format!("{what} BEFORE the document element"), format!("{outside}{document}")));
        rows.push((format!("{what} AFTER the document element"), format!("{document}{outside}")));
    }
    rows.push((
        "text between two root elements".to_string(),
        format!("{document}between{document}"),
    ));
    rows.push((
        "the document element left unclosed".to_string(),
        format!(
            "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
             xml:lang=\"{LANGUAGE}\">{LEXEME}"
        ),
    ));
    rows.push((
        "a child element left unclosed".to_string(),
        format!(
            "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
             xml:lang=\"{LANGUAGE}\"><lexeme><grapheme>W3C</grapheme></lexicon>"
        ),
    ));
    rows
}

/// The content XML DOES admit around a document element, as whole-document padding of
/// the one valid lexicon.
fn documents_padded_as_xml_allows() -> Vec<(&'static str, String)> {
    let document = lexicon();
    vec![
        ("the document element alone", document.clone()),
        ("whitespace before it", format!("  \n\t{document}")),
        ("whitespace after it", format!("{document}\n  ")),
        ("whitespace on both sides", format!("\n {document} \n")),
        ("a comment before it", format!("<!-- a note -->{document}")),
        ("a comment after it", format!("{document}<!-- a note -->")),
        ("a comment on both sides", format!("<!--a-->{document}<!--b-->")),
        ("a processing instruction after it", format!("{document}<?pls ignore?>")),
        (
            "the XML declaration ahead of it",
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{document}"),
        ),
    ]
}

/// RULE (the refusal half, for PLS): a `Content` that is not one well-formed XML
/// document is HTTP 400 `InvalidLexiconException` and stores nothing — wherever the
/// offending character data or second root element sits.
#[tokio::test]
async fn a_pls_content_that_is_not_one_xml_document_is_refused_and_stores_nothing() {
    let polly = Polly::new();
    for (index, (case, content)) in documents_that_are_not_one_xml_document()
        .into_iter()
        .enumerate()
    {
        let name = format!("NotXml{index:02}");
        let response = polly.put_lexicon(&name, &content).await;
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

        let read_back = polly.get_lexicon(&name).await;
        assert_eq!(
            (read_back.status().as_u16(), error_code(&read_back)),
            (404, "LexiconNotFoundException".to_string()),
            "case {case:?}: a refused put must store nothing, got {:?}",
            json(&read_back)
        );
    }

    let listed = json(&polly.list_lexicons("").await);
    assert_eq!(
        listed["Lexicons"].as_array().map(Vec::len),
        Some(0),
        "no refused document may appear in the store, got {listed:?}"
    );
}

/// RULE (the acceptance half, for PLS): whitespace, a comment, a processing instruction
/// and the XML declaration are what XML admits outside a document element, so a lexicon
/// padded with them is stored and reads back VERBATIM with the attributes its document
/// element declares.
#[tokio::test]
async fn a_pls_content_padded_as_xml_allows_is_stored_and_reads_back_verbatim() {
    let polly = Polly::new();
    for (index, (case, content)) in documents_padded_as_xml_allows().into_iter().enumerate() {
        let name = format!("Padded{index:02}");
        let put = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {case:?} ({content:?}): expected the document to be stored, got {:?}",
            json(&put)
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            (
                got["Lexicon"]["Content"].as_str(),
                got["LexiconAttributes"]["LanguageCode"].as_str(),
                got["LexiconAttributes"]["LexemesCount"].as_u64(),
                got["LexiconAttributes"]["Size"].as_u64(),
            ),
            (
                Some(content.as_str()),
                Some(LANGUAGE),
                Some(1),
                Some(content.chars().count() as u64),
            ),
            "case {case:?}: the padded lexicon must read back as it was put, got {got:?}"
        );
    }
}

/// RULE (the invariant the asymmetry broke): the SAME byte string is refused whether it
/// comes before or after a valid lexicon, because neither position is inside the
/// document — a document element that closed does not make what follows it part of the
/// lexicon.
#[tokio::test]
async fn the_position_of_the_extra_content_does_not_decide_the_answer() {
    let polly = Polly::new();
    let document = lexicon();
    for (what, outside) in character_data_outside() {
        // Each placement of the same byte string: before the lexicon, after it and —
        // for everything that is not itself a whole lexicon — alone.
        let mut placements = vec![
            ("before the lexicon", format!("{outside}{document}")),
            ("after the lexicon", format!("{document}{outside}")),
        ];
        if outside != document {
            placements.push(("alone", outside.clone()));
        }
        for (placement, content) in placements {
            let case = format!("{what} {placement}");
            let response = polly.put_lexicon("Asymmetry", &content).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidLexiconException".to_string()),
                "case {case:?} ({content:?}): the same content must be refused in every \
                 position, got {:?}",
                json(&response)
            );
        }
    }
}

/// One `SynthesizeSpeech` call with `text` as SSML.
async fn synthesize_ssml(polly: &Polly, text: &str) -> http::Response<bytes::Bytes> {
    polly
        .synthesize(&serde_json::json!({
            "Text": text,
            "TextType": "ssml",
            "VoiceId": "Joanna",
            "OutputFormat": "mp3",
        }))
        .await
}

/// RULE (the same rule, at the other XML reader): an `ssml` `Text` that is not one
/// well-formed XML document is HTTP 400 `InvalidSsmlException` rather than a document
/// whose stray characters are billed.
#[tokio::test]
async fn an_ssml_text_that_is_not_one_xml_document_is_refused() {
    let polly = Polly::new();
    const DOCUMENT: &str = "<speak>hi</speak>";
    // What sits outside the document element, spanning the same class as the PLS
    // table: text, one character, a CDATA section, both spellings of a reference, and a
    // second root element both of the same name and from the vocabulary.
    let outside = [
        ("a word of text", "garbage"),
        ("a single character", "x"),
        ("a CDATA section", "<![CDATA[hello]]>"),
        ("an entity reference", "&amp;"),
        ("a numeric character reference", "&#120;"),
        ("a second root element of the same name", "<speak>again</speak>"),
        ("a second root element from the vocabulary", "<break time=\"1s\"/>"),
    ];
    for (what, extra) in outside {
        for (placement, text) in [
            ("before the document element", format!("{extra}{DOCUMENT}")),
            ("after the document element", format!("{DOCUMENT}{extra}")),
        ] {
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

/// RULE (the acceptance half, for SSML): what XML admits outside a document element is
/// not text content, so a document padded with it is synthesized and bills exactly the
/// characters BETWEEN its tags.
#[tokio::test]
async fn an_ssml_text_padded_as_xml_allows_bills_only_the_document_content() {
    let polly = Polly::new();
    // Every row carries the same two characters of text content, so the billed count
    // is what the padding must not change.
    let rows = [
        ("the document element alone", "<speak>hi</speak>".to_string()),
        ("whitespace before it", "  \n<speak>hi</speak>".to_string()),
        ("whitespace after it", "<speak>hi</speak>\n ".to_string()),
        ("whitespace on both sides", "\n <speak>hi</speak> \n".to_string()),
        ("a comment on both sides", "<!--a--><speak>hi</speak><!--b-->".to_string()),
        (
            "a processing instruction after it",
            "<speak>hi</speak><?ssml ignore?>".to_string(),
        ),
        (
            "the XML declaration ahead of it",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<speak>hi</speak>".to_string(),
        ),
    ];
    for (case, text) in rows {
        let response = synthesize_ssml(&polly, &text).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?} ({text:?}): expected the document to be synthesized, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            header(&response, "x-amzn-requestcharacters"),
            "2",
            "case {case:?} ({text:?}): only the text content of the document is billed"
        );
    }
}

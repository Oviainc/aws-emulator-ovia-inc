// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the entity and character REFERENCES the two XML documents this
//! service reads may carry: `PutLexicon`'s PLS `Content` and `SynthesizeSpeech`'s
//! `ssml` `Text`.
//!
//! RULE: a reference is character data of the document that carries it, so a reference
//! that stands for nothing — a general entity nothing declares (XML 1.0 declares only
//! `amp`, `lt`, `gt`, `quot` and `apos`, and a PLS or SSML document declares no others),
//! or a character reference naming no code point — makes the content something other
//! than one well-formed XML document and the request is refused with the document error
//! of the operation that read it (`PutLexicon` -> `InvalidLexiconException`,
//! `SynthesizeSpeech` -> `InvalidSsmlException`), storing, billing and synthesizing
//! nothing, WHEREVER in the document the reference sits; a reference that does resolve
//! is the character it stands for and leaves the document acceptable.
//!
//! The tests check that over the cross product of WHICH reference (each predefined
//! entity, a decimal, a hexadecimal and a non-ASCII character reference, an entity
//! nothing declares, a reference with no digits, and a character reference past the
//! code-point range or inside the surrogate block) and WHERE it sits (a `<grapheme>`,
//! an `<alias>`, a `<phoneme>`, the text of a `<lexeme>` between its children, directly
//! under `<lexicon>`, inside a foreign-vocabulary child, an attribute value of a child
//! and of the document element, and outside the document element) — plus the parity
//! invariant that the same reference gets the same verdict at both XML doors, and the
//! contrast that a reference-looking sequence inside a CDATA section is literal
//! characters and resolves nothing. The one document an issue happens to quote is a
//! single row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{PLS_NAMESPACE, PLS_VERSION};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, header, json, Polly};

/// The language every document element below declares.
const LANGUAGE: &str = "en-US";

/// One `<lexeme>`, so no row is refused merely for carrying no lexeme.
const LEXEME: &str = "<lexeme><grapheme>W3C</grapheme>\
                      <alias>World Wide Web Consortium</alias></lexeme>";

/// The references, and whether each stands for something.
///
/// The resolvable half is the five entities XML 1.0 predefines (the only general
/// entities a document that declares no DTD may refer to) plus the decimal and
/// hexadecimal spellings of a character reference; the unresolvable half is an entity
/// nothing declares, a character reference with no digits, one past the largest code
/// point and one inside the surrogate block, which denotes no character.
const REFERENCES: &[(&str, &str, bool)] = &[
    ("the predefined ampersand entity", "&amp;", true),
    ("the predefined less-than entity", "&lt;", true),
    ("the predefined greater-than entity", "&gt;", true),
    ("the predefined quote entity", "&quot;", true),
    ("the predefined apostrophe entity", "&apos;", true),
    ("a decimal character reference", "&#120;", true),
    ("a hexadecimal character reference", "&#x78;", true),
    ("a character reference past ASCII", "&#233;", true),
    ("an entity nothing declares", "&nosuchentity;", false),
    ("another entity nothing declares", "&undefined;", false),
    ("a character reference with no digits", "&#;", false),
    ("a character reference past the code-point range", "&#x110000;", false),
    ("a character reference inside the surrogate block", "&#xD800;", false),
];

/// A PLS lexicon whose document element carries `attributes` beyond the three PLS
/// requires of it, around `body`.
fn lexicon(attributes: &str, body: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"{LANGUAGE}\"{attributes}>{body}</lexicon>"
    )
}

/// Every place inside a PLS document `reference` is put, as (case, content).
///
/// Every row places the SAME reference; what differs is which element's character data
/// or attribute value holds it, which is exactly what the rule must not depend on.
fn places_inside_the_document(reference: &str) -> Vec<(String, String)> {
    vec![
        (
            "in a <grapheme>".to_string(),
            lexicon(
                "",
                &format!("<lexeme><grapheme>caf{reference}</grapheme><alias>cafe</alias></lexeme>"),
            ),
        ),
        (
            "in an <alias>".to_string(),
            lexicon(
                "",
                &format!("<lexeme><grapheme>G</grapheme><alias>a{reference}b</alias></lexeme>"),
            ),
        ),
        (
            "in a <phoneme>".to_string(),
            lexicon(
                " alphabet=\"ipa\"",
                &format!("<lexeme><grapheme>G</grapheme><phoneme>a{reference}b</phoneme></lexeme>"),
            ),
        ),
        (
            "in the text of a <lexeme>, between its children".to_string(),
            lexicon(
                "",
                &format!(
                    "<lexeme><grapheme>G</grapheme>{reference}<alias>alias</alias></lexeme>"
                ),
            ),
        ),
        (
            "directly under <lexicon>, outside every lexeme".to_string(),
            lexicon("", &format!("{LEXEME}{reference}")),
        ),
        (
            "inside a child from another vocabulary".to_string(),
            lexicon(
                "",
                &format!(
                    "{LEXEME}<other:note xmlns:other=\"http://example.invalid/annotation\">\
                     {reference}</other:note>"
                ),
            ),
        ),
        (
            "in an attribute value of a child element".to_string(),
            lexicon(
                "",
                &format!(
                    "<lexeme role=\"a{reference}b\"><grapheme>G</grapheme>\
                     <alias>alias</alias></lexeme>"
                ),
            ),
        ),
        (
            "in an attribute value of the document element".to_string(),
            lexicon(&format!(" label=\"a{reference}b\""), LEXEME),
        ),
    ]
}

/// RULE: a PLS `Content` carrying a reference that stands for nothing is HTTP 400
/// `InvalidLexiconException` and stores nothing, wherever in the document the reference
/// sits; one carrying a reference that resolves is stored and reads back verbatim.
#[tokio::test]
async fn a_pls_reference_that_stands_for_nothing_is_refused_wherever_it_sits() {
    let polly = Polly::new();
    let mut stored: Vec<String> = Vec::new();
    let mut index = 0usize;
    for &(what, reference, resolves) in REFERENCES {
        for (place, content) in places_inside_the_document(reference) {
            let name = format!("Ref{index:03}");
            index += 1;
            let case = format!("{what} {place}");
            let response = polly.put_lexicon(&name, &content).await;
            if resolves {
                assert_eq!(
                    response.status().as_u16(),
                    200,
                    "case {case:?} ({content:?}): a reference that resolves leaves the \
                     document well-formed, got {:?}",
                    json(&response)
                );
                let got = json(&polly.get_lexicon(&name).await);
                assert_eq!(
                    (
                        got["Lexicon"]["Content"].as_str(),
                        got["LexiconAttributes"]["LanguageCode"].as_str(),
                        got["LexiconAttributes"]["Size"].as_u64(),
                    ),
                    (
                        Some(content.as_str()),
                        Some(LANGUAGE),
                        Some(content.chars().count() as u64),
                    ),
                    "case {case:?}: the stored lexicon must read back as it was put, \
                     got {got:?}"
                );
                stored.push(name);
            } else {
                let body = json(&response);
                assert_eq!(
                    (
                        response.status().as_u16(),
                        error_code(&response),
                        body.get("message").and_then(|m| m.as_str()),
                    ),
                    (
                        400,
                        "InvalidLexiconException".to_string(),
                        Some(errors::INVALID_LEXICON),
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
        }
    }

    let listed = json(&polly.list_lexicons("").await);
    assert_eq!(
        listed["Lexicons"].as_array().map(Vec::len),
        Some(stored.len()),
        "only the documents whose references resolve may be in the store ({} expected), \
         got {listed:?}",
        stored.len()
    );
}

/// RULE (the same rule, outside the document element): a reference there is character
/// data where XML admits none, so it is refused whether or not it stands for something.
#[tokio::test]
async fn a_pls_reference_outside_the_document_element_is_refused_either_way() {
    let polly = Polly::new();
    let document = lexicon("", LEXEME);
    for &(what, reference, _) in REFERENCES {
        for (placement, content) in [
            ("before the document element", format!("{reference}{document}")),
            ("after the document element", format!("{document}{reference}")),
            ("alone", reference.to_string()),
        ] {
            let case = format!("{what} {placement}");
            let response = polly.put_lexicon("Outside", &content).await;
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (400, "InvalidLexiconException".to_string()),
                "case {case:?} ({content:?}): character data outside the document \
                 element is refused, got {:?}",
                json(&response)
            );
        }
    }
}

/// RULE (the contrast that keeps the refusal from being over-broad): a CDATA section
/// carries its characters verbatim and resolves nothing, so a sequence that merely LOOKS
/// like a reference is text there and the document stays well-formed — and its
/// characters count as the ones it literally spells.
#[tokio::test]
async fn a_reference_looking_sequence_inside_cdata_is_literal_text() {
    let polly = Polly::new();
    for (index, &(what, reference, _)) in REFERENCES.iter().enumerate() {
        let name = format!("Cdata{index:03}");
        let content = lexicon(
            "",
            &format!(
                "<lexeme><grapheme>G</grapheme><alias><![CDATA[{reference}]]></alias></lexeme>"
            ),
        );
        let response = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {what:?} ({content:?}): a CDATA section resolves nothing, so it is \
             well-formed text, got {:?}",
            json(&response)
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            got["Lexicon"]["Content"].as_str(),
            Some(content.as_str()),
            "case {what:?}: the stored lexicon must read back as it was put, got {got:?}"
        );
    }
}

/// RULE (the invariant the guard broke): the SAME reference gets the SAME verdict at
/// both of this service's XML doors, because the rule is XML's and not the operation's
/// — only the error code differs.
#[tokio::test]
async fn the_same_reference_gets_the_same_verdict_at_both_xml_doors() {
    let polly = Polly::new();
    for &(what, reference, resolves) in REFERENCES {
        // The PLS document puts the reference in a `<grapheme>`: inside the document,
        // outside any replacement element.
        let content = lexicon(
            "",
            &format!("<lexeme><grapheme>a{reference}b</grapheme><alias>alias</alias></lexeme>"),
        );
        let put = polly.put_lexicon("Parity", &content).await;
        let text = format!("<speak>a{reference}b</speak>");
        let synthesized = polly
            .synthesize(&serde_json::json!({
                "Text": text,
                "TextType": "ssml",
                "VoiceId": "Joanna",
                "OutputFormat": "mp3",
            }))
            .await;
        assert_eq!(
            (
                put.status().as_u16() == 200,
                synthesized.status().as_u16() == 200
            ),
            (resolves, resolves),
            "case {what:?}: the lexicon door answered {} and the SSML door {} for the \
             same reference; expected both to accept it: {resolves}",
            put.status(),
            synthesized.status()
        );
        if resolves {
            assert_eq!(
                header(&synthesized, "x-amzn-requestcharacters"),
                "3",
                "case {what:?} ({text:?}): a reference that resolves bills as the one \
                 character it stands for"
            );
        } else {
            let put_body = json(&put);
            assert_eq!(
                (error_code(&put), put_body.get("message").and_then(|m| m.as_str())),
                (
                    "InvalidLexiconException".to_string(),
                    Some(errors::INVALID_LEXICON)
                ),
                "case {what:?}: expected the lexicon document refusal, got {put_body:?}"
            );
            let ssml_body = json(&synthesized);
            assert_eq!(
                (
                    error_code(&synthesized),
                    ssml_body.get("message").and_then(|m| m.as_str())
                ),
                ("InvalidSsmlException".to_string(), Some(errors::INVALID_SSML)),
                "case {what:?}: expected the SSML document refusal, got {ssml_body:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for HOW MUCH of a `<lexeme>`'s replacement the per-lexeme quota is
//! measured against.
//!
//! RULE: the replacement a `<lexeme>` defines is the whole character content of its
//! `<alias>`/`<phoneme>` element — every text node under that element, whatever
//! elements the text is nested in and whatever text follows them — so `PutLexicon`
//! refuses the document with HTTP 400 `MaxLexemeLengthExceededException`, storing
//! nothing, exactly when that total passes Amazon Polly's published per-lexeme quota,
//! and stores it when the total is at or under the quota. The tests check that over a
//! TABLE whose rows differ along the nesting shape (flat, a child at the end, a child
//! with text after it, text on both sides, three levels deep, a child from another
//! vocabulary), the form the characters arrive in (plain text, CDATA, entity
//! references), which replacement element is outermost, and whether the total lands
//! below, on or past the quota — the one document an issue happens to quote is a single
//! row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    MAX_LEXEME_CHARACTERS, PLS_NAMESPACE, PLS_VERSION,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// A PLS lexicon whose single `<lexeme>` carries `body` as its replacement element.
fn pls(body: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" alphabet=\"ipa\" \
         xml:lang=\"en-US\"><lexeme><grapheme>G</grapheme>{body}</lexeme></lexicon>"
    )
}

/// The character content of one replacement element, and the markup that carries it.
///
/// `outer` is the replacement element the quota is measured on; `inner` is the markup
/// between its tags; `characters` is how many characters of content that markup holds,
/// counted by hand so the expectation is independent of the parser under test.
struct Row {
    case: &'static str,
    inner: String,
    characters: usize,
}

/// The rows, built for `outer` as the outermost replacement element and `nested` as the
/// replacement element nested inside it. PLS defines both forms, and the quota is on
/// both whichever way round they are nested.
fn rows(outer: &str, nested: &str) -> Vec<Row> {
    let quota = MAX_LEXEME_CHARACTERS;
    let mut rows = vec![
        Row {
            case: "flat text well under the quota",
            inner: "x".to_string(),
            characters: 1,
        },
        Row {
            case: "flat text exactly at the quota",
            inner: "x".repeat(quota),
            characters: quota,
        },
        Row {
            case: "flat text one past the quota",
            inner: "x".repeat(quota + 1),
            characters: quota + 1,
        },
        Row {
            case: "text then a nested replacement element, one past the quota",
            inner: format!("{}<{nested}>z</{nested}>", "x".repeat(quota)),
            characters: quota + 1,
        },
        Row {
            case: "text far past the quota then a nested replacement element",
            inner: format!("{}<{nested}>z</{nested}>", "x".repeat(quota * 2)),
            characters: quota * 2 + 1,
        },
        Row {
            case: "text then a nested replacement element, exactly at the quota",
            inner: format!("{}<{nested}>z</{nested}>", "x".repeat(quota - 1)),
            characters: quota,
        },
        Row {
            case: "a nested replacement element then text, one past the quota",
            inner: format!("<{nested}>z</{nested}>{}", "x".repeat(quota)),
            characters: quota + 1,
        },
        Row {
            case: "text on both sides of a nested element, one past the quota",
            inner: format!(
                "{}<{nested}>z</{nested}>{}",
                "x".repeat(quota / 2),
                "y".repeat(quota - quota / 2)
            ),
            characters: quota + 1,
        },
        Row {
            case: "text on both sides of a nested element, one under the quota",
            inner: format!(
                "{}<{nested}>z</{nested}>{}",
                "x".repeat(quota / 2 - 1),
                "y".repeat(quota - quota / 2 - 1)
            ),
            characters: quota - 1,
        },
        Row {
            case: "an empty nested element adds no characters",
            inner: format!("{}<{nested}/>", "x".repeat(quota)),
            characters: quota,
        },
        Row {
            case: "three levels of replacement elements summing past the quota",
            inner: format!(
                "{}<{nested}>{}<{outer}>{}</{outer}></{nested}>",
                "x".repeat(40),
                "y".repeat(40),
                "z".repeat(40)
            ),
            characters: 120,
        },
        Row {
            case: "three levels of replacement elements summing under the quota",
            inner: format!(
                "{}<{nested}>{}<{outer}>{}</{outer}></{nested}>",
                "x".repeat(30),
                "y".repeat(30),
                "z".repeat(30)
            ),
            characters: 90,
        },
        Row {
            case: "the innermost element alone is past the quota",
            inner: format!("<{nested}>{}</{nested}>", "y".repeat(quota + 1)),
            characters: quota + 1,
        },
        Row {
            case: "a child from another vocabulary carrying the text",
            inner: format!(
                "{}<other:note xmlns:other=\"http://example.invalid/annotation\">{}\
                 </other:note>",
                "x".repeat(60),
                "y".repeat(60)
            ),
            characters: 120,
        },
        Row {
            case: "a CDATA section inside a nested replacement element",
            inner: format!(
                "{}<{nested}><![CDATA[{}]]></{nested}>",
                "x".repeat(60),
                "y".repeat(60)
            ),
            characters: 120,
        },
        Row {
            // An entity reference stands for the one character it names, so five of
            // them are five characters of replacement and not thirty.
            case: "entity references inside a nested element, one past the quota",
            inner: format!(
                "{}<{nested}>{}</{nested}>",
                "x".repeat(quota - 4),
                "&amp;".repeat(5)
            ),
            characters: quota + 1,
        },
        Row {
            // Every character of the content counts once, however it is spelled.
            case: "flat text spelled with entity references, one past the quota",
            inner: "&amp;".repeat(quota + 1),
            characters: quota + 1,
        },
        Row {
            case: "flat text spelled with entity references, at the quota",
            inner: "&amp;".repeat(quota),
            characters: quota,
        },
        Row {
            case: "flat text spelled with numeric character references, one past the quota",
            inner: "&#120;".repeat(quota + 1),
            characters: quota + 1,
        },
        Row {
            case: "entity references inside a nested element, at the quota",
            inner: format!(
                "{}<{nested}>{}</{nested}>",
                "x".repeat(quota - 5),
                "&amp;".repeat(5)
            ),
            characters: quota,
        },
    ];
    for row in &mut rows {
        row.inner = format!("<{outer}>{}</{outer}>", row.inner);
    }
    rows
}

/// RULE: the quota is measured against the WHOLE content of a replacement element, so a
/// document is refused exactly when that total passes the quota — nesting an element
/// inside a replacement neither hides the text around it nor exempts the replacement.
#[tokio::test]
async fn a_replacement_is_measured_over_its_whole_content() {
    // AWS's published quotas for Amazon Polly cap a lexeme at 100 characters.
    assert_eq!(
        MAX_LEXEME_CHARACTERS, 100,
        "the published per-lexeme quota is 100 characters"
    );
    let polly = Polly::new();
    for (outer, nested) in [("alias", "phoneme"), ("phoneme", "alias")] {
        for (index, row) in rows(outer, nested).into_iter().enumerate() {
            let case = format!("<{outer}> with <{nested}> inside: {}", row.case);
            let name = format!("Nest{}{index:02}", &outer[..1].to_uppercase());
            let content = pls(&row.inner);
            let response = polly.put_lexicon(&name, &content).await;
            let refused = row.characters > MAX_LEXEME_CHARACTERS;
            if refused {
                let body = json(&response);
                assert_eq!(
                    (
                        response.status().as_u16(),
                        error_code(&response),
                        body.get("message").and_then(|m| m.as_str())
                    ),
                    (
                        400,
                        "MaxLexemeLengthExceededException".to_string(),
                        Some(errors::MAX_LEXEME_LENGTH_EXCEEDED)
                    ),
                    "case {case:?} ({} characters of replacement): expected the \
                     lexeme-quota refusal, got {body:?}",
                    row.characters
                );
                let read_back = polly.get_lexicon(&name).await;
                assert_eq!(
                    (read_back.status().as_u16(), error_code(&read_back)),
                    (404, "LexiconNotFoundException".to_string()),
                    "case {case:?}: a refused put must store nothing, got {:?}",
                    json(&read_back)
                );
            } else {
                assert_eq!(
                    response.status().as_u16(),
                    200,
                    "case {case:?} ({} characters of replacement): expected the \
                     document to be stored, got {:?}",
                    row.characters,
                    json(&response)
                );
                let got = json(&polly.get_lexicon(&name).await);
                assert_eq!(
                    (
                        got["Lexicon"]["Content"].as_str(),
                        got["LexiconAttributes"]["LexemesCount"].as_u64()
                    ),
                    (Some(content.as_str()), Some(1)),
                    "case {case:?}: the stored document must read back as it was put, \
                     got {got:?}"
                );
            }
        }
    }
}

/// RULE (the same measurement, reached through a sibling): every replacement element of
/// every `<lexeme>` in the document is measured, so one oversized replacement refuses
/// the document however many well-sized ones surround it.
#[tokio::test]
async fn one_oversized_replacement_among_many_refuses_the_document() {
    let polly = Polly::new();
    let short = "short";
    let long = "x".repeat(MAX_LEXEME_CHARACTERS + 1);
    // case, the lexemes of the document, whether the document is refused
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "two lexemes, both well-sized",
            format!(
                "<lexeme><grapheme>A</grapheme><alias>{short}</alias></lexeme>\
                 <lexeme><grapheme>B</grapheme><phoneme>{short}</phoneme></lexeme>"
            ),
            false,
        ),
        (
            "two replacement siblings in one lexeme, both well-sized",
            format!(
                "<lexeme><grapheme>A</grapheme><alias>{short}</alias>\
                 <phoneme>{short}</phoneme></lexeme>"
            ),
            false,
        ),
        (
            "the second of two replacement siblings is oversized",
            format!(
                "<lexeme><grapheme>A</grapheme><alias>{short}</alias>\
                 <phoneme>{long}</phoneme></lexeme>"
            ),
            true,
        ),
        (
            "the oversized replacement is in the last of three lexemes",
            format!(
                "<lexeme><grapheme>A</grapheme><alias>{short}</alias></lexeme>\
                 <lexeme><grapheme>B</grapheme><alias>{short}</alias></lexeme>\
                 <lexeme><grapheme>C</grapheme><alias>{long}</alias></lexeme>"
            ),
            true,
        ),
        (
            // A `<lexeme>` closing inside another does not take what follows it out of
            // the enclosing lexeme, so the replacement after it is still measured.
            "an oversized replacement following a lexeme nested in a lexeme",
            format!(
                "<lexeme><grapheme>A</grapheme>\
                 <lexeme><grapheme>B</grapheme><alias>{short}</alias></lexeme>\
                 <alias>{long}</alias></lexeme>"
            ),
            true,
        ),
        (
            "a well-sized replacement following a lexeme nested in a lexeme",
            format!(
                "<lexeme><grapheme>A</grapheme>\
                 <lexeme><grapheme>B</grapheme><alias>{short}</alias></lexeme>\
                 <alias>{short}</alias></lexeme>"
            ),
            false,
        ),
    ];

    for (index, (case, body, refused)) in rows.into_iter().enumerate() {
        let name = format!("Many{index:02}");
        let content = format!(
            "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" alphabet=\"ipa\" \
             xml:lang=\"en-US\">{body}</lexicon>"
        );
        let response = polly.put_lexicon(&name, &content).await;
        let expected_status = if refused { 400 } else { 200 };
        assert_eq!(
            response.status().as_u16(),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {:?}",
            json(&response)
        );
        if refused {
            assert_eq!(
                error_code(&response),
                "MaxLexemeLengthExceededException".to_string(),
                "case {case:?}: expected the lexeme-quota code, got {:?}",
                json(&response)
            );
        }
        let read_back = polly.get_lexicon(&name).await;
        assert_eq!(
            read_back.status().as_u16(),
            if refused { 404 } else { 200 },
            "case {case:?}: the store must hold the document only when it was \
             accepted, got {:?}",
            json(&read_back)
        );
    }
}

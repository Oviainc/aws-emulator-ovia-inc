// SPDX-License-Identifier: CC0-1.0

//! What WORDING each modelled Polly failure carries on the wire.
//!
//! RULE: Amazon Polly words a modelled exception with one fixed sentence per error
//! code — the sentence its own responses carry — and the documentation prose the
//! Smithy model attaches to an error SHAPE is not that sentence, so a code whose
//! wording has been observed carries the observed sentence rather than the model's
//! doc text.
//!
//! The observed sentences are pinned as LITERALS in this file (a reworded or mistyped
//! constant then cannot pass), and each test checks the wire really carries them over a
//! TABLE of requests spanning the class the code covers: both PLS replacement forms,
//! several lengths past the quota, ASCII and non-ASCII replacements, the oversized
//! lexeme in several positions, and several lexicon names.

use bytes::Bytes;
use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    MAX_LEXEME_CHARACTERS, MAX_LEXICON_CHARACTERS,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// The sentence Amazon Polly's own responses carry for
/// `MaxLexemeLengthExceededException` (recorded trace
/// `polly.put-lexicon-max-lexeme-length-exceeded-rejected`).
const OBSERVED_MAX_LEXEME_LENGTH_EXCEEDED: &str = "Lexeme rule length has been exceeded";

/// The prose the Smithy model documents the `MaxLexemeLengthExceededException` SHAPE
/// with (`smithy_query(query="shape",shape="MaxLexemeLengthExceededException")`). It
/// documents the error; it is not what the service answers with.
const MODEL_DOC_FOR_MAX_LEXEME_LENGTH_EXCEEDED: &str =
    "The maximum size of the lexeme would be exceeded by this operation.";

/// The `message` an error response carries, absent when it carries none.
fn error_message(response: &http::Response<Bytes>) -> Option<String> {
    let body = json(response);
    for key in ["message", "Message"] {
        if let Some(message) = body.get(key).and_then(|m| m.as_str()) {
            return Some(message.to_string());
        }
    }
    None
}

/// Assert a refusal's status, code and exact message.
fn assert_refusal(
    case: &str,
    response: &http::Response<Bytes>,
    expected_status: u16,
    expected_code: &str,
    expected_message: &str,
) {
    let actual = (
        response.status().as_u16(),
        error_code(response),
        error_message(response),
    );
    assert_eq!(
        actual,
        (
            expected_status,
            expected_code.to_string(),
            Some(expected_message.to_string())
        ),
        "case {case:?}: expected {expected_status} {expected_code} / \
         {expected_message:?}, got {actual:?}"
    );
}

/// A PLS lexicon whose `index`-th lexeme (of `lexemes`) carries a `replacement` in
/// `element`; every other lexeme carries a short one.
fn pls_with(element: &str, replacement: &str, index: usize, lexemes: usize) -> String {
    let body: String = (0..lexemes)
        .map(|i| {
            let value = if i == index { replacement } else { "ok" };
            format!("<lexeme><grapheme>G{i}</grapheme><{element}>{value}</{element}></lexeme>")
        })
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <lexicon version=\"1.0\" \
         xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
         alphabet=\"ipa\" xml:lang=\"en-US\">{body}</lexicon>"
    )
}

/// RULE, for `MaxLexemeLengthExceededException`: every `PutLexicon` whose lexeme
/// replacement is past the published quota is refused HTTP 400 with that code and with
/// Amazon Polly's observed sentence for the code — the same sentence for every such
/// request, and never the model's shape documentation — and stores nothing.
#[tokio::test]
async fn an_oversized_lexeme_is_refused_with_pollys_observed_sentence() {
    // The constant the whole service words this code with is the observed sentence,
    // not the model's prose about the shape.
    assert_eq!(
        errors::MAX_LEXEME_LENGTH_EXCEEDED, OBSERVED_MAX_LEXEME_LENGTH_EXCEEDED,
        "the service's sentence for MaxLexemeLengthExceededException must be the \
         observed one, got {:?}",
        errors::MAX_LEXEME_LENGTH_EXCEEDED
    );
    assert_ne!(
        errors::MAX_LEXEME_LENGTH_EXCEEDED, MODEL_DOC_FOR_MAX_LEXEME_LENGTH_EXCEEDED,
        "the model's documentation of the error shape is not the sentence Amazon \
         Polly answers with"
    );

    let polly = Polly::new();
    // The rows differ along what the rule generalises over: the replacement form, the
    // character the replacement is built from (ASCII and multi-byte), how far past the
    // quota it is, which lexeme of the document is the oversized one, and how many
    // lexemes the document has.
    let rows: [(&str, &str, char, usize, usize, usize); 8] = [
        // case, element, filler, replacement length, lexeme index, lexeme count
        ("alias one past the quota", "alias", 'x', MAX_LEXEME_CHARACTERS + 1, 0, 1),
        ("alias far past the quota", "alias", 'x', 150, 0, 1),
        ("alias of 1,000 characters", "alias", 'x', 1_000, 0, 1),
        ("phoneme one past the quota", "phoneme", 'x', MAX_LEXEME_CHARACTERS + 1, 0, 1),
        ("phoneme far past the quota", "phoneme", 'ˈ', 180, 0, 1),
        ("non-ASCII alias past the quota", "alias", 'é', 101, 0, 1),
        ("the second of three lexemes", "alias", 'x', 120, 1, 3),
        ("the last of five lexemes", "phoneme", 'x', 101, 4, 5),
    ];
    for (index, (case, element, filler, length, lexeme_index, lexemes)) in
        rows.into_iter().enumerate()
    {
        let replacement: String = filler.to_string().repeat(length);
        let content = pls_with(element, &replacement, lexeme_index, lexemes);
        assert!(
            content.chars().count() <= MAX_LEXICON_CHARACTERS,
            "case {case:?}: the document must be inside the lexicon size quota so the \
             lexeme quota is what refuses it ({} characters)",
            content.chars().count()
        );
        let name = format!("LongLexeme{index}");
        let response = polly.put_lexicon(&name, &content).await;
        assert_refusal(
            case,
            &response,
            400,
            "MaxLexemeLengthExceededException",
            OBSERVED_MAX_LEXEME_LENGTH_EXCEEDED,
        );

        // A refused put stores nothing, so the read back is the not-found refusal with
        // its own observed sentence.
        let read_back = polly.get_lexicon(&name).await;
        assert_refusal(
            &format!("{case}: read back after the refusal"),
            &read_back,
            404,
            "LexiconNotFoundException",
            "Lexicon not found",
        );
    }

    // The positive control: a replacement at the quota is stored and reads back
    // byte-identical, so the sentence above is a refusal and not this operation's
    // answer to every document.
    let within = pls_with("alias", &"x".repeat(MAX_LEXEME_CHARACTERS), 0, 1);
    let stored = polly.put_lexicon("AtTheQuota", &within).await;
    assert_eq!(
        stored.status(),
        200,
        "a replacement at the quota is stored, body {:?}",
        json(&stored)
    );
    let read_back = polly.get_lexicon("AtTheQuota").await;
    assert_eq!(
        read_back.status(),
        200,
        "the stored lexicon must read back, body {:?}",
        json(&read_back)
    );
    assert_eq!(
        json(&read_back)["Lexicon"]["Content"].as_str(),
        Some(within.as_str()),
        "the stored lexicon must read back byte-identical"
    );
}

/// RULE, across the codes one operation can answer: each refusal carries the observed
/// fixed sentence for ITS code, so one code's wording is never another's and none of
/// them is the model's shape documentation.
#[tokio::test]
async fn each_put_lexicon_refusal_carries_the_sentence_for_its_own_code() {
    let polly = Polly::new();
    // The sentences are the observed literals; each row is a different document that
    // reaches the code.
    let rows: [(&str, String, &str, &str); 9] = [
        (
            "not XML at all",
            "nonsense <".to_string(),
            "InvalidLexiconException",
            "Invalid PLS Lexicon",
        ),
        (
            "an SSML document",
            "<speak>hello</speak>".to_string(),
            "InvalidLexiconException",
            "Invalid PLS Lexicon",
        ),
        (
            "an unsupported alphabet",
            r#"<lexicon version="1.0" alphabet="x-klingon" xml:lang="en-US"/>"#.to_string(),
            "InvalidLexiconException",
            "Invalid PLS Lexicon",
        ),
        (
            "an unsupported language",
            r#"<lexicon version="1.0" alphabet="ipa" xml:lang="xx-ZZ"/>"#.to_string(),
            "UnsupportedPlsLanguageException",
            "Unsupported PLS language",
        ),
        (
            "another unsupported language",
            r#"<lexicon version="1.0" alphabet="ipa" xml:lang="klingon"/>"#.to_string(),
            "UnsupportedPlsLanguageException",
            "Unsupported PLS language",
        ),
        (
            "past the lexicon size quota",
            pls_with("grapheme", &"g".repeat(MAX_LEXICON_CHARACTERS), 0, 1),
            "LexiconSizeExceededException",
            "Maximum lexicons size has been exceeded",
        ),
        (
            "far past the lexicon size quota",
            pls_with("grapheme", &"g".repeat(MAX_LEXICON_CHARACTERS * 10), 0, 1),
            "LexiconSizeExceededException",
            "Maximum lexicons size has been exceeded",
        ),
        (
            "an alias past the lexeme quota",
            pls_with("alias", &"x".repeat(MAX_LEXEME_CHARACTERS + 1), 0, 1),
            "MaxLexemeLengthExceededException",
            OBSERVED_MAX_LEXEME_LENGTH_EXCEEDED,
        ),
        (
            "a phoneme past the lexeme quota",
            pls_with("phoneme", &"x".repeat(MAX_LEXEME_CHARACTERS * 2), 0, 1),
            "MaxLexemeLengthExceededException",
            OBSERVED_MAX_LEXEME_LENGTH_EXCEEDED,
        ),
    ];
    for (index, (case, content, code, sentence)) in rows.into_iter().enumerate() {
        let name = format!("Refused{index}");
        let response = polly.put_lexicon(&name, &content).await;
        assert_refusal(case, &response, 400, code, sentence);
        // A fixed sentence says nothing about the request.
        let message = error_message(&response).unwrap_or_default();
        for value in [name.as_str(), content.as_str()] {
            assert!(
                !message.contains(value),
                "case {case:?}: message {message:?} echoes the request value {value:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one thing a lexicon's document element has to do: declare
//! the document to be a PLS lexicon.
//!
//! RULE: a `PutLexicon` whose `Content` has a document element that does not declare a
//! PLS lexicon — `<lexicon>` bound to the PLS namespace, carrying `version` fixed at
//! the PLS version and the `xml:lang` naming the lexicon's language — is refused HTTP
//! 400 `InvalidLexiconException` and stores nothing; one that does is stored and reads
//! back with the language it declared. The tests check that over a TABLE whose rows
//! differ along which declaration is missing, how it is missing (absent, empty, a
//! foreign value, the unprefixed attribute instead of the prefixed one), the prefix the
//! document spells its elements with, and what the document carries inside — the one
//! document an issue happens to quote is a single row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{PLS_NAMESPACE, PLS_VERSION};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// One `<lexeme>`, so a row is not refused merely for being an empty document.
const LEXEME: &str = "<lexeme><grapheme>W3C</grapheme>\
                      <alias>World Wide Web Consortium</alias></lexeme>";

/// The documents whose document element does NOT declare a PLS lexicon, with the
/// declaration each one is missing.
fn documents_that_declare_no_lexicon() -> Vec<(&'static str, String)> {
    vec![
        // The document the issue quotes: no namespace, no version, no language.
        (
            "no declaration at all",
            format!("<lexicon>{LEXEME}</lexicon>"),
        ),
        ("an empty lexicon with no declaration", "<lexicon/>".to_string()),
        (
            "the namespace missing",
            format!("<lexicon version=\"{PLS_VERSION}\" xml:lang=\"en-US\">{LEXEME}</lexicon>"),
        ),
        (
            "a foreign namespace",
            format!(
                "<lexicon xmlns=\"http://example.invalid/pronunciation-lexicon\" \
                 version=\"{PLS_VERSION}\" xml:lang=\"en-US\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "the SSML namespace",
            format!(
                "<lexicon xmlns=\"http://www.w3.org/2001/10/synthesis\" \
                 version=\"{PLS_VERSION}\" xml:lang=\"en-US\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "a prefix bound to a foreign namespace",
            format!(
                "<pls:lexicon xmlns:pls=\"http://example.invalid/lexicon\" \
                 version=\"{PLS_VERSION}\" xml:lang=\"en-US\"/>"
            ),
        ),
        (
            "the version missing",
            format!("<lexicon xmlns=\"{PLS_NAMESPACE}\" xml:lang=\"en-US\">{LEXEME}</lexicon>"),
        ),
        (
            "an empty version",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"\" \
                 xml:lang=\"en-US\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "another version",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"2.0\" \
                 xml:lang=\"en-US\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "the language missing",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 alphabet=\"ipa\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "an empty language",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"\">{LEXEME}</lexicon>"
            ),
        ),
        (
            // Only the prefixed attribute is the language of an XML element.
            "an unprefixed lang instead of xml:lang",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 lang=\"en-US\">{LEXEME}</lexicon>"
            ),
        ),
        (
            "every declaration on a child instead of the document element",
            format!(
                "<lexicon><lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"en-US\">{LEXEME}</lexicon></lexicon>"
            ),
        ),
        (
            "the XML declaration alone",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        ),
    ]
}

/// RULE (the refusal half): a document element declaring no PLS lexicon is refused
/// HTTP 400 `InvalidLexiconException` with the sentence this service words that code
/// with, and the store is left exactly as it was.
#[tokio::test]
async fn a_document_element_that_declares_no_pls_lexicon_is_refused_and_stores_nothing() {
    let polly = Polly::new();
    for (index, (case, content)) in documents_that_declare_no_lexicon()
        .into_iter()
        .enumerate()
    {
        let name = format!("NoDecl{index:02}");
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
            "case {case:?}: expected the invalid-document refusal, got {body:?}"
        );

        // Nothing was stored, so the read back is the not-found refusal.
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

/// RULE (the acceptance half): a document element that DOES declare a PLS lexicon is
/// stored whatever prefix it spells the PLS vocabulary with and whatever it carries
/// inside, and reads back with the language it declared.
#[tokio::test]
async fn a_document_element_that_declares_a_pls_lexicon_is_stored_with_its_language() {
    let polly = Polly::new();
    // case, document, the language the document declares, its lexeme count
    let rows: Vec<(&str, String, &str, u64)> = vec![
        (
            "the default namespace with an alphabet",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 alphabet=\"ipa\" xml:lang=\"en-US\">{LEXEME}</lexicon>"
            ),
            "en-US",
            1,
        ),
        (
            "no alphabet, which PLS leaves optional",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"de-DE\">{LEXEME}</lexicon>"
            ),
            "de-DE",
            1,
        ),
        (
            "no lexeme at all",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"ja-JP\"/>"
            ),
            "ja-JP",
            0,
        ),
        (
            "a bare primary language subtag",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"en\">{LEXEME}{LEXEME}</lexicon>"
            ),
            "en",
            2,
        ),
        (
            "a language with no region at all",
            format!(
                "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"arb\">{LEXEME}</lexicon>"
            ),
            "arb",
            1,
        ),
        (
            "the PLS namespace bound to a prefix",
            format!(
                "<pls:lexicon xmlns:pls=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"cmn-CN\"><pls:lexeme><pls:grapheme>W3C</pls:grapheme>\
                 <pls:alias>W3C</pls:alias></pls:lexeme></pls:lexicon>"
            ),
            "cmn-CN",
            1,
        ),
        (
            "an XML declaration ahead of the document element",
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <lexicon version=\"{PLS_VERSION}\" xmlns=\"{PLS_NAMESPACE}\" \
                 xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
                 alphabet=\"x-sampa\" xml:lang=\"en-GB\">{LEXEME}</lexicon>"
            ),
            "en-GB",
            1,
        ),
    ];

    for (index, (case, content, language, lexemes)) in rows.into_iter().enumerate() {
        let name = format!("Declared{index:02}");
        let put = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            put.status(),
            200,
            "case {case:?}: expected the document to be stored, got {:?}",
            json(&put)
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            (
                got["Lexicon"]["Content"].as_str(),
                got["LexiconAttributes"]["LanguageCode"].as_str(),
                got["LexiconAttributes"]["LexemesCount"].as_u64(),
            ),
            (Some(content.as_str()), Some(language), Some(lexemes)),
            "case {case:?}: the lexicon must read back with what its document declared, \
             got {got:?}"
        );
    }
}

/// RULE (the invariant behind it): every lexicon a read reports carries a
/// `LanguageCode`, because a document that declares no language is never stored —
/// checked over both readers and over a store built from the accepted and the refused
/// documents together.
#[tokio::test]
async fn every_reported_lexicon_carries_a_language_code() {
    let polly = Polly::new();
    let accepted = [
        ("WithIpa", "ipa", "en-US"),
        ("WithSampa", "x-sampa", "en-GB"),
        ("WithKana", "x-amazon-pron-kana", "ja-JP"),
    ];
    for (name, alphabet, language) in accepted {
        let content = format!(
            "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
             alphabet=\"{alphabet}\" xml:lang=\"{language}\">{LEXEME}</lexicon>"
        );
        assert_eq!(
            polly.put_lexicon(name, &content).await.status(),
            200,
            "lexicon {name} must be stored"
        );
    }
    // Interleaved refusals must leave the store as it was.
    for (case, content) in documents_that_declare_no_lexicon() {
        let response = polly.put_lexicon("Refused", &content).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected a refusal, got {:?}",
            json(&response)
        );
    }

    let listed = json(&polly.list_lexicons("").await);
    let entries = listed["Lexicons"].as_array().expect("Lexicons is a list");
    assert_eq!(
        entries.len(),
        accepted.len(),
        "only the accepted documents are stored, got {listed:?}"
    );
    for entry in entries {
        let name = entry["Name"].as_str().unwrap_or_default().to_string();
        assert!(
            entry["Attributes"]["LanguageCode"].as_str().is_some(),
            "ListLexicons reported {name:?} with no LanguageCode: {entry:?}"
        );
        let got = json(&polly.get_lexicon(&name).await);
        assert!(
            got["LexiconAttributes"]["LanguageCode"].as_str().is_some(),
            "GetLexicon reported {name:?} with no LanguageCode: {got:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the language a stored lexicon reports.
//!
//! RULE: a reported member that targets a modelled enum only ever carries a value that
//! enum declares (or, for `LexiconAttributes$LanguageCode`, the catalog's own spelling
//! of a bare primary subtag, which that member's documentation blesses). The language a
//! PLS document declares in `xml:lang` is a language tag, whose subtags are
//! case-insensitive, so a supported language spelled in any case is accepted — and
//! stored and reported in the CATALOG's spelling, never the document's. A tag naming no
//! catalogued language is refused HTTP 400 `UnsupportedPlsLanguageException` and stores
//! nothing.
//!
//! Each test checks that over a TABLE that spans the class: every language the model's
//! `LanguageCode` enum declares, in every case form, the bare primary subtag of each of
//! them, and tags outside the catalog in several shapes. The one document an issue
//! happens to quote is a single row.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{PLS_NAMESPACE, PLS_VERSION};
use service_com_amazonaws_polly_parrot_v1::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// One `<lexeme>`, so no row is refused merely for carrying nothing.
const LEXEME: &str = "<lexeme><grapheme>W3C</grapheme>\
                      <alias>World Wide Web Consortium</alias></lexeme>";

/// A PLS lexicon declaring `language`.
fn document(language: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         alphabet=\"ipa\" xml:lang=\"{language}\">{LEXEME}</lexicon>"
    )
}

/// The case forms of one language tag a document may spell: as given, all lower, all
/// upper, and with every letter's case inverted.
fn case_forms(tag: &str) -> Vec<(&'static str, String)> {
    vec![
        ("as the catalog spells it", tag.to_string()),
        ("all lower case", tag.to_ascii_lowercase()),
        ("all upper case", tag.to_ascii_uppercase()),
        (
            "every letter's case inverted",
            tag.chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        c.to_ascii_lowercase()
                    } else {
                        c.to_ascii_uppercase()
                    }
                })
                .collect(),
        ),
    ]
}

/// Whether `reported` is a value the `LanguageCode` enum declares, or the primary
/// subtag of one — the only two forms `LexiconAttributes$LanguageCode` may carry.
fn is_reportable_language(reported: &str) -> bool {
    LANGUAGE_CODE_VALUES.contains(&reported)
        || LANGUAGE_CODE_VALUES
            .iter()
            .any(|code| code.split('-').next() == Some(reported))
}

/// RULE: whatever case a document spells a catalogued language in, the lexicon reads
/// back with that language in the CATALOG's spelling — checked over every value the
/// model's `LanguageCode` enum declares.
#[tokio::test]
async fn every_catalogued_language_reads_back_in_the_catalog_spelling() {
    let polly = Polly::new();
    // One name, re-put per row: `PutLexicon` is an insert-or-replace, so the store
    // stays one entry deep and the row under test is the one being read back.
    let name = "Catalogued";
    for language in LANGUAGE_CODE_VALUES {
        for (case, declared) in case_forms(language) {
            let content = document(&declared);
            let put = polly.put_lexicon(name, &content).await;
            assert_eq!(
                put.status().as_u16(),
                200,
                "language {language:?} {case}: xml:lang {declared:?} names a supported \
                 language, so the document is stored, got {:?}",
                json(&put)
            );
            let got = json(&polly.get_lexicon(name).await);
            let reported = got["LexiconAttributes"]["LanguageCode"].as_str();
            assert_eq!(
                reported,
                Some(*language),
                "language {language:?} {case}: xml:lang {declared:?} must read back as \
                 the catalogued value, got {reported:?}"
            );
        }
    }
}

/// RULE (the documented widening): a bare primary subtag is a language a lexicon may
/// declare, and it too reads back in the catalog's spelling of that subtag rather than
/// the document's — checked over the primary subtag of every catalogued language.
#[tokio::test]
async fn every_primary_subtag_reads_back_in_the_catalog_spelling() {
    let polly = Polly::new();
    let name = "Primary";
    for language in LANGUAGE_CODE_VALUES {
        let primary = language
            .split('-')
            .next()
            .expect("a language tag has a primary subtag");
        for (case, declared) in case_forms(primary) {
            let content = document(&declared);
            let put = polly.put_lexicon(name, &content).await;
            assert_eq!(
                put.status().as_u16(),
                200,
                "subtag {primary:?} {case}: xml:lang {declared:?} names a supported \
                 language, so the document is stored, got {:?}",
                json(&put)
            );
            let got = json(&polly.get_lexicon(name).await);
            let reported = got["LexiconAttributes"]["LanguageCode"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_reportable_language(&reported),
                "subtag {primary:?} {case}: xml:lang {declared:?} must read back as a \
                 value the LanguageCode enum declares or a primary subtag of one, got \
                 {reported:?}"
            );
            // The catalog spells every primary subtag in lower case, so that is the
            // spelling this form reads back in whatever case the document used.
            assert_eq!(
                reported,
                primary.to_ascii_lowercase(),
                "subtag {primary:?} {case}: xml:lang {declared:?} must read back as the \
                 catalogued subtag, got {reported:?}"
            );
        }
    }
}

/// RULE (the refusal half): a tag naming no catalogued language is refused HTTP 400
/// `UnsupportedPlsLanguageException` with the sentence this service words that code
/// with, and nothing is stored for it.
#[tokio::test]
async fn a_language_outside_the_catalog_is_refused_and_stores_nothing() {
    let polly = Polly::new();
    let rows = [
        ("a primary subtag no language uses", "zz"),
        ("a tag no language uses", "xx-ZZ"),
        ("a catalogued language with a foreign region", "en-XX"),
        ("the subtags the other way round", "US-en"),
        ("an underscore instead of a hyphen", "en_US"),
        ("a three-letter code outside the catalog", "eng"),
        ("a language name rather than a tag", "klingon"),
    ];
    for (index, (case, declared)) in rows.into_iter().enumerate() {
        let name = format!("Unsupported{index:02}");
        let response = polly.put_lexicon(&name, &document(declared)).await;
        let body = json(&response);
        assert_eq!(
            (
                response.status().as_u16(),
                error_code(&response),
                body.get("message").and_then(|m| m.as_str()),
            ),
            (
                400,
                "UnsupportedPlsLanguageException".to_string(),
                Some(errors::UNSUPPORTED_PLS_LANGUAGE),
            ),
            "case {case:?} (xml:lang {declared:?}): expected the unsupported-language \
             refusal, got {body:?}"
        );

        let read_back = polly.get_lexicon(&name).await;
        assert_eq!(
            (read_back.status().as_u16(), error_code(&read_back)),
            (404, "LexiconNotFoundException".to_string()),
            "case {case:?} (xml:lang {declared:?}): a refused put stores nothing, got {:?}",
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

/// RULE (over both readers): `GetLexicon` and `ListLexicons` report one stored lexicon's
/// language identically, and both report only a value the enum the member targets can
/// carry — checked over a store built from documents spelling their language in
/// different cases and forms at once.
#[tokio::test]
async fn both_readers_report_the_catalogued_language() {
    let polly = Polly::new();
    // name, the xml:lang the document declares, the language it names
    let rows = [
        ("Mixed", "EN-us", "en-US"),
        ("Lower", "fr-fr", "fr-FR"),
        ("Upper", "JA-JP", "ja-JP"),
        ("Subtag", "EN", "en"),
        ("Three", "YUE-cn", "yue-CN"),
        ("Regionless", "ARB", "arb"),
        ("Extended", "en-gb-wls", "en-GB-WLS"),
        ("Canonical", "de-DE", "de-DE"),
    ];
    for (name, declared, _) in rows {
        let put = polly.put_lexicon(name, &document(declared)).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "lexicon {name:?} (xml:lang {declared:?}) must be stored, got {:?}",
            json(&put)
        );
    }

    let listed = json(&polly.list_lexicons("").await);
    let entries = listed["Lexicons"]
        .as_array()
        .expect("ListLexicons reports a list")
        .clone();
    assert_eq!(
        entries.len(),
        rows.len(),
        "every accepted document is stored once, got {listed:?}"
    );
    for (name, declared, expected) in rows {
        let entry = entries
            .iter()
            .find(|entry| entry["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("ListLexicons must report {name:?}: {listed:?}"));
        let listed_language = entry["Attributes"]["LanguageCode"].as_str();
        let got = json(&polly.get_lexicon(name).await);
        let got_language = got["LexiconAttributes"]["LanguageCode"].as_str();
        assert_eq!(
            (listed_language, got_language),
            (Some(expected), Some(expected)),
            "lexicon {name:?} (xml:lang {declared:?}): both readers must report \
             {expected:?}, got {listed_language:?} and {got_language:?}"
        );
        assert!(
            is_reportable_language(got_language.unwrap_or_default()),
            "lexicon {name:?} (xml:lang {declared:?}): {got_language:?} is not a value \
             the LanguageCode enum declares nor a primary subtag of one"
        );
    }
}

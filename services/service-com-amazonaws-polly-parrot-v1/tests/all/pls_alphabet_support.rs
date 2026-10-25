// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the phonetic alphabet a PLS lexicon declares.
//!
//! RULE: which modelled code a refusal carries is decided by what Amazon Polly's own
//! responses carry, not by the model merely declaring a code for the condition — so a
//! `PutLexicon` whose document declares an `alphabet` outside the supported set is
//! refused as an unusable PLS document AS A WHOLE: HTTP 400 `InvalidLexiconException`
//! with the one sentence this service words that code with, and nothing is stored.
//! `UnsupportedPlsAlphabetException` is declared by the model but is not on this
//! service's wire for any alphabet (the same way `EngineNotSupportedException` is
//! declared yet a voice/engine mismatch is a request-shape refusal), while a condition
//! Amazon Polly DOES answer specifically — an unsupported `xml:lang` — keeps its own
//! code.
//!
//! The tests check that over TABLES whose rows differ along the FORM of the value (an
//! unknown vendor extension, an unprefixed unknown name, a supported value without its
//! vendor prefix or with a suffix, a token from another vocabulary, a phrase) and over
//! EVERY supported alphabet rather than the one an issue happens to quote.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::SUPPORTED_PLS_ALPHABETS;
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, pls, Polly};

/// A PLS lexicon declaring `alphabet`, for a language every row shares.
fn lexicon_with_alphabet(alphabet: &str) -> String {
    pls(alphabet, "en-US", 2)
}

/// A PLS lexicon declaring both `alphabet` and `language`.
fn lexicon_with_alphabet_and_language(alphabet: &str, language: &str) -> String {
    pls(alphabet, language, 1)
}

/// The alphabets Amazon Polly supports, as its `UnsupportedPlsAlphabetException`
/// documentation lists them
/// (`smithy_query(query="shape",shape="UnsupportedPlsAlphabetException")`: "Valid
/// values are ipa, x-sampa, x-amazon-pinyin, x-amazon-jyutping, x-amazon-pron-kana,
/// and x-amazon-yomigana"). Pinned here so the service's own catalog cannot drift from
/// the contract unnoticed.
const DOCUMENTED_ALPHABETS: [&str; 6] = [
    "ipa",
    "x-sampa",
    "x-amazon-pinyin",
    "x-amazon-jyutping",
    "x-amazon-pron-kana",
    "x-amazon-yomigana",
];

/// The alphabet values no lexicon may declare, one row per FORM of a value outside the
/// supported set.
fn unsupported_alphabets() -> [(&'static str, &'static str); 7] {
    [
        // The value Amazon Polly's own recorded refusal was taken for.
        ("an unprefixed unknown name", "klingon"),
        ("an unknown vendor extension", "x-klingon"),
        ("another unknown vendor extension", "x-amazon-klingon"),
        // A supported value is supported as written; a near miss is not one of them.
        ("a supported value without its vendor prefix", "sampa"),
        ("a supported value with a suffix", "ipa-extended"),
        ("a supported value run together", "xamazonpinyin"),
        ("a phrase", "the international phonetic alphabet"),
    ]
}

/// The service's supported set is exactly the one the contract documents.
#[test]
fn the_supported_alphabets_are_the_documented_ones() {
    assert_eq!(
        SUPPORTED_PLS_ALPHABETS, &DOCUMENTED_ALPHABETS,
        "the supported alphabets must be the documented ones, got {SUPPORTED_PLS_ALPHABETS:?}"
    );
}

/// RULE (the refusal half): a document declaring an unsupported `alphabet` is HTTP 400
/// `InvalidLexiconException` — never the alphabet-specific code the model declares —
/// carries the fixed sentence for that code, and stores nothing.
#[tokio::test]
async fn an_unsupported_alphabet_is_refused_as_an_unusable_pls_document() {
    let polly = Polly::new();
    for (index, (case, alphabet)) in unsupported_alphabets().into_iter().enumerate() {
        let name = format!("BadAlphabet{index:02}");
        let content = lexicon_with_alphabet(alphabet);
        let response = polly.put_lexicon(&name, &content).await;
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
            "case {case:?} (alphabet {alphabet:?}): wrong refusal, got {body:?}"
        );
        // The sentence is a property of the code, so the request's own values are not
        // in it.
        let message = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(&name) && !message.contains(&content),
            "case {case:?}: message {message:?} echoes the request"
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

/// RULE (the acceptance half): every alphabet the contract names is stored and reads
/// back as the lexicon's `Alphabet`, and a document declaring no alphabet — which PLS
/// leaves optional — is stored with none.
#[tokio::test]
async fn every_supported_alphabet_is_stored_and_reads_back() {
    let polly = Polly::new();
    for (index, alphabet) in SUPPORTED_PLS_ALPHABETS.iter().enumerate() {
        let name = format!("Alphabet{index:02}");
        let content = lexicon_with_alphabet(alphabet);
        let put = polly.put_lexicon(&name, &content).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "alphabet {alphabet:?}: PutLexicon failed with {:?}",
            json(&put)
        );
        let read_back = json(&polly.get_lexicon(&name).await);
        assert_eq!(
            read_back["LexiconAttributes"]["Alphabet"].as_str(),
            Some(*alphabet),
            "alphabet {alphabet:?}: wrong Alphabet read back, got {read_back:?}"
        );
    }

    // PLS leaves `alphabet` off the document element when a `<phoneme>` names its own,
    // so a document without one is not a document with an unsupported one.
    let without = "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
                   version=\"1.0\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
                   <alias>World Wide Web Consortium</alias></lexeme></lexicon>";
    let put = polly.put_lexicon("NoAlphabet", without).await;
    assert_eq!(
        put.status().as_u16(),
        200,
        "a document declaring no alphabet must be stored, got {:?}",
        json(&put)
    );
    let read_back = json(&polly.get_lexicon("NoAlphabet").await);
    assert!(
        read_back["LexiconAttributes"]
            .get("Alphabet")
            .is_none_or(serde_json::Value::is_null),
        "a lexicon declaring no alphabet must report none, got {read_back:?}"
    );
}

/// RULE (which refusals are which): an unusable alphabet joins the documents Amazon
/// Polly's PLS reader cannot use at all under ONE code, whatever form the value takes
/// and whatever prefix the document spells the PLS vocabulary with, while the condition
/// Amazon Polly does answer specifically — an unsupported `xml:lang` — keeps its own.
#[tokio::test]
async fn the_unusable_document_codes_are_the_ones_amazon_polly_answers() {
    let polly = Polly::new();
    // case, document, the code it must carry
    let mut rows: Vec<(String, String, &str)> = vec![
        (
            "not XML at all".to_string(),
            "nonsense <".to_string(),
            "InvalidLexiconException",
        ),
        (
            "another vocabulary's document".to_string(),
            "<speak>hello</speak>".to_string(),
            "InvalidLexiconException",
        ),
        (
            "a lexicon declaring no version".to_string(),
            "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             alphabet=\"ipa\" xml:lang=\"en-US\"/>"
                .to_string(),
            "InvalidLexiconException",
        ),
        (
            "an empty lexicon with an unsupported alphabet".to_string(),
            "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             version=\"1.0\" alphabet=\"x-klingon\" xml:lang=\"en-US\"/>"
                .to_string(),
            "InvalidLexiconException",
        ),
        (
            // The alphabet is the document element's attribute whatever prefix the PLS
            // vocabulary is spelled with.
            "a prefixed lexicon with an unsupported alphabet".to_string(),
            "<pls:lexicon xmlns:pls=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
             version=\"1.0\" alphabet=\"x-klingon\" xml:lang=\"en-US\"><pls:lexeme>\
             <pls:grapheme>W3C</pls:grapheme><pls:alias>W3C</pls:alias></pls:lexeme>\
             </pls:lexicon>"
                .to_string(),
            "InvalidLexiconException",
        ),
        (
            "a supported alphabet in an unsupported language".to_string(),
            lexicon_with_alphabet_and_language("ipa", "xx-ZZ"),
            "UnsupportedPlsLanguageException",
        ),
        (
            "another supported alphabet in another unsupported language".to_string(),
            lexicon_with_alphabet_and_language("x-sampa", "klingon"),
            "UnsupportedPlsLanguageException",
        ),
    ];
    for (case, alphabet) in unsupported_alphabets() {
        rows.push((
            format!("a lexicon declaring {alphabet:?} ({case})"),
            lexicon_with_alphabet(alphabet),
            "InvalidLexiconException",
        ));
    }
    for (case, content, expected) in rows {
        let response = polly.put_lexicon("Refused", &content).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, expected.to_string()),
            "case {case:?}: wrong code, body {:?}",
            json(&response)
        );
    }
}

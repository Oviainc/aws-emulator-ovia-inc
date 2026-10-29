// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHERE a PLS document may declare a phonetic alphabet.
//!
//! RULE: the set of phonetic alphabets is the SERVICE's, so every `alphabet` a PLS
//! document declares — the document element's, naming the lexicon's alphabet, and any
//! a `<phoneme>` names for its own pronunciation — has to be one Amazon Polly
//! supports; a document declaring one outside that set is an unusable PLS document as
//! a whole (HTTP 400 `InvalidLexiconException`, nothing stored) whichever of the two
//! places declares it, and a document declaring only supported ones is stored
//! wherever it declares them.
//!
//! The tests check that over a TABLE whose rows differ along the DECLARATION SITE (the
//! document element, an open `<phoneme>`, an empty `<phoneme/>`, a `<phoneme>` spelling
//! the PLS vocabulary with a prefix, a later `<phoneme>` after one that declares
//! nothing, and both sites at once) crossed with the value's form, and over every
//! supported alphabet rather than the one an issue happens to quote.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    PLS_NAMESPACE, PLS_VERSION, SUPPORTED_PLS_ALPHABETS,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};

/// A document element declaring `alphabet`, over a lexeme whose `<phoneme>` declares
/// none.
fn on_the_document_element(alphabet: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         alphabet=\"{alphabet}\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
         <phoneme>w3c</phoneme></lexeme></lexicon>"
    )
}

/// A `<phoneme>` declaring its own alphabet, the document element declaring none —
/// which is what PLS leaves the document element's attribute optional for.
fn on_a_phoneme(alphabet: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
         <phoneme alphabet=\"{alphabet}\">w3c</phoneme></lexeme></lexicon>"
    )
}

/// An EMPTY `<phoneme/>` declaring one: an element carrying no content still declares
/// its attributes.
fn on_an_empty_phoneme(alphabet: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
         <phoneme alphabet=\"{alphabet}\" ph=\"w3c\"/></lexeme></lexicon>"
    )
}

/// The same declaration with the PLS vocabulary spelled on a prefix: the attribute is
/// the element's whatever prefix binds the namespace.
fn on_a_prefixed_phoneme(alphabet: &str) -> String {
    format!(
        "<pls:lexicon xmlns:pls=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><pls:lexeme><pls:grapheme>W3C</pls:grapheme>\
         <pls:phoneme alphabet=\"{alphabet}\">w3c</pls:phoneme></pls:lexeme>\
         </pls:lexicon>"
    )
}

/// A declaration on the SECOND of two lexemes' phonemes, after one that declares
/// nothing: a declaration counts wherever in the document it sits.
fn on_a_later_phoneme(alphabet: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme><phoneme>w3c</phoneme>\
         </lexeme><lexeme><grapheme>PLS</grapheme>\
         <phoneme alphabet=\"{alphabet}\">pls</phoneme></lexeme></lexicon>"
    )
}

/// Both sites at once, the `<phoneme>` restating the lexicon's alphabet.
fn on_both_sites(alphabet: &str) -> String {
    format!(
        "<lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
         alphabet=\"{alphabet}\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
         <phoneme alphabet=\"{alphabet}\">w3c</phoneme></lexeme></lexicon>"
    )
}

/// One place a PLS document may declare an alphabet: the case name, the document
/// builder for a value, and whether the DOCUMENT ELEMENT is the declaring site.
type DeclarationSite = (&'static str, fn(&str) -> String, bool);

/// Every place PLS admits an `alphabet` declaration, as the service's own PLS reader
/// documents them (`put_lexicon::parse_pls`: the document element names the lexicon's
/// alphabet and a `<phoneme>` may name its own): the case name, the document builder,
/// and whether the DOCUMENT ELEMENT is the one declaring it — which is what
/// `LexiconAttributes$Alphabet` reports.
fn declaration_sites() -> [DeclarationSite; 6] {
    [
        ("on the document element", on_the_document_element, true),
        ("on a <phoneme>", on_a_phoneme, false),
        ("on an empty <phoneme/>", on_an_empty_phoneme, false),
        ("on a prefixed <pls:phoneme>", on_a_prefixed_phoneme, false),
        ("on a later <phoneme>", on_a_later_phoneme, false),
        ("on both sites", on_both_sites, true),
    ]
}

/// The alphabet values no declaration may name, one row per FORM of a value outside
/// the supported set.
fn unsupported_alphabets() -> [(&'static str, &'static str); 6] {
    [
        ("an unprefixed unknown name", "klingon"),
        ("an unknown vendor extension", "x-klingon"),
        ("another unknown vendor extension", "x-amazon-klingon"),
        // A supported value is supported as written; a near miss is not one of them.
        ("a supported value without its vendor prefix", "sampa"),
        ("a supported value with a suffix", "ipa-extended"),
        ("a phrase", "the international phonetic alphabet"),
    ]
}

/// RULE (the refusal half): an alphabet outside the supported set refuses the document
/// as an unusable PLS document — HTTP 400 `InvalidLexiconException` with the one
/// sentence this service words that code with — and stores nothing, whichever of the
/// places PLS admits the attribute declares it.
#[tokio::test]
async fn an_unsupported_alphabet_is_refused_wherever_the_document_declares_it() {
    let polly = Polly::new();
    for (site_index, (site, build, _)) in declaration_sites().into_iter().enumerate() {
        for (value_index, (case, alphabet)) in unsupported_alphabets().into_iter().enumerate() {
            let name = format!("Bad{site_index}{value_index:02}");
            let content = build(alphabet);
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
                "case {site:?} / {case:?} (alphabet {alphabet:?}): wrong refusal for \
                 {content:?}, got {body:?}"
            );

            let read_back = polly.get_lexicon(&name).await;
            assert_eq!(
                (read_back.status().as_u16(), error_code(&read_back)),
                (404, "LexiconNotFoundException".to_string()),
                "case {site:?} / {case:?}: a refused put must store nothing, got {:?}",
                json(&read_back)
            );
        }
    }
    let listed = json(&polly.list_lexicons("").await);
    assert_eq!(
        listed["Lexicons"].as_array().map(Vec::len),
        Some(0),
        "no refused document may appear in the store, got {listed:?}"
    );
}

/// RULE (the acceptance half): every alphabet the contract names is usable at every
/// place PLS admits the declaration, and what `LexiconAttributes$Alphabet` reports is
/// the LEXICON's alphabet — the document element's declaration — so a document that
/// declares one only on a `<phoneme>` is stored and reports none.
#[tokio::test]
async fn every_supported_alphabet_is_accepted_wherever_the_document_declares_it() {
    let polly = Polly::new();
    for (site_index, (site, build, on_document_element)) in
        declaration_sites().into_iter().enumerate()
    {
        for (value_index, alphabet) in SUPPORTED_PLS_ALPHABETS.iter().enumerate() {
            let name = format!("Good{site_index}{value_index:02}");
            let content = build(alphabet);
            let put = polly.put_lexicon(&name, &content).await;
            assert_eq!(
                put.status().as_u16(),
                200,
                "case {site:?} (alphabet {alphabet:?}): PutLexicon must store \
                 {content:?}, got {:?}",
                json(&put)
            );
            let read_back = json(&polly.get_lexicon(&name).await);
            let reported = read_back["LexiconAttributes"]
                .get("Alphabet")
                .and_then(serde_json::Value::as_str);
            assert_eq!(
                reported,
                on_document_element.then_some(*alphabet),
                "case {site:?} (alphabet {alphabet:?}): wrong reported Alphabet, got \
                 {read_back:?}"
            );
            assert_eq!(
                read_back["LexiconAttributes"]["LanguageCode"].as_str(),
                Some("en-US"),
                "case {site:?} (alphabet {alphabet:?}): the stored lexicon must keep \
                 its language, got {read_back:?}"
            );
        }
    }
}

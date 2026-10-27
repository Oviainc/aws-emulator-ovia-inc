// SPDX-License-Identifier: CC0-1.0

//! The shared PLS fixture, driven through the path the suites that share it drive.
//!
//! `crate::support::pls_with_replacement` is the ONE definition of the PLS document the
//! refusal suites measure against the lexeme quota: `error_semantics` (the status and
//! the error code each refusal carries) and `error_document_message` (the `message`
//! member the refusal document carries) both build it there, so neither can drift onto
//! a different document while both keep passing.
//!
//! This suite drives that one definition end to end over a TABLE whose rows differ
//! along the replacement element, the length of the replacement and which quota the
//! document lands past, and checks BOTH of the paths the two suites own on every row:
//! the status and error code one of them asserts, and the `message` member the other
//! reads. It also pins what the shared document itself declares, so a change to it —
//! its alphabet, its language, how many lexemes it carries, how the replacement nests —
//! is visible here rather than silently splitting the two suites.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{
    MAX_LEXEME_CHARACTERS, MAX_LEXICON_CHARACTERS, PLS_NAMESPACE, PLS_VERSION,
};
use service_com_amazonaws_polly_parrot_v1::service::errors;

use crate::lexicons_and_speech::{error_code, json, Polly};
use crate::support::{message_slot, pls_with_replacement};

/// The phonetic alphabet the shared document declares.
const FIXTURE_ALPHABET: &str = "ipa";

/// The language the shared document applies to.
const FIXTURE_LANGUAGE: &str = "en-US";

/// How many `<lexeme>`s the shared document carries.
const FIXTURE_LEXEMES: u64 = 1;

/// The character content of `document`'s LAST `<element>` ... `</element>` pair — the
/// one the fixture's length was asked for, since the shared document always opens its
/// lexeme with a fixed `<grapheme>` head that a measured `<grapheme>` then follows.
fn replacement(document: &str, element: &str) -> String {
    let open = format!("<{element}>");
    let close = format!("</{element}>");
    let start = document
        .rfind(&open)
        .unwrap_or_else(|| panic!("the shared document carries no {open}: {document:?}"))
        + open.len();
    let rest = &document[start..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("the shared document carries no {close}: {document:?}"));
    rest[..end].to_string()
}

/// RULE: the shared fixture builds one PLS document — the namespace and version PLS
/// requires of its document element, one phonetic alphabet, one language, and exactly
/// one `<lexeme>` whose named replacement element carries the requested number of
/// characters — whatever element and length a caller asks it for.
#[test]
fn the_shared_fixture_builds_the_one_document_both_suites_mean() {
    for element in ["alias", "phoneme", "grapheme"] {
        for length in [0usize, 1, 4, MAX_LEXEME_CHARACTERS, MAX_LEXEME_CHARACTERS + 1, 150] {
            let case = format!("<{element}> of {length} characters");
            let document = pls_with_replacement(element, length);
            for declaration in [
                format!("xmlns=\"{PLS_NAMESPACE}\""),
                format!("version=\"{PLS_VERSION}\""),
                format!("alphabet=\"{FIXTURE_ALPHABET}\""),
                format!("xml:lang=\"{FIXTURE_LANGUAGE}\""),
            ] {
                assert!(
                    document.contains(&declaration),
                    "case {case:?}: the shared document must declare {declaration}, \
                     got {document:?}"
                );
            }
            let lexemes = document.matches("<lexeme>").count() as u64;
            assert_eq!(
                lexemes, FIXTURE_LEXEMES,
                "case {case:?}: expected {FIXTURE_LEXEMES} lexeme(s), got {lexemes} in \
                 {document:?}"
            );
            let actual = replacement(&document, element).chars().count();
            assert_eq!(
                actual, length,
                "case {case:?}: the <{element}> content must be {length} characters, \
                 got {actual}"
            );
        }
    }
}

/// RULE: the document the shared fixture builds is refused with HTTP 400
/// `LexiconSizeExceededException` when the whole document passes the per-lexicon size
/// quota, otherwise with HTTP 400 `MaxLexemeLengthExceededException` when a
/// REPLACEMENT element's own content passes the per-lexeme quota (a `<grapheme>` is no
/// replacement), and is stored and read back verbatim with the attributes its own
/// document element declares otherwise — and every refusal carries the fixed message
/// for its code in the error document's `message` member.
#[tokio::test]
async fn the_shared_fixture_drives_both_quota_paths_end_to_end() {
    let polly = Polly::new();
    // The rows span the class the shared document covers: both PLS replacement
    // elements and the `<grapheme>` that is not one, below, at, one past and far past
    // the per-lexeme quota, and documents past the per-lexicon size quota.
    let rows = [
        ("alias", 1usize),
        ("phoneme", 1),
        ("grapheme", 1),
        ("alias", MAX_LEXEME_CHARACTERS),
        ("phoneme", MAX_LEXEME_CHARACTERS),
        ("grapheme", MAX_LEXEME_CHARACTERS * 3),
        ("alias", MAX_LEXEME_CHARACTERS + 1),
        ("phoneme", MAX_LEXEME_CHARACTERS + 1),
        ("alias", 150),
        ("grapheme", MAX_LEXICON_CHARACTERS + 1),
        ("phoneme", MAX_LEXICON_CHARACTERS * 10),
    ];
    for (index, (element, length)) in rows.into_iter().enumerate() {
        let case = format!("<{element}> of {length} characters");
        let name = format!("Shared{index:02}");
        let content = pls_with_replacement(element, length);
        let characters = content.chars().count();
        // The expectation is derived from the rule, in the order the service applies
        // it, rather than written per row.
        let is_replacement = matches!(element, "alias" | "phoneme");
        let expected: Option<(&str, &str)> = if characters > MAX_LEXICON_CHARACTERS {
            Some((
                "LexiconSizeExceededException",
                errors::LEXICON_SIZE_EXCEEDED,
            ))
        } else if is_replacement && length > MAX_LEXEME_CHARACTERS {
            Some((
                "MaxLexemeLengthExceededException",
                errors::MAX_LEXEME_LENGTH_EXCEEDED,
            ))
        } else {
            None
        };

        let response = polly.put_lexicon(&name, &content).await;
        let read_back = polly.get_lexicon(&name).await;
        match expected {
            Some((code, message)) => {
                // The path `error_semantics` owns: the status and the error code.
                assert_eq!(
                    (response.status().as_u16(), error_code(&response)),
                    (400, code.to_string()),
                    "case {case:?}: expected the {code} refusal, got {:?}",
                    json(&response)
                );
                // The path `error_document_message` owns: the member is PRESENT and
                // carries the fixed sentence for the code.
                let slot = message_slot(&response);
                assert_eq!(
                    slot.as_ref().and_then(|value| value.as_str()),
                    Some(message),
                    "case {case:?}: expected the message {message:?} on the error \
                     document, got {slot:?}"
                );
                assert_eq!(
                    (read_back.status().as_u16(), error_code(&read_back)),
                    (404, "LexiconNotFoundException".to_string()),
                    "case {case:?}: a refused put must store nothing, got {:?}",
                    json(&read_back)
                );
            }
            None => {
                assert_eq!(
                    response.status().as_u16(),
                    200,
                    "case {case:?}: expected the document to be stored, got {:?}",
                    json(&response)
                );
                assert_eq!(
                    read_back.status().as_u16(),
                    200,
                    "case {case:?}: the stored document must read back, got {:?}",
                    json(&read_back)
                );
                let got = json(&read_back);
                // `LexiconAttributes` members, from the model
                // (`smithy_query(query="shape",shape="LexiconAttributes")`): the
                // attributes are read from the document that was put.
                let actual = (
                    got["Lexicon"]["Content"].as_str(),
                    got["Lexicon"]["Name"].as_str(),
                    got["LexiconAttributes"]["Alphabet"].as_str(),
                    got["LexiconAttributes"]["LanguageCode"].as_str(),
                    got["LexiconAttributes"]["LexemesCount"].as_u64(),
                    got["LexiconAttributes"]["Size"].as_u64(),
                );
                assert_eq!(
                    actual,
                    (
                        Some(content.as_str()),
                        Some(name.as_str()),
                        Some(FIXTURE_ALPHABET),
                        Some(FIXTURE_LANGUAGE),
                        Some(FIXTURE_LEXEMES),
                        Some(characters as u64),
                    ),
                    "case {case:?}: the shared document must read back as it was put, \
                     got {got:?}"
                );
            }
        }
    }
}

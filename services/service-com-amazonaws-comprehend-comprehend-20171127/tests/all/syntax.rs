// SPDX-License-Identifier: CC0-1.0

//! Comprehend's `DetectSyntax`.
//!
//! The operation is a pure function of the submitted document: it reports one token
//! per word-boundary unit of the text, in document order, each carrying its 1-based
//! `TokenId`, the token's own `Text`, a half-open range of zero-based character
//! offsets, and a `PartOfSpeech` tag drawn from the modelled `PartOfSpeechTagType`
//! set with a confidence in (0, 1].
//!
//! Every suite asserts that RULE over a table of documents rather than one recorded
//! request: the rows differ along what the rule generalises over — the word classes
//! present, token count, figures and symbols, contractions, multi-byte characters,
//! sentence count and whitespace form.

use serde_json::json;
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    PART_OF_SPEECH_TAG_TYPE_VALUES, SYNTAX_LANGUAGE_CODE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// One row of [`TAGGED_DOCUMENTS`]: the case name, the document, and the `(Text,
/// Tag)` of every token it must come back as.
type TaggedDocument = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// Documents paired with the `(Text, Tag)` of every token they must come back as.
///
/// The tag set is the model's own (`PART_OF_SPEECH_TAG_TYPE_VALUES`, asserted below);
/// each tag names the Universal Dependencies v2 word class of that name
/// (<https://universaldependencies.org/u/pos/>), which is the set Comprehend reports.
/// The rows differ along the classes they carry and along token count, figures,
/// symbols, contractions, multi-byte characters and sentence count.
const TAGGED_DOCUMENTS: &[TaggedDocument] = &[
    (
        "simple declarative",
        "The dog barks loudly.",
        &[
            ("The", "DET"),
            ("dog", "NOUN"),
            ("barks", "VERB"),
            ("loudly", "ADV"),
            (".", "PUNCT"),
        ],
    ),
    (
        "copula, adverb and a proper noun",
        "It is raining today in Seattle.",
        &[
            ("It", "PRON"),
            ("is", "AUX"),
            ("raining", "VERB"),
            ("today", "ADV"),
            ("in", "ADP"),
            ("Seattle", "PROPN"),
            (".", "PUNCT"),
        ],
    ),
    (
        "figures and a currency symbol",
        "She paid $25.50 for 3 books.",
        &[
            ("She", "PRON"),
            ("paid", "VERB"),
            ("$", "SYM"),
            ("25.50", "NUM"),
            ("for", "ADP"),
            ("3", "NUM"),
            ("books", "NOUN"),
            (".", "PUNCT"),
        ],
    ),
    (
        "contraction and adjectives",
        "I don't like cold coffee.",
        &[
            ("I", "PRON"),
            ("don't", "AUX"),
            ("like", "VERB"),
            ("cold", "ADJ"),
            ("coffee", "NOUN"),
            (".", "PUNCT"),
        ],
    ),
    (
        "interjection and a question",
        "Wow, who ate the quick brown fox?",
        &[
            ("Wow", "INTJ"),
            (",", "PUNCT"),
            ("who", "PRON"),
            ("ate", "VERB"),
            ("the", "DET"),
            ("quick", "ADJ"),
            ("brown", "ADJ"),
            ("fox", "NOUN"),
            ("?", "PUNCT"),
        ],
    ),
    (
        "conjunctions",
        "He waited and read a book because the train was late.",
        &[
            ("He", "PRON"),
            ("waited", "VERB"),
            ("and", "CCONJ"),
            ("read", "VERB"),
            ("a", "DET"),
            ("book", "NOUN"),
            ("because", "SCONJ"),
            ("the", "DET"),
            // "train" is also a verb, but an article opens a noun phrase.
            ("train", "NOUN"),
            ("was", "AUX"),
            ("late", "ADJ"),
            (".", "PUNCT"),
        ],
    ),
    (
        "multi-byte characters",
        "He visited a café in Paris — twice.",
        &[
            ("He", "PRON"),
            ("visited", "VERB"),
            ("a", "DET"),
            ("café", "NOUN"),
            ("in", "ADP"),
            ("Paris", "PROPN"),
            ("—", "PUNCT"),
            ("twice", "ADV"),
            (".", "PUNCT"),
        ],
    ),
    (
        "two sentences",
        "Dogs bark. Cats sleep.",
        &[
            ("Dogs", "NOUN"),
            ("bark", "VERB"),
            (".", "PUNCT"),
            ("Cats", "NOUN"),
            ("sleep", "VERB"),
            (".", "PUNCT"),
        ],
    ),
    (
        "non-linguistic identifier",
        "The column user_id holds it.",
        &[
            ("The", "DET"),
            ("column", "NOUN"),
            ("user_id", "O"),
            ("holds", "VERB"),
            ("it", "PRON"),
            (".", "PUNCT"),
        ],
    ),
    (
        "one word",
        "Hello",
        &[("Hello", "INTJ")],
    ),
    (
        "an article demotes the verb reading of the word it opens",
        "I watch the play and the running water.",
        &[
            ("I", "PRON"),
            ("watch", "VERB"),
            ("the", "DET"),
            ("play", "NOUN"),
            ("and", "CCONJ"),
            ("the", "DET"),
            ("running", "NOUN"),
            ("water", "NOUN"),
            (".", "PUNCT"),
        ],
    ),
];

/// `text`'s characters in `[begin, end)` — the offsets `DetectSyntax` reports are
/// zero-based CHARACTER positions, not byte positions.
fn slice_characters(text: &str, begin: usize, end: usize) -> String {
    text.chars().skip(begin).take(end.saturating_sub(begin)).collect()
}

/// Every token of `output`, asserted structurally, as `(text, tag, score)`.
fn structural_tokens(case: &str, text: &str, output: &serde_json::Value) -> Vec<(String, String, f64)> {
    let tokens = output["SyntaxTokens"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SyntaxTokens array, got {output}"))
        .clone();

    let mut collected = Vec::new();
    let mut previous_end = 0i64;
    for (index, token) in tokens.iter().enumerate() {
        let token_id = token["TokenId"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a TokenId on token {index}, got {token}")
        });
        assert_eq!(
            token_id,
            index as i64 + 1,
            "case {case:?}: TokenId must be the token's 1-based position, got {token_id} \
             at position {index} in {token}"
        );

        let token_text = token["Text"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a Text on token {index}, got {token}")
        });
        let begin = token["BeginOffset"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a BeginOffset on token {index}, got {token}")
        });
        let end = token["EndOffset"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected an EndOffset on token {index}, got {token}")
        });

        assert!(
            begin >= previous_end,
            "case {case:?}: tokens must not overlap, got BeginOffset {begin} after \
             EndOffset {previous_end} in {token}"
        );
        assert!(
            end > begin,
            "case {case:?}: expected EndOffset after BeginOffset, got {begin}..{end} in {token}"
        );
        previous_end = end;

        let sliced = slice_characters(text, begin as usize, end as usize);
        assert_eq!(
            sliced, token_text,
            "case {case:?}: the document sliced by {begin}..{end} must be the token's \
             Text, got {sliced:?} for {token_text:?}"
        );

        let tag = token["PartOfSpeech"]["Tag"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a PartOfSpeech.Tag on token {index}, got {token}")
        });
        assert!(
            PART_OF_SPEECH_TAG_TYPE_VALUES.contains(&tag),
            "case {case:?}: {tag:?} is not a modelled PartOfSpeechTagType value"
        );
        let score = token["PartOfSpeech"]["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a PartOfSpeech.Score in (0, 1], got {score} in {token}"
        );

        collected.push((token_text.to_string(), tag.to_string(), score));
    }
    collected
}

#[tokio::test]
async fn detect_syntax_labels_every_word_with_its_part_of_speech() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in TAGGED_DOCUMENTS {
        let output = comprehend
            .ok(
                "DetectSyntax",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let tokens = structural_tokens(case, text, &output);

        let actual: Vec<(&str, &str)> = tokens
            .iter()
            .map(|(token_text, tag, _)| (token_text.as_str(), tag.as_str()))
            .collect();
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected the tokens {expected:?}, got {actual:?}"
        );
    }
}

/// Documents with no expected tag list: the invariants hold for any text, whatever
/// the tagger makes of the words. The rows differ along size, script, whitespace
/// form and the presence of word characters at all.
const ANY_DOCUMENT: &[(&str, &str)] = &[
    ("single letter", "a"),
    ("single punctuation mark", "!"),
    ("leading and trailing whitespace", "   spaced out   "),
    ("tabs and newlines", "first line\n\tsecond line\r\nthird"),
    ("repeated punctuation", "Really?!?! No way..."),
    ("no letters at all", "123 456,789 -- ***"),
    ("emoji among words", "I ❤ my dog 🐕 very much"),
    ("accented script", "Él envió un camión al jardín"),
    ("long document", "The quick brown fox jumps over the lazy dog. "),
];

#[tokio::test]
async fn offsets_and_ids_round_trip_the_document_for_any_text() {
    let comprehend = Comprehend::new();

    for (case, document) in ANY_DOCUMENT {
        // The long row is repeated until it is a few hundred tokens, so the
        // invariants are asserted at size too, not only on a short sentence.
        let text = if *case == "long document" {
            document.repeat(40)
        } else {
            (*document).to_string()
        };

        let output = comprehend
            .ok(
                "DetectSyntax",
                &json!({ "Text": &text, "LanguageCode": "en" }),
            )
            .await;
        let tokens = structural_tokens(case, &text, &output);

        // Every non-whitespace character of the document belongs to exactly one
        // token, in order: concatenating the tokens drops only the whitespace.
        let rejoined: String = tokens.iter().map(|(token_text, _, _)| token_text.as_str()).collect();
        let without_whitespace: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(
            rejoined, without_whitespace,
            "case {case:?}: the tokens must cover every non-whitespace character of \
             the document exactly once, got {rejoined:?} for {without_whitespace:?}"
        );
    }
}

/// A word only one word class claims is reported with a higher confidence than a
/// word several classes claim, whose tag is the most frequent of its readings.
#[tokio::test]
async fn an_ambiguous_word_is_reported_with_a_lower_confidence_than_an_unambiguous_one() {
    let comprehend = Comprehend::new();
    let text = "The dog that barks is mine.";

    let output = comprehend
        .ok(
            "DetectSyntax",
            &json!({ "Text": text, "LanguageCode": "en" }),
        )
        .await;
    let tokens = structural_tokens("ambiguity", text, &output);

    let score_of = |word: &str| -> f64 {
        tokens
            .iter()
            .find(|(token_text, _, _)| token_text.eq_ignore_ascii_case(word))
            .map(|(_, _, score)| *score)
            .unwrap_or_else(|| panic!("case \"ambiguity\": no token {word:?} in {tokens:?}"))
    };

    // "the" is a determiner and nothing else; "that" is also a pronoun and a
    // subordinating conjunction.
    let unambiguous = score_of("The");
    let ambiguous = score_of("that");
    assert!(
        ambiguous < unambiguous,
        "case \"ambiguity\": expected \"that\" to score below \"the\", got \
         {ambiguous} and {unambiguous}"
    );
}

#[tokio::test]
async fn detect_syntax_is_deterministic() {
    let comprehend = Comprehend::new();
    let body = json!({ "Text": TAGGED_DOCUMENTS[1].1, "LanguageCode": "en" });

    let first = comprehend.ok("DetectSyntax", &body).await;
    let second = comprehend.ok("DetectSyntax", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical tokens, got {first} then {second}"
    );
}

#[tokio::test]
async fn detect_syntax_refuses_a_language_the_tagger_cannot_read() {
    let comprehend = Comprehend::new();

    // Every value of the modelled `SyntaxLanguageCode` enum, so the row the engine
    // reads and every row it refuses are both covered.
    for code in SYNTAX_LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "DetectSyntax",
                &json!({ "Text": "The dog barks loudly.", "LanguageCode": code }),
            )
            .await;

        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: the tagger reads English, expected 200, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguageException",
                "case {code:?}: expected UnsupportedLanguageException, got {body}"
            );
        }
    }
}

// The per-document byte quota is the one every real-time inference operation
// shares, so it is asserted in `tests/all/realtime_text_quotas.rs` (the
// `DetectSyntax` row of its `QUOTAS` table) rather than a second time here.

#[tokio::test]
async fn a_language_code_outside_the_modelled_enum_is_refused() {
    let comprehend = Comprehend::new();

    for code in ["xx", "EN", "english", "zh", ""] {
        let (status, body) = comprehend
            .call(
                "DetectSyntax",
                &json!({ "Text": "The dog barks loudly.", "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the SyntaxLanguageCode \
             enum, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn an_absent_or_empty_text_is_refused_by_the_modelled_length_floor() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("absent Text", json!({ "LanguageCode": "en" })),
    ] {
        let (status, response) = comprehend.call("DetectSyntax", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What `BatchDetectKeyPhrases` reports for a document, as a CONVENTION rather than
//! as one recorded answer.
//!
//! The rule: the operation "detects the key noun phrases found in the text" and
//! reports, for each one, "the text of the key phrase, where the key phrase begins and
//! ends" and the confidence of the detection. So a reported phrase is a whole NOUN
//! PHRASE of the document — its determiner and modifiers through its head noun — whose
//! `BeginOffset` is the first character of that phrase and whose `EndOffset` is one
//! past its last, and whose `Score` is the confidence of the analysis that found it:
//! a phrase every word of which the analyser reads unambiguously comes back near
//! certainty, one resting on a reading the context had to resolve comes back below it,
//! and neither comes back as a rank over the document's other phrases.
//!
//! The table spans what a noun phrase can be built from — a determiner and a noun, a
//! determiner with modifiers, a compound, a proper noun, a possessive determiner, a
//! phrase repeated in one document — and what is NOT one: a predicate's adjective, a
//! clause's verb, a bare figure, punctuation alone.
//!
//! `key_phrases.rs` holds the rules every language shares (a phrase is a span of its
//! own document, the spans walk it forwards, one `ResultList` entry per document);
//! `batch_inference_calibration.rs` holds the other three analysers of the family and
//! the batch helpers this suite reads.

use serde_json::json;

use crate::batch_inference_calibration::batch_results;
use crate::comprehend_client::Comprehend;

/// The zero-based CHARACTER offset of the `occurrence`-th appearance of `needle` in
/// `document` — the offset a span naming that text has to report.
fn character_offset(document: &str, needle: &str, occurrence: usize) -> usize {
    let byte = document
        .match_indices(needle)
        .nth(occurrence)
        .map(|(byte, _)| byte)
        .unwrap_or_else(|| {
            panic!(
                "the test document {document:?} does not carry {needle:?} \
                 {occurrence} times"
            )
        });
    document[..byte].chars().count()
}

/// `(case, document, the key phrases it holds, in document order)`.
const KEY_PHRASE_DOCUMENTS: &[(&str, &str, &[&str])] = &[
    (
        "a determiner and a noun, twice",
        "The flight was delayed and the staff were rude.",
        &["The flight", "the staff"],
    ),
    (
        "a determiner with two modifiers",
        "A long red carpet covered the floor.",
        &["A long red carpet", "the floor"],
    ),
    (
        "a proper noun and a compound",
        "Seattle has excellent coffee shops.",
        &["Seattle", "excellent coffee shops"],
    ),
    (
        "a gerund inside a compound",
        "The language processing service reads plain text.",
        &["The language processing service", "plain text"],
    ),
    (
        "a possessive determiner",
        "My laptop screen is broken.",
        &["My laptop screen"],
    ),
    (
        "a passive with an agent",
        "The window was broken by a storm.",
        &["The window", "a storm"],
    ),
    (
        "a repeated phrase, once per occurrence",
        "The battery life is poor. The battery life worries me.",
        &["The battery life", "The battery life"],
    ),
    ("a bare figure carries no noun phrase", "1234567890", &[]),
    ("punctuation alone", "!!! ... ???", &[]),
];

#[tokio::test]
async fn every_key_phrase_is_a_whole_noun_phrase_of_its_document() {
    let comprehend = Comprehend::new();
    let text_list: Vec<&str> = KEY_PHRASE_DOCUMENTS
        .iter()
        .map(|(_, document, _)| *document)
        .collect();
    // The whole table in ONE batch, so each row is answered as a `ResultList` entry
    // beside the others, as the recorded request's documents were.
    let results = batch_results(
        &comprehend,
        "BatchDetectKeyPhrases",
        &json!({ "TextList": text_list, "LanguageCode": "en" }),
    )
    .await;

    let mut scores: Vec<(String, f64)> = Vec::new();
    for (position, (case, document, expected)) in KEY_PHRASE_DOCUMENTS.iter().enumerate() {
        let phrases = results[position]["KeyPhrases"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let reported: Vec<&str> = phrases
            .iter()
            .map(|phrase| phrase["Text"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected the document's noun phrases {expected:?}, got \
             {reported:?} in {phrases:?}"
        );

        let mut occurrences: Vec<&str> = Vec::new();
        for phrase in &phrases {
            let text = phrase["Text"].as_str().unwrap_or_default();
            let begin = phrase["BeginOffset"].as_u64().unwrap_or_default() as usize;
            let end = phrase["EndOffset"].as_u64().unwrap_or_default() as usize;
            // The span covers the WHOLE phrase, starting at its first character — the
            // determiner included, which is what a client highlighting the span in
            // its own copy of the document marks.
            let occurrence = occurrences.iter().filter(|seen| **seen == text).count();
            occurrences.push(text);
            assert_eq!(
                begin,
                character_offset(document, text, occurrence),
                "case {case:?}: expected BeginOffset at the first character of \
                 {text:?}, got {phrase}"
            );
            let sliced: String = document
                .chars()
                .skip(begin)
                .take(end.saturating_sub(begin))
                .collect();
            assert_eq!(
                sliced, text,
                "case {case:?}: the document sliced by {begin}..{end} must be the \
                 phrase's own Text, got {sliced:?} for {text:?}"
            );

            let score = phrase["Score"].as_f64().unwrap_or(f64::NAN);
            assert!(
                score > 0.0 && score < 1.0,
                "case {case:?}: a detection is a confidence, never a certainty, so \
                 expected a Score strictly inside (0, 1), got {score} in {phrase}"
            );
            scores.push((text.to_string(), score));
        }
    }

    // The confidence follows the ANALYSIS: a phrase every word of which the analyser
    // reads unambiguously comes back near certainty, and one resting on a reading the
    // context had to resolve comes back measurably below it.
    let score_of = |needle: &str| -> f64 {
        scores
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, score)| *score)
            .unwrap_or_else(|| panic!("no phrase {needle:?} among {scores:?}"))
    };
    let unambiguous = score_of("The flight");
    let resolved = score_of("The language processing service");
    assert!(
        unambiguous > 0.99,
        "expected a phrase of unambiguous words to come back near certainty, got \
         {unambiguous}"
    );
    assert!(
        resolved < unambiguous - 0.01,
        "expected a phrase resting on a resolved reading to score below one of \
         unambiguous words, got {resolved} against {unambiguous}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! A document written from scratch for the batch analysers, and a dump of each
//! analyser's FULL answer for it, pinned as a baseline.
//!
//! `batch_inference_calibration.rs` and `noun_phrase_key_phrases.rs` assert the rule
//! each analyser follows, one rule at a time; this one pins what those rules add up to
//! on ONE document read by all four, so a change to one rule that quietly changes
//! another analyser's answer — a tag that moves a noun-phrase boundary, a phrase that
//! swallows its clause's verb, a polarity that flips a verdict — fails here rather
//! than silently. It is the key-phrase and syntax counterpart of `scratch_dump.rs`,
//! which pins the entity recogniser's whole answer.
//!
//! Only the DISCRETE parts of each answer are pinned: the tags, the phrases and their
//! spans, the verdict, the languages detected. The confidences beside them follow the
//! analysers' evidence and are asserted as the rules they obey, in the two suites
//! above, rather than pinned as literals here.

use serde_json::json;

use crate::batch_inference_calibration::batch_results;
use crate::comprehend_client::Comprehend;

/// One document carrying two clauses, each a noun phrase with a determiner, a copula
/// standing as its clause's main verb, a participle and an adjective in predicative
/// position, and plainly one language.
const DOCUMENT: &str = "The flight was delayed and the staff were rude.";

/// Every syntax token of [`DOCUMENT`], as `(Text, Tag)`.
const TOKENS: &[(&str, &str)] = &[
    ("The", "DET"),
    ("flight", "NOUN"),
    ("was", "VERB"),
    ("delayed", "ADJ"),
    ("and", "CCONJ"),
    ("the", "DET"),
    ("staff", "NOUN"),
    ("were", "VERB"),
    ("rude", "ADJ"),
    (".", "PUNCT"),
];

/// Every key phrase of [`DOCUMENT`], as `(Text, BeginOffset, EndOffset)`.
const PHRASES: &[(&str, u64, u64)] = &[("The flight", 0, 10), ("the staff", 27, 36)];

#[tokio::test]
async fn the_whole_answer_for_one_document_is_pinned_across_the_four_analysers() {
    let comprehend = Comprehend::new();

    let syntax = batch_results(
        &comprehend,
        "BatchDetectSyntax",
        &json!({ "TextList": [DOCUMENT], "LanguageCode": "en" }),
    )
    .await;
    let tokens: Vec<(String, String)> = syntax[0]["SyntaxTokens"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|token| {
            (
                token["Text"].as_str().unwrap_or_default().to_string(),
                token["PartOfSpeech"]["Tag"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = TOKENS
        .iter()
        .map(|(text, tag)| ((*text).to_string(), (*tag).to_string()))
        .collect();
    assert_eq!(
        tokens, expected,
        "case \"syntax baseline\": expected {expected:?}, got {tokens:?}"
    );

    let key_phrases = batch_results(
        &comprehend,
        "BatchDetectKeyPhrases",
        &json!({ "TextList": [DOCUMENT], "LanguageCode": "en" }),
    )
    .await;
    let phrases: Vec<(String, u64, u64)> = key_phrases[0]["KeyPhrases"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|phrase| {
            (
                phrase["Text"].as_str().unwrap_or_default().to_string(),
                phrase["BeginOffset"].as_u64().unwrap_or_default(),
                phrase["EndOffset"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    let expected: Vec<(String, u64, u64)> = PHRASES
        .iter()
        .map(|(text, begin, end)| ((*text).to_string(), *begin, *end))
        .collect();
    assert_eq!(
        phrases, expected,
        "case \"key-phrase baseline\": expected {expected:?}, got {phrases:?}"
    );

    let sentiment = batch_results(
        &comprehend,
        "BatchDetectSentiment",
        &json!({ "TextList": [DOCUMENT], "LanguageCode": "en" }),
    )
    .await;
    assert_eq!(
        sentiment[0]["Sentiment"].as_str(),
        Some("NEGATIVE"),
        "case \"sentiment baseline\": expected NEGATIVE, got {}",
        sentiment[0]
    );

    let languages = batch_results(
        &comprehend,
        "BatchDetectDominantLanguage",
        &json!({ "TextList": [DOCUMENT] }),
    )
    .await;
    let detected: Vec<&str> = languages[0]["Languages"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["LanguageCode"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        detected,
        vec!["en"],
        "case \"dominant-language baseline\": expected the document's own language \
         alone, got {}",
        languages[0]
    );
}

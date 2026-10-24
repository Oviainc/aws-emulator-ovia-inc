// SPDX-License-Identifier: CC0-1.0

//! Comprehend's batched key-phrase inference: `BatchDetectKeyPhrases`.
//!
//! The rule under test: each document of `TextList` is analysed independently by the
//! service's ONE key-phrase extractor, and every phrase it reports is a span CUT OUT
//! OF that document — `Text` is exactly the document's `BeginOffset..EndOffset`
//! characters, the spans walk the document forwards without overlapping, and `Score`
//! is a confidence in `(0, 1]` relative to the document's own strongest phrase. The
//! analysis is a function of the document and `LanguageCode` alone, so a document
//! repeated inside one batch, submitted beside other documents, or submitted again to
//! a fresh emulator is answered identically, and `ResultList` carries one entry per
//! document at that document's own zero-based `Index`.
//!
//! Every suite asserts that rule over a TABLE spanning what it generalises over —
//! language and script, document length, punctuation inside a phrase, repetition,
//! digits, a document with no word at all, and batch size — rather than over one
//! recorded request. The two documented batch quotas refuse the REQUEST rather than
//! one item of it; `sync_inference_refusals.rs` holds that rule for the whole batch
//! family, this operation included.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::LANGUAGE_CODE_VALUES;

use crate::comprehend_client::{error_code, Comprehend};
use crate::sync_inference::DOCUMENTS as SHARED_DOCUMENTS;

/// The key-phrase-specific documents: `(case, LanguageCode, text, whether the
/// document carries a content word to extract a phrase from)`.
///
/// The languages and scripts the rule generalises over are already spanned by
/// `sync_inference::DOCUMENTS` (one document per modelled `LanguageCode`), which
/// [`documents`] reuses. These rows add only what key-phrase extraction itself
/// generalises over and that table does not carry: punctuation standing INSIDE a
/// phrase (`Services, LLC`, `1111-0000-1111-0000`, `dell'anno`), a phrase that
/// repeats in one document, a one-word document, a document of digits alone (a bare
/// number is no key phrase), and a document carrying no word at all.
const KEY_PHRASE_DOCUMENTS: &[(&str, &str, &str, bool)] = &[
    (
        "english invoice",
        "en",
        "Hello Zhang Wei. Your AnyCompany Financial Services, LLC credit card account \
         1111-0000-1111-0000 has a minimum payment of $24.53 that is due by July 31st.",
        true,
    ),
    (
        "english repeated phrase",
        "en",
        "Amazon Comprehend is a natural language processing service. The natural \
         language processing service finds insights in documents.",
        true,
    ),
    ("english one word", "en", "Seattle", true),
    ("english digits alone", "en", "1234567890", false),
    ("english no word at all", "en", "!!! ... ???", false),
    (
        "italian apostrophe inside a phrase",
        "it",
        "Il tempo a Roma è molto bello in questo periodo dell'anno.",
        true,
    ),
];

/// Every document these suites assert the rule over: the crate's shared per-language
/// table — one document for each value of the modelled `LanguageCode` enum, every one
/// of which this operation accepts — plus the key-phrase-specific rows above. Each
/// shared document is prose in its own language, so each carries a content word.
fn documents() -> Vec<(&'static str, &'static str, &'static str, bool)> {
    SHARED_DOCUMENTS
        .iter()
        .map(|(case, language, text)| (*case, *language, *text, true))
        .chain(KEY_PHRASE_DOCUMENTS.iter().copied())
        .collect()
}

/// The `[begin, end)` character slice of `document`, the substring a `KeyPhrase`'s
/// offsets name.
fn character_slice(document: &str, begin: usize, end: usize) -> String {
    document
        .chars()
        .skip(begin)
        .take(end.saturating_sub(begin))
        .collect()
}

/// Assert every invariant one document's `KeyPhrases` list must satisfy, and return
/// the list.
fn assert_phrases_describe_document(case: &str, document: &str, result: &Value) -> Vec<Value> {
    let phrases = result["KeyPhrases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a KeyPhrases array, got {result}"))
        .clone();
    let characters = document.chars().count();

    let mut previous_end = 0usize;
    let mut strongest = 0.0f64;
    for phrase in &phrases {
        let text = phrase["Text"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected KeyPhrase.Text, got {phrase}"));
        let begin = phrase["BeginOffset"].as_u64().unwrap_or_else(|| {
            panic!("case {case:?}: expected KeyPhrase.BeginOffset, got {phrase}")
        }) as usize;
        let end = phrase["EndOffset"].as_u64().unwrap_or_else(|| {
            panic!("case {case:?}: expected KeyPhrase.EndOffset, got {phrase}")
        }) as usize;
        let score = phrase["Score"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected KeyPhrase.Score, got {phrase}"));

        assert!(
            begin < end && end <= characters,
            "case {case:?}: expected 0 <= BeginOffset < EndOffset <= {characters}, got \
             {begin}..{end} for {phrase}"
        );
        // The phrase is a span of the document, not text assembled for the response.
        assert_eq!(
            text,
            character_slice(document, begin, end),
            "case {case:?}: KeyPhrase.Text is not the document's {begin}..{end} characters, \
             got {phrase}"
        );
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a Score in (0, 1], got {score} for {phrase}"
        );
        // Key phrases are reported in order of appearance and never overlap, so the
        // spans walk the document forwards.
        assert!(
            begin >= previous_end,
            "case {case:?}: expected the spans in order of appearance and disjoint, got \
             {begin}..{end} after a span ending at {previous_end} in {phrases:?}"
        );
        previous_end = end;
        strongest = strongest.max(score);
    }

    // The score is relative to the document's own strongest phrase, so a document
    // with any key phrase at all has one scoring 1.0 — the phrase the document is
    // most about.
    if !phrases.is_empty() {
        assert!(
            (strongest - 1.0).abs() < 1e-6,
            "case {case:?}: expected the document's strongest phrase to score 1.0, the \
             highest was {strongest} in {phrases:?}"
        );
    }
    phrases
}

/// Call the operation on `documents` and return the parsed `ResultList`, after
/// asserting the batch-level invariants: one result per document, in ascending
/// `Index` order matching the input order, and an empty `ErrorList` — every document
/// of an accepted batch is analysed.
async fn results(comprehend: &Comprehend, language: &str, documents: &[&str]) -> Vec<Value> {
    let output = comprehend
        .ok(
            "BatchDetectKeyPhrases",
            &json!({ "TextList": documents, "LanguageCode": language }),
        )
        .await;

    let result_list = output["ResultList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResultList array, got {output}"))
        .clone();
    assert_eq!(
        result_list.len(),
        documents.len(),
        "expected one result per document, got {output}"
    );
    for (index, result) in result_list.iter().enumerate() {
        assert_eq!(
            result["Index"].as_u64(),
            Some(index as u64),
            "expected ResultList sorted ascending by Index matching the input order, got {output}"
        );
    }
    assert_eq!(
        output["ErrorList"].as_array().map(Vec::len),
        Some(0),
        "expected an empty ErrorList for a batch of analysable documents, got {output}"
    );
    result_list
}

#[tokio::test]
async fn every_key_phrase_is_a_span_of_the_document_it_was_found_in() {
    let comprehend = Comprehend::new();

    for (case, language, document, has_content_word) in documents() {
        let result_list = results(&comprehend, language, &[document]).await;
        let phrases = assert_phrases_describe_document(case, document, &result_list[0]);

        // A document that carries a content word carries at least one key phrase; one
        // that carries none — punctuation alone, or a bare number — carries no phrase
        // rather than an invented one.
        assert_eq!(
            !phrases.is_empty(),
            has_content_word,
            "case {case:?}: expected key phrases only for a document carrying a content \
             word (has_content_word={has_content_word}), got {phrases:?}"
        );
    }
}

#[tokio::test]
async fn a_phrase_that_occurs_twice_is_reported_once_per_occurrence() {
    let comprehend = Comprehend::new();
    // The same noun phrase in two sentences of one document: each occurrence carries
    // its own offsets, so the document's own repetition is visible in the answer.
    let case = "english repeated phrase";
    let document = KEY_PHRASE_DOCUMENTS
        .iter()
        .find(|(name, _, _, _)| *name == case)
        .map(|(_, _, document, _)| *document)
        .expect("the table carries the repeated-phrase document");

    let result_list = results(&comprehend, "en", &[document]).await;
    let phrases = assert_phrases_describe_document(case, document, &result_list[0]);

    let repeated = phrases.iter().find(|phrase| {
        phrases.iter().any(|other| {
            other["Text"] == phrase["Text"] && other["BeginOffset"] != phrase["BeginOffset"]
        })
    });
    assert!(
        repeated.is_some(),
        "case {case:?}: expected one entry per occurrence of a repeated phrase, got {phrases:?}"
    );
}

#[tokio::test]
async fn duplicate_documents_in_one_batch_are_answered_identically() {
    let comprehend = Comprehend::new();

    for (case, language, document, _) in documents() {
        // The same document at three indices, with a different document between them,
        // so a duplicate is answered from the document and not from its position.
        let other = "Amazon Web Services runs the emulator under test.";
        let documents = [document, other, document, document];
        let result_list = results(&comprehend, language, &documents).await;

        let expected = assert_phrases_describe_document(case, document, &result_list[0]);
        for index in [2usize, 3] {
            let phrases = assert_phrases_describe_document(case, document, &result_list[index]);
            assert_eq!(
                phrases, expected,
                "case {case:?}: expected the duplicate at index {index} to carry the same key \
                 phrases as index 0, got {phrases:?} against {expected:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_document_is_answered_the_same_alone_as_in_a_batch_and_across_calls() {
    let comprehend = Comprehend::new();
    let batch: Vec<&str> = documents()
        .into_iter()
        .filter(|(_, language, _, _)| *language == "en")
        .map(|(_, _, document, _)| document)
        .collect();

    let batched = results(&comprehend, "en", &batch).await;
    for (position, document) in batch.iter().enumerate() {
        let case = format!("document at index {position}");
        let alone = results(&comprehend, "en", &[document]).await;
        assert_eq!(
            batched[position]["KeyPhrases"], alone[0]["KeyPhrases"],
            "case {case:?}: expected a document's key phrases to be independent of the \
             documents beside it, got {:?} in the batch against {:?} alone",
            batched[position]["KeyPhrases"], alone[0]["KeyPhrases"]
        );

        // A second emulator, with its own store, answers the same: the inference
        // persists nothing and carries nothing over from an earlier call.
        let elsewhere = results(&Comprehend::new(), "en", &[document]).await;
        assert_eq!(
            alone[0]["KeyPhrases"], elsewhere[0]["KeyPhrases"],
            "case {case:?}: expected a repeated call to be answered identically, got {:?} then \
             {:?}",
            alone[0]["KeyPhrases"], elsewhere[0]["KeyPhrases"]
        );
    }
}

#[tokio::test]
async fn every_modelled_language_code_is_analysed() {
    let comprehend = Comprehend::new();

    // The catalog is the model's own `LanguageCode` enum — AWS documents this
    // operation for "any of the primary languages supported by Amazon Comprehend" —
    // so every value is covered, including the ones the issue did not cite. The
    // extractor has a stop-word list for each, so none is refused.
    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "BatchDetectKeyPhrases",
                &json!({
                    "TextList": ["The weather in London is rather pleasant today."],
                    "LanguageCode": code,
                }),
            )
            .await;

        assert_eq!(
            status, 200,
            "case {code:?}: every modelled LanguageCode is analysed, expected 200, got \
             {status}: {body}"
        );
        assert!(
            body["ResultList"][0]["KeyPhrases"].is_array(),
            "case {code:?}: expected a KeyPhrases array for the document, got {body}"
        );
    }
}

#[tokio::test]
async fn a_language_code_outside_the_modelled_enum_is_refused() {
    let comprehend = Comprehend::new();

    for code in ["xx", "EN", "english", ""] {
        let (status, body) = comprehend
            .call(
                "BatchDetectKeyPhrases",
                &json!({ "TextList": ["London weather"], "LanguageCode": code }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {code:?}: expected 400 for a value outside the LanguageCode enum, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {code:?}: a value outside the modelled enum is an input-validation \
             refusal, got {body}"
        );
    }
}

/// The batch ceiling the modelled member documentation of
/// `BatchDetectKeyPhrasesRequest$TextList` states: "The list can contain a maximum of
/// 25 documents." `sync_inference_refusals.rs` holds the refusal past it for the whole
/// batch family.
const MAX_BATCH_DOCUMENTS: usize = 25;

/// The per-document quota, the SAME one a single-document inference call enforces
/// (`crate::state::REALTIME_TEXT_QUOTA_BYTES`): a count of UTF-8 bytes, which the
/// member documentation rounds to "The maximum size of each document is 5 KB".
const MAX_DOCUMENT_BYTES: usize = 5_000;

/// A paragraph of prose, the unit a realistic document is filled with.
const PARAGRAPH: &str = "Amazon Comprehend uses natural language processing to find \
     insights in documents. The service detects the key phrases, the entities and the \
     sentiment of plain text. A customer review mentions the battery life, the screen \
     quality and the delivery time of a parcel. ";

/// A document filled with [`PARAGRAPH`] to just under the per-document quota.
fn prose_document() -> String {
    let mut document = String::new();
    while document.len() + PARAGRAPH.len() <= MAX_DOCUMENT_BYTES {
        document.push_str(PARAGRAPH);
    }
    document
}

#[tokio::test]
async fn a_batch_of_any_size_up_to_the_ceiling_reports_one_result_per_document() {
    let comprehend = Comprehend::new();
    let document = prose_document();

    // Rows span the batch sizes the quota allows: the modelled `@length` floor of 1,
    // a middling batch, and the documented ceiling exactly — each at the largest
    // document the per-document quota accepts, so the heaviest accepted request is
    // analysed rather than refused or truncated.
    for size in [1usize, 7, MAX_BATCH_DOCUMENTS] {
        let documents = vec![document.as_str(); size];
        let result_list = results(&comprehend, "en", &documents).await;

        for (index, result) in result_list.iter().enumerate() {
            let case = format!("{size} prose documents, index {index}");
            let phrases = assert_phrases_describe_document(&case, &document, result);
            assert!(
                !phrases.is_empty(),
                "case {case:?}: expected key phrases in a document of prose, got none"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_or_empty_text_list_is_refused_by_the_modelled_length_floor() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        (
            "empty TextList",
            json!({ "TextList": [], "LanguageCode": "en" }),
        ),
        ("absent TextList", json!({ "LanguageCode": "en" })),
        (
            "empty document",
            json!({ "TextList": [""], "LanguageCode": "en" }),
        ),
    ] {
        let (status, response) = comprehend.call("BatchDetectKeyPhrases", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the constraint violation to be reported as a \
             ValidationException, got {response}"
        );
    }
}

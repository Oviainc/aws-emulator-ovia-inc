// SPDX-License-Identifier: CC0-1.0

//! Comprehend's batched synchronous sentiment inference: `BatchDetectSentiment`.
//!
//! The rule under test is the one the modelled output documents: the operation
//! answers one entry per submitted document, carrying that document's zero-based
//! position in `TextList` as `Index` — a `ResultList` entry for a document it scored
//! and an `ErrorList` entry for one it could not — so both lists are aligned with the
//! input list and sorted ascending by `Index`. Batching changes how many documents one
//! request carries, never how a document is scored, so every suite below asserts over
//! a SET of batches that differ in size, in content, in document size and in the ORDER
//! the same documents are submitted in — the recorded request is one row of that table.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, SENTIMENT_TYPE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};
use crate::sync_inference::SENTIMENTS;

/// The documented ceiling on the number of documents in one batch, from
/// `BatchDetectSentimentRequest$TextList` ("The list can contain a maximum of 25
/// documents") and from `BatchSizeLimitExceededException` ("The number of documents
/// in the request exceeds the limit of 25").
const MAX_BATCH_DOCUMENTS: usize = 25;

/// The documented ceiling on each document in a batch: the same per-document
/// real-time quota a single-document inference call enforces
/// (`crate::state::REALTIME_TEXT_QUOTA_BYTES`): a count of UTF-8 bytes, which the
/// member prose rounds to "5 KB".
const MAX_DOCUMENT_BYTES: usize = 5_000;

/// The `BatchItemError$ErrorCode` the handler reports a document past
/// [`MAX_DOCUMENT_BYTES`] under.
const TEXT_SIZE_LIMIT_ERROR_CODE: &str = "TEXT_SIZE_LIMIT_EXCEEDED";

/// The batch built by taking `size` documents from [`SENTIMENTS`], cycling when the
/// batch is longer than the table: `(expected sentiment, text)` per position.
fn batch_of(size: usize) -> Vec<(&'static str, &'static str)> {
    (0..size)
        .map(|position| {
            let (_, expected, text) = SENTIMENTS[position % SENTIMENTS.len()];
            (expected, text)
        })
        .collect()
}

/// Assert the modelled per-item invariants of one `ResultList` entry and return its
/// `Sentiment`.
fn item_sentiment(case: &str, position: usize, item: &Value) -> String {
    let index = item["Index"].as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an Index on the result at position {position}, got {item}")
    });
    assert_eq!(
        index, position as i64,
        "case {case:?}: expected the result at position {position} to carry Index \
         {position}, got {index} in {item}"
    );

    let sentiment = item["Sentiment"].as_str().unwrap_or_default().to_string();
    assert!(
        SENTIMENT_TYPE_VALUES.contains(&sentiment.as_str()),
        "case {case:?}: {sentiment:?} at position {position} is not a modelled \
         SentimentType value, got {item}"
    );

    let scores = &item["SentimentScore"];
    let by_label: Vec<(&str, f64)> = ["Positive", "Negative", "Neutral", "Mixed"]
        .into_iter()
        .map(|label| {
            let score = scores[label].as_f64().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected SentimentScore.{label} at position \
                     {position}, got {item}"
                )
            });
            (label, score)
        })
        .collect();
    let total: f64 = by_label.iter().map(|(_, score)| score).sum();
    assert!(
        (total - 1.0).abs() < 1e-5,
        "case {case:?}: expected the four scores at position {position} to sum to 1.0, \
         got {total} from {scores}"
    );

    let winner = by_label
        .iter()
        .copied()
        .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
        .map(|(label, _)| label.to_uppercase())
        .unwrap_or_default();
    assert_eq!(
        winner, sentiment,
        "case {case:?}: Sentiment at position {position} is not the arg-max of \
         SentimentScore, got {sentiment:?} with {scores}"
    );

    sentiment
}

#[tokio::test]
async fn every_document_gets_one_result_carrying_its_own_index() {
    let comprehend = Comprehend::new();

    // Batch sizes spanning the modelled range: the `@length` floor of 1, a pair, a
    // batch longer than the document table, and the documented ceiling of 25.
    for size in [1usize, 2, 7, MAX_BATCH_DOCUMENTS] {
        let case = format!("{size} documents");
        let batch = batch_of(size);
        let text_list: Vec<&str> = batch.iter().map(|(_, text)| *text).collect();

        let output = comprehend
            .ok(
                "BatchDetectSentiment",
                &json!({ "TextList": text_list, "LanguageCode": "en" }),
            )
            .await;

        let results = output["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList array, got {output}"))
            .clone();
        assert_eq!(
            results.len(),
            size,
            "case {case:?}: expected one result per submitted document, got {output}"
        );
        assert_eq!(
            output["ErrorList"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: every document was scored, so ErrorList must be empty, got {output}"
        );

        let mut previous_index = -1i64;
        for (position, item) in results.iter().enumerate() {
            let sentiment = item_sentiment(&case, position, item);
            let index = item["Index"].as_i64().unwrap_or_default();
            assert!(
                index > previous_index,
                "case {case:?}: ResultList is not sorted ascending by Index, got \
                 {index} after {previous_index} in {output}"
            );
            previous_index = index;
            assert_eq!(
                sentiment, batch[position].0,
                "case {case:?}: expected {:?} for the document at position {position}, \
                 got {sentiment:?} in {output}",
                batch[position].0
            );
        }
    }
}

#[tokio::test]
async fn results_follow_the_order_the_documents_were_submitted_in() {
    let comprehend = Comprehend::new();

    // The same four documents submitted in different orders. If a result were
    // keyed to anything but its position, at least one of these orderings would
    // disagree with the batch it was built from.
    let orderings: [(&str, Vec<usize>); 4] = [
        ("table order", (0..SENTIMENTS.len()).collect()),
        ("reversed", (0..SENTIMENTS.len()).rev().collect()),
        ("rotated by one", (1..SENTIMENTS.len()).chain(0..1).collect()),
        ("one document repeated three times", vec![1, 1, 1]),
    ];

    for (case, positions) in orderings {
        let expected: Vec<&str> = positions.iter().map(|row| SENTIMENTS[*row].1).collect();
        let text_list: Vec<&str> = positions.iter().map(|row| SENTIMENTS[*row].2).collect();

        let output = comprehend
            .ok(
                "BatchDetectSentiment",
                &json!({ "TextList": text_list, "LanguageCode": "en" }),
            )
            .await;
        let results = output["ResultList"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            expected.len(),
            "case {case:?}: expected one result per submitted document, got {output}"
        );

        let actual: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(position, item)| item_sentiment(case, position, item))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the sentiments to follow the submitted order, got \
             {actual:?} in {output}"
        );
    }
}

#[tokio::test]
async fn a_batched_document_is_scored_exactly_as_the_single_document_operation_scores_it() {
    let comprehend = Comprehend::new();

    let text_list: Vec<&str> = SENTIMENTS.iter().map(|(_, _, text)| *text).collect();
    let batched = comprehend
        .ok(
            "BatchDetectSentiment",
            &json!({ "TextList": text_list, "LanguageCode": "en" }),
        )
        .await;
    let results = batched["ResultList"].as_array().cloned().unwrap_or_default();

    for (position, (case, _, text)) in SENTIMENTS.iter().enumerate() {
        let single = comprehend
            .ok(
                "DetectSentiment",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let item = &results[position];
        assert_eq!(
            item["Sentiment"], single["Sentiment"],
            "case {case:?}: batched Sentiment differs from DetectSentiment, got {item} \
             against {single}"
        );
        assert_eq!(
            item["SentimentScore"], single["SentimentScore"],
            "case {case:?}: batched SentimentScore differs from DetectSentiment, got \
             {item} against {single}"
        );
    }
}

#[tokio::test]
async fn a_batch_at_the_documented_size_limit_is_accepted_and_one_past_it_is_not() {
    let comprehend = Comprehend::new();

    for size in [1usize, MAX_BATCH_DOCUMENTS, MAX_BATCH_DOCUMENTS + 1] {
        let text_list: Vec<&str> = batch_of(size).iter().map(|(_, text)| *text).collect();
        let (status, body) = comprehend
            .call(
                "BatchDetectSentiment",
                &json!({ "TextList": text_list, "LanguageCode": "en" }),
            )
            .await;

        if size > MAX_BATCH_DOCUMENTS {
            assert_eq!(
                status, 400,
                "case \"{size} documents\": expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "BatchSizeLimitExceededException",
                "case \"{size} documents\": expected BatchSizeLimitExceededException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case \"{size} documents\": expected 200, got {status}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_document_past_the_documented_byte_ceiling_is_reported_at_its_index() {
    let comprehend = Comprehend::new();
    let scored = SENTIMENTS[0].2;
    let at_ceiling = "a".repeat(MAX_DOCUMENT_BYTES);
    let past_ceiling = "a".repeat(MAX_DOCUMENT_BYTES + 1);

    // Rows differ in where the oversized documents sit and how many there are:
    // (case, documents, the indices expected in ErrorList). The ceiling is per
    // document, not per batch, so a document exactly at it is still scored.
    let cases: Vec<(&str, Vec<&str>, Vec<i64>)> = vec![
        (
            "a document exactly at the ceiling",
            vec![&at_ceiling],
            vec![],
        ),
        ("one document, one byte past it", vec![&past_ceiling], vec![0]),
        (
            "an oversized document first",
            vec![&past_ceiling, scored, &at_ceiling],
            vec![0],
        ),
        (
            "an oversized document in the middle",
            vec![scored, &past_ceiling, scored],
            vec![1],
        ),
        (
            "two oversized documents apart",
            vec![&past_ceiling, scored, &past_ceiling],
            vec![0, 2],
        ),
        (
            "every document oversized",
            vec![&past_ceiling, &past_ceiling],
            vec![0, 1],
        ),
    ];

    for (case, text_list, expected_errors) in cases {
        let output = comprehend
            .ok(
                "BatchDetectSentiment",
                &json!({ "TextList": text_list, "LanguageCode": "en" }),
            )
            .await;

        let errors = output["ErrorList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ErrorList, got {output}"));
        let error_indices: Vec<i64> = errors
            .iter()
            .map(|entry| entry["Index"].as_i64().unwrap_or(-1))
            .collect();
        assert_eq!(
            error_indices, expected_errors,
            "case {case:?}: expected ErrorList at indices {expected_errors:?} in \
             ascending order, got {output}"
        );
        for entry in errors {
            assert_eq!(
                entry["ErrorCode"].as_str().unwrap_or_default(),
                TEXT_SIZE_LIMIT_ERROR_CODE,
                "case {case:?}: expected the text-size error code on {entry}"
            );
            assert!(
                !entry["ErrorMessage"].as_str().unwrap_or_default().is_empty(),
                "case {case:?}: expected a non-empty ErrorMessage on {entry}"
            );
        }

        // Every document NOT in ErrorList is in ResultList, still at its own index.
        let result_indices: Vec<i64> = output["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList, got {output}"))
            .iter()
            .map(|entry| entry["Index"].as_i64().unwrap_or(-1))
            .collect();
        let expected_results: Vec<i64> = (0..text_list.len() as i64)
            .filter(|index| !expected_errors.contains(index))
            .collect();
        assert_eq!(
            result_indices, expected_results,
            "case {case:?}: expected ResultList at indices {expected_results:?} in \
             ascending order, got {output}"
        );
    }
}

#[tokio::test]
async fn a_language_the_engine_cannot_read_is_refused_for_the_whole_batch() {
    let comprehend = Comprehend::new();

    // The catalog is the model's own `LanguageCode` enum, so every value the issue
    // did not cite is covered too.
    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "BatchDetectSentiment",
                &json!({ "TextList": [SENTIMENTS[0].2, SENTIMENTS[1].2], "LanguageCode": code }),
            )
            .await;

        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: the engine reads English, expected 200, got {status}: {body}"
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

#[tokio::test]
async fn an_empty_list_an_empty_document_or_an_unmodelled_language_is_refused() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        (
            "empty TextList",
            json!({ "TextList": [], "LanguageCode": "en" }),
        ),
        ("absent TextList", json!({ "LanguageCode": "en" })),
        (
            "empty document in the list",
            json!({ "TextList": ["I love it", ""], "LanguageCode": "en" }),
        ),
        (
            "LanguageCode outside the modelled enum",
            json!({ "TextList": ["I love it"], "LanguageCode": "english" }),
        ),
        (
            "absent LanguageCode",
            json!({ "TextList": ["I love it"] }),
        ),
    ] {
        let (status, response) = comprehend.call("BatchDetectSentiment", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

#[tokio::test]
async fn repeating_a_batch_returns_the_same_results() {
    let comprehend = Comprehend::new();
    let text_list: Vec<&str> = SENTIMENTS.iter().map(|(_, _, text)| *text).collect();
    let body = json!({ "TextList": text_list, "LanguageCode": "en" });

    let first = comprehend.ok("BatchDetectSentiment", &body).await;
    let second = comprehend.ok("BatchDetectSentiment", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical batch\": expected identical results, got {first} \
         then {second}"
    );
}

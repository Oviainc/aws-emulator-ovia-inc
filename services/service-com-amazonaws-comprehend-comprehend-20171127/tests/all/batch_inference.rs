// SPDX-License-Identifier: CC0-1.0

//! Comprehend's batched synchronous inference: `BatchDetectDominantLanguage`.
//!
//! The rule under test: the operation answers one `ResultList` entry per submitted
//! document, carrying that document's own identified languages and the document's
//! zero-based position in `TextList` as `Index`; the list comes back in ascending
//! index order; and a list longer than the documented 25 documents is refused with
//! `BatchSizeLimitExceededException`.
//!
//! Every suite spans the class the rule generalises over — batch size, document
//! language, document ordering, repetition and document size — rather than one
//! recorded request. The two documented batch quotas refuse the REQUEST rather than
//! one item of it; `sync_inference_refusals.rs` holds that rule for the whole batch
//! family.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::LANGUAGE_CODE_VALUES;

use crate::comprehend_client::{error_code, Comprehend};
use crate::sync_inference::DOCUMENTS;

/// The documented batch ceiling: `BatchDetectDominantLanguageRequest$TextList` says
/// "The list can contain a maximum of 25 documents."
const MAX_BATCH_DOCUMENTS: usize = 25;

/// The per-document quota, the SAME one a single-document inference call enforces
/// (`crate::state::REALTIME_TEXT_QUOTA_BYTES`): a count of UTF-8 bytes, which the
/// member documentation rounds to "The maximum size of each document is 5 KB".
/// `realtime_text_quotas.rs` holds the single-document side of the same quota.
const MAX_DOCUMENT_BYTES: usize = 5_000;

/// The byte length a quota misread as kibibytes would still accept — past the quota,
/// so a batch carrying a document of this size is refused whole.
const KIBIBYTE_ROUNDED_BYTES: usize = 5 * 1024;

/// The `ResultList` of a successful call, as (Index, top LanguageCode) pairs in the
/// order the response lists them.
fn results(output: &Value) -> Vec<(i64, String)> {
    output["ResultList"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ResultList array, got {output}"))
        .iter()
        .map(|entry| {
            let index = entry["Index"]
                .as_i64()
                .unwrap_or_else(|| panic!("expected an Index on {entry}"));
            let languages = entry["Languages"]
                .as_array()
                .unwrap_or_else(|| panic!("expected a Languages array on {entry}"));
            assert!(
                !languages.is_empty(),
                "expected at least one identified language on {entry}"
            );
            let mut previous = f64::INFINITY;
            for language in languages {
                let code = language["LanguageCode"].as_str().unwrap_or_default();
                assert!(
                    LANGUAGE_CODE_VALUES.contains(&code),
                    "{code:?} is not a modelled LanguageCode value, in {entry}"
                );
                let score = language["Score"].as_f64().unwrap_or(f64::NAN);
                assert!(
                    score > 0.0 && score <= 1.0,
                    "expected a Score in (0, 1], got {score} in {entry}"
                );
                assert!(
                    score <= previous,
                    "Languages is not sorted by descending Score: {score} after \
                     {previous} in {entry}"
                );
                previous = score;
            }
            let top = languages[0]["LanguageCode"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (index, top)
        })
        .collect()
}

/// Batches assembled from the language table, differing along what the rule
/// generalises over: count, the languages present, their ordering and repetition.
/// Each row is a list of indices into `DOCUMENTS`.
fn batches() -> Vec<(&'static str, Vec<usize>)> {
    let all: Vec<usize> = (0..DOCUMENTS.len()).collect();
    let reversed: Vec<usize> = (0..DOCUMENTS.len()).rev().collect();
    vec![
        ("one document", vec![2]),
        ("two languages", vec![4, 2]),
        ("the same two, swapped", vec![2, 4]),
        ("one language repeated", vec![6, 6, 6]),
        ("every modelled language", all),
        ("every modelled language, reversed", reversed),
        (
            "the documented maximum of 25 documents",
            (0..MAX_BATCH_DOCUMENTS)
                .map(|position| position % DOCUMENTS.len())
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn each_document_is_identified_at_its_own_index() {
    let comprehend = Comprehend::new();

    for (case, positions) in batches() {
        let text_list: Vec<&str> = positions.iter().map(|at| DOCUMENTS[*at].2).collect();
        let expected: Vec<(i64, String)> = positions
            .iter()
            .enumerate()
            .map(|(index, at)| (index as i64, DOCUMENTS[*at].1.to_string()))
            .collect();

        let output = comprehend
            .ok(
                "BatchDetectDominantLanguage",
                &json!({ "TextList": text_list }),
            )
            .await;

        assert_eq!(
            output["ErrorList"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: every document is processable, so ErrorList is empty, \
             got {output}"
        );
        let actual = results(&output);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected (Index, dominant language) pairs {expected:?}, \
             got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_of_one_agrees_with_the_single_document_operation() {
    let comprehend = Comprehend::new();

    for (case, _, text) in DOCUMENTS {
        let single = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let batched = comprehend
            .ok(
                "BatchDetectDominantLanguage",
                &json!({ "TextList": [text] }),
            )
            .await;

        assert_eq!(
            batched["ResultList"][0]["Languages"], single["Languages"],
            "case {case:?}: the batch answer must be the same inference as the \
             single-document one, got {batched} against {single}"
        );
    }
}

#[tokio::test]
async fn a_batch_longer_than_the_documented_maximum_is_refused() {
    let comprehend = Comprehend::new();

    // The modelled `@length` floor of 1, sizes inside the ceiling, the ceiling
    // exactly, and sizes past it.
    for size in [0usize, 1, 2, MAX_BATCH_DOCUMENTS, MAX_BATCH_DOCUMENTS + 1, 40] {
        let text_list: Vec<&str> = (0..size)
            .map(|position| DOCUMENTS[position % DOCUMENTS.len()].2)
            .collect();
        let (status, body) = comprehend
            .call(
                "BatchDetectDominantLanguage",
                &json!({ "TextList": text_list }),
            )
            .await;

        if size == 0 {
            // Refused by the modelled list `@length` floor, not by the batch ceiling.
            assert_eq!(
                status, 400,
                "case \"{size} documents\": expected 400, got {status}: {body}"
            );
        } else if size > MAX_BATCH_DOCUMENTS {
            assert_eq!(
                status, 400,
                "case \"{size} documents\": expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "BatchSizeLimitExceededException",
                "case \"{size} documents\": expected BatchSizeLimitExceededException, \
                 got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case \"{size} documents\": expected 200, got {status}: {body}"
            );
            assert_eq!(
                body["ResultList"].as_array().map(Vec::len),
                Some(size),
                "case \"{size} documents\": expected one result per document, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_whose_documents_all_fit_the_per_document_ceiling_is_identified_whole() {
    let comprehend = Comprehend::new();
    let english = DOCUMENTS[2].2;
    let at_ceiling = "a".repeat(MAX_DOCUMENT_BYTES);
    let past_ceiling = "a".repeat(MAX_DOCUMENT_BYTES + 1);
    let kibibyte_rounded = "a".repeat(KIBIBYTE_ROUNDED_BYTES);

    // The ceiling is measured per document and enforced over the REQUEST: a batch
    // whose documents all fit is identified in full with an empty `ErrorList`, and a
    // batch carrying one past it is refused whole rather than answered with a
    // per-index error. Rows differ in document size, batch size and the position of
    // the oversized document. `sync_inference_refusals.rs` asserts the refusal's code
    // and message for the whole batch family.
    let cases: Vec<(&str, Vec<&str>, bool)> = vec![
        ("a document exactly at the ceiling", vec![&at_ceiling], true),
        (
            "a document at the ceiling beside a short one",
            vec![english, &at_ceiling],
            true,
        ),
        ("one document, one byte past it", vec![&past_ceiling], false),
        (
            "a document at the kibibyte-rounded size",
            vec![&kibibyte_rounded],
            false,
        ),
        (
            "an oversized document first",
            vec![&past_ceiling, english, &at_ceiling],
            false,
        ),
        (
            "an oversized document in the middle",
            vec![english, &past_ceiling, english],
            false,
        ),
        (
            "every document oversized",
            vec![&past_ceiling, &past_ceiling],
            false,
        ),
    ];

    for (case, text_list, accepted) in cases {
        let (status, body) = comprehend
            .call(
                "BatchDetectDominantLanguage",
                &json!({ "TextList": text_list }),
            )
            .await;

        if !accepted {
            assert_eq!(
                status, 400,
                "case {case:?}: a batch carrying an oversized document is refused \
                 whole, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "TextSizeLimitExceededException",
                "case {case:?}: expected TextSizeLimitExceededException, got {body}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: every document fits the ceiling, expected 200, got \
             {status}: {body}"
        );
        assert_eq!(
            body["ErrorList"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: an accepted batch leaves no document unanalysed, so \
             ErrorList is empty, got {body}"
        );
        let result_indices: Vec<i64> = body["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList, got {body}"))
            .iter()
            .map(|entry| entry["Index"].as_i64().unwrap_or(-1))
            .collect();
        let expected_results: Vec<i64> = (0..text_list.len() as i64).collect();
        assert_eq!(
            result_indices, expected_results,
            "case {case:?}: expected ResultList at indices {expected_results:?} in \
             ascending order, got {body}"
        );
    }
}

#[tokio::test]
async fn batch_detect_dominant_language_is_deterministic() {
    let comprehend = Comprehend::new();
    let body = json!({ "TextList": [DOCUMENTS[4].2, DOCUMENTS[2].2] });

    let first = comprehend.ok("BatchDetectDominantLanguage", &body).await;
    let second = comprehend.ok("BatchDetectDominantLanguage", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical call\": expected identical results, got {first} \
         then {second}"
    );
}

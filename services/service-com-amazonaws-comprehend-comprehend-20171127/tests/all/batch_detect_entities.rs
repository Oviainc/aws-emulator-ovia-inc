// SPDX-License-Identifier: CC0-1.0

//! `BatchDetectEntities`: the batch projection of named-entity recognition, and the
//! documented preconditions of the batch itself.
//!
//! RULE: a synchronous batch inference operation answers ONE result per input
//! document, carrying that document's zero-based position as `Index` and exactly what
//! the single-document sibling answers for the same text, with an empty `ErrorList`;
//! and it refuses the WHOLE request, before any document is analysed and with no
//! partial result, when the request breaks a precondition of the batch — the document
//! count with `BatchSizeLimitExceededException`, the per-document byte ceiling with
//! `TextSizeLimitExceededException`, and a language the engine has no resources for
//! with `UnsupportedLanguageException`.
//!
//! Every suite asserts that rule over a TABLE: the recorded oversized batch is one row
//! of the count table, and the batch-versus-single equivalence is asserted over
//! documents differing in how many entities they carry, which evidence types them,
//! their order in the list, and whether they are ASCII.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::batch_detect_entities::{
    MAX_BATCH_DOCUMENTS, MAX_DOCUMENT_BYTES,
};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::LANGUAGE_CODE_VALUES;

use crate::comprehend_client::{error_code, error_message, expected_text_size_message, Comprehend};

/// The documents the batch suites submit, one per class of recognition evidence plus
/// the degenerate cases a batch must still answer for: a document with several
/// entities, one with a single entity, one with none at all, and one whose entity
/// sits behind non-ASCII text (so character offsets, not byte offsets, are what the
/// batch reports).
const DOCUMENTS: &[(&str, &str)] = &[
    ("several entities", "Jeff Bezos founded Amazon in Seattle."),
    ("one lexicon entity", "They expanded into Portugal."),
    ("no entity at all", "nothing to see in this sentence"),
    ("a pattern entity", "The outage began on 2024-01-15."),
    (
        "an entity behind non-ASCII text",
        "Café notes — Grace Hopper opened the hearing.",
    ),
];

/// A batch of `count` short documents, each naming its own position.
fn batch(count: usize) -> Value {
    json!({
        "TextList": (0..count)
            .map(|index| json!(format!("Document number {index} mentions Seattle.")))
            .collect::<Vec<Value>>(),
        "LanguageCode": "en",
    })
}

/// The `Entities` list `DetectEntities` answers for `text`.
async fn single_document_entities(comprehend: &Comprehend, text: &str) -> Value {
    let output = comprehend
        .ok(
            "DetectEntities",
            &json!({ "Text": text, "LanguageCode": "en" }),
        )
        .await;
    output
        .get("Entities")
        .cloned()
        .unwrap_or_else(|| json!([]))
}

#[tokio::test]
async fn every_document_is_answered_with_what_the_single_document_call_answers() {
    let comprehend = Comprehend::new();

    // The whole table in ONE batch, then the same table REVERSED: a result is tied to
    // its document by `Index`, not by the document's content or by the order the
    // table happens to list it in.
    for reversed in [false, true] {
        let mut rows: Vec<(&str, &str)> = DOCUMENTS.to_vec();
        if reversed {
            rows.reverse();
        }
        let case = if reversed { "reversed" } else { "in order" };

        let output = comprehend
            .ok(
                "BatchDetectEntities",
                &json!({
                    "TextList": rows.iter().map(|(_, text)| json!(text)).collect::<Vec<Value>>(),
                    "LanguageCode": "en",
                }),
            )
            .await;

        let result_list = output["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a ResultList array, got {output}"));
        assert_eq!(
            result_list.len(),
            rows.len(),
            "case {case}: expected one result per document, got {output}"
        );
        assert_eq!(
            output["ErrorList"],
            json!([]),
            "case {case}: an accepted batch leaves no document unanalysed, got {output}"
        );

        for (position, (label, text)) in rows.iter().enumerate() {
            let result = &result_list[position];
            assert_eq!(
                result["Index"],
                json!(position as i64),
                "case {case}/{label}: expected Index {position}, got {result}"
            );
            let expected = single_document_entities(&comprehend, text).await;
            assert_eq!(
                result["Entities"], expected,
                "case {case}/{label}: the batch must answer what DetectEntities answers for \
                 the same text, got {result}"
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_document_is_answered_once_per_position() {
    let comprehend = Comprehend::new();

    // The same text at several positions: inference is a function of the document, so
    // each position carries the identical entity list under its own `Index`, and
    // nothing is deduplicated.
    let text = "Jeff Bezos founded Amazon in Seattle.";
    for count in [1usize, 2, 5, MAX_BATCH_DOCUMENTS] {
        let output = comprehend
            .ok(
                "BatchDetectEntities",
                &json!({
                    "TextList": (0..count).map(|_| json!(text)).collect::<Vec<Value>>(),
                    "LanguageCode": "en",
                }),
            )
            .await;
        let result_list = output["ResultList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {count} copies: expected a ResultList, got {output}"));
        assert_eq!(
            result_list.len(),
            count,
            "case {count} copies: expected one result per document, got {output}"
        );
        let expected = single_document_entities(&comprehend, text).await;
        assert!(
            expected.as_array().is_some_and(|list| !list.is_empty()),
            "case {count} copies: the probe document must carry entities, got {expected}"
        );
        for (position, result) in result_list.iter().enumerate() {
            assert_eq!(
                result["Index"],
                json!(position as i64),
                "case {count} copies: expected Index {position}, got {result}"
            );
            assert_eq!(
                result["Entities"], expected,
                "case {count} copies: position {position} must carry the same entities as \
                 every other copy, got {result}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_past_the_documented_document_count_is_refused_whole() {
    let comprehend = Comprehend::new();

    // The modelled ceiling (`TextList`: "a maximum of 25 documents"): well inside it,
    // exactly at it, one past it, and far past it.
    for count in [1usize, 2, 24, MAX_BATCH_DOCUMENTS, MAX_BATCH_DOCUMENTS + 1, 40] {
        let (status, body) = comprehend.call("BatchDetectEntities", &batch(count)).await;

        if count > MAX_BATCH_DOCUMENTS {
            assert_eq!(
                status, 400,
                "case {count} documents: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "BatchSizeLimitExceededException",
                "case {count} documents: expected BatchSizeLimitExceededException, got {body}"
            );
            // Refused wholesale: no partial results travel with the refusal.
            assert!(
                body.get("ResultList").is_none() && body.get("ErrorList").is_none(),
                "case {count} documents: an oversized batch is refused with no partial \
                 ResultList/ErrorList, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {count} documents: a batch inside the quota must be analysed, got \
                 {status}: {body}"
            );
            assert_eq!(
                body["ResultList"].as_array().map(Vec::len),
                Some(count),
                "case {count} documents: expected one result per document, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_document_past_the_documented_byte_ceiling_is_refused_and_named() {
    let comprehend = Comprehend::new();

    // The modelled ceiling (`TextList`: "The maximum size of each document is 5 KB"):
    // one byte, exactly at it, and one past it. Each size is placed at a different
    // position in the batch, so the refusal is about the document and not about the
    // first one.
    for (position, size) in [
        (0usize, 1usize),
        (1, MAX_DOCUMENT_BYTES),
        (2, MAX_DOCUMENT_BYTES + 1),
        (0, MAX_DOCUMENT_BYTES + 500),
    ] {
        let text_list: Vec<Value> = (0..3)
            .map(|index| {
                if index == position {
                    json!("a".repeat(size))
                } else {
                    json!("a short document")
                }
            })
            .collect();
        let (status, body) = comprehend
            .call(
                "BatchDetectEntities",
                &json!({ "TextList": text_list, "LanguageCode": "en" }),
            )
            .await;

        if size > MAX_DOCUMENT_BYTES {
            assert_eq!(
                status, 400,
                "case {size} bytes at index {position}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "TextSizeLimitExceededException",
                "case {size} bytes at index {position}: expected \
                 TextSizeLimitExceededException, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                expected_text_size_message(MAX_DOCUMENT_BYTES, size),
                "case {size} bytes at index {position}: the refusal must report the quota \
                 and the offending document's byte length, got {message:?}"
            );
            assert!(
                body.get("ResultList").is_none(),
                "case {size} bytes at index {position}: no partial ResultList travels with \
                 the refusal, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {size} bytes at index {position}: a document inside the ceiling must \
                 not be refused for it, got {status}: {body}"
            );
            assert_eq!(
                body["ResultList"].as_array().map(Vec::len),
                Some(3),
                "case {size} bytes at index {position}: expected one result per document, \
                 got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_document_count_is_checked_before_any_document_is_looked_at() {
    let comprehend = Comprehend::new();

    // Both quotas are broken at once. The count is the quota that decides, because
    // the batch is refused before any document in it is measured.
    let text_list: Vec<Value> = (0..MAX_BATCH_DOCUMENTS + 1)
        .map(|_| json!("a".repeat(MAX_DOCUMENT_BYTES + 1)))
        .collect();
    let (status, body) = comprehend
        .call(
            "BatchDetectEntities",
            &json!({ "TextList": text_list, "LanguageCode": "en" }),
        )
        .await;

    assert_eq!(
        status, 400,
        "case \"both quotas broken\": expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "BatchSizeLimitExceededException",
        "case \"both quotas broken\": the document count decides, got {body}"
    );
}

#[tokio::test]
async fn a_language_the_recognizer_has_no_resources_for_refuses_the_whole_batch() {
    let comprehend = Comprehend::new();

    // Every value the modelled `LanguageCode` enum names
    // (`super::types::LANGUAGE_CODE_VALUES`), so the row set is the model's and not a
    // hand-picked pair: the batch answers for the languages the engine has resources
    // for and refuses the rest wholesale, exactly as the single-document sibling
    // does for the same code.
    for language_code in LANGUAGE_CODE_VALUES {
        let request = json!({
            "TextList": ["Jeff Bezos founded Amazon in Seattle.", "No entities here."],
            "LanguageCode": language_code,
        });
        let (status, body) = comprehend.call("BatchDetectEntities", &request).await;
        let (single_status, single_body) = comprehend
            .call(
                "DetectEntities",
                &json!({ "Text": "Jeff Bezos founded Amazon in Seattle.", "LanguageCode": language_code }),
            )
            .await;

        assert_eq!(
            status, single_status,
            "case {language_code:?}: a batch must accept exactly the languages the \
             single-document call accepts, got {status} vs {single_status}: {body}"
        );
        if single_status == 200 {
            assert_eq!(
                body["ResultList"].as_array().map(Vec::len),
                Some(2),
                "case {language_code:?}: expected one result per document, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguageException",
                "case {language_code:?}: expected UnsupportedLanguageException, got {body}"
            );
            assert_eq!(
                error_code(&body),
                error_code(&single_body),
                "case {language_code:?}: the batch must refuse with the same modelled error \
                 the single-document call refuses with, got {body}"
            );
            assert!(
                body.get("ResultList").is_none(),
                "case {language_code:?}: no ResultList may be invented for documents nothing \
                 analysed, got {body}"
            );
        }
    }
}

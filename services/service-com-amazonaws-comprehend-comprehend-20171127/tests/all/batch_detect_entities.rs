// SPDX-License-Identifier: CC0-1.0

//! `BatchDetectEntities`' documented batch quotas.
//!
//! RULE: a synchronous batch inference operation refuses the WHOLE request, before
//! any document is looked at, when the request breaks a quota the model documents for
//! the batch — the document count with `BatchSizeLimitExceededException` and the
//! per-document byte ceiling with `TextSizeLimitExceededException` — and answers no
//! partial result for it. The recorded 26-document batch is ONE row of the table
//! below; the rows differ along the count, the document sizes and which document in
//! the batch breaks the ceiling.
//!
//! Entity recognition ITSELF is still unimplemented — it needs a trained named-entity
//! recognition model and this emulator has no in-process engine for one — so a
//! request that satisfies every quota answers the scaffold's HTTP 501, never an
//! invented `ResultList`. The suites pin that too: the 501 is what "nobody has
//! written it yet" looks like, and it is NOT what a quota breach answers.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::batch_detect_entities::{
    MAX_BATCH_DOCUMENTS, MAX_DOCUMENT_BYTES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// The wire error code of the scaffold's 501: what an unimplemented operation
/// answers, as `crate::scaffolded::NotImplemented` renders it.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// A batch of `count` short documents.
fn batch(count: usize) -> Value {
    json!({
        "TextList": (0..count)
            .map(|index| json!(format!("Document number {index}.")))
            .collect::<Vec<Value>>(),
        "LanguageCode": "en",
    })
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
                status, 501,
                "case {count} documents: a batch inside the quota must not be refused for \
                 it, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_IMPLEMENTED,
                "case {count} documents: expected the scaffold's {NOT_IMPLEMENTED} for the \
                 unimplemented recognition, got {body}"
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
    for (position, size) in [(0usize, 1usize), (1, MAX_DOCUMENT_BYTES), (2, MAX_DOCUMENT_BYTES + 1)]
    {
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
            let message = body["message"]
                .as_str()
                .or_else(|| body["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(&position.to_string()),
                "case {size} bytes at index {position}: the refusal must name the document \
                 that broke the ceiling, got {message:?}"
            );
        } else {
            assert_eq!(
                status, 501,
                "case {size} bytes at index {position}: a document inside the ceiling must \
                 not be refused for it, got {status}: {body}"
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
async fn the_recognition_itself_is_refused_rather_than_invented() {
    let comprehend = Comprehend::new();

    // Every modelled `LanguageCode` is one AWS recognises entities in, so none of
    // them turns a compliant batch into a quota refusal: what comes back is the
    // scaffold's 501 for the unwritten recognition, naming the operation.
    for language_code in ["en", "de", "zh-TW"] {
        let (status, body) = comprehend
            .call(
                "BatchDetectEntities",
                &json!({
                    "TextList": ["Jeff Bezos founded Amazon in Seattle.", "No entities here."],
                    "LanguageCode": language_code,
                }),
            )
            .await;
        assert_eq!(
            status, 501,
            "case {language_code:?}: expected 501 for the unimplemented recognition, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            NOT_IMPLEMENTED,
            "case {language_code:?}: expected {NOT_IMPLEMENTED}, got {body}"
        );
        assert!(
            body.get("ResultList").is_none(),
            "case {language_code:?}: no ResultList may be invented for documents nothing \
             analysed, got {body}"
        );
    }
}

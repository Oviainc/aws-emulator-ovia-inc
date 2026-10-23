// SPDX-License-Identifier: CC0-1.0

//! The per-document size quota Comprehend's real-time (synchronous) inference
//! operations enforce, and the refusal they answer a document past it with.
//!
//! The rule under test, for every real-time inference operation: a document whose
//! UTF-8 byte length exceeds that operation's published per-document quota is
//! refused with HTTP 400 `TextSizeLimitExceededException`, whose message reports
//! the quota and the submitted document's byte length; a document at or below the
//! quota is accepted. The quota is a count of BYTES — not characters, and not a
//! multiple of 1024 — so the rows below span both sides of each quota, the
//! kibibyte-rounded value just above it, and documents whose characters are wider
//! than one byte.

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, Comprehend};

/// Every real-time inference operation this emulator implements, with the
/// per-document quota it enforces in UTF-8 bytes.
///
/// Source: `crate::state::REALTIME_TEXT_QUOTA_BYTES`,
/// `crate::state::DOMINANT_LANGUAGE_TEXT_QUOTA_BYTES` and
/// `crate::state::PII_TEXT_QUOTA_BYTES`, the service's own quota table — the quota is
/// the one each operation's own modelled member documentation states, or the one the
/// service publishes for real-time analysis where the model states none: 100,000
/// bytes for `DetectDominantLanguage` and for PII analysis (whose `Text` member
/// documents "The maximum string size is 100 KB"), and 5,000 bytes for a
/// single-document inference call whose model states no maximum. Comprehend's other
/// real-time `Detect*` operations are still scaffolded (they answer HTTP 501), so
/// they have no row here yet.
const QUOTAS: &[(&str, usize)] = &[
    ("ContainsPiiEntities", 100_000),
    ("DetectSentiment", 5_000),
    ("DetectSyntax", 5_000),
    ("DetectDominantLanguage", 100_000),
];

/// The request body for `operation` carrying `text`.
fn body(operation: &str, text: String) -> Value {
    match operation {
        // `LanguageCode` is `@required` on these operations, and each accepts
        // English, so the size check is the only thing that can refuse the call.
        "ContainsPiiEntities" | "DetectSentiment" | "DetectSyntax" => {
            json!({ "Text": text, "LanguageCode": "en" })
        }
        _ => json!({ "Text": text }),
    }
}

/// The message the service reports for a document of `actual_bytes` bytes against a
/// `quota_bytes` quota: the size-report sentence, built from the request.
fn expected_message(quota_bytes: usize, actual_bytes: usize) -> String {
    format!(
        "Input text size exceeds limit. Max length of request text allowed is \
         {quota_bytes} bytes while in this request the text size is {actual_bytes} bytes"
    )
}

/// The `Message` member of an error response, whatever casing the protocol renders
/// it with.
fn error_message(response: &Value) -> String {
    response
        .get("Message")
        .or_else(|| response.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

#[tokio::test]
async fn a_document_is_accepted_up_to_the_quota_and_refused_one_byte_past_it() {
    let comprehend = Comprehend::new();

    for (operation, quota) in QUOTAS {
        // Each row is a byte length: the modelled `@length` floor, well inside the
        // quota, the last accepted length, the first refused one, the value a quota
        // misread as kibibytes would still accept (`5 * 1024` and `100 * 1024`),
        // and far past it.
        for size in [
            1,
            quota / 2,
            quota - 1,
            *quota,
            quota + 1,
            (quota / 1_000) * 1_024,
            quota * 2,
        ] {
            let (status, response) = comprehend
                .call(operation, &body(operation, "a".repeat(size)))
                .await;

            if size > *quota {
                assert_eq!(
                    status, 400,
                    "case {operation} with {size} bytes: expected 400, got {status}: {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "TextSizeLimitExceededException",
                    "case {operation} with {size} bytes: expected \
                     TextSizeLimitExceededException, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    expected_message(*quota, size),
                    "case {operation} with {size} bytes: the refusal must report the \
                     quota and the submitted byte length, got {response}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {operation} with {size} bytes: expected 200, got {status}: {response}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_quota_counts_utf8_bytes_and_not_characters() {
    let comprehend = Comprehend::new();

    // Characters wider than one byte, with their UTF-8 width: a document of
    // `quota / width` of them sits exactly ON the quota, and one more character is
    // `width` bytes past it — while its CHARACTER count is still far below it.
    for (case, character, width) in [
        ("two-byte", "é", 2),
        ("three-byte", "ก", 3),
        ("four-byte", "😀", 4),
    ] {
        for (operation, quota) in QUOTAS {
            let fitting = quota / width;
            let (status, response) = comprehend
                .call(operation, &body(operation, character.repeat(fitting)))
                .await;
            assert_eq!(
                status, 200,
                "case {case} {operation} with {fitting} characters ({} bytes): expected \
                 200, got {status}: {response}",
                fitting * width
            );

            let over = fitting + 1;
            let over_bytes = over * width;
            let (status, response) = comprehend
                .call(operation, &body(operation, character.repeat(over)))
                .await;
            assert_eq!(
                status, 400,
                "case {case} {operation} with {over} characters ({over_bytes} bytes): \
                 expected 400 because the quota counts bytes, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TextSizeLimitExceededException",
                "case {case} {operation} with {over_bytes} bytes: expected \
                 TextSizeLimitExceededException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected_message(*quota, over_bytes),
                "case {case} {operation} with {over_bytes} bytes: the refusal must \
                 report the submitted UTF-8 byte length, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn every_operation_reports_its_own_quota_in_the_same_sentence() {
    let comprehend = Comprehend::new();

    // The sentence is one form parameterized on the operation's quota and the
    // request's byte length, so the SAME oversized document gets a different limit
    // reported by each operation — and nothing about the operation's name or the
    // emulator's own wording enters it.
    let text = "x".repeat(200_000);
    for (operation, quota) in QUOTAS {
        let (status, response) = comprehend
            .call(operation, &body(operation, text.clone()))
            .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400 for a 200000-byte document, got {status}: {response}"
        );
        assert_eq!(
            error_message(&response),
            expected_message(*quota, text.len()),
            "case {operation}: expected the quota {quota} and the byte length {} in \
             the message, got {response}",
            text.len()
        );
    }
}

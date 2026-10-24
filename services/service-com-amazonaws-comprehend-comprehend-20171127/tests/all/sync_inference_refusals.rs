// SPDX-License-Identifier: CC0-1.0

//! How Comprehend's synchronous (real-time) inference surface REFUSES a request, and
//! how decisive the confidence it reports is.
//!
//! Four rules, each asserted over a SET of requests rather than one recorded call:
//!
//! 1. A batch inference operation applies the documented per-document byte quota as a
//!    PRECONDITION OF THE REQUEST: a `TextList` carrying any document past the quota
//!    is refused in its entirety with HTTP 400 `TextSizeLimitExceededException` and no
//!    `ResultList`/`ErrorList` body, whatever else the batch holds — never with a
//!    per-index `ErrorList` entry beside a partial `ResultList`.
//! 2. The documented batch document-count quota is refused with HTTP 400
//!    `BatchSizeLimitExceededException` carrying a CONSTANT message: the same sentence
//!    for every batch operation and for every batch size, with nothing substituted
//!    from the request.
//! 3. Where AWS's own per-operation language set is narrower than the modelled
//!    `LanguageCode` enum the member targets, a value outside that set is refused as
//!    an input-validation constraint — HTTP 400 `ValidationException` naming the
//!    member, the offending value and the allowed set — rather than with the modelled
//!    `UnsupportedLanguageException`.
//! 4. `DetectDominantLanguage` reports the leading candidate's confidence DECISIVELY
//!    for a document with a clear dominant language: `Languages[0].Score` is past the
//!    0.5 threshold a client gates on, while the engine's ranking is untouched.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::LANGUAGE_CODE_VALUES;

use crate::comprehend_client::{error_code, error_message, Comprehend};

/// The per-document quota every batch inference operation enforces, in UTF-8 bytes:
/// `crate::state::REALTIME_TEXT_QUOTA_BYTES`, which each operation's `TextList`
/// member documentation rounds to "The maximum size of each document is 5 KB".
const MAX_DOCUMENT_BYTES: usize = 5_000;

/// The documented document-count quota of one batch:
/// `crate::state::MAX_BATCH_DOCUMENTS`, from every batch operation's `TextList`
/// ("The list can contain a maximum of 25 documents").
const MAX_BATCH_DOCUMENTS: usize = 25;

/// The constant message `BatchSizeLimitExceededException` carries, for every batch
/// operation and every batch size.
const BATCH_SIZE_LIMIT_MESSAGE: &str =
    "Please use a smaller number of items in a single batch. Maximum batch size is 25.";

/// Every batch inference operation this emulator implements, with the HTTP status a
/// request that satisfies both quotas answers with.
///
/// `BatchDetectEntities` answers the scaffold's 501 for the recognition itself (its
/// own stub issue), which is exactly why it belongs here: the quotas are refusals of
/// the REQUEST, so they are decided before the operation's own analysis is reached.
const BATCH_OPERATIONS: &[(&str, u16)] = &[
    ("BatchDetectSentiment", 200),
    ("BatchDetectDominantLanguage", 200),
    ("BatchDetectSyntax", 200),
    ("BatchDetectKeyPhrases", 200),
    ("BatchDetectEntities", 501),
];

/// A well-formed document every batch operation accepts.
const GOOD_DOCUMENT: &str = "Short valid text for sentiment analysis.";

/// The request body for `operation` carrying `text_list`.
///
/// `LanguageCode` is `@required` on every batch operation except
/// `BatchDetectDominantLanguage`, whose input models `TextList` alone.
fn body(operation: &str, text_list: &[&str]) -> Value {
    if operation == "BatchDetectDominantLanguage" {
        json!({ "TextList": text_list })
    } else {
        json!({ "TextList": text_list, "LanguageCode": "en" })
    }
}

/// The size-report sentence the service refuses an oversized document with, built
/// from the request: the quota that applies and the offending document's own UTF-8
/// byte length. Same form as the single-document refusal in `realtime_text_quotas`.
fn expected_size_message(actual_bytes: usize) -> String {
    format!(
        "Input text size exceeds limit. Max length of request text allowed is \
         {MAX_DOCUMENT_BYTES} bytes while in this request the text size is {actual_bytes} bytes"
    )
}

#[tokio::test]
async fn a_batch_carrying_an_oversized_document_is_refused_whole() {
    let comprehend = Comprehend::new();

    // Rows span what the rule generalises over: the byte length past the quota (one
    // byte past, the recorded 6000, the value a quota misread as kibibytes would
    // accept, and the recorded 300000), where the oversized document sits in the
    // batch, how many documents travel with it, and how many of them are oversized.
    // `offending` is the byte length the refusal must report: the FIRST oversized
    // document in submission order.
    let sizes = [MAX_DOCUMENT_BYTES + 1, 6_000, 5 * 1024, 300_000];
    for size in sizes {
        let oversized = "a".repeat(size);
        let cases: Vec<(&str, Vec<&str>, usize)> = vec![
            ("the oversized document alone", vec![&oversized], size),
            (
                "an oversized document first",
                vec![&oversized, GOOD_DOCUMENT],
                size,
            ),
            (
                "an oversized document last",
                vec![GOOD_DOCUMENT, &oversized],
                size,
            ),
            (
                "an oversized document in the middle",
                vec![GOOD_DOCUMENT, &oversized, GOOD_DOCUMENT],
                size,
            ),
            (
                "two oversized documents apart",
                vec![&oversized, GOOD_DOCUMENT, &oversized],
                size,
            ),
        ];

        for (operation, _) in BATCH_OPERATIONS {
            for (case, text_list, offending) in &cases {
                let (status, response) = comprehend.call(operation, &body(operation, text_list)).await;
                let case = format!("{operation}: {case} at {size} bytes");

                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status}: {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "TextSizeLimitExceededException",
                    "case {case:?}: expected TextSizeLimitExceededException, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    expected_size_message(*offending),
                    "case {case:?}: the refusal must report the quota and the offending \
                     document's byte length, got {response}"
                );
                // Refused wholesale: no partial analysis and no per-index error list
                // travel with the refusal.
                assert!(
                    response.get("ResultList").is_none() && response.get("ErrorList").is_none(),
                    "case {case:?}: a refused batch carries no ResultList/ErrorList, got \
                     {response}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_batch_of_documents_inside_the_quota_is_accepted() {
    let comprehend = Comprehend::new();

    // The other side of the same rule: the quota refuses the request only for a
    // document PAST it, so a document exactly at the quota — and the well-formed
    // document that travelled beside an oversized one above, submitted on its own —
    // is accepted. Rows differ in document size and batch size.
    let at_ceiling = "a".repeat(MAX_DOCUMENT_BYTES);
    let just_inside = "a".repeat(MAX_DOCUMENT_BYTES - 1);
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("one well-formed document", vec![GOOD_DOCUMENT]),
        (
            "two well-formed documents",
            vec![GOOD_DOCUMENT, GOOD_DOCUMENT],
        ),
        ("one document one byte inside the quota", vec![&just_inside]),
        ("one document exactly at the quota", vec![&at_ceiling]),
        (
            "a document at the quota beside a short one",
            vec![GOOD_DOCUMENT, &at_ceiling],
        ),
    ];

    for (operation, accepted_status) in BATCH_OPERATIONS {
        for (case, text_list) in &cases {
            let (status, response) = comprehend.call(operation, &body(operation, text_list)).await;
            assert_eq!(
                status, *accepted_status,
                "case {operation}: {case:?}: a batch inside both quotas must not be \
                 refused for them, expected {accepted_status}, got {status}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_batch_size_refusal_carries_the_same_constant_message_everywhere() {
    let comprehend = Comprehend::new();

    // Rows: one past the documented ceiling (the recorded 26), and further past it.
    // The message may not vary with the count, so every row expects the same
    // sentence — and none of them may contain the submitted count.
    for count in [MAX_BATCH_DOCUMENTS + 1, 30, 50] {
        let documents: Vec<String> = (0..count)
            .map(|index| format!("Document number {index} for analysis."))
            .collect();
        let text_list: Vec<&str> = documents.iter().map(String::as_str).collect();

        for (operation, _) in BATCH_OPERATIONS {
            let (status, response) = comprehend.call(operation, &body(operation, &text_list)).await;
            let case = format!("{operation} with {count} documents");

            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "BatchSizeLimitExceededException",
                "case {case:?}: expected BatchSizeLimitExceededException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                BATCH_SIZE_LIMIT_MESSAGE,
                "case {case:?}: expected the service's constant batch-size message, got \
                 {response}"
            );
            assert!(
                !error_message(&response).contains(&count.to_string()),
                "case {case:?}: the message substitutes nothing from the request, so it \
                 cannot carry the submitted count, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_count_quota_decides_before_any_document_is_measured() {
    let comprehend = Comprehend::new();

    // Both quotas broken at once: the count is the quota the service reads first,
    // so it is the one reported.
    let oversized = "a".repeat(MAX_DOCUMENT_BYTES + 1);
    let text_list: Vec<&str> = (0..MAX_BATCH_DOCUMENTS + 1)
        .map(|_| oversized.as_str())
        .collect();

    for (operation, _) in BATCH_OPERATIONS {
        let (status, response) = comprehend.call(operation, &body(operation, &text_list)).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "BatchSizeLimitExceededException",
            "case {operation}: with both quotas broken the document count decides, got \
             {response}"
        );
    }
}

#[tokio::test]
async fn a_language_outside_the_operations_own_set_is_refused_as_an_enum_constraint() {
    let comprehend = Comprehend::new();

    // `DetectTargetedSentimentRequest$LanguageCode`: "Currently, English is the only
    // supported language" — an operation-scoped set narrower than the modelled
    // `LanguageCode` enum the member targets. The catalog is the model's own enum, so
    // every value the issue did not cite is covered too.
    let allowed = ["en"];
    for code in LANGUAGE_CODE_VALUES {
        let (status, response) = comprehend
            .call(
                "DetectTargetedSentiment",
                &json!({
                    "Text": "La batterie de cet ordinateur est incroyable.",
                    "LanguageCode": code,
                }),
            )
            .await;

        if allowed.contains(code) {
            assert_eq!(
                status, 200,
                "case {code:?}: the operation's own set accepts this language, expected \
                 200, got {status}: {response}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {code:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {code:?}: a value outside the operation's own language set is an \
             input-validation refusal, got {response}"
        );
        assert_eq!(
            error_message(&response),
            format!(
                "Value '{code}' at 'languageCode'failed to satisfy constraint: Member \
                 must satisfy enum value set: [{}]",
                allowed.join(", ")
            ),
            "case {code:?}: the refusal must name the member, the offending value and \
             the allowed set, got {response}"
        );
    }
}

/// Documents with a clear dominant language, and the language it is.
///
/// Rows differ along what the rule generalises over: whether the document is
/// monolingual or carries shorter material in other languages, which language
/// predominates, how many other languages appear, and whether the minority material
/// is a greeting or a whole clause. The first row is the document the issue cited.
const DECISIVE_DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "an english clause beside greetings in five languages",
        "en",
        "Hello 你好 Bonjour Hola Ciao Guten Tag Namaste. I am Bob, a software engineer \
         in Seattle.",
    ),
    (
        "a german paragraph beside an english clause",
        "de",
        "Das Wetter in Berlin ist heute sehr schön und wir gehen später mit den Kindern \
         im Park spazieren. The weather is nice.",
    ),
    (
        "a french paragraph beside a spanish greeting",
        "fr",
        "Il fait très beau à Paris en ce moment et nous allons nous promener dans le \
         parc cet après-midi avec les enfants. Hola amigo, buenos días.",
    ),
    (
        "a monolingual english document",
        "en",
        "Good afternoon, how are you today? The weather in London is rather pleasant.",
    ),
    (
        "a monolingual portuguese document",
        "pt",
        "Bom dia, como você está hoje? O tempo em Lisboa está muito bom nesta altura.",
    ),
    (
        "a monolingual italian document",
        "it",
        "Buongiorno, come stai oggi? Il tempo a Roma è molto bello in questo periodo.",
    ),
];

/// The confidence threshold a client gates a detection on: the leading candidate of
/// a document with a clear dominant language has to be past it, or an
/// otherwise-correct detection is unusable.
const DECISION_THRESHOLD: f64 = 0.5;

#[tokio::test]
async fn the_dominant_language_of_a_clear_document_is_reported_decisively() {
    let comprehend = Comprehend::new();

    for (case, expected, text) in DECISIVE_DOCUMENTS {
        let output = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let languages = output["Languages"].as_array().cloned().unwrap_or_default();
        assert!(
            !languages.is_empty(),
            "case {case:?}: expected a ranked Languages array, got {output}"
        );

        let top_code = languages[0]["LanguageCode"].as_str().unwrap_or_default();
        assert_eq!(
            top_code, *expected,
            "case {case:?}: expected the dominant language {expected:?} first, got \
             {top_code:?} in {languages:?}"
        );
        let top_score = languages[0]["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            top_score > DECISION_THRESHOLD,
            "case {case:?}: expected the dominant language's Score past the \
             {DECISION_THRESHOLD} decision threshold, got {top_score} in {languages:?}"
        );
    }
}

#[tokio::test]
async fn the_batched_identification_reports_the_same_decisive_scores() {
    let comprehend = Comprehend::new();

    // The batch variant shares the single-document inference, so the calibrated
    // distribution is the same one, document for document.
    let text_list: Vec<&str> = DECISIVE_DOCUMENTS.iter().map(|(_, _, text)| *text).collect();
    let batched = comprehend
        .ok(
            "BatchDetectDominantLanguage",
            &json!({ "TextList": text_list }),
        )
        .await;

    for (position, (case, expected, text)) in DECISIVE_DOCUMENTS.iter().enumerate() {
        let single = comprehend
            .ok("DetectDominantLanguage", &json!({ "Text": text }))
            .await;
        let item = &batched["ResultList"][position];
        assert_eq!(
            item["Languages"], single["Languages"],
            "case {case:?}: the batched identification must be the single-document one, \
             got {item} against {single}"
        );
        let top = &item["Languages"][0];
        assert_eq!(
            top["LanguageCode"].as_str().unwrap_or_default(),
            *expected,
            "case {case:?}: expected the dominant language {expected:?} first, got {item}"
        );
        assert!(
            top["Score"].as_f64().unwrap_or(f64::NAN) > DECISION_THRESHOLD,
            "case {case:?}: expected the dominant language's Score past the \
             {DECISION_THRESHOLD} decision threshold, got {item}"
        );
    }
}

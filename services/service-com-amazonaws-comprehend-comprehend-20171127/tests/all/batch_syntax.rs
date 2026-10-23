// SPDX-License-Identifier: CC0-1.0

//! Comprehend's batched syntax analysis: `BatchDetectSyntax`.
//!
//! Two rules, each asserted over a SET of batches rather than one recorded request:
//!
//! 1. The operation answers one `ResultList` entry per submitted document, carrying
//!    that document's zero-based position in `TextList` as `Index`, so `ResultList` is
//!    aligned with the input list and sorted ascending by `Index`, and `ErrorList` is
//!    empty for a batch every document of which was analysed.
//! 2. Batching changes how many documents one request carries, never how a document is
//!    analysed: each entry's `SyntaxTokens` are exactly the tokens `DetectSyntax`
//!    reports for that document on its own — one token per word-boundary unit, a
//!    1-based `TokenId`, a half-open range of zero-based CHARACTER offsets that cuts
//!    the token's own `Text` out of the document, and a modelled
//!    `PartOfSpeechTagType` tag with a confidence in (0, 1].
//!
//! The rows differ along what the rules generalise over: batch size, the order the
//! documents are submitted in, document size, token count, word classes, figures,
//! symbols, multi-byte characters, sentence count and whitespace form. The offset rule
//! is an invariant, so beside the hand-written table it is also asserted over an
//! EXHAUSTIVE sweep of every ordered triple of a fragment alphabet — the whole product,
//! so the sweep needs no generator crate and no seed to reproduce.
//!
//! The documented batch quotas refuse the REQUEST rather than one item of it;
//! `sync_inference_refusals.rs` holds that rule for the whole batch family, with
//! `BatchDetectSyntax` as one of its rows.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    PART_OF_SPEECH_TAG_TYPE_VALUES, SYNTAX_LANGUAGE_CODE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// The documented ceiling on the number of documents in one batch, from
/// `BatchDetectSyntaxRequest$TextList` ("The list can contain a maximum of 25
/// documents") and from `BatchSizeLimitExceededException` ("The number of documents
/// in the request exceeds the limit of 25").
const MAX_BATCH_DOCUMENTS: usize = 25;

/// The documented ceiling on each document in a batch: the same per-document
/// real-time quota a single-document inference call enforces
/// (`crate::state::REALTIME_TEXT_QUOTA_BYTES`): a count of UTF-8 bytes, which the
/// member prose rounds to "The maximum size for each document is 5 KB".
const MAX_DOCUMENT_BYTES: usize = 5_000;

/// The language the emulator's syntax tagger has a lexicon for, of the modelled
/// `SyntaxLanguageCode` values; every other one is refused with
/// `UnsupportedLanguageException`, as `DetectSyntax` refuses it.
const TAGGABLE_LANGUAGE: &str = "en";

/// Documents the batch suites submit. The rows differ along token count, the word
/// classes present, figures and symbols, contractions, identifiers, multi-byte and
/// astral-plane characters, sentence count, whitespace form, and documents with no
/// word at all.
const DOCUMENTS: &[(&str, &str)] = &[
    ("simple declarative", "The dog barks loudly."),
    ("two sentences", "She runs quickly. He walks slowly!"),
    ("figures and a symbol", "Order 42 costs $19.99 + tax."),
    ("contraction", "I don't like cold coffee."),
    ("identifier", "The column user_id holds it."),
    ("one word", "Hello"),
    ("punctuation run", "Wait... really?!"),
    ("multi-byte characters", "He visited a café in Paris — twice."),
    ("astral plane", "Great work \u{1f600} everyone"),
    ("combining accents", "The manha\u{303} arrived early."),
    ("leading and trailing whitespace", "   spaced out   "),
    ("tabs and newlines", "first line\n\tsecond line\r\nthird"),
    ("whitespace only", " \t \n "),
    ("no letters at all", "### 123 ###"),
];

/// Every character index of `text` a token covers must be the token's own text, and
/// every index no token covers must be whitespace; `TokenId` must number the tokens
/// from 1 in reading order, and every token must carry a modelled tag and a
/// confidence in (0, 1].
///
/// This is the whole offset rule, asserted character by character so a document with
/// multi-byte characters cannot pass by accident.
fn assert_document_invariants(case: &str, text: &str, tokens: &[Value]) {
    let characters: Vec<char> = text.chars().collect();
    let mut covered = vec![false; characters.len()];
    let mut previous_end = 0usize;

    for (position, token) in tokens.iter().enumerate() {
        let expected_id = i64::try_from(position).expect("a test token count fits an i64") + 1;
        let token_id = token["TokenId"].as_i64().unwrap_or(-1);
        assert_eq!(
            token_id, expected_id,
            "case {case:?}: expected TokenId {expected_id} at position {position}, got \
             {token_id} in {token}"
        );

        let token_text = token["Text"].as_str().unwrap_or_default();
        assert!(
            !token_text.is_empty(),
            "case {case:?}: expected a non-empty Text, got {token}"
        );
        assert!(
            !token_text.chars().any(char::is_whitespace),
            "case {case:?}: a token is a word, not a run of whitespace, got {token}"
        );

        let begin = token["BeginOffset"].as_i64().unwrap_or(-1);
        let end = token["EndOffset"].as_i64().unwrap_or(-1);
        assert!(
            begin >= 0 && end > begin,
            "case {case:?}: expected 0 <= BeginOffset < EndOffset, got {token}"
        );
        let begin = usize::try_from(begin).expect("a non-negative offset fits a usize");
        let end = usize::try_from(end).expect("a non-negative offset fits a usize");
        assert!(
            end <= characters.len(),
            "case {case:?}: EndOffset {end} is past the document's {} characters in {token}",
            characters.len()
        );
        assert!(
            begin >= previous_end,
            "case {case:?}: tokens must be ordered and must not overlap, got BeginOffset \
             {begin} after EndOffset {previous_end} in {token}"
        );
        previous_end = end;

        let sliced: String = characters[begin..end].iter().collect();
        assert_eq!(
            sliced, token_text,
            "case {case:?}: the offsets must cut the token's own text out of the document, \
             got {sliced:?} at [{begin}, {end}) for {token}"
        );
        for flag in &mut covered[begin..end] {
            *flag = true;
        }

        let tag = token["PartOfSpeech"]["Tag"].as_str().unwrap_or_default();
        assert!(
            PART_OF_SPEECH_TAG_TYPE_VALUES.contains(&tag),
            "case {case:?}: {tag:?} is not a modelled PartOfSpeechTagType value, in {token}"
        );
        let score = token["PartOfSpeech"]["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a PartOfSpeech.Score in (0, 1], got {score} in {token}"
        );
    }

    for (index, (character, is_covered)) in characters.iter().zip(&covered).enumerate() {
        assert!(
            *is_covered || character.is_whitespace(),
            "case {case:?}: character {character:?} at index {index} belongs to no token, so \
             it must be whitespace, in {text:?}"
        );
    }
}

/// `ResultList` must hold one entry per submitted document, sorted ascending by
/// `Index` and matching the order of `TextList`, with an empty `ErrorList`; the tokens
/// of each entry must satisfy the offset rule. Returns the per-document token arrays.
fn assert_batch_invariants(case: &str, texts: &[String], output: &Value) -> Vec<Vec<Value>> {
    let results = output["ResultList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResultList array, got {output}"));
    assert_eq!(
        results.len(),
        texts.len(),
        "case {case:?}: expected one result per submitted document, got {output}"
    );
    assert_eq!(
        output["ErrorList"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: every document in an accepted batch is analysed, so ErrorList \
         must be empty, got {output}"
    );

    let mut per_document = Vec::new();
    let mut previous_index = -1i64;
    for (position, (result, text)) in results.iter().zip(texts).enumerate() {
        let index = result["Index"].as_i64().unwrap_or(-1);
        assert_eq!(
            index,
            i64::try_from(position).expect("a test batch size fits an i64"),
            "case {case:?}: expected the result at position {position} to carry Index \
             {position}, got {result}"
        );
        assert!(
            index > previous_index,
            "case {case:?}: ResultList is not sorted ascending by Index, got {index} \
             after {previous_index} in {output}"
        );
        previous_index = index;

        let tokens = result["SyntaxTokens"].as_array().cloned().unwrap_or_default();
        assert_document_invariants(&format!("{case} / document {position}"), text, &tokens);
        per_document.push(tokens);
    }
    per_document
}

/// The batch built by taking `size` documents from [`DOCUMENTS`], cycling when the
/// batch is longer than the table.
fn batch_of(size: usize) -> Vec<String> {
    (0..size)
        .map(|position| DOCUMENTS[position % DOCUMENTS.len()].1.to_string())
        .collect()
}

#[tokio::test]
async fn every_document_gets_one_result_of_tokens_carrying_its_own_index() {
    let comprehend = Comprehend::new();

    // Batch sizes spanning the modelled range: the `@length` floor of 1, a pair, a
    // batch longer than the document table, and the documented ceiling of 25.
    for size in [1usize, 2, 7, MAX_BATCH_DOCUMENTS] {
        let case = format!("{size} documents");
        let texts = batch_of(size);
        let output = comprehend
            .ok(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        assert_batch_invariants(&case, &texts, &output);
    }
}

#[tokio::test]
async fn a_document_with_words_is_tokenized_and_one_without_is_not() {
    let comprehend = Comprehend::new();

    for (case, text) in DOCUMENTS {
        let texts = vec![(*text).to_string()];
        let output = comprehend
            .ok(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        let tokens = assert_batch_invariants(case, &texts, &output);

        // A document carrying a word must produce at least one token; one carrying
        // nothing but whitespace must produce none rather than an invented one.
        let expected_tokens = text.chars().any(|character| !character.is_whitespace());
        assert_eq!(
            !tokens[0].is_empty(),
            expected_tokens,
            "case {case:?}: expected tokens only for a document with content, got {output}"
        );
    }
}

#[tokio::test]
async fn a_batched_document_is_tagged_exactly_as_the_single_document_operation_tags_it() {
    let comprehend = Comprehend::new();

    let texts: Vec<String> = DOCUMENTS
        .iter()
        // `DetectSyntax` models a `@length` floor of 1 on `Text`, and the
        // whitespace-only row is accepted by both, so every row can travel through
        // both operations.
        .map(|(_, text)| (*text).to_string())
        .collect();
    let batched = comprehend
        .ok(
            "BatchDetectSyntax",
            &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
        )
        .await;
    let per_document = assert_batch_invariants("every row in one batch", &texts, &batched);

    for (position, (case, text)) in DOCUMENTS.iter().enumerate() {
        let single = comprehend
            .ok(
                "DetectSyntax",
                &json!({ "Text": text, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        let expected = single["SyntaxTokens"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            per_document[position], expected,
            "case {case:?}: the batched tokens must be the tokens DetectSyntax reports \
             for the same document, got {:?} against {expected:?}",
            per_document[position]
        );
    }
}

#[tokio::test]
async fn results_follow_the_order_the_documents_were_submitted_in() {
    let comprehend = Comprehend::new();

    // The same documents submitted in different orders. If a result were keyed to
    // anything but its position, at least one of these orderings would disagree with
    // the batch it was built from.
    let orderings: [(&str, Vec<usize>); 4] = [
        ("table order", (0..DOCUMENTS.len()).collect()),
        ("reversed", (0..DOCUMENTS.len()).rev().collect()),
        ("rotated by one", (1..DOCUMENTS.len()).chain(0..1).collect()),
        ("one document repeated three times", vec![1, 1, 1]),
    ];

    // The tokens each document comes back with when it is the only one in the batch:
    // the expectation every ordering is checked against.
    let mut alone = Vec::new();
    for (case, text) in DOCUMENTS {
        let texts = vec![(*text).to_string()];
        let output = comprehend
            .ok(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        let tokens = assert_batch_invariants(case, &texts, &output);
        alone.push(tokens.into_iter().next().unwrap_or_default());
    }

    for (case, positions) in orderings {
        let texts: Vec<String> = positions
            .iter()
            .map(|row| DOCUMENTS[*row].1.to_string())
            .collect();
        let output = comprehend
            .ok(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        let per_document = assert_batch_invariants(case, &texts, &output);

        for (position, row) in positions.iter().enumerate() {
            assert_eq!(
                per_document[position], alone[*row],
                "case {case:?}: the document at position {position} must be analysed as \
                 it is on its own, got {:?} against {:?}",
                per_document[position], alone[*row]
            );
        }
    }
}

#[tokio::test]
async fn a_batch_at_the_documented_document_count_is_accepted_and_one_past_it_is_not() {
    let comprehend = Comprehend::new();

    for count in [1usize, MAX_BATCH_DOCUMENTS, MAX_BATCH_DOCUMENTS + 1] {
        let texts = batch_of(count);
        let (status, response) = comprehend
            .call(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;

        if count > MAX_BATCH_DOCUMENTS {
            assert_eq!(
                status, 400,
                "case \"{count} documents\": expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "BatchSizeLimitExceededException",
                "case \"{count} documents\": expected BatchSizeLimitExceededException, \
                 got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case \"{count} documents\": expected 200, got {status}: {response}"
            );
            assert_batch_invariants(&format!("{count} documents"), &texts, &response);
        }
    }
}

#[tokio::test]
async fn a_batch_whose_documents_all_fit_the_byte_ceiling_is_tagged_whole() {
    let comprehend = Comprehend::new();
    let short = DOCUMENTS[0].1.to_string();
    let at_ceiling = "a ".repeat(MAX_DOCUMENT_BYTES / 2);
    let past_ceiling = "a".repeat(MAX_DOCUMENT_BYTES + 1);

    // The ceiling is measured per document and enforced over the REQUEST: a batch
    // whose documents all fit is tagged in full, and a batch carrying one past it is
    // refused whole rather than answered with a per-index error. Rows differ in
    // document size, in batch size and in where the oversized document sits.
    let cases: Vec<(&str, Vec<String>, bool)> = vec![
        ("a document exactly at the ceiling", vec![at_ceiling.clone()], true),
        (
            "a document at the ceiling beside a short one",
            vec![short.clone(), at_ceiling.clone()],
            true,
        ),
        (
            "one document, one byte past it",
            vec![past_ceiling.clone()],
            false,
        ),
        (
            "an oversized document first",
            vec![past_ceiling.clone(), short.clone(), at_ceiling.clone()],
            false,
        ),
        (
            "an oversized document in the middle",
            vec![short.clone(), past_ceiling.clone(), short.clone()],
            false,
        ),
    ];

    for (case, texts, accepted) in cases {
        assert_eq!(
            at_ceiling.len(),
            MAX_DOCUMENT_BYTES,
            "case {case:?}: the at-ceiling document must be exactly {MAX_DOCUMENT_BYTES} \
             bytes, got {}",
            at_ceiling.len()
        );
        let (status, response) = comprehend
            .call(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: every document fits the ceiling, expected 200, got \
                 {status}: {response}"
            );
            assert_batch_invariants(case, &texts, &response);
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a batch carrying an oversized document is refused whole, \
                 got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TextSizeLimitExceededException",
                "case {case:?}: expected TextSizeLimitExceededException, got {response}"
            );
            assert!(
                response.get("ResultList").is_none(),
                "case {case:?}: a refused batch carries no partial ResultList, got \
                 {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_language_the_tagger_cannot_read_is_refused_for_the_whole_batch() {
    let comprehend = Comprehend::new();

    // The catalog is the model's own `SyntaxLanguageCode` enum, so the one value the
    // tagger reads and every value it refuses are both covered.
    for code in SYNTAX_LANGUAGE_CODE_VALUES {
        let (status, response) = comprehend
            .call(
                "BatchDetectSyntax",
                &json!({ "TextList": [DOCUMENTS[0].1, DOCUMENTS[1].1], "LanguageCode": code }),
            )
            .await;

        if *code == TAGGABLE_LANGUAGE {
            assert_eq!(
                status, 200,
                "case {code:?}: the tagger reads this language, expected 200, got \
                 {status}: {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "UnsupportedLanguageException",
                "case {code:?}: expected UnsupportedLanguageException, got {response}"
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
            json!({ "TextList": ["The dog barks loudly.", ""], "LanguageCode": "en" }),
        ),
        (
            "LanguageCode outside the modelled enum",
            json!({ "TextList": ["The dog barks loudly."], "LanguageCode": "english" }),
        ),
        (
            // A language Comprehend supports for OTHER operations but the
            // `SyntaxLanguageCode` enum does not name.
            "LanguageCode of another operation's enum",
            json!({ "TextList": ["The dog barks loudly."], "LanguageCode": "ja" }),
        ),
        (
            "absent LanguageCode",
            json!({ "TextList": ["The dog barks loudly."] }),
        ),
    ] {
        let (status, response) = comprehend.call("BatchDetectSyntax", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

#[tokio::test]
async fn repeating_a_batch_returns_the_same_results() {
    let comprehend = Comprehend::new();
    let body = json!({
        "TextList": [DOCUMENTS[0].1, DOCUMENTS[1].1],
        "LanguageCode": TAGGABLE_LANGUAGE,
    });

    let first = comprehend.ok("BatchDetectSyntax", &body).await;
    let second = comprehend.ok("BatchDetectSyntax", &body).await;
    assert_eq!(
        first, second,
        "case \"repeated identical batch\": expected identical results, got {first} \
         then {second}"
    );
}

/// The alphabet the exhaustive sweep builds its documents from: words, numbers,
/// punctuation, symbols, whitespace, a combining accent and an astral-plane
/// character, so a generated document mixes byte widths with token and whitespace
/// boundaries.
const FRAGMENTS: &[&str] = &[
    "",
    "The",
    "dog",
    "café",
    "don't",
    "user_id",
    " ",
    "\t\n",
    ".",
    "?!...",
    "42",
    "19.99",
    "$",
    "a\u{303}",
    "\u{1f600}",
];

#[tokio::test]
async fn offsets_cut_the_token_out_of_the_document_for_every_fragment_triple() {
    let comprehend = Comprehend::new();

    // Every ordered triple of the alphabet: the whole product, not a sample, so this
    // sweep is reproducible without a seed.
    let mut documents = Vec::new();
    for first in FRAGMENTS {
        for second in FRAGMENTS {
            for third in FRAGMENTS {
                documents.push(format!("{first}{second}{third}"));
            }
        }
    }
    // A document must satisfy the modelled `@length` floor of 1 on the member.
    documents.retain(|document| !document.is_empty());
    assert!(
        documents.len() > 1_000,
        "the sweep must span the whole fragment product, got {} documents",
        documents.len()
    );

    for (batch_index, batch) in documents.chunks(MAX_BATCH_DOCUMENTS).enumerate() {
        let texts = batch.to_vec();
        let output = comprehend
            .ok(
                "BatchDetectSyntax",
                &json!({ "TextList": texts, "LanguageCode": TAGGABLE_LANGUAGE }),
            )
            .await;
        assert_batch_invariants(
            &format!("fragment sweep batch {batch_index}"),
            &texts,
            &output,
        );
    }
}

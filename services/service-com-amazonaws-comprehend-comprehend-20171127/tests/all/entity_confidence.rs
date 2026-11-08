// SPDX-License-Identifier: CC0-1.0

//! What `BatchDetectEntities` reports for a detection's `Score`, as a convention
//! rather than as one recorded answer.
//!
//! The modelled `Entity$Score` is "the level of confidence that Amazon Comprehend has
//! in the accuracy of the detection", so it belongs to THAT detection: it is weighed
//! over the characters of the mention the recogniser's evidence accounts for and over
//! what the rest of the document says about the same mention, never read out of a
//! table keyed by the category or the rule class the detection ended up in. Four
//! consequences, each asserted over a table whose rows differ along what the rule
//! generalises over:
//!
//! 1. Two mentions of the SAME type in ONE document, found by the SAME rule class,
//!    come back with two confidences — a score belonging to the type or to the rule
//!    could not tell them apart.
//! 2. The more of the mention's characters the evidence accounts for, the higher the
//!    confidence: a name the lexicon spells out at length, a calendar form that fixes
//!    more fields, a designator written out rather than abbreviated.
//! 3. A mention the document writes again, read the same way, is corroborated by that
//!    second writing, so it is reported above the same mention written once.
//! 4. The confidence is a function of the document alone: a document answered inside
//!    a batch is answered exactly as the single-document operation answers it, and
//!    repeating either call returns the same values.
//!
//! Every row rides in one batch, so each is answered as its own `ResultList` entry
//! beside the others, exactly as the recorded three-document request's were.

use serde_json::{json, Value};

use crate::batch_inference_calibration::{batch_results, distinct};
use crate::comprehend_client::Comprehend;

/// Every entity each document of a batch was answered with, in document order.
async fn batched_entities(comprehend: &Comprehend, documents: &[&str]) -> Vec<Vec<Value>> {
    let results = batch_results(
        comprehend,
        "BatchDetectEntities",
        &json!({ "TextList": documents, "LanguageCode": "en" }),
    )
    .await;
    results
        .iter()
        .map(|result| result["Entities"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// The `Score` the answer gives the entity whose `Text` is `mention`, asserted to be
/// a confidence (strictly inside `(0, 1)`: a rule-and-lexicon recogniser never
/// excludes the reading in which the span names nothing) and to carry `entity_type`.
fn score_of(case: &str, found: &[Value], mention: &str, entity_type: &str) -> f64 {
    let matching: Vec<&Value> = found
        .iter()
        .filter(|entity| entity["Text"].as_str() == Some(mention))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one entity reading {mention:?}, got {found:?}"
    );
    let entity = matching[0];
    assert_eq!(
        entity["Type"].as_str(),
        Some(entity_type),
        "case {case:?}: expected {mention:?} to be typed {entity_type}, got {entity}"
    );
    let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
    assert!(
        score > 0.0 && score < 1.0,
        "case {case:?}: expected a Score strictly inside (0, 1) for {mention:?}, got {score} \
         in {entity}"
    );
    score
}

// ---------------------------------------------------------------------------
// 1 and 2. Two mentions of one type in one document
// ---------------------------------------------------------------------------

/// `(case, document, Type, the mention whose evidence accounts for more of it, the
/// other mention, why the first one leaves less open)`.
///
/// Each row holds TWO mentions of one type, found by one rule class, in one document,
/// so a confidence read out of the type or out of the rule would answer both the
/// same. The rows differ along the KIND of evidence behind the pair — a lexicon entry
/// spelling the name out, two closed calendar forms, a legal-form designator inside
/// the name, a unit carrying a measurement, an honorific standing in front of a name —
/// and inside each pair along how much of the mention that evidence accounts for.
type Pair = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const PAIRS: &[Pair] = &[
    (
        "two names the place lexicon spells out",
        "They expanded into Portugal and Chile.",
        "LOCATION",
        "Portugal",
        "Chile",
        "a longer spelling the lexicon names is less likely to be a coincidence than a \
         shorter one",
    ),
    (
        "two closed calendar forms",
        "The outage ran from 2024-01-15 to January 20, 2024.",
        "DATE",
        "January 20, 2024",
        "2024-01-15",
        "the form written with its month name fixes more characters than the all-numeric \
         one",
    ),
    (
        "a legal-form designator written out against its abbreviation",
        "Globex Corporation filed the papers. Acme Widgets LLC paid the fee.",
        "ORGANIZATION",
        "Globex Corporation",
        "Acme Widgets LLC",
        "a designator spelled out accounts for more of the name than a three-character \
         abbreviation of it",
    ),
    (
        "two measurements carrying their units",
        "The parcel weighs 12 kg and the crate weighs 1,250 kilograms.",
        "QUANTITY",
        "1,250 kilograms",
        "12 kg",
        "the amount written with its scale and its unit spelled out fixes more characters \
         than the short form",
    ),
    (
        "two names an honorific announces",
        "Dr. Alexandra Rutherford and Dr. Jane Doe reviewed the report.",
        "PERSON",
        "Jane Doe",
        "Alexandra Rutherford",
        "the honorific names the head of the name, which is more of the shorter mention \
         than of the longer one",
    ),
];

#[tokio::test]
async fn two_mentions_of_one_type_are_scored_from_their_own_evidence() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = PAIRS.iter().map(|(_, document, ..)| *document).collect();
    let answers = batched_entities(&comprehend, &documents).await;

    let mut every_score: Vec<f64> = Vec::new();
    for (position, (case, _, entity_type, stronger, weaker, reason)) in PAIRS.iter().enumerate() {
        let found = &answers[position];
        let stronger_score = score_of(case, found, stronger, entity_type);
        let weaker_score = score_of(case, found, weaker, entity_type);

        // 1. Two detections of one type in one document, two confidences.
        assert!(
            (stronger_score - weaker_score).abs() > 1e-6,
            "case {case:?}: expected {stronger:?} and {weaker:?} to be scored from their own \
             evidence, got {stronger_score} and {weaker_score}"
        );
        // 2. More of the mention accounted for, more confidence.
        assert!(
            stronger_score > weaker_score,
            "case {case:?}: expected {stronger:?} to be scored above {weaker:?} ({reason}), \
             got {stronger_score} against {weaker_score}"
        );
        every_score.push(stronger_score);
        every_score.push(weaker_score);
    }

    // No ladder: ten detections spanning five kinds of evidence take ten values.
    let spread = distinct(every_score.iter().copied());
    assert_eq!(
        spread,
        every_score.len(),
        "expected every detection to carry its own confidence, got {spread} distinct values \
         across {every_score:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. The document's own corroboration of the mention
// ---------------------------------------------------------------------------

/// `(case, the document writing the mention once, the document writing it again,
/// Type, the mention)`.
///
/// The rows differ along the kind of evidence that found the mention — a lexicon
/// entry, a closed form, a designator inside the name — so the corroboration is not a
/// property of one rule class.
const REPEATED: &[(&str, &str, &str, &str, &str)] = &[
    (
        "a name the place lexicon spells out",
        "They expanded into Portugal.",
        "They expanded into Portugal and later left Portugal.",
        "LOCATION",
        "Portugal",
    ),
    (
        "a closed calendar form",
        "The outage began on 2024-01-15.",
        "The outage began on 2024-01-15 and the report is dated 2024-01-15.",
        "DATE",
        "2024-01-15",
    ),
    (
        "a name a legal-form designator types",
        "Acme Widgets LLC filed the papers.",
        "Acme Widgets LLC filed the papers, and Acme Widgets LLC paid the fee.",
        "ORGANIZATION",
        "Acme Widgets LLC",
    ),
];

#[tokio::test]
async fn a_mention_the_document_writes_again_is_scored_above_one_written_once() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = REPEATED
        .iter()
        .flat_map(|(_, once, again, _, _)| [*once, *again])
        .collect();
    let answers = batched_entities(&comprehend, &documents).await;

    for (position, (case, _, _, entity_type, mention)) in REPEATED.iter().enumerate() {
        let written_once = score_of(case, &answers[position * 2], mention, entity_type);
        // The repeated mention is reported twice, and the corroboration belongs to the
        // document, so both writings carry it.
        let repeated = &answers[position * 2 + 1];
        let writings: Vec<f64> = repeated
            .iter()
            .filter(|entity| entity["Text"].as_str() == Some(*mention))
            .map(|entity| entity["Score"].as_f64().unwrap_or(f64::NAN))
            .collect();
        assert_eq!(
            writings.len(),
            2,
            "case {case:?}: expected {mention:?} to be reported at both of its writings, got \
             {repeated:?}"
        );
        assert_eq!(
            distinct(writings.iter().copied()),
            1,
            "case {case:?}: the corroboration belongs to the document, so expected both \
             writings of {mention:?} to carry one confidence, got {writings:?}"
        );
        assert!(
            writings[0] > written_once,
            "case {case:?}: expected {mention:?} written twice to be scored above the same \
             mention written once, got {} against {written_once}",
            writings[0]
        );
        assert!(
            writings[0] < 1.0,
            "case {case:?}: a corroborated reading is still a confidence, so expected a Score \
             below 1, got {}",
            writings[0]
        );
    }
}

// ---------------------------------------------------------------------------
// 4. The confidence is a function of the document alone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batched_document_is_scored_exactly_as_the_single_document_operation_scores_it() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = PAIRS
        .iter()
        .map(|(_, document, ..)| *document)
        .chain(REPEATED.iter().map(|(_, _, again, _, _)| *again))
        .collect();
    let batched = batched_entities(&comprehend, &documents).await;
    // Repeating the batch returns the same answer: nothing about a detection depends
    // on how often it has been asked for.
    let repeated = batched_entities(&comprehend, &documents).await;

    for (position, document) in documents.iter().enumerate() {
        let case = format!("document at index {position}");
        assert_eq!(
            batched[position], repeated[position],
            "case {case:?}: expected a repeated batch to answer {document:?} identically, got \
             {:?} then {:?}",
            batched[position], repeated[position]
        );

        let alone = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": document, "LanguageCode": "en" }),
            )
            .await;
        let alone = alone["Entities"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            batched[position], alone,
            "case {case:?}: a batch answers each document on its own, so expected {document:?} \
             to come back exactly as DetectEntities answers it, got {:?} against {alone:?}",
            batched[position]
        );
    }
}

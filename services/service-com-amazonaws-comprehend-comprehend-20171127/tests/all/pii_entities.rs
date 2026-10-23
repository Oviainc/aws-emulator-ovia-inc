// SPDX-License-Identifier: CC0-1.0

//! Comprehend's real-time PII surface: the TWO projections of its one recognizer.
//!
//! `DetectPiiEntities` answers the entities a document carries and
//! `ContainsPiiEntities` answers the types it carries, and both project the SAME
//! spans from `ops::contains_pii_entities::detect_pii_spans`. The rule asserted here
//! is that agreement: for any document, the set of `Entities[].Type` the one answers
//! is exactly the set of `Labels[].Name` the other answers, and every type either
//! reports is a modelled `PiiEntityType`. Neither operation can recognise a value the
//! other misses, so a rule added to the recognizer reaches both.
//!
//! This drives two operations, so it belongs beside neither: the operations'
//! own behaviour is asserted in `detect_pii_entities.rs` and `pii_detection.rs`. The
//! document table is the one in `detect_pii_entities.rs`, read here rather than
//! copied, so the two suites cannot drift apart on what a document carries.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::PII_ENTITY_TYPE_VALUES;

use crate::comprehend_client::Comprehend;
use crate::detect_pii_entities::{entities, DOCUMENTS};

/// The distinct `Type` values of a `DetectPiiEntities` response, each checked to be a
/// value of the modelled enum.
fn detected_types(case: &str, output: &Value) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    for entity in entities(case, output) {
        let reported = entity["Type"].as_str().unwrap_or_default().to_string();
        assert!(
            PII_ENTITY_TYPE_VALUES.contains(&reported.as_str()),
            "case {case:?}: {reported:?} is not a modelled PiiEntityType value, got {entity}"
        );
        types.insert(reported);
    }
    types
}

/// The distinct `Name` values of a `ContainsPiiEntities` response, each checked to be
/// a value of the modelled enum.
fn labelled_types(case: &str, output: &Value) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    for label in output["Labels"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {output}"))
    {
        let reported = label["Name"].as_str().unwrap_or_default().to_string();
        assert!(
            PII_ENTITY_TYPE_VALUES.contains(&reported.as_str()),
            "case {case:?}: {reported:?} is not a modelled PiiEntityType value, got {label}"
        );
        types.insert(reported);
    }
    types
}

#[tokio::test]
async fn both_projections_of_the_recognizer_answer_the_same_types_for_a_document() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in DOCUMENTS {
        let body = json!({ "Text": text, "LanguageCode": "en" });
        let detected = detected_types(
            case,
            &comprehend.ok("DetectPiiEntities", &body).await,
        );
        let labelled = labelled_types(
            case,
            &comprehend.ok("ContainsPiiEntities", &body).await,
        );

        assert_eq!(
            detected, labelled,
            "case {case:?}: the two projections of the one recognizer must answer the \
             same types, got {detected:?} detected and {labelled:?} labelled"
        );

        // And that shared set is the one the table says the document carries.
        let wanted: BTreeSet<String> = expected
            .iter()
            .map(|(entity_type, _)| (*entity_type).to_string())
            .collect();
        assert_eq!(
            detected, wanted,
            "case {case:?}: expected the type set {wanted:?}, got {detected:?}"
        );
    }
}

/// A document the recognizer finds nothing in is answered with an EMPTY list by both
/// operations — the member is present and empty, never absent and never a type
/// nothing found.
#[tokio::test]
async fn a_document_with_no_pii_is_answered_with_an_empty_list_by_both() {
    let comprehend = Comprehend::new();
    let body = json!({
        "Text": "The quarterly report is attached for your review.",
        "LanguageCode": "en",
    });

    let detected = comprehend.ok("DetectPiiEntities", &body).await;
    assert_eq!(
        detected["Entities"],
        json!([]),
        "case \"clean prose\": expected an empty Entities list, got {detected}"
    );

    let labelled = comprehend.ok("ContainsPiiEntities", &body).await;
    assert_eq!(
        labelled["Labels"],
        json!([]),
        "case \"clean prose\": expected an empty Labels list, got {labelled}"
    );
}

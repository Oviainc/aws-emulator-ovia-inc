// SPDX-License-Identifier: CC0-1.0

//! What the PII recognizer types a value the document itself LABELS as.
//!
//! RULE: where the text names what a value is, that label is the evidence about THAT
//! value, so the span is typed as the label names it — against a rule that claimed
//! exactly the same characters on their form or issuance range alone. A form or a
//! range is evidence about values of a kind, and cannot overrule a document that says
//! what its own value is; where nothing labels the value, the form is all there is and
//! types it.
//!
//! The recognizer is the one rule set of the PII family
//! (`ops::contains_pii_entities::detect_pii_spans`), so the rule is asserted through
//! `DetectPiiEntities` — whose modelled `PiiEntity{Score, Type, BeginOffset,
//! EndOffset}` carries the span — and through `ContainsPiiEntities`, which projects
//! the same spans into the set of types.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::PII_ENTITY_TYPE_VALUES;

use crate::comprehend_client::Comprehend;

/// `(case, document, [(Type, the exact entity text)])`, the entities in DOCUMENT
/// ORDER.
///
/// The rows differ along what the rule generalises over: whether the document labels
/// the value at all, how many labelled values it carries, whether the labelled
/// reading and the structural reading agree or disagree, which pair of types they
/// disagree about, and which published form the label is written in. The types are
/// the model's own `PiiEntityType` enum, and each structural form is the one its
/// defining authority publishes: the SSA's issuance ranges for a social security
/// number, the IRS's middle-pair ranges for an individual taxpayer identification
/// number, and the ISO/IEC 7812-1 Luhn digit for a payment card number.
#[allow(clippy::type_complexity)]
const DOCUMENTS: &[(&str, &str, &[(&str, &str)])] = &[
    (
        // Both numbers are labelled, and the second one's digits are outside the
        // SSA's issuance ranges and inside the IRS's: the label decides both.
        "two labelled social security numbers, the second outside the issuance ranges",
        "Please update your records: primary SSN 123-45-6789, and the joint account \
         SSN is 987-65-4321.",
        &[("SSN", "123-45-6789"), ("SSN", "987-65-4321")],
    ),
    (
        "a labelled social security number whose digits are an itin form",
        "The intake form lists social security number 912-70-1234 for the applicant.",
        &[("SSN", "912-70-1234")],
    ),
    (
        // Nothing labels this one, so the published form is all the evidence there
        // is — and it is an ITIN form.
        "the same digits with nothing labelling them",
        "The filing lists 912-70-1234 on the second page.",
        &[("US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER", "912-70-1234")],
    ),
    (
        "a labelled value whose digits are also a valid payment card number",
        "Wire it to bank account number 4111111111111111 before Friday.",
        &[("BANK_ACCOUNT_NUMBER", "4111111111111111")],
    ),
    (
        "the same digits with nothing labelling them",
        "The charge went to 4111111111111111 yesterday.",
        &[("CREDIT_DEBIT_NUMBER", "4111111111111111")],
    ),
    (
        // The label and the form agree, so there is still exactly one entity.
        "a labelled value the form agrees about",
        "SSN: 123456789 is on file.",
        &[("SSN", "123456789")],
    ),
    (
        // The label is written in the recognizer's other published form, in the
        // other language the PII family reads.
        "a labelled social security number named in spanish",
        "El n\u{fa}mero de seguro social 987-65-4321 est\u{e1} en el archivo.",
        &[("SSN", "987-65-4321")],
    ),
    (
        "three labelled values of three types in document order",
        "Her SSN is 987-65-4321, the PIN 4096 and bank account number 00123456789.",
        &[
            ("SSN", "987-65-4321"),
            ("PIN", "4096"),
            ("BANK_ACCOUNT_NUMBER", "00123456789"),
        ],
    ),
    (
        "a document labelling nothing and carrying no recognisable form",
        "The quarterly report is attached for your review.",
        &[],
    ),
];

/// The `(BeginOffset, EndOffset)` the modelled members require for `needle` inside
/// `haystack`: zero-based CHARACTER offsets, `EndOffset` one past the last character.
fn expected_offsets(case: &str, haystack: &str, needle: &str) -> (i64, i64) {
    let byte_offset = haystack
        .find(needle)
        .unwrap_or_else(|| panic!("case {case:?}: {needle:?} is not in {haystack:?}"));
    let begin = haystack[..byte_offset].chars().count() as i64;
    (begin, begin + needle.chars().count() as i64)
}

#[tokio::test]
async fn a_labelled_value_is_typed_as_the_label_names_it() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in DOCUMENTS {
        let output = comprehend
            .ok(
                "DetectPiiEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found: Vec<Value> = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
            .clone();

        assert_eq!(
            found.len(),
            expected.len(),
            "case {case:?}: expected {} entities ({expected:?}), got {output}",
            expected.len()
        );

        for (entity, (expected_type, entity_text)) in found.iter().zip(expected.iter()) {
            let reported_type = entity["Type"].as_str().unwrap_or_default();
            assert!(
                PII_ENTITY_TYPE_VALUES.contains(&reported_type),
                "case {case:?}: {reported_type:?} is not a modelled PiiEntityType value"
            );
            assert_eq!(
                reported_type, *expected_type,
                "case {case:?}: expected {expected_type:?} for {entity_text:?}, got {entity}"
            );

            let (begin, end) = expected_offsets(case, text, entity_text);
            assert_eq!(
                (
                    entity["BeginOffset"].as_i64().unwrap_or(-1),
                    entity["EndOffset"].as_i64().unwrap_or(-1),
                ),
                (begin, end),
                "case {case:?}: expected the character offsets ({begin}, {end}) for \
                 {entity_text:?}, got {entity}"
            );

            let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: expected a Score in (0, 1] for {entity_text:?}, got {entity}"
            );
        }
    }
}

/// The two PII operations project the SAME spans, so a type the labelled reading
/// decides is the type BOTH of them report — a client filtering on the label's type
/// cannot see one answer from the detector and another from the classifier.
#[tokio::test]
async fn both_pii_operations_report_the_labelled_type() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in DOCUMENTS {
        let body = json!({ "Text": text, "LanguageCode": "en" });

        let mut detected: Vec<String> = comprehend
            .ok("DetectPiiEntities", &body)
            .await["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entity| entity["Type"].as_str().map(str::to_string))
            .collect();
        detected.sort();
        detected.dedup();

        let mut labelled: Vec<String> = comprehend
            .ok("ContainsPiiEntities", &body)
            .await["Labels"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|label| label["Name"].as_str().map(str::to_string))
            .collect();
        labelled.sort();
        labelled.dedup();

        let mut wanted: Vec<String> = expected
            .iter()
            .map(|(entity_type, _)| (*entity_type).to_string())
            .collect();
        wanted.sort();
        wanted.dedup();

        assert_eq!(
            detected, wanted,
            "case {case:?}: expected DetectPiiEntities to report the type set {wanted:?}, \
             got {detected:?}"
        );
        assert_eq!(
            labelled, wanted,
            "case {case:?}: expected ContainsPiiEntities to report the type set {wanted:?}, \
             got {labelled:?}"
        );
    }
}

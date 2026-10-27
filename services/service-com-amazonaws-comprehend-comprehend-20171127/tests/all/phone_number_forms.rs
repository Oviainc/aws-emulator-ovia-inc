// SPDX-License-Identifier: CC0-1.0

//! The `PHONE` rules of the shared PII recognizer: which written forms of a
//! telephone number it places, and which digit runs it does not.
//!
//! The rule asserted here: a telephone number is recognised in the forms its
//! numbering plan publishes, not only in the longest of them. The North American
//! Numbering Plan writes a national number as an area code, a central office code
//! (NXX — its first digit is 2-9) and a four-digit line number, and writes the
//! subscriber number ALONE as the office code and the line number; ITU-T E.164
//! writes the international form. Each row is one of those forms, and the rows span
//! what the rule generalises over: whether the area code is present, which separator
//! the number is written with, whether the surrounding text names the value, and
//! each side of the published bound on the office code's first digit.
//!
//! `DetectPiiEntities` answers the recognizer's spans, so a row asserts the type AND
//! the offsets, which is what tells a placed number from a lucky label. Every
//! assertion names its case and the actual value.

use serde_json::{json, Value};

use crate::comprehend_client::Comprehend;
use crate::detect_pii_entities::{entities, expected_offsets};

/// One row: a document, the candidate in it, and whether the recognizer must place a
/// `PHONE` at that candidate.
struct NumberRow {
    case: &'static str,
    text: &'static str,
    candidate: &'static str,
    is_phone: bool,
}

const NUMBER_ROWS: &[NumberRow] = &[
    NumberRow {
        case: "a three-part national number",
        text: "Reach the desk on 206-555-0147 before noon.",
        candidate: "206-555-0147",
        is_phone: true,
    },
    NumberRow {
        case: "a three-part number with a country code and brackets",
        text: "Reach the desk on +1 (206) 555-0147 before noon.",
        candidate: "+1 (206) 555-0147",
        is_phone: true,
    },
    NumberRow {
        case: "an international number in its E.164 form",
        text: "Reach the desk on +12065550147 before noon.",
        candidate: "+12065550147",
        is_phone: true,
    },
    NumberRow {
        case: "a subscriber number on its own",
        text: "Reach the desk on 555-0147 before noon.",
        candidate: "555-0147",
        is_phone: true,
    },
    NumberRow {
        case: "a subscriber number at the lowest published office code",
        text: "The night line is 200-0199 until Friday.",
        candidate: "200-0199",
        is_phone: true,
    },
    NumberRow {
        case: "a subscriber number at the highest published office code",
        text: "The night line is 999-4477 until Friday.",
        candidate: "999-4477",
        is_phone: true,
    },
    NumberRow {
        case: "a subscriber number written with a dot",
        text: "Dial 555.0188 for the warehouse.",
        candidate: "555.0188",
        is_phone: true,
    },
    NumberRow {
        case: "a subscriber number the document names as a telephone number",
        text: "Mobile: 555 0170 most afternoons.",
        candidate: "555 0170",
        is_phone: true,
    },
    // One past the published bound on the office code's first digit: N is 2-9, so a
    // run starting 0 or 1 is not a subscriber number however it is written.
    NumberRow {
        case: "a digit run whose office code starts past the published bound",
        text: "Order 112-0100 shipped today.",
        candidate: "112-0100",
        is_phone: false,
    },
    NumberRow {
        case: "a digit run whose office code starts at zero",
        text: "Order 012-3456 shipped today.",
        candidate: "012-3456",
        is_phone: false,
    },
    // The context a telephone number is named in does not widen the plan: the office
    // code's first digit is bounded whether the form or the surrounding words
    // admitted the value.
    NumberRow {
        case: "a context-named run whose office code is out of plan",
        text: "Fax: 0123456 if the line is busy.",
        candidate: "0123456",
        is_phone: false,
    },
    NumberRow {
        case: "a postal code with its add-on",
        text: "The depot is in 98101-1234 near the water.",
        candidate: "98101-1234",
        is_phone: false,
    },
    // A social security number has the same digits with another grouping, and its
    // own rule confirms the issuance rules, so it stays an SSN rather than becoming
    // a telephone number.
    NumberRow {
        case: "a social security number",
        text: "The form lists 123-45-6789 on the second page.",
        candidate: "123-45-6789",
        is_phone: false,
    },
];

/// The `PHONE` entities of a response, in the order it returned them.
fn phones(case: &str, output: &Value) -> Vec<Value> {
    entities(case, output)
        .into_iter()
        .filter(|entity| entity["Type"] == json!("PHONE"))
        .collect()
}

#[tokio::test]
async fn detect_pii_entities_places_a_telephone_number_in_every_published_form() {
    let comprehend = Comprehend::new();

    for row in NUMBER_ROWS {
        let case = row.case;
        let output = comprehend
            .ok(
                "DetectPiiEntities",
                &json!({ "Text": row.text, "LanguageCode": "en" }),
            )
            .await;
        let found = phones(case, &output);

        if !row.is_phone {
            assert!(
                found.is_empty(),
                "case {case:?}: {:?} is not a telephone number, so no PHONE may be \
                 placed, got {output}",
                row.candidate
            );
            continue;
        }

        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly one PHONE for {:?}, got {output}",
            row.candidate
        );
        let (begin, end) = expected_offsets(case, row.text, row.candidate);
        assert_eq!(
            (
                found[0]["BeginOffset"].as_i64(),
                found[0]["EndOffset"].as_i64()
            ),
            (Some(begin), Some(end)),
            "case {case:?}: expected the offsets {begin}..{end} of {:?}, got {}",
            row.candidate,
            found[0]
        );
        let score = found[0]["Score"].as_f64().unwrap_or(-1.0);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a Score in (0, 1], got {score} in {}",
            found[0]
        );
    }
}

/// The two operations of the PII family read the ONE recognizer, so a form
/// `DetectPiiEntities` places is also a label `ContainsPiiEntities` answers, and a
/// run it does not place is labelled by neither.
#[tokio::test]
async fn contains_pii_entities_labels_the_same_forms() {
    let comprehend = Comprehend::new();

    for row in NUMBER_ROWS {
        let case = row.case;
        let output = comprehend
            .ok(
                "ContainsPiiEntities",
                &json!({ "Text": row.text, "LanguageCode": "en" }),
            )
            .await;
        let labelled = output["Labels"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Labels array, got {output}"))
            .iter()
            .any(|label| label["Name"] == json!("PHONE"));
        assert_eq!(
            labelled, row.is_phone,
            "case {case:?}: expected the PHONE label to be present={}, got {output}",
            row.is_phone
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! How `DetectEntities` types a PERSONAL NAME the recogniser's lexicons do not carry.
//!
//! RULE: a capitalized multi-token span written the way English writes a personal
//! name — the first and last token capitalized, every token either capitalized or one
//! of the particles a name carries, and no article in front of it — is typed `PERSON`
//! when it stands where English puts a person: in the subject position of a verb whose
//! subject is human, or in the participant position such a verb marks with a
//! preposition. The name itself is NOT what decides it, so the rule holds for a name
//! no given-name list carries; and a span without that position, or one a lexicon
//! already names, is typed by the evidence it does carry instead.
//!
//! Every row below is asserted over the one modelled contract
//! (`smithy_query(query="operation",operation="DetectEntities")`): `Entities` of
//! `Entity{Score, Type, Text, BeginOffset, EndOffset}`, whose offsets delimit exactly
//! the entity's own `Text`.

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::ENTITY_TYPE_VALUES;

use crate::comprehend_client::Comprehend;

/// `(case, document, expected Type, expected Text)` — the entity the rule requires
/// the document to carry.
///
/// The rows differ along what the rule generalises over: which syntactic position
/// puts the person there (subject, object, passive agent, addressee), how many tokens
/// the name spans (two, three with a middle initial, three with a name particle),
/// where in the sentence it sits, and whether the evidence is present at all — the
/// negative rows carry the same name shape with the position or the orthography
/// missing, and are typed by what they do carry. None of the names is a member of the
/// recogniser's given-name lexicon
/// (`src/ops/detect_entities.rs`, `GIVEN_NAMES`), so no row is typed by that list.
const DOCUMENTS: &[(&str, &str, &str, &str)] = &[
    (
        "name as the agent of a passive verb",
        "The lab was founded by Thandiwe Okafor in 1998.",
        "PERSON",
        "Thandiwe Okafor",
    ),
    (
        "name as the subject of a verb of saying",
        "Rashida Verlinden said the rollout was complete.",
        "PERSON",
        "Rashida Verlinden",
    ),
    (
        "name as the object of a verb of appointment",
        "The committee appointed Soren Haugland to the seat.",
        "PERSON",
        "Soren Haugland",
    ),
    (
        "name as the addressee a preposition marks",
        "The auditors spoke to Mira Kalinowski about the invoice.",
        "PERSON",
        "Mira Kalinowski",
    ),
    (
        "name written with a middle initial",
        "Lars E. Nyqvist signed the lease.",
        "PERSON",
        "Lars E. Nyqvist",
    ),
    (
        "name written with a lowercase particle",
        "Annika van Hoorn met the inspectors.",
        "PERSON",
        "Annika van Hoorn",
    ),
    (
        "name in a person position late in the sentence",
        "After the review the auditors interviewed Baldur Jonsdottir.",
        "PERSON",
        "Baldur Jonsdottir",
    ),
    (
        // The position is part of the evidence: the same name shape beside a verb an
        // institution fills as readily as a person says nothing about what it names.
        "name shape beside a verb that does not select a person",
        "Zorblax Initiative published its findings.",
        "OTHER",
        "Zorblax Initiative",
    ),
    (
        // A personal name in English is not written with an article.
        "capitalized phrase written with an article",
        "The Thandiwe Okafor Fellowship closed yesterday.",
        "OTHER",
        "Thandiwe Okafor Fellowship",
    ),
    (
        // One token is not a name shape, whatever stands beside it.
        "single capitalized token in a person position",
        "The study was reviewed by Larkspur.",
        "OTHER",
        "Larkspur",
    ),
    (
        // A lexicon entry naming the span says WHAT it is, which outranks the shape.
        "lexicon entry in a person position",
        "The study was reviewed by Harvard.",
        "ORGANIZATION",
        "Harvard",
    ),
    (
        // A designator inside the span says what it is, for the same reason.
        "designator inside a span in a person position",
        "The grant was reviewed by Prairie Hill University.",
        "ORGANIZATION",
        "Prairie Hill University",
    ),
];

/// Assert the invariants every entity carries: a modelled type, a confidence in
/// (0, 1], and character offsets delimiting exactly its `Text`, in document order.
fn assert_entity_invariants(case: &str, text: &str, found: &[Value]) {
    let mut previous_end = 0i64;
    for entity in found {
        let entity_type = entity["Type"].as_str().unwrap_or_default();
        assert!(
            ENTITY_TYPE_VALUES.contains(&entity_type),
            "case {case:?}: {entity_type:?} is not a modelled EntityType value, in {entity}"
        );
        let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
        assert!(
            score > 0.0 && score <= 1.0,
            "case {case:?}: expected a Score in (0, 1], got {score} in {entity}"
        );
        let begin = entity["BeginOffset"].as_i64().unwrap_or(-1);
        let end = entity["EndOffset"].as_i64().unwrap_or(-1);
        assert!(
            begin >= previous_end && end > begin,
            "case {case:?}: expected ordered, non-overlapping offsets, got {begin}..{end} \
             after an entity ending at {previous_end} in {found:?}"
        );
        let slice: String = text
            .chars()
            .skip(usize::try_from(begin).unwrap_or_default())
            .take(usize::try_from(end - begin).unwrap_or_default())
            .collect();
        assert_eq!(
            slice,
            entity["Text"].as_str().unwrap_or_default(),
            "case {case:?}: the characters {begin}..{end} are {slice:?}, which is not the \
             entity's Text in {entity}"
        );
        previous_end = end;
    }
}

#[tokio::test]
async fn a_personal_name_in_a_person_bearing_position_is_typed_person() {
    let comprehend = Comprehend::new();

    for (case, text, expected_type, expected_text) in DOCUMENTS {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": text, "LanguageCode": "en" }),
            )
            .await;
        let found: Vec<Value> = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
            .clone();
        assert_entity_invariants(case, text, &found);

        let matching: Vec<&Value> = found
            .iter()
            .filter(|entity| entity["Text"].as_str() == Some(*expected_text))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one entity reading {expected_text:?}, got {found:?}"
        );
        let entity_type = matching[0]["Type"].as_str().unwrap_or_default();
        assert_eq!(
            entity_type, *expected_type,
            "case {case:?}: expected {expected_text:?} to be typed {expected_type}, got \
             {entity_type} in {:?}",
            matching[0]
        );
    }
}

/// The rule is about the POSITION, not about the document: the same name is typed
/// `PERSON` in every position that puts a person there, and is left to the catch-all
/// in a sentence that gives it none — so no row of the table above is passing on the
/// strength of the name it happens to use.
#[tokio::test]
async fn the_same_name_is_typed_from_its_position_in_each_sentence() {
    let comprehend = Comprehend::new();
    let name = "Oyelaran Fitzwilliam";

    for (case, text, expected_type) in [
        ("subject of a verb of saying", format!("{name} told the board about the delay."), "PERSON"),
        ("object of a verb of hiring", format!("The firm hired {name} last spring."), "PERSON"),
        ("agent of a passive verb", format!("The report was written by {name}."), "PERSON"),
        ("co-participant a preposition marks", format!("The chair met with {name} on Friday."), "PERSON"),
        (
            "no verb that selects a person anywhere near it",
            format!("Our records list {name}."),
            "OTHER",
        ),
    ] {
        let output = comprehend
            .ok(
                "DetectEntities",
                &json!({ "Text": &text, "LanguageCode": "en" }),
            )
            .await;
        let found: Vec<Value> = output["Entities"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
            .clone();
        assert_entity_invariants(case, &text, &found);

        let reported = found
            .iter()
            .find(|entity| entity["Text"].as_str() == Some(name))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an entity reading {name:?}, got {found:?}")
            });
        assert_eq!(
            reported["Type"].as_str(),
            Some(expected_type),
            "case {case:?}: expected {name:?} to be typed {expected_type}, got {reported}"
        );
    }
}

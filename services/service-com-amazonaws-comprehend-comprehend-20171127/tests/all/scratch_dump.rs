// SPDX-License-Identifier: CC0-1.0

//! Documents written from scratch for this suite, and a dump of the recogniser's
//! FULL answer for each one — every entity, in order, with its type, its text and
//! both offsets — pinned as a baseline.
//!
//! `entity_detection.rs` asserts the rule each class follows one class at a time;
//! this one pins what the rules add up to on a whole document, so a change to one
//! rule that quietly changes another class's answer (a span swallowing its
//! neighbour, an entity dropped by overlap resolution, an offset shifted by a
//! multi-token name) fails here rather than silently.

use serde_json::{json, Value};

use crate::comprehend_client::Comprehend;

/// `(case, document, [(Type, Text, BeginOffset, EndOffset)])` — the whole answer.
type Baseline = (&'static str, &'static str, &'static [(&'static str, &'static str, i64, i64)]);

const BASELINES: &[Baseline] = &[
    (
        "a person, an employer, a city and a salary",
        "John Smith works at Amazon in Seattle and earns $100,000 per year.",
        &[
            ("PERSON", "John Smith", 0, 10),
            ("ORGANIZATION", "Amazon", 20, 26),
            ("LOCATION", "Seattle", 30, 37),
            ("QUANTITY", "$100,000", 48, 56),
        ],
    ),
    (
        "a company, a year, two founders joined by \"and\", and two places",
        "Microsoft Corporation was founded in 1975 by Bill Gates and Paul Allen in \
         Albuquerque, New Mexico.",
        &[
            ("ORGANIZATION", "Microsoft Corporation", 0, 21),
            ("DATE", "1975", 37, 41),
            ("PERSON", "Bill Gates", 45, 55),
            ("PERSON", "Paul Allen", 60, 70),
            ("LOCATION", "Albuquerque", 74, 85),
            ("LOCATION", "New Mexico", 87, 97),
        ],
    ),
    (
        "a person, a landmark, two places and a date written day-month-year",
        "Jane Doe visited the Eiffel Tower in Paris, France on 12 June 2023.",
        &[
            ("PERSON", "Jane Doe", 0, 8),
            ("LOCATION", "Eiffel Tower", 21, 33),
            ("LOCATION", "Paris", 37, 42),
            ("LOCATION", "France", 44, 50),
            ("DATE", "12 June 2023", 54, 66),
        ],
    ),
    (
        "an organization written with an article, a grouped number and a country",
        "The Red Cross delivered 2,000 blankets to Haiti after the earthquake.",
        &[
            ("ORGANIZATION", "Red Cross", 4, 13),
            ("QUANTITY", "2,000", 24, 29),
            ("LOCATION", "Haiti", 42, 47),
        ],
    ),
];

#[tokio::test]
async fn the_whole_answer_for_a_mixed_document_is_pinned() {
    let comprehend = Comprehend::new();

    for (case, text, expected) in BASELINES {
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

        let reported: Vec<(&str, &str, i64, i64)> = found
            .iter()
            .map(|entity| {
                (
                    entity["Type"].as_str().unwrap_or_default(),
                    entity["Text"].as_str().unwrap_or_default(),
                    entity["BeginOffset"].as_i64().unwrap_or(-1),
                    entity["EndOffset"].as_i64().unwrap_or(-1),
                )
            })
            .collect();
        assert_eq!(
            reported, *expected,
            "case {case:?}: expected {expected:?}, got {reported:?} from {output}"
        );
    }
}

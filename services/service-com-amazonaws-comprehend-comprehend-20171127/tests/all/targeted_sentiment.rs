// SPDX-License-Identifier: CC0-1.0

//! `DetectTargetedSentiment`: the entities a document mentions, each mention with
//! the offsets it occupies in the document and the sentiment of the clause it sits
//! in.
//!
//! The rule these suites assert, over a table of documents rather than over one
//! recorded request: a reported mention is the document's OWN substring at
//! `[BeginOffset, EndOffset)`, counted in characters, every mention of one entity
//! reports one type with confidences in (0, 1] and a four-way sentiment score that
//! sums to 1, and the answer is a function of the document alone — so repeating a
//! call reports the same mentions at the same offsets.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    LANGUAGE_CODE_VALUES, SENTIMENT_TYPE_VALUES, TARGETED_SENTIMENT_ENTITY_TYPE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend};

/// Documents spanning what the rule generalises over: how many entities a document
/// mentions, how often it mentions one, which rule places the mention (a name, a
/// legal-entity suffix, an honorific, a calendar name, a numeral, a determiner-headed
/// noun phrase, a pronoun), how many sentences it has, which script it is written
/// in, and documents with no mention at all.
const DOCUMENTS: &[(&str, &str)] = &[
    ("one proper noun", "Acme shipped the parcel on time."),
    (
        "one name mentioned twice",
        "Amazon refunded the fee. Amazon apologised for the delay.",
    ),
    (
        "a name and a pronoun",
        "Maria rewrote the parser. She was brilliant about it.",
    ),
    (
        "legal-entity suffix",
        "Initech Inc. shipped a terrible release.",
    ),
    (
        "honorific and name",
        "Dr Okonkwo explained the diagnosis very clearly.",
    ),
    (
        "calendar name and numeral",
        "The invoice arrived in March 2024 and the total was 1,250.",
    ),
    (
        "contrasting clauses",
        "The food was great but the service was terrible.",
    ),
    (
        "multi-byte characters before the mention",
        "The café was lovely and Zoë was wonderful to us.",
    ),
    (
        "four sentences",
        "The driver was late. The parcel was damaged. Acme refunded us. We were still unhappy.",
    ),
    (
        "first and second person",
        "I loved my new phone but you hated yours.",
    ),
    ("no words at all", "!!! ... ???"),
    ("one word", "Wonderful!"),
    ("one character", "a"),
];

/// Assert every invariant a `DetectTargetedSentiment` response carries for `text`.
fn assert_invariants(case: &str, text: &str, output: &Value) {
    let characters: Vec<char> = text.chars().collect();
    let entities = output["Entities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Entities array, got {output}"))
        .clone();

    // Every mention of every entity, so no two mentions of the document may claim
    // overlapping text.
    let mut spans: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut previous_entity_start = -1i64;

    for (entity_index, entity) in entities.iter().enumerate() {
        let mentions = entity["Mentions"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: entity {entity_index} has no Mentions array: {entity}")
            })
            .clone();
        assert!(
            !mentions.is_empty(),
            "case {case:?}: entity {entity_index} reports no mention at all: {entity}"
        );

        let mut previous_begin = -1i64;
        let mut types: BTreeSet<&str> = BTreeSet::new();

        for (mention_index, mention) in mentions.iter().enumerate() {
            let at = format!("case {case:?}: entity {entity_index} mention {mention_index}");
            let begin = mention["BeginOffset"].as_i64().unwrap_or(-1);
            let end = mention["EndOffset"].as_i64().unwrap_or(-1);
            assert!(
                begin >= 0 && end > begin && end as usize <= characters.len(),
                "{at}: expected 0 <= BeginOffset < EndOffset <= {} characters, got \
                 [{begin}, {end}) in {mention}",
                characters.len()
            );

            let slice: String = characters[begin as usize..end as usize].iter().collect();
            let reported = mention["Text"].as_str().unwrap_or_default();
            assert_eq!(
                reported, slice,
                "{at}: Text is not the document's substring at [{begin}, {end}), which is \
                 {slice:?}"
            );

            assert!(
                begin > previous_begin,
                "{at}: mentions must ascend by BeginOffset, got {begin} after {previous_begin} \
                 in {entity}"
            );
            previous_begin = begin;

            assert!(
                spans
                    .iter()
                    .all(|(other_begin, other_end)| end <= *other_begin || begin >= *other_end),
                "{at}: [{begin}, {end}) overlaps an already reported mention in {spans:?}"
            );
            spans.insert((begin, end));

            let kind = mention["Type"].as_str().unwrap_or_default();
            assert!(
                TARGETED_SENTIMENT_ENTITY_TYPE_VALUES.contains(&kind),
                "{at}: {kind:?} is not a modelled TargetedSentimentEntityType value"
            );
            types.insert(mention["Type"].as_str().unwrap_or_default());

            for label in ["Score", "GroupScore"] {
                let score = mention[label].as_f64().unwrap_or(f64::NAN);
                assert!(
                    score > 0.0 && score <= 1.0,
                    "{at}: expected {label} in (0, 1], got {score} in {mention}"
                );
            }

            let sentiment = mention["MentionSentiment"]["Sentiment"]
                .as_str()
                .unwrap_or_default();
            assert!(
                SENTIMENT_TYPE_VALUES.contains(&sentiment),
                "{at}: {sentiment:?} is not a modelled SentimentType value"
            );
            let scores = &mention["MentionSentiment"]["SentimentScore"];
            let by_label: Vec<(&str, f64)> = ["Positive", "Negative", "Neutral", "Mixed"]
                .into_iter()
                .map(|label| {
                    let score = scores[label]
                        .as_f64()
                        .unwrap_or_else(|| panic!("{at}: expected SentimentScore.{label} in {scores}"));
                    (label, score)
                })
                .collect();
            let total: f64 = by_label.iter().map(|(_, score)| score).sum();
            assert!(
                (total - 1.0).abs() < 1e-5,
                "{at}: expected the four sentiment scores to sum to 1.0, got {total} from {scores}"
            );
            // The reported sentiment is the arg-max of the four it reports, with the
            // tie-break the scorer documents: the first of (Positive, Negative,
            // Neutral, Mixed), which is the order `by_label` is built in.
            let winner = by_label
                .iter()
                .copied()
                .reduce(|best, candidate| if candidate.1 > best.1 { candidate } else { best })
                .map(|(label, _)| label.to_uppercase())
                .unwrap_or_default();
            assert_eq!(
                winner, sentiment,
                "{at}: Sentiment is not the arg-max of SentimentScore, got {sentiment:?} \
                 with {scores}"
            );
        }

        assert_eq!(
            types.len(),
            1,
            "case {case:?}: entity {entity_index} reports more than one Type for one \
             entity: {types:?}"
        );

        let descriptive = entity["DescriptiveMentionIndex"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: entity {entity_index} has no DescriptiveMentionIndex: \
                     {entity}"
                )
            })
            .clone();
        assert!(
            !descriptive.is_empty(),
            "case {case:?}: entity {entity_index} names no descriptive mention: {entity}"
        );
        for index in &descriptive {
            let index = index.as_i64().unwrap_or(-1);
            assert!(
                index >= 0 && (index as usize) < mentions.len(),
                "case {case:?}: entity {entity_index} has DescriptiveMentionIndex {index}, \
                 which is not an index into its {} mentions",
                mentions.len()
            );
        }

        let first = mentions[0]["BeginOffset"].as_i64().unwrap_or(-1);
        assert!(
            first > previous_entity_start,
            "case {case:?}: entities must be ordered by the document's first mention of \
             them, got entity {entity_index} starting at {first} after {previous_entity_start}"
        );
        previous_entity_start = first;
    }
}

/// Analyse `text` as English, expecting success.
async fn analyse(comprehend: &Comprehend, text: &str) -> Value {
    comprehend
        .ok(
            "DetectTargetedSentiment",
            &json!({ "Text": text, "LanguageCode": "en" }),
        )
        .await
}

/// Every mention reported for an entity, as `(Text, Type, BeginOffset)` triples.
fn mentions_of(entity: &Value) -> Vec<(String, String, i64)> {
    entity["Mentions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|mention| {
            (
                mention["Text"].as_str().unwrap_or_default().to_string(),
                mention["Type"].as_str().unwrap_or_default().to_string(),
                mention["BeginOffset"].as_i64().unwrap_or(-1),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_mention_is_the_documents_own_substring_at_its_offsets() {
    let comprehend = Comprehend::new();

    for (case, text) in DOCUMENTS {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
    }
}

/// A systematic sweep rather than one example: every combination of a prefix that
/// pushes character offsets away from byte offsets, a mention form each recognition
/// rule places, and a sentiment context, is a document whose invariants must hold.
#[tokio::test]
async fn the_invariants_hold_over_every_prefix_form_and_context() {
    let comprehend = Comprehend::new();

    let prefixes = ["", "Привет. ", "Héllo — ", "😀 "];
    let forms = [
        "Acme",
        "Acme Inc.",
        "Dr Okonkwo",
        "the parcel",
        "March 2024",
        "1,250",
        "it",
        "my phone",
    ];
    let contexts = [
        "{} was wonderful.",
        "{} was terrible.",
        "Nobody mentioned {} at all.",
        "I liked {} but my colleague hated {}.",
    ];

    for prefix in prefixes {
        for form in forms {
            for context in contexts {
                let text = format!("{prefix}{}", context.replace("{}", form));
                let case = format!("prefix {prefix:?} + form {form:?} + context {context:?}");
                let output = analyse(&comprehend, &text).await;
                assert_invariants(&case, &text, &output);

                // Each `forms` row is a mention form one of the recognition rules
                // must place, so the sweep cannot pass vacuously on an empty
                // `Entities`: some reported mention has to come out of that form.
                let reported: Vec<String> = output["Entities"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .flat_map(mentions_of)
                    .map(|(text, _, _)| text)
                    .collect();
                assert!(
                    reported
                        .iter()
                        .any(|mention| form.contains(mention.as_str())),
                    "case {case:?}: expected a mention placed in {form:?}, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn offsets_count_characters_rather_than_bytes() {
    let comprehend = Comprehend::new();

    // Each row carries multi-byte characters BEFORE the mention it names, so the
    // mention's character offset is strictly smaller than its byte offset: a
    // response reporting byte positions cannot pass.
    for (case, text, mention) in [
        ("accents", "The café was lovely and Zoë was wonderful.", "Zoë"),
        ("cyrillic", "Привет! Acme was wonderful.", "Acme"),
        ("emoji", "😀😀😀 Acme was wonderful.", "Acme"),
        ("ideographs", "我们 said Acme was wonderful.", "Acme"),
    ] {
        let text = text.to_string();
        let output = analyse(&comprehend, &text).await;
        assert_invariants(case, &text, &output);

        let expected_characters = text
            .chars()
            .collect::<Vec<char>>()
            .windows(mention.chars().count())
            .position(|window| window.iter().collect::<String>() == mention)
            .unwrap_or_else(|| panic!("case {case:?}: {mention:?} is not in {text:?}"))
            as i64;
        let byte_offset = text
            .find(mention)
            .unwrap_or_else(|| panic!("case {case:?}: {mention:?} is not in {text:?}"))
            as i64;
        assert!(
            byte_offset > expected_characters,
            "case {case:?}: the row must place multi-byte characters before {mention:?}, \
             but its byte offset {byte_offset} is not past its character offset \
             {expected_characters}"
        );

        let found = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(mentions_of)
            .find(|(text, _, _)| text == mention);
        let (_, _, begin) = found.unwrap_or_else(|| {
            panic!("case {case:?}: no mention of {mention:?} in {output}")
        });
        assert_eq!(
            begin, expected_characters,
            "case {case:?}: expected the character offset {expected_characters} for \
             {mention:?}, got {begin} (its byte offset is {byte_offset})"
        );
    }
}

#[tokio::test]
async fn mentions_of_one_entity_are_grouped_into_one_entity() {
    let comprehend = Comprehend::new();

    // Each row: the document, and the mention texts that must land in ONE entity.
    for (case, text, grouped) in [
        (
            "a repeated name",
            "Amazon refunded the fee. Amazon apologised for the delay.",
            vec!["Amazon", "Amazon"],
        ),
        (
            "a name and the pronoun referring to it",
            "Maria rewrote the parser. She was brilliant.",
            vec!["Maria", "She"],
        ),
        (
            "a name repeated after the pronoun that typed it a person's",
            "Maria rewrote the parser. She was brilliant. Maria left early.",
            vec!["Maria", "She", "Maria"],
        ),
        (
            "a full name and its surname",
            "Alice Lennox joined us. Lennox was wonderful.",
            vec!["Alice Lennox", "Lennox"],
        ),
        (
            "every first-person pronoun",
            "I loved it. I said so to the team and I meant it.",
            vec!["I", "I", "I"],
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let entities = output["Entities"].as_array().cloned().unwrap_or_default();
        let found = entities
            .iter()
            .map(|entity| {
                mentions_of(entity)
                    .into_iter()
                    .map(|(text, _, _)| text)
                    .collect::<Vec<String>>()
            })
            .find(|texts| *texts == grouped);
        assert!(
            found.is_some(),
            "case {case:?}: expected one entity mentioned as {grouped:?}, got {entities:?}"
        );
    }
}

#[tokio::test]
async fn a_pronoun_reports_the_type_of_what_it_refers_to() {
    let comprehend = Comprehend::new();

    for (case, text, mention, expected) in [
        (
            "a personal pronoun types its antecedent a person",
            "Maria rewrote the parser. She was brilliant.",
            "She",
            "PERSON",
        ),
        (
            "a first-person pronoun is a person",
            "I was unhappy with the delay.",
            "I",
            "PERSON",
        ),
        (
            "an honorific is a personal title",
            "Dr Okonkwo explained the diagnosis.",
            "Dr",
            "PERSONAL_TITLE",
        ),
        (
            "a legal-entity suffix makes an organization",
            "Initech Inc. shipped a release.",
            "Initech Inc",
            "ORGANIZATION",
        ),
        (
            "a calendar name makes a date",
            "The invoice arrived in March 2024.",
            "March 2024",
            "DATE",
        ),
        (
            "a numeral is a quantity",
            "The total was 1,250 last week.",
            "1,250",
            "QUANTITY",
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let reported = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(mentions_of)
            .find(|(found, _, _)| found == mention)
            .map(|(_, kind, _)| kind);
        assert_eq!(
            reported.as_deref(),
            Some(expected),
            "case {case:?}: expected {mention:?} to be typed {expected:?}, got {output}"
        );
    }
}

#[tokio::test]
async fn each_mention_is_scored_on_the_clause_it_sits_in() {
    let comprehend = Comprehend::new();

    // One document carries two mentions with OPPOSITE polarity, so a response
    // scoring the whole document once per mention cannot pass. The rows differ in
    // which clause carries which polarity and in how the clauses are separated.
    // The assertion is on the DIRECTION of the four confidences — which of
    // `Positive` and `Negative` the clause puts the weight on — because the label is
    // their arg-max and `NEUTRAL` can out-score a lone sentiment word in a short
    // clause.
    for (case, text, expected) in [
        (
            "contrast with but",
            "The food was great but the service was terrible.",
            vec![("food", "positive"), ("service", "negative")],
        ),
        (
            "contrast the other way round",
            "Acme was terrible but Globex was wonderful.",
            vec![("Acme", "negative"), ("Globex", "positive")],
        ),
        (
            "two sentences",
            "Acme was wonderful. Globex was terrible.",
            vec![("Acme", "positive"), ("Globex", "negative")],
        ),
        (
            "clauses separated by a semicolon",
            "Acme was wonderful; Globex was terrible.",
            vec![("Acme", "positive"), ("Globex", "negative")],
        ),
        (
            "three clauses",
            "Acme was wonderful but Globex was terrible; Initech was excellent.",
            vec![
                ("Acme", "positive"),
                ("Globex", "negative"),
                ("Initech", "positive"),
            ],
        ),
        (
            "no sentiment anywhere",
            "The invoice arrived on Tuesday.",
            vec![("invoice", "neutral")],
        ),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);

        let reported: Vec<(String, Value)> = output["Entities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
            .map(|mention| {
                (
                    mention["Text"].as_str().unwrap_or_default().to_string(),
                    mention["MentionSentiment"].clone(),
                )
            })
            .collect();

        for (mention, direction) in expected {
            let sentiment = reported
                .iter()
                .find(|(text, _)| text == mention)
                .map(|(_, sentiment)| sentiment.clone())
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no mention of {mention:?} in {reported:?}")
                });
            let positive = sentiment["SentimentScore"]["Positive"]
                .as_f64()
                .unwrap_or(f64::NAN);
            let negative = sentiment["SentimentScore"]["Negative"]
                .as_f64()
                .unwrap_or(f64::NAN);
            match direction {
                "positive" => assert!(
                    positive > negative,
                    "case {case:?}: the clause around {mention:?} is positive, so expected \
                     Positive > Negative, got {positive} and {negative} in {sentiment}"
                ),
                "negative" => assert!(
                    negative > positive,
                    "case {case:?}: the clause around {mention:?} is negative, so expected \
                     Negative > Positive, got {negative} and {positive} in {sentiment}"
                ),
                _ => assert_eq!(
                    sentiment["Sentiment"].as_str().unwrap_or_default(),
                    "NEUTRAL",
                    "case {case:?}: the clause around {mention:?} carries no sentiment, so \
                     expected NEUTRAL, got {sentiment}"
                ),
            }
        }
    }
}

/// The label is still the arg-max of the clause's four confidences, which a clause
/// with an unambiguous polarity settles: the same document reports a POSITIVE and a
/// NEGATIVE mention.
#[tokio::test]
async fn a_mention_in_an_unambiguous_clause_carries_that_label() {
    let comprehend = Comprehend::new();

    let text = "The food was great but the service was terrible.";
    let output = analyse(&comprehend, text).await;
    assert_invariants("unambiguous clauses", text, &output);

    let labels: Vec<(String, String)> = output["Entities"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|entity| entity["Mentions"].as_array().cloned().unwrap_or_default())
        .map(|mention| {
            (
                mention["Text"].as_str().unwrap_or_default().to_string(),
                mention["MentionSentiment"]["Sentiment"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();

    for (mention, expected) in [("food", "POSITIVE"), ("service", "NEGATIVE")] {
        let found = labels
            .iter()
            .find(|(text, _)| text == mention)
            .map(|(_, label)| label.as_str());
        assert_eq!(
            found,
            Some(expected),
            "case {mention:?}: expected {expected:?}, got {labels:?}"
        );
    }
}

#[tokio::test]
async fn a_document_with_no_word_reports_no_entity() {
    let comprehend = Comprehend::new();

    for (case, text) in [
        ("punctuation only", "!!! ... ???"),
        ("whitespace and punctuation", " - , . "),
        ("one function word", "the"),
    ] {
        let output = analyse(&comprehend, text).await;
        assert_invariants(case, text, &output);
        let entities = output["Entities"].as_array().cloned().unwrap_or_default();
        assert!(
            entities.is_empty(),
            "case {case:?}: no rule places a mention in {text:?}, so no entity may be \
             reported, got {entities:?}"
        );
    }
}

#[tokio::test]
async fn the_same_document_is_analysed_the_same_way_twice() {
    let comprehend = Comprehend::new();

    for (case, text) in DOCUMENTS {
        let body = json!({ "Text": text, "LanguageCode": "en" });
        let first = comprehend.ok("DetectTargetedSentiment", &body).await;
        let second = comprehend.ok("DetectTargetedSentiment", &body).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected the same entities and offsets twice, got {first} then \
             {second}"
        );
    }
}

#[tokio::test]
async fn a_language_the_operation_does_not_analyse_is_refused() {
    let comprehend = Comprehend::new();

    // `DetectTargetedSentimentRequest$LanguageCode` documents "Currently, English is
    // the only supported language", so every other modelled value is refused.
    for code in LANGUAGE_CODE_VALUES {
        let (status, body) = comprehend
            .call(
                "DetectTargetedSentiment",
                &json!({ "Text": "Acme was wonderful.", "LanguageCode": code }),
            )
            .await;

        if *code == "en" {
            assert_eq!(
                status, 200,
                "case {code:?}: English is analysed, expected 200, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnsupportedLanguageException",
                "case {code:?}: expected UnsupportedLanguageException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn text_at_the_documented_ceiling_is_accepted_and_one_byte_past_it_is_not() {
    let comprehend = Comprehend::new();

    // `DetectTargetedSentimentRequest$Text` documents "The maximum string length is
    // 5 KB": the modelled `@length` floor of 1, the ceiling exactly, one byte past.
    let ceiling = 5 * 1024;
    for size in [1usize, ceiling, ceiling + 1] {
        let (status, body) = comprehend
            .call(
                "DetectTargetedSentiment",
                &json!({ "Text": "a".repeat(size), "LanguageCode": "en" }),
            )
            .await;

        if size > ceiling {
            assert_eq!(
                status, 400,
                "case {size} bytes: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "TextSizeLimitExceededException",
                "case {size} bytes: expected TextSizeLimitExceededException, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {size} bytes: expected 200, got {status}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_empty_absent_or_unmodelled_input_is_refused() {
    let comprehend = Comprehend::new();

    for (case, body) in [
        ("empty Text", json!({ "Text": "", "LanguageCode": "en" })),
        ("absent Text", json!({ "LanguageCode": "en" })),
        ("absent LanguageCode", json!({ "Text": "Acme was wonderful." })),
        (
            "LanguageCode outside the modelled enum",
            json!({ "Text": "Acme was wonderful.", "LanguageCode": "EN" }),
        ),
    ] {
        let (status, response) = comprehend.call("DetectTargetedSentiment", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
    }
}

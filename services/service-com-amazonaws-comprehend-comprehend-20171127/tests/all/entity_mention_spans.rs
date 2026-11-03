// SPDX-License-Identifier: CC0-1.0

//! What `BatchDetectEntities` reports for a detection's SPAN and its `Score`, as a
//! convention rather than as one recorded answer.
//!
//! Two rules, each asserted over a table whose rows differ along what that rule
//! generalises over:
//!
//! 1. The span reported for an entity covers the COMPLETE mention the document
//!    writes, so `BeginOffset` is the offset of the mention's first character,
//!    `EndOffset` one past its last, and `Text` the characters they delimit. A date
//!    expression is the case the modelled members make visible: it is written with
//!    the deictic modifier that says which day it names and with the part of the day
//!    that narrows it, and all of that is the mention a client highlights or redacts.
//! 2. `Score` is "the level of confidence that Amazon Comprehend has in the accuracy
//!    of the detection" (the modelled `Entity$Score`), so it follows THAT detection's
//!    own evidence — how much the rule that fired constrains the characters, how much
//!    of the mention it accounts for, and what else the same characters could be —
//!    rather than being a constant of the category the detection ended up in.
//!
//! Every row rides in one batch, so each is answered as its own `ResultList` entry
//! beside the others, exactly as the recorded three-document request's were.

use serde_json::{json, Value};

use crate::batch_inference_calibration::{batch_results, distinct};
use crate::comprehend_client::Comprehend;

/// The zero-based CHARACTER offset `mention` sits at in `document` — what the
/// response's `BeginOffset` has to be, computed from the document rather than
/// written down as a literal.
fn char_offset_of(case: &str, document: &str, mention: &str) -> i64 {
    let byte = document
        .find(mention)
        .unwrap_or_else(|| panic!("case {case:?}: {mention:?} is not in {document:?}"));
    document[..byte].chars().count() as i64
}

/// Every entity one document of a batch was answered with.
async fn entities_of(comprehend: &Comprehend, documents: &[&str]) -> Vec<Vec<Value>> {
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

// ---------------------------------------------------------------------------
// 1. The span covers the whole mention
// ---------------------------------------------------------------------------

/// `(case, document, the mention the recogniser must report whole)`.
///
/// The rows differ along the shape of the mention rather than along the words in it:
/// which calendar word the expression is built on (a weekday, a month name, a named
/// period, a full calendar date, a day adverb), whether a modifier places it, whether
/// a day part narrows it, whether the modifiers stack, and the cases where there is
/// nothing to join — a mention written on its own, one whose would-be modifier sits
/// on the previous line, which no mention runs across, one whose modifier stands
/// elsewhere in the sentence and places nothing, and one behind multi-byte
/// characters, where a byte offset and a character offset differ.
const MENTIONS: &[(&str, &str, &str)] = &[
    (
        "a weekday with the modifier that places it",
        "The report is due next Friday at noon.",
        "next Friday",
    ),
    (
        "a weekday with the part of the day that narrows it",
        "The crew works Friday morning.",
        "Friday morning",
    ),
    (
        "a weekday with both a modifier and a day part",
        "She lands next Thursday evening.",
        "next Thursday evening",
    ),
    (
        "stacked modifiers, which English writes as one expression",
        "The audit is due this coming Monday afternoon.",
        "this coming Monday afternoon",
    ),
    (
        "a named period with its modifier",
        "They met last week in Portugal.",
        "last week",
    ),
    (
        "a month name with its modifier",
        "The rates change next January for everyone.",
        "next January",
    ),
    (
        "a calendar date with its modifier",
        "The clerk will file it this January 5, 2024.",
        "this January 5, 2024",
    ),
    (
        "a day adverb with the part of the day that narrows it",
        "The results arrive tomorrow morning.",
        "tomorrow morning",
    ),
    (
        "a bare weekday, which is the whole mention",
        "We ship on Friday.",
        "Friday",
    ),
    (
        "a calendar form that carries no modifier",
        "The outage began on 2024-01-15 and lasted an hour.",
        "2024-01-15",
    ),
    (
        "a line break, which no mention runs across",
        "The plan is due next\nFriday at noon.",
        "Friday",
    ),
    (
        "a modifier elsewhere in the sentence, which places nothing",
        "The next item ships on Friday.",
        "Friday",
    ),
    (
        "multi-byte characters in front of the mention",
        "Caf\u{e9} notes \u{2014} the review is next Friday.",
        "next Friday",
    ),
];

#[tokio::test]
async fn a_span_covers_the_whole_mention_the_document_writes() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = MENTIONS.iter().map(|(_, document, _)| *document).collect();
    let answers = entities_of(&comprehend, &documents).await;

    for (position, (case, document, mention)) in MENTIONS.iter().enumerate() {
        let found = &answers[position];
        let begin = char_offset_of(case, document, mention);
        let end = begin + mention.chars().count() as i64;

        let dates: Vec<&Value> = found
            .iter()
            .filter(|entity| entity["Type"].as_str() == Some("DATE"))
            .collect();
        assert_eq!(
            dates.len(),
            1,
            "case {case:?}: expected exactly one DATE entity in {document:?}, got {found:?}"
        );
        let date = dates[0];
        assert_eq!(
            date["Text"].as_str(),
            Some(*mention),
            "case {case:?}: expected the whole mention {mention:?}, got {date}"
        );
        assert_eq!(
            date["BeginOffset"].as_i64(),
            Some(begin),
            "case {case:?}: expected BeginOffset at the mention's first character \
             ({begin}), got {date}"
        );
        assert_eq!(
            date["EndOffset"].as_i64(),
            Some(end),
            "case {case:?}: expected EndOffset one past the mention's last character \
             ({end}), got {date}"
        );

        // The offsets and the text are one statement about the document, so the
        // characters they delimit are the entity's own text.
        let slice: String = document
            .chars()
            .skip(usize::try_from(begin).unwrap_or_default())
            .take(usize::try_from(end - begin).unwrap_or_default())
            .collect();
        assert_eq!(
            slice,
            *mention,
            "case {case:?}: the characters {begin}..{end} of the document are {slice:?}, \
             which is not the mention {mention:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. The score follows the detection's own evidence
// ---------------------------------------------------------------------------

/// `(case, document, the entity's Type, its Text)`.
///
/// The rows differ along the EVIDENCE behind the detection, which is what the rule
/// generalises over: a closed form that fixes separators and field widths, a looser
/// one another category also reads, a mention reported with the modifier that places
/// it against the bare calendar word, a lexicon entry naming every token of the span,
/// a designator naming one token of three, an honorific standing in front of the
/// name, and a capitalized phrase nothing places at all. Several rows share a Type,
/// so a score read out of the category rather than out of the detection disagrees
/// with the table.
const DETECTIONS: &[(&str, &str, &str, &str)] = &[
    (
        "a closed form that fixes separators and field widths",
        "The outage began on 2024-01-15 and lasted an hour.",
        "DATE",
        "2024-01-15",
    ),
    (
        "a month name with its day and year",
        "The hearing is set for Sept 5, 2024 at the courthouse.",
        "DATE",
        "Sept 5, 2024",
    ),
    (
        "a bare four-digit year another category also reads",
        "Microsoft Corporation was founded in 1975.",
        "DATE",
        "1975",
    ),
    (
        "a weekday with the modifier that places it",
        "The report is due next Friday at noon.",
        "DATE",
        "next Friday",
    ),
    (
        "a bare weekday a name reading also claims",
        "We ship on Friday.",
        "DATE",
        "Friday",
    ),
    (
        "a named period with its modifier",
        "They met last week in Seattle.",
        "DATE",
        "last week",
    ),
    (
        "a lexicon entry naming every token of the span",
        "They expanded into Portugal.",
        "LOCATION",
        "Portugal",
    ),
    (
        "a designator naming one token of three",
        "Acme Widgets LLC filed the papers.",
        "ORGANIZATION",
        "Acme Widgets LLC",
    ),
    (
        "an honorific standing in front of the name",
        "Dr. Jane Doe reviewed the report.",
        "PERSON",
        "Jane Doe",
    ),
    (
        "a capitalized phrase no rule places",
        "The Zorblax Initiative published its findings.",
        "OTHER",
        "Zorblax Initiative",
    ),
    (
        "a measurement carrying its unit",
        "The parcel weighs 12 kg.",
        "QUANTITY",
        "12 kg",
    ),
    (
        "a percentage, whose form fixes more than a unit does",
        "Revenue grew 12 percent over the quarter.",
        "QUANTITY",
        "12 percent",
    ),
];

#[tokio::test]
async fn a_score_is_the_confidence_the_detections_own_evidence_leaves() {
    let comprehend = Comprehend::new();
    let documents: Vec<&str> = DETECTIONS
        .iter()
        .map(|(_, document, _, _)| *document)
        .collect();
    let answers = entities_of(&comprehend, &documents).await;

    let mut scored: Vec<(&str, &str, f64)> = Vec::new();
    for (position, (case, document, entity_type, text)) in DETECTIONS.iter().enumerate() {
        let found = &answers[position];
        let matching: Vec<&Value> = found
            .iter()
            .filter(|entity| entity["Text"].as_str() == Some(*text))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one entity reading {text:?} in {document:?}, \
             got {found:?}"
        );
        let entity = matching[0];
        assert_eq!(
            entity["Type"].as_str(),
            Some(*entity_type),
            "case {case:?}: expected {text:?} to be typed {entity_type}, got {entity}"
        );

        let score = entity["Score"].as_f64().unwrap_or(f64::NAN);
        // A rule-and-lexicon recogniser never excludes the reading in which the span
        // names nothing, so a detection is a confidence and never a certainty.
        assert!(
            score > 0.0 && score < 1.0,
            "case {case:?}: expected a Score strictly inside (0, 1), got {score} in {entity}"
        );
        scored.push((case, entity_type, score));
    }

    let score_of = |wanted: &str| -> f64 {
        scored
            .iter()
            .find(|(case, _, _)| *case == wanted)
            .map(|(_, _, score)| *score)
            .unwrap_or_else(|| panic!("no score for case {wanted:?}"))
    };

    // The score follows the detection, so the table cannot come back as a handful of
    // repeated constants.
    let spread = distinct(scored.iter().map(|(_, _, score)| *score));
    assert!(
        spread >= 8,
        "expected the confidences to follow each detection's own evidence, got {spread} \
         distinct values across {scored:?}"
    );

    // Neither can it come back as one constant PER CATEGORY: every type the table
    // reports more than once is reported with more than one confidence.
    for entity_type in ["DATE", "QUANTITY"] {
        let of_type: Vec<f64> = scored
            .iter()
            .filter(|(_, reported, _)| *reported == entity_type)
            .map(|(_, _, score)| *score)
            .collect();
        assert!(
            of_type.len() > 1 && distinct(of_type.iter().copied()) > 1,
            "expected {entity_type} detections to be scored from their own evidence, got \
             {of_type:?}"
        );
    }

    // More evidence, more confidence — each pair differs in exactly one thing about
    // the evidence and in nothing about the category.
    for (more, less, reason) in [
        (
            "a closed form that fixes separators and field widths",
            "a bare four-digit year another category also reads",
            "a form that fixes separators and field widths leaves less open than a run \
             of four digits another category reads too",
        ),
        (
            "a weekday with the modifier that places it",
            "a bare weekday a name reading also claims",
            "a mention reported whole accounts for more than the bare calendar word, \
             which a name reading also claims",
        ),
        (
            "a lexicon entry naming every token of the span",
            "a designator naming one token of three",
            "a lexicon entry naming every token of the span leaves no token of it \
             unaccounted for",
        ),
        (
            "a lexicon entry naming every token of the span",
            "a capitalized phrase no rule places",
            "a span a lexicon names is placed, where a capitalized phrase nothing \
             places is only orthography",
        ),
        (
            "a month name with its day and year",
            "an honorific standing in front of the name",
            "a closed calendar form leaves less open than a cue beside the span",
        ),
    ] {
        assert!(
            score_of(more) > score_of(less),
            "expected {more:?} to be scored above {less:?} ({reason}), got {} against {}",
            score_of(more),
            score_of(less)
        );
    }

    // Two forms of the SAME category, differing in how much of the span each one
    // fixes: a score read out of the category could not tell them apart.
    let percentage = score_of("a percentage, whose form fixes more than a unit does");
    let measurement = score_of("a measurement carrying its unit");
    assert!(
        (percentage - measurement).abs() > 1e-6,
        "expected two forms of one category to be scored from their own evidence, got \
         {percentage} and {measurement}"
    );
}

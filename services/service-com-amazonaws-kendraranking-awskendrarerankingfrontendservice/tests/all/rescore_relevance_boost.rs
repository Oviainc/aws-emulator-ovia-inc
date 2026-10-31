// SPDX-License-Identifier: CC0-1.0

//! One rule: the `Score` `Rescore` answers is a NEW relevancy score for the document,
//! not a small perturbation of the score the search service arrived with.
//!
//! `RescoreResult$ResultItems` is "a list of result items for documents with new
//! relevancy scores. The results are in descending order", and `Document$OriginalScore`
//! is "the original document score or rank from the search service. Amazon Kendra
//! Intelligent Ranking gives the document a new score or rank based on its intelligent
//! search algorithm" (both from
//! `smithy_query(query="shape",shape="Document,RescoreResult")`). Together: the answer
//! is the ranking's own view of how well the document's text answers the query, so a
//! document whose text carries what the query ASKS ABOUT comes back materially above the
//! original score it was submitted with — above documents the query says nothing about
//! even when those arrived with a higher original score — and the words a query asks
//! about are its content-bearing ones, so its function words neither dilute nor change
//! the measure, and a content term matches whichever inflected form a document spells it
//! in.
//!
//! The MAGNITUDE of a semantic score is model output and no contract, so these suites
//! assert the direction and the invariances, over a table whose rows vary the original
//! score the relevant document arrives with (including zero and a negative one), the
//! scale the batch's scores are on (the unit scale a client normalises to, and the tens
//! and hundreds a lexical search engine emits), how
//! many function words the query carries and where, which text form
//! (`Title`/`TokenizedTitle`, `Body`/`TokenizedBody`) carries the terms, how many
//! documents the batch holds and how high their original scores are, and which
//! inflection the document spells a query term in.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    create_active_plan, emulator, rescore, rescore_items, SCORE_MAX, SCORE_MIN, SCORE_TOLERANCE,
};

/// How far above the `OriginalScore` it arrived with a document whose text answers the
/// query has to come back for the lift to be one a caller can act on ("only show items
/// that beat their original score", "drop everything under X"), as a SHARE of the scale
/// the batch's scores are on — a client that normalises its scores to zero-to-one and a
/// lexical engine emitting scores in the hundreds both want a lift they can see. A floor
/// on the direction, not a pin on the magnitude: the emulator lifts such a document much
/// further than this.
const MATERIAL_BOOST: f64 = 0.05;

/// The scale a batch's scores are on: the largest `OriginalScore` magnitude it carries,
/// or one where every score it carries is smaller than that.
fn batch_scale(documents: &Value) -> f64 {
    documents
        .as_array()
        .expect("the batch's documents")
        .iter()
        .map(|document| {
            document["OriginalScore"]
                .as_f64()
                .expect("a document's original score")
                .abs()
        })
        .fold(1.0, f64::max)
}

/// The scores of a `Rescore` answer by `DocumentId`.
fn scores_by_id(body: &Value) -> BTreeMap<String, f64> {
    rescore_items(body).into_iter().collect()
}

/// Assert the shape every answer keeps, whatever the rule under test: HTTP 200, one
/// result item per submitted document, descending order, and every score inside the
/// range the model gives `Score`.
fn assert_answer_shape(case: &str, status: u16, answer: &Value, documents: &Value) {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {answer}");
    let items = rescore_items(answer);
    let submitted = documents.as_array().expect("the case's documents").len();
    assert_eq!(
        items.len(),
        submitted,
        "case {case:?}: one result item per submitted document, got {items:?}"
    );
    for window in items.windows(2) {
        assert!(
            window[0].1 >= window[1].1,
            "case {case:?}: the results must be in descending order, got {items:?}"
        );
    }
    for (id, score) in &items {
        assert!(
            (SCORE_MIN..=SCORE_MAX).contains(score),
            "case {case:?}: the score of {id:?} must stay inside the modelled range, \
             got {score}"
        );
    }
}

/// One row of the promotion table: a query, the document whose text answers it, and the
/// documents the query's content terms are absent from — each submitted with a HIGHER
/// `OriginalScore` than the relevant one.
struct BoostCase {
    /// What this row varies.
    case: &'static str,
    query: &'static str,
    /// The document whose `Title`/`Body` (or their pre-tokenized forms) carry the query's
    /// content-bearing terms.
    relevant: Value,
    /// The documents the query says nothing about.
    distractors: Value,
}

fn boost_cases() -> Vec<BoostCase> {
    vec![
        BoostCase {
            case: "a low original score, under a query carrying function words",
            query: "how do electric car batteries work",
            relevant: json!({
                "Id": "relevant", "Title": "Electric vehicle batteries",
                "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.2
            }),
            distractors: json!([
                { "Id": "gardening", "Title": "Gardening tips",
                  "Body": "Pruning roses in spring.", "OriginalScore": 0.9 },
            ]),
        },
        BoostCase {
            case: "an original score of zero, under a query of content words only",
            query: "electric car batteries",
            relevant: json!({
                "Id": "relevant", "Title": "Electric vehicle batteries",
                "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.0
            }),
            distractors: json!([
                { "Id": "sourdough", "Title": "Sourdough",
                  "Body": "Bread recipes for beginners.", "OriginalScore": 0.75 },
            ]),
        },
        BoostCase {
            case: "a negative original score",
            query: "what is a lithium ion battery",
            relevant: json!({
                "Id": "relevant", "Title": "Lithium ion battery chemistry",
                "Body": "A lithium ion battery stores charge.", "OriginalScore": -0.4
            }),
            distractors: json!([
                { "Id": "roses", "Title": "Roses", "Body": "Pruning in spring.",
                  "OriginalScore": 0.5 },
            ]),
        },
        BoostCase {
            case: "the pre-tokenized forms carry the document's text",
            query: "how does a wind turbine work",
            relevant: json!({
                "Id": "relevant", "TokenizedTitle": ["Wind", "turbines"],
                "TokenizedBody": ["A", "wind", "turbine", "converts", "wind", "into", "power"],
                "OriginalScore": 0.1
            }),
            distractors: json!([
                { "Id": "bread", "TokenizedBody": ["Sourdough", "bread", "recipes"],
                  "OriginalScore": 0.8 },
            ]),
        },
        BoostCase {
            case: "the document spells the query's terms in other inflections",
            query: "solar panel efficiency",
            relevant: json!({
                "Id": "relevant", "Title": "Solar panels",
                "Body": "Panels improved the efficiencies of a roof.", "OriginalScore": 0.15
            }),
            distractors: json!([
                { "Id": "roses", "Title": "Roses", "Body": "Pruning in spring.",
                  "OriginalScore": 0.6 },
            ]),
        },
        BoostCase {
            case: "three distractors, each with a higher original score",
            query: "the history of the printing press",
            relevant: json!({
                "Id": "relevant", "Title": "Printing press history",
                "Body": "The printing press changed printing.", "OriginalScore": 0.3
            }),
            distractors: json!([
                { "Id": "d-1", "Title": "Roses", "Body": "Pruning in spring.", "OriginalScore": 0.6 },
                { "Id": "d-2", "Title": "Sourdough", "Body": "Bread recipes.", "OriginalScore": 0.8 },
                { "Id": "d-3", "Title": "Kayaks", "Body": "Paddling downstream.", "OriginalScore": 0.95 },
            ]),
        },
        BoostCase {
            case: "a batch of lexical-engine scores, in the tens",
            query: "how do electric car batteries work",
            relevant: json!({
                "Id": "relevant", "Title": "Electric vehicle batteries",
                "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 7.5
            }),
            distractors: json!([
                { "Id": "gardening", "Title": "Gardening tips",
                  "Body": "Pruning roses in spring.", "OriginalScore": 12.0 },
            ]),
        },
        BoostCase {
            case: "a batch of scores in the hundreds, with two distractors above it",
            query: "electric car batteries",
            relevant: json!({
                "Id": "relevant", "Title": "Electric vehicle batteries",
                "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 150.0
            }),
            distractors: json!([
                { "Id": "roses", "Title": "Roses", "Body": "Pruning in spring.",
                  "OriginalScore": 400.0 },
                { "Id": "kayaks", "Title": "Kayaks", "Body": "Paddling downstream.",
                  "OriginalScore": 900.0 },
            ]),
        },
        BoostCase {
            case: "a long query whose content words sit among many function words",
            query: "can you tell me how the cooling system of a data center is designed",
            relevant: json!({
                "Id": "relevant", "Title": "Data center cooling systems",
                "Body": "A data center cooling system is designed to remove heat.",
                "OriginalScore": 0.25
            }),
            distractors: json!([
                { "Id": "kayaks", "Title": "Kayaks", "Body": "Paddling downstream.",
                  "OriginalScore": 0.7 },
            ]),
        },
    ]
}

#[tokio::test]
async fn a_document_whose_text_answers_the_query_is_lifted_above_its_original_score() {
    let router = emulator();
    let plan = create_active_plan(&router, "relevance-boost-plan").await;

    for case in boost_cases() {
        let mut documents = vec![case.relevant.clone()];
        documents.extend(
            case.distractors
                .as_array()
                .expect("the case's distractors")
                .clone(),
        );
        let documents = json!(documents);
        let (status, answer) = rescore(&router, &plan, case.query, &documents).await;
        assert_answer_shape(case.case, status, &answer, &documents);

        let items = rescore_items(&answer);
        let scores = scores_by_id(&answer);
        let original = case.relevant["OriginalScore"]
            .as_f64()
            .expect("the relevant document's original score");
        let relevant = scores["relevant"];

        // The lift itself: the document whose text answers the query comes back well
        // above the score the search service gave it, measured against the scale the
        // batch's own scores are on.
        let material = MATERIAL_BOOST * batch_scale(&documents);
        assert!(
            relevant > original + material,
            "case {:?}: the relevant document must be lifted materially above its \
             OriginalScore {original}, expected more than {}, got {relevant} in {items:?}",
            case.case,
            original + material
        );

        // And the promotion a caller re-ranks FOR: it outranks every document the query
        // says nothing about, each of which arrived with a higher original score.
        for distractor in case.distractors.as_array().expect("the distractors") {
            let id = distractor["Id"].as_str().expect("the distractor's id");
            let their_original = distractor["OriginalScore"]
                .as_f64()
                .expect("the distractor's original score");
            assert!(
                their_original > original,
                "case {:?}: the row must submit {id:?} with a HIGHER original score than \
                 the relevant document's {original}, got {their_original}",
                case.case
            );
            assert!(
                relevant > scores[id],
                "case {:?}: the relevant document must outrank {id:?}, which arrived with \
                 the higher original score {their_original}, got {relevant} against {} in \
                 {items:?}",
                case.case,
                scores[id]
            );
        }
        assert_eq!(
            items[0].0, "relevant",
            "case {:?}: the relevant document must be ranked first, got {items:?}",
            case.case
        );
    }
}

/// The function words a query is padded with: every one of them is named by the
/// published English stop-word list the handler measures against (the `stop-words`
/// crate's ISO list), so padding a query with them adds nothing the query asks about.
const PADDING: [&str; 5] = [
    "{base}",
    "how do {base}",
    "{base} and what is it",
    "what is the {base} and how does it",
    "the the a of {base} in to for",
];

/// One row of the function-word table: a query's content words, and the documents it is
/// asked over.
struct DilutionCase {
    /// What this row varies.
    case: &'static str,
    /// The query's content words, which every padding keeps.
    base: &'static str,
    documents: Value,
}

fn dilution_cases() -> Vec<DilutionCase> {
    vec![
        DilutionCase {
            case: "one document covering the query, one covering none of it",
            base: "electric car batteries",
            documents: json!([
                { "Id": "covers", "Title": "Electric vehicle batteries",
                  "Body": "Lithium ion batteries power electric cars.", "OriginalScore": 0.2 },
                { "Id": "covers-none", "Title": "Gardening tips",
                  "Body": "Pruning roses in spring.", "OriginalScore": 0.9 },
            ]),
        },
        DilutionCase {
            case: "documents covering part of the query, in both text forms",
            base: "cloud search tuning",
            documents: json!([
                { "Id": "title-only", "Title": "Cloud search", "Body": "a note",
                  "OriginalScore": 0.0 },
                { "Id": "body-only", "Body": "Tuning a search cluster.", "OriginalScore": 0.0 },
                { "Id": "tokenized", "TokenizedTitle": ["Cloud"],
                  "TokenizedBody": ["search", "tuning"], "OriginalScore": 0.0 },
            ]),
        },
        DilutionCase {
            case: "a single content word, over a grouped batch",
            base: "batteries",
            documents: json!([
                { "Id": "m-1", "GroupId": "doc", "Title": "Batteries", "Body": "a note",
                  "OriginalScore": 0.3 },
                { "Id": "m-2", "GroupId": "doc", "Body": "Charge and discharge.",
                  "OriginalScore": 0.1 },
                { "Id": "solo", "Title": "Kayaks", "Body": "Paddling downstream.",
                  "OriginalScore": 0.4 },
            ]),
        },
    ]
}

#[tokio::test]
async fn the_querys_function_words_neither_dilute_nor_change_the_scores() {
    let router = emulator();
    let plan = create_active_plan(&router, "function-word-plan").await;

    for case in dilution_cases() {
        let mut expected: Option<(Vec<(String, f64)>, String)> = None;
        for pattern in PADDING {
            let query = pattern.replace("{base}", case.base);
            let (status, answer) = rescore(&router, &plan, &query, &case.documents).await;
            assert_answer_shape(case.case, status, &answer, &case.documents);
            let items = rescore_items(&answer);
            match &expected {
                None => expected = Some((items, query)),
                Some((baseline, baseline_query)) => {
                    assert_eq!(
                        items.len(),
                        baseline.len(),
                        "case {:?}: query {query:?} answered {items:?}, while \
                         {baseline_query:?} answered {baseline:?}",
                        case.case
                    );
                    for ((id, score), (baseline_id, baseline_score)) in
                        items.iter().zip(baseline.iter())
                    {
                        assert_eq!(
                            id, baseline_id,
                            "case {:?}: query {query:?} must rank the documents exactly as \
                             {baseline_query:?} does, got {items:?} against {baseline:?}",
                            case.case
                        );
                        assert!(
                            (score - baseline_score).abs() < SCORE_TOLERANCE,
                            "case {:?}: the function words of {query:?} must not change \
                             {id:?}'s score, expected {baseline_score} as answered for \
                             {baseline_query:?}, got {score}",
                            case.case
                        );
                    }
                }
            }
        }
    }
}

/// One row of the inflection table: a query term, the form the query spells it in, and
/// another English inflection of the same word. The pairs are inflections of one word,
/// which is what the handler folds together when it compares a query term with a
/// document's.
struct InflectionCase {
    /// What this row varies.
    case: &'static str,
    query: &'static str,
    /// The form the query itself spells.
    exact: &'static str,
    /// Another inflection of the same word.
    inflected: &'static str,
}

#[tokio::test]
async fn a_query_term_matches_the_inflection_a_document_spells_it_in() {
    let router = emulator();
    let plan = create_active_plan(&router, "inflection-plan").await;

    let cases = vec![
        InflectionCase {
            case: "a plural noun against a singular query term",
            query: "battery",
            exact: "battery",
            inflected: "batteries",
        },
        InflectionCase {
            case: "a singular noun against a plural query term",
            query: "cars",
            exact: "cars",
            inflected: "car",
        },
        InflectionCase {
            case: "a plural spelled with a bare s",
            query: "panel",
            exact: "panel",
            inflected: "panels",
        },
        InflectionCase {
            case: "a gerund against the bare verb",
            query: "rank",
            exact: "rank",
            inflected: "ranking",
        },
        InflectionCase {
            case: "a past participle against the bare verb",
            query: "convert",
            exact: "convert",
            inflected: "converted",
        },
    ];

    for case in cases {
        // The two documents differ ONLY in which inflection their title spells, so a
        // difference in their scores is a difference in how the term was matched.
        let documents = json!([
            { "Id": "exact", "Title": case.exact, "Body": "a note", "OriginalScore": 0.0 },
            { "Id": "inflected", "Title": case.inflected, "Body": "a note", "OriginalScore": 0.0 },
            { "Id": "unrelated", "Title": "Kayaks", "Body": "a note", "OriginalScore": 0.0 },
        ]);
        let (status, answer) = rescore(&router, &plan, case.query, &documents).await;
        assert_answer_shape(case.case, status, &answer, &documents);
        let items = rescore_items(&answer);
        let scores = scores_by_id(&answer);

        assert!(
            (scores["exact"] - scores["inflected"]).abs() < SCORE_TOLERANCE,
            "case {:?}: a document spelling {:?} must score as one spelling {:?}, got {} \
             against {} in {items:?}",
            case.case,
            case.inflected,
            case.exact,
            scores["inflected"],
            scores["exact"]
        );
        assert!(
            scores["inflected"] > scores["unrelated"],
            "case {:?}: the query term must match {:?} at all, but it scored {} against \
             the unrelated document's {} in {items:?}",
            case.case,
            case.inflected,
            scores["inflected"],
            scores["unrelated"]
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ROLL-UPS and PILLAR SUMMARIES of a lens review whose lens
//! is a custom lens the caller imported.
//!
//! The rule these pin, over a table of lens documents rather than one recorded request:
//! every derivation over "what does this lens ask?" — the `Risk` roll-up of a lens review
//! and of the workload carrying it, the `QuestionCounts` of a review template's lens
//! review, the pillar review summaries of either, and the `PillarId` a request supplies —
//! is read off the REVIEWED LENS's own content, which for a custom lens is the pillars and
//! questions of the document its published version carries and for an AWS-official lens is
//! its catalog entry. `UNANSWERED` therefore equals the number of questions that lens asks
//! until an answer is recorded, and an answered question moves to the risk its answer
//! carries.
//!
//! The rows differ along the SHAPE of the document — how many pillars it defines and how
//! many questions and choices each of those asks and offers — and, for the roll-up a
//! recorded answer moves, along what the answer selects. The official half of the catalog
//! is attached to the same workload in every row, so a roll-up that counted one half only
//! would fail.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import, publish
//! and document helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_review_answers` already spell.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{
    expected_questions, import_shape, publish, question_count, Shape,
};
use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, review_template, stored_workload, Answer,
    Client,
};

/// The alias of the AWS Well-Architected Framework lens — the official half of the catalog
/// every row attaches beside its custom lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// How many questions that lens asks: the published Well-Architected Framework numbering
/// (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6), spelled out here rather
/// than read back out of the code under test.
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// Every member the `Risk` enum publishes (`smithy_query(query="shape", shape="Risk")`), so
/// a roll-up is checked for ALL its buckets rather than the one a case moved.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// The `Question` buckets a `QuestionCounts` map carries
/// (`smithy_query(query="shape", shape="Question")`).
const QUESTION_BUCKETS: [&str; 2] = ["ANSWERED", "UNANSWERED"];

/// The `LensVersion` every row publishes its imported lens as, which is also the version
/// the review of it is against.
const PUBLISHED_VERSION: &str = "1.0";

/// The `LensStatus` of a review against the lens's latest version
/// (`smithy_query(query="shape", shape="LensStatus")`).
const LENS_STATUS_CURRENT: &str = "CURRENT";

/// The `ValidationExceptionReason` a refused member value is classified under
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers.
const SHAPES: &[(&str, Shape)] = &[
    ("one-question-one-choice", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 4, 2)]),
    (
        "several-pillars",
        &[("operations", 1, 3), ("security", 2, 1), ("cost", 5, 2)],
    ),
    (
        "more-questions-than-one-page",
        &[("reliability", 30, 2), ("sustainability", 25, 1)],
    ),
];

/// The `PillarName` the document of a shape gives the pillar at `index`, mirroring what
/// `super::custom_lens_import::lens_document` writes, so the expectation comes from the
/// document the test sent.
fn expected_pillar_name(index: usize, pillar_id: &str) -> String {
    format!("Pillar {index}: {pillar_id}")
}

/// The name the document of `case` gives its lens, mirroring
/// `super::custom_lens_review_answers::import_shape`.
fn expected_lens_name(case: &str) -> String {
    format!("Lens {case}")
}

/// A workload reviewed against `lenses`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lenses: &[&str]) -> String {
    let mut request = minimal_create(&format!("wl-rollup-{case}"), &format!("tok-wl-{case}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, case)
}

/// `ListLensReviews`: `GET /workloads/{WorkloadId}/lensReviews`.
async fn list_lens_reviews(client: &Client, workload_id: &str, case: &str) -> Vec<Value> {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: LensReviewSummaries must be present, got {answer:?}")
        })
}

/// The ONE listed summary of the review of `lens`, found by the `LensAlias` the attach
/// recorded.
fn summary_of(summaries: &[Value], lens: &str, case: &str) -> Value {
    summaries
        .iter()
        .find(|summary| summary.get("LensAlias").and_then(Value::as_str) == Some(lens))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing must report a review of {lens:?}, got {summaries:?}")
        })
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`. The lens travels
/// percent-encoded, because a custom lens is named by its ARN — which carries `:` and `/`
/// inside ONE path segment.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str, case: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// `UpdateLensReview`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}`.
async fn update_lens_review(client: &Client, workload_id: &str, lens: &str, body: Value) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
}

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn update_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
}

/// One `RiskCounts` map of a response, as a sorted bucket table.
fn risk_counts(owner: &Value, case: &str) -> BTreeMap<String, i64> {
    owner
        .get("RiskCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: RiskCounts must be present, got {owner:?}"))
}

/// One `QuestionCounts` map of a response, as a sorted bucket table.
fn question_counts(owner: &Value, case: &str) -> BTreeMap<String, i64> {
    owner
        .get("QuestionCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_else(|| panic!("case {case:?}: QuestionCounts must be present, got {owner:?}"))
}

/// The roll-up expected over a question set of `total` questions, of which `recorded` names
/// the risk of each question that carries an answer: every other question is `UNANSWERED`,
/// and every bucket the enum publishes is present. A question whose answer leaves it
/// unanswered counts in that same bucket, so `recorded` may name `UNANSWERED` too.
fn expected_risk_counts(total: usize, recorded: &[&str]) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> =
        RISKS.iter().map(|risk| ((*risk).to_string(), 0)).collect();
    for risk in recorded {
        *counts.entry((*risk).to_string()).or_default() += 1;
    }
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    let answered = i64::try_from(recorded.len()).unwrap_or(i64::MAX);
    *counts.entry("UNANSWERED".to_string()).or_default() += total - answered;
    counts
}

/// The question roll-up expected over `total` questions of which `answered` carry an answer.
fn expected_question_counts(total: usize, answered: usize) -> BTreeMap<String, i64> {
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    let answered = i64::try_from(answered).unwrap_or(i64::MAX);
    QUESTION_BUCKETS
        .iter()
        .map(|bucket| {
            let count = if *bucket == "ANSWERED" {
                answered
            } else {
                total - answered
            };
            ((*bucket).to_string(), count)
        })
        .collect()
}

/// The `PillarReviewSummaries` of a lens review, in the order they were served.
fn pillar_summaries(review: &Value, case: &str) -> Vec<Value> {
    review
        .get("PillarReviewSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: PillarReviewSummaries must be present, got {review:?}")
        })
}

fn string_member(owner: &Value, name: &str) -> String {
    owner
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Import every row's lens, wait for the import window once, and publish each: the lens
/// "cannot be applied to workloads … until it's published with CreateLensVersion"
/// (`ImportLens` doc).
async fn published_lenses(client: &Client, shapes: &[(&str, Shape)]) -> Vec<(String, Shape, String)> {
    let mut lenses: Vec<(String, Shape, String)> = Vec::new();
    for (case, shape) in shapes {
        let lens_arn = import_shape(client, case, shape).await;
        lenses.push(((*case).to_string(), *shape, lens_arn));
    }
    wait_for_the_import_to_land();
    for (case, _, lens_arn) in &lenses {
        publish(client, case, lens_arn).await;
    }
    lenses
}

/// Rule 1. A review of a published custom lens identifies that lens by the name and version
/// its document was published as, and its risk roll-up counts exactly the questions that
/// document defines — reported identically by the listing and by the single read, with one
/// pillar summary per pillar the document defines, each counting that pillar's own
/// questions. The AWS-official lens attached beside it keeps counting its catalog's
/// questions, and the workload's own roll-up is the sum of both.
#[tokio::test]
async fn a_review_of_an_imported_lens_counts_the_questions_its_document_defines() {
    let client = Client::new();
    let lenses = published_lenses(&client, SHAPES).await;

    for (case, shape, lens_arn) in &lenses {
        let case = case.as_str();
        let workload_id = workload_reviewing(&client, case, &[lens_arn, FRAMEWORK_LENS]).await;
        let asked = question_count(shape);

        let summaries = list_lens_reviews(&client, &workload_id, case).await;
        let summary = summary_of(&summaries, lens_arn, case);
        assert_eq!(
            string_member(&summary, "LensName"),
            expected_lens_name(case),
            "case {case:?}: the summary must name the lens its document names, got {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "LensVersion"),
            PUBLISHED_VERSION,
            "case {case:?}: the summary must report the published version the review is \
             against, got {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "LensStatus"),
            LENS_STATUS_CURRENT,
            "case {case:?}: a review against the lens's latest version is CURRENT, got \
             {summary:?}"
        );
        assert_eq!(
            string_member(&summary, "LensArn"),
            *lens_arn,
            "case {case:?}: the summary must report the ARN the import minted, got {summary:?}"
        );
        assert_eq!(
            risk_counts(&summary, case),
            expected_risk_counts(asked, &[]),
            "case {case:?}: the listed roll-up must count the {asked} questions the document \
             defines, got {summary:?}"
        );

        // The official half of the catalog, attached to the same workload: still counted
        // from its catalog entry.
        let official = summary_of(&summaries, FRAMEWORK_LENS, case);
        assert_eq!(
            risk_counts(&official, case),
            expected_risk_counts(FRAMEWORK_QUESTIONS, &[]),
            "case {case:?}: the official lens must still count its catalog's \
             {FRAMEWORK_QUESTIONS} questions, got {official:?}"
        );

        // The single read of the same review cannot disagree with the listing, and reports
        // one pillar summary per pillar of the document, in the document's own order.
        let review = get_lens_review(&client, &workload_id, lens_arn, case).await;
        assert_eq!(
            risk_counts(&review, case),
            risk_counts(&summary, case),
            "case {case:?}: the read and the listing must agree about the roll-up, got \
             {review:?}"
        );
        let pillars = pillar_summaries(&review, case);
        let expected_pillars: Vec<(String, String, usize)> = shape
            .iter()
            .enumerate()
            .map(|(index, (pillar_id, questions, _))| {
                (
                    (*pillar_id).to_string(),
                    expected_pillar_name(index, pillar_id),
                    *questions,
                )
            })
            .collect();
        assert_eq!(
            pillars.len(),
            expected_pillars.len(),
            "case {case:?}: the review must report one summary per pillar the document \
             defines, got {pillars:?}"
        );
        for (summary, (pillar_id, pillar_name, questions)) in pillars.iter().zip(&expected_pillars) {
            assert_eq!(
                string_member(summary, "PillarId"),
                *pillar_id,
                "case {case:?}: the pillar summaries must follow the document's order, got \
                 {pillars:?}"
            );
            assert_eq!(
                string_member(summary, "PillarName"),
                *pillar_name,
                "case {case:?}: pillar {pillar_id:?} must carry the name its document gives \
                 it, got {summary:?}"
            );
            assert_eq!(
                risk_counts(summary, case),
                expected_risk_counts(*questions, &[]),
                "case {case:?}: pillar {pillar_id:?} must count its own {questions} questions, \
                 got {summary:?}"
            );
        }

        // The workload's roll-up is the roll-up of every lens it reviews, both halves of the
        // catalog included.
        let workload = stored_workload(&client, &workload_id, case).await;
        assert_eq!(
            risk_counts(&workload, case),
            expected_risk_counts(asked + FRAMEWORK_QUESTIONS, &[]),
            "case {case:?}: the workload must count both its lenses' questions, got {workload:?}"
        );
    }
}

/// Rule 2. Recording an answer to a question of an imported lens MOVES the roll-ups derived
/// from that lens's content: the answered question leaves `UNANSWERED` for the risk its
/// answer carries, in the lens review's roll-up, in the roll-up of the pillar that question
/// belongs to and in the workload's own — while the official lens attached beside it does
/// not move. The rows differ along what the answer selects, which is what the risk is
/// derived from.
#[tokio::test]
async fn an_answer_to_an_imported_lenss_question_moves_the_rollups_of_its_review() {
    let client = Client::new();
    // One document with several pillars, so a pillar's own roll-up can be told from the
    // review's.
    let shape: Shape = &[("operations", 2, 2), ("security", 3, 2)];
    let lenses = published_lenses(&client, &[("answered", shape)]).await;
    let (_, _, lens_arn) = lenses.first().expect("the lens under test must be published");
    let questions = expected_questions(shape);
    let asked = question_count(shape);

    // What an answer records, and the `Risk` the service derives from it: a question that
    // does not apply is NOT_APPLICABLE, one whose answer selects no best practice is still
    // UNANSWERED, one with some choice still unselected is MEDIUM and one with nothing left
    // unselected is NONE (`smithy_query(query="shape", shape="Risk")` publishes the value
    // set).
    let cases: [(&str, usize, bool, &str); 4] = [
        ("nothing-selected", 0, true, "UNANSWERED"),
        ("one-of-two-selected", 1, true, "MEDIUM"),
        ("every-choice-selected", 2, true, "NONE"),
        ("not-applicable", 0, false, "NOT_APPLICABLE"),
    ];

    for (index, (case, selected, is_applicable, expected_risk)) in cases.into_iter().enumerate() {
        // A fresh workload per row, so each row's roll-up is moved by exactly its own
        // answer.
        let workload_id = workload_reviewing(&client, case, &[lens_arn, FRAMEWORK_LENS]).await;
        let question = questions
            .get(index)
            .expect("the document must define a question per row");
        let choices: Vec<String> = question
            .choices
            .iter()
            .take(selected)
            .map(|(choice_id, _)| choice_id.clone())
            .collect();
        let mut body = json!({ "SelectedChoices": choices });
        if !is_applicable {
            body["IsApplicable"] = json!(false);
            body["Reason"] = json!("OUT_OF_SCOPE");
        }
        let written = update_answer(&client, &workload_id, lens_arn, &question.question_id, body)
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the answer to {:?} must be recorded, got {written:?}",
            question.question_id
        );

        let expected_review = expected_risk_counts(asked, &[expected_risk]);
        let review = get_lens_review(&client, &workload_id, lens_arn, case).await;
        assert_eq!(
            risk_counts(&review, case),
            expected_review,
            "case {case:?}: the review's roll-up must move the answered question to \
             {expected_risk}, got {review:?}"
        );
        let summaries = list_lens_reviews(&client, &workload_id, case).await;
        assert_eq!(
            risk_counts(&summary_of(&summaries, lens_arn, case), case),
            expected_review,
            "case {case:?}: the listing must report the same moved roll-up, got {summaries:?}"
        );

        // The pillar the answered question belongs to moves; every other pillar of the
        // document stays as it was.
        for summary in pillar_summaries(&review, case) {
            let pillar_id = string_member(&summary, "PillarId");
            let in_pillar = shape
                .iter()
                .find(|(id, _, _)| *id == pillar_id)
                .map(|(_, questions, _)| *questions)
                .unwrap_or_default();
            let recorded: &[&str] = if pillar_id == question.pillar_id {
                &[expected_risk]
            } else {
                &[]
            };
            assert_eq!(
                risk_counts(&summary, case),
                expected_risk_counts(in_pillar, recorded),
                "case {case:?}: pillar {pillar_id:?} must count its own questions with the \
                 answer this row recorded, got {summary:?}"
            );
        }

        // The official lens's review did not move, and the workload's roll-up is both.
        let official = summary_of(&summaries, FRAMEWORK_LENS, case);
        assert_eq!(
            risk_counts(&official, case),
            expected_risk_counts(FRAMEWORK_QUESTIONS, &[]),
            "case {case:?}: an answer to the custom lens must not move the official lens's \
             roll-up, got {official:?}"
        );
        let workload = stored_workload(&client, &workload_id, case).await;
        assert_eq!(
            risk_counts(&workload, case),
            expected_risk_counts(asked + FRAMEWORK_QUESTIONS, &[expected_risk]),
            "case {case:?}: the workload's roll-up must carry the answer too, got {workload:?}"
        );
    }
}

/// Rule 3. The same resolution serves a REVIEW TEMPLATE's lens review: its `QuestionCounts`
/// and those of each of its pillar summaries count the questions the applied lens's own
/// content defines, and the template's roll-up spans every lens it applies.
#[tokio::test]
async fn a_review_templates_imported_lens_review_counts_that_documents_questions() {
    let client = Client::new();
    let lenses = published_lenses(&client, SHAPES).await;

    for (case, shape, lens_arn) in &lenses {
        let case = case.as_str();
        let template_arn = review_template(
            &client,
            &format!("tpl-rollup-{case}"),
            &[lens_arn, FRAMEWORK_LENS],
        )
        .await;
        let asked = question_count(shape);

        let answer = client
            .get_review_template_lens_review(&template_arn, lens_arn)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
        );
        let review = answer
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"));
        assert_eq!(
            string_member(&review, "LensName"),
            expected_lens_name(case),
            "case {case:?}: the template's review must name the lens its document names, got \
             {review:?}"
        );
        assert_eq!(
            question_counts(&review, case),
            expected_question_counts(asked, 0),
            "case {case:?}: the template's review must count the {asked} questions the \
             document defines, got {review:?}"
        );

        let pillars = pillar_summaries(&review, case);
        assert_eq!(
            pillars.len(),
            shape.len(),
            "case {case:?}: one pillar summary per pillar the document defines, got {pillars:?}"
        );
        for (summary, (index, (pillar_id, questions, _))) in
            pillars.iter().zip(shape.iter().enumerate())
        {
            assert_eq!(
                string_member(summary, "PillarId"),
                *pillar_id,
                "case {case:?}: the pillar summaries must follow the document's order, got \
                 {pillars:?}"
            );
            assert_eq!(
                string_member(summary, "PillarName"),
                expected_pillar_name(index, pillar_id),
                "case {case:?}: pillar {pillar_id:?} must carry the name its document gives \
                 it, got {summary:?}"
            );
            assert_eq!(
                question_counts(summary, case),
                expected_question_counts(*questions, 0),
                "case {case:?}: pillar {pillar_id:?} must count its own {questions} questions, \
                 got {summary:?}"
            );
        }

        // The template's own roll-up spans both applied lenses.
        let fetched = client.get_review_template(&template_arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetReviewTemplate 200, got {fetched:?}"
        );
        let template = fetched
            .body
            .get("ReviewTemplate")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: ReviewTemplate must be present, got {fetched:?}")
            });
        assert_eq!(
            question_counts(&template, case),
            expected_question_counts(asked + FRAMEWORK_QUESTIONS, 0),
            "case {case:?}: the template must count both applied lenses' questions, got \
             {template:?}"
        );
    }
}

/// Rule 4. A `PillarId` a request supplies for a review of an imported lens is judged
/// against THAT document's pillars: a pillar the document defines is accepted and its notes
/// are reported back on that pillar's summary, and one it does not define — including a
/// pillar of the AWS-official framework lens — is the modelled 400.
#[tokio::test]
async fn a_pillar_note_is_judged_against_the_imported_documents_pillars() {
    let client = Client::new();
    let shape: Shape = &[("governance", 1, 2), ("resilience", 2, 1)];
    let lenses = published_lenses(&client, &[("pillar-notes", shape)]).await;
    let (_, _, lens_arn) = lenses.first().expect("the lens under test must be published");
    let workload_id = workload_reviewing(&client, "pillar-notes", &[lens_arn]).await;

    // Each pillar the document defines takes notes, by the id the document gives it.
    let notes: BTreeMap<String, String> = shape
        .iter()
        .map(|(pillar_id, _, _)| {
            (
                (*pillar_id).to_string(),
                format!("notes about {pillar_id}"),
            )
        })
        .collect();
    let written = update_lens_review(
        &client,
        &workload_id,
        lens_arn,
        json!({ "PillarNotes": notes }),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "the notes of every pillar the document defines must be accepted, got {written:?}"
    );
    let review = get_lens_review(&client, &workload_id, lens_arn, "pillar-notes").await;
    for summary in pillar_summaries(&review, "pillar-notes") {
        let pillar_id = string_member(&summary, "PillarId");
        assert_eq!(
            string_member(&summary, "Notes"),
            notes.get(&pillar_id).cloned().unwrap_or_default(),
            "case {pillar_id:?}: the notes written for the pillar must be read back on its \
             summary, got {summary:?}"
        );
    }

    // A key naming no pillar of THIS document: one of the framework lens's pillars, an id
    // of no lens at all, and a defined pillar beside an undefined one.
    for (case, keys) in [
        ("official-lenss-pillar", vec!["security"]),
        ("no-lenss-pillar", vec!["not-a-pillar"]),
        (
            "a-defined-and-an-undefined-pillar",
            vec!["governance", "sustainability"],
        ),
    ] {
        let body = json!({
            "PillarNotes": keys
                .iter()
                .map(|key| ((*key).to_string(), json!("notes")))
                .collect::<serde_json::Map<String, Value>>(),
        });
        let refused = update_lens_review(&client, &workload_id, lens_arn, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a pillar the document does not define must be refused, got \
             {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.body.get("Reason").and_then(Value::as_str),
            Some(REASON_FIELD_VALIDATION_FAILED),
            "case {case:?}: a refused member value is a field validation failure, got {refused:?}"
        );
    }
}

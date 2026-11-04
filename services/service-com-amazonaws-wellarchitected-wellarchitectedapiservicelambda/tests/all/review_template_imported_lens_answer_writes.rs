// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateReviewTemplateAnswer` when the lens the review template
//! applies is a CUSTOM lens the caller imported.
//!
//! The rule these pin, over tables of lens documents and of lens spellings rather than one
//! recorded request: the question a write of a lens review's answer names is resolved from
//! the content of the LENS UNDER REVIEW — an AWS-official lens's catalog entry, or the
//! document a custom lens's applied version was published from — which is the same
//! resolution every READ of those answers goes through, so every question a read of the
//! review reports is answerable and only a `QuestionId` that lens does not ask names no
//! answer to write. A lens is one lens however a request spells it, so a write that names
//! it by its ARN and a write that names it by a version-pinned ARN land on the same review.
//!
//! The rows differ along the SHAPE of the document — how many pillars it defines, how many
//! questions each pillar asks and how many choices each question offers — along WHICH of a
//! question's choices the answer selects, and along the spelling the request names the lens
//! by, because those are what the recorded answer, the question roll-ups derived from it
//! and the review the write lands on move with.
//!
//! `super::custom_lens_answer_writes` pins the same rule across the WORKLOAD write
//! (`UpdateAnswer`) with one shape of review-template write beside it;
//! `super::review_template_imported_lens_answers` pins the review-template READS. This
//! suite is the review-template WRITE on its own: every question of every shape, the
//! roll-ups each recorded answer moves, and either spelling of the lens. The document
//! builder, the import/publish wire helpers and the expectation derived from a document's
//! shape are `super::custom_lens_import`'s and `super::custom_lens_review_answers`', reused
//! here rather than respelled.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{
    expected_questions, import_shape, publish, reported_choices, string_member, ExpectedQuestion,
    Shape,
};
use super::workload_harness::{
    assert_field_validation, member, review_template, Answer, Client,
};

/// The `LensVersion` every row publishes its imported lens as, which is also the version a
/// review of it is against.
const PUBLISHED_VERSION: &str = "1.0";

/// The `ReviewTemplateAnswerStatus` values, from
/// `smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// Two members of the `ChoiceStatus` value set
/// (`smithy_query(query="shape", shape="ChoiceStatus")`): the status of a choice an answer
/// records as in place, and of one it does not.
const SELECTED: &str = "SELECTED";
const UNSELECTED: &str = "UNSELECTED";

/// A `QuestionId` of the AWS Well-Architected Framework lens, as this emulator's own stable
/// ids spell it (pinned by `super::question_choice_catalog`): a question no imported
/// document under test defines, so it names no answer of a custom lens's review.
const FRAMEWORK_QUESTION: &str = "ops-01";

/// Which of a question's choices an answer records as in place. The plans span the shape of
/// a selection — nothing, one choice, every other choice and all of them — which is what
/// the recorded per-choice statuses and the `SelectedChoices` read back move with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    None,
    First,
    EveryOther,
    All,
}

impl Selection {
    /// The `ChoiceId`s this plan selects out of a question's own catalog, in the catalog's
    /// order.
    fn of(self, choices: &[String]) -> Vec<String> {
        match self {
            Selection::None => Vec::new(),
            Selection::First => choices.iter().take(1).cloned().collect(),
            Selection::EveryOther => choices.iter().step_by(2).cloned().collect(),
            Selection::All => choices.to_vec(),
        }
    }
}

const SELECTIONS: [Selection; 4] = [
    Selection::None,
    Selection::First,
    Selection::EveryOther,
    Selection::All,
];

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers — including a
/// question that offers exactly ONE choice, where "the first choice" and "every choice" are
/// the same selection.
const SHAPES: &[(&str, Shape)] = &[
    ("one-question-one-choice", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 3, 2)]),
    (
        "several-pillars",
        &[("operations", 1, 4), ("security", 2, 1), ("cost", 3, 2)],
    ),
];

/// The `Answer` of a successful write or read, with the envelope checked: the template the
/// answer belongs to and the lens AS THE REQUEST NAMED IT.
fn answer_of(case: &str, answer: &Answer, template_arn: &str, lens: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        member(answer, "TemplateArn"),
        template_arn,
        "case {case:?}: the envelope must name the review template, got {answer:?}"
    );
    assert_eq!(
        member(answer, "LensAlias"),
        lens,
        "case {case:?}: the envelope must name the lens as sent, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Answer must be present, got {answer:?}"))
}

fn strings(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The answer's `ChoiceAnswers` as a `ChoiceId` -> `ChoiceStatus` table.
fn choice_statuses(answer: &Value) -> BTreeMap<String, String> {
    answer
        .get("ChoiceAnswers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        string_member(entry, "ChoiceId"),
                        string_member(entry, "Status"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert an answer records what the request wrote, against the question the DOCUMENT
/// defines: the question's identity, its whole choice catalog, the choices in place, one
/// status per choice the question offers, the notes written and the answered state.
fn assert_records(
    case: &str,
    answer: &Value,
    question: &ExpectedQuestion,
    selected: &[String],
    notes: &str,
) {
    assert_eq!(
        string_member(answer, "QuestionId"),
        question.question_id,
        "case {case:?}: the answer must name the question it was written for, got {answer:?}"
    );
    assert_eq!(
        string_member(answer, "PillarId"),
        question.pillar_id,
        "case {case:?}: the answer must name the pillar that asks the question, got {answer:?}"
    );
    assert_eq!(
        string_member(answer, "QuestionTitle"),
        question.title,
        "case {case:?}: the question's title comes from the lens document, got {answer:?}"
    );
    assert_eq!(
        reported_choices(answer),
        question.choices,
        "case {case:?}: the answer must report the choice catalog its lens defines, got {answer:?}"
    );
    assert_eq!(
        strings(answer, "SelectedChoices"),
        selected.to_vec(),
        "case {case:?}: expected SelectedChoices {selected:?}, got {answer:?}"
    );
    let expected: BTreeMap<String, String> = question
        .choices
        .iter()
        .map(|(choice_id, _)| {
            let status = if selected.contains(choice_id) {
                SELECTED
            } else {
                UNSELECTED
            };
            (choice_id.clone(), status.to_string())
        })
        .collect();
    let actual = choice_statuses(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected ChoiceAnswers statuses {expected:?}, got {actual:?}"
    );
    assert_eq!(
        string_member(answer, "Notes"),
        notes,
        "case {case:?}: the answer must carry the notes the request wrote, got {answer:?}"
    );
    assert_eq!(
        answer.get("AnswerStatus").and_then(Value::as_str),
        Some(ANSWERED),
        "case {case:?}: a question the template has answered is {ANSWERED}, got {answer:?}"
    );
}

/// The `QuestionId`s a review template's lens review lists, in the order served — the
/// question set as a CLIENT discovers it before writing to it.
async fn listed_questions(client: &Client, case: &str, template_arn: &str, lens: &str) -> Vec<Value> {
    let answer = client
        .list_review_template_answers(template_arn, lens, "")
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the template's lens review must be readable, got {answer:?}"
    );
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `QuestionCounts` roll-up of ONE lens review of a review template, as a bucket table.
async fn lens_review_counts(
    client: &Client,
    case: &str,
    template_arn: &str,
    lens: &str,
) -> BTreeMap<String, i64> {
    let answer = client
        .get_review_template_lens_review(template_arn, lens)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetReviewTemplateLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("QuestionCounts"))
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// The two buckets a `QuestionCounts` map carries
/// (`smithy_query(query="shape", shape="Question")`), with the counts a case expects.
fn expected_counts(answered: i64, unanswered: i64) -> BTreeMap<String, i64> {
    [
        (ANSWERED.to_string(), answered),
        (UNANSWERED.to_string(), unanswered),
    ]
    .into_iter()
    .collect()
}

/// Rule: every question a review template's review of a published custom lens reports is
/// answerable, the answer reads back as written, and each recorded answer moves that
/// review's question roll-up — over every shape of document.
#[tokio::test]
async fn every_question_a_templates_imported_lens_defines_is_answerable() {
    let client = Client::new();

    // Every row's lens is imported first, so the whole table waits for the import window
    // once rather than per row.
    let mut lenses: Vec<(&str, Shape, String)> = Vec::new();
    for (case, shape) in SHAPES {
        let lens_arn = import_shape(&client, case, shape).await;
        lenses.push((case, shape, lens_arn));
    }
    wait_for_the_import_to_land();

    for (case, shape, lens_arn) in &lenses {
        publish(&client, case, lens_arn).await;
        let template_arn = review_template(&client, &format!("tmpl-{case}"), &[lens_arn]).await;

        // What a client sees before it writes: the questions the document defines, in the
        // document's own order.
        let expected = expected_questions(shape);
        let summaries = listed_questions(&client, case, &template_arn, lens_arn).await;
        assert_eq!(
            summaries
                .iter()
                .map(|summary| string_member(summary, "QuestionId"))
                .collect::<Vec<String>>(),
            expected
                .iter()
                .map(|question| question.question_id.clone())
                .collect::<Vec<String>>(),
            "case {case:?}: the review must list the questions the document defines, got \
             {summaries:?}"
        );
        let total = expected.len() as i64;
        assert_eq!(
            lens_review_counts(&client, case, &template_arn, lens_arn).await,
            expected_counts(0, total),
            "case {case:?}: nothing has answered the review yet"
        );

        for (position, question) in expected.iter().enumerate() {
            let choices: Vec<String> = question
                .choices
                .iter()
                .map(|(choice_id, _)| choice_id.clone())
                .collect();
            let selection = SELECTIONS[position % SELECTIONS.len()];
            let selected = selection.of(&choices);
            let case = format!("{case}/{}/{selection:?}", question.question_id);
            let notes = format!("notes for {}", question.question_id);

            let written = answer_of(
                &case,
                &client
                    .update_review_template_answer(
                        &template_arn,
                        lens_arn,
                        &question.question_id,
                        json!({ "SelectedChoices": selected, "Notes": notes }),
                    )
                    .await,
                &template_arn,
                lens_arn,
            );
            assert_records(&case, &written, question, &selected, &notes);

            // The answer a later caller reads is the answer the write recorded.
            let read = answer_of(
                &format!("{case}-read-back"),
                &client
                    .get_review_template_answer(&template_arn, lens_arn, &question.question_id)
                    .await,
                &template_arn,
                lens_arn,
            );
            assert_eq!(
                read, written,
                "case {case:?}: the answer read back must be the one the write recorded, got \
                 {read:?}"
            );

            // Recording an answer is what makes the question ANSWERED in the review's
            // roll-up, so the buckets move with every write.
            let answered = position as i64 + 1;
            assert_eq!(
                lens_review_counts(&client, &case, &template_arn, lens_arn).await,
                expected_counts(answered, total - answered),
                "case {case:?}: the review must count {answered} of {total} questions answered"
            );
        }
    }
}

/// Rule: a lens is ONE lens however a request spells it, so a write that names the custom
/// lens by its ARN and one that names it by a version-pinned ARN resolve the same question
/// set and land on the same review — and a `QuestionId` the document does not define is a
/// 404 under either spelling.
#[tokio::test]
async fn a_write_reaches_the_same_review_however_it_spells_the_imported_lens() {
    const SHAPE: Shape = &[("governance", 2, 3)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "spellings", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "spellings", &lens_arn).await;
    let template_arn = review_template(&client, "tmpl-spellings", &[&lens_arn]).await;

    let expected = expected_questions(SHAPE);
    let total = expected.len() as i64;

    // The spellings under test: the lens ARN, and the same ARN with the applied version
    // pinned after it ("A lens ARN may also pin a version after the alias").
    let spellings: [(&str, String); 2] = [
        ("arn", lens_arn.clone()),
        (
            "version-pinned-arn",
            format!("{lens_arn}/{PUBLISHED_VERSION}"),
        ),
    ];

    for (row, (label, spelling)) in spellings.iter().enumerate() {
        let question = &expected[row];
        let choices: Vec<String> = question
            .choices
            .iter()
            .map(|(choice_id, _)| choice_id.clone())
            .collect();
        let selected = Selection::First.of(&choices);
        let case = format!("{label}/{}", question.question_id);
        let notes = format!("written through the {label}");

        let written = answer_of(
            &case,
            &client
                .update_review_template_answer(
                    &template_arn,
                    spelling,
                    &question.question_id,
                    json!({ "SelectedChoices": selected, "Notes": notes }),
                )
                .await,
            &template_arn,
            spelling,
        );
        assert_records(&case, &written, question, &selected, &notes);

        // Read back through the OTHER spelling: one review, so the answer written under
        // either name is the answer either name reads.
        let other = &spellings[(row + 1) % spellings.len()].1;
        let read = answer_of(
            &format!("{case}-read-through-the-other-spelling"),
            &client
                .get_review_template_answer(&template_arn, other, &question.question_id)
                .await,
            &template_arn,
            other,
        );
        assert_records(
            &format!("{case}-read-through-the-other-spelling"),
            &read,
            question,
            &selected,
            &notes,
        );

        // And the one review's roll-up counts both writes, under either spelling.
        let answered = row as i64 + 1;
        for (label, spelling) in &spellings {
            assert_eq!(
                lens_review_counts(&client, &case, &template_arn, spelling).await,
                expected_counts(answered, total - answered),
                "case {case:?}: the review read as {label:?} must count {answered} of {total} \
                 questions answered"
            );
        }
    }

    // A `QuestionId` the document does not define names no answer of this review under
    // either spelling, and the refusal names the lens as sent.
    for (label, spelling) in &spellings {
        let refused = client
            .update_review_template_answer(
                &template_arn,
                spelling,
                FRAMEWORK_QUESTION,
                json!({ "Notes": "nothing to record" }),
            )
            .await;
        assert_field_validation(
            &format!("unasked-question/{label}"),
            &refused,
            "QuestionId",
            &format!(
                "No question with ID {FRAMEWORK_QUESTION} was found in the lens with alias \
                 {spelling}."
            ),
        );
    }
}

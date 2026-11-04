// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetReviewTemplateAnswer`: the answer a review template
//! records for ONE question of ONE of its lens reviews.
//!
//! The rule these pin, over TABLES of inputs rather than one request: the read reports the
//! question as the REVIEWED LENS defines it — its id, the pillar that asks it, its title
//! and the whole choice catalog it asks about — together with the answer the TEMPLATE
//! records for that question: the choices in place, what the answer says about each choice
//! it speaks about, whether the question applies and the reason it does not. A question the
//! template records no answer for still HAS an answer to report: the `UNANSWERED` state,
//! which applies, selects nothing and speaks about no choice. The same report is served by
//! either spelling of the lens (`LensAlias` is "either the lens alias … or the lens ARN")
//! and for either half of the lens catalog — an AWS-official lens and an imported custom
//! one — and it agrees member for member with the `ReviewTemplateAnswerSummary` that
//! `ListReviewTemplateAnswers` gives the same question, because both project one stored
//! answer.
//!
//! The rows span EVERY question the applied lens asks — not one cited question — crossed
//! with the states an answer can be stored in: nothing recorded, an answer selecting no
//! choice, one selecting a single choice, one selecting every choice the question offers,
//! and one that says the question does not apply.
//!
//! `super::review_template_answer_updates` pins the WRITES these reads report and the
//! refusals both resolve; this suite is the report itself, over the whole question set of
//! each half of the catalog.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the answer seeding and the lens-document helpers live in
//! the shared `super::workload_harness`, `super::custom_lens_import` and
//! `super::custom_lens_review_answers`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord;

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{
    expected_questions, import_shape, publish, question_count, reported_choices, string_member,
    Shape,
};
use super::workload_harness::{
    answered, assert_field_validation, official_lens_arn, review_template as template, Answer,
    Client,
    SIGNED_REGION,
};

/// The alias of the AWS Well-Architected Framework lens, and the pillars it organizes its
/// questions into with the published question numbering of each: OPS 1-11, SEC 1-11,
/// REL 1-13, PERF 1-5, COST 1-11, SUS 1-6. Spelled out here so the table pins the catalog
/// rather than reading it back out of the code under test.
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_PILLAR_QUESTIONS: [(&str, usize); 6] = [
    ("operationalExcellence", 11),
    ("security", 11),
    ("reliability", 13),
    ("performance", 5),
    ("costOptimization", 11),
    ("sustainability", 6),
];

/// The `ReviewTemplateAnswerStatus` values, from
/// `smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// Three of the `ChoiceStatus` values, from
/// `smithy_query(query="shape", shape="ChoiceStatus")` (`SELECTED` | `NOT_APPLICABLE` |
/// `UNSELECTED`).
const SELECTED: &str = "SELECTED";
const UNSELECTED: &str = "UNSELECTED";
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// One of the `AnswerReason` values, from
/// `smithy_query(query="shape", shape="AnswerReason")` — "the reason why the question is
/// not applicable to your review template".
const OUT_OF_SCOPE: &str = "OUT_OF_SCOPE";

/// The state an answer is stored in before the read — what a row varies, because the
/// report follows the stored answer and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    /// The template records no answer for the question at all.
    Nothing,
    /// An answer that selects none of the choices the question offers.
    NoChoice,
    /// An answer that selects the first choice the question offers.
    FirstChoice,
    /// An answer that selects every choice the question offers.
    EveryChoice,
    /// An answer that says the question does not apply, with the reason why.
    NotApplicable,
}

/// The states the rows cycle through, so every question of a lens is read in one of them
/// and each state is exercised on questions whose choice catalogs differ in size.
const RECORDED: [Recorded; 5] = [
    Recorded::Nothing,
    Recorded::NoChoice,
    Recorded::FirstChoice,
    Recorded::EveryChoice,
    Recorded::NotApplicable,
];

/// What `GetReviewTemplateAnswer` must report about the ANSWER of a question in one
/// recorded state; the question's own identity and catalog are checked beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expected {
    answer_status: &'static str,
    selected: Vec<String>,
    choice_statuses: BTreeMap<String, String>,
    is_applicable: bool,
    reason: Option<&'static str>,
}

/// The answer one state records for a question offering `offered` — `None` for the state
/// that records nothing — and the report the read must then give. Built together so the
/// expectation cannot drift from what was stored.
fn recorded_answer(state: Recorded, offered: &[String]) -> (Option<AnswerRecord>, Expected) {
    let selected: Vec<String> = match state {
        Recorded::Nothing | Recorded::NoChoice | Recorded::NotApplicable => Vec::new(),
        Recorded::FirstChoice => offered[..1].to_vec(),
        Recorded::EveryChoice => offered.to_vec(),
    };
    let choice_statuses: BTreeMap<String, String> = match state {
        // The key space an answer's per-choice statuses live in is the question's own
        // choice set, so every state that records an answer records one status per choice
        // the question offers — a best practice is not in place until an answer says it is.
        Recorded::Nothing => BTreeMap::new(),
        Recorded::NoChoice | Recorded::FirstChoice | Recorded::EveryChoice => offered
            .iter()
            .map(|choice_id| {
                let status = if selected.contains(choice_id) {
                    SELECTED
                } else {
                    UNSELECTED
                };
                (choice_id.clone(), status.to_string())
            })
            .collect(),
        Recorded::NotApplicable => offered
            .iter()
            .map(|choice_id| (choice_id.clone(), NOT_APPLICABLE.to_string()))
            .collect(),
    };
    let is_applicable = state != Recorded::NotApplicable;
    let reason = if state == Recorded::NotApplicable {
        Some(OUT_OF_SCOPE)
    } else {
        None
    };

    let expected = Expected {
        answer_status: if state == Recorded::Nothing {
            UNANSWERED
        } else {
            ANSWERED
        },
        selected: selected.clone(),
        choice_statuses: choice_statuses.clone(),
        is_applicable,
        reason,
    };
    let record = if state == Recorded::Nothing {
        None
    } else {
        let choice_answers: Vec<(&str, &str)> = choice_statuses
            .iter()
            .map(|(choice_id, status)| (choice_id.as_str(), status.as_str()))
            .collect();
        Some(answered(&choice_answers, is_applicable, reason))
    };
    (record, expected)
}

fn string_list(value: &Value, name: &str) -> Vec<String> {
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

/// The `ChoiceId` -> status table of a list of per-choice entries, which is how an answer's
/// `ChoiceAnswers` and a summary's `ChoiceAnswerSummaries` both report what the answer says
/// about each choice it speaks about.
fn choice_statuses(value: &Value, member: &str) -> BTreeMap<String, String> {
    value
        .get(member)
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

/// The `Answer` of a successful read, with the envelope checked: the template as STORED and
/// the lens as the REQUEST named it.
fn answer_of(case: &str, answer: &Answer, template_arn: &str, lens: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("TemplateArn").and_then(Value::as_str),
        Some(template_arn),
        "case {case:?}: TemplateArn must be the template the answer belongs to, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("LensAlias").and_then(Value::as_str),
        Some(lens),
        "case {case:?}: LensAlias must be the lens as the request named it, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Answer must be present, got {answer:?}"))
}

/// Assert the reported answer records what `expected` says, member by member.
fn assert_records(case: &str, answer: &Value, expected: &Expected) {
    assert_eq!(
        answer.get("AnswerStatus").and_then(Value::as_str),
        Some(expected.answer_status),
        "case {case:?}: expected AnswerStatus {:?}, got {answer:?}",
        expected.answer_status
    );
    assert_eq!(
        string_list(answer, "SelectedChoices"),
        expected.selected,
        "case {case:?}: expected SelectedChoices {:?}, got {answer:?}",
        expected.selected
    );
    let statuses = choice_statuses(answer, "ChoiceAnswers");
    assert_eq!(
        statuses, expected.choice_statuses,
        "case {case:?}: expected ChoiceAnswers {:?}, got {statuses:?}",
        expected.choice_statuses
    );
    assert_eq!(
        answer.get("IsApplicable").and_then(Value::as_bool),
        Some(expected.is_applicable),
        "case {case:?}: expected IsApplicable {:?}, got {answer:?}",
        expected.is_applicable
    );
    assert_eq!(
        answer.get("Reason").and_then(Value::as_str),
        expected.reason,
        "case {case:?}: expected Reason {:?}, got {answer:?}",
        expected.reason
    );
}

/// Assert the reported answer and the summary `ListReviewTemplateAnswers` gives the same
/// question report ONE stored answer: the question's identity, its catalog and everything
/// the summary also carries agree.
fn assert_agrees_with_summary(case: &str, answer: &Value, summary: &Value) {
    for name in ["QuestionId", "PillarId", "QuestionTitle", "AnswerStatus"] {
        assert_eq!(
            string_member(answer, name),
            string_member(summary, name),
            "case {case:?}: the read and the listing must report the same {name}, got \
             {answer:?} and {summary:?}"
        );
    }
    assert_eq!(
        reported_choices(answer),
        reported_choices(summary),
        "case {case:?}: the read and the listing must report the same choice catalog, got \
         {answer:?} and {summary:?}"
    );
    assert_eq!(
        string_list(answer, "SelectedChoices"),
        string_list(summary, "SelectedChoices"),
        "case {case:?}: the read and the listing must report the same SelectedChoices, got \
         {answer:?} and {summary:?}"
    );
    assert_eq!(
        choice_statuses(answer, "ChoiceAnswers"),
        choice_statuses(summary, "ChoiceAnswerSummaries"),
        "case {case:?}: the read and the listing must say the same about each choice, got \
         {answer:?} and {summary:?}"
    );
    assert_eq!(
        answer.get("IsApplicable").and_then(Value::as_bool),
        summary.get("IsApplicable").and_then(Value::as_bool),
        "case {case:?}: the read and the listing must agree on IsApplicable, got {answer:?} \
         and {summary:?}"
    );
    assert_eq!(
        answer.get("Reason").and_then(Value::as_str),
        summary.get("Reason").and_then(Value::as_str),
        "case {case:?}: the read and the listing must agree on Reason, got {answer:?} and \
         {summary:?}"
    );
}

/// Every `ReviewTemplateAnswerSummary` of one template lens review, following `NextToken`
/// to the end: the question set of the reviewed lens, in the lens's own order.
async fn all_summaries(client: &Client, case: &str, template_arn: &str, lens: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let query = match &next_token {
            Some(token) => format!("NextToken={token}"),
            None => String::new(),
        };
        let answer = client
            .list_review_template_answers(template_arn, lens, &query)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} of the listing must be a 200, got {answer:?}"
        );
        collected.extend(
            answer
                .body
                .get("AnswerSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        pages += 1;
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            pages <= collected.len() + 1,
            "case {case:?}: paging must terminate, got {pages} pages for {} summaries",
            collected.len()
        );
        if next_token.is_none() {
            break;
        }
    }

    assert!(
        !collected.is_empty(),
        "case {case:?}: an applied lens asks questions, got no summary at all"
    );
    collected
}

/// Read one question through `GetReviewTemplateAnswer` and assert the whole report: the
/// question as the lens defines it (checked against `catalog`, the question's entry in the
/// lens's own question set), the answer `expected` describes, and agreement with the
/// summary the listing gives the same question.
async fn assert_read(
    case: &str,
    client: &Client,
    template_arn: &str,
    lens: &str,
    catalog: &Value,
    expected: &Expected,
) {
    let question_id = string_member(catalog, "QuestionId");
    let answer = answer_of(
        case,
        &client
            .get_review_template_answer(template_arn, lens, &question_id)
            .await,
        template_arn,
        lens,
    );

    assert_eq!(
        string_member(&answer, "QuestionId"),
        question_id,
        "case {case:?}: the report must name the question it was asked for, got {answer:?}"
    );
    assert_eq!(
        string_member(&answer, "PillarId"),
        string_member(catalog, "PillarId"),
        "case {case:?}: the pillar is the one the lens files the question under, got {answer:?}"
    );
    assert_eq!(
        string_member(&answer, "QuestionTitle"),
        string_member(catalog, "QuestionTitle"),
        "case {case:?}: the title is the one the lens gives the question, got {answer:?}"
    );
    assert_eq!(
        reported_choices(&answer),
        reported_choices(catalog),
        "case {case:?}: the choice catalog is the one the lens defines, got {answer:?}"
    );
    assert_records(case, &answer, expected);
    assert_agrees_with_summary(case, &answer, catalog);
}

/// Rule, over the AWS-official half of the catalog: every one of the framework lens's
/// questions reports the answer the template stores for it, by either spelling of the lens,
/// and agrees with the listing. The rows are the whole question set crossed with the five
/// states an answer can be stored in.
#[tokio::test]
async fn every_question_of_an_official_lens_review_reports_the_stored_answer() {
    let client = Client::new();
    let template_arn = template(&client, "answer-reads-official", &[FRAMEWORK_LENS]).await;
    let lens_arn = official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS);

    // The question set as the lens defines it, before anything is answered.
    let catalog = all_summaries(&client, "catalog", &template_arn, FRAMEWORK_LENS).await;
    let asked: usize = FRAMEWORK_PILLAR_QUESTIONS
        .iter()
        .map(|(_, questions)| questions)
        .sum();
    assert_eq!(
        catalog.len(),
        asked,
        "the framework lens asks {asked} questions, got {}",
        catalog.len()
    );
    let published_pillars: BTreeSet<&str> = FRAMEWORK_PILLAR_QUESTIONS
        .iter()
        .map(|(pillar_id, _)| *pillar_id)
        .collect();

    // Record one answer state per question, cycling through the states.
    let mut expectations: Vec<(String, Expected)> = Vec::new();
    for (position, question) in catalog.iter().enumerate() {
        let question_id = string_member(question, "QuestionId");
        let pillar_id = string_member(question, "PillarId");
        assert!(
            published_pillars.contains(pillar_id.as_str()),
            "case {question_id:?}: the question belongs to a published pillar, got \
             {pillar_id:?}"
        );
        let offered: Vec<String> = reported_choices(question)
            .into_iter()
            .map(|(choice_id, _)| choice_id)
            .collect();
        assert!(
            !offered.is_empty(),
            "case {question_id:?}: a question asks about at least one best practice, got \
             {question:?}"
        );

        let state = RECORDED[position % RECORDED.len()];
        let (record, expected) = recorded_answer(state, &offered);
        if let Some(record) = record {
            // Seeded through the shared state so one test can cover the lens's whole
            // question set; the reads themselves all cross the HTTP boundary.
            client.record_template_answer(&template_arn, FRAMEWORK_LENS, &question_id, record);
        }
        expectations.push((format!("{question_id}/{state:?}"), expected));
    }

    // The listing now reports the stored answers; the read of each question must match it.
    let answered_set = all_summaries(&client, "answered", &template_arn, FRAMEWORK_LENS).await;
    assert_eq!(
        answered_set.len(),
        catalog.len(),
        "recording answers must not change which questions the lens asks, got {} summaries",
        answered_set.len()
    );

    for (position, (case, expected)) in expectations.iter().enumerate() {
        // Half the rows name the lens by its alias and half by its AWS-official ARN: the
        // same lens review either way.
        let lens = if position % 2 == 0 {
            FRAMEWORK_LENS
        } else {
            lens_arn.as_str()
        };
        assert_read(
            case,
            &client,
            &template_arn,
            lens,
            &answered_set[position],
            expected,
        )
        .await;
    }
}

/// Rule, over the CUSTOM half of the catalog: the questions a review of an imported lens
/// reports are the ones its document defines, and each reports the answer the template
/// stores for it. The expectations come from the document the test sent, not from the
/// service that answered.
#[tokio::test]
async fn every_question_of_an_imported_lens_review_reports_the_stored_answer() {
    const SHAPE: Shape = &[("governance", 2, 2), ("cost", 3, 1)];

    let client = Client::new();
    let case = "answer-reads-imported";
    let lens_arn = import_shape(&client, case, SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, case, &lens_arn).await;

    // The framework lens is applied beside the imported one, so a review that resolved the
    // wrong half of the catalog would fail here.
    let template_arn = template(&client, case, &[&lens_arn, FRAMEWORK_LENS]).await;

    let summaries = all_summaries(&client, case, &template_arn, &lens_arn).await;
    assert_eq!(
        summaries.len(),
        question_count(SHAPE),
        "the review asks the questions the document defines, got {} summaries",
        summaries.len()
    );
    let defined = expected_questions(SHAPE);
    for (position, question) in defined.iter().enumerate() {
        let reported = &summaries[position];
        assert_eq!(
            string_member(reported, "QuestionId"),
            question.question_id,
            "position {position}: expected the document's question order, got {reported:?}"
        );
        assert_eq!(
            reported_choices(reported),
            question.choices,
            "case {:?}: expected the document's choice catalog, got {reported:?}",
            question.question_id
        );
    }

    let mut expectations: Vec<(String, Expected)> = Vec::new();
    for (position, question) in defined.iter().enumerate() {
        let offered: Vec<String> = question
            .choices
            .iter()
            .map(|(choice_id, _)| choice_id.clone())
            .collect();
        let state = RECORDED[position % RECORDED.len()];
        let (record, expected) = recorded_answer(state, &offered);
        if let Some(record) = record {
            client.record_template_answer(&template_arn, &lens_arn, &question.question_id, record);
        }
        expectations.push((format!("{}/{state:?}", question.question_id), expected));
    }

    let answered_set = all_summaries(&client, case, &template_arn, &lens_arn).await;
    for (position, (case, expected)) in expectations.iter().enumerate() {
        assert_read(
            case,
            &client,
            &template_arn,
            &lens_arn,
            &answered_set[position],
            expected,
        )
        .await;
    }

    // A question belongs to the LENS under review: one lens review's question is no
    // question of the other review of the same template, so the value is refused as input.
    let crossed = defined[0].question_id.clone();
    assert_field_validation(
        "imported-question-under-the-official-lens",
        &client
            .get_review_template_answer(&template_arn, FRAMEWORK_LENS, &crossed)
            .await,
        "QuestionId",
        &format!(
            "No question with ID {crossed} was found in the lens with alias {FRAMEWORK_LENS}."
        ),
    );
}

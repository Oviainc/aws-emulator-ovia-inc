// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListReviewTemplateAnswers` when the lens the review template
//! applies is a CUSTOM lens the caller imported.
//!
//! The rule these pin, over tables of lens documents and page sizes rather than one
//! recorded request: the answers a review template's lens review lists are one summary per
//! question the LENS's own content defines — for a custom lens, the pillars, questions and
//! choices of the document the applied version was published from, in the document's own
//! order — each carrying the answer the TEMPLATE records for that question and `UNANSWERED`
//! with the question's whole `Choices` catalog where it records none; `PillarId` and
//! `MaxResults`/`NextToken` then FILTER and PAGE that set rather than resolving it.
//!
//! The rows differ along the SHAPE of the document (how many pillars it defines, how many
//! questions each pillar asks, how many choices each question offers, whether the set fits
//! in one page) and along the page size a request asks for, because those are what the
//! listed set, its order and its paging move with. The AWS-official half of the lens
//! catalog is resolved by the same path, which the two-lens test here pins: one template
//! applying both halves lists each lens's own question set.
//!
//! `super::custom_lens_review_answers` pins the same rule across the OTHER answer reads (a
//! workload's `ListAnswers` and `GetAnswer`, plus this list in one shape); this suite is
//! the review-template list on its own, with the filter, the paging and the answered
//! summaries it serves. The document builder, the import/publish wire helpers and the
//! expectation derived from a document's shape are that suite's and
//! `super::custom_lens_import`'s, reused here rather than respelled.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{
    expected_questions, import_shape, publish, question_count, reported_choices, string_member,
    ExpectedQuestion, Shape,
};
use super::workload_harness::{assert_field_validation, member, review_template, Answer, Client};

/// The alias of the AWS-official lens this emulator ships a question set for, and how many
/// questions that lens asks: the published Well-Architected Framework numbering (OPS 1-11,
/// SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6), spelled out here rather than read
/// back out of the code under test.
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The page a request that omits `MaxResults` gets: the modelled maximum for this
/// operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListReviewTemplateAnswers")`).
const PAGE_CEILING: usize = 50;

/// The `ReviewTemplateAnswerStatus` values, from
/// `smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// The `QuestionType` a review-template question carries here, one of the values
/// `smithy_query(query="shape", shape="QuestionType")` publishes (`NON_PRIORITIZED` |
/// `PRIORITIZED`): nothing prioritizes a review-template question.
const QUESTION_TYPE: &str = "NON_PRIORITIZED";

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers — from the smallest
/// document the lens JSON format admits (one pillar, one question, one choice) up to a
/// question set that does not fit in one page.
const SHAPES: &[(&str, Shape)] = &[
    ("smallest-document", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 4, 3)]),
    (
        "four-pillars",
        &[
            ("operations", 2, 1),
            ("security", 1, 5),
            ("cost", 3, 2),
            ("sustainability", 1, 1),
        ],
    ),
    (
        "longer-than-one-page",
        &[("reliability", 28, 2), ("performance", 29, 1)],
    ),
];

/// `ListReviewTemplateAnswers` with the query members a case names, through the one request
/// builder this crate's suites share.
async fn list_answers(
    client: &Client,
    template_arn: &str,
    lens: &str,
    query: &[String],
) -> Answer {
    client
        .list_review_template_answers(template_arn, lens, &query.join("&"))
        .await
}

fn summaries_of(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn question_ids(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| string_member(summary, "QuestionId"))
        .collect()
}

/// Every `ReviewTemplateAnswerSummary` of one template lens review, following `NextToken`
/// to the end: the summaries in the order they were served, and how many pages it took.
async fn collect_answers(
    client: &Client,
    case: &str,
    template_arn: &str,
    lens: &str,
    pillar_id: Option<&str>,
    max_results: Option<usize>,
) -> (Vec<Value>, usize) {
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;

    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(pillar) = pillar_id {
            query.push(format!("PillarId={pillar}"));
        }
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = &next_token {
            query.push(format!("NextToken={token}"));
        }
        let answer = list_answers(client, template_arn, lens, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "TemplateArn"),
            template_arn,
            "case {case:?}: the envelope must name the template as stored, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "LensAlias"),
            lens,
            "case {case:?}: the envelope must name the lens as sent, got {answer:?}"
        );
        let page = summaries_of(&answer);
        let ceiling = max_results.unwrap_or(PAGE_CEILING);
        assert!(
            page.len() <= ceiling,
            "case {case:?}: a page must hold at most {ceiling} summaries, got {}",
            page.len()
        );
        collected.extend(page);
        pages += 1;
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            pages <= collected.len() + 1,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        if next_token.is_none() {
            break;
        }
    }

    (collected, pages)
}

/// How many pages a selection of `selected` questions takes at `page_size`.
fn expected_pages(selected: usize, page_size: usize) -> usize {
    selected.div_ceil(page_size).max(1)
}

/// One summary against the question the document defines at that position, in the state a
/// template that has answered nothing reports it in: the question's identity, title and
/// whole choice catalog, and the members an UNANSWERED question carries.
fn assert_unanswered(case: &str, summary: &Value, expected: &ExpectedQuestion) {
    assert_eq!(
        string_member(summary, "QuestionId"),
        expected.question_id,
        "case {case:?}: unexpected QuestionId, got {summary:?}"
    );
    assert_eq!(
        string_member(summary, "PillarId"),
        expected.pillar_id,
        "case {case:?}: unexpected PillarId, got {summary:?}"
    );
    assert_eq!(
        string_member(summary, "QuestionTitle"),
        expected.title,
        "case {case:?}: unexpected QuestionTitle, got {summary:?}"
    );
    assert_eq!(
        reported_choices(summary),
        expected.choices,
        "case {case:?}: the question must report the choice catalog its lens defines, got \
         {summary:?}"
    );
    assert_eq!(
        summary.get("AnswerStatus").and_then(Value::as_str),
        Some(UNANSWERED),
        "case {case:?}: a question the template has not answered is {UNANSWERED}, got {summary:?}"
    );
    assert_eq!(
        summary.get("IsApplicable"),
        Some(&Value::Bool(true)),
        "case {case:?}: a question applies until an answer says otherwise, got {summary:?}"
    );
    assert_eq!(
        summary.get("SelectedChoices"),
        Some(&json!([])),
        "case {case:?}: an unanswered question selects nothing, got {summary:?}"
    );
    assert_eq!(
        summary.get("ChoiceAnswerSummaries"),
        Some(&json!([])),
        "case {case:?}: an unanswered question answers no choice, got {summary:?}"
    );
    assert_eq!(
        summary.get("Reason"),
        None,
        "case {case:?}: no answer has given a reason, got {summary:?}"
    );
    assert_eq!(
        summary.get("QuestionType").and_then(Value::as_str),
        Some(QUESTION_TYPE),
        "case {case:?}: unexpected QuestionType, got {summary:?}"
    );
}

/// A review template applying a published custom lens lists one `ReviewTemplateAnswerSummary`
/// per question that lens's document defines, in the document's own order, each UNANSWERED
/// and applicable with that question's own choice catalog — whatever shape the document has.
#[tokio::test]
async fn every_question_a_templates_imported_lens_defines_is_listed_unanswered() {
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

        let (summaries, pages) =
            collect_answers(&client, case, &template_arn, lens_arn, None, None).await;
        let expected = expected_questions(shape);

        assert_eq!(
            question_ids(&summaries),
            expected
                .iter()
                .map(|question| question.question_id.clone())
                .collect::<Vec<String>>(),
            "case {case:?}: the template's review must list the document's questions, in the \
             document's own order, got {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            question_count(shape),
            "case {case:?}: every question the document defines must be listed exactly once, got \
             {} summaries",
            summaries.len()
        );
        assert_eq!(
            pages,
            expected_pages(question_count(shape), PAGE_CEILING),
            "case {case:?}: a request that omits MaxResults must serve pages of at most \
             {PAGE_CEILING}, got {pages} pages for {} questions",
            summaries.len()
        );
        for (position, (summary, question)) in summaries.iter().zip(&expected).enumerate() {
            assert_unanswered(&format!("{case}-{position}"), summary, question);
        }
    }
}

/// `PillarId` FILTERS the custom lens's question set: each pillar the DOCUMENT defines lists
/// its own questions in the document's order, and a pillar it does not define — including
/// one the AWS-official lens does — lists nothing.
#[tokio::test]
async fn the_pillar_filter_narrows_a_templates_imported_question_set() {
    const SHAPE: Shape = &[("governance", 2, 2), ("cost", 3, 1), ("delivery", 1, 3)];

    /// Pillars the document under test does not define: one this emulator's AWS-official
    /// lens does define (`super::review_template_answers` pins that catalog) and one no
    /// lens defines.
    const ABSENT_PILLARS: [&str; 2] = ["operationalExcellence", "nosuchpillar"];

    let client = Client::new();
    let lens_arn = import_shape(&client, "filter", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "filter", &lens_arn).await;
    let template_arn = review_template(&client, "tmpl-filter", &[&lens_arn]).await;

    let expected = expected_questions(SHAPE);

    for (pillar_id, questions, _) in SHAPE {
        let case = format!("pillar-{pillar_id}");
        let (summaries, _) =
            collect_answers(&client, &case, &template_arn, &lens_arn, Some(pillar_id), None).await;
        let wanted: Vec<String> = expected
            .iter()
            .filter(|question| question.pillar_id == *pillar_id)
            .map(|question| question.question_id.clone())
            .collect();
        assert_eq!(
            question_ids(&summaries),
            wanted,
            "case {case:?}: the filter must select exactly that pillar's questions, got \
             {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            *questions,
            "case {case:?}: the pillar asks {questions} questions, got {} summaries",
            summaries.len()
        );
    }

    // The filter and the cursor compose: a page of one over one pillar still enumerates
    // exactly that pillar's questions, in order.
    let case = "pillar-paged";
    let (paged, pages) =
        collect_answers(&client, case, &template_arn, &lens_arn, Some("cost"), Some(1)).await;
    assert_eq!(
        question_ids(&paged),
        expected
            .iter()
            .filter(|question| question.pillar_id == "cost")
            .map(|question| question.question_id.clone())
            .collect::<Vec<String>>(),
        "case {case:?}: paging a filtered selection must serve it whole and in order, got \
         {paged:?}"
    );
    assert_eq!(
        pages,
        paged.len(),
        "case {case:?}: MaxResults=1 must take one page per selected question, got {pages} pages \
         for {} questions",
        paged.len()
    );

    for pillar_id in ABSENT_PILLARS {
        let case = format!("absent-pillar-{pillar_id}");
        let answer = list_answers(
            &client,
            &template_arn,
            &lens_arn,
            &[format!("PillarId={pillar_id}")],
        )
        .await;
        // The pillars of an imported lens are the ones its DOCUMENT defines, so a pillar
        // outside them — including one the framework lens defines — is a value this review
        // has no pillar for, and is refused rather than selecting nothing.
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a pillar the document does not define is refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .and_then(|fields| fields.first())
                .and_then(|field| field.get("Name"))
                .and_then(Value::as_str),
            Some("PillarId"),
            "case {case:?}: the finding must name PillarId, got {answer:?}"
        );
    }
}

/// Paging over a template's imported question set enumerates every question exactly once,
/// in the document's order, whatever page size the request asks for — including the modelled
/// bounds of `MaxResults` and a size larger than the whole selection.
#[tokio::test]
async fn paging_enumerates_every_question_of_a_templates_imported_lens_exactly_once() {
    const SHAPE: Shape = &[("reliability", 20, 2), ("sustainability", 13, 1)];
    /// The page sizes under test: the modelled minimum, two sizes that divide the selection
    /// unevenly, and the modelled maximum, which is larger than the selection.
    const PAGE_SIZES: [usize; 4] = [1, 7, 16, PAGE_CEILING];

    let client = Client::new();
    let lens_arn = import_shape(&client, "paging", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "paging", &lens_arn).await;
    let template_arn = review_template(&client, "tmpl-paging", &[&lens_arn]).await;

    let wanted: Vec<String> = expected_questions(SHAPE)
        .iter()
        .map(|question| question.question_id.clone())
        .collect();

    for page_size in PAGE_SIZES {
        let case = format!("max-results-{page_size}");
        let (summaries, pages) = collect_answers(
            &client,
            &case,
            &template_arn,
            &lens_arn,
            None,
            Some(page_size),
        )
        .await;
        let served = question_ids(&summaries);
        assert_eq!(
            served, wanted,
            "case {case:?}: paging must serve the document's questions whole and in order, got \
             {served:?}"
        );
        assert_eq!(
            served.iter().collect::<BTreeSet<&String>>().len(),
            wanted.len(),
            "case {case:?}: no question may be served twice, got {served:?}"
        );
        assert_eq!(
            pages,
            expected_pages(wanted.len(), page_size),
            "case {case:?}: {} questions at {page_size} per page must take \
             {} pages, got {pages}",
            wanted.len(),
            expected_pages(wanted.len(), page_size)
        );
    }
}

/// A question the TEMPLATE has answered is listed with that answer: the choices in place,
/// a status per choice the answer speaks about, `ANSWERED`, and the applicability and reason
/// the answer recorded — while every other question of the same document stays UNANSWERED.
#[tokio::test]
async fn an_answer_recorded_on_a_templates_imported_question_is_listed_with_it() {
    const SHAPE: Shape = &[("governance", 2, 3), ("cost", 2, 1)];
    /// `AnswerReason`, from `smithy_query(query="shape", shape="AnswerReason")`.
    const OUT_OF_SCOPE: &str = "OUT_OF_SCOPE";

    let client = Client::new();
    let lens_arn = import_shape(&client, "answered", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "answered", &lens_arn).await;
    let template_arn = review_template(&client, "tmpl-answered", &[&lens_arn]).await;

    let expected = expected_questions(SHAPE);
    // The questions answered, and how: the first selects its whole choice catalog, the
    // second is taken out of scope with a reason, and the rest are left unanswered.
    let selected: Vec<String> = expected[0]
        .choices
        .iter()
        .map(|(choice_id, _)| choice_id.clone())
        .collect();

    let written = client
        .update_review_template_answer(
            &template_arn,
            &lens_arn,
            &expected[0].question_id,
            json!({ "SelectedChoices": selected }),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "case \"select-every-choice\": a question the document defines must be answerable, got \
         {written:?}"
    );
    let excluded = client
        .update_review_template_answer(
            &template_arn,
            &lens_arn,
            &expected[1].question_id,
            json!({ "IsApplicable": false, "Reason": OUT_OF_SCOPE }),
        )
        .await;
    assert_eq!(
        excluded.status, 200,
        "case \"out-of-scope\": a question may be taken out of scope, got {excluded:?}"
    );

    let (summaries, _) =
        collect_answers(&client, "answered", &template_arn, &lens_arn, None, None).await;
    assert_eq!(
        question_ids(&summaries),
        expected
            .iter()
            .map(|question| question.question_id.clone())
            .collect::<Vec<String>>(),
        "case \"answered\": answering a question must not move the question set, got {summaries:?}"
    );

    let answered = &summaries[0];
    assert_eq!(
        answered.get("AnswerStatus").and_then(Value::as_str),
        Some(ANSWERED),
        "case \"select-every-choice\": a question the template has answered is {ANSWERED}, got \
         {answered:?}"
    );
    assert_eq!(
        answered
            .get("SelectedChoices")
            .and_then(Value::as_array)
            .map(|choices| choices
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<String>>()),
        Some(selected.clone()),
        "case \"select-every-choice\": the summary must report the choices in place, got \
         {answered:?}"
    );
    assert_eq!(
        reported_choices(answered),
        expected[0].choices,
        "case \"select-every-choice\": an answered question still reports its lens's whole choice \
         catalog, got {answered:?}"
    );
    assert_eq!(
        answered
            .get("ChoiceAnswerSummaries")
            .and_then(Value::as_array)
            .map(|answers| answers
                .iter()
                .map(|choice| string_member(choice, "ChoiceId"))
                .collect::<Vec<String>>()),
        Some(selected),
        "case \"select-every-choice\": the summary must report a status per choice the answer \
         speaks about, got {answered:?}"
    );

    let out_of_scope = &summaries[1];
    assert_eq!(
        out_of_scope.get("AnswerStatus").and_then(Value::as_str),
        Some(ANSWERED),
        "case \"out-of-scope\": a question the template has answered is {ANSWERED}, got \
         {out_of_scope:?}"
    );
    assert_eq!(
        out_of_scope.get("IsApplicable"),
        Some(&Value::Bool(false)),
        "case \"out-of-scope\": the summary must report the applicability the answer recorded, \
         got {out_of_scope:?}"
    );
    assert_eq!(
        out_of_scope.get("Reason").and_then(Value::as_str),
        Some(OUT_OF_SCOPE),
        "case \"out-of-scope\": the summary must report the reason the answer recorded, got \
         {out_of_scope:?}"
    );

    for (position, question) in expected.iter().enumerate().skip(2) {
        assert_unanswered(
            &format!("untouched-{position}"),
            &summaries[position],
            question,
        );
    }
}

/// Each lens review of a template resolves ITS OWN lens: a template applying both a custom
/// lens and the AWS-official framework lens lists the imported document's questions for the
/// one and the framework's catalog for the other, and a published lens the template does not
/// apply has no review to list at all.
#[tokio::test]
async fn each_lens_review_of_a_template_lists_its_own_lens() {
    const APPLIED: Shape = &[("governance", 2, 2)];
    const UNAPPLIED: Shape = &[("security", 1, 1)];

    let client = Client::new();
    let applied_arn = import_shape(&client, "applied", APPLIED).await;
    let unapplied_arn = import_shape(&client, "unapplied", UNAPPLIED).await;
    wait_for_the_import_to_land();
    publish(&client, "applied", &applied_arn).await;
    publish(&client, "unapplied", &unapplied_arn).await;

    let template_arn =
        review_template(&client, "tmpl-two-lenses", &[&applied_arn, FRAMEWORK_LENS]).await;

    let (custom, _) =
        collect_answers(&client, "custom-half", &template_arn, &applied_arn, None, None).await;
    assert_eq!(
        question_ids(&custom),
        expected_questions(APPLIED)
            .iter()
            .map(|question| question.question_id.clone())
            .collect::<Vec<String>>(),
        "case \"custom-half\": the custom lens's review must list its document's questions, got \
         {custom:?}"
    );

    let (official, _) = collect_answers(
        &client,
        "official-half",
        &template_arn,
        FRAMEWORK_LENS,
        None,
        None,
    )
    .await;
    assert_eq!(
        official.len(),
        FRAMEWORK_QUESTIONS,
        "case \"official-half\": the framework lens's review must list its catalog's \
         {FRAMEWORK_QUESTIONS} questions, got {} summaries",
        official.len()
    );
    let shared: BTreeSet<String> = question_ids(&custom)
        .into_iter()
        .collect::<BTreeSet<String>>()
        .intersection(&question_ids(&official).into_iter().collect())
        .cloned()
        .collect();
    assert!(
        shared.is_empty(),
        "case \"two-lenses\": each review lists only its own lens's questions, got {shared:?} in \
         both"
    );

    let refused = client
        .list_review_template_answers(&template_arn, &unapplied_arn, "")
        .await;
    assert_field_validation(
        "unapplied-lens",
        &refused,
        "LensAlias",
        &format!(
            "No lens review for lens alias {unapplied_arn} was found in review template \
             {template_arn}."
        ),
    );
}

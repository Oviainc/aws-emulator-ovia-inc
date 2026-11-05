// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the QUESTION SET a lens review asks when the lens under
//! review is a custom lens the caller imported.
//!
//! The rule these pin, over a table of lens documents rather than one recorded request: a
//! lens review's answers are the questions the LENS's own content defines — for a custom
//! lens, the pillars, questions and choices of the document a version was published from,
//! in the document's own order — each reported `UNANSWERED` and applicable while the
//! reviewed resource records no answer for it, with that question's own `Choices`
//! catalog. The AWS-official half of the lens catalog keeps reporting its catalog entry's
//! questions, which the last test here pins, so the one resolution serves both halves.
//!
//! The rows differ along the SHAPE of the document — how many pillars it defines, how many
//! questions each pillar asks and how many choices each question offers — because the
//! document is the lens's content, so every shape of it must be reported. The reads
//! covered are the answer list of a workload's lens review (`ListAnswers`), the single
//! answer read (`GetAnswer`) and the answer list of a review template's lens review
//! (`ListReviewTemplateAnswers`).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import and
//! publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    created_id, encode_path_segment, member, minimal_create, review_template, Answer, Client,
};

/// The shape of one lens document: `(pillar id, questions in it, choices per question)`,
/// exactly what `super::custom_lens_import::lens_document` builds a document from.
pub(crate) type Shape = &'static [(&'static str, usize, usize)];

/// The alias of the AWS-official lens this emulator ships a question set for, and how many
/// questions that lens asks: the published Well-Architected Framework numbering (OPS 1-11,
/// SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6), spelled out here rather than read
/// back out of the code under test.
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The page a request that omits `MaxResults` gets: the modelled maximum for this
/// operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListAnswers")`).
const PAGE_CEILING: usize = 50;

/// One question a document of a given [`Shape`] defines, as the lens JSON that shape is
/// built from spells it: the pillar it belongs to, its id, its title and its choices as
/// `(ChoiceId, Choice$Title)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpectedQuestion {
    pub(crate) pillar_id: String,
    pub(crate) question_id: String,
    pub(crate) title: String,
    pub(crate) choices: Vec<(String, String)>,
}

/// The questions a document of `shape` defines, in the document's own order: pillar by
/// pillar, each pillar's questions in the order it lists them.
///
/// Derived from the SHAPE, mirroring the ids and titles
/// `super::custom_lens_import::lens_document` writes, so the expectation comes from the
/// document the test sent rather than from the service that answered.
pub(crate) fn expected_questions(shape: Shape) -> Vec<ExpectedQuestion> {
    shape
        .iter()
        .flat_map(|(pillar_id, questions, choices)| {
            (0..*questions).map(move |question| ExpectedQuestion {
                pillar_id: (*pillar_id).to_string(),
                question_id: format!("{pillar_id}_q{question}"),
                title: format!("Question {question} of {pillar_id}?"),
                choices: (0..*choices)
                    .map(|choice| {
                        let choice_id = format!("{pillar_id}_q{question}_c{choice}");
                        let title = format!("Choice {choice_id}");
                        (choice_id, title)
                    })
                    .collect(),
            })
        })
        .collect()
}

/// How many questions a document of `shape` defines in all.
pub(crate) fn question_count(shape: Shape) -> usize {
    shape.iter().map(|(_, questions, _)| questions).sum()
}

/// Import a document of `shape` as a fresh custom lens and answer its ARN. The caller
/// waits for the import to land and publishes the version itself, so a table can import
/// every row's lens before waiting once.
pub(crate) async fn import_shape(client: &Client, case: &str, shape: Shape) -> String {
    let document = lens_document(&format!("Lens {case}"), None, shape).to_string();
    imported_arn(
        &import(client, &document, None, &format!("tok-import-{case}"), None).await,
        case,
    )
}

/// Publish version `1.0` of an imported lens, so it can be applied to a workload or a
/// review template: "The lens cannot be applied to workloads … until it's published with
/// CreateLensVersion" (`ImportLens` doc).
pub(crate) async fn publish(client: &Client, case: &str, lens_arn: &str) {
    let published = create_version(
        client,
        lens_arn,
        "1.0",
        Some(true),
        &format!("tok-publish-{case}"),
    )
    .await;
    assert_eq!(
        published.status, 200,
        "case {case:?}: CreateLensVersion must publish the imported lens, got {published:?}"
    );
}

/// A workload reviewed against `lens`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lens: &str) -> String {
    let mut request = minimal_create(&format!("wl-{case}"), &format!("tok-wl-{case}"));
    request["Lenses"] = json!([lens]);
    created_id(&client.create(request).await, case)
}

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers`. The lens is
/// percent-encoded because a custom lens is named by its ARN, which carries `:` and `/`
/// inside ONE path segment.
async fn list_answers(client: &Client, workload_id: &str, lens: &str, query: &[String]) -> Answer {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers{suffix}",
                encode_path_segment(lens)
            ),
            None,
        )
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

/// Every `AnswerSummary` of one lens review, following `NextToken` to the end: the
/// summaries in the order they were served, and how many pages it took.
async fn collect_answers(
    client: &Client,
    case: &str,
    workload_id: &str,
    lens: &str,
    filters: &[String],
    max_results: Option<usize>,
) -> (Vec<Value>, usize) {
    let mut collected: Vec<Value> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;

    loop {
        let mut query = filters.to_vec();
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = &next_token {
            query.push(format!("NextToken={token}"));
        }
        let answer = list_answers(client, workload_id, lens, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        let page = summaries_of(&answer);
        if let Some(max) = max_results {
            assert!(
                page.len() <= max,
                "case {case:?}: a page must hold at most MaxResults={max} summaries, got {}",
                page.len()
            );
        }
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

pub(crate) fn string_member(summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `Choices` a summary reports, as `(ChoiceId, Title)` in the order they were served.
pub(crate) fn reported_choices(summary: &Value) -> Vec<(String, String)> {
    summary
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| (string_member(choice, "ChoiceId"), string_member(choice, "Title")))
                .collect()
        })
        .unwrap_or_default()
}

/// One summary against the question the document defines at that position: the question's
/// identity and title, its whole choice catalog, and the members an UNANSWERED question
/// carries.
///
/// `choice_answers_member` is the member the shape under test reports the per-choice
/// answers in — `AnswerSummary` carries `ChoiceAnswerSummaries` and `Answer` carries
/// `ChoiceAnswers` — so both reads are checked by the one assertion.
fn assert_unanswered(
    case: &str,
    summary: &Value,
    expected: &ExpectedQuestion,
    choice_answers_member: &str,
) {
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
        summary.get("Risk").and_then(Value::as_str),
        Some("UNANSWERED"),
        "case {case:?}: a question with no stored answer is UNANSWERED, got {summary:?}"
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
        summary.get(choice_answers_member),
        Some(&json!([])),
        "case {case:?}: an unanswered question answers no choice, got {summary:?}"
    );
    assert_eq!(
        summary.get("Reason"),
        None,
        "case {case:?}: no answer has given a reason, got {summary:?}"
    );
}

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers — including a
/// question set longer than one page.
const SHAPES: &[(&str, Shape)] = &[
    ("one-question-one-choice", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 3, 2)]),
    (
        "several-pillars",
        &[("operations", 1, 4), ("security", 2, 1), ("cost", 3, 2)],
    ),
    (
        "longer-than-one-page",
        &[("reliability", 30, 2), ("sustainability", 25, 1)],
    ),
];

/// A workload lens review of a published custom lens reports one `AnswerSummary` per
/// question that lens's document defines, in the document's own order, each UNANSWERED and
/// applicable with that question's own choice catalog.
#[tokio::test]
async fn every_question_an_imported_lens_defines_is_reported_unanswered() {
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
        let workload_id = workload_reviewing(&client, case, lens_arn).await;

        let expected = expected_questions(shape);
        let (summaries, pages) =
            collect_answers(&client, case, &workload_id, lens_arn, &[], None).await;
        assert_eq!(
            summaries.len(),
            expected.len(),
            "case {case:?}: the review must report the {} questions the lens document defines, got \
             {}",
            expected.len(),
            summaries.len()
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(PAGE_CEILING).max(1),
            "case {case:?}: a request that omits MaxResults gets the modelled maximum page, got \
             {pages} pages for {} questions",
            expected.len()
        );

        let reported: BTreeSet<String> = summaries
            .iter()
            .map(|summary| string_member(summary, "QuestionId"))
            .collect();
        assert_eq!(
            reported.len(),
            summaries.len(),
            "case {case:?}: every question must be reported once, got {reported:?}"
        );

        for (position, (summary, question)) in summaries.iter().zip(&expected).enumerate() {
            assert_unanswered(
                &format!("{case}-question-{position}"),
                summary,
                question,
                "ChoiceAnswerSummaries",
            );
        }

        // The envelope names the review that was read: the workload, the lens as the
        // request named it and the ARN it resolves to — for a custom lens, "this is the
        // lens ARN" (`LensAlias` doc), so the two agree.
        let answer = list_answers(&client, &workload_id, lens_arn, &[]).await;
        assert_eq!(
            member(&answer, "WorkloadId"),
            workload_id,
            "case {case:?}: the envelope must name the workload, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "LensAlias"),
            lens_arn.as_str(),
            "case {case:?}: the envelope must name the lens as sent, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "LensArn"),
            lens_arn.as_str(),
            "case {case:?}: the envelope must carry the lens ARN, got {answer:?}"
        );
        assert_eq!(
            question_count(shape),
            expected.len(),
            "case {case:?}: the shape must define the questions the expectation lists"
        );
    }
}

/// `PillarId` SELECTS one pillar of the custom lens's question set and `QuestionPriority`
/// filters by the `QuestionType` the questions carry: each pillar the DOCUMENT defines
/// reports its own questions, and a `PillarId` the document does not define is refused
/// rather than selecting nothing — the pillars are that lens's own content.
#[tokio::test]
async fn the_filters_narrow_an_imported_lenss_question_set() {
    const SHAPE: Shape = &[("operations", 1, 2), ("security", 4, 1), ("cost", 2, 3)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "filters", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "filters", &lens_arn).await;
    let workload_id = workload_reviewing(&client, "filters", &lens_arn).await;

    let mut cases: Vec<(String, Vec<String>, usize)> = Vec::new();
    for (pillar_id, questions, _) in SHAPE {
        cases.push((
            format!("pillar-{pillar_id}"),
            vec![format!("PillarId={pillar_id}")],
            *questions,
        ));
    }
    // `QuestionPriority` publishes PRIORITIZED | NONE
    // (`smithy_query(query="shape", shape="QuestionPriority")`). Every question of a lens
    // review here is NON_PRIORITIZED, so NONE selects them all and PRIORITIZED none.
    cases.push((
        "priority-none".to_string(),
        vec!["QuestionPriority=NONE".to_string()],
        question_count(SHAPE),
    ));
    cases.push((
        "priority-prioritized".to_string(),
        vec!["QuestionPriority=PRIORITIZED".to_string()],
        0,
    ));
    cases.push((
        "pillar-and-priority".to_string(),
        vec![
            "PillarId=security".to_string(),
            "QuestionPriority=NONE".to_string(),
        ],
        4,
    ));

    for (case, filters, expected) in cases {
        let (summaries, _) =
            collect_answers(&client, &case, &workload_id, &lens_arn, &filters, None).await;
        assert_eq!(
            summaries.len(),
            expected,
            "case {case:?}: expected {expected} summaries, got {}",
            summaries.len()
        );
        if let Some(pillar) = filters
            .iter()
            .find_map(|filter| filter.strip_prefix("PillarId="))
        {
            for summary in &summaries {
                assert_eq!(
                    string_member(summary, "PillarId"),
                    pillar,
                    "case {case:?}: only {pillar} questions may be reported, got {summary:?}"
                );
            }
        }
    }

    // A pillar the document does not define is refused rather than selecting nothing —
    // including one of its own pillars in another case, because a `PillarId` is matched
    // exactly.
    for (case, pillar) in [
        ("pillar-absent", "notapillar"),
        ("pillar-wrong-case", "SECURITY"),
    ] {
        let answer = list_answers(
            &client,
            &workload_id,
            &lens_arn,
            &[format!("PillarId={pillar}")],
        )
        .await;
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

/// Paging over a custom lens's questions enumerates every one of them exactly once, in the
/// document's order, whatever page size the request asks for.
#[tokio::test]
async fn paging_enumerates_every_question_of_an_imported_lens_exactly_once() {
    const SHAPE: Shape = &[("operations", 20, 2), ("security", 35, 1)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "paging", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "paging", &lens_arn).await;
    let workload_id = workload_reviewing(&client, "paging", &lens_arn).await;

    let expected: Vec<String> = expected_questions(SHAPE)
        .into_iter()
        .map(|question| question.question_id)
        .collect();

    // The page sizes: the modelled minimum, sizes that do and do not divide the question
    // set, the modelled maximum, and a request that names none.
    for max_results in [Some(1), Some(2), Some(7), Some(PAGE_CEILING), None] {
        let case = format!("max-results-{max_results:?}");
        let (summaries, pages) = collect_answers(
            &client,
            &case,
            &workload_id,
            &lens_arn,
            &[],
            max_results,
        )
        .await;
        let ids: Vec<String> = summaries
            .iter()
            .map(|summary| string_member(summary, "QuestionId"))
            .collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: paging must serve the document's questions once, in order"
        );
        let page_size = max_results.unwrap_or(PAGE_CEILING);
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {case:?}: {} questions take {} pages of {page_size}, got {pages}",
            expected.len(),
            expected.len().div_ceil(page_size)
        );
    }
}

/// The questions a review asks are the lens's PUBLISHED content: importing a new document
/// over the lens leaves its review asking what the published version defines, because a
/// published version is immutable and an unpublished draft is not applied to anything.
#[tokio::test]
async fn an_import_over_a_published_lens_does_not_move_the_questions_its_review_asks() {
    const PUBLISHED: Shape = &[("governance", 2, 2)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "republished", PUBLISHED).await;
    wait_for_the_import_to_land();
    publish(&client, "republished", &lens_arn).await;
    let workload_id = workload_reviewing(&client, "republished", &lens_arn).await;

    let expected: Vec<String> = expected_questions(PUBLISHED)
        .into_iter()
        .map(|question| question.question_id)
        .collect();
    let (summaries, _) =
        collect_answers(&client, "before-the-reimport", &workload_id, &lens_arn, &[], None).await;
    assert_eq!(
        summaries
            .iter()
            .map(|summary| string_member(summary, "QuestionId"))
            .collect::<Vec<String>>(),
        expected,
        "case \"before-the-reimport\": the review must ask the published document's questions"
    );

    // A second import of the SAME lens, carrying a different question set. It lands as a
    // DRAFT, so no review asks it.
    let redraft = lens_document("Redrafted lens", None, &[("operations", 3, 1)]).to_string();
    let reimported = import(
        &client,
        &redraft,
        Some(&lens_arn),
        "tok-import-republished-2",
        None,
    )
    .await;
    assert_eq!(
        reimported.status, 200,
        "the second import of the lens must be accepted, got {reimported:?}"
    );
    wait_for_the_import_to_land();

    let (summaries, _) = collect_answers(
        &client,
        "after-the-reimport",
        &workload_id,
        &lens_arn,
        &[],
        None,
    )
    .await;
    assert_eq!(
        summaries
            .iter()
            .map(|summary| string_member(summary, "QuestionId"))
            .collect::<Vec<String>>(),
        expected,
        "case \"after-the-reimport\": an unpublished draft must not move the questions the \
         review asks"
    );
}

/// The single-answer read and a review template's answer list resolve the same question
/// set: `GetAnswer` reports any question the imported document defines, and
/// `ListReviewTemplateAnswers` reports one summary per question of a template's review of
/// that lens.
#[tokio::test]
async fn the_other_answer_reads_resolve_the_same_imported_question_set() {
    const SHAPE: Shape = &[("governance", 2, 3), ("cost", 1, 1)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "other-reads", SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "other-reads", &lens_arn).await;
    let workload_id = workload_reviewing(&client, "other-reads", &lens_arn).await;

    let expected = expected_questions(SHAPE);

    // Every question of the document reads back through the single-answer read, with the
    // same identity and choice catalog the list reports.
    for question in &expected {
        let answer = client
            .send(
                "GET",
                &format!(
                    "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                    encode_path_segment(&lens_arn),
                    encode_path_segment(&question.question_id)
                ),
                None,
            )
            .await;
        let case = format!("get-answer-{}", question.question_id);
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a question the lens defines must be readable, got {answer:?}"
        );
        let reported = answer.body.get("Answer").cloned().unwrap_or(Value::Null);
        assert_unanswered(&case, &reported, question, "ChoiceAnswers");
    }

    // A review template applying the same lens reports the same question set.
    let template_arn = review_template(&client, "tmpl-other-reads", &[&lens_arn]).await;
    let answer = client
        .list_review_template_answers(&template_arn, &lens_arn, "")
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"template-answers\": the template's lens review must be readable, got {answer:?}"
    );
    let summaries = summaries_of(&answer);
    assert_eq!(
        summaries
            .iter()
            .map(|summary| string_member(summary, "QuestionId"))
            .collect::<Vec<String>>(),
        expected
            .iter()
            .map(|question| question.question_id.clone())
            .collect::<Vec<String>>(),
        "case \"template-answers\": the template's review must ask the document's questions, got \
         {answer:?}"
    );
    for (position, (summary, question)) in summaries.iter().zip(&expected).enumerate() {
        let case = format!("template-answer-{position}");
        assert_eq!(
            reported_choices(summary),
            question.choices,
            "case {case:?}: the question must report the choice catalog its lens defines, got \
             {summary:?}"
        );
        assert_eq!(
            summary.get("AnswerStatus").and_then(Value::as_str),
            Some("UNANSWERED"),
            "case {case:?}: a question with no stored answer is UNANSWERED, got {summary:?}"
        );
    }
}

/// The AWS-official half of the lens catalog is reported by the same resolution: a review
/// of the framework lens still asks its catalog entry's questions, each with the choice ids
/// derived from the question and the best practice's position.
#[tokio::test]
async fn an_official_lenss_review_still_asks_its_catalog_questions() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("wl-official-half", "tok-wl-official-half"))
            .await,
        "official-half",
    );

    let (summaries, _) = collect_answers(
        &client,
        "official-half",
        &workload_id,
        FRAMEWORK_LENS,
        &[],
        None,
    )
    .await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "the framework lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );
    for summary in &summaries {
        let question_id = string_member(summary, "QuestionId");
        let choices = reported_choices(summary);
        assert!(
            !choices.is_empty(),
            "case {question_id:?}: every question of the catalog asks about best practices, got \
             {summary:?}"
        );
        // A choice of the framework lens is named by the `ChoiceId` the lens publishes for
        // that best practice (pinned against the lens's content by
        // `super::lens_catalog_identity`), so what this read has to show is one id per best
        // practice, each naming exactly one choice of the question.
        let mut named: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (choice_id, title) in &choices {
            assert!(
                !choice_id.is_empty() && named.insert(choice_id.as_str()),
                "case {question_id:?}: every choice carries a ChoiceId naming one best \
                 practice of the question, got {summary:?}"
            );
            assert!(
                !title.is_empty(),
                "case {question_id:?}: every choice carries its best practice's title, got \
                 {summary:?}"
            );
        }
        assert_eq!(
            summary.get("Risk").and_then(Value::as_str),
            Some("UNANSWERED"),
            "case {question_id:?}: a question with no stored answer is UNANSWERED, got {summary:?}"
        );
    }
}

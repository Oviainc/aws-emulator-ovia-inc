// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListReviewTemplateAnswers`: the answers of ONE lens review
//! of ONE review template.
//!
//! The rule these pin, over TABLES of inputs rather than one recorded request: a review
//! template's lens review reports one `ReviewTemplateAnswerSummary` per question the
//! applied lens asks, in the lens's own order, narrowed by the request's `PillarId`
//! filter and paged by `MaxResults`/`NextToken`; each summary carries the answer the
//! TEMPLATE stores for that question and `UNANSWERED` where it stores none; a
//! `TemplateArn` nothing is stored under is a modelled 404, while a lens the template does
//! not apply and a `PillarId` the lens does not define are each the modelled 400 naming
//! the member that carried the value.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the state seeding live in the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::workload_harness::{
    answered, answered_over, assert_field_validation, framework_choice_ids,
    review_template as template, Answer, Client,
};

/// The published question numbering of the AWS Well-Architected Framework lens, pillar
/// by pillar in the order the framework presents them: OPS 1-11, SEC 1-11, REL 1-13,
/// PERF 1-5, COST 1-11, SUS 1-6. Spelled out here so the tests pin the catalog rather
/// than reading it back out of the code under test.
const FRAMEWORK_PILLAR_QUESTIONS: [(&str, usize); 6] = [
    ("operationalExcellence", 11),
    ("security", 11),
    ("reliability", 13),
    ("performance", 5),
    ("costOptimization", 11),
    ("sustainability", 6),
];

/// How many questions that lens asks in all.
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The same lens named by its AWS-official ARN: "this is either the lens alias … or the
/// lens ARN, such as `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias`
/// doc).
const FRAMEWORK_LENS_ARN: &str = "arn:aws:wellarchitected:us-east-1::lens/wellarchitected";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so it asks no questions here (the catalog itself is pinned by
/// `super::official_lens_catalog`).
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The page a request that omits `MaxResults` gets: the modelled maximum, so the lens's
/// 57 questions do not fit in one page.
const PAGE_CEILING: usize = 50;

/// The modelled `MaxResults` range for this operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListReviewTemplateAnswers")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

/// The `QuestionType` every question of this emulator's framework lens carries, from
/// `smithy_query(query="shape", shape="QuestionType")` (`NON_PRIORITIZED` |
/// `PRIORITIZED`): nothing prioritizes a review-template question here.
const DEFAULT_QUESTION_TYPE: &str = "NON_PRIORITIZED";

/// The `ReviewTemplateAnswerStatus` values, from
/// `smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) that no
/// stored template carries.
fn absent_template_arn(template_id: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/{template_id}",
        engine::emulator_account_id()
    )
}

/// `ListReviewTemplateAnswers` with the query members this suite's cases name, through
/// the one request builder the crate's suites share.
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
        .map(|summary| {
            summary
                .get("QuestionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every `ReviewTemplateAnswerSummary` of one template lens review, following `NextToken`
/// to the end. Returns the summaries in the order they were served and how many pages it
/// took.
async fn collect_answers(
    client: &Client,
    case: &str,
    template_arn: &str,
    lens: &str,
    filters: &[String],
    max_results: Option<i64>,
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
        let answer = list_answers(client, template_arn, lens, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        let page = summaries_of(&answer);
        if let Some(max) = max_results {
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
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
            pages <= FRAMEWORK_QUESTIONS + 1,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        if next_token.is_none() {
            break;
        }
    }

    (collected, pages)
}

/// The `QuestionCounts` `GetReviewTemplate` reports for a template, as a sorted map: the
/// roll-up the answer list must agree with.
async fn question_counts(client: &Client, template_arn: &str) -> BTreeMap<String, i64> {
    let fetched = client.get_review_template(template_arn).await;
    fetched
        .body
        .get("ReviewTemplate")
        .and_then(|template| template.get("QuestionCounts"))
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(bucket, count)| (bucket.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every question of the applied lens is reported, once, in the lens's own order, with
/// the members the summary derives from the lens and from the (absent) answer. The rows
/// are the pillars of the catalog — each with its published question count — so the whole
/// question set is checked rather than one cited question.
#[tokio::test]
async fn every_question_of_the_applied_lens_is_reported_unanswered() {
    let client = Client::new();
    let template_arn = template(&client, "answers-template", &[FRAMEWORK_LENS]).await;

    let (summaries, _) =
        collect_answers(&client, "all", &template_arn, FRAMEWORK_LENS, &[], None).await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "the framework lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );

    let ids = question_ids(&summaries);
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "every question must be reported once, got {ids:?}"
    );

    // The summaries arrive grouped by pillar, in the framework's pillar order, each
    // group as long as that pillar's published question count.
    let mut offset = 0usize;
    for (pillar, count) in FRAMEWORK_PILLAR_QUESTIONS {
        let group = &summaries[offset..offset + count];
        for summary in group {
            assert_eq!(
                summary.get("PillarId").and_then(Value::as_str),
                Some(pillar),
                "case {pillar:?}: questions must arrive grouped in pillar order, got {summary:?}"
            );
            assert_eq!(
                summary.get("AnswerStatus").and_then(Value::as_str),
                Some(UNANSWERED),
                "case {pillar:?}: a question with no stored answer is {UNANSWERED}, got {summary:?}"
            );
            assert_eq!(
                summary.get("IsApplicable").and_then(Value::as_bool),
                Some(true),
                "case {pillar:?}: a question applies unless an answer says otherwise, got {summary:?}"
            );
            assert_eq!(
                summary.get("SelectedChoices").and_then(Value::as_array),
                Some(&vec![]),
                "case {pillar:?}: an unanswered question selects no choice, got {summary:?}"
            );
            assert_eq!(
                summary.get("ChoiceAnswerSummaries").and_then(Value::as_array),
                Some(&vec![]),
                "case {pillar:?}: an unanswered question answers no choice, got {summary:?}"
            );
            assert_eq!(
                summary.get("QuestionType").and_then(Value::as_str),
                Some(DEFAULT_QUESTION_TYPE),
                "case {pillar:?}: expected QuestionType {DEFAULT_QUESTION_TYPE}, got {summary:?}"
            );
            assert!(
                summary.get("Reason").is_none(),
                "case {pillar:?}: an unanswered question gives no Reason, got {summary:?}"
            );
            let title = summary
                .get("QuestionTitle")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                (1..=512).contains(&title.len()),
                "case {pillar:?}: QuestionTitle must satisfy @length(1, 512), got {title:?}"
            );
        }
        offset += count;
    }

    // The echoed identifiers name the resource the answers belong to.
    let answer = list_answers(&client, &template_arn, FRAMEWORK_LENS, &[]).await;
    assert_eq!(
        member(&answer, "TemplateArn"),
        template_arn,
        "the listing must echo the template it read, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "LensAlias"),
        FRAMEWORK_LENS,
        "the listing must echo the lens as the request named it, got {answer:?}"
    );
}

/// One row of the answer table: the question answered, and the status its answer records
/// for each choice the question asks about — its WHOLE choice set, in the lens's own
/// order, which is the key space those statuses live in. What the summary must report
/// follows from them.
struct AnswerCase {
    question_id: &'static str,
    statuses: Vec<(String, &'static str)>,
    is_applicable: bool,
    reason: Option<&'static str>,
}

/// One case's per-choice statuses over a framework question's OWN choice set: the choices
/// at the positions `named` carry the status named with them, and every other choice of
/// the question carries `rest`.
fn statuses_of(
    question_id: &str,
    named: &[(usize, &'static str)],
    rest: &'static str,
) -> Vec<(String, &'static str)> {
    framework_choice_ids(question_id)
        .into_iter()
        .enumerate()
        .map(|(position, choice_id)| {
            let status = named
                .iter()
                .find(|(named_position, _)| *named_position == position)
                .map_or(rest, |(_, status)| *status);
            (choice_id, status)
        })
        .collect()
}

/// A stored answer is reported on ITS question, and only there; the template's own
/// `QuestionCounts` roll-up counts exactly the questions that carry one. The rows differ
/// along what an answer records — how many choices are selected, which statuses the
/// choices carry, and whether the question applies at all — and span four pillars, so the
/// projection is checked over the class rather than on one question.
#[tokio::test]
async fn a_stored_answer_is_reported_on_its_question() {
    let cases: Vec<AnswerCase> = vec![
        AnswerCase {
            question_id: "organization-priorities",
            statuses: statuses_of("organization-priorities", &[(0, "SELECTED")], "UNSELECTED"),
            is_applicable: true,
            reason: None,
        },
        AnswerCase {
            question_id: "detect-investigate-events",
            statuses: statuses_of(
                "detect-investigate-events",
                &[(0, "SELECTED"), (1, "SELECTED")],
                "UNSELECTED",
            ),
            is_applicable: true,
            reason: None,
        },
        AnswerCase {
            question_id: "plan-for-dr",
            statuses: statuses_of("plan-for-dr", &[], "UNSELECTED"),
            is_applicable: false,
            reason: Some("OUT_OF_SCOPE"),
        },
        AnswerCase {
            question_id: "dev-deploy-patterns",
            statuses: statuses_of("dev-deploy-patterns", &[(0, "NOT_APPLICABLE")], "SELECTED"),
            is_applicable: true,
            reason: None,
        },
    ];

    let client = Client::new();
    let template_arn = template(&client, "answered-template", &[FRAMEWORK_LENS]).await;

    let mut recorded = 0i64;
    for case in cases {
        let question_id = case.question_id;
        // The choices in place are the ones recorded `SELECTED`, and the per-choice
        // answers are one entry per choice of the question, in the lens's own order.
        let selected: Vec<&str> = case
            .statuses
            .iter()
            .filter(|(_, status)| *status == "SELECTED")
            .map(|(choice_id, _)| choice_id.as_str())
            .collect();
        let selected_choices = Value::Array(
            selected
                .iter()
                .map(|choice_id| json!(choice_id))
                .collect::<Vec<Value>>(),
        );
        let choice_answer_summaries = Value::Array(
            case.statuses
                .iter()
                .map(|(choice_id, status)| json!({ "ChoiceId": choice_id, "Status": status }))
                .collect::<Vec<Value>>(),
        );
        let status_refs: Vec<(&str, &str)> = case
            .statuses
            .iter()
            .map(|(choice_id, status)| (choice_id.as_str(), *status))
            .collect();
        let record = answered(&status_refs, case.is_applicable, case.reason);
        client.record_template_answer(&template_arn, FRAMEWORK_LENS, question_id, record);
        recorded += 1;

        let (summaries, _) = collect_answers(
            &client,
            question_id,
            &template_arn,
            FRAMEWORK_LENS,
            &[],
            None,
        )
        .await;
        let summary = summaries
            .iter()
            .find(|summary| summary.get("QuestionId").and_then(Value::as_str) == Some(question_id))
            .cloned()
            .unwrap_or(Value::Null);

        assert_eq!(
            summary.get("AnswerStatus").and_then(Value::as_str),
            Some(ANSWERED),
            "case {question_id:?}: a question with a stored answer is {ANSWERED}, got {summary:?}"
        );
        assert_eq!(
            summary.get("SelectedChoices"),
            Some(&selected_choices),
            "case {question_id:?}: expected SelectedChoices {selected_choices:?}, got {summary:?}"
        );
        assert_eq!(
            summary.get("ChoiceAnswerSummaries"),
            Some(&choice_answer_summaries),
            "case {question_id:?}: expected ChoiceAnswerSummaries \
             {choice_answer_summaries:?}, got {summary:?}"
        );
        assert_eq!(
            summary.get("IsApplicable").and_then(Value::as_bool),
            Some(case.is_applicable),
            "case {question_id:?}: expected IsApplicable {}, got {summary:?}",
            case.is_applicable
        );
        assert_eq!(
            summary.get("Reason").and_then(Value::as_str),
            case.reason,
            "case {question_id:?}: expected Reason {:?}, got {summary:?}",
            case.reason
        );

        // The roll-up the template reports moves with the stored answers, so the reader
        // of the template and the reader of its answers agree.
        let expected: BTreeMap<String, i64> = [
            (ANSWERED.to_string(), recorded),
            (
                UNANSWERED.to_string(),
                i64::try_from(FRAMEWORK_QUESTIONS).unwrap_or(0) - recorded,
            ),
        ]
        .into_iter()
        .collect();
        let actual = question_counts(&client, &template_arn).await;
        assert_eq!(
            actual, expected,
            "case {question_id:?}: expected QuestionCounts {expected:?}, got {actual:?}"
        );

        // Every other question is still unanswered: an answer belongs to its question.
        let answered_elsewhere: Vec<String> = summaries
            .iter()
            .filter(|summary| {
                summary.get("AnswerStatus").and_then(Value::as_str) == Some(ANSWERED)
            })
            .map(|summary| {
                summary
                    .get("QuestionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            i64::try_from(answered_elsewhere.len()).unwrap_or(-1),
            recorded,
            "case {question_id:?}: only the answered questions report {ANSWERED}, got {answered_elsewhere:?}"
        );
    }
}

/// `PillarId` SELECTS the questions of one pillar of the lens: each pillar of the catalog
/// is a row — including the pillars no issue cited — and a value naming no pillar of the
/// lens is refused, because the pillars are the lens's own content.
#[tokio::test]
async fn the_pillar_filter_narrows_the_page() {
    let client = Client::new();
    let template_arn = template(&client, "filtered-template", &[FRAMEWORK_LENS]).await;

    for (pillar, count) in FRAMEWORK_PILLAR_QUESTIONS {
        let filters = vec![format!("PillarId={pillar}")];
        let (summaries, _) = collect_answers(
            &client,
            pillar,
            &template_arn,
            FRAMEWORK_LENS,
            &filters,
            None,
        )
        .await;
        assert_eq!(
            summaries.len(),
            count,
            "case {pillar:?}: expected {count} questions, got {}",
            summaries.len()
        );
        assert!(
            summaries
                .iter()
                .all(|summary| summary.get("PillarId").and_then(Value::as_str) == Some(pillar)),
            "case {pillar:?}: every summary must belong to the filtered pillar, got {summaries:?}"
        );
    }

    // A pillar this lens does not define belongs to no pillar of the review, so the value
    // is refused rather than selecting nothing.
    let answer = list_answers(
        &client,
        &template_arn,
        FRAMEWORK_LENS,
        &["PillarId=noSuchPillar".to_string()],
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case \"unknown-pillar\": a pillar the lens does not define is refused, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case \"unknown-pillar\": expected ValidationException, got {answer:?}"
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
        "case \"unknown-pillar\": the finding must name PillarId, got {answer:?}"
    );
}

/// `MaxResults`/`NextToken` page the SAME question set: however small the page, following
/// the cursor to the end serves every question once, in the lens's order. The rows are
/// the modelled range's published bounds and sizes in between; one past each bound is
/// refused by the boundary validator.
#[tokio::test]
async fn paging_serves_every_question_exactly_once() {
    let client = Client::new();
    let template_arn = template(&client, "paged-template", &[FRAMEWORK_LENS]).await;

    let (unpaged, pages) =
        collect_answers(&client, "unpaged", &template_arn, FRAMEWORK_LENS, &[], None).await;
    assert_eq!(
        pages,
        FRAMEWORK_QUESTIONS.div_ceil(PAGE_CEILING),
        "case \"unpaged\": a request without MaxResults takes pages of {PAGE_CEILING}, got {pages}"
    );
    let expected_ids = question_ids(&unpaged);

    for max_results in [MAX_RESULTS_RANGE.0, 2, 7, 13, MAX_RESULTS_RANGE.1] {
        let case = format!("MaxResults={max_results}");
        let (paged, pages) = collect_answers(
            &client,
            &case,
            &template_arn,
            FRAMEWORK_LENS,
            &[],
            Some(max_results),
        )
        .await;
        assert_eq!(
            question_ids(&paged),
            expected_ids,
            "case {case:?}: paging must serve every question once, in order"
        );
        let expected_pages =
            FRAMEWORK_QUESTIONS.div_ceil(usize::try_from(max_results).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }

    // One past each published bound: `MaxResults` is `@range(min: 1, max: 50)`, so the
    // boundary validator refuses it before the handler runs.
    for max_results in [MAX_RESULTS_RANGE.0 - 1, MAX_RESULTS_RANGE.1 + 1] {
        let case = format!("MaxResults={max_results}");
        let answer = list_answers(
            &client,
            &template_arn,
            FRAMEWORK_LENS,
            &[format!("MaxResults={max_results}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a MaxResults outside @range(1, 50) is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// A `NextToken` this service did not issue is refused as `CANNOT_PARSE` naming the
/// member, not served as an empty page. The rows are the ways a token can fail to be one
/// of ours: not this service's encoding at all, this service's encoding carrying another
/// cursor's shape, and a well-formed question cursor naming a question the lens does not
/// ask.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let template_arn = template(&client, "token-template", &[FRAMEWORK_LENS]).await;

    let cases: Vec<(&str, String)> = vec![
        ("not-this-services-encoding", "not-a-token".to_string()),
        // A `ListMilestones` cursor is this service's encoding carrying another
        // operation's cursor: decodable, but not a question cursor.
        (
            "another-operations-cursor",
            state::encode_milestone_next_token(1),
        ),
        (
            "unknown-question",
            state::encode_question_token("no-such-question"),
        ),
    ];

    for (case, token) in cases {
        let answer = list_answers(
            &client,
            &template_arn,
            FRAMEWORK_LENS,
            &[format!("NextToken={token}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued NextToken is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable value is CANNOT_PARSE, got {answer:?}"
        );
        let fields: Vec<String> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| {
                        field
                            .get("Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec!["NextToken".to_string()],
            "case {case:?}: the finding must name NextToken, got {answer:?}"
        );
    }
}

/// The lens review is resolved from the TEMPLATE's own state: a `TemplateArn` that names
/// no stored template is the modelled 404 carrying the ARN as sent and the missing
/// resource's type, and a lens the stored template does not apply is the modelled 400
/// naming `LensAlias` — the lens is a VALUE of the request, not a resource it identifies.
#[tokio::test]
async fn an_unresolvable_template_or_lens_review_is_refused() {
    let client = Client::new();
    let template_arn = template(&client, "resolved-template", &[FRAMEWORK_LENS]).await;
    let deleted_template = absent_template_arn("0123456789abcdef0123456789abcdef");

    let absent = list_answers(&client, &deleted_template, FRAMEWORK_LENS, &[]).await;
    assert_eq!(
        absent.status, 404,
        "case \"absent-template\": an unresolvable template is a 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code, "ResourceNotFoundException",
        "case \"absent-template\": expected ResourceNotFoundException, got {absent:?}"
    );
    assert_eq!(
        absent.body.get("ResourceType").and_then(Value::as_str),
        Some("REVIEW_TEMPLATE"),
        "case \"absent-template\": expected ResourceType REVIEW_TEMPLATE, got {absent:?}"
    );
    assert_eq!(
        absent.body.get("ResourceId").and_then(Value::as_str),
        Some(deleted_template.as_str()),
        "case \"absent-template\": expected the ARN as sent, got {absent:?}"
    );

    // The rows differ along how the unapplied lens is named, because what the request asks
    // for does not depend on the spelling.
    for (case, lens) in [
        ("lens-not-applied", UNSHIPPED_LENS),
        (
            "lens-not-applied-by-arn",
            "arn:aws:wellarchitected:us-east-1::lens/softwareasaservice",
        ),
    ] {
        let answer = list_answers(&client, &template_arn, lens, &[]).await;
        assert_field_validation(
            case,
            &answer,
            "LensAlias",
            &format!(
                "No lens review for lens alias {lens} was found in review template \
                 {template_arn}."
            ),
        );
    }
}

/// A lens is named either way on the wire, and an applied lens this emulator ships no
/// question set for has a review with nothing in it. The rows are the spellings of the
/// same applied lens, then the applied lens with no shipped questions.
#[tokio::test]
async fn an_applied_lens_resolves_by_either_name() {
    let client = Client::new();
    let template_arn = template(
        &client,
        "named-template",
        &[FRAMEWORK_LENS, UNSHIPPED_LENS],
    )
    .await;

    for lens in [
        FRAMEWORK_LENS,
        FRAMEWORK_LENS_ARN,
        // A lens ARN may pin a version after the alias, which names the same lens.
        "arn:aws:wellarchitected:us-east-1::lens/wellarchitected/2023-10-03",
    ] {
        let (summaries, _) =
            collect_answers(&client, lens, &template_arn, lens, &[], None).await;
        assert_eq!(
            summaries.len(),
            FRAMEWORK_QUESTIONS,
            "case {lens:?}: the same lens must report the same {FRAMEWORK_QUESTIONS} questions, got {}",
            summaries.len()
        );
        let answer = list_answers(&client, &template_arn, lens, &[]).await;
        assert_eq!(
            member(&answer, "LensAlias"),
            lens,
            "case {lens:?}: the listing must echo the lens as sent, got {answer:?}"
        );
    }

    // The template applies a lens whose question set this emulator does not ship, so its
    // review exists and asks nothing.
    let answer = list_answers(&client, &template_arn, UNSHIPPED_LENS, &[]).await;
    assert_eq!(
        answer.status, 200,
        "case {UNSHIPPED_LENS:?}: an applied lens has a review, got {answer:?}"
    );
    assert!(
        summaries_of(&answer).is_empty(),
        "case {UNSHIPPED_LENS:?}: a lens with no shipped question set asks nothing, got {answer:?}"
    );
}

/// The listing is a REPORT: reading it changes nothing a later caller sees — neither the
/// answers it projects nor the template's own roll-up. The rows are repeated reads, with
/// and without the paging and filter members.
#[tokio::test]
async fn listing_the_answers_writes_nothing() {
    let client = Client::new();
    let template_arn = template(&client, "report-template", &[FRAMEWORK_LENS]).await;
    let answered_question_choices = framework_choice_ids("select-service");
    let in_place: Vec<&str> = answered_question_choices
        .iter()
        .take(1)
        .map(String::as_str)
        .collect();
    client.record_template_answer(
        &template_arn,
        FRAMEWORK_LENS,
        "select-service",
        answered_over(&answered_question_choices, &in_place, true, None),
    );

    let before = question_counts(&client, &template_arn).await;
    let mut seen: Vec<Vec<Value>> = Vec::new();
    for (case, filters, max_results) in [
        ("plain", vec![], None),
        ("paged", vec![], Some(3)),
        (
            "filtered",
            vec!["PillarId=costOptimization".to_string()],
            None,
        ),
        ("plain-again", vec![], None),
    ] {
        let (summaries, _) = collect_answers(
            &client,
            case,
            &template_arn,
            FRAMEWORK_LENS,
            &filters,
            max_results,
        )
        .await;
        if filters.is_empty() {
            seen.push(summaries);
        }
    }

    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "two listings of the same state must answer identically, got {seen:?}"
    );
    let after = question_counts(&client, &template_arn).await;
    assert_eq!(
        after, before,
        "reading the answers must not move the template's roll-up, expected {before:?}, got {after:?}"
    );
}

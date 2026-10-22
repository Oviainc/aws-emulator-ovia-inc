// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListReviewTemplateAnswers`: the answers of ONE lens review
//! of ONE review template.
//!
//! The rule these pin, over TABLES of inputs rather than one recorded request: a review
//! template's lens review reports one `ReviewTemplateAnswerSummary` per question the
//! applied lens asks, in the lens's own order, narrowed by the request's `PillarId`
//! filter and paged by `MaxResults`/`NextToken`; each summary carries the answer the
//! TEMPLATE stores for that question and `UNANSWERED` where it stores none, and a
//! `TemplateArn` or a lens the template does not apply resolves to a modelled 404.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the state seeding live in the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    self, AnswerRecord, ChoiceAnswerRecord,
};

use super::workload_harness::{encode_path_segment, Answer, Client};

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

/// The alias of the one lens this emulator ships a question set for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The same lens named by its AWS-official ARN: "this is either the lens alias … or the
/// lens ARN, such as `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias`
/// doc).
const FRAMEWORK_LENS_ARN: &str = "arn:aws:wellarchitected:us-east-1::lens/wellarchitected";

/// A lens alias AWS publishes but this emulator ships no question set for.
const UNSHIPPED_LENS: &str = "serverless";

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

/// Create a review template applying `lenses` and answer its `TemplateArn`.
async fn template(client: &Client, name: &str, lenses: &[&str]) -> String {
    let answer = client
        .create_review_template(json!({
            "TemplateName": name,
            "Description": "a review template under test",
            "Lenses": lenses,
            "ClientRequestToken": format!("tok-{name}"),
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateReviewTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("TemplateArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers`: both labels are
/// non-greedy, so each travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn list_answers(
    client: &Client,
    template_arn: &str,
    lens: &str,
    query: &[String],
) -> Answer {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    client
        .send(
            "GET",
            &format!(
                "/reviewTemplates/{}/lensReviews/{}/answers{suffix}",
                encode_path_segment(template_arn),
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

/// An answer that records `selected` as the best practices in place and `choices` as the
/// status of each choice it speaks about.
fn answered(
    selected: &[&str],
    choices: &[(&str, &str)],
    is_applicable: bool,
    reason: Option<&str>,
) -> AnswerRecord {
    AnswerRecord {
        selected_choices: selected.iter().map(ToString::to_string).collect(),
        choice_answers: choices
            .iter()
            .map(|(choice_id, status)| {
                (
                    (*choice_id).to_string(),
                    ChoiceAnswerRecord {
                        status: (*status).to_string(),
                        reason: None,
                    },
                )
            })
            .collect(),
        is_applicable,
        reason: reason.map(ToString::to_string),
    }
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

/// One row of the stored-answer table: the question an answer is recorded on, the answer
/// as the store holds it, and the members the summary must then report for it.
struct AnswerCase {
    question_id: &'static str,
    answer: AnswerRecord,
    selected_choices: Value,
    choice_answer_summaries: Value,
    is_applicable: bool,
    reason: Option<&'static str>,
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
            question_id: "ops-01",
            answer: answered(&["bp-1"], &[("bp-1", "SELECTED")], true, None),
            selected_choices: json!(["bp-1"]),
            choice_answer_summaries: json!([{ "ChoiceId": "bp-1", "Status": "SELECTED" }]),
            is_applicable: true,
            reason: None,
        },
        AnswerCase {
            question_id: "sec-04",
            answer: answered(
                &["bp-1", "bp-2"],
                &[("bp-1", "SELECTED"), ("bp-2", "SELECTED"), ("bp-3", "UNSELECTED")],
                true,
                None,
            ),
            selected_choices: json!(["bp-1", "bp-2"]),
            choice_answer_summaries: json!([
                { "ChoiceId": "bp-1", "Status": "SELECTED" },
                { "ChoiceId": "bp-2", "Status": "SELECTED" },
                { "ChoiceId": "bp-3", "Status": "UNSELECTED" },
            ]),
            is_applicable: true,
            reason: None,
        },
        AnswerCase {
            question_id: "rel-13",
            answer: answered(&[], &[], false, Some("OUT_OF_SCOPE")),
            selected_choices: json!([]),
            choice_answer_summaries: json!([]),
            is_applicable: false,
            reason: Some("OUT_OF_SCOPE"),
        },
        AnswerCase {
            question_id: "sus-06",
            answer: answered(&["bp-9"], &[("bp-9", "NOT_APPLICABLE")], true, None),
            selected_choices: json!(["bp-9"]),
            choice_answer_summaries: json!([{ "ChoiceId": "bp-9", "Status": "NOT_APPLICABLE" }]),
            is_applicable: true,
            reason: None,
        },
    ];

    let client = Client::new();
    let template_arn = template(&client, "answered-template", &[FRAMEWORK_LENS]).await;

    let mut recorded = 0i64;
    for case in cases {
        let question_id = case.question_id;
        client.record_template_answer(&template_arn, FRAMEWORK_LENS, question_id, case.answer);
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
            Some(&case.selected_choices),
            "case {question_id:?}: expected SelectedChoices {:?}, got {summary:?}",
            case.selected_choices
        );
        assert_eq!(
            summary.get("ChoiceAnswerSummaries"),
            Some(&case.choice_answer_summaries),
            "case {question_id:?}: expected ChoiceAnswerSummaries {:?}, got {summary:?}",
            case.choice_answer_summaries
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

/// `PillarId` narrows the question set and resolves nothing: each pillar of the catalog
/// is a row — including the pillars no issue cited — and a pillar the lens does not ask
/// about is an EMPTY page rather than a failure.
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

    // A pillar this lens asks nothing about matches no question.
    let answer = list_answers(
        &client,
        &template_arn,
        FRAMEWORK_LENS,
        &["PillarId=noSuchPillar".to_string()],
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"unknown-pillar\": a filter that matches nothing is an empty page, got {answer:?}"
    );
    assert!(
        summaries_of(&answer).is_empty(),
        "case \"unknown-pillar\": expected no summaries, got {answer:?}"
    );
    assert!(
        answer.body.get("NextToken").is_none(),
        "case \"unknown-pillar\": a last page carries no NextToken, got {answer:?}"
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
/// no stored template, and a lens the stored template does not apply, are each the
/// modelled 404 carrying the identifier as sent and the missing resource's type.
#[tokio::test]
async fn an_unresolvable_template_or_lens_review_is_a_404() {
    let client = Client::new();
    let template_arn = template(&client, "resolved-template", &[FRAMEWORK_LENS]).await;
    let deleted_template = absent_template_arn("0123456789abcdef0123456789abcdef");

    // (case, template ARN, lens, expected ResourceType, expected ResourceId)
    let cases: Vec<(&str, String, &str, &str, String)> = vec![
        (
            "absent-template",
            deleted_template.clone(),
            FRAMEWORK_LENS,
            "REVIEW_TEMPLATE",
            deleted_template,
        ),
        (
            "lens-not-applied",
            template_arn.clone(),
            UNSHIPPED_LENS,
            "LENS_REVIEW",
            format!("{template_arn}.{UNSHIPPED_LENS}"),
        ),
        (
            "lens-not-applied-by-arn",
            template_arn.clone(),
            "arn:aws:wellarchitected:us-east-1::lens/softwareasaservice",
            "LENS_REVIEW",
            format!("{template_arn}.arn:aws:wellarchitected:us-east-1::lens/softwareasaservice"),
        ),
    ];

    for (case, arn, lens, resource_type, resource_id) in cases {
        let answer = list_answers(&client, &arn, lens, &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unresolvable lookup is a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some(resource_type),
            "case {case:?}: expected ResourceType {resource_type:?}, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(resource_id.as_str()),
            "case {case:?}: expected ResourceId {resource_id:?}, got {answer:?}"
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
    client.record_template_answer(
        &template_arn,
        FRAMEWORK_LENS,
        "cost-05",
        answered(&["bp-1"], &[("bp-1", "SELECTED")], true, None),
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

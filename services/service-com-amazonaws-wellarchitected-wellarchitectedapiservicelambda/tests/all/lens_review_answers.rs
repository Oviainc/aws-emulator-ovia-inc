// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAnswers`: the answers of ONE lens review of ONE
//! workload.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: a lens
//! review reports one `AnswerSummary` per question the lens asks, in the lens's own
//! order, narrowed by the request's `PillarId` / `QuestionPriority` filters and paged by
//! `MaxResults`/`NextToken`; each summary carries the answer the reviewed workload
//! stores for that question, and `UNANSWERED` where it stores none. The workload is the
//! live one, or — when the request names a milestone — the one that milestone recorded.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::workload_harness::{
    answered, answered_over, assert_field_validation, created_id, framework_choice_ids,
    minimal_create, official_lens_arn, Answer, Client,
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

/// The alias of the one lens this emulator ships a question set for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that this emulator's AWS-official lens catalog carries no
/// content for, so its review asks no questions here. The catalog itself — every lens it
/// DOES carry, each reviewable with its own question set — is pinned by
/// `super::official_lens_catalog`.
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the ARNs in
/// a response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// The modelled `MaxResults` range for this operation, `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListAnswers")`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 50);

/// The page a request that omits `MaxResults` gets: the modelled maximum, so the lens's
/// 57 questions do not fit in one page.
const PAGE_CEILING: usize = 50;

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

async fn list_answers(client: &Client, workload_id: &str, lens: &str, query: &[String]) -> Answer {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/{lens}/answers{suffix}"),
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

/// Every `AnswerSummary` of one lens review, following `NextToken` to the end. Returns
/// the summaries in the order they were served and how many pages it took.
async fn collect_answers(
    client: &Client,
    case: &str,
    workload_id: &str,
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
        let answer = list_answers(client, workload_id, lens, &query).await;
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

async fn framework_workload(client: &Client, name: &str) -> String {
    created_id(
        &client.create(minimal_create(name, &format!("tok-{name}"))).await,
        name,
    )
}

/// The `RiskCounts` `GetWorkload` reports for a workload, as a sorted map.
async fn risk_counts(client: &Client, workload_id: &str) -> BTreeMap<String, i64> {
    let fetched = client.get(workload_id).await;
    fetched
        .body
        .get("Workload")
        .and_then(|workload| workload.get("RiskCounts"))
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// The roll-up expected when `answered` questions of the framework lens carry the named
/// risks and the rest are outstanding.
fn expected_counts(answered: &[(&str, i64)]) -> BTreeMap<String, i64> {
    let mut counts: BTreeMap<String, i64> = ["UNANSWERED", "HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE"]
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
        .collect();
    let mut outstanding = i64::try_from(FRAMEWORK_QUESTIONS).unwrap_or(0);
    for (risk, count) in answered {
        *counts.entry((*risk).to_string()).or_default() += count;
        outstanding -= count;
    }
    counts.insert("UNANSWERED".to_string(), outstanding);
    counts
}

/// Every question of the attached lens is reported, once, in the lens's own order, with
/// the members the summary derives from the lens and from the (absent) answer. The rows
/// are the pillars of the catalog — each with its published question count — so the
/// whole question set is checked rather than one cited question.
#[tokio::test]
async fn every_question_of_the_attached_lens_is_reported_unanswered() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "answers-host").await;

    let (summaries, _) =
        collect_answers(&client, "all", &workload_id, FRAMEWORK_LENS, &[], None).await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "the framework lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );

    let ids = question_ids(&summaries);
    let unique: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every question must be reported once, got {ids:?}"
    );

    // The pillars appear as consecutive runs, in the catalog's order, each as long as
    // that pillar's published question count.
    let mut offset = 0usize;
    for (pillar, count) in FRAMEWORK_PILLAR_QUESTIONS {
        let run: Vec<&str> = summaries[offset..offset + count]
            .iter()
            .map(|summary| summary.get("PillarId").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert!(
            run.iter().all(|reported| *reported == pillar),
            "case {pillar:?}: questions {offset}..{} must all belong to {pillar}, got {run:?}",
            offset + count
        );
        offset += count;
    }

    for (index, summary) in summaries.iter().enumerate() {
        let case = format!("question-{index}");
        let question_id = summary.get("QuestionId").and_then(Value::as_str).unwrap_or("");
        let title = summary
            .get("QuestionTitle")
            .and_then(Value::as_str)
            .unwrap_or("");
        let pillar = summary.get("PillarId").and_then(Value::as_str).unwrap_or("");
        // The modelled bounds: QuestionId @length(1, 128), QuestionTitle @length(1, 512),
        // PillarId @length(1, 64) (smithy_query: ListAnswers closure).
        assert!(
            (1..=128).contains(&question_id.len()),
            "case {case:?}: QuestionId must satisfy @length(1, 128), got {question_id:?}"
        );
        assert!(
            (1..=512).contains(&title.len()),
            "case {case:?}: QuestionTitle must satisfy @length(1, 512), got {title:?}"
        );
        assert!(
            (1..=64).contains(&pillar.len()),
            "case {case:?}: PillarId must satisfy @length(1, 64), got {pillar:?}"
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
            summary.get("QuestionType").and_then(Value::as_str),
            Some("NON_PRIORITIZED"),
            "case {case:?}: no profile can prioritize a question here, got {summary:?}"
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
    }

    // The envelope names the review that was read: the workload, the lens as the request
    // named it and the ARN it resolves to. No milestone was named, so none is reported.
    let answer = list_answers(&client, &workload_id, FRAMEWORK_LENS, &[]).await;
    assert_eq!(
        member(&answer, "WorkloadId"),
        workload_id,
        "the envelope must name the workload, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "LensAlias"),
        FRAMEWORK_LENS,
        "the envelope must name the lens, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "LensArn"),
        official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
        "the envelope must carry the lens ARN, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("MilestoneNumber"),
        None,
        "a read of the live review names no milestone, got {answer:?}"
    );
}

/// `PillarId` SELECTS the questions of one pillar of the lens and `QuestionPriority`
/// filters by the `QuestionType` the questions carry: each pillar reports its own
/// published question count, and a `PillarId` the lens does not define is refused rather
/// than selecting nothing — the pillars are the lens's own content.
#[tokio::test]
async fn the_filters_narrow_the_question_set() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "filter-host").await;

    let mut cases: Vec<(String, Vec<String>, usize)> = Vec::new();
    for (pillar, count) in FRAMEWORK_PILLAR_QUESTIONS {
        cases.push((
            format!("pillar-{pillar}"),
            vec![format!("PillarId={pillar}")],
            count,
        ));
    }
    // `QuestionPriority` publishes PRIORITIZED | NONE (smithy_query: QuestionPriority).
    // Every question here is NON_PRIORITIZED, so NONE selects them all and PRIORITIZED
    // none of them.
    cases.push((
        "priority-none".to_string(),
        vec!["QuestionPriority=NONE".to_string()],
        FRAMEWORK_QUESTIONS,
    ));
    cases.push((
        "priority-prioritized".to_string(),
        vec!["QuestionPriority=PRIORITIZED".to_string()],
        0,
    ));
    // Filters combine.
    cases.push((
        "pillar-and-priority".to_string(),
        vec![
            "PillarId=sustainability".to_string(),
            "QuestionPriority=NONE".to_string(),
        ],
        6,
    ));
    cases.push((
        "pillar-and-prioritized".to_string(),
        vec![
            "PillarId=sustainability".to_string(),
            "QuestionPriority=PRIORITIZED".to_string(),
        ],
        0,
    ));

    for (case, filters, expected) in cases {
        let (summaries, pages) =
            collect_answers(&client, &case, &workload_id, FRAMEWORK_LENS, &filters, None).await;
        assert_eq!(
            summaries.len(),
            expected,
            "case {case:?}: expected {expected} summaries, got {}",
            summaries.len()
        );
        // A request that omits `MaxResults` gets the modelled maximum page, so the
        // filtered set takes as many pages as it needs.
        let expected_pages = expected.div_ceil(PAGE_CEILING).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {expected} summaries take {expected_pages} full pages, got {pages}"
        );
        if let Some(pillar) = filters
            .iter()
            .find_map(|filter| filter.strip_prefix("PillarId="))
        {
            for summary in &summaries {
                assert_eq!(
                    summary.get("PillarId").and_then(Value::as_str),
                    Some(pillar),
                    "case {case:?}: only {pillar} questions may be reported, got {summary:?}"
                );
            }
        }
    }

    // A `PillarId` the lens does not define is refused rather than selecting nothing —
    // including the right pillar in the wrong case, because a `PillarId` is matched
    // exactly.
    for (case, pillar) in [
        ("pillar-absent", "notapillar"),
        ("pillar-wrong-case", "SECURITY"),
    ] {
        let answer = list_answers(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            &[format!("PillarId={pillar}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a pillar the lens does not define is refused, got {answer:?}"
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

/// Paging enumerates every question exactly once, in the same order, whatever page size
/// the request asks for: checked EXHAUSTIVELY over the published `MaxResults` range, and
/// at each bound and one step past it the model's own refusal applies.
#[tokio::test]
async fn paging_enumerates_every_question_exactly_once() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "paging-host").await;

    let (whole, pages) =
        collect_answers(&client, "unpaged", &workload_id, FRAMEWORK_LENS, &[], None).await;
    assert_eq!(
        pages,
        FRAMEWORK_QUESTIONS.div_ceil(PAGE_CEILING),
        "a request that omits MaxResults asks for the modelled maximum page, got {pages} pages"
    );
    let expected_ids = question_ids(&whole);

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    for max in minimum..=maximum {
        let case = format!("max-results-{max}");
        let (paged, pages) = collect_answers(
            &client,
            &case,
            &workload_id,
            FRAMEWORK_LENS,
            &[],
            Some(max),
        )
        .await;
        assert_eq!(
            question_ids(&paged),
            expected_ids,
            "case {case:?}: paging must serve the same questions in the same order"
        );
        let expected_pages = usize::try_from(
            (i64::try_from(FRAMEWORK_QUESTIONS).unwrap_or(0) + max - 1) / max,
        )
        .unwrap_or(0);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {FRAMEWORK_QUESTIONS} questions at {max} per page is {expected_pages} pages, got {pages}"
        );
    }

    // One step past each published bound is refused by the modelled @range.
    for (case, max) in [
        ("one-below-the-minimum", minimum - 1),
        ("one-above-the-maximum", maximum + 1),
    ] {
        let answer = list_answers(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            &[format!("MaxResults={max}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: MaxResults={max} is outside @range(1, 50), got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // A token this service did not issue is a bad request, not an empty page. Rows differ
    // along how the token is malformed.
    for (case, token) in [
        ("not-hex", "notatoken"),
        ("hex-but-not-a-cursor", "0a0b0c"),
        // A well-formed cursor naming a question the lens does not ask.
        (
            "unknown-question",
            "7b2271223a226e6f742d612d7175657374696f6e227d",
        ),
    ] {
        let answer = list_answers(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            &[format!("NextToken={token}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued NextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// One row of the lookup table: the review a request asks for, and the 404 it must be
/// answered with.
struct MissingReview {
    case: &'static str,
    workload_id: String,
    lens: &'static str,
    filters: Vec<String>,
    resource_type: &'static str,
    resource_id: String,
    message: String,
}

/// The review that is read must exist: the workload is RESOLVED — so an id nothing is
/// stored under, and a milestone the workload never recorded, are each the modelled 404 —
/// while the lens is a VALUE of the request, so a lens the workload does not review is the
/// modelled 400 naming `LensAlias`. An
/// associated lens this emulator ships no question set for asks nothing, so its review
/// is empty rather than missing.
#[tokio::test]
async fn a_review_that_does_not_exist_is_a_not_found() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "lookup-host").await;
    // A second workload carries only a lens this emulator ships no questions for.
    let mut unshipped = minimal_create("unshipped-lens-host", "tok-unshipped");
    unshipped["Lenses"] = json!([UNSHIPPED_LENS]);
    let unshipped_id = created_id(&client.create(unshipped).await, "unshipped");

    let not_found_message = |subject: &str| {
        format!(
            "[ResourceNotFound] No {subject} was found in account {}.",
            engine::emulator_account_id()
        )
    };

    let cases: Vec<MissingReview> = vec![
        MissingReview {
            case: "absent-workload",
            workload_id: ABSENT_HEX_ID.to_string(),
            lens: FRAMEWORK_LENS,
            filters: Vec::new(),
            resource_type: "WORKLOAD",
            resource_id: ABSENT_HEX_ID.to_string(),
            message: not_found_message(&format!("workload with ID {ABSENT_HEX_ID}")),
        },
        MissingReview {
            case: "milestone-not-recorded",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            filters: vec!["MilestoneNumber=1".to_string()],
            resource_type: "MILESTONE",
            resource_id: format!("{workload_id}.1"),
            message: not_found_message(&format!(
                "milestone with number 1 in a workload with ID {workload_id}"
            )),
        },
        MissingReview {
            case: "milestone-at-the-range-maximum",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            filters: vec!["MilestoneNumber=100".to_string()],
            resource_type: "MILESTONE",
            resource_id: format!("{workload_id}.100"),
            message: not_found_message(&format!(
                "milestone with number 100 in a workload with ID {workload_id}"
            )),
        },
    ];

    for row in cases {
        let MissingReview {
            case,
            workload_id: host,
            lens,
            filters,
            resource_type,
            resource_id,
            message,
        } = row;
        let answer = list_answers(&client, &host, lens, &filters).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            resource_type,
            "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            resource_id,
            "case {case:?}: expected ResourceId {resource_id}, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            message,
            "case {case:?}: expected the resource/account not-found message, got {answer:?}"
        );
    }

    // A lens the workload does not review names no review of it, whichever workload of the
    // two is asked and whichever lens it is: the value is refused as input.
    for (case, host, lens) in [
        ("lens-not-associated", &workload_id, UNSHIPPED_LENS),
        ("framework-lens-not-associated", &unshipped_id, FRAMEWORK_LENS),
    ] {
        let answer = list_answers(&client, host, lens, &[]).await;
        assert_field_validation(
            case,
            &answer,
            "LensAlias",
            &format!("No lens review for lens alias {lens} was found in workload {host}."),
        );
    }

    // The associated lens with no question set here: a review with no questions, not a
    // missing review.
    let empty = list_answers(&client, &unshipped_id, UNSHIPPED_LENS, &[]).await;
    assert_eq!(
        empty.status, 200,
        "case \"associated-unshipped-lens\": the review exists, got {empty:?}"
    );
    assert_eq!(
        empty.body.get("AnswerSummaries"),
        Some(&json!([])),
        "case \"associated-unshipped-lens\": the lens asks no questions here, got {empty:?}"
    );
    assert_eq!(
        empty.body.get("NextToken"),
        None,
        "case \"associated-unshipped-lens\": an empty review has no next page, got {empty:?}"
    );
}

/// One row of the rating table: the status an answer records for each choice of the
/// question — its WHOLE choice set, in the lens's own order, which is the key space those
/// statuses live in — and the summary the read must report for it.
struct RatedAnswer {
    case: &'static str,
    statuses: Vec<(String, &'static str)>,
    is_applicable: bool,
    reason: Option<&'static str>,
    risk: &'static str,
}

/// A stored answer is what the summary reports, and the question's `Risk` is DERIVED
/// from it: a question that does not apply is `NOT_APPLICABLE`, one with no best
/// practice in place is `HIGH`, one with a choice still unselected is `MEDIUM` and one
/// with nothing unselected is `NONE`. The same derivation moves the workload's risk
/// roll-up, so the rows check both. Rows differ along what the answer records.
#[tokio::test]
async fn a_stored_answer_is_reported_and_rated() {
    // The question answered is the same in every row; what varies is the answer. Its
    // choices are the question's own, so a row records a status for each of them.
    const QUESTION: &str = "sec-01";
    let choices = framework_choice_ids(QUESTION);
    let last = choices
        .last()
        .cloned()
        .expect("the question under test offers choices");

    // One status per choice of the question, from a plan that says which of them carry
    // something other than `SELECTED`.
    let statuses = |other: &[(&str, &'static str)]| -> Vec<(String, &'static str)> {
        choices
            .iter()
            .map(|choice_id| {
                let status = other
                    .iter()
                    .find(|(named, _)| *named == choice_id.as_str())
                    .map_or("SELECTED", |(_, status)| *status);
                (choice_id.clone(), status)
            })
            .collect()
    };
    let every_choice_unselected: Vec<(&str, &'static str)> = choices
        .iter()
        .map(|choice_id| (choice_id.as_str(), "UNSELECTED"))
        .collect();

    let cases: Vec<RatedAnswer> = vec![
        RatedAnswer {
            case: "every-best-practice-in-place",
            statuses: statuses(&[]),
            is_applicable: true,
            reason: None,
            risk: "NONE",
        },
        RatedAnswer {
            case: "one-best-practice-missing",
            statuses: statuses(&[(last.as_str(), "UNSELECTED")]),
            is_applicable: true,
            reason: None,
            risk: "MEDIUM",
        },
        RatedAnswer {
            case: "nothing-in-place",
            statuses: statuses(&every_choice_unselected),
            is_applicable: true,
            reason: None,
            risk: "HIGH",
        },
        RatedAnswer {
            case: "unselected-choice-does-not-apply",
            statuses: statuses(&[(last.as_str(), "NOT_APPLICABLE")]),
            is_applicable: true,
            reason: None,
            risk: "NONE",
        },
        RatedAnswer {
            case: "question-does-not-apply",
            statuses: statuses(&every_choice_unselected),
            is_applicable: false,
            reason: Some("OUT_OF_SCOPE"),
            risk: "NOT_APPLICABLE",
        },
    ];

    for row in cases {
        let RatedAnswer {
            case,
            statuses,
            is_applicable,
            reason,
            risk,
        } = row;
        // What the read must report follows from the statuses the answer records: the
        // choices in place are the ones recorded `SELECTED`, and the per-choice answers
        // are one entry per choice of the question, in the lens's own choice order.
        let selected: Vec<&str> = statuses
            .iter()
            .filter(|(_, status)| *status == "SELECTED")
            .map(|(choice_id, _)| choice_id.as_str())
            .collect();
        let expected_selected = Value::Array(
            selected
                .iter()
                .map(|choice_id| json!(choice_id))
                .collect::<Vec<Value>>(),
        );
        let expected_choice_answers = Value::Array(
            statuses
                .iter()
                .map(|(choice_id, status)| json!({ "ChoiceId": choice_id, "Status": status }))
                .collect::<Vec<Value>>(),
        );
        let status_refs: Vec<(&str, &str)> = statuses
            .iter()
            .map(|(choice_id, status)| (choice_id.as_str(), *status))
            .collect();
        let record = answered(&status_refs, is_applicable, reason);

        let client = Client::new();
        let workload_id = framework_workload(&client, &format!("rated-{case}")).await;
        client.record_answer(&workload_id, FRAMEWORK_LENS, QUESTION, record);

        let (summaries, _) =
            collect_answers(&client, case, &workload_id, FRAMEWORK_LENS, &[], None).await;
        assert_eq!(
            summaries.len(),
            FRAMEWORK_QUESTIONS,
            "case {case:?}: answering a question does not change the question set, got {}",
            summaries.len()
        );
        let summary = summaries
            .iter()
            .find(|summary| summary.get("QuestionId").and_then(Value::as_str) == Some(QUESTION))
            .cloned()
            .unwrap_or(Value::Null);

        assert_eq!(
            summary.get("Risk").and_then(Value::as_str),
            Some(risk),
            "case {case:?}: expected Risk {risk}, got {summary:?}"
        );
        assert_eq!(
            summary.get("SelectedChoices"),
            Some(&expected_selected),
            "case {case:?}: expected SelectedChoices {expected_selected:?}, got {summary:?}"
        );
        assert_eq!(
            summary.get("ChoiceAnswerSummaries"),
            Some(&expected_choice_answers),
            "case {case:?}: expected ChoiceAnswerSummaries {expected_choice_answers:?}, \
             got {summary:?}"
        );
        assert_eq!(
            summary.get("IsApplicable"),
            Some(&Value::Bool(is_applicable)),
            "case {case:?}: expected IsApplicable {is_applicable}, got {summary:?}"
        );
        assert_eq!(
            summary.get("Reason").and_then(Value::as_str),
            reason,
            "case {case:?}: expected Reason {reason:?}, got {summary:?}"
        );

        // Every other question is still outstanding, and the workload's roll-up has
        // moved the answered one out of UNANSWERED into its risk.
        let outstanding = summaries
            .iter()
            .filter(|summary| summary.get("Risk").and_then(Value::as_str) == Some("UNANSWERED"))
            .count();
        assert_eq!(
            outstanding,
            FRAMEWORK_QUESTIONS - 1,
            "case {case:?}: one question is answered, so {} must be outstanding, got {outstanding}",
            FRAMEWORK_QUESTIONS - 1
        );
        let expected = expected_counts(&[(risk, 1)]);
        let actual = risk_counts(&client, &workload_id).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: the roll-up must count the answer as {risk}, expected {expected:?}, got {actual:?}"
        );
    }
}

/// A milestone records the workload at an instant, answers included: a read scoped to a
/// milestone reports the answers as they were, while the live review moves on. Rows
/// differ along which question the later change touches.
#[tokio::test]
async fn a_milestone_freezes_the_answers_it_recorded() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "frozen-host").await;

    // Before the milestone: one question answered with nothing in place.
    let frozen_question_choices = framework_choice_ids("rel-01");
    let later_question_choices = framework_choice_ids("cost-02");
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "rel-01",
        answered_over(&frozen_question_choices, &[], true, None),
    );
    let recorded = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": "before-the-answers", "ClientRequestToken": "tok-frozen" })),
        )
        .await;
    assert_eq!(
        recorded.status, 200,
        "case \"record\": CreateMilestone must succeed, got {recorded:?}"
    );

    // After it: that answer improves and a second question is answered.
    let frozen_all: Vec<&str> = frozen_question_choices
        .iter()
        .map(String::as_str)
        .collect();
    let later_all: Vec<&str> = later_question_choices.iter().map(String::as_str).collect();
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "rel-01",
        answered_over(&frozen_question_choices, &frozen_all, true, None),
    );
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "cost-02",
        answered_over(&later_question_choices, &later_all, true, None),
    );

    let risk_of = |summaries: &[Value], question_id: &str| -> String {
        summaries
            .iter()
            .find(|summary| summary.get("QuestionId").and_then(Value::as_str) == Some(question_id))
            .and_then(|summary| summary.get("Risk").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string()
    };

    let (live, _) = collect_answers(&client, "live", &workload_id, FRAMEWORK_LENS, &[], None).await;
    let (frozen, _) = collect_answers(
        &client,
        "milestone",
        &workload_id,
        FRAMEWORK_LENS,
        &["MilestoneNumber=1".to_string()],
        None,
    )
    .await;

    for (case, summaries, question, expected) in [
        ("live/changed-answer", &live, "rel-01", "NONE"),
        ("live/new-answer", &live, "cost-02", "NONE"),
        ("milestone/changed-answer", &frozen, "rel-01", "HIGH"),
        ("milestone/new-answer", &frozen, "cost-02", "UNANSWERED"),
    ] {
        let actual = risk_of(summaries, question);
        assert_eq!(
            actual, expected,
            "case {case:?}: {question} must report {expected}, got {actual}"
        );
    }

    // Both reads report the whole question set, and the milestone read names the
    // milestone it was scoped to.
    for (case, summaries) in [("live", &live), ("milestone", &frozen)] {
        assert_eq!(
            summaries.len(),
            FRAMEWORK_QUESTIONS,
            "case {case:?}: the lens still asks {FRAMEWORK_QUESTIONS} questions, got {}",
            summaries.len()
        );
    }
    let scoped = list_answers(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        &["MilestoneNumber=1".to_string()],
    )
    .await;
    assert_eq!(
        scoped.body.get("MilestoneNumber").and_then(Value::as_i64),
        Some(1),
        "a milestone-scoped read must name the milestone, got {scoped:?}"
    );

    // The roll-ups follow the same answers: the live workload counts two answered
    // questions, the milestone's recorded workload one.
    let expected_live = expected_counts(&[("NONE", 2)]);
    let actual_live = risk_counts(&client, &workload_id).await;
    assert_eq!(
        actual_live, expected_live,
        "the live roll-up must count both answers, expected {expected_live:?}, got {actual_live:?}"
    );
    let milestone = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/1"),
            None,
        )
        .await;
    let frozen_counts: BTreeMap<String, i64> = milestone
        .body
        .get("Milestone")
        .and_then(|recorded| recorded.get("Workload"))
        .and_then(|workload| workload.get("RiskCounts"))
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default();
    let expected_frozen = expected_counts(&[("HIGH", 1)]);
    assert_eq!(
        frozen_counts, expected_frozen,
        "the recorded workload must keep its roll-up, expected {expected_frozen:?}, got {milestone:?}"
    );
}

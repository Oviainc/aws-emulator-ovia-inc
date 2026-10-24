// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLensReviewImprovements`: the improvement plan of ONE
//! lens review of ONE workload.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: the
//! improvement plan of a lens review reports one `ImprovementSummary` per question of
//! that lens whose stored answer the review rates at an IDENTIFIED risk — `HIGH` or
//! `MEDIUM` — in the lens's own order, narrowed by the request's `PillarId` /
//! `QuestionPriority` filters and paged by `MaxResults`/`NextToken`. A question whose
//! answer leaves no best practice out (`NONE`), one the workload says does not apply
//! (`NOT_APPLICABLE`) and one with no stored answer (`UNANSWERED`) leave no identified
//! risk, so none of them is an improvement item. The reviewed workload is the live one,
//! or — when the request names a milestone — the one that milestone recorded; a review of
//! a lens the workload does not carry, of a workload that does not exist or of a
//! milestone that was never recorded does not resolve.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::AnswerRecord;

use super::workload_harness::{
    answered, created_id, encode_path_segment, minimal_create, official_lens_arn, Answer, Client,
};

/// The AWS Well-Architected Framework lens's pillars: the `PillarId` each is named by on
/// the wire, the prefix its `QuestionId`s carry and the number of questions it asks (OPS
/// 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6). Spelled out here — in the
/// order the framework presents them — so the tests pin the catalog rather than reading
/// it back out of the code under test.
const FRAMEWORK_PILLARS: [(&str, &str, usize); 6] = [
    ("operationalExcellence", "ops", 11),
    ("security", "sec", 11),
    ("reliability", "rel", 13),
    ("performance", "perf", 5),
    ("costOptimization", "cost", 11),
    ("sustainability", "sus", 6),
];

/// How many questions that lens asks in all.
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// The alias of the one lens this emulator ships a question set for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes but this emulator ships no question set for.
const UNSHIPPED_LENS: &str = "serverless";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The Region the shared harness signs its requests for, which is the Region the ARNs in
/// a response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// The modelled `MaxResults` range for THIS operation, `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListLensReviewImprovements")`) — twice
/// what `ListAnswers` over the same question set allows.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The `MaxResults` ceiling `ListAnswers` publishes, `@range(min: 1, max: 50)`: a value
/// this operation accepts and that one refuses, which is how the two reads are told apart.
const ANSWERS_MAX_RESULTS_CEILING: i64 = 50;

/// Every question of the lens, in the framework's order, as `(PillarId, QuestionId)`.
fn catalog() -> Vec<(String, String)> {
    FRAMEWORK_PILLARS
        .iter()
        .flat_map(|(pillar, prefix, count)| {
            (1..=*count).map(move |number| {
                ((*pillar).to_string(), format!("{prefix}-{number:02}"))
            })
        })
        .collect()
}

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/improvements`, with every filter
/// and paging member bound to the query string. The lens travels percent-encoded in ONE
/// path segment, exactly as an SDK sends an ARN-spelled alias.
async fn list_improvements(
    client: &Client,
    workload_id: &str,
    lens: &str,
    query: &[String],
) -> Answer {
    let encoded = encode_path_segment(lens);
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/{encoded}/improvements{suffix}"),
            None,
        )
        .await
}

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers`: the read the improvement
/// plan is a selection over, so the two can be checked against each other.
async fn list_answers(client: &Client, workload_id: &str, lens: &str, query: &[String]) -> Answer {
    let encoded = encode_path_segment(lens);
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/{encoded}/answers{suffix}"),
            None,
        )
        .await
}

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

fn summaries_of(answer: &Answer) -> Vec<Value> {
    answer
        .body
        .get("ImprovementSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn string_member(summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Each summary as `(QuestionId, Risk)`, which is what the selection rule is about.
fn rated_questions(summaries: &[Value]) -> Vec<(String, String)> {
    summaries
        .iter()
        .map(|summary| {
            (
                string_member(summary, "QuestionId"),
                string_member(summary, "Risk"),
            )
        })
        .collect()
}

/// Every `ImprovementSummary` of one lens review, following `NextToken` to the end.
/// Returns the summaries in the order they were served and how many pages it took.
async fn collect_improvements(
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
        let answer = list_improvements(client, workload_id, lens, &query).await;
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
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        assert!(
            next_token.is_none() || !page.is_empty(),
            "case {case:?}: a page that carries a NextToken must not be empty, got {answer:?}"
        );
        collected.extend(page);
        pages += 1;
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
        &client
            .create(minimal_create(name, &format!("tok-{name}")))
            .await,
        name,
    )
}

/// One of the four ratings an answered question can carry, chosen by `index` so a seeded
/// question set mixes all of them: the record to store and the `Risk` it is rated at.
fn spread_answer(index: usize) -> (AnswerRecord, &'static str) {
    match index % 4 {
        // Nothing left out: no risk.
        0 => (
            answered(
                &["bp-1", "bp-2"],
                &[("bp-1", "SELECTED"), ("bp-2", "SELECTED")],
                true,
                None,
            ),
            "NONE",
        ),
        // No best practice in place at all.
        1 => (
            answered(
                &[],
                &[("bp-1", "UNSELECTED"), ("bp-2", "UNSELECTED")],
                true,
                None,
            ),
            "HIGH",
        ),
        // Some in place, one still out.
        2 => (
            answered(
                &["bp-1"],
                &[("bp-1", "SELECTED"), ("bp-2", "UNSELECTED")],
                true,
                None,
            ),
            "MEDIUM",
        ),
        // The question does not apply to this workload.
        _ => (
            answered(&[], &[("bp-1", "NOT_APPLICABLE")], false, Some("OUT_OF_SCOPE")),
            "NOT_APPLICABLE",
        ),
    }
}

/// A workload whose every question is answered, cycling through the four ratings. Answers
/// the workload id and the `(PillarId, QuestionId, Risk)` the seeding stored, so a test
/// states its expectation from the seed rather than from the code under test.
async fn workload_with_spread_answers(
    client: &Client,
    name: &str,
) -> (String, Vec<(String, String, String)>) {
    let workload_id = framework_workload(client, name).await;
    let mut seeded: Vec<(String, String, String)> = Vec::new();
    for (index, (pillar, question_id)) in catalog().into_iter().enumerate() {
        let (record, risk) = spread_answer(index);
        client.record_answer(&workload_id, FRAMEWORK_LENS, &question_id, record);
        seeded.push((pillar, question_id, risk.to_string()));
    }
    (workload_id, seeded)
}

/// Whether a `Risk` is one the review IDENTIFIED, which is what makes a question an
/// improvement item: `HIGH` and `MEDIUM`, of the five members `Risk` publishes.
fn is_identified_risk(risk: &str) -> bool {
    matches!(risk, "HIGH" | "MEDIUM")
}

/// One row of the rating table: an answer, and whether — and at what risk — it leaves the
/// question in the improvement plan.
struct Rated {
    case: &'static str,
    question_id: &'static str,
    record: Option<AnswerRecord>,
    plan_risk: Option<&'static str>,
}

/// The plan is the questions whose stored answer carries an identified risk, and nothing
/// else: rows differ along what the answer records, and the unanswered row records no
/// answer at all.
#[tokio::test]
async fn only_identified_risks_appear_in_the_improvement_plan() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "plan-host").await;

    let rows: Vec<Rated> = vec![
        Rated {
            case: "nothing-in-place",
            question_id: "ops-02",
            record: Some(answered(
                &[],
                &[("bp-1", "UNSELECTED"), ("bp-2", "UNSELECTED")],
                true,
                None,
            )),
            plan_risk: Some("HIGH"),
        },
        Rated {
            case: "one-best-practice-missing",
            question_id: "sec-03",
            record: Some(answered(
                &["bp-1"],
                &[("bp-1", "SELECTED"), ("bp-2", "UNSELECTED")],
                true,
                None,
            )),
            plan_risk: Some("MEDIUM"),
        },
        Rated {
            case: "every-best-practice-in-place",
            question_id: "rel-04",
            record: Some(answered(
                &["bp-1", "bp-2"],
                &[("bp-1", "SELECTED"), ("bp-2", "SELECTED")],
                true,
                None,
            )),
            plan_risk: None,
        },
        Rated {
            case: "unselected-choice-does-not-apply",
            question_id: "perf-02",
            record: Some(answered(
                &["bp-1"],
                &[("bp-1", "SELECTED"), ("bp-2", "NOT_APPLICABLE")],
                true,
                None,
            )),
            plan_risk: None,
        },
        Rated {
            case: "question-does-not-apply",
            question_id: "cost-06",
            record: Some(answered(
                &[],
                &[("bp-1", "NOT_APPLICABLE")],
                false,
                Some("OUT_OF_SCOPE"),
            )),
            plan_risk: None,
        },
        Rated {
            case: "no-stored-answer",
            question_id: "sus-03",
            record: None,
            plan_risk: None,
        },
    ];

    for row in &rows {
        if let Some(record) = row.record.clone() {
            client.record_answer(&workload_id, FRAMEWORK_LENS, row.question_id, record);
        }
    }

    let (summaries, pages) =
        collect_improvements(&client, "plan", &workload_id, FRAMEWORK_LENS, &[], None).await;
    assert_eq!(
        pages, 1,
        "the seeded plan fits in one full page, got {pages} pages"
    );

    // The plan, in the lens's own order: the rows that carry a risk, each at that risk.
    let order = catalog();
    let mut expected: Vec<(String, String)> = Vec::new();
    for (_, question_id) in &order {
        if let Some(risk) = rows
            .iter()
            .find(|row| row.question_id == question_id.as_str())
            .and_then(|row| row.plan_risk)
        {
            expected.push((question_id.clone(), risk.to_string()));
        }
    }
    let actual = rated_questions(&summaries);
    assert_eq!(
        actual, expected,
        "the plan must report the identified risks in the lens's order, expected {expected:?}, got {actual:?}"
    );

    for row in &rows {
        let reported = actual
            .iter()
            .find(|(question_id, _)| question_id == row.question_id);
        match row.plan_risk {
            Some(risk) => assert_eq!(
                reported.map(|(_, reported_risk)| reported_risk.as_str()),
                Some(risk),
                "case {:?}: {} must be an improvement item at {risk}, got {reported:?}",
                row.case,
                row.question_id
            ),
            None => assert!(
                reported.is_none(),
                "case {:?}: {} leaves no identified risk, so it must not be an improvement item, got {reported:?}",
                row.case,
                row.question_id
            ),
        }
    }

    // Every reported summary describes the question the lens defines, within the modelled
    // bounds: QuestionId @length(1, 128), PillarId @length(1, 64), QuestionTitle
    // @length(1, 512) (`smithy_query(query="shape", shape="ImprovementSummary")`).
    for summary in &summaries {
        let question_id = string_member(summary, "QuestionId");
        let pillar = string_member(summary, "PillarId");
        let title = string_member(summary, "QuestionTitle");
        let expected_pillar = order
            .iter()
            .find(|(_, candidate)| *candidate == question_id)
            .map(|(pillar, _)| pillar.clone())
            .unwrap_or_default();
        assert_eq!(
            pillar, expected_pillar,
            "case {question_id:?}: expected PillarId {expected_pillar:?}, got {summary:?}"
        );
        assert!(
            (1..=128).contains(&question_id.len()),
            "case {question_id:?}: QuestionId must satisfy @length(1, 128), got {summary:?}"
        );
        assert!(
            (1..=64).contains(&pillar.len()),
            "case {question_id:?}: PillarId must satisfy @length(1, 64), got {summary:?}"
        );
        assert!(
            (1..=512).contains(&title.len()),
            "case {question_id:?}: QuestionTitle must satisfy @length(1, 512), got {summary:?}"
        );
        // `ImprovementPlans` "is only applicable to custom lenses"
        // (`smithy_query(query="shape", shape="ImprovementSummary")`), and this is an
        // AWS-official lens.
        assert_eq!(
            summary.get("ImprovementPlans"),
            None,
            "case {question_id:?}: a choice-level improvement plan applies only to a custom lens, got {summary:?}"
        );
        // No Jira integration has synced an issue for this question.
        assert_eq!(
            summary.get("JiraConfiguration"),
            None,
            "case {question_id:?}: nothing has synced a Jira issue here, got {summary:?}"
        );
    }

    // The envelope names the review that was read: the workload, the lens as the request
    // named it and the ARN it resolves to. No milestone was named, so none is reported.
    let answer = list_improvements(&client, &workload_id, FRAMEWORK_LENS, &[]).await;
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

    // The same review, named by the lens's ARN rather than its alias: the same plan, with
    // the alias echoed as the request spelled it.
    let by_arn = list_improvements(
        &client,
        &workload_id,
        &official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
        &[],
    )
    .await;
    assert_eq!(
        by_arn.status, 200,
        "case \"lens-by-arn\": the ARN names the same lens, got {by_arn:?}"
    );
    assert_eq!(
        rated_questions(&summaries_of(&by_arn)),
        expected,
        "case \"lens-by-arn\": the ARN must resolve to the same plan, got {by_arn:?}"
    );
    assert_eq!(
        member(&by_arn, "LensAlias"),
        official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS),
        "case \"lens-by-arn\": the envelope echoes the lens as the request named it, got {by_arn:?}"
    );
}

/// The plan is a SELECTION over the answers of the same review: for a question set that
/// mixes all four ratings, the plan is exactly the questions `ListAnswers` rates at an
/// identified risk, at the same risk and with the same question description.
#[tokio::test]
async fn the_plan_selects_the_answers_rated_at_an_identified_risk() {
    let client = Client::new();
    let (workload_id, seeded) = workload_with_spread_answers(&client, "spread-host").await;

    let expected: Vec<(String, String)> = seeded
        .iter()
        .filter(|(_, _, risk)| is_identified_risk(risk))
        .map(|(_, question_id, risk)| (question_id.clone(), risk.clone()))
        .collect();
    assert_eq!(
        expected.len(),
        FRAMEWORK_QUESTIONS / 2,
        "the seed must rate half the question set at an identified risk, got {}",
        expected.len()
    );

    let (summaries, _) =
        collect_improvements(&client, "spread", &workload_id, FRAMEWORK_LENS, &[], None).await;
    let actual = rated_questions(&summaries);
    assert_eq!(
        actual, expected,
        "the plan must be the questions the seed left at risk, expected {expected:?}, got {actual:?}"
    );

    // The same questions, read through `ListAnswers`: the risk and the question
    // description the two reads report cannot disagree.
    let answers = list_answers(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        &[format!("MaxResults={ANSWERS_MAX_RESULTS_CEILING}")],
    )
    .await;
    assert_eq!(
        answers.status, 200,
        "case \"answers\": ListAnswers must serve the same review, got {answers:?}"
    );
    let answer_summaries = answers
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for summary in &summaries {
        let question_id = string_member(summary, "QuestionId");
        let Some(answer_summary) = answer_summaries
            .iter()
            .find(|candidate| string_member(candidate, "QuestionId") == question_id)
        else {
            // The answers read is paged at its own ceiling, so only its first page is
            // available for comparison; a question it did not serve is not a mismatch.
            continue;
        };
        for field in ["Risk", "PillarId", "QuestionTitle"] {
            assert_eq!(
                string_member(summary, field),
                string_member(answer_summary, field),
                "case {question_id:?}: the plan and the answers must agree about {field}, got {summary:?} and {answer_summary:?}"
            );
        }
    }
}

/// `PillarId` and `QuestionPriority` FILTER the plan: each pillar reports its own
/// risk-bearing questions, a pillar the lens does not have reports nothing, and the
/// priority filter selects by the `QuestionType` the questions carry.
#[tokio::test]
async fn the_filters_narrow_the_improvement_plan() {
    let client = Client::new();
    let (workload_id, seeded) = workload_with_spread_answers(&client, "filter-host").await;

    let at_risk = |pillar: Option<&str>| -> usize {
        seeded
            .iter()
            .filter(|(seeded_pillar, _, risk)| {
                is_identified_risk(risk) && pillar.is_none_or(|wanted| wanted == seeded_pillar)
            })
            .count()
    };

    let mut cases: Vec<(String, Vec<String>, usize)> = Vec::new();
    for (pillar, _, _) in FRAMEWORK_PILLARS {
        cases.push((
            format!("pillar-{pillar}"),
            vec![format!("PillarId={pillar}")],
            at_risk(Some(pillar)),
        ));
    }
    // A pillar id the lens does not have matches no question; so does the right pillar in
    // the wrong case, because a `PillarId` is matched exactly.
    cases.push((
        "pillar-absent".to_string(),
        vec!["PillarId=notapillar".to_string()],
        0,
    ));
    cases.push((
        "pillar-wrong-case".to_string(),
        vec!["PillarId=SECURITY".to_string()],
        0,
    ));
    // `QuestionPriority` publishes PRIORITIZED | NONE (`smithy_query(query="shape",
    // shape="QuestionPriority")`). Every question here is NON_PRIORITIZED, so NONE selects
    // them all and PRIORITIZED none of them.
    cases.push((
        "priority-none".to_string(),
        vec!["QuestionPriority=NONE".to_string()],
        at_risk(None),
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
        at_risk(Some("sustainability")),
    ));
    cases.push((
        "pillar-and-prioritized".to_string(),
        vec![
            "PillarId=reliability".to_string(),
            "QuestionPriority=PRIORITIZED".to_string(),
        ],
        0,
    ));

    for (case, filters, expected) in cases {
        let (summaries, pages) = collect_improvements(
            &client,
            &case,
            &workload_id,
            FRAMEWORK_LENS,
            &filters,
            None,
        )
        .await;
        assert_eq!(
            summaries.len(),
            expected,
            "case {case:?}: expected {expected} improvement summaries, got {}",
            summaries.len()
        );
        assert_eq!(
            pages, 1,
            "case {case:?}: a request that omits MaxResults asks for a full page, got {pages}"
        );
        for summary in &summaries {
            let risk = string_member(summary, "Risk");
            assert!(
                is_identified_risk(&risk),
                "case {case:?}: only an identified risk is an improvement item, got {summary:?}"
            );
            if let Some(pillar) = filters
                .iter()
                .find_map(|filter| filter.strip_prefix("PillarId="))
            {
                assert_eq!(
                    string_member(summary, "PillarId"),
                    pillar,
                    "case {case:?}: only {pillar} questions may be reported, got {summary:?}"
                );
            }
        }
    }
}

/// Paging enumerates every improvement item exactly once, in the same order, whatever page
/// size the request asks for: checked EXHAUSTIVELY over this operation's published
/// `MaxResults` range, and at each bound and one step past it the model's own refusal
/// applies.
#[tokio::test]
async fn paging_enumerates_every_improvement_exactly_once() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "paging-host").await;
    // Every question answered at an identified risk, so the plan is the whole question
    // set: HIGH where nothing is in place, MEDIUM where something still is not.
    for (index, (_, question_id)) in catalog().into_iter().enumerate() {
        let (record, risk) = spread_answer(if index % 2 == 0 { 1 } else { 2 });
        assert!(
            is_identified_risk(risk),
            "case {question_id:?}: the seed must leave an identified risk, got {risk}"
        );
        client.record_answer(&workload_id, FRAMEWORK_LENS, &question_id, record);
    }

    let (whole, pages) = collect_improvements(
        &client,
        "unpaged",
        &workload_id,
        FRAMEWORK_LENS,
        &[],
        None,
    )
    .await;
    assert_eq!(
        whole.len(),
        FRAMEWORK_QUESTIONS,
        "every question is at risk, so the plan holds {FRAMEWORK_QUESTIONS} items, got {}",
        whole.len()
    );
    assert_eq!(
        pages, 1,
        "a request that omits MaxResults asks for this operation's full page, which holds \
         the whole plan, got {pages} pages"
    );
    let expected = rated_questions(&whole);

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    for max in minimum..=maximum {
        let case = format!("max-results-{max}");
        let (paged, pages) = collect_improvements(
            &client,
            &case,
            &workload_id,
            FRAMEWORK_LENS,
            &[],
            Some(max),
        )
        .await;
        assert_eq!(
            rated_questions(&paged),
            expected,
            "case {case:?}: paging must serve the same items in the same order"
        );
        let expected_pages = FRAMEWORK_QUESTIONS.div_ceil(usize::try_from(max).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {FRAMEWORK_QUESTIONS} items at {max} per page is {expected_pages} \
             pages, got {pages}"
        );
    }

    // This operation's ceiling is its own: a page size `ListAnswers` refuses is served
    // here, and the whole plan fits in ONE such page.
    let wide = list_improvements(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        &[format!("MaxResults={maximum}")],
    )
    .await;
    assert_eq!(
        wide.status, 200,
        "case \"at-the-maximum\": this operation allows up to {maximum}, got {wide:?}"
    );
    assert_eq!(
        summaries_of(&wide).len(),
        FRAMEWORK_QUESTIONS,
        "case \"at-the-maximum\": the whole plan fits in that page, got {wide:?}"
    );
    assert_eq!(
        wide.body.get("NextToken"),
        None,
        "case \"at-the-maximum\": nothing is left to serve, got {wide:?}"
    );
    for max in [ANSWERS_MAX_RESULTS_CEILING + 1, maximum] {
        let narrow = list_answers(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            &[format!("MaxResults={max}")],
        )
        .await;
        assert_eq!(
            narrow.status, 400,
            "case \"answers-above-its-ceiling\": ListAnswers publishes a @range ending at \
             {ANSWERS_MAX_RESULTS_CEILING}, so MaxResults={max} is refused, got {narrow:?}"
        );
    }

    // One step past each published bound is refused by the modelled @range.
    for (case, max) in [
        ("one-below-the-minimum", minimum - 1),
        ("one-above-the-maximum", maximum + 1),
    ] {
        let answer = list_improvements(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            &[format!("MaxResults={max}")],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: MaxResults={max} is outside @range({minimum}, {maximum}), got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // A token this service did not issue is a bad request, not an empty page. Rows differ
    // along how the token is malformed.
    for (case, token) in [
        ("not-a-cursor", "notatoken"),
        ("hex-but-not-a-cursor", "0a0b0c"),
        // A well-formed cursor naming a question the lens does not ask.
        (
            "unknown-question",
            "7b2271223a226e6f742d612d7175657374696f6e227d",
        ),
    ] {
        let answer = list_improvements(
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
}

/// The review whose plan is read must exist: the workload is resolved, the milestone is
/// resolved when one is named, and the lens must be associated with the workload. An
/// associated lens this emulator ships no question set for asks nothing, so its plan is
/// empty rather than missing.
#[tokio::test]
async fn a_review_that_does_not_exist_is_a_not_found() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "lookup-host").await;
    // A second workload carries only a lens this emulator ships no questions for.
    let mut unshipped = minimal_create("unshipped-lens-host", "tok-unshipped");
    unshipped["Lenses"] = json!([UNSHIPPED_LENS]);
    let unshipped_id = created_id(&client.create(unshipped).await, "unshipped");

    let cases: Vec<MissingReview> = vec![
        MissingReview {
            case: "absent-workload",
            workload_id: ABSENT_HEX_ID.to_string(),
            lens: FRAMEWORK_LENS,
            filters: Vec::new(),
            resource_type: "WORKLOAD",
            resource_id: ABSENT_HEX_ID.to_string(),
        },
        MissingReview {
            case: "lens-not-associated",
            workload_id: workload_id.clone(),
            lens: UNSHIPPED_LENS,
            filters: Vec::new(),
            resource_type: "LENS_REVIEW",
            resource_id: format!("{workload_id}.{UNSHIPPED_LENS}"),
        },
        MissingReview {
            case: "framework-lens-not-associated",
            workload_id: unshipped_id.clone(),
            lens: FRAMEWORK_LENS,
            filters: Vec::new(),
            resource_type: "LENS_REVIEW",
            resource_id: format!("{unshipped_id}.{FRAMEWORK_LENS}"),
        },
        MissingReview {
            case: "milestone-not-recorded",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            filters: vec!["MilestoneNumber=1".to_string()],
            resource_type: "MILESTONE",
            resource_id: format!("{workload_id}.1"),
        },
        MissingReview {
            case: "milestone-at-the-range-maximum",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            filters: vec!["MilestoneNumber=100".to_string()],
            resource_type: "MILESTONE",
            resource_id: format!("{workload_id}.100"),
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
        } = row;
        let answer = list_improvements(&client, &host, lens, &filters).await;
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
    }

    // The associated lens with no question set here: a review with no questions to
    // improve, not a missing review.
    let empty = list_improvements(&client, &unshipped_id, UNSHIPPED_LENS, &[]).await;
    assert_eq!(
        empty.status, 200,
        "case \"associated-unshipped-lens\": the review exists, got {empty:?}"
    );
    assert_eq!(
        empty.body.get("ImprovementSummaries"),
        Some(&json!([])),
        "case \"associated-unshipped-lens\": the lens asks no questions here, got {empty:?}"
    );
    assert_eq!(
        empty.body.get("NextToken"),
        None,
        "case \"associated-unshipped-lens\": an empty plan has no next page, got {empty:?}"
    );

    // A review whose questions are all unanswered leaves nothing to improve either.
    let unanswered = list_improvements(&client, &workload_id, FRAMEWORK_LENS, &[]).await;
    assert_eq!(
        unanswered.body.get("ImprovementSummaries"),
        Some(&json!([])),
        "case \"nothing-answered\": an unanswered review identifies no risk, got {unanswered:?}"
    );
}

/// A milestone records the workload at an instant, answers included: a plan scoped to a
/// milestone reports the improvements as they were, while the live plan moves on. Rows
/// differ along what the later change does to the question.
#[tokio::test]
async fn a_milestone_freezes_the_improvement_plan_it_recorded() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "frozen-plan-host").await;

    // Before the milestone: one question at HIGH risk, nothing else answered.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "rel-01",
        answered(&[], &[("bp-1", "UNSELECTED")], true, None),
    );
    let recorded = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({
                "MilestoneName": "before-the-improvements",
                "ClientRequestToken": "tok-frozen-plan",
            })),
        )
        .await;
    assert_eq!(
        recorded.status, 200,
        "case \"record\": CreateMilestone must succeed, got {recorded:?}"
    );

    // After it: that question is fully answered (so it leaves the plan) and another is
    // answered with something still outstanding (so it joins the plan).
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "rel-01",
        answered(&["bp-1"], &[("bp-1", "SELECTED")], true, None),
    );
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "cost-02",
        answered(
            &["bp-1"],
            &[("bp-1", "SELECTED"), ("bp-2", "UNSELECTED")],
            true,
            None,
        ),
    );

    for (case, filters, expected) in [
        ("live", Vec::new(), vec![("cost-02", "MEDIUM")]),
        (
            "milestone",
            vec!["MilestoneNumber=1".to_string()],
            vec![("rel-01", "HIGH")],
        ),
    ] {
        let (summaries, _) = collect_improvements(
            &client,
            case,
            &workload_id,
            FRAMEWORK_LENS,
            &filters,
            None,
        )
        .await;
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(question_id, risk)| (question_id.to_string(), risk.to_string()))
            .collect();
        let actual = rated_questions(&summaries);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the plan {expected:?}, got {actual:?}"
        );
    }

    // A milestone-scoped read names the milestone it was scoped to.
    let scoped = list_improvements(
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
}

/// Reading the plan changes nothing a later caller sees: the same request, repeated and
/// paged differently, keeps answering the same plan, and the answers it is derived from
/// are untouched.
#[tokio::test]
async fn reading_the_plan_does_not_change_it() {
    let client = Client::new();
    let (workload_id, _) = workload_with_spread_answers(&client, "idempotent-host").await;

    let (first, _) =
        collect_improvements(&client, "first", &workload_id, FRAMEWORK_LENS, &[], None).await;
    let baseline = rated_questions(&first);

    for (case, max_results) in [("repeat", None), ("paged-small", Some(3)), ("paged-one", Some(1))]
    {
        let (again, _) = collect_improvements(
            &client,
            case,
            &workload_id,
            FRAMEWORK_LENS,
            &[],
            max_results,
        )
        .await;
        let actual = rated_questions(&again);
        assert_eq!(
            actual, baseline,
            "case {case:?}: a read must not change the plan, expected {baseline:?}, got {actual:?}"
        );
    }
}

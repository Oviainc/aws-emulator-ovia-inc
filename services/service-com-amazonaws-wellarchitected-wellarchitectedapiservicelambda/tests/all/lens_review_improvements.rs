// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListLensReviewImprovements`: the improvement plan of ONE
//! lens review of ONE workload.
//!
//! The rule these pin, over tables of inputs rather than one recorded request: the
//! improvement plan of a lens review reports one `ImprovementSummary` per question of
//! that lens whose stored answer the review rates at an IDENTIFIED risk — `HIGH` or
//! `MEDIUM` — in the order the WORKLOAD's `PillarPriorities` put their pillars in (and,
//! inside one pillar, in the lens's own order), narrowed by the request's `PillarId` /
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

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::{
    self, AnswerRecord,
};

use super::workload_harness::{
    assert_field_validation, created_id, encode_path_segment, framework_answer,
    framework_answer_that_does_not_apply, framework_choice_ids, minimal_create, official_lens_arn,
    stored_workload, Answer, Client, InPlace,
};

/// The AWS Well-Architected Framework lens's pillars: the `PillarId` each is named by on
/// the wire and the number of questions it asks (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5,
/// COST 1-11, SUS 1-6). Spelled out here — in the order the framework presents them — so
/// the tests pin the framework's published numbering rather than reading it back out of
/// the code under test.
const FRAMEWORK_PILLARS: [(&str, usize); 6] = [
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
///
/// The framework lens publishes its own `QuestionId`s, so they are read from the lens's
/// content (the question ids themselves are pinned over the wire by
/// `super::question_choice_catalog`); what this suite pins independently is the framework's
/// published numbering — the pillar order and the number of questions each pillar asks
/// ([`FRAMEWORK_PILLARS`]), which this asserts the content against.
fn catalog() -> Vec<(String, String)> {
    let questions: Vec<(String, String)> = state::FRAMEWORK_LENS_QUESTIONS
        .iter()
        .map(|question| {
            (
                question.pillar_id.to_string(),
                question.question_id.to_string(),
            )
        })
        .collect();
    let mut expected: Vec<&str> = Vec::new();
    for (pillar, count) in FRAMEWORK_PILLARS {
        for _ in 0..count {
            expected.push(pillar);
        }
    }
    let pillars: Vec<&str> = questions
        .iter()
        .map(|(pillar, _)| pillar.as_str())
        .collect();
    assert_eq!(
        pillars, expected,
        "the lens must ask the framework's published numbering, in its pillar order"
    );
    questions
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
fn spread_answer(question_id: &str, index: usize) -> (AnswerRecord, &'static str) {
    match index % 4 {
        // Nothing left out: no risk.
        0 => (framework_answer(question_id, InPlace::Every), "NONE"),
        // No best practice in place at all: the last choice is recorded NOT_APPLICABLE, so
        // the answer speaks about the question and is rated, with nothing in place.
        1 => (
            framework_answer(question_id, InPlace::NothingAndTheLastDoesNotApply),
            "HIGH",
        ),
        // Some in place, one still out — unless the question offers a single best
        // practice, where "all but the last" would select nothing and leave the question
        // unanswered, so that one records none in place with its one choice not applying.
        2 => {
            if framework_choice_ids(question_id).len() > 1 {
                (framework_answer(question_id, InPlace::AllButLast), "MEDIUM")
            } else {
                (
                    framework_answer(question_id, InPlace::NothingAndTheLastDoesNotApply),
                    "HIGH",
                )
            }
        }
        // The question does not apply to this workload.
        _ => (
            framework_answer_that_does_not_apply(question_id, "OUT_OF_SCOPE"),
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
    let seeded = seed_spread_answers(client, &workload_id);
    (workload_id, seeded)
}

/// Answer every question of the framework lens review of one STORED workload, cycling
/// through the four ratings, and answer the `(PillarId, QuestionId, Risk)` the seeding
/// stored — in the LENS's own order, so a test states its expectation from the seed rather
/// than from the code under test. Split from [`workload_with_spread_answers`] so a case
/// that creates its workload itself (one that names its own `PillarPriorities`, say) seeds
/// the same answers.
fn seed_spread_answers(client: &Client, workload_id: &str) -> Vec<(String, String, String)> {
    let mut seeded: Vec<(String, String, String)> = Vec::new();
    for (index, (pillar, question_id)) in catalog().into_iter().enumerate() {
        let (record, risk) = spread_answer(&question_id, index);
        client.record_answer(workload_id, FRAMEWORK_LENS, &question_id, record);
        seeded.push((pillar, question_id, risk.to_string()));
    }
    seeded
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
            question_id: "organization-structure",
            record: Some(framework_answer(
                "organization-structure",
                InPlace::NothingAndTheLastDoesNotApply,
            )),
            plan_risk: Some("HIGH"),
        },
        Rated {
            case: "one-best-practice-missing",
            question_id: "permissions",
            record: Some(framework_answer("permissions", InPlace::AllButLast)),
            plan_risk: Some("MEDIUM"),
        },
        Rated {
            case: "every-best-practice-in-place",
            question_id: "prevent-interaction-failure",
            record: Some(framework_answer("prevent-interaction-failure", InPlace::Every)),
            plan_risk: None,
        },
        Rated {
            case: "unselected-choice-does-not-apply",
            question_id: "compute-architecture-selection",
            record: Some(framework_answer(
                "compute-architecture-selection",
                InPlace::AllButLastWhichDoesNotApply,
            )),
            plan_risk: None,
        },
        Rated {
            case: "question-does-not-apply",
            question_id: "resource-type-size-number",
            record: Some(framework_answer_that_does_not_apply(
                "resource-type-size-number",
                "OUT_OF_SCOPE",
            )),
            plan_risk: None,
        },
        Rated {
            case: "nothing-selected-at-all",
            question_id: "test-reliability",
            record: Some(framework_answer("test-reliability", InPlace::Nothing)),
            plan_risk: None,
        },
        Rated {
            case: "no-stored-answer",
            question_id: "software-architecture",
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

    // The plan, in the lens's own order: this workload named no `PillarPriorities`, so it
    // is reviewed in the framework's own pillar order — the order its question catalog is
    // already in. The rows that carry a risk, each at that risk.
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

/// `PillarId` SELECTS the plan of one pillar of the lens and `QuestionPriority` filters by
/// the `QuestionType` the questions carry: each pillar reports its own risk-bearing
/// questions, and a `PillarId` the lens does not define is refused rather than selecting
/// nothing — the pillars are the lens's own content.
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
    for (pillar, _) in FRAMEWORK_PILLARS {
        cases.push((
            format!("pillar-{pillar}"),
            vec![format!("PillarId={pillar}")],
            at_risk(Some(pillar)),
        ));
    }
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

    // A `PillarId` the lens does not define is refused rather than selecting nothing —
    // including the right pillar in the wrong case, because a `PillarId` is matched
    // exactly.
    for (case, pillar) in [
        ("pillar-absent", "notapillar"),
        ("pillar-wrong-case", "SECURITY"),
    ] {
        let answer = list_improvements(
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
        let (record, risk) = spread_answer(&question_id, if index % 2 == 0 { 1 } else { 2 });
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

/// The review whose plan is read must exist: the workload is RESOLVED — so an id nothing
/// is stored under, and a milestone the workload never recorded, are each the modelled 404
/// — while the lens is a VALUE of the request, so a lens the workload does not review is
/// the modelled 400 naming `LensAlias`. An
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

    // A lens the workload does not review names no review of it, whichever workload of the
    // two is asked and whichever lens it is: the value is refused as input.
    for (case, host, lens) in [
        ("lens-not-associated", &workload_id, UNSHIPPED_LENS),
        ("framework-lens-not-associated", &unshipped_id, FRAMEWORK_LENS),
    ] {
        let answer = list_improvements(&client, host, lens, &[]).await;
        assert_field_validation(
            case,
            &answer,
            "LensAlias",
            &format!("No lens review for lens alias {lens} was found in workload {host}."),
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

    // Before the milestone: one question at HIGH risk (its last best practice recorded as
    // not applicable, with none in place), nothing else answered.
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "manage-service-quotas",
        framework_answer(
            "manage-service-quotas",
            InPlace::NothingAndTheLastDoesNotApply,
        ),
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
        "manage-service-quotas",
        framework_answer("manage-service-quotas", InPlace::Every),
    );
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "govern-usage",
        framework_answer("govern-usage", InPlace::AllButLast),
    );

    for (case, filters, expected) in [
        ("live", Vec::new(), vec![("govern-usage", "MEDIUM")]),
        (
            "milestone",
            vec!["MilestoneNumber=1".to_string()],
            vec![("manage-service-quotas", "HIGH")],
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

/// Where a pillar sits in a workload's `PillarPriorities`: its position in that list, and
/// behind every pillar the list names when it names this one nowhere. "The priorities of
/// the pillars, which are used to order items in the improvement plan. Each pillar is
/// represented by its PillarReviewSummary$PillarId" (`smithy_query(query="doc",
/// shape="WorkloadPillarPriorities")`) — a pillar nothing prioritised is not prioritised
/// ahead of one that was.
fn priority_rank(priorities: &[&str], pillar_id: &str) -> usize {
    priorities
        .iter()
        .position(|priority| *priority == pillar_id)
        .unwrap_or(priorities.len())
}

/// The plan a workload whose priorities are `priorities` must serve, given what the seed
/// stored: the questions left at an identified risk, ordered by their pillar's priority
/// rank. The sort is STABLE and `seeded` is in the lens's own order, so that order is what
/// breaks every tie — including the tie between two pillars the priorities do not name.
fn expected_plan(
    priorities: &[&str],
    seeded: &[(String, String, String)],
) -> Vec<(String, String)> {
    let mut at_risk: Vec<&(String, String, String)> = seeded
        .iter()
        .filter(|(_, _, risk)| is_identified_risk(risk))
        .collect();
    at_risk.sort_by_key(|(pillar_id, _, _)| priority_rank(priorities, pillar_id));
    at_risk
        .iter()
        .map(|(_, question_id, risk)| (question_id.clone(), risk.clone()))
        .collect()
}

/// The pillars one plan serves, in the order it first served each of them.
fn served_pillars(summaries: &[Value]) -> Vec<String> {
    let mut pillars: Vec<String> = Vec::new();
    for summary in summaries {
        let pillar = string_member(summary, "PillarId");
        if !pillars.contains(&pillar) {
            pillars.push(pillar);
        }
    }
    pillars
}

/// One row of the priority table: the `PillarPriorities` the workload is created with, the
/// list a later `UpdateWorkload` sets (none for a row that updates nothing), and the
/// priorities the workload then holds — what `GetWorkload` must report and what the plan
/// must be ordered by.
struct Priorities {
    case: &'static str,
    created_with: Option<Vec<&'static str>>,
    updated_to: Option<Vec<&'static str>>,
    priorities: Vec<&'static str>,
}

/// What one row of the priority table SERVED: the case, the priorities the workload held
/// and the plan it answered with, kept so the rows can be compared against each other.
struct Served {
    case: &'static str,
    priorities: Vec<&'static str>,
    plan: Vec<(String, String)>,
}

/// The workload's own `PillarPriorities` order the improvement plan: the plan serves each
/// prioritised pillar's risk-bearing questions in turn, in the lens's own order inside one
/// pillar, with the pillars those priorities do not name behind every pillar they do. Rows
/// differ along how the workload came by its priorities and along the list itself — its
/// length, its order (the framework's own, reversed, and rotated), whether it names every
/// pillar or leaves some unranked, the same pillar twice, or nothing at all (which leaves
/// the order the workload already has).
#[tokio::test]
async fn the_workloads_pillar_priorities_order_the_improvement_plan() {
    // The framework's own pillar order, which is what a workload that names no priorities
    // is created with, read off this suite's catalog table rather than the code under test.
    let framework_order: Vec<&str> = FRAMEWORK_PILLARS
        .iter()
        .map(|(pillar_id, _)| *pillar_id)
        .collect();
    // The framework's own order rotated, so the row that names it ranks every pillar
    // differently from the rows that name the order itself or its reverse.
    let rotated_order: Vec<&str> = {
        let mut rotated = framework_order.clone();
        rotated.rotate_left(2);
        rotated
    };

    let rows: Vec<Priorities> = vec![
        Priorities {
            case: "named-none",
            created_with: None,
            updated_to: None,
            priorities: framework_order.clone(),
        },
        Priorities {
            case: "every-pillar-reversed",
            created_with: Some(framework_order.iter().rev().copied().collect()),
            updated_to: None,
            priorities: framework_order.iter().rev().copied().collect(),
        },
        Priorities {
            case: "two-pillars-first",
            created_with: Some(vec!["sustainability", "security"]),
            updated_to: None,
            priorities: vec!["sustainability", "security"],
        },
        Priorities {
            case: "one-pillar-first",
            created_with: Some(vec!["costOptimization"]),
            updated_to: None,
            priorities: vec!["costOptimization"],
        },
        Priorities {
            // This row used to name a pillar no lens defines. An order whose entry resolves
            // to no pillar of the workload never reaches a stored workload now — both
            // operations that write the member refuse it, which
            // `super::workload_pillar_identity` pins — so the row varies the ORDER the plan
            // is served in instead.
            case: "every-pillar-rotated",
            created_with: Some(rotated_order.clone()),
            updated_to: None,
            priorities: rotated_order.clone(),
        },
        Priorities {
            case: "same-pillar-twice",
            created_with: Some(vec!["reliability", "reliability", "operationalExcellence"]),
            updated_to: None,
            priorities: vec!["reliability", "reliability", "operationalExcellence"],
        },
        Priorities {
            case: "updated",
            created_with: Some(vec!["sustainability"]),
            updated_to: Some(vec!["performance", "sustainability"]),
            priorities: vec!["performance", "sustainability"],
        },
        Priorities {
            // An update that names no priority order leaves the workload in the order it is
            // already reviewed in (`crate::state::resolved_pillar_priorities`), so the plan
            // keeps the order the create set.
            case: "updated-to-name-none",
            created_with: Some(vec!["security"]),
            updated_to: Some(Vec::new()),
            priorities: vec!["security"],
        },
    ];

    // Every row seeds the SAME answers, so the plans they serve hold the same items and can
    // differ only in their order.
    let mut served: Vec<Served> = Vec::new();

    for row in &rows {
        let case = row.case;
        let client = Client::new();
        let mut request = minimal_create(&format!("priority-{case}"), &format!("tok-{case}"));
        if let Some(created_with) = &row.created_with {
            request["PillarPriorities"] = json!(created_with);
        }
        let workload_id = created_id(&client.create(request).await, case);
        let seeded = seed_spread_answers(&client, &workload_id);

        if let Some(updated_to) = &row.updated_to {
            let updated = client
                .update(&workload_id, json!({ "PillarPriorities": updated_to }))
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: UpdateWorkload must set the priorities, got {updated:?}"
            );
        }

        // Read the priorities back through `GetWorkload`: the plan is ordered by what the
        // workload holds, so the test and the service must agree about that first.
        let workload = stored_workload(&client, &workload_id, case).await;
        assert_eq!(
            workload.get("PillarPriorities"),
            Some(&json!(row.priorities)),
            "case {case:?}: expected PillarPriorities {:?}, got {workload:?}",
            row.priorities
        );

        let (summaries, _) =
            collect_improvements(&client, case, &workload_id, FRAMEWORK_LENS, &[], None).await;
        let plan = rated_questions(&summaries);
        let expected = expected_plan(&row.priorities, &seeded);
        assert_eq!(
            plan, expected,
            "case {case:?}: the plan must be ordered by the workload's pillar priorities \
             {:?}, expected {expected:?}, got {plan:?}",
            row.priorities
        );

        // The priorities ORDER the plan; they select nothing. Compared as sets, every row
        // serves the questions the seed left at an identified risk.
        let mut sorted_plan = plan.clone();
        sorted_plan.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_plan, sorted_expected,
            "case {case:?}: the priorities must not change WHICH questions the plan holds"
        );

        // Each pillar's items are served together, and the pillars come in priority rank
        // order: a prioritised pillar ahead of a less prioritised one, and both ahead of a
        // pillar the priorities do not name.
        let pillars = served_pillars(&summaries);
        let mut ranked = pillars.clone();
        ranked.sort_by_key(|pillar_id| priority_rank(&row.priorities, pillar_id));
        assert_eq!(
            pillars, ranked,
            "case {case:?}: the plan must serve the pillars in priority rank order, got {pillars:?}"
        );
        assert_eq!(
            pillars.len(),
            FRAMEWORK_PILLARS.len(),
            "case {case:?}: the seed leaves a risk in every pillar, so each must be served \
             exactly once, got {pillars:?}"
        );

        // Inside one pillar the lens's own order stands, and the `PillarId` filter serves
        // exactly that pillar's slice of the plan.
        for (pillar_id, _) in FRAMEWORK_PILLARS {
            let in_plan: Vec<(String, String)> = summaries
                .iter()
                .filter(|summary| string_member(summary, "PillarId") == pillar_id)
                .map(|summary| {
                    (
                        string_member(summary, "QuestionId"),
                        string_member(summary, "Risk"),
                    )
                })
                .collect();
            let lens_order: Vec<(String, String)> = seeded
                .iter()
                .filter(|(seeded_pillar, _, risk)| {
                    seeded_pillar == pillar_id && is_identified_risk(risk)
                })
                .map(|(_, question_id, risk)| (question_id.clone(), risk.clone()))
                .collect();
            assert_eq!(
                in_plan, lens_order,
                "case {case:?}: inside {pillar_id} the lens's own order stands, expected \
                 {lens_order:?}, got {in_plan:?}"
            );
            let (filtered, _) = collect_improvements(
                &client,
                &format!("{case}/{pillar_id}"),
                &workload_id,
                FRAMEWORK_LENS,
                &[format!("PillarId={pillar_id}")],
                None,
            )
            .await;
            assert_eq!(
                rated_questions(&filtered),
                in_plan,
                "case {case:?}: a read filtered to {pillar_id} must serve that pillar's slice \
                 of the plan"
            );
        }

        // Paging serves the same order, whatever page size the request asks for: the
        // cursor names a position in the plan's order, not in the lens's.
        for page_size in [1, 7] {
            let (paged, pages) = collect_improvements(
                &client,
                &format!("{case}/paged-{page_size}"),
                &workload_id,
                FRAMEWORK_LENS,
                &[],
                Some(page_size),
            )
            .await;
            assert_eq!(
                rated_questions(&paged),
                expected,
                "case {case:?}: paging at {page_size} per page must serve the plan in the same \
                 order, in {pages} pages"
            );
        }

        served.push(Served {
            case,
            priorities: row.priorities.clone(),
            plan,
        });
    }

    // Two workloads that differ only in their priorities serve the same items in a
    // DIFFERENT order whenever those priorities rank the pillars differently — and in the
    // SAME order when they rank them the same: the member the model says exists in order to
    // order this list orders it.
    let ranked_pillars = |priorities: &[&str]| -> Vec<String> {
        let mut pillars: Vec<String> = FRAMEWORK_PILLARS
            .iter()
            .map(|(pillar_id, _)| (*pillar_id).to_string())
            .collect();
        pillars.sort_by_key(|pillar_id| priority_rank(priorities, pillar_id));
        pillars
    };
    for (index, row) in served.iter().enumerate() {
        let case = row.case;
        for other in served.iter().take(index) {
            let other_case = other.case;
            let mut items = row.plan.clone();
            items.sort();
            let mut other_items = other.plan.clone();
            other_items.sort();
            assert_eq!(
                items, other_items,
                "case {case:?} vs {other_case:?}: every row seeds the same answers, so the \
                 plans must hold the same items"
            );
            if ranked_pillars(&row.priorities) == ranked_pillars(&other.priorities) {
                assert_eq!(
                    row.plan, other.plan,
                    "case {case:?} vs {other_case:?}: these priorities rank the pillars the \
                     same, so the plans must agree"
                );
            } else {
                assert_ne!(
                    row.plan, other.plan,
                    "case {case:?} vs {other_case:?}: these priorities rank the pillars \
                     differently, so the plans must differ"
                );
            }
        }
    }
}

/// A milestone records the workload at an instant, its `PillarPriorities` included: the
/// plan it reports is ordered by the priorities it froze, while the live plan follows the
/// priorities the workload now holds.
#[tokio::test]
async fn a_milestone_orders_its_plan_by_the_priorities_it_recorded() {
    let client = Client::new();
    let recorded_priorities = vec!["sustainability", "operationalExcellence"];
    let live_priorities = vec!["operationalExcellence", "sustainability"];

    let mut request = minimal_create("frozen-priorities-host", "tok-frozen-priorities");
    request["PillarPriorities"] = json!(recorded_priorities);
    let workload_id = created_id(&client.create(request).await, "recorded");
    let seeded = seed_spread_answers(&client, &workload_id);

    let recorded = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({
                "MilestoneName": "before-the-reprioritisation",
                "ClientRequestToken": "tok-frozen-priorities-milestone",
            })),
        )
        .await;
    assert_eq!(
        recorded.status, 200,
        "case \"record\": CreateMilestone must succeed, got {recorded:?}"
    );

    let updated = client
        .update(&workload_id, json!({ "PillarPriorities": live_priorities }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"reprioritise\": UpdateWorkload must set the priorities, got {updated:?}"
    );

    for (case, filters, priorities) in [
        ("live", Vec::new(), &live_priorities),
        (
            "milestone",
            vec!["MilestoneNumber=1".to_string()],
            &recorded_priorities,
        ),
    ] {
        let (summaries, _) =
            collect_improvements(&client, case, &workload_id, FRAMEWORK_LENS, &filters, None).await;
        let expected = expected_plan(priorities, &seeded);
        let actual = rated_questions(&summaries);
        assert_eq!(
            actual, expected,
            "case {case:?}: the plan must be ordered by {priorities:?}, expected {expected:?}, \
             got {actual:?}"
        );
        assert_eq!(
            served_pillars(&summaries).first().map(String::as_str),
            priorities.first().copied(),
            "case {case:?}: the most prioritised pillar must be served first, got {summaries:?}"
        );
    }
}

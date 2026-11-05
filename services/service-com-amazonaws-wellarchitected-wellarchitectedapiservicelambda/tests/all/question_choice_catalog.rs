// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the per-question CHOICE CATALOG an answer summary reports.
//!
//! The rule these pin, over TABLES of inputs rather than one recorded request: every
//! projection of a question of a lens this emulator ships content for reports that
//! question's whole choice catalog — one `Choice` per best practice the question asks
//! about, each with a `ChoiceId` unique within the question and a `Title`, in the lens's
//! own order — independently of the answer recorded for the question, of which read
//! serves it, and of the filter or page it came through. Those `ChoiceId`s are the only
//! way a client can learn the values `SelectedChoices` names, so a discovered id has to
//! round-trip through the stored answer.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::workload_harness::{
    answered_over, created_id, encode_path_segment, minimal_create, Answer, Client,
};

/// The alias of the one lens this emulator ships a question set — and therefore a choice
/// catalog — for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// How many questions that lens asks in all: OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5,
/// COST 1-11, SUS 1-6, the framework's published numbering.
const FRAMEWORK_QUESTIONS: usize = 11 + 11 + 13 + 5 + 11 + 6;

/// `ChoiceId` is `@length(min: 1, max: 64)` and `Choice$Title` `@length(min: 1, max:
/// 512)` (`smithy_query(query="shape", shape="Choice")`).
const CHOICE_ID_RANGE: (usize, usize) = (1, 64);
const CHOICE_TITLE_RANGE: (usize, usize) = (1, 512);

/// `ChoiceStatus` members, from `smithy_query(query="shape", shape="ChoiceStatus")`.
const SELECTED: &str = "SELECTED";
const UNSELECTED: &str = "UNSELECTED";

/// One question's choice catalog as the wire reports it: `(ChoiceId, Title)` in the
/// order the lens offers the choices.
type Catalog = Vec<(String, String)>;

/// The catalog of every question one read reported, keyed by `QuestionId`.
type Catalogs = BTreeMap<String, Catalog>;

/// One row of the answer table: the case name, the `ChoiceId`s the answer records as in
/// place, and the `Risk` that answer carries. The status of every choice of the question
/// follows from the first: the choices named are `SELECTED` and every other choice of the
/// question `UNSELECTED`, which is the key space those statuses live in.
type AnswerRow = (&'static str, Vec<String>, &'static str);

/// `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers`. The query members
/// arrive owned, so the future this builds borrows nothing from its caller's page loop.
async fn list_answers(
    client: &Client,
    workload_id: &str,
    lens: &str,
    query: Vec<String>,
) -> Answer {
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

/// `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers`: both labels are
/// non-greedy, so each travels percent-encoded in one path segment.
async fn list_template_answers(
    client: &Client,
    template_arn: &str,
    lens: &str,
    query: Vec<String>,
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

/// Every answer summary of one lens review, following `NextToken` to the end. `read`
/// serves one page for the query it is given, so one collector covers both list
/// operations and every page size.
async fn collect_summaries<Read, Fut>(
    case: &str,
    filters: &[String],
    max_results: Option<i64>,
    read: Read,
) -> Vec<Value>
where
    Read: Fn(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = Answer>,
{
    let mut collected: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = filters.to_vec();
        if let Some(max) = max_results {
            query.push(format!("MaxResults={max}"));
        }
        if let Some(token) = &next_token {
            query.push(format!("NextToken={token}"));
        }
        let answer = read(query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: page {pages} must be a 200, got {answer:?}"
        );
        collected.extend(summaries_of(&answer));
        pages += 1;
        assert!(
            pages <= FRAMEWORK_QUESTIONS + 1,
            "case {case:?}: paging must terminate, got {pages} pages"
        );
        next_token = answer
            .body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    collected
}

/// The `(ChoiceId, Title)` pairs one summary reports, in the order the summary reports
/// them. Asserts the member is PRESENT: an absent `Choices` is the defect this suite
/// pins, so it fails here rather than silently reading an empty catalog.
fn catalog_of(case: &str, summary: &Value) -> Catalog {
    let choices = summary
        .get("Choices")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: every answer summary must carry Choices, got {summary:?}")
        });
    assert!(
        !choices.is_empty(),
        "case {case:?}: a question's choice catalog must not be empty, got {summary:?}"
    );
    choices
        .iter()
        .map(|choice| {
            (
                choice
                    .get("ChoiceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                choice
                    .get("Title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The catalog of every question one read reports, keyed by `QuestionId`.
fn catalogs_of(case: &str, summaries: &[Value]) -> Catalogs {
    summaries
        .iter()
        .map(|summary| {
            let question_id = summary
                .get("QuestionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let catalog = catalog_of(&format!("{case}/{question_id}"), summary);
            (question_id, catalog)
        })
        .collect()
}

/// Create a workload reviewing the framework lens.
async fn framework_workload(client: &Client, name: &str) -> String {
    created_id(
        &client.create(minimal_create(name, &format!("tok-{name}"))).await,
        name,
    )
}

/// Create a review template applying the framework lens and answer its `TemplateArn`.
async fn framework_template(client: &Client, name: &str) -> String {
    let answer = client
        .create_review_template(json!({
            "TemplateName": name,
            "Description": "a review template under test",
            "Lenses": [FRAMEWORK_LENS],
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

/// Record a milestone of `workload_id` and answer its `MilestoneNumber`.
async fn milestone(client: &Client, workload_id: &str, name: &str) -> i64 {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": format!("tok-{name}") })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateMilestone 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// Every question of a shipped lens carries a well-formed choice catalog, on every read
/// that projects a question and whatever the question's answer is. The rows differ along
/// which read serves the summary (the live lens review, the same review frozen in a
/// milestone, and a review template's), and each read is paged with a different
/// `MaxResults` so a catalog is checked on a first, a middle and a last page.
#[tokio::test]
async fn every_answer_summary_carries_the_questions_choice_catalog() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "choices-host").await;
    let template_arn = framework_template(&client, "choices-template").await;
    let frozen = milestone(&client, &workload_id, "choices-milestone").await;

    let mut by_read: Vec<(&str, Catalogs)> = Vec::new();

    // Row 1: the live lens review, one question per page.
    let case = "list-answers-live";
    let summaries = collect_summaries(case, &[], Some(1), |query| {
        list_answers(&client, &workload_id, FRAMEWORK_LENS, query)
    })
    .await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "case {case:?}: the lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );
    by_read.push((case, catalogs_of(case, &summaries)));

    // Row 2: the same review as the milestone recorded it — a catalog is lens content, so
    // freezing the workload does not change it.
    let case = "list-answers-milestone";
    let summaries = collect_summaries(
        case,
        &[format!("MilestoneNumber={frozen}")],
        Some(7),
        |query| list_answers(&client, &workload_id, FRAMEWORK_LENS, query),
    )
    .await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "case {case:?}: the lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );
    by_read.push((case, catalogs_of(case, &summaries)));

    // Row 3: a review template's lens review, the modelled maximum page.
    let case = "list-review-template-answers";
    let summaries = collect_summaries(case, &[], Some(50), |query| {
        list_template_answers(&client, &template_arn, FRAMEWORK_LENS, query)
    })
    .await;
    assert_eq!(
        summaries.len(),
        FRAMEWORK_QUESTIONS,
        "case {case:?}: the lens asks {FRAMEWORK_QUESTIONS} questions, got {}",
        summaries.len()
    );
    by_read.push((case, catalogs_of(case, &summaries)));

    for (case, catalogs) in &by_read {
        for (question_id, catalog) in catalogs {
            let ids: Vec<&str> = catalog.iter().map(|(id, _)| id.as_str()).collect();
            let unique: BTreeSet<&&str> = ids.iter().collect();
            assert_eq!(
                unique.len(),
                ids.len(),
                "case {case:?}/{question_id:?}: a ChoiceId must be unique within its \
                 question, got {ids:?}"
            );
            for (choice_id, title) in catalog {
                // The framework lens publishes its own `ChoiceId` per best practice, so
                // the id is the lens's rather than derived from the question; what every
                // read has to agree on is the id, its uniqueness within the question and
                // the modelled bounds. The published ids themselves are pinned against the
                // lens's content by `super::lens_catalog_identity`.
                assert!(
                    (CHOICE_ID_RANGE.0..=CHOICE_ID_RANGE.1).contains(&choice_id.len()),
                    "case {case:?}/{question_id:?}: ChoiceId is @length{CHOICE_ID_RANGE:?}, \
                     got {choice_id:?} of {} character(s)",
                    choice_id.len()
                );
                assert!(
                    (CHOICE_TITLE_RANGE.0..=CHOICE_TITLE_RANGE.1).contains(&title.len()),
                    "case {case:?}/{question_id:?}: Choice$Title is \
                     @length{CHOICE_TITLE_RANGE:?}, got {title:?} of {} character(s)",
                    title.len()
                );
            }
        }
    }

    // A catalog describes the QUESTION, so the three reads must report the same one for
    // the same question id.
    let (first_case, first) = &by_read[0];
    for (case, catalogs) in &by_read[1..] {
        assert_eq!(
            catalogs, first,
            "case {case:?}: must report the same choice catalog per question as \
             {first_case:?}"
        );
    }
}

/// A `ChoiceId` discovered over the wire is the id the stored answer speaks about: the
/// summary reports it in `SelectedChoices` and `ChoiceAnswerSummaries` and derives the
/// question's `Risk` from it, while the catalog itself is unchanged by the answer. The
/// rows differ along HOW MUCH of the discovered catalog the answer selects — the first
/// choice, the last choice, all of them, and none — which is what moves the risk.
#[tokio::test]
async fn a_discovered_choice_id_round_trips_through_the_stored_answer() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "choice-roundtrip-host").await;

    // Discover the catalog exactly as a client does: read the answers, then answer a
    // question with the ids the read reported.
    let discovered = collect_summaries("discovery", &[], Some(50), |query| {
        list_answers(&client, &workload_id, FRAMEWORK_LENS, query)
    })
    .await;
    let catalogs = catalogs_of("discovery", &discovered);

    // Four questions from four pillars, so the rule is exercised across the catalog
    // rather than on one question.
    let questions: Vec<String> = ["organization-priorities", "detect-investigate-events", "back-up-data", "user-behavior-patterns"]
        .iter()
        .map(ToString::to_string)
        .collect();

    for question_id in &questions {
        let catalog = catalogs
            .get(question_id)
            .unwrap_or_else(|| panic!("{question_id} must be reported by ListAnswers"));
        let ids: Vec<String> = catalog.iter().map(|(id, _)| id.clone()).collect();
        let last = ids.last().cloned().expect("a non-empty catalog");

        let rows: Vec<AnswerRow> = vec![
            ("first-choice-only", vec![ids[0].clone()], "MEDIUM"),
            ("last-choice-only", vec![last.clone()], "MEDIUM"),
            ("every-choice", ids.clone(), "NONE"),
            ("no-choice", Vec::new(), "HIGH"),
        ];

        for (row, selected, expected_risk) in rows {
            let case = format!("{question_id}/{row}");
            let selected_refs: Vec<&str> = selected.iter().map(String::as_str).collect();
            client.record_answer(
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                answered_over(&ids, &selected_refs, true, None),
            );
            // What the read must report for every choice of the question: the ids the
            // answer named are in place and the rest are not.
            let choice_answers: Vec<(String, String)> = ids
                .iter()
                .map(|choice_id| {
                    let status = if selected.contains(choice_id) {
                        SELECTED
                    } else {
                        UNSELECTED
                    };
                    (choice_id.clone(), status.to_string())
                })
                .collect();

            let summaries = collect_summaries(&case, &[], Some(50), |query| {
                list_answers(&client, &workload_id, FRAMEWORK_LENS, query)
            })
            .await;
            let summary = summaries
                .iter()
                .find(|summary| {
                    summary.get("QuestionId").and_then(Value::as_str) == Some(question_id.as_str())
                })
                .unwrap_or_else(|| panic!("case {case:?}: {question_id} must be reported"));

            assert_eq!(
                catalog_of(&case, summary),
                *catalog,
                "case {case:?}: answering a question must not change its choice catalog, \
                 got {summary:?}"
            );
            assert_eq!(
                summary
                    .get("SelectedChoices")
                    .and_then(Value::as_array)
                    .map(|values| values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<String>>()),
                Some(selected.clone()),
                "case {case:?}: expected SelectedChoices {selected:?}, got {summary:?}"
            );
            let reported: BTreeMap<String, String> = summary
                .get("ChoiceAnswerSummaries")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|entry| {
                            (
                                entry
                                    .get("ChoiceId")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                entry
                                    .get("Status")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let expected: BTreeMap<String, String> = choice_answers.iter().cloned().collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the per-choice answers {expected:?}, got {summary:?}"
            );
            assert_eq!(
                summary.get("Risk").and_then(Value::as_str),
                Some(expected_risk),
                "case {case:?}: expected Risk {expected_risk}, got {summary:?}"
            );
        }
    }
}

/// The catalog is the Well-Architected Framework's published best practices for the
/// question, not a placeholder: a row names one best practice the framework publishes for
/// a question and asserts the question's catalog offers it under an id of its own.
///
/// Source of the rows: the AWS Well-Architected Framework's best practices for those
/// questions (OPS01-BP01, SEC01-BP01, REL09-BP02, COST02-BP05 and SUS05-BP03) — the
/// pillar whitepapers the lens's question titles come from.
#[tokio::test]
async fn a_questions_catalog_offers_the_frameworks_published_best_practices() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "best-practice-host").await;
    let summaries = collect_summaries("best-practices", &[], Some(50), |query| {
        list_answers(&client, &workload_id, FRAMEWORK_LENS, query)
    })
    .await;
    let catalogs = catalogs_of("best-practices", &summaries);

    for (question_id, best_practice) in [
        ("organization-priorities", "Evaluate external customer needs"),
        ("securely-operate", "Separate workloads using accounts"),
        ("back-up-data", "Secure and encrypt backups"),
        ("govern-usage", "Implement cost controls"),
        ("hardware-patterns", "Use managed services"),
    ] {
        let catalog = catalogs
            .get(question_id)
            .unwrap_or_else(|| panic!("{question_id} must be reported by ListAnswers"));
        let offered: Vec<&str> = catalog.iter().map(|(_, title)| title.as_str()).collect();
        assert!(
            offered.contains(&best_practice),
            "case {question_id:?}: the catalog must offer the published best practice \
             {best_practice:?}, got {offered:?}"
        );
    }
}

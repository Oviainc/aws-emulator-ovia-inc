// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the derivation of `Answer$Risk` from the selection an answer
//! currently records.
//!
//! The rule these pin, over a TABLE of questions and both ways of clearing a selection
//! rather than one recorded request: a question's `Risk` follows what its answer records
//! about that question's own choices, and an answer that records NO choice — because it
//! selected none, or because a later update took the selection away — leaves the question
//! unanswered, so it reports `UNANSWERED` (`smithy_query(query="shape", shape="Risk")`
//! publishes it beside the four rated risks) and counts in the review's `UNANSWERED`
//! bucket. Selecting again rates it again, and a milestone recorded while the question was
//! rated keeps the rating it recorded.
//!
//! The rows span the lens the review is against (the AWS-official framework lens and an
//! imported custom lens), the number of best practices the question asks about, how much
//! of the selection the answer records first, and whether the selection is cleared by
//! `SelectedChoices` or by `ChoiceUpdates`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::custom_lens_import::{import, imported_arn, lens_document};
use super::workload_harness::{created_id, encode_path_segment, minimal_create, Answer, Client};

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The `Risk` a question with no answer, and a question whose answer records nothing about
/// any of its choices, both report.
const UNANSWERED: &str = "UNANSWERED";

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
/// Every label is non-greedy, so each travels percent-encoded in one path segment — which
/// is what carries a custom lens's ARN, with its `:` and `/`, as one segment.
async fn update_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
}

/// `GetAnswer`: the same path read with `GET`.
async fn get_answer(client: &Client, workload_id: &str, lens: &str, question_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// The `Risk` a read of one answer reports.
async fn reported_risk(
    client: &Client,
    case: &str,
    workload_id: &str,
    lens: &str,
    question_id: &str,
) -> String {
    let read = get_answer(client, workload_id, lens, question_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected GetAnswer 200, got {read:?}"
    );
    read.body
        .get("Answer")
        .and_then(|answer| answer.get("Risk"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `RiskCounts` of one lens review, keyed by the `Risk` each bucket counts.
async fn review_risk_counts(
    client: &Client,
    case: &str,
    workload_id: &str,
    lens: &str,
) -> BTreeMap<String, i64> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("RiskCounts"))
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Risk` of the ONE question a review rates, and how many of its questions are
/// unanswered.
fn rated_and_unanswered(counts: &BTreeMap<String, i64>) -> (Vec<String>, i64) {
    let rated: Vec<String> = counts
        .iter()
        .filter(|(risk, count)| risk.as_str() != UNANSWERED && **count > 0)
        .map(|(risk, _)| risk.clone())
        .collect();
    (rated, counts.get(UNANSWERED).copied().unwrap_or(-1))
}

/// One question of the framework lens per DISTINCT number of best practices it asks about,
/// read off the catalog that lens's content is (`state::FRAMEWORK_LENS_QUESTIONS`), so the
/// table spans the choice counts the lens really publishes rather than one cited question.
fn framework_questions_spanning_choice_counts() -> Vec<(&'static str, Vec<String>)> {
    let mut by_count: BTreeMap<usize, (&'static str, Vec<String>)> = BTreeMap::new();
    for question in state::FRAMEWORK_LENS_QUESTIONS {
        let choices: Vec<String> = (0..question.best_practices.len())
            .map(|position| question.choice_id(position))
            .collect();
        by_count
            .entry(choices.len())
            .or_insert((question.question_id, choices));
    }
    by_count.into_values().collect()
}

/// How much of a question's choice set the answer records as in place before it is cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    /// The first choice alone.
    First,
    /// Every choice the question asks about.
    Every,
}

/// How the request that takes the selection away is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clearing {
    /// `SelectedChoices` naming no choice at all: "the values entered replace the
    /// previously selected choices" (`SelectedChoices` doc).
    EmptySelectedChoices,
    /// Every choice that was in place recorded `UNSELECTED` through `ChoiceUpdates`.
    UnselectEveryChoice,
}

impl Clearing {
    /// The `UpdateAnswer` body that clears `in_place` out of `choices`.
    fn body(self, in_place: &[String]) -> Value {
        match self {
            Clearing::EmptySelectedChoices => json!({ "SelectedChoices": [] }),
            Clearing::UnselectEveryChoice => {
                let updates: BTreeMap<String, Value> = in_place
                    .iter()
                    .map(|choice_id| (choice_id.clone(), json!({ "Status": "UNSELECTED" })))
                    .collect();
                json!({ "ChoiceUpdates": updates })
            }
        }
    }
}

/// One cycle under test: which question of which lens review is answered, how much of its
/// choice set the answer records, and how the selection is then taken away.
struct Cycle<'a> {
    case: &'a str,
    workload_id: &'a str,
    lens: &'a str,
    question_id: &'a str,
    choices: &'a [String],
    selection: Selection,
    clearing: Clearing,
}

/// The rating of one question over the whole cycle — rated, cleared, rated again — for one
/// lens review. The `Risk` the selection earns is derived from the sizes rather than
/// restated per row.
async fn assert_clearing_unanswers(client: &Client, cycle: &Cycle<'_>) {
    let Cycle {
        case,
        workload_id,
        lens,
        question_id,
        choices,
        selection,
        clearing,
    } = *cycle;
    let in_place: Vec<String> = match selection {
        Selection::First => choices[..1].to_vec(),
        Selection::Every => choices.to_vec(),
    };
    let expected_rated = if in_place.len() == choices.len() {
        "NONE"
    } else {
        "MEDIUM"
    };

    // Answered: the selection is what rates the question, and the review counts it there.
    let answered = update_answer(
        client,
        workload_id,
        lens,
        question_id,
        json!({ "SelectedChoices": in_place }),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected UpdateAnswer 200, got {answered:?}"
    );
    let rated = answered
        .body
        .get("Answer")
        .and_then(|answer| answer.get("Risk"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        rated, expected_rated,
        "case {case:?}: {} of {} best practices in place must rate {expected_rated}, got \
         {answered:?}",
        in_place.len(),
        choices.len()
    );

    // Cleared: the answer records no choice, so the question is unanswered again — in the
    // 200 response, in the read of the answer, and in the review's own roll-up.
    let cleared = update_answer(
        client,
        workload_id,
        lens,
        question_id,
        clearing.body(&in_place),
    )
    .await;
    assert_eq!(
        cleared.status, 200,
        "case {case:?}: expected UpdateAnswer 200 for the clearing request, got {cleared:?}"
    );
    let cleared_answer = cleared
        .body
        .get("Answer")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        cleared_answer.get("Risk").and_then(Value::as_str),
        Some(UNANSWERED),
        "case {case:?}: clearing the selection must report Risk UNANSWERED, got {cleared:?}"
    );
    assert_eq!(
        cleared_answer.get("SelectedChoices"),
        Some(&json!([])),
        "case {case:?}: the cleared answer selects nothing, got {cleared:?}"
    );
    assert_eq!(
        reported_risk(client, case, workload_id, lens, question_id).await,
        UNANSWERED,
        "case {case:?}: GetAnswer must report the cleared question as UNANSWERED"
    );
    let counts = review_risk_counts(client, case, workload_id, lens).await;
    let (rated_buckets, _) = rated_and_unanswered(&counts);
    assert!(
        rated_buckets.is_empty(),
        "case {case:?}: a review whose only answer was cleared rates nothing, got {counts:?}"
    );

    // And the question is answerable again: selecting the same choices rates it as before,
    // so clearing takes nothing away but the selection.
    let reanswered = update_answer(
        client,
        workload_id,
        lens,
        question_id,
        json!({ "SelectedChoices": in_place }),
    )
    .await;
    assert_eq!(
        reanswered
            .body
            .get("Answer")
            .and_then(|answer| answer.get("Risk"))
            .and_then(Value::as_str),
        Some(expected_rated),
        "case {case:?}: selecting the same best practices again must rate {expected_rated}, \
         got {reanswered:?}"
    );
}

/// The rule over the AWS-official framework lens: every distinct choice count the lens
/// publishes, each cleared both ways, from both a partial and a full selection.
#[tokio::test]
async fn clearing_the_selection_unanswers_a_framework_question() {
    let client = Client::new();
    let questions = framework_questions_spanning_choice_counts();
    assert!(
        questions.len() > 1,
        "the framework lens must publish questions of differing choice counts, got {questions:?}"
    );

    for (index, (question_id, choices)) in questions.iter().enumerate() {
        for selection in [Selection::First, Selection::Every] {
            for clearing in [Clearing::EmptySelectedChoices, Clearing::UnselectEveryChoice] {
                let case = format!(
                    "{question_id}/{}-choices/{selection:?}/{clearing:?}",
                    choices.len()
                );
                // A fresh workload per row, so each row's roll-up is moved by its own
                // answer alone.
                let workload_id = created_id(
                    &client
                        .create(minimal_create(
                            &format!("cleared-{index}-{selection:?}-{clearing:?}"),
                            &format!("tok-cleared-{index}-{selection:?}-{clearing:?}"),
                        ))
                        .await,
                    &case,
                );
                assert_clearing_unanswers(
                    &client,
                    &Cycle {
                        case: &case,
                        workload_id: &workload_id,
                        lens: FRAMEWORK_LENS,
                        question_id,
                        choices,
                        selection,
                        clearing,
                    },
                )
                .await;
            }
        }
    }
}

/// The same rule over an imported custom lens, whose questions and choices are the ones its
/// own document defines: the derivation is the lens review's, not the catalog's.
#[tokio::test]
async fn clearing_the_selection_unanswers_a_custom_lens_question() {
    let client = Client::new();
    // One pillar, two questions, three choices each: enough for a partial selection to
    // differ from a full one.
    let document = lens_document("Cleared selection lens", None, &[("governance", 2, 3)]);
    let lens_arn = imported_arn(
        &import(
            &client,
            &document.to_string(),
            None,
            "tok-cleared-custom",
            None,
        )
        .await,
        "import",
    );
    let published = client
        .send(
            "POST",
            &format!("/lenses/{}/versions", encode_path_segment(&lens_arn)),
            Some(json!({ "LensVersion": "1.0", "ClientRequestToken": "tok-cleared-publish" })),
        )
        .await;
    assert_eq!(
        published.status, 200,
        "the lens under test must publish, got {published:?}"
    );

    for (index, question_id) in ["governance_q0", "governance_q1"].iter().enumerate() {
        let choices: Vec<String> = (0..3)
            .map(|choice| format!("governance_q{index}_c{choice}"))
            .collect();
        for clearing in [Clearing::EmptySelectedChoices, Clearing::UnselectEveryChoice] {
            let case = format!("{question_id}/{clearing:?}");
            let mut request = minimal_create(
                &format!("cleared-custom-{index}-{clearing:?}"),
                &format!("tok-cleared-custom-{index}-{clearing:?}"),
            );
            request["Lenses"] = json!([lens_arn]);
            let workload_id = created_id(&client.create(request).await, &case);
            assert_clearing_unanswers(
                &client,
                &Cycle {
                    case: &case,
                    workload_id: &workload_id,
                    lens: &lens_arn,
                    question_id,
                    choices: &choices,
                    selection: Selection::First,
                    clearing,
                },
            )
            .await;
        }
    }
}

/// A milestone keeps the rating it recorded: clearing the live selection unanswers the live
/// question and leaves the milestone's own answer — and its roll-up — where they were.
#[tokio::test]
async fn a_cleared_selection_leaves_the_milestones_rating_alone() {
    let client = Client::new();
    let (question_id, choices) = framework_questions_spanning_choice_counts()
        .into_iter()
        .find(|(_, choices)| choices.len() > 1)
        .expect("the framework lens must ask a question about several best practices");
    let workload_id = created_id(
        &client
            .create(minimal_create("cleared-milestone", "tok-cleared-ms"))
            .await,
        "host",
    );

    let answered = update_answer(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        question_id,
        json!({ "SelectedChoices": choices }),
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "the question under test must be answerable, got {answered:?}"
    );
    let milestone = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({
                "MilestoneName": "answered-before-the-clearing",
                "ClientRequestToken": "tok-cleared-ms-1",
            })),
        )
        .await;
    assert_eq!(
        milestone.status, 200,
        "CreateMilestone must record the answered review, got {milestone:?}"
    );
    let number = milestone
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    let cleared = update_answer(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        question_id,
        json!({ "SelectedChoices": [] }),
    )
    .await;
    assert_eq!(
        cleared.status, 200,
        "clearing the selection must be accepted, got {cleared:?}"
    );

    for (case, milestone_number, expected) in [
        ("live", None, UNANSWERED),
        ("milestone", Some(number), "NONE"),
    ] {
        let path = match milestone_number {
            None => format!(
                "/workloads/{}/lensReviews/{}/answers/{}",
                encode_path_segment(&workload_id),
                encode_path_segment(FRAMEWORK_LENS),
                encode_path_segment(question_id)
            ),
            Some(number) => format!(
                "/workloads/{}/lensReviews/{}/answers/{}?MilestoneNumber={number}",
                encode_path_segment(&workload_id),
                encode_path_segment(FRAMEWORK_LENS),
                encode_path_segment(question_id)
            ),
        };
        let read = client.send("GET", &path, None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAnswer 200, got {read:?}"
        );
        assert_eq!(
            read.body
                .get("Answer")
                .and_then(|answer| answer.get("Risk"))
                .and_then(Value::as_str),
            Some(expected),
            "case {case:?}: expected Risk {expected}, got {read:?}"
        );
    }
}

/// The bucket the review counts a cleared question in is the one it counted before anything
/// answered it: the whole question set, and nothing in any rated bucket. Pinned over the
/// lens's own question set rather than over one question.
#[tokio::test]
async fn a_review_whose_answers_are_all_cleared_counts_every_question_unanswered() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("cleared-rollup", "tok-cleared-rollup"))
            .await,
        "host",
    );

    let before = review_risk_counts(&client, "before", &workload_id, FRAMEWORK_LENS).await;
    let (rated_before, outstanding) = rated_and_unanswered(&before);
    assert!(
        rated_before.is_empty() && outstanding > 0,
        "a fresh review rates nothing and counts every question unanswered, got {before:?}"
    );

    // Answer a handful of questions, each with a different selection size, then clear every
    // one of them.
    let answered: Vec<(&str, Vec<String>)> = framework_questions_spanning_choice_counts()
        .into_iter()
        .take(4)
        .collect();
    for (question_id, choices) in &answered {
        let written = update_answer(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            question_id,
            json!({ "SelectedChoices": choices[..1].to_vec() }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {question_id:?}: expected UpdateAnswer 200, got {written:?}"
        );
    }
    let while_answered =
        review_risk_counts(&client, "answered", &workload_id, FRAMEWORK_LENS).await;
    let (rated_buckets, still_outstanding) = rated_and_unanswered(&while_answered);
    assert!(
        !rated_buckets.is_empty(),
        "the answered questions must be rated somewhere, got {while_answered:?}"
    );
    assert_eq!(
        still_outstanding,
        outstanding - i64::try_from(answered.len()).unwrap_or(0),
        "every answered question leaves the UNANSWERED bucket, got {while_answered:?}"
    );

    for (question_id, _) in &answered {
        let cleared = update_answer(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            question_id,
            json!({ "SelectedChoices": [] }),
        )
        .await;
        assert_eq!(
            cleared.status, 200,
            "case {question_id:?}: clearing must be accepted, got {cleared:?}"
        );
    }

    let after = review_risk_counts(&client, "cleared", &workload_id, FRAMEWORK_LENS).await;
    assert_eq!(
        after, before,
        "a review whose every answer was cleared counts exactly as it did before any of \
         them, expected {before:?}, got {after:?}"
    );

    // The question set itself did not move: the review still reports every question, each
    // one unanswered.
    let listed = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers?MaxResults=50",
                encode_path_segment(&workload_id),
                encode_path_segment(FRAMEWORK_LENS)
            ),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "ListAnswers must serve the cleared review, got {listed:?}"
    );
    let risks: BTreeSet<String> = listed
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| {
            summary
                .get("Risk")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    assert_eq!(
        risks,
        BTreeSet::from([UNANSWERED.to_string()]),
        "every question of the cleared review must read as UNANSWERED, got {risks:?}"
    );
}

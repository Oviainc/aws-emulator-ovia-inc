// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the review content a `CreateWorkload` carries over from the
//! review template it applies.
//!
//! The rule pinned here: a review template applied to a workload PRE-POPULATES that
//! workload's review of each lens the template applies — the answers the template records
//! for that lens's questions, and the notes written against that lens review and its
//! pillars ("for a review template, these are the notes that will be associated with the
//! workload when the template is applied", `smithy_query(query="doc", shape="Notes")` and
//! `smithy_query(query="doc", shape="PillarNotes")`; reading the template's answers is what
//! `CreateWorkload` documents `wellarchitected:GetReviewTemplateAnswer` and
//! `wellarchitected:ListReviewTemplateAnswers` for). Nothing else is pre-populated: a lens
//! the template does not apply, and a workload created from no template at all, report
//! every question `UNANSWERED`.
//!
//! The rows are the lens's WHOLE question set, each question answered a different way
//! (every choice in place, one of them, none of them, the question not applicable), so the
//! carry-over is pinned over every answer shape rather than over one recorded answer. The
//! notes rows differ along which halves of the notes the template carries.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, review_template, Answer, Client,
};

/// The Serverless Applications Lens and how many questions this emulator's content for it
/// asks — the catalog itself is pinned by `super::official_lens_catalog`. Its content asks
/// nothing in the framework's sustainability pillar, so a review of it summarises five
/// pillars rather than six.
const SERVERLESS_LENS: &str = "serverless";
const SERVERLESS_QUESTIONS: usize = 9;

/// The AWS Well-Architected Framework lens and how many questions it asks (the framework's
/// published numbering: OPS 11, SEC 11, REL 13, PERF 5, COST 11, SUS 6). Its content asks
/// in every pillar the framework publishes, so a review of it summarises all of
/// [`FRAMEWORK_PILLARS`].
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// Every bucket the `Risk` enum publishes (`smithy_query(query="shape", shape="Risk")`), so
/// a roll-up is checked for ALL of them rather than the ones a case moved.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// The `AnswerReason` this suite records for a question it marks not applicable, from the
/// modelled value set (`smithy_query(query="shape", shape="AnswerReason")`).
const OUT_OF_SCOPE: &str = "OUT_OF_SCOPE";

/// The pillars the Well-Architected Framework publishes, in the order it presents them
/// (`PillarId` and the published `PillarName`).
const FRAMEWORK_PILLARS: [(&str, &str); 6] = [
    ("operationalExcellence", "Operational Excellence"),
    ("security", "Security"),
    ("reliability", "Reliability"),
    ("performance", "Performance Efficiency"),
    ("costOptimization", "Cost Optimization"),
    ("sustainability", "Sustainability"),
];

/// How one row answers its question in the template.
#[derive(Debug, Clone, Copy)]
enum Selection {
    /// Every choice the question asks about is in place.
    Every,
    /// Only the first choice is in place.
    First,
    /// No choice is in place.
    None,
}

/// `GetAnswer`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn get_answer(client: &Client, workload_id: &str, lens: &str, question_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// The `Answer` / `ReviewTemplateAnswer` of a successful answer read.
fn answer_body(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Answer must be present, got {answer:?}"))
}

/// The `ChoiceId`s a question offers, in the order its `Choices` catalog reports them — the
/// only place a client discovers them.
fn choice_ids(answer: &Value) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| {
                    choice
                        .get("ChoiceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An answer's `ChoiceAnswers` as a `ChoiceId` -> `ChoiceStatus` table.
fn choice_statuses(answer: &Value) -> BTreeMap<String, String> {
    answer
        .get("ChoiceAnswers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
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
        .unwrap_or_default()
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

/// One `RiskCounts` map of a response, as a sorted map.
fn risk_counts(value: &Value) -> BTreeMap<String, i64> {
    value
        .get("RiskCounts")
        .and_then(Value::as_object)
        .map(|counts| {
            counts
                .iter()
                .map(|(risk, count)| (risk.clone(), count.as_i64().unwrap_or(-1)))
                .collect()
        })
        .unwrap_or_default()
}

/// A roll-up with every published bucket at zero, for a test to add to.
fn empty_counts() -> BTreeMap<String, i64> {
    RISKS.iter().map(|risk| ((*risk).to_string(), 0)).collect()
}

/// The `QuestionId`s of a review template's lens review, in the order the listing serves
/// them: `GET /reviewTemplates/{TemplateArn}/lensReviews/{LensAlias}/answers`.
async fn template_question_ids(client: &Client, template_arn: &str, lens: &str) -> Vec<String> {
    let answer = client
        .list_review_template_answers(template_arn, lens, "")
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"template-questions\": expected ListReviewTemplateAnswers 200, got {answer:?}"
    );
    assert!(
        answer.body.get("NextToken").is_none(),
        "case \"template-questions\": the lens's whole question set must fit in one page, got \
         {answer:?}"
    );
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
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
        })
        .unwrap_or_default()
}

/// Rule: the answers a template records are the answers the workload created from it
/// starts with, question by question, and a lens the template says nothing about is
/// untouched.
#[tokio::test]
async fn a_workload_created_from_a_template_starts_with_the_templates_answers() {
    let client = Client::new();
    let template_arn = review_template(&client, "tpl-carried-answers", &[SERVERLESS_LENS]).await;

    let question_ids = template_question_ids(&client, &template_arn, SERVERLESS_LENS).await;
    assert_eq!(
        question_ids.len(),
        SERVERLESS_QUESTIONS,
        "case \"template-questions\": expected the lens's whole question set, got {question_ids:?}"
    );

    // Answer every question of the template's lens, each in a different way, and remember
    // the `Risk` the answer earns: a question that does not apply is NOT_APPLICABLE, one
    // whose answer selects nothing is still UNANSWERED, one with something left out is
    // MEDIUM, one with nothing left out is NONE.
    let mut expected_risks: BTreeMap<String, &'static str> = BTreeMap::new();
    for (index, question_id) in question_ids.iter().enumerate() {
        let case = format!("answer-{question_id}");
        let stored = answer_body(
            &client
                .get_review_template_answer(&template_arn, SERVERLESS_LENS, question_id)
                .await,
            &case,
        );
        let choices = choice_ids(&stored);
        assert!(
            !choices.is_empty(),
            "case {case:?}: the question must offer choices to answer, got {stored:?}"
        );

        let selection = match index % 4 {
            0 => Selection::Every,
            1 => Selection::First,
            2 => Selection::None,
            _ => Selection::Every,
        };
        let applicable = index % 4 != 3;
        let selected: Vec<String> = match selection {
            Selection::Every => choices.clone(),
            Selection::First => choices[..1].to_vec(),
            Selection::None => Vec::new(),
        };
        let risk = if !applicable {
            "NOT_APPLICABLE"
        } else if selected.is_empty() {
            "UNANSWERED"
        } else if selected.len() < choices.len() {
            "MEDIUM"
        } else {
            "NONE"
        };
        expected_risks.insert(question_id.clone(), risk);

        let mut body = json!({
            "SelectedChoices": selected,
            "IsApplicable": applicable,
            "Notes": format!("template notes for {question_id}"),
        });
        if !applicable {
            body["Reason"] = json!(OUT_OF_SCOPE);
        }
        let written = client
            .update_review_template_answer(&template_arn, SERVERLESS_LENS, question_id, body)
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: expected UpdateReviewTemplateAnswer 200, got {written:?}"
        );
    }

    // The workload brings a lens of its own and applies the template.
    let mut request = minimal_create("workload-from-answered-template", "tok-carried");
    request["Lenses"] = json!([FRAMEWORK_LENS]);
    request["ReviewTemplateArns"] = json!([template_arn]);
    let workload_id = created_id(&client.create(request).await, "create-from-template");

    // Every answer the template recorded is the answer the workload starts with.
    for question_id in &question_ids {
        let case = format!("carried-{question_id}");
        let from_template = answer_body(
            &client
                .get_review_template_answer(&template_arn, SERVERLESS_LENS, question_id)
                .await,
            &case,
        );
        let on_workload = answer_body(
            &get_answer(&client, &workload_id, SERVERLESS_LENS, question_id).await,
            &case,
        );

        assert_eq!(
            string_list(&on_workload, "SelectedChoices"),
            string_list(&from_template, "SelectedChoices"),
            "case {case:?}: expected the workload's SelectedChoices to be the template's, got \
             {on_workload:?}"
        );
        assert_eq!(
            choice_statuses(&on_workload),
            choice_statuses(&from_template),
            "case {case:?}: expected the workload's per-choice answers to be the template's, got \
             {on_workload:?}"
        );
        for member in ["IsApplicable", "Notes", "Reason"] {
            assert_eq!(
                on_workload.get(member),
                from_template.get(member),
                "case {case:?}: expected the workload's {member} to be the template's, got \
                 {on_workload:?}"
            );
        }
        assert_eq!(
            on_workload.get("Risk").and_then(Value::as_str),
            expected_risks.get(question_id.as_str()).copied(),
            "case {case:?}: expected the carried answer to carry its own risk, got {on_workload:?}"
        );
    }

    // The roll-up of the carried review moves with those answers...
    let mut carried_counts = empty_counts();
    for risk in expected_risks.values() {
        *carried_counts.entry((*risk).to_string()).or_default() += 1;
    }
    let carried_review = get_lens_review(&client, &workload_id, SERVERLESS_LENS).await;
    let review = carried_review
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        risk_counts(&review),
        carried_counts,
        "case \"carried-rollup\": expected the carried review's roll-up to count the template's \
         answers, got {carried_review:?}"
    );

    // ...while the lens the template said nothing about is still entirely unanswered.
    let own_review = get_lens_review(&client, &workload_id, FRAMEWORK_LENS).await;
    let review = own_review
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or(Value::Null);
    let mut own_counts = empty_counts();
    own_counts.insert("UNANSWERED".to_string(), FRAMEWORK_QUESTIONS);
    assert_eq!(
        risk_counts(&review),
        own_counts,
        "case \"own-lens-untouched\": a lens the template does not apply must report every \
         question UNANSWERED, got {own_review:?}"
    );

    // And the workload's own roll-up is the two reviews together.
    let mut total = carried_counts.clone();
    *total.entry("UNANSWERED".to_string()).or_default() += FRAMEWORK_QUESTIONS;
    let fetched = client.get(&workload_id).await;
    let workload = fetched
        .body
        .get("Workload")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        risk_counts(&workload),
        total,
        "case \"workload-rollup\": expected the workload's roll-up to be both reviews, got \
         {fetched:?}"
    );

    // The control: the same lenses WITHOUT the template answer nothing.
    let mut request = minimal_create("workload-without-a-template", "tok-no-template");
    request["Lenses"] = json!([FRAMEWORK_LENS, SERVERLESS_LENS]);
    let bare_id = created_id(&client.create(request).await, "create-without-template");
    let mut bare_counts = empty_counts();
    bare_counts.insert(
        "UNANSWERED".to_string(),
        FRAMEWORK_QUESTIONS + i64::try_from(SERVERLESS_QUESTIONS).unwrap_or_default(),
    );
    let fetched = client.get(&bare_id).await;
    let workload = fetched
        .body
        .get("Workload")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        risk_counts(&workload),
        bare_counts,
        "case \"no-template\": a workload created from no template must answer nothing, got \
         {fetched:?}"
    );
}

/// The notes half of the rule: the notes of the template's lens review and of that
/// review's pillars carry over to the workload's review of the same lens. The rows differ
/// along which halves the template carries and how many pillars it speaks about. The
/// template applies the FRAMEWORK lens here, whose content asks in every published pillar,
/// so the pillar rows can span all of them.
#[tokio::test]
async fn a_workload_created_from_a_template_starts_with_the_templates_review_notes() {
    // One row: the lens notes the template carries, and the pillars it notes.
    struct Case {
        case: &'static str,
        lens_notes: Option<&'static str>,
        pillar_notes: Vec<(&'static str, &'static str)>,
    }

    let cases: Vec<Case> = vec![
        Case {
            case: "neither",
            lens_notes: None,
            pillar_notes: Vec::new(),
        },
        Case {
            case: "lens-notes-only",
            lens_notes: Some("the template's lens notes"),
            pillar_notes: Vec::new(),
        },
        Case {
            case: "one-pillar-only",
            lens_notes: None,
            pillar_notes: vec![(FRAMEWORK_PILLARS[1].0, "the template's security notes")],
        },
        Case {
            case: "lens-notes-and-two-pillars",
            lens_notes: Some("the template's lens notes"),
            pillar_notes: vec![
                (FRAMEWORK_PILLARS[0].0, "the template's operations notes"),
                (FRAMEWORK_PILLARS[5].0, "the template's sustainability notes"),
            ],
        },
        Case {
            case: "every-pillar",
            lens_notes: Some("the template's lens notes"),
            pillar_notes: FRAMEWORK_PILLARS
                .iter()
                .map(|(pillar_id, pillar_name)| (*pillar_id, *pillar_name))
                .collect(),
        },
    ];

    for row in cases {
        let Case {
            case,
            lens_notes,
            pillar_notes,
        } = row;
        let client = Client::new();
        let template_arn = review_template(&client, &format!("tpl-{case}"), &[FRAMEWORK_LENS]).await;

        let mut body = json!({});
        if let Some(notes) = lens_notes {
            body["LensNotes"] = json!(notes);
        }
        if !pillar_notes.is_empty() {
            body["PillarNotes"] = Value::Object(
                pillar_notes
                    .iter()
                    .map(|(pillar_id, notes)| ((*pillar_id).to_string(), json!(notes)))
                    .collect(),
            );
        }
        if body.as_object().is_some_and(|members| !members.is_empty()) {
            let written = client
                .update_review_template_lens_review(&template_arn, FRAMEWORK_LENS, body)
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected UpdateReviewTemplateLensReview 200, got {written:?}"
            );
        }

        let mut request = minimal_create(&format!("wl-{case}"), &format!("tok-{case}"));
        request["Lenses"] = json!([SERVERLESS_LENS]);
        request["ReviewTemplateArns"] = json!([template_arn]);
        let workload_id = created_id(&client.create(request).await, case);

        let answer = get_lens_review(&client, &workload_id, FRAMEWORK_LENS).await;
        let review = answer
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            review.get("Notes").and_then(Value::as_str),
            lens_notes,
            "case {case:?}: expected the carried review's notes to be the template's, got {answer:?}"
        );

        let expected: BTreeMap<&str, &str> = pillar_notes.iter().copied().collect();
        let summaries = review
            .get("PillarReviewSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            FRAMEWORK_PILLARS.len(),
            "case {case:?}: the review must summarise every pillar of the lens, got {answer:?}"
        );
        for summary in &summaries {
            let pillar_id = summary
                .get("PillarId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                summary.get("Notes").and_then(Value::as_str),
                expected.get(pillar_id).copied(),
                "case {case:?}: expected pillar {pillar_id:?} to carry the template's notes, got \
                 {summary:?}"
            );
        }

        // The workload's OWN lens, which the template says nothing about, carries no notes.
        let answer = get_lens_review(&client, &workload_id, SERVERLESS_LENS).await;
        let review = answer
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            review.get("Notes").and_then(Value::as_str),
            None,
            "case {case:?}: a lens the template does not apply must carry no notes, got {answer:?}"
        );
    }
}

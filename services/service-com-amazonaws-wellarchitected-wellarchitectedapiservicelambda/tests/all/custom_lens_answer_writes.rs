// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WRITING the answers of a lens review when the lens under
//! review is a custom lens the caller imported: `UpdateAnswer` on a workload's review and
//! `UpdateReviewTemplateAnswer` on a review template's.
//!
//! The rule these pin, over a table of lens documents rather than one recorded request: the
//! questions of a lens review are the ones the LENS's own content defines — the AWS-official
//! catalog entry for an official lens, the published imported document for a custom one —
//! so every question a read of the review reports can be answered, and only a `QuestionId`
//! that lens does not ask names no answer to write. The question each case names is
//! discovered the way a client discovers it, from the review's own answer list, so a write
//! is judged against exactly what the read served.
//!
//! The rows differ along the SHAPE of the document — how many pillars it defines, how many
//! questions each pillar asks and how many choices each question offers — and, per
//! question, along WHICH of its choices the answer selects, because both are what the
//! recorded answer and the risk derived from it move with. The AWS-official half of the
//! catalog is written through the same resolution, which the official-lens test here pins.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`; the import and
//! publish wire helpers are the ones `super::custom_lens_import` and
//! `super::custom_lens_versions` already spell.

use serde_json::{json, Value};

use super::custom_lens_import::{import, imported_arn, lens_document, wait_for_the_import_to_land};
use super::custom_lens_versions::create_version;
use super::workload_harness::{
    assert_field_validation, created_id, encode_path_segment, member, minimal_create,
    review_template, Answer, Client,
};

/// The shape of one lens document: `(pillar id, questions in it, choices per question)`,
/// exactly what `super::custom_lens_import::lens_document` builds a document from.
type Shape = &'static [(&'static str, usize, usize)];

/// The alias of the AWS-official lens this emulator ships a question set for, and one
/// question of it: the published Well-Architected Framework numbering, as the emulator's
/// own ids spell it (`tests/all/answer_updates.rs` answers the same question).
const FRAMEWORK_LENS: &str = "wellarchitected";
const FRAMEWORK_QUESTION: &str = "ops-01";

/// The documents under test: the rows differ along the number of pillars, the number of
/// questions a pillar asks and the number of choices a question offers — including a
/// question that offers exactly ONE choice, where "every choice" and "the first choice"
/// are the same selection and "all but the last" is none of them.
const SHAPES: &[(&str, Shape)] = &[
    ("one-question-one-choice", &[("governance", 1, 1)]),
    ("one-pillar-many-questions", &[("security", 2, 3)]),
    ("several-pillars", &[("operations", 1, 4), ("cost", 2, 2)]),
];

/// Which of a question's choices an answer selects. The plans span the risk the service
/// derives from a recorded answer: nothing selected, one choice, every choice and all but
/// the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selection {
    None,
    First,
    All,
    AllButLast,
}

impl Selection {
    /// The `ChoiceId`s this plan selects out of a question's own catalog, in the catalog's
    /// order.
    fn of(self, choices: &[String]) -> Vec<String> {
        match self {
            Selection::None => Vec::new(),
            Selection::First => choices.iter().take(1).cloned().collect(),
            Selection::All => choices.to_vec(),
            Selection::AllButLast => choices
                .iter()
                .take(choices.len().saturating_sub(1))
                .cloned()
                .collect(),
        }
    }
}

const SELECTIONS: [Selection; 4] = [
    Selection::None,
    Selection::First,
    Selection::All,
    Selection::AllButLast,
];

/// The `Risk` the service derives from an applicable answer: a question with no best
/// practice in place is `HIGH`, one with some choice still unselected is `MEDIUM`, and one
/// with nothing left unselected is `NONE` (`smithy_query(query="shape", shape="Risk")`
/// publishes the value set).
fn expected_risk(choices: usize, selected: usize) -> &'static str {
    if selected == 0 {
        "HIGH"
    } else if selected < choices {
        "MEDIUM"
    } else {
        "NONE"
    }
}

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
/// Each label is non-greedy, so it travels percent-encoded in one path segment — which is
/// what carries a custom lens's ARN, with its `:` and `/`, as one segment.
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

/// `ListAnswers` for one lens review, in one page: the question set as a CLIENT discovers
/// it before writing to it.
async fn listed_answers(client: &Client, case: &str, workload_id: &str, lens: &str) -> Vec<Value> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers",
                encode_path_segment(workload_id),
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the review's answers must be readable, got {answer:?}"
    );
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Import a document of `shape` as a fresh custom lens and answer its ARN. The caller waits
/// for the import to land and publishes the version itself, so a table can import every
/// row's lens before waiting once.
async fn import_shape(client: &Client, case: &str, shape: Shape) -> String {
    let document = lens_document(&format!("Lens {case}"), None, shape).to_string();
    imported_arn(
        &import(client, &document, None, &format!("tok-import-{case}"), None).await,
        case,
    )
}

/// Publish version `1.0` of an imported lens, so it can be applied to a workload or a
/// review template: "The lens cannot be applied to workloads … until it's published with
/// CreateLensVersion" (`ImportLens` doc).
async fn publish(client: &Client, case: &str, lens_arn: &str) {
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

/// Import, land and publish ONE custom lens of `shape`, and answer its ARN.
async fn published_lens(client: &Client, case: &str, shape: Shape) -> String {
    let lens_arn = import_shape(client, case, shape).await;
    wait_for_the_import_to_land();
    publish(client, case, &lens_arn).await;
    lens_arn
}

fn string_member(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `ChoiceId`s a question's own catalog (`Choices`) offers, in the order served — the
/// only place a client discovers them.
fn choice_ids(value: &Value) -> Vec<String> {
    value
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| string_member(choice, "ChoiceId"))
                .collect()
        })
        .unwrap_or_default()
}

/// The `Answer` / `ReviewTemplateAnswer` a read or a write reported.
fn answer_of(answer: &Answer) -> Value {
    answer.body.get("Answer").cloned().unwrap_or(Value::Null)
}

/// One list-of-strings member of a JSON object, empty when it is absent.
fn strings(value: &Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The per-choice statuses an answer records, as `(ChoiceId, Status)` sorted by id.
fn choice_statuses(answer: &Value, member_name: &str) -> Vec<(String, String)> {
    let mut statuses: Vec<(String, String)> = answer
        .get(member_name)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        string_member(entry, "ChoiceId"),
                        string_member(entry, "Status"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    statuses.sort();
    statuses
}

/// The answer a question of `summary` must report once `selected` has been recorded against
/// it: the question as the LENS defines it, the choices in place in the lens's own choice
/// order, a status for every choice of the question and the derived risk.
fn assert_recorded(case: &str, answer: &Value, summary: &Value, selected: &[String], notes: &str) {
    let choices = choice_ids(summary);
    assert_eq!(
        string_member(answer, "QuestionId"),
        string_member(summary, "QuestionId"),
        "case {case:?}: the answer must name the question it was written for, got {answer:?}"
    );
    assert_eq!(
        string_member(answer, "PillarId"),
        string_member(summary, "PillarId"),
        "case {case:?}: the answer must carry the question's pillar, got {answer:?}"
    );
    assert_eq!(
        string_member(answer, "QuestionTitle"),
        string_member(summary, "QuestionTitle"),
        "case {case:?}: the answer must carry the question's title, got {answer:?}"
    );
    assert_eq!(
        choice_ids(answer),
        choices,
        "case {case:?}: the answer must report the choice catalog its lens defines, got {answer:?}"
    );
    assert_eq!(
        strings(answer, "SelectedChoices"),
        selected,
        "case {case:?}: the choices in place are the ones the request named, in the lens's own \
         choice order, got {answer:?}"
    );
    let expected_statuses: Vec<(String, String)> = {
        let mut statuses: Vec<(String, String)> = choices
            .iter()
            .map(|choice_id| {
                let status = if selected.contains(choice_id) {
                    "SELECTED"
                } else {
                    "UNSELECTED"
                };
                (choice_id.clone(), status.to_string())
            })
            .collect();
        statuses.sort();
        statuses
    };
    assert_eq!(
        choice_statuses(answer, "ChoiceAnswers"),
        expected_statuses,
        "case {case:?}: every choice of the question must record its status, got {answer:?}"
    );
    assert_eq!(
        answer.get("Risk").and_then(Value::as_str),
        Some(expected_risk(choices.len(), selected.len())),
        "case {case:?}: the risk is derived from the choices in place, got {answer:?}"
    );
    assert_eq!(
        string_member(answer, "Notes"),
        notes,
        "case {case:?}: the answer must carry the notes the request wrote, got {answer:?}"
    );
    assert_eq!(
        answer.get("IsApplicable"),
        Some(&Value::Bool(true)),
        "case {case:?}: a question applies until an answer says otherwise, got {answer:?}"
    );
}

/// Every question a published custom lens's document defines is answerable: the write is
/// accepted, records exactly the choices it named, and reads back through `GetAnswer`.
#[tokio::test]
async fn every_question_an_imported_lens_defines_can_be_answered() {
    let client = Client::new();

    // Every row's lens is imported first, so the whole table waits for the import window
    // once rather than per row.
    let mut lenses: Vec<(&str, String)> = Vec::new();
    for (case, shape) in SHAPES {
        lenses.push((case, import_shape(&client, case, shape).await));
    }
    wait_for_the_import_to_land();

    for (case, lens_arn) in &lenses {
        publish(&client, case, lens_arn).await;
        let workload_id = workload_reviewing(&client, case, lens_arn).await;

        // The question set as a CLIENT discovers it: the answer list of the review it is
        // about to write to.
        let summaries = listed_answers(&client, case, &workload_id, lens_arn).await;
        assert!(
            !summaries.is_empty(),
            "case {case:?}: the review must report the questions its lens defines, got \
             {summaries:?}"
        );

        for (position, summary) in summaries.iter().enumerate() {
            let question_id = string_member(summary, "QuestionId");
            let choices = choice_ids(summary);
            let selection = SELECTIONS[position % SELECTIONS.len()];
            let selected = selection.of(&choices);
            let case = format!("{case}-{question_id}-{selection:?}");
            let notes = format!("notes for {question_id}");

            // The request lists the choices in the REVERSE of the catalog's order: the
            // choices in place are a set, so what is reported back is the lens's order
            // whatever order they were sent in.
            let mut sent: Vec<String> = selected.clone();
            sent.reverse();
            let written = update_answer(
                &client,
                &workload_id,
                lens_arn,
                &question_id,
                json!({ "SelectedChoices": sent, "Notes": notes }),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: a question the lens defines must be answerable, got {written:?}"
            );
            assert_eq!(
                member(&written, "WorkloadId"),
                workload_id,
                "case {case:?}: the envelope must name the workload, got {written:?}"
            );
            assert_eq!(
                member(&written, "LensAlias"),
                lens_arn.as_str(),
                "case {case:?}: the envelope must name the lens as sent, got {written:?}"
            );
            assert_eq!(
                member(&written, "LensArn"),
                lens_arn.as_str(),
                "case {case:?}: the envelope must carry the lens ARN, got {written:?}"
            );
            assert_recorded(&case, &answer_of(&written), summary, &selected, &notes);

            // The write is persisted, not just echoed: the single-answer read reports it.
            let read = get_answer(&client, &workload_id, lens_arn, &question_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the recorded answer must read back, got {read:?}"
            );
            assert_recorded(
                &format!("{case}-read-back"),
                &answer_of(&read),
                summary,
                &selected,
                &notes,
            );
        }

        // The review's own answer list moves with the writes: no question of it is
        // UNANSWERED any more.
        for summary in listed_answers(&client, case, &workload_id, lens_arn).await {
            assert_ne!(
                summary.get("Risk").and_then(Value::as_str),
                Some("UNANSWERED"),
                "case {case:?}: every question of the review was answered, got {summary:?}"
            );
        }
    }
}

/// An answer to a custom lens's question records every member the request carries:
/// `ChoiceUpdates` writes one choice's status, reason and notes, and `IsApplicable` with a
/// `Reason` takes the question out of scope.
#[tokio::test]
async fn an_answer_to_an_imported_lenss_question_records_every_member() {
    const SHAPE: Shape = &[("governance", 2, 3)];
    // `ChoiceStatus` publishes SELECTED | UNSELECTED | NOT_APPLICABLE and `ChoiceReason`
    // the five reasons (`smithy_query(query="shape", shape="ChoiceStatus")` /
    // `shape="ChoiceReason"`); the rows below span the statuses and three of the reasons.
    const UPDATES: [(&str, &str); 3] = [
        ("SELECTED", "NONE"),
        ("UNSELECTED", "BUSINESS_PRIORITIES"),
        ("NOT_APPLICABLE", "OUT_OF_SCOPE"),
    ];

    let client = Client::new();
    let lens_arn = published_lens(&client, "members", SHAPE).await;
    let workload_id = workload_reviewing(&client, "members", &lens_arn).await;

    let summaries = listed_answers(&client, "members", &workload_id, &lens_arn).await;
    let summary = summaries
        .first()
        .cloned()
        .expect("the review must report the questions its lens defines");
    let question_id = string_member(&summary, "QuestionId");
    let choices = choice_ids(&summary);

    let mut choice_updates = serde_json::Map::new();
    let mut expected_statuses: Vec<(String, String)> = Vec::new();
    for (choice_id, (status, reason)) in choices.iter().zip(UPDATES) {
        choice_updates.insert(
            choice_id.clone(),
            json!({ "Status": status, "Reason": reason, "Notes": format!("why {choice_id}") }),
        );
        expected_statuses.push((choice_id.clone(), status.to_string()));
    }
    expected_statuses.sort();

    let written = update_answer(
        &client,
        &workload_id,
        &lens_arn,
        &question_id,
        json!({
            "ChoiceUpdates": Value::Object(choice_updates),
            "Notes": "the answer's own notes",
            "IsApplicable": false,
            "Reason": "OUT_OF_SCOPE",
        }),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case \"members\": the question the lens defines must be answerable, got {written:?}"
    );

    // Each member is read back through the single-answer read, so what is asserted is what
    // the store holds rather than what the write echoed.
    let read = get_answer(&client, &workload_id, &lens_arn, &question_id).await;
    assert_eq!(
        read.status, 200,
        "case \"members\": the recorded answer must read back, got {read:?}"
    );
    for (case, answer) in [
        ("members-written", answer_of(&written)),
        ("members-read-back", answer_of(&read)),
    ] {
        assert_eq!(
            choice_statuses(&answer, "ChoiceAnswers"),
            expected_statuses,
            "case {case:?}: each ChoiceUpdates entry must record its choice's status, got \
             {answer:?}"
        );
        assert_eq!(
            strings(&answer, "SelectedChoices"),
            vec![choices[0].clone()],
            "case {case:?}: the choices in place are the ones recorded SELECTED, got {answer:?}"
        );
        assert_eq!(
            answer.get("IsApplicable"),
            Some(&Value::Bool(false)),
            "case {case:?}: the question must record that it does not apply, got {answer:?}"
        );
        assert_eq!(
            string_member(&answer, "Reason"),
            "OUT_OF_SCOPE",
            "case {case:?}: the question must record why it does not apply, got {answer:?}"
        );
        assert_eq!(
            string_member(&answer, "Notes"),
            "the answer's own notes",
            "case {case:?}: the answer must carry its own notes, got {answer:?}"
        );
        assert_eq!(
            answer.get("Risk").and_then(Value::as_str),
            Some("NOT_APPLICABLE"),
            "case {case:?}: a question that does not apply carries no risk, got {answer:?}"
        );
    }
}

/// What a write to a custom lens's review still refuses: a `QuestionId` that lens's
/// document does not define — including one another lens does — is the modelled 400 naming
/// `QuestionId`, and a `ChoiceId`
/// that is not a choice of the question being answered is a 400; neither records anything.
#[tokio::test]
async fn a_question_an_imported_lens_does_not_define_is_refused() {
    const SHAPE: Shape = &[("governance", 2, 2)];
    const OTHER_SHAPE: Shape = &[("elsewhere", 1, 1)];

    let client = Client::new();
    let lens_arn = import_shape(&client, "refusals", SHAPE).await;
    let other_arn = import_shape(&client, "refusals-other", OTHER_SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "refusals", &lens_arn).await;
    publish(&client, "refusals-other", &other_arn).await;
    let workload_id = workload_reviewing(&client, "refusals", &lens_arn).await;

    let summaries = listed_answers(&client, "refusals", &workload_id, &lens_arn).await;
    let answered = summaries
        .first()
        .cloned()
        .expect("the review must report the questions its lens defines");
    let question_id = string_member(&answered, "QuestionId");
    let other_question = string_member(
        &listed_answers(
            &client,
            "refusals-other",
            &workload_reviewing(&client, "refusals-other", &other_arn).await,
            &other_arn,
        )
        .await
        .first()
        .cloned()
        .expect("the other lens's review must report its own questions"),
        "QuestionId",
    );

    // A `QuestionId` of no lens at all, of the AWS-OFFICIAL catalog, and of ANOTHER custom
    // lens's document: none of them is a question the lens under review asks.
    for unasked in ["not-a-question", FRAMEWORK_QUESTION, other_question.as_str()] {
        let refused = update_answer(
            &client,
            &workload_id,
            &lens_arn,
            unasked,
            json!({ "Notes": "nothing to record" }),
        )
        .await;
        let case = format!("unasked-{unasked}");
        let sentence =
            format!("No question with ID {unasked} was found in the lens with alias {lens_arn}.");
        assert_field_validation(&case, &refused, "QuestionId", &sentence);
        let read = get_answer(&client, &workload_id, &lens_arn, unasked).await;
        assert_field_validation(&format!("{case}-read"), &read, "QuestionId", &sentence);
    }

    // A `ChoiceId` of ANOTHER question of the same document is not a choice of the question
    // being answered.
    let foreign_choice = choice_ids(
        summaries
            .last()
            .expect("the document defines more than one question"),
    )
    .first()
    .cloned()
    .expect("every question of the document offers a choice");
    for (case, body) in [
        (
            "selected-choices",
            json!({ "SelectedChoices": [foreign_choice.clone()] }),
        ),
        (
            "choice-updates",
            json!({ "ChoiceUpdates": { foreign_choice.clone(): { "Status": "SELECTED" } } }),
        ),
    ] {
        let refused = update_answer(&client, &workload_id, &lens_arn, &question_id, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a choice of another question belongs to no answer of this one, got \
             {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
    }
    let unchanged = get_answer(&client, &workload_id, &lens_arn, &question_id).await;
    assert_eq!(
        answer_of(&unchanged).get("Risk").and_then(Value::as_str),
        Some("UNANSWERED"),
        "a refused write must record nothing, got {unchanged:?}"
    );
}

/// The AWS-official half of the lens catalog is written through the same resolution: a
/// question of the framework lens still records its answer and reads back.
#[tokio::test]
async fn an_official_lenss_question_is_still_answerable() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("wl-official-half", "tok-wl-official-half"))
            .await,
        "official-half",
    );

    // The question's own catalog, as a client discovers it.
    let summary = answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, FRAMEWORK_QUESTION).await,
    );
    let choices = choice_ids(&summary);
    assert!(
        !choices.is_empty(),
        "the framework lens's question must offer its best practices, got {summary:?}"
    );

    for selection in SELECTIONS {
        let selected = selection.of(&choices);
        let case = format!("official-{selection:?}");
        let notes = format!("notes for {case}");
        let written = update_answer(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            FRAMEWORK_QUESTION,
            json!({ "SelectedChoices": selected, "Notes": notes }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a question of the official catalog must stay answerable, got \
             {written:?}"
        );
        assert_recorded(&case, &answer_of(&written), &summary, &selected, &notes);
        let read = get_answer(&client, &workload_id, FRAMEWORK_LENS, FRAMEWORK_QUESTION).await;
        assert_recorded(
            &format!("{case}-read-back"),
            &answer_of(&read),
            &summary,
            &selected,
            &notes,
        );
    }
}

/// A review template's review of a published custom lens is answerable by the same rule:
/// `UpdateReviewTemplateAnswer` records the answer to any question the document defines and
/// `GetReviewTemplateAnswer` reads it back.
#[tokio::test]
async fn a_review_templates_imported_lens_review_can_be_answered() {
    const SHAPE: Shape = &[("governance", 1, 2), ("cost", 2, 1)];

    let client = Client::new();
    let lens_arn = published_lens(&client, "template", SHAPE).await;
    let template_arn = review_template(&client, "tmpl-imported-answers", &[&lens_arn]).await;

    let listed = client
        .list_review_template_answers(&template_arn, &lens_arn, "")
        .await;
    assert_eq!(
        listed.status, 200,
        "the template's lens review must be readable, got {listed:?}"
    );
    let summaries = listed
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !summaries.is_empty(),
        "the template's review must report the questions its lens defines, got {listed:?}"
    );

    for (position, summary) in summaries.iter().enumerate() {
        let question_id = string_member(summary, "QuestionId");
        let choices = choice_ids(summary);
        let selection = SELECTIONS[position % SELECTIONS.len()];
        let selected = selection.of(&choices);
        let case = format!("template-{question_id}-{selection:?}");
        let notes = format!("template notes for {question_id}");

        let written = client
            .update_review_template_answer(
                &template_arn,
                &lens_arn,
                &question_id,
                json!({ "SelectedChoices": selected, "Notes": notes }),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a question the lens defines must be answerable, got {written:?}"
        );
        assert_eq!(
            member(&written, "TemplateArn"),
            template_arn.as_str(),
            "case {case:?}: the envelope must name the review template, got {written:?}"
        );
        assert_eq!(
            member(&written, "LensAlias"),
            lens_arn.as_str(),
            "case {case:?}: the envelope must name the lens as sent, got {written:?}"
        );

        let read = client
            .get_review_template_answer(&template_arn, &lens_arn, &question_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the recorded answer must read back, got {read:?}"
        );
        for (case, answer) in [
            (format!("{case}-written"), answer_of(&written)),
            (format!("{case}-read-back"), answer_of(&read)),
        ] {
            assert_eq!(
                string_member(&answer, "QuestionId"),
                question_id,
                "case {case:?}: the answer must name the question it was written for, got \
                 {answer:?}"
            );
            assert_eq!(
                choice_ids(&answer),
                choices,
                "case {case:?}: the answer must report the choice catalog its lens defines, got \
                 {answer:?}"
            );
            assert_eq!(
                strings(&answer, "SelectedChoices"),
                selected,
                "case {case:?}: the choices in place are the ones the request named, in the \
                 lens's own choice order, got {answer:?}"
            );
            assert_eq!(
                string_member(&answer, "Notes"),
                notes,
                "case {case:?}: the answer must carry the notes the request wrote, got {answer:?}"
            );
            assert_eq!(
                answer.get("AnswerStatus").and_then(Value::as_str),
                Some("ANSWERED"),
                "case {case:?}: a question the template has answered is ANSWERED, got {answer:?}"
            );
        }
    }

    // A `QuestionId` the lens's document does not define is still no answer of this
    // template's review.
    let refused = client
        .update_review_template_answer(
            &template_arn,
            &lens_arn,
            FRAMEWORK_QUESTION,
            json!({ "Notes": "nothing to record" }),
        )
        .await;
    assert_field_validation(
        "template-unasked",
        &refused,
        "QuestionId",
        &format!(
            "No question with ID {FRAMEWORK_QUESTION} was found in the lens with alias \
             {lens_arn}."
        ),
    );
}

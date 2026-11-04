// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateReviewTemplateAnswer` and `GetReviewTemplateAnswer`:
//! the answer to ONE question of ONE lens review of ONE review template, written and read
//! back.
//!
//! The rules these pin, over TABLES of inputs rather than one recorded request:
//!
//! * an answer records one `ChoiceStatus` per choice its question asks about:
//!   `SelectedChoices` names exactly the choices in place — every other choice of the
//!   question becomes `UNSELECTED` — a `ChoiceUpdates` entry replaces the whole stored
//!   answer of the choice it keys, and the `SelectedChoices` read back are always the
//!   choices recorded `SELECTED`;
//! * `IsApplicable`, `Reason` and `Notes` each replace the stored value when the request
//!   supplies them and are left as stored when it does not, so the members are written
//!   independently of each other;
//! * recording an answer is what makes a question `ANSWERED`, so the answer read back,
//!   the summary the listing reports and the question roll-ups of the template and its
//!   lens review all move with it — by either spelling of the lens (`LensAlias` is
//!   "either the lens alias … or the lens ARN");
//! * every member a request can write here is optional, so a request that supplies NONE of
//!   them asks for no change: it answers nothing and leaves the question as it stands,
//!   while a member it DOES supply is a write however empty the value it carries;
//! * a `TemplateArn` nothing is stored under is a modelled 404, while a lens the template
//!   does not apply, a `QuestionId` that lens does not ask, a `ChoiceId` the
//!   question does not offer and a value outside a modelled enum's value set are each a
//!   modelled 400 naming the member that carried the value, and nothing is stored.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the state seeding live in the shared
//! `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{
    assert_field_validation, review_template as template, Answer, Client,
};

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

/// The `ReviewTemplateAnswerStatus` values, from
/// `smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`.
const ANSWERED: &str = "ANSWERED";
const UNANSWERED: &str = "UNANSWERED";

/// The whole `ChoiceStatus` value set, from
/// `smithy_query(query="shape", shape="ChoiceStatus")`.
const CHOICE_STATUS_VALUES: [&str; 3] = ["SELECTED", "NOT_APPLICABLE", "UNSELECTED"];

/// Two members of that set this suite names directly.
const SELECTED: &str = "SELECTED";
const UNSELECTED: &str = "UNSELECTED";

/// The whole `AnswerReason` value set, from
/// `smithy_query(query="shape", shape="AnswerReason")`; `ChoiceReason` publishes the same
/// five values (`smithy_query(query="shape", shape="ChoiceReason")`).
const ANSWER_REASON_VALUES: [&str; 5] = [
    "OUT_OF_SCOPE",
    "BUSINESS_PRIORITIES",
    "ARCHITECTURE_CONSTRAINTS",
    "OTHER",
    "NONE",
];

/// The longest `ChoiceNotes` the model accepts: `ChoiceUpdate$Notes` is
/// `@length(max: 250)` (`smithy_query(query="shape", shape="ChoiceUpdate")`).
const CHOICE_NOTES_MAX: usize = 250;

/// The longest `Notes` the model accepts: `Notes` is `@length(max: 2084)`
/// (`smithy_query(query="operation", operation="UpdateReviewTemplateAnswer")`).
const NOTES_MAX: usize = 2084;

/// How many questions the framework lens asks in all, from the framework's published
/// numbering (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6).
const FRAMEWORK_QUESTIONS: i64 = 11 + 11 + 13 + 5 + 11 + 6;

/// The questions this suite answers, each with the `PillarId` of the pillar that asks it:
/// four of the lens's six pillars, and question sets of different sizes (`sus-01` asks
/// about a single best practice, `ops-01` about several). The ids are this emulator's own
/// stable question ids, pinned by `super::question_choice_catalog`.
const QUESTIONS: [(&str, &str); 4] = [
    ("ops-01", "operationalExcellence"),
    ("sec-05", "security"),
    ("cost-11", "costOptimization"),
    ("sus-01", "sustainability"),
];

/// A well-formed `TemplateArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:review-template/[a-f0-9]{32}$`) that no
/// stored template carries.
fn absent_template_arn() -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:review-template/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    )
}

/// The `Answer` of a successful answer, with the envelope members checked: the template as
/// stored and the lens as the REQUEST named it.
fn answer_of(answer: &Answer, template_arn: &str, lens: &str, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("TemplateArn").and_then(Value::as_str),
        Some(template_arn),
        "case {case:?}: TemplateArn must be the template the answer belongs to, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("LensAlias").and_then(Value::as_str),
        Some(lens),
        "case {case:?}: LensAlias must echo the lens as the request named it, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Answer must be present, got {answer:?}"))
}

fn member<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
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

/// The `ChoiceId`s the question offers, in the order its `Choices` catalog reports them —
/// the only place a client discovers them, so every case reads them from there rather
/// than composing one.
fn choice_ids(answer: &Value) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .map(|choice| member(choice, "ChoiceId").unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The answer's `ChoiceAnswers` as a `ChoiceId` -> `ChoiceStatus` table.
fn choice_statuses(answer: &Value) -> BTreeMap<String, String> {
    answer
        .get("ChoiceAnswers")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        member(entry, "ChoiceId").unwrap_or_default().to_string(),
                        member(entry, "Status").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole `ChoiceAnswers` entry of one choice.
fn choice_answer(answer: &Value, choice_id: &str) -> Value {
    answer
        .get("ChoiceAnswers")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| member(entry, "ChoiceId") == Some(choice_id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// Rewrite the `ChoiceAnswers` entry of `choice_id` in an EXPECTED answer the way a
/// `ChoiceUpdate` rewrites the stored one: the status it names, plus only the reason and
/// notes it carries.
fn expect_choice_replaced(
    answer: &mut Value,
    choice_id: &str,
    status: &str,
    reason: Option<&str>,
    notes: Option<&str>,
) {
    let mut entry = json!({ "ChoiceId": choice_id, "Status": status });
    if let Some(reason) = reason {
        entry["Reason"] = json!(reason);
    }
    if let Some(notes) = notes {
        entry["Notes"] = json!(notes);
    }
    *entry_of(answer, choice_id) = entry;
}

/// Rewrite only the STATUS of the `ChoiceAnswers` entry of `choice_id` in an EXPECTED
/// answer, which is all a `SelectedChoices` write touches.
fn expect_choice_status(answer: &mut Value, choice_id: &str, status: &str) {
    entry_of(answer, choice_id)["Status"] = json!(status);
}

fn entry_of<'a>(answer: &'a mut Value, choice_id: &str) -> &'a mut Value {
    answer
        .get_mut("ChoiceAnswers")
        .and_then(Value::as_array_mut)
        .and_then(|entries| {
            entries
                .iter_mut()
                .find(|entry| member(entry, "ChoiceId") == Some(choice_id))
        })
        .unwrap_or_else(|| panic!("the answer must carry a ChoiceAnswers entry for {choice_id:?}"))
}

/// Assert the answer records exactly `selected` as the choices in place: those choices
/// `SELECTED`, every other choice the question offers `UNSELECTED`, and the question
/// answered and applying.
fn assert_records_selection(case: &str, answer: &Value, offered: &[String], selected: &[String]) {
    assert_eq!(
        member(answer, "AnswerStatus"),
        Some(ANSWERED),
        "case {case:?}: an answered question reports {ANSWERED}, got {answer:?}"
    );
    assert_eq!(
        string_list(answer, "SelectedChoices"),
        selected.to_vec(),
        "case {case:?}: expected SelectedChoices {selected:?}, got {answer:?}"
    );
    let expected: BTreeMap<String, String> = offered
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
    let actual = choice_statuses(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected ChoiceAnswers statuses {expected:?}, got {actual:?}"
    );
}

/// The `ReviewTemplateAnswerSummary` `ListReviewTemplateAnswers` reports for one question,
/// read through the pillar filter so the question is always on the first page.
async fn summary_of(client: &Client, template_arn: &str, question: (&str, &str)) -> Value {
    let (question_id, pillar_id) = question;
    let answer = client
        .list_review_template_answers(template_arn, FRAMEWORK_LENS, &format!("PillarId={pillar_id}"))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {question_id:?}: expected ListReviewTemplateAnswers 200, got {answer:?}"
    );
    answer
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| member(summary, "QuestionId") == Some(question_id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The `QuestionCounts` roll-up `GetReviewTemplate` reports for a template, as a sorted
/// bucket table.
async fn template_question_counts(client: &Client, template_arn: &str) -> BTreeMap<String, i64> {
    let answer = client.get_review_template(template_arn).await;
    assert_eq!(
        answer.status, 200,
        "expected GetReviewTemplate 200, got {answer:?}"
    );
    answer
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

/// A question of an applied lens that nothing has answered: the `Choices` the question
/// offers, with the `UNANSWERED` state of its answer asserted on the way.
async fn offered_choices(client: &Client, template_arn: &str, question: (&str, &str)) -> Vec<String> {
    let (question_id, pillar_id) = question;
    let answer = answer_of(
        &client
            .get_review_template_answer(template_arn, FRAMEWORK_LENS, question_id)
            .await,
        template_arn,
        FRAMEWORK_LENS,
        question_id,
    );
    assert_eq!(
        member(&answer, "QuestionId"),
        Some(question_id),
        "case {question_id:?}: the answer must report its own question, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "PillarId"),
        Some(pillar_id),
        "case {question_id:?}: expected PillarId {pillar_id:?}, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "AnswerStatus"),
        Some(UNANSWERED),
        "case {question_id:?}: a question nothing has answered reports {UNANSWERED}, got {answer:?}"
    );
    assert_eq!(
        answer.get("IsApplicable").and_then(Value::as_bool),
        Some(true),
        "case {question_id:?}: an unanswered question applies, got {answer:?}"
    );
    assert!(
        string_list(&answer, "SelectedChoices").is_empty(),
        "case {question_id:?}: an unanswered question selects nothing, got {answer:?}"
    );
    assert!(
        choice_statuses(&answer).is_empty(),
        "case {question_id:?}: an unanswered question speaks about no choice, got {answer:?}"
    );
    assert!(
        member(&answer, "QuestionTitle").is_some_and(|title| !title.is_empty()),
        "case {question_id:?}: the question's title comes from the lens, got {answer:?}"
    );
    let offered = choice_ids(&answer);
    assert!(
        !offered.is_empty(),
        "case {question_id:?}: the question's Choices catalog is what a client selects \
         from, got {answer:?}"
    );
    offered
}

/// `SelectedChoices` replaces the choices in place: the rows differ in HOW MANY of the
/// question's choices they name and WHICH — none of them, the first, every other one and
/// all of them — over four questions of four pillars, whose choice catalogs differ in
/// size. Each recorded answer is read back through `GetReviewTemplateAnswer` and
/// summarised by `ListReviewTemplateAnswers`.
#[tokio::test]
async fn selected_choices_replace_the_choices_in_place() {
    let client = Client::new();
    let template_arn = template(&client, "selected-choices-template", &[FRAMEWORK_LENS]).await;

    for question in QUESTIONS {
        let (question_id, _) = question;
        let offered = offered_choices(&client, &template_arn, question).await;
        let selections: Vec<(&str, Vec<String>)> = vec![
            ("none", Vec::new()),
            ("first", offered[..1].to_vec()),
            ("every-other", offered.iter().step_by(2).cloned().collect()),
            ("all", offered.clone()),
        ];

        for (label, selected) in selections {
            let case = format!("{question_id}/{label}");
            let written = answer_of(
                &client
                    .update_review_template_answer(
                        &template_arn,
                        FRAMEWORK_LENS,
                        question_id,
                        json!({ "SelectedChoices": selected }),
                    )
                    .await,
                &template_arn,
                FRAMEWORK_LENS,
                &case,
            );
            assert_records_selection(&case, &written, &offered, &selected);

            // The answer a later caller reads is the answer the update recorded.
            let read = answer_of(
                &client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
                &template_arn,
                FRAMEWORK_LENS,
                &case,
            );
            assert_eq!(
                read, written,
                "case {case:?}: the answer read back must be the one the update recorded, \
                 got {read:?}"
            );

            // And the summary the listing reports cannot disagree with it.
            let summary = summary_of(&client, &template_arn, question).await;
            assert_eq!(
                member(&summary, "AnswerStatus"),
                Some(ANSWERED),
                "case {case:?}: the summary of an answered question reports {ANSWERED}, \
                 got {summary:?}"
            );
            assert_eq!(
                string_list(&summary, "SelectedChoices"),
                selected,
                "case {case:?}: expected the summary's SelectedChoices {selected:?}, \
                 got {summary:?}"
            );
        }
    }
}

/// A request that supplies one member answers the question even when that member carries
/// no value — `SelectedChoices` naming no choice records the state a newly answered
/// question has: every choice the question offers `UNSELECTED`, nothing selected, the
/// question applying and no reason or notes. A request that supplies NO member asks for no
/// change and answers nothing, so the question is still `UNANSWERED` afterwards. The rows
/// are questions of two pillars whose choice catalogs differ in size.
#[tokio::test]
async fn an_empty_member_answers_the_question_and_no_member_answers_nothing() {
    let client = Client::new();
    let template_arn = template(&client, "empty-update-template", &[FRAMEWORK_LENS]).await;

    for question in [
        ("ops-11", "operationalExcellence"),
        ("sus-01", "sustainability"),
    ] {
        let (question_id, _) = question;
        let offered = offered_choices(&client, &template_arn, question).await;

        // Every member this operation writes is optional, so a request carrying none of
        // them records nothing: the question is answered by the request that answers it.
        let untouched = answer_of(
            &client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({}),
                )
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            question_id,
        );
        assert_eq!(
            member(&untouched, "AnswerStatus"),
            Some(UNANSWERED),
            "case {question_id:?}: an update that supplies no member answers nothing, got \
             {untouched:?}"
        );
        assert_eq!(
            summary_of(&client, &template_arn, question)
                .await
                .get("AnswerStatus")
                .and_then(Value::as_str),
            Some(UNANSWERED),
            "case {question_id:?}: the listing must report the question unanswered too"
        );

        let written = answer_of(
            &client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({ "SelectedChoices": [] }),
                )
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            question_id,
        );
        assert_records_selection(question_id, &written, &offered, &[]);
        assert_eq!(
            written.get("IsApplicable").and_then(Value::as_bool),
            Some(true),
            "case {question_id:?}: a newly answered question applies, got {written:?}"
        );
        assert_eq!(
            member(&written, "Reason"),
            None,
            "case {question_id:?}: a newly answered question carries no reason, got {written:?}"
        );
        assert_eq!(
            member(&written, "Notes"),
            None,
            "case {question_id:?}: a newly answered question carries no notes, got {written:?}"
        );

        let read = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            question_id,
        );
        assert_eq!(
            read, written,
            "case {question_id:?}: the answer read back must be the one the update recorded, \
             got {read:?}"
        );
        let summary = summary_of(&client, &template_arn, question).await;
        assert_eq!(
            member(&summary, "AnswerStatus"),
            Some(ANSWERED),
            "case {question_id:?}: the question is answered in the listing too, got {summary:?}"
        );
    }
}

/// A `ChoiceUpdate` carries one choice's answer in full, so it replaces what was stored
/// for that choice and leaves every other choice of the question alone; the choices
/// recorded `SELECTED` are exactly the `SelectedChoices` read back. The rows are every
/// `ChoiceStatus` the enum publishes, each with and without the reason and the notes a
/// `ChoiceUpdate` may carry — including notes at the published maximum length and the
/// last `ChoiceReason` of the value set.
#[tokio::test]
async fn a_choice_update_replaces_that_choices_answer() {
    let client = Client::new();
    let template_arn = template(&client, "choice-updates-template", &[FRAMEWORK_LENS]).await;
    let question = ("ops-04", "operationalExcellence");
    let (question_id, _) = question;
    let offered = offered_choices(&client, &template_arn, question).await;
    let target = offered[0].clone();
    let long_notes = "n".repeat(CHOICE_NOTES_MAX);

    // (case, the ChoiceUpdate as sent, the reason and notes it records)
    let mut rows: Vec<(String, Value, Option<String>, Option<String>)> = Vec::new();
    for status in CHOICE_STATUS_VALUES {
        rows.push((
            format!("{status}/bare"),
            json!({ "Status": status }),
            None,
            None,
        ));
        rows.push((
            format!("{status}/with-reason-and-notes"),
            json!({ "Status": status, "Reason": ANSWER_REASON_VALUES[4], "Notes": long_notes }),
            Some(ANSWER_REASON_VALUES[4].to_string()),
            Some(long_notes.clone()),
        ));
    }

    for (label, update, reason, notes) in rows {
        let case = format!("{question_id}/{label}");
        let status = member(&update, "Status").unwrap_or_default().to_string();
        let written = answer_of(
            &client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({ "ChoiceUpdates": { target.clone(): update } }),
                )
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            &case,
        );

        let mut expected = json!({ "ChoiceId": target, "Status": status });
        if let Some(reason) = reason.as_deref() {
            expected["Reason"] = json!(reason);
        }
        if let Some(notes) = notes.as_deref() {
            expected["Notes"] = json!(notes);
        }
        let actual = choice_answer(&written, &target);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the choice's answer {expected:?}, got {actual:?}"
        );

        // Every other choice of the question is untouched by an update that does not name
        // it, and the selected choices follow the statuses.
        let statuses = choice_statuses(&written);
        for choice_id in offered.iter().filter(|choice_id| **choice_id != target) {
            assert_eq!(
                statuses.get(choice_id).map(String::as_str),
                Some(UNSELECTED),
                "case {case:?}: choice {choice_id:?} was not named, got {written:?}"
            );
        }
        let expected_selection: Vec<String> = if status == SELECTED {
            vec![target.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            string_list(&written, "SelectedChoices"),
            expected_selection,
            "case {case:?}: SelectedChoices are the choices recorded {SELECTED}, got {written:?}"
        );

        let read = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            &case,
        );
        assert_eq!(
            read, written,
            "case {case:?}: the answer read back must be the one the update recorded, got {read:?}"
        );
    }
}

/// Each member of an answer is written independently: the request's members replace the
/// stored ones and every member it omits is left as stored. The rows are the members of a
/// fully specified answer, updated one at a time from the same baseline — and a request
/// that supplies none of them, which changes nothing.
#[tokio::test]
async fn each_answer_member_is_written_independently() {
    let client = Client::new();
    let template_arn = template(&client, "members-template", &[FRAMEWORK_LENS]).await;
    let question = ("sec-02", "security");
    let (question_id, _) = question;
    let offered = offered_choices(&client, &template_arn, question).await;
    assert!(
        offered.len() >= 2,
        "the question under test must offer at least two choices, got {offered:?}"
    );
    let (first, second) = (offered[0].clone(), offered[1].clone());
    let baseline_notes = "the notes of the baseline answer";

    // A fully specified answer: a choice in place, a choice answered as not applicable
    // with its own reason and notes, and the answer's own applicability, reason and notes.
    let baseline = json!({
        "SelectedChoices": [first.clone()],
        "ChoiceUpdates": {
            second.clone(): {
                "Status": "NOT_APPLICABLE",
                "Reason": "OUT_OF_SCOPE",
                "Notes": "this best practice is out of scope",
            },
        },
        "IsApplicable": true,
        "Reason": "NONE",
        "Notes": baseline_notes,
    });

    let write_baseline = |case: String| {
        let body = baseline.clone();
        let template_arn = template_arn.clone();
        let client = &client;
        async move {
            answer_of(
                &client
                    .update_review_template_answer(
                        &template_arn,
                        FRAMEWORK_LENS,
                        question_id,
                        body,
                    )
                    .await,
                &template_arn,
                FRAMEWORK_LENS,
                &case,
            )
        }
    };

    let recorded = write_baseline("baseline".to_string()).await;
    assert_eq!(
        member(&recorded, "Notes"),
        Some(baseline_notes),
        "case \"baseline\": expected Notes {baseline_notes:?}, got {recorded:?}"
    );
    assert_eq!(
        member(&recorded, "Reason"),
        Some("NONE"),
        "case \"baseline\": expected Reason \"NONE\", got {recorded:?}"
    );
    assert_eq!(
        string_list(&recorded, "SelectedChoices"),
        vec![first.clone()],
        "case \"baseline\": expected SelectedChoices [{first:?}], got {recorded:?}"
    );

    let long_notes = "N".repeat(NOTES_MAX);
    // (case, the request's members, what that changes about the stored answer)
    type Change = Box<dyn Fn(&mut Value)>;
    let rows: Vec<(&str, Value, Change)> = vec![
        (
            "notes-only",
            json!({ "Notes": long_notes.clone() }),
            Box::new({
                let long_notes = long_notes.clone();
                move |expected: &mut Value| expected["Notes"] = json!(long_notes)
            }),
        ),
        (
            "is-applicable-only",
            json!({ "IsApplicable": false }),
            Box::new(|expected: &mut Value| expected["IsApplicable"] = json!(false)),
        ),
        (
            "reason-only",
            json!({ "Reason": ANSWER_REASON_VALUES[2] }),
            Box::new(|expected: &mut Value| expected["Reason"] = json!(ANSWER_REASON_VALUES[2])),
        ),
        (
            "selected-choices-only",
            json!({ "SelectedChoices": [second.clone()] }),
            Box::new({
                let (first, second) = (first.clone(), second.clone());
                move |expected: &mut Value| {
                    // Only each choice's STATUS moves: the reason and notes a
                    // `ChoiceUpdate` wrote for the second choice stay as stored.
                    expected["SelectedChoices"] = json!([second.clone()]);
                    expect_choice_status(expected, &first, UNSELECTED);
                    expect_choice_status(expected, &second, SELECTED);
                }
            }),
        ),
        (
            "choice-updates-only",
            json!({ "ChoiceUpdates": { second.clone(): { "Status": UNSELECTED } } }),
            Box::new({
                let second = second.clone();
                move |expected: &mut Value| {
                    // The update carries that choice's answer in full, so the reason and
                    // notes it leaves out are gone.
                    expect_choice_replaced(expected, &second, UNSELECTED, None, None);
                }
            }),
        ),
        (
            "nothing",
            json!({}),
            Box::new(|_expected: &mut Value| ()),
        ),
    ];

    for (case, patch, change) in rows {
        let restored = write_baseline(format!("{case}/restore")).await;
        assert_eq!(
            restored, recorded,
            "case {case:?}: writing the same answer again must record the same answer, \
             got {restored:?}"
        );

        let mut expected = recorded.clone();
        change(&mut expected);

        let written = answer_of(
            &client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    patch,
                )
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            case,
        );
        assert_eq!(
            written, expected,
            "case {case:?}: expected the answer {expected:?}, got {written:?}"
        );

        let read = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            case,
        );
        assert_eq!(
            read, written,
            "case {case:?}: the answer read back must be the one the update recorded, got {read:?}"
        );
    }
}

/// Recording an answer is what makes a question `ANSWERED`, so the roll-ups the template
/// and its lens review report move with the answers and with nothing else. The rows are
/// questions of three different pillars, answered through the lens ARN and read back
/// through the lens alias — the same lens review either way — and then one of them
/// answered again, which answers no new question.
#[tokio::test]
async fn the_question_rollups_follow_the_recorded_answers() {
    let client = Client::new();
    let template_arn = template(&client, "rollup-template", &[FRAMEWORK_LENS]).await;

    let before = template_question_counts(&client, &template_arn).await;
    assert_eq!(
        before,
        [
            (ANSWERED.to_string(), 0),
            (UNANSWERED.to_string(), FRAMEWORK_QUESTIONS),
        ]
        .into_iter()
        .collect::<BTreeMap<String, i64>>(),
        "a template nothing has answered counts every question {UNANSWERED}, got {before:?}"
    );

    // (case, question, pillar, how many distinct questions are answered afterwards)
    let rows: [(&str, &str, &str, i64); 4] = [
        ("first-answer", "rel-01", "reliability", 1),
        ("second-pillar", "perf-03", "performance", 2),
        ("third-pillar", "sus-02", "sustainability", 3),
        ("answered-again", "rel-01", "reliability", 3),
    ];

    for (case, question_id, pillar_id, answered_questions) in rows {
        // Answered by the lens's AWS-official ARN …
        let written = answer_of(
            &client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS_ARN,
                    question_id,
                    json!({ "Notes": format!("answered for {case}") }),
                )
                .await,
            &template_arn,
            FRAMEWORK_LENS_ARN,
            case,
        );
        assert_eq!(
            member(&written, "AnswerStatus"),
            Some(ANSWERED),
            "case {case:?}: an answered question reports {ANSWERED}, got {written:?}"
        );

        // … and read back by its alias: the same lens review, so the same answer.
        let read = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            case,
        );
        assert_eq!(
            read, written,
            "case {case:?}: the answer is the same by either spelling of the lens, got {read:?}"
        );

        let counts = template_question_counts(&client, &template_arn).await;
        let expected: BTreeMap<String, i64> = [
            (ANSWERED.to_string(), answered_questions),
            (
                UNANSWERED.to_string(),
                FRAMEWORK_QUESTIONS - answered_questions,
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            counts, expected,
            "case {case:?}: expected the template's QuestionCounts {expected:?}, got {counts:?}"
        );

        // The lens review of the template reports the same roll-up over the lens's own
        // questions, and its pillar summary over that pillar's.
        let review = client
            .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
            .await;
        assert_eq!(
            review.status, 200,
            "case {case:?}: expected GetReviewTemplateLensReview 200, got {review:?}"
        );
        let lens_review = review
            .body
            .get("LensReview")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            lens_review
                .get("QuestionCounts")
                .and_then(|counts| counts.get(ANSWERED))
                .and_then(Value::as_i64),
            Some(answered_questions),
            "case {case:?}: the lens review counts the answered questions, got {lens_review:?}"
        );
        let pillar_answered = lens_review
            .get("PillarReviewSummaries")
            .and_then(Value::as_array)
            .and_then(|summaries| {
                summaries
                    .iter()
                    .find(|summary| member(summary, "PillarId") == Some(pillar_id))
                    .cloned()
            })
            .and_then(|summary| {
                summary
                    .get("QuestionCounts")
                    .and_then(|counts| counts.get(ANSWERED))
                    .and_then(Value::as_i64)
            });
        assert_eq!(
            pillar_answered,
            Some(1),
            "case {case:?}: pillar {pillar_id:?} has exactly one answered question, \
             got {lens_review:?}"
        );
    }
}

/// A template nothing is stored under is a modelled 404, and every value the request got
/// wrong a modelled 400 — and neither writes: the question each row names is still
/// `UNANSWERED` afterwards. The rows
/// are a template nothing is stored under, a lens the template does not apply, a lens it
/// applies that this emulator ships no question set for, a question the lens does not ask,
/// a `ChoiceId` the question does not offer through each member that carries one, and a
/// value outside each modelled enum the request can set.
#[tokio::test]
async fn an_unresolvable_or_refused_update_writes_nothing() {
    let client = Client::new();
    let template_arn = template(
        &client,
        "refusals-template",
        &[FRAMEWORK_LENS, UNSHIPPED_LENS],
    )
    .await;
    let question = ("ops-06", "operationalExcellence");
    let (question_id, _) = question;
    let offered = offered_choices(&client, &template_arn, question).await;
    let undefined_choice = format!("{question_id}-bp-99");
    let absent = absent_template_arn();

    // (case, template ARN, lens, question, body, expected status, expected error code)
    let rows: Vec<(&str, String, &str, String, Value, u16, &str)> = vec![
        (
            "absent-template",
            absent.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "Notes": "nothing to answer" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "lens-not-applied",
            template_arn.clone(),
            "arn:aws:wellarchitected:us-east-1::lens/serverless",
            question_id.to_string(),
            json!({ "Notes": "nothing to answer" }),
            400,
            "ValidationException",
        ),
        (
            "lens-without-questions",
            template_arn.clone(),
            UNSHIPPED_LENS,
            question_id.to_string(),
            json!({ "Notes": "nothing to answer" }),
            400,
            "ValidationException",
        ),
        (
            "unknown-question",
            template_arn.clone(),
            FRAMEWORK_LENS,
            "ops-99".to_string(),
            json!({ "Notes": "nothing to answer" }),
            400,
            "ValidationException",
        ),
        (
            "undefined-selected-choice",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "SelectedChoices": [offered[0].clone(), undefined_choice.clone()] }),
            400,
            "ValidationException",
        ),
        (
            "undefined-choice-update",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "ChoiceUpdates": { undefined_choice.clone(): { "Status": SELECTED } } }),
            400,
            "ValidationException",
        ),
        (
            "unknown-answer-reason",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "Reason": "NOT_A_REASON" }),
            400,
            "ValidationException",
        ),
        (
            "unknown-choice-status",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "ChoiceUpdates": { offered[0].clone(): { "Status": "MAYBE" } } }),
            400,
            "ValidationException",
        ),
        (
            "unknown-choice-reason",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({
                "ChoiceUpdates": {
                    offered[0].clone(): { "Status": SELECTED, "Reason": "NOT_A_REASON" },
                },
            }),
            400,
            "ValidationException",
        ),
        (
            // One character past the modelled `Notes` `@length(max: 2084)`, refused at the
            // boundary before the handler runs.
            "notes-past-the-maximum",
            template_arn.clone(),
            FRAMEWORK_LENS,
            question_id.to_string(),
            json!({ "Notes": "N".repeat(NOTES_MAX + 1) }),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, lens, asked, body, status, error_code) in rows {
        let answer = client
            .update_review_template_answer(&arn, lens, &asked, body)
            .await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, error_code,
            "case {case:?}: expected {error_code}, got {answer:?}"
        );

        // Nothing was stored: the question is still unanswered, by the reader and by the
        // template's own roll-up.
        let read = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            case,
        );
        assert_eq!(
            member(&read, "AnswerStatus"),
            Some(UNANSWERED),
            "case {case:?}: a refused update stores nothing, got {read:?}"
        );
        let counts = template_question_counts(&client, &template_arn).await;
        assert_eq!(
            counts.get(ANSWERED).copied(),
            Some(0),
            "case {case:?}: a refused update answers no question, got {counts:?}"
        );
    }
}

/// What an answer that cannot be resolved is refused with: the TEMPLATE is a resource, so
/// an ARN nothing is stored under is the 404 naming it and its type, while the lens review
/// and the lens's question are VALUES of the request, so each is the modelled 400 whose
/// `Fields` names the member that carried it. Each row is read through
/// `GetReviewTemplateAnswer` and written through `UpdateReviewTemplateAnswer`, which
/// resolve them identically.
#[tokio::test]
async fn an_unresolvable_answer_names_the_missing_resource() {
    let client = Client::new();
    let template_arn = template(&client, "missing-template", &[FRAMEWORK_LENS]).await;
    let absent = absent_template_arn();

    for (verb, answer) in [
        (
            "read",
            client
                .get_review_template_answer(&absent, FRAMEWORK_LENS, "ops-01")
                .await,
        ),
        (
            "write",
            client
                .update_review_template_answer(&absent, FRAMEWORK_LENS, "ops-01", json!({}))
                .await,
        ),
    ] {
        let case = format!("absent-template/{verb}");
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unresolvable template is a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some("REVIEW_TEMPLATE"),
            "case {case:?}: expected ResourceType REVIEW_TEMPLATE, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(absent.as_str()),
            "case {case:?}: expected the ARN as sent, got {answer:?}"
        );
    }

    // (case, lens, question, the member the finding names, its sentence)
    let rows: Vec<(&str, &str, &str, &str, String)> = vec![
        (
            "lens-not-applied",
            UNSHIPPED_LENS,
            "ops-01",
            "LensAlias",
            format!(
                "No lens review for lens alias {UNSHIPPED_LENS} was found in review template \
                 {template_arn}."
            ),
        ),
        (
            "unknown-question",
            FRAMEWORK_LENS,
            "ops-99",
            "QuestionId",
            format!(
                "No question with ID ops-99 was found in the lens with alias {FRAMEWORK_LENS}."
            ),
        ),
    ];

    for (case, lens, question_id, named, sentence) in rows {
        for (verb, answer) in [
            (
                "read",
                client
                    .get_review_template_answer(&template_arn, lens, question_id)
                    .await,
            ),
            (
                "write",
                client
                    .update_review_template_answer(&template_arn, lens, question_id, json!({}))
                    .await,
            ),
        ] {
            assert_field_validation(&format!("{case}/{verb}"), &answer, named, &sentence);
        }
    }
}

/// Reading an answer changes nothing a later caller sees: the rows are repeated reads of
/// an unanswered question and of an answered one, each of which must leave the answer and
/// the template's roll-up exactly as they were.
#[tokio::test]
async fn reading_an_answer_writes_nothing() {
    let client = Client::new();
    let template_arn = template(&client, "reporting-template", &[FRAMEWORK_LENS]).await;
    let question = ("cost-02", "costOptimization");
    let (question_id, _) = question;
    let offered = offered_choices(&client, &template_arn, question).await;

    for (case, expected_answered) in [("unanswered", 0i64), ("answered", 1i64)] {
        if case == "answered" {
            let written = client
                .update_review_template_answer(
                    &template_arn,
                    FRAMEWORK_LENS,
                    question_id,
                    json!({ "SelectedChoices": [offered[0].clone()] }),
                )
                .await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected UpdateReviewTemplateAnswer 200, got {written:?}"
            );
        }

        let first = answer_of(
            &client
                .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                .await,
            &template_arn,
            FRAMEWORK_LENS,
            case,
        );
        for repeat in 1..=3 {
            let again = answer_of(
                &client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
                &template_arn,
                FRAMEWORK_LENS,
                case,
            );
            assert_eq!(
                again, first,
                "case {case:?}: read {repeat} must report the same answer, got {again:?}"
            );
        }
        let counts = template_question_counts(&client, &template_arn).await;
        assert_eq!(
            counts.get(ANSWERED).copied(),
            Some(expected_answered),
            "case {case:?}: reading an answer answers nothing, got {counts:?}"
        );
    }
}

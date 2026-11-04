// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAnswer` and the read that proves what it recorded,
//! `GetAnswer`.
//!
//! The rules these pin, over tables of inputs rather than one recorded request:
//!
//! * `SelectedChoices` REPLACES the choices an answer records as in place — a choice it
//!   names is `SELECTED`, a choice it omits is no longer selected — and the answer a read
//!   reports lists exactly those choices, in the lens's own choice order.
//! * each `ChoiceUpdates` entry records its `Status`, `Reason` and `Notes` against the
//!   choice it names, and the answer's selected choices follow those statuses.
//! * an update applies only the members it carries and leaves every member it omits as it
//!   was.
//! * a question's `Risk` is DERIVED from what the answer records on every read, so it
//!   moves when the answer does and every read of the review agrees about it.
//! * a `WorkloadId`, a lens or a `QuestionId` the review does not hold is a 404, a choice
//!   the question does not ask about or a value an enum does not publish is a 400, and
//!   neither records anything.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, encode_path_segment, minimal_create, official_lens_arn, Answer, Client,
};

/// The alias of the one AWS-official lens a workload is created with here.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that the workloads under test do not review, so it names
/// no lens review of theirs to answer in.
const UNSHIPPED_LENS: &str = "mergersandacquisitions";

/// The Region the shared harness signs its requests for, which is the Region the ARNs in
/// a response are built in.
const REQUEST_REGION: &str = "us-west-2";

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The questions this suite answers: one from each of four pillars of the framework
/// lens's published numbering, chosen to differ in HOW MANY best practices they ask about
/// — `sus-01` asks about exactly one, so "every choice" and "the first choice" are the
/// same selection there and "all but the last" is none of them.
///
/// The `ChoiceId`s themselves are never written down: every case reads the question's own
/// catalog (`Answer$Choices`) first, which is where a client discovers them too.
const QUESTIONS: [&str; 4] = ["ops-01", "sec-02", "cost-01", "sus-01"];

/// The `ChoiceStatus` members, as `smithy_query(query="shape", shape="ChoiceStatus")`
/// publishes them: what an answer can record about one best practice.
const CHOICE_STATUSES: [&str; 3] = ["SELECTED", "UNSELECTED", "NOT_APPLICABLE"];

/// The `ChoiceReason` members, as `smithy_query(query="shape", shape="ChoiceReason")`
/// publishes them. `AnswerReason` publishes the same five values
/// (`smithy_query(query="shape", shape="AnswerReason")`), so the rows below span both.
const CHOICE_REASONS: [&str; 5] = [
    "OUT_OF_SCOPE",
    "BUSINESS_PRIORITIES",
    "ARCHITECTURE_CONSTRAINTS",
    "OTHER",
    "NONE",
];

/// `UpdateAnswer`: `PATCH /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
/// Each label is non-greedy, so it travels percent-encoded in one path segment, exactly
/// as an SDK sends it.
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

/// `GetAnswer`: the same path read with `GET`, with the optional `MilestoneNumber` bound
/// to the query string.
async fn get_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    milestone_number: Option<i64>,
) -> Answer {
    let suffix = match milestone_number {
        None => String::new(),
        Some(number) => format!("?MilestoneNumber={number}"),
    };
    client
        .send(
            "GET",
            &format!(
                "/workloads/{}/lensReviews/{}/answers/{}{suffix}",
                encode_path_segment(workload_id),
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// A workload reviewing the framework lens, and its `WorkloadId`.
async fn framework_workload(client: &Client, name: &str) -> String {
    created_id(
        &client
            .create(minimal_create(name, &format!("tok-{name}")))
            .await,
        name,
    )
}

/// The `Answer` an answer read or an answer write reported.
fn answer_of(answer: &Answer) -> Value {
    answer.body.get("Answer").cloned().unwrap_or(Value::Null)
}

/// One list-of-strings member of a JSON object, empty when it is absent.
fn strings(value: &Value, member: &str) -> Vec<String> {
    value
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One string member of a JSON object, `None` when it is absent.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `ChoiceId`s the question asks about, in the order its catalog publishes them.
fn choice_ids(answer: &Value) -> Vec<String> {
    answer
        .get("Choices")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| text(choice, "ChoiceId"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `ChoiceAnswer` the answer reports, as `ChoiceId -> (Status, Reason, Notes)`.
#[allow(clippy::type_complexity)]
fn choice_answers(answer: &Value) -> BTreeMap<String, (String, Option<String>, Option<String>)> {
    answer
        .get("ChoiceAnswers")
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| {
                    Some((
                        text(choice, "ChoiceId")?,
                        (
                            text(choice, "Status").unwrap_or_default(),
                            text(choice, "Reason"),
                            text(choice, "Notes"),
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Risk` a question carries, derived the way the Well-Architected Tool rates one:
/// a question that does not apply is `NOT_APPLICABLE`, one answered with no best practice
/// in place is `HIGH`, one with every best practice in place is `NONE`, and one in between
/// is `MEDIUM`. `UNANSWERED` is what a question with no answer at all reports, so it is
/// not reachable here. The five members are what
/// `smithy_query(query="shape", shape="Risk")` publishes.
fn expected_risk(selected: usize, asks: usize, is_applicable: bool) -> &'static str {
    if !is_applicable {
        "NOT_APPLICABLE"
    } else if selected == 0 {
        "HIGH"
    } else if selected == asks {
        "NONE"
    } else {
        "MEDIUM"
    }
}

/// The `ChoiceId`s of `asks` that `selected` names, in the catalog's own order — the order
/// the answer is expected to report them in, whatever order the request sent them in.
fn in_catalog_order(asks: &[String], selected: &[String]) -> Vec<String> {
    asks.iter()
        .filter(|choice_id| selected.contains(choice_id))
        .cloned()
        .collect()
}

/// `SelectedChoices` replaces the choices an answer records as in place, and every member
/// a read derives from them follows. The rows span the selection sizes a question allows —
/// every choice, all but one, one, and none — and one row sends the selection in reverse
/// order, because the ORDER the answer reports is the lens's, not the request's.
#[tokio::test]
async fn selected_choices_replace_the_best_practices_an_answer_records() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "selected-choices").await;
    let lens_arn = official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS);

    for question_id in QUESTIONS {
        let before = get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await;
        assert_eq!(
            before.status, 200,
            "case {question_id:?}: expected GetAnswer 200 for an unanswered question, got {before:?}"
        );
        let unanswered = answer_of(&before);
        let asks = choice_ids(&unanswered);
        assert!(
            !asks.is_empty(),
            "case {question_id:?}: the question must publish the choices an answer names, got {unanswered:?}"
        );
        assert_eq!(
            text(&unanswered, "Risk").as_deref(),
            Some("UNANSWERED"),
            "case {question_id:?}: a question with no answer must report UNANSWERED, got {unanswered:?}"
        );
        assert!(
            strings(&unanswered, "SelectedChoices").is_empty()
                && choice_answers(&unanswered).is_empty(),
            "case {question_id:?}: a question with no answer records no choice, got {unanswered:?}"
        );

        let rows: [(&str, Vec<String>); 5] = [
            ("every-choice", asks.clone()),
            (
                "all-but-the-last",
                asks[..asks.len().saturating_sub(1)].to_vec(),
            ),
            ("the-first-choice", asks[..1].to_vec()),
            (
                "reverse-order-every-choice",
                asks.iter().rev().cloned().collect(),
            ),
            ("no-choice", Vec::new()),
        ];

        for (row, sent) in rows {
            let case = format!("{question_id}/{row}");
            let recorded = update_answer(
                &client,
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                json!({ "SelectedChoices": sent }),
            )
            .await;
            assert_eq!(
                recorded.status, 200,
                "case {case:?}: expected UpdateAnswer 200, got {recorded:?}"
            );

            for (member, expected) in [
                ("WorkloadId", workload_id.as_str()),
                ("LensAlias", FRAMEWORK_LENS),
                ("LensArn", lens_arn.as_str()),
            ] {
                assert_eq!(
                    text(&recorded.body, member).as_deref(),
                    Some(expected),
                    "case {case:?}: {member} must be {expected:?}, got {:?}",
                    recorded.body
                );
            }

            let answer = answer_of(&recorded);
            let expected_selection = in_catalog_order(&asks, &sent);
            assert_eq!(
                strings(&answer, "SelectedChoices"),
                expected_selection,
                "case {case:?}: the answer must record {expected_selection:?} as in place, got {answer:?}"
            );
            let expected_statuses: BTreeMap<String, String> = asks
                .iter()
                .map(|choice_id| {
                    let status = if expected_selection.contains(choice_id) {
                        "SELECTED"
                    } else {
                        "UNSELECTED"
                    };
                    (choice_id.clone(), status.to_string())
                })
                .collect();
            let actual_statuses: BTreeMap<String, String> = choice_answers(&answer)
                .into_iter()
                .map(|(choice_id, (status, _, _))| (choice_id, status))
                .collect();
            assert_eq!(
                actual_statuses, expected_statuses,
                "case {case:?}: the answer must report every choice of the question once, with \
                 the selected ones SELECTED, expected {expected_statuses:?}, got {actual_statuses:?}"
            );
            let risk = expected_risk(expected_selection.len(), asks.len(), true);
            assert_eq!(
                text(&answer, "Risk").as_deref(),
                Some(risk),
                "case {case:?}: {} of {} best practices in place must rate {risk}, got {answer:?}",
                expected_selection.len(),
                asks.len()
            );
            assert_eq!(
                answer.get("IsApplicable"),
                Some(&Value::Bool(true)),
                "case {case:?}: a question nothing said was inapplicable still applies, got {answer:?}"
            );

            // The write and the read report the SAME answer: the read is what proves the
            // write was persisted rather than echoed.
            let read = get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected GetAnswer 200 after the update, got {read:?}"
            );
            assert_eq!(
                answer_of(&read),
                answer,
                "case {case:?}: GetAnswer must report the answer UpdateAnswer recorded, got {read:?}"
            );
        }
    }
}

/// Each `ChoiceUpdates` entry records its own status, reason and notes against the choice
/// it names, and the answer's selected choices follow those statuses. The rows cross every
/// `ChoiceStatus` the enum publishes with every `ChoiceReason` it publishes.
#[tokio::test]
async fn a_choice_update_records_the_status_reason_and_notes_of_its_choice() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "choice-updates").await;
    let question_id = "ops-01";

    let asks = choice_ids(&answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await,
    ));
    assert!(
        asks.len() >= 2,
        "the question under test must ask about at least two best practices, got {asks:?}"
    );
    let under_test = asks[1].clone();

    for status in CHOICE_STATUSES {
        for reason in CHOICE_REASONS {
            let case = format!("{status}/{reason}");
            let notes = format!("choice notes for {case}");
            let recorded = update_answer(
                &client,
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                json!({
                    "ChoiceUpdates": {
                        under_test.clone(): {
                            "Status": status,
                            "Reason": reason,
                            "Notes": notes.clone(),
                        }
                    }
                }),
            )
            .await;
            assert_eq!(
                recorded.status, 200,
                "case {case:?}: expected UpdateAnswer 200, got {recorded:?}"
            );

            let answer = answer_of(&recorded);
            let answered = choice_answers(&answer);
            let actual = answered.get(&under_test).cloned();
            let expected = Some((
                status.to_string(),
                Some(reason.to_string()),
                Some(notes.clone()),
            ));
            assert_eq!(
                actual, expected,
                "case {case:?}: the updated choice must record {expected:?}, got {actual:?}"
            );
            assert_eq!(
                answered.len(),
                asks.len(),
                "case {case:?}: updating one choice must leave the question's other choices \
                 answered too, got {answered:?}"
            );

            let expected_selection: Vec<String> = if status == "SELECTED" {
                vec![under_test.clone()]
            } else {
                Vec::new()
            };
            assert_eq!(
                strings(&answer, "SelectedChoices"),
                expected_selection,
                "case {case:?}: the selected choices must be the ones recorded SELECTED, got {answer:?}"
            );
            let risk = expected_risk(expected_selection.len(), asks.len(), true);
            assert_eq!(
                text(&answer, "Risk").as_deref(),
                Some(risk),
                "case {case:?}: the answer must rate {risk}, got {answer:?}"
            );

            let read = get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await;
            assert_eq!(
                answer_of(&read),
                answer,
                "case {case:?}: GetAnswer must report the answer UpdateAnswer recorded, got {read:?}"
            );
        }
    }
}

/// One step of the partial-update walk: the request to send, and what the WHOLE answer
/// must read as once it has been applied.
struct Step {
    case: &'static str,
    body: Value,
    selection: Vec<String>,
    notes: Option<&'static str>,
    reason: Option<&'static str>,
    is_applicable: bool,
}

/// An update applies the members it carries and leaves the ones it omits as they were.
/// The rows walk one answer through a sequence of partial updates, each naming a different
/// subset of the members, and state what the whole answer must read as afterwards.
#[tokio::test]
async fn an_update_applies_only_the_members_it_carries() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "partial-updates").await;
    let question_id = "sec-02";

    let asks = choice_ids(&answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await,
    ));
    assert!(
        asks.len() >= 2,
        "the question under test must ask about at least two best practices, got {asks:?}"
    );

    let one = vec![asks[0].clone()];
    let two = vec![asks[0].clone(), asks[1].clone()];

    let rows = [
        Step {
            case: "first-answer",
            body: json!({
                "SelectedChoices": one.clone(),
                "Notes": "the first note",
                "IsApplicable": true,
            }),
            selection: one.clone(),
            notes: Some("the first note"),
            reason: None,
            is_applicable: true,
        },
        Step {
            case: "notes-only",
            body: json!({ "Notes": "the second note" }),
            selection: one.clone(),
            notes: Some("the second note"),
            reason: None,
            is_applicable: true,
        },
        Step {
            case: "selection-only",
            body: json!({ "SelectedChoices": two.clone() }),
            selection: two.clone(),
            notes: Some("the second note"),
            reason: None,
            is_applicable: true,
        },
        Step {
            case: "not-applicable",
            body: json!({ "IsApplicable": false, "Reason": "OUT_OF_SCOPE" }),
            selection: two.clone(),
            notes: Some("the second note"),
            reason: Some("OUT_OF_SCOPE"),
            is_applicable: false,
        },
        Step {
            case: "applicable-again",
            body: json!({ "IsApplicable": true }),
            selection: two.clone(),
            notes: Some("the second note"),
            // Omitted, so left as it was: a caller clears the reason by sending the
            // `AnswerReason` member for "no reason" rather than by leaving it out.
            reason: Some("OUT_OF_SCOPE"),
            is_applicable: true,
        },
    ];

    for Step {
        case,
        body,
        selection,
        notes,
        reason,
        is_applicable,
    } in rows
    {
        let recorded =
            update_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, body).await;
        assert_eq!(
            recorded.status, 200,
            "case {case:?}: expected UpdateAnswer 200, got {recorded:?}"
        );
        let answer = answer_of(&recorded);
        assert_eq!(
            strings(&answer, "SelectedChoices"),
            selection,
            "case {case:?}: the answer must still record {selection:?} as in place, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "Notes").as_deref(),
            notes,
            "case {case:?}: Notes must be {notes:?}, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "Reason").as_deref(),
            reason,
            "case {case:?}: Reason must be {reason:?}, got {answer:?}"
        );
        assert_eq!(
            answer.get("IsApplicable"),
            Some(&Value::Bool(is_applicable)),
            "case {case:?}: IsApplicable must be {is_applicable}, got {answer:?}"
        );
        let risk = expected_risk(selection.len(), asks.len(), is_applicable);
        assert_eq!(
            text(&answer, "Risk").as_deref(),
            Some(risk),
            "case {case:?}: the answer must rate {risk}, got {answer:?}"
        );
        let read = get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await;
        assert_eq!(
            answer_of(&read),
            answer,
            "case {case:?}: GetAnswer must report the answer UpdateAnswer recorded, got {read:?}"
        );
    }
}

/// One refused request: what it names, and the refusal it must answer with.
struct Refusal {
    case: &'static str,
    workload_id: String,
    lens: &'static str,
    question_id: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
    /// The member a 400 has to name, `None` for a lookup that resolved nothing.
    member: Option<&'static str>,
}

/// What an update that names something the review does not hold is refused with, and that
/// a refusal records nothing. The rows span each identifier the request resolves and each
/// value class the handler enforces: the workload is RESOLVED, so an id nothing is stored
/// under is a 404, while every other value — the lens review, the question, a choice, an
/// enum member — is a value of the request, so it is a 400 naming the member.
#[tokio::test]
async fn an_update_naming_nothing_the_review_holds_is_refused() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "refused-updates").await;
    let question_id = "ops-01";

    let asks = choice_ids(&answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await,
    ));
    let other_question = choice_ids(&answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, "sus-01", None).await,
    ));
    let absent_choice = format!("{question_id}-bp-99");

    let rows = [
        Refusal {
            case: "absent-workload",
            workload_id: ABSENT_HEX_ID.to_string(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({}),
            status: 404,
            code: "ResourceNotFoundException",
            member: None,
        },
        Refusal {
            case: "lens-the-workload-does-not-review",
            workload_id: workload_id.clone(),
            lens: UNSHIPPED_LENS,
            question_id,
            body: json!({}),
            status: 400,
            code: "ValidationException",
            member: Some("LensAlias"),
        },
        Refusal {
            case: "question-the-lens-does-not-ask",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id: "ops-99",
            body: json!({}),
            status: 400,
            code: "ValidationException",
            member: Some("QuestionId"),
        },
        Refusal {
            case: "choice-no-question-asks",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "SelectedChoices": [absent_choice.clone()] }),
            status: 400,
            code: "ValidationException",
            member: Some("SelectedChoices"),
        },
        Refusal {
            case: "choice-of-another-question",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "SelectedChoices": [other_question[0].clone()] }),
            status: 400,
            code: "ValidationException",
            member: Some("SelectedChoices"),
        },
        Refusal {
            case: "choice-update-of-an-unasked-choice",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "ChoiceUpdates": { absent_choice.clone(): { "Status": "SELECTED" } } }),
            status: 400,
            code: "ValidationException",
            member: Some("ChoiceUpdates"),
        },
        Refusal {
            case: "reason-the-enum-does-not-publish",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "Reason": "PERHAPS" }),
            status: 400,
            code: "ValidationException",
            member: Some("Reason"),
        },
        Refusal {
            case: "choice-status-the-enum-does-not-publish",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "ChoiceUpdates": { asks[0].clone(): { "Status": "PERHAPS" } } }),
            status: 400,
            code: "ValidationException",
            member: Some("ChoiceUpdates"),
        },
        Refusal {
            case: "choice-update-without-the-required-status",
            workload_id: workload_id.clone(),
            lens: FRAMEWORK_LENS,
            question_id,
            body: json!({ "ChoiceUpdates": { asks[0].clone(): { "Reason": "NONE" } } }),
            status: 400,
            code: "ValidationException",
            member: Some("Status"),
        },
    ];

    for row in rows {
        let Refusal {
            case,
            workload_id,
            lens,
            question_id,
            body,
            status,
            code,
            member,
        } = row;
        let refused = update_answer(&client, &workload_id, lens, question_id, body).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, code,
            "case {case:?}: expected {code}, got {refused:?}"
        );
        if let Some(member) = member {
            let fields: Vec<String> = refused
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| text(field, "Name"))
                        .collect()
                })
                .unwrap_or_default();
            let message = text(&refused.body, "Message").unwrap_or_default();
            // The refusal has to name the member the caller must fix. A member the
            // framework's own constraint walk refuses states it in the message; one the
            // handler refuses also reports it as a `Fields` entry.
            let named = fields.iter().any(|name| name.contains(member))
                || message.to_lowercase().contains(&member.to_lowercase());
            assert!(
                named,
                "case {case:?}: the refusal must name {member}, got {fields:?} in {refused:?}"
            );
        }
    }

    // Nothing above was recorded: the question the refused requests named is still
    // unanswered.
    let read = get_answer(&client, &workload_id, FRAMEWORK_LENS, "ops-01", None).await;
    let answer = answer_of(&read);
    assert_eq!(
        text(&answer, "Risk").as_deref(),
        Some("UNANSWERED"),
        "a refused update records nothing, so the question must still be unanswered, got {read:?}"
    );
}

/// Every read of the review reports the one recorded answer: the single-answer read by
/// either name of the lens, the answer list, the workload's risk roll-up — and a read of
/// it leaves it as it was. A milestone recorded before a later update still reports the
/// answer it froze.
#[tokio::test]
async fn a_recorded_answer_is_what_every_read_of_the_review_reports() {
    let client = Client::new();
    let workload_id = framework_workload(&client, "answer-reads").await;
    let question_id = "cost-01";
    let lens_arn = official_lens_arn(REQUEST_REGION, FRAMEWORK_LENS);

    let asks = choice_ids(&answer_of(
        &get_answer(&client, &workload_id, FRAMEWORK_LENS, question_id, None).await,
    ));
    let recorded = update_answer(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        question_id,
        json!({ "SelectedChoices": asks.clone(), "Notes": "every best practice is in place" }),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "case \"answer\": expected UpdateAnswer 200, got {recorded:?}"
    );
    let answered = answer_of(&recorded);

    // A lens is ONE lens however the request names it, so the answer is found by its ARN
    // too — and the read answers with what it read, twice over.
    for (case, lens) in [("by-alias", FRAMEWORK_LENS), ("by-arn", lens_arn.as_str())] {
        for pass in ["first", "second"] {
            let read = get_answer(&client, &workload_id, lens, question_id, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}/{pass:?}: expected GetAnswer 200, got {read:?}"
            );
            assert_eq!(
                answer_of(&read),
                answered,
                "case {case:?}/{pass:?}: GetAnswer must report the recorded answer, got {read:?}"
            );
            assert_eq!(
                text(&read.body, "LensArn").as_deref(),
                Some(lens_arn.as_str()),
                "case {case:?}/{pass:?}: the lens must resolve to {lens_arn}, got {read:?}"
            );
            assert_eq!(
                read.body.get("MilestoneNumber"),
                None,
                "case {case:?}/{pass:?}: a read of the live review names no milestone, got {read:?}"
            );
        }
    }

    // The answer list of the same review reports the same selection and the same risk.
    let listed = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/lensReviews/{FRAMEWORK_LENS}/answers"),
            None,
        )
        .await;
    let summary = listed
        .body
        .get("AnswerSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| text(summary, "QuestionId").as_deref() == Some(question_id))
                .cloned()
        })
        .unwrap_or(Value::Null);
    assert_eq!(
        strings(&summary, "SelectedChoices"),
        asks,
        "the answer list must report the recorded selection, got {summary:?}"
    );
    assert_eq!(
        text(&summary, "Risk").as_deref(),
        Some("NONE"),
        "the answer list must report the derived risk, got {summary:?}"
    );

    // The workload's roll-up counts the answered question under that same risk.
    let counts = risk_counts(&client, &workload_id).await;
    assert_eq!(
        counts.get("NONE").copied(),
        Some(1),
        "the roll-up must count the one answered question under NONE, got {counts:?}"
    );

    // A milestone records the workload at an instant, answers included: a later update
    // moves the live review and leaves the milestone's answer where it was.
    let milestone = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({
                "MilestoneName": "every-best-practice-in-place",
                "ClientRequestToken": "tok-answer-milestone",
            })),
        )
        .await;
    assert_eq!(
        milestone.status, 200,
        "case \"milestone\": CreateMilestone must succeed, got {milestone:?}"
    );
    let number = milestone
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    let regressed = update_answer(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        question_id,
        json!({ "SelectedChoices": [] }),
    )
    .await;
    assert_eq!(
        regressed.status, 200,
        "case \"regress\": expected UpdateAnswer 200, got {regressed:?}"
    );

    for (case, milestone_number, expected_selection, risk) in [
        ("live", None, Vec::new(), "HIGH"),
        ("milestone", Some(number), asks.clone(), "NONE"),
    ] {
        let read = get_answer(
            &client,
            &workload_id,
            FRAMEWORK_LENS,
            question_id,
            milestone_number,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAnswer 200, got {read:?}"
        );
        let answer = answer_of(&read);
        assert_eq!(
            strings(&answer, "SelectedChoices"),
            expected_selection,
            "case {case:?}: expected {expected_selection:?} in place, got {answer:?}"
        );
        assert_eq!(
            text(&answer, "Risk").as_deref(),
            Some(risk),
            "case {case:?}: expected {risk}, got {answer:?}"
        );
        assert_eq!(
            read.body.get("MilestoneNumber").and_then(Value::as_i64),
            milestone_number,
            "case {case:?}: a read must name the milestone it was scoped to, got {read:?}"
        );
    }

    // A milestone number the workload never recorded names no read.
    let absent = get_answer(
        &client,
        &workload_id,
        FRAMEWORK_LENS,
        question_id,
        Some(number + 1),
    )
    .await;
    assert_eq!(
        absent.status, 404,
        "case \"absent-milestone\": expected HTTP 404, got {absent:?}"
    );
    assert_eq!(
        absent.error_code, "ResourceNotFoundException",
        "case \"absent-milestone\": expected ResourceNotFoundException, got {absent:?}"
    );
}

/// The `RiskCounts` `GetWorkload` reports for a workload, as a sorted map.
async fn risk_counts(client: &Client, workload_id: &str) -> BTreeMap<String, i64> {
    client
        .get(workload_id)
        .await
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

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE profile template this service publishes.
//!
//! The rules these pin, over the WHOLE published table rather than one row:
//!
//! 1. `GetProfileTemplate` reports the published template — its name, the questions it
//!    asks in the template's own order, and the instants the revision of that content was
//!    published — and every value satisfies the bounds the modelled shapes publish, with
//!    each question's selection bounds reachable within the choices it offers.
//! 2. It is a REPORT: a second read, and a read taken after a profile has been created,
//!    answers exactly the same template.
//! 3. The ids it publishes are what a profile's answers name: for EVERY question the
//!    template asks, a profile answering it with the choices that question offers
//!    round-trips through `GetProfile`, which reports the question's published title,
//!    description, choice catalog and selection bounds beside the selection stored for it.
//! 4. The ids it publishes are also what a profile's answers are RESOLVED against: a
//!    `QuestionId` the template does not ask, and a `SelectedChoiceIds` entry the named
//!    question does not offer, are refused `400 ValidationException` rather than stored,
//!    so a stored profile never answers a question nothing asks or selects a choice its
//!    own published catalog does not carry.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::profile_lifecycle::{created, profile_of};
use super::workload_harness::Client;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The published bounds of the template and of the questions it asks, from
/// `smithy_query(query="shape",
/// shape="ProfileTemplate,ProfileTemplateQuestion,ProfileTemplateChoice")`:
/// `TemplateName` is a `ProfileName` (`@length(3, 100)`), `QuestionId` `@length(1, 128)`,
/// `QuestionTitle` `@length(1, 512)`, `QuestionDescription` `@length(1, 1024)`,
/// `ChoiceId` `@length(1, 64)`, `ChoiceTitle` `@length(1, 512)` and `ChoiceDescription`
/// `@length(1, 1024)`.
const TEMPLATE_NAME_LENGTH: (usize, usize) = (3, 100);
/// See [`TEMPLATE_NAME_LENGTH`].
const QUESTION_ID_LENGTH: (usize, usize) = (1, 128);
/// See [`TEMPLATE_NAME_LENGTH`].
const QUESTION_TITLE_LENGTH: (usize, usize) = (1, 512);
/// See [`TEMPLATE_NAME_LENGTH`].
const QUESTION_DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);
/// See [`TEMPLATE_NAME_LENGTH`].
const CHOICE_ID_LENGTH: (usize, usize) = (1, 64);
/// See [`TEMPLATE_NAME_LENGTH`].
const CHOICE_TITLE_LENGTH: (usize, usize) = (1, 512);
/// See [`TEMPLATE_NAME_LENGTH`].
const CHOICE_DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);

fn member<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// The `ProfileTemplate` one read answered with, checked to be present.
async fn published_template(client: &Client, case: &str) -> Value {
    let answer = client.get_profile_template().await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetProfileTemplate 200, got {answer:?}"
    );
    answer
        .body
        .get("ProfileTemplate")
        .cloned()
        .unwrap_or(Value::Null)
}

fn template_questions(template: &Value) -> Vec<Value> {
    template
        .get("TemplateQuestions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn choices_of(question: &Value) -> Vec<Value> {
    question
        .get("QuestionChoices")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn selection_bounds(question: &Value) -> (usize, usize) {
    let min = question
        .get("MinSelectedChoices")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let max = question
        .get("MaxSelectedChoices")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    (
        usize::try_from(min).unwrap_or_default(),
        usize::try_from(max).unwrap_or_default(),
    )
}

/// A `CreateProfile` request answering exactly `questions`, under a name and token of its
/// own so one client can create every case's profile.
fn create_request(name: &str, questions: Value) -> Value {
    json!({
        "ProfileName": name,
        "ProfileDescription": "a profile answering the published template",
        "ProfileQuestions": questions,
        "ClientRequestToken": format!("tok-{name}"),
    })
}

/// Rule 1. Every question the template publishes is checked, so the table is the published
/// catalog itself rather than one question named here: a question added to it is covered by
/// construction.
#[tokio::test]
async fn the_published_profile_template_satisfies_every_modelled_bound() {
    let client = Client::new();
    let template = published_template(&client, "published").await;

    let name = member(&template, "TemplateName");
    assert!(
        (TEMPLATE_NAME_LENGTH.0..=TEMPLATE_NAME_LENGTH.1).contains(&name.len()),
        "case \"published\": TemplateName is @length{TEMPLATE_NAME_LENGTH:?}, got {name:?}"
    );

    // `CreatedAt`/`UpdatedAt` are when the template was created and last updated, and for
    // content the service publishes that is when the revision a caller reads was
    // published — so they name midnight of the day that revision names, and nothing has
    // revised what the service published.
    let created_at = template.get("CreatedAt").and_then(Value::as_f64);
    let updated_at = template.get("UpdatedAt").and_then(Value::as_f64);
    assert_eq!(
        created_at, updated_at,
        "case \"published\": nothing revises the published template, so CreatedAt must equal \
         UpdatedAt, got {template:?}"
    );
    let rendered = created_at
        .map(::aws_smithy_types::DateTime::from_secs_f64)
        .and_then(|at| at.fmt(::aws_smithy_types::date_time::Format::DateTime).ok())
        .unwrap_or_default();
    assert_eq!(
        rendered,
        format!("{}T00:00:00Z", state::PROFILE_TEMPLATE_REVISION),
        "case \"published\": CreatedAt must be midnight of the day the published revision \
         names, got {rendered:?}"
    );

    let questions = template_questions(&template);
    assert!(
        !questions.is_empty(),
        "case \"published\": the template a client reads to discover its questions must \
         publish some, got {template:?}"
    );

    let mut seen_questions: BTreeSet<String> = BTreeSet::new();
    for question in &questions {
        let question_id = member(question, "QuestionId").to_string();
        let case = question_id.clone();
        assert!(
            (QUESTION_ID_LENGTH.0..=QUESTION_ID_LENGTH.1).contains(&question_id.len()),
            "case {case:?}: QuestionId is @length{QUESTION_ID_LENGTH:?}, got {question_id:?}"
        );
        assert!(
            seen_questions.insert(question_id.clone()),
            "case {case:?}: a profile records one answer per question, so a template asks \
             each question once, got {questions:?}"
        );

        let title = member(question, "QuestionTitle");
        assert!(
            (QUESTION_TITLE_LENGTH.0..=QUESTION_TITLE_LENGTH.1).contains(&title.len()),
            "case {case:?}: QuestionTitle is @length{QUESTION_TITLE_LENGTH:?}, got {title:?}"
        );
        let description = member(question, "QuestionDescription");
        assert!(
            (QUESTION_DESCRIPTION_LENGTH.0..=QUESTION_DESCRIPTION_LENGTH.1)
                .contains(&description.len()),
            "case {case:?}: QuestionDescription is @length{QUESTION_DESCRIPTION_LENGTH:?}, got \
             {description:?}"
        );

        let choices = choices_of(question);
        assert!(
            !choices.is_empty(),
            "case {case:?}: a question a profile answers by selecting choices must offer \
             some, got {question:?}"
        );

        let mut seen_choices: BTreeSet<String> = BTreeSet::new();
        for choice in &choices {
            let choice_id = member(choice, "ChoiceId").to_string();
            assert!(
                (CHOICE_ID_LENGTH.0..=CHOICE_ID_LENGTH.1).contains(&choice_id.len()),
                "case {case:?}: ChoiceId is @length{CHOICE_ID_LENGTH:?}, got {choice_id:?}"
            );
            assert!(
                seen_choices.insert(choice_id.clone()),
                "case {case:?}: a selection names a choice of this question, so each is \
                 offered once, got {choices:?}"
            );
            let choice_title = member(choice, "ChoiceTitle");
            assert!(
                (CHOICE_TITLE_LENGTH.0..=CHOICE_TITLE_LENGTH.1).contains(&choice_title.len()),
                "case {case:?}: ChoiceTitle is @length{CHOICE_TITLE_LENGTH:?}, got \
                 {choice_title:?}"
            );
            let choice_description = member(choice, "ChoiceDescription");
            assert!(
                (CHOICE_DESCRIPTION_LENGTH.0..=CHOICE_DESCRIPTION_LENGTH.1)
                    .contains(&choice_description.len()),
                "case {case:?}: ChoiceDescription is \
                 @length{CHOICE_DESCRIPTION_LENGTH:?}, got {choice_description:?}"
            );
        }

        // `MinSelectedChoices`/`MaxSelectedChoices` are `@range(min: 0)`, and they bound a
        // selection of THIS question's choices: a question whose minimum exceeds its
        // maximum, or whose maximum exceeds the number of choices it offers, could not be
        // answered at all.
        let (min, max) = selection_bounds(question);
        assert!(
            min <= max && max <= choices.len() && max >= 1,
            "case {case:?}: the selection bounds must be reachable within the {} choices \
             offered, got min {min} and max {max}",
            choices.len()
        );
    }
}

/// Rule 2. A read of published content is a REPORT: it answers the same template to every
/// caller, and a profile created in between changes nothing about it.
#[tokio::test]
async fn reading_the_profile_template_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let first = published_template(&client, "first").await;

    let questions = template_questions(&first);
    let answered = questions
        .first()
        .cloned()
        .expect("the template publishes at least one question");
    let choice = choices_of(&answered)
        .first()
        .map(|choice| member(choice, "ChoiceId").to_string())
        .expect("a published question offers at least one choice");
    let answer = client
        .create_profile(create_request(
            "template-read-is-a-report",
            json!([{
                "QuestionId": member(&answered, "QuestionId"),
                "SelectedChoiceIds": [choice],
            }]),
        ))
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"answered\": a profile answering a published question must be created, got \
         {answer:?}"
    );

    let again = published_template(&client, "re-read").await;
    assert_eq!(
        again, first,
        "case \"re-read\": the published template must not change what a later caller sees"
    );
}

/// Rule 3. The rows are every question the template publishes, crossed with the selection
/// sizes its own bounds allow — the fewest choices it accepts and the most — because what
/// a profile reports about a question is read off the template while the SELECTION is the
/// profile's own.
#[tokio::test]
async fn a_discovered_question_round_trips_through_a_created_profile() {
    let client = Client::new();
    let template = published_template(&client, "published").await;

    for question in template_questions(&template) {
        let question_id = member(&question, "QuestionId").to_string();
        let offered: Vec<String> = choices_of(&question)
            .iter()
            .map(|choice| member(choice, "ChoiceId").to_string())
            .collect();
        let (min, max) = selection_bounds(&question);

        for (size, label) in [(min, "fewest"), (max, "most")] {
            let case = format!("{question_id}/{label}");
            let selection: Vec<String> = offered.iter().take(size).cloned().collect();
            assert_eq!(
                selection.len(),
                size,
                "case {case:?}: the question must offer as many choices as its bounds allow, \
                 got {offered:?}"
            );

            // The profile name is @length(3, 100), so it is the case's own short label.
            let name = format!("p-{label}-{}", question_id.replace("profile-", ""));
            let answer = client
                .create_profile(create_request(
                    &name,
                    json!([{
                        "QuestionId": question_id,
                        "SelectedChoiceIds": selection,
                    }]),
                ))
                .await;
            let (arn, _version) = created(&answer, &case);

            let read = client.get_profile(&arn, None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected GetProfile 200, got {read:?}"
            );
            let recorded = profile_of(&read)
                .get("ProfileQuestions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            // The stored answer: the question answered, and the choices selected for it as
            // a set — so the profile reports the selection it recorded, in question-id
            // order within the set.
            let mut expected_selection = selection.clone();
            expected_selection.sort();
            assert_eq!(
                recorded,
                vec![json!({
                    "QuestionId": question_id,
                    "SelectedChoiceIds": expected_selection,
                    "QuestionTitle": question.get("QuestionTitle"),
                    "QuestionDescription": question.get("QuestionDescription"),
                    "QuestionChoices": question.get("QuestionChoices"),
                    "MinSelectedChoices": question.get("MinSelectedChoices"),
                    "MaxSelectedChoices": question.get("MaxSelectedChoices"),
                })],
                "case {case:?}: the profile must report the stored selection beside the \
                 question's published text, got {recorded:?}"
            );
        }
    }
}

/// Rule 4. The complement of rule 3: an identifier the published template does not name
/// belongs to no answer a profile can record, so it is refused rather than stored. The
/// rows differ along WHICH identifier is unpublished and along what it looks like —
/// including one that only resembles a published id, and a choice that is a choice of
/// ANOTHER published question — because what decides this is whether the template names
/// it where the request put it.
#[tokio::test]
async fn an_identifier_the_template_does_not_publish_is_refused() {
    let client = Client::new();
    let template = published_template(&client, "published").await;
    let questions = template_questions(&template);
    let published: BTreeSet<String> = questions
        .iter()
        .map(|question| member(question, "QuestionId").to_string())
        .collect();
    let first = questions
        .first()
        .cloned()
        .expect("the template publishes at least one question");
    let first_id = member(&first, "QuestionId").to_string();
    let offered: BTreeSet<String> = choices_of(&first)
        .iter()
        .map(|choice| member(choice, "ChoiceId").to_string())
        .collect();

    // (case, the one ProfileQuestions entry the request carries)
    let mut rows: Vec<(String, Value)> = vec![
        (
            "a-question-id-of-its-own".to_string(),
            json!({ "QuestionId": "not-a-template-question" }),
        ),
        (
            "an-uppercased-published-question-id".to_string(),
            json!({ "QuestionId": first_id.to_uppercase() }),
        ),
        (
            "a-published-question-id-with-a-suffix".to_string(),
            json!({ "QuestionId": format!("{first_id}-2") }),
        ),
        (
            "a-question-id-at-the-published-maximum-length".to_string(),
            json!({ "QuestionId": "q".repeat(QUESTION_ID_LENGTH.1) }),
        ),
        (
            "a-choice-of-no-question".to_string(),
            json!({
                "QuestionId": first_id,
                "SelectedChoiceIds": ["a-choice-of-its-own"],
            }),
        ),
        (
            "an-uppercased-published-choice-id".to_string(),
            json!({
                "QuestionId": first_id,
                "SelectedChoiceIds": [
                    offered
                        .iter()
                        .next()
                        .expect("a published question offers at least one choice")
                        .to_uppercase(),
                ],
            }),
        ),
    ];

    // A choice of every OTHER published question, named under the first one: a choice
    // belongs to the question that offers it.
    for question in questions.iter().skip(1) {
        let other_id = member(question, "QuestionId").to_string();
        let Some(choice) = choices_of(question)
            .first()
            .map(|choice| member(choice, "ChoiceId").to_string())
        else {
            continue;
        };
        if offered.contains(&choice) {
            continue;
        }
        rows.push((
            format!("a-choice-of-{other_id}"),
            json!({
                "QuestionId": first_id,
                "SelectedChoiceIds": [choice],
            }),
        ));
    }

    for (case, entry) in rows {
        if let Some(question_id) = entry.get("QuestionId").and_then(Value::as_str) {
            if question_id != first_id {
                assert!(
                    !published.contains(question_id),
                    "case {case:?}: this row must name a question the template does not ask, got \
                     {question_id:?}"
                );
            }
        }

        let answer = client
            .create_profile(create_request(&case, json!([entry])))
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an identifier the template does not publish must be refused, got \
             {answer:?}"
        );
        let fields = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            fields
                .iter()
                .any(|field| field.get("Name").and_then(Value::as_str) == Some("ProfileQuestions")),
            "case {case:?}: the refusal must name ProfileQuestions in Fields, got {answer:?}"
        );
        assert!(
            client.store().lock().profiles.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

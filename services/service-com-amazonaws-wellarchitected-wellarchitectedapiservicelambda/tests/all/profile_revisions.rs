// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateProfile` — the revision of a stored Well-Architected
//! Tool profile (`PATCH /profiles/{ProfileArn}`;
//! `smithy_query(query="operation", operation="UpdateProfile")`).
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. IMPLEMENTED, AND A WRITE OF WHAT IT NAMES. A revision replaces the description the
//!    request carries and the answer of each question it names, and leaves every member it
//!    does not name — the other questions, the name, the tags, the owner and `CreatedAt` —
//!    as stored. The answer is the profile as STORED, so a later `GetProfile` reports the
//!    same thing.
//! 2. A REVISION MOVES THE VERSION AND THE INSTANT. A profile's version is the service's
//!    own counter over its revisions, so a revision that changes what the profile records
//!    moves it to the next count and moves `UpdatedAt`; a request that records nothing new
//!    moves neither and reports the profile exactly as it stands.
//! 3. THE VERSION A READ AND AN ASSOCIATION RESOLVE AT. A profile is read AT a version, so
//!    after a revision the version it moved off resolves to nothing while the new one
//!    resolves; a workload's association stays pinned where it was until
//!    `UpgradeProfileVersion` re-pins it at the version the profile is now at.
//! 4. A REQUEST THE PROFILE CANNOT RECORD REVISES NOTHING. An ARN naming no stored profile
//!    is a `404 ResourceNotFoundException`; a `ProfileQuestions` entry no profile can
//!    record — one naming no question, two naming the same question, one naming a question
//!    the published profile template does not ask, or one selecting a choice that question
//!    does not offer — is a `400 ValidationException` naming the member; and a value
//!    outside a published bound is refused by the boundary walk. In every case the stored
//!    profile is byte-identical afterwards.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client lives in `super::workload_harness`; the profile request
//! builder and the create assertions live in `super::profile_lifecycle`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::{json, Value};

use super::profile_lifecycle::{
    absent_profile_arn, created, minimal_profile, profile_of, reported_question,
};
use super::workload_harness::{
    age_profile, assert_not_found, created_id, minimal_create, template_choice,
    template_question, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The published bounds of a profile question's identifiers and of a profile's
/// description: `QuestionId` is `@length(1, 128)`, `ChoiceId` `@length(1, 64)` and
/// `ProfileDescription` `@length(3, 100)`
/// (`smithy_query(query="shape", shape="QuestionId,ChoiceId,ProfileDescription")`).
const MAX_QUESTION_ID: usize = 128;
/// See [`MAX_QUESTION_ID`].
const MAX_CHOICE_ID: usize = 64;
/// See [`MAX_QUESTION_ID`].
const MIN_DESCRIPTION: usize = 3;
/// See [`MAX_QUESTION_ID`].
const MAX_DESCRIPTION: usize = 100;

/// How far into the past a stored `UpdatedAt` is moved before the request under test, so a
/// write the handler makes to it is observable whatever the clock's granularity between two
/// in-process requests — and so a write it must NOT make cannot hide behind that
/// granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// The answers a profile records: the choices selected for each `QuestionId`, exactly as
/// `Profile$ProfileQuestions` reports them — one entry per question, in question-id order,
/// each carrying its selection as a set.
type Questions = BTreeMap<String, BTreeSet<String>>;

/// The answers every profile in this suite starts from, so a revision has answers to
/// replace and answers to leave alone. Both questions are ones the published profile
/// template asks, and each selection is a choice that question offers.
fn starting_questions() -> Questions {
    [
        (template_question(0), vec![template_choice(0, 0)]),
        (template_question(3), vec![template_choice(3, 3)]),
    ]
    .into_iter()
    .map(|(question_id, choices)| {
        (
            question_id.to_string(),
            choices.into_iter().map(str::to_string).collect(),
        )
    })
    .collect()
}

/// A question map as the wire carries it: the `ProfileQuestionUpdates` a request sends.
fn questions_json(questions: &Questions) -> Value {
    Value::Array(
        questions
            .iter()
            .map(|(question_id, choices)| {
                json!({
                    "QuestionId": question_id,
                    "SelectedChoiceIds": choices.iter().cloned().collect::<Vec<String>>(),
                })
            })
            .collect(),
    )
}

/// A question map as a READ reports it (`Profile$ProfileQuestions`): the selection stored
/// for each question, beside the text the published template gives that question.
fn reported_questions(questions: &Questions) -> Value {
    Value::Array(
        questions
            .iter()
            .map(|(question_id, choices)| reported_question(question_id, choices.clone()))
            .collect(),
    )
}

/// The answers a profile holding `start` records after a request carrying `updates`:
/// each update replaces the selection of the question it names, and a question the request
/// does not name keeps its answer. Computed here from the request, independently of the
/// code under test.
fn merged(start: &Questions, updates: &Value) -> Questions {
    let mut questions = start.clone();
    for update in updates.as_array().cloned().unwrap_or_default() {
        let question_id = update
            .get("QuestionId")
            .and_then(Value::as_str)
            .expect("every question update in a success case names its question")
            .to_string();
        let choices = update
            .get("SelectedChoiceIds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|choice| {
                choice
                    .as_str()
                    .expect("a selected choice id is a string")
                    .to_string()
            })
            .collect();
        questions.insert(question_id, choices);
    }
    questions
}

/// A profile created over HTTP answering `questions`: its `(ProfileArn, ProfileVersion)`.
async fn stored_profile(client: &Client, name: &str, questions: &Questions) -> (String, String) {
    let mut request = minimal_profile(name, &format!("tok-{name}"));
    request["ProfileQuestions"] = questions_json(questions);
    let answer = client.create_profile(request).await;
    created(&answer, name)
}

/// The `Profile` `GetProfile` reports for `profile_arn`, read without a version.
async fn read_profile(client: &Client, profile_arn: &str, case: &str) -> Value {
    let answer = client.get_profile(profile_arn, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetProfile 200, got {answer:?}"
    );
    profile_of(&answer)
}

/// One member of a reported `Profile` as epoch seconds, which is how restJson1 carries a
/// `Timestamp`.
fn instant(profile: &Value, name: &str, case: &str) -> f64 {
    profile
        .get(name)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the profile must report {name}, got {profile:?}"))
}

/// The version a revision moves a profile to: the next value of the service's counter over
/// that profile's revisions, computed here from the version the profile is at.
fn next_version(current: &str, case: &str) -> String {
    let count: u64 = current
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: a minted ProfileVersion is a count, got {current:?}"));
    (count + 1).to_string()
}

/// Rules 1 and 2. The rows differ along WHAT the revision names — the description, one
/// question's answer, a question the profile does not yet answer, several at once, a
/// selection cleared, a choice named twice, each published bound of the members it
/// carries — and along whether what it names is a CHANGE at all, because a profile is
/// projected from its own state rather than from one recorded request.
#[tokio::test]
async fn a_revision_writes_what_it_names_and_leaves_the_rest() {
    let cases: Vec<(&str, Value)> = vec![
        // Rule 2: a request that records nothing new.
        ("nothing-asked-for", json!({})),
        ("no-questions-named", json!({ "ProfileQuestions": [] })),
        (
            "the-description-already-stored",
            json!({ "ProfileDescription": "a profile under test" }),
        ),
        (
            "the-answers-already-stored",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(3),
                        "SelectedChoiceIds": [template_choice(3, 3)],
                    },
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 0)],
                    },
                ]
            }),
        ),
        // Rule 1: each member the request carries is written on its own.
        (
            "description-only",
            json!({ "ProfileDescription": "a revised profile" }),
        ),
        (
            "one-answer-replaced",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 4)],
                    },
                ]
            }),
        ),
        (
            "an-answer-cleared",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [],
                }]
            }),
        ),
        (
            "an-answer-without-a-selection",
            json!({ "ProfileQuestions": [{ "QuestionId": template_question(0) }] }),
        ),
        (
            "a-question-not-yet-answered",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(1),
                        "SelectedChoiceIds": [template_choice(1, 3)],
                    },
                ]
            }),
        ),
        (
            "several-questions-at-once",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(3),
                        "SelectedChoiceIds": [template_choice(3, 0)],
                    },
                    { "QuestionId": template_question(1), "SelectedChoiceIds": [] },
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 1), template_choice(0, 0)],
                    },
                ]
            }),
        ),
        (
            "choices-out-of-order-and-repeated",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [
                        template_choice(0, 1), template_choice(0, 0),
                        template_choice(0, 1), template_choice(0, 0),
                    ],
                }]
            }),
        ),
        (
            "the-description-and-the-answers",
            json!({
                "ProfileDescription": "a profile revised in both members",
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 2)],
                    },
                ]
            }),
        ),
        // Each published bound of the members a revision carries.
        (
            "description-at-the-minimum-length",
            json!({ "ProfileDescription": "d".repeat(MIN_DESCRIPTION) }),
        ),
        (
            "description-at-the-maximum-length",
            json!({ "ProfileDescription": "d".repeat(MAX_DESCRIPTION) }),
        ),
        // Every question the published template asks, answered at once: the table it
        // publishes is what bounds a stored profile's question map.
        (
            "every-published-question-answered",
            json!({
                "ProfileQuestions": (0..state::PROFILE_TEMPLATE_QUESTIONS.len())
                    .map(|index| json!({
                        "QuestionId": template_question(index),
                        "SelectedChoiceIds": [template_choice(index, 1)],
                    }))
                    .collect::<Vec<Value>>()
            }),
        ),
    ];

    for (case, body) in cases {
        let client = Client::new();
        let start = starting_questions();
        let (arn, version) = stored_profile(&client, &format!("profile-{case}"), &start).await;

        // The profile as it stands, with its `UpdatedAt` moved into the past so a write to
        // it — or the absence of one — is observable.
        age_profile(&client, &arn, AGE);
        let before = read_profile(&client, &arn, case).await;
        let stored_description = before
            .get("ProfileDescription")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let updated_at_before = instant(&before, "UpdatedAt", case);

        // What the request asks the profile to record, computed from the request and the
        // profile as it stands rather than from the handler.
        let expected_description = body
            .get("ProfileDescription")
            .and_then(Value::as_str)
            .unwrap_or(&stored_description)
            .to_string();
        let expected_questions = match body.get("ProfileQuestions") {
            Some(updates) => merged(&start, updates),
            None => start.clone(),
        };
        let revises = expected_description != stored_description || expected_questions != start;
        let expected_version = if revises {
            next_version(&version, case)
        } else {
            version.clone()
        };

        let answer = client.update_profile(&arn, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateProfile 200, got {answer:?}"
        );
        let revised = profile_of(&answer);

        // Rule 1: the members the request named, and the ones it did not.
        assert_eq!(
            revised.get("ProfileDescription").and_then(Value::as_str),
            Some(expected_description.as_str()),
            "case {case:?}: expected ProfileDescription {expected_description:?}, got {revised:?}"
        );
        let expected_reported = reported_questions(&expected_questions);
        assert_eq!(
            revised.get("ProfileQuestions"),
            Some(&expected_reported),
            "case {case:?}: expected ProfileQuestions {expected_reported:?}, got {revised:?}"
        );
        for name in ["ProfileArn", "ProfileName", "Owner", "Tags", "CreatedAt"] {
            assert_eq!(
                revised.get(name),
                before.get(name),
                "case {case:?}: a revision must leave {name} as stored, got {revised:?}"
            );
        }

        // Rule 2: the version and the instant move exactly when the profile changed.
        assert_eq!(
            revised.get("ProfileVersion").and_then(Value::as_str),
            Some(expected_version.as_str()),
            "case {case:?}: expected ProfileVersion {expected_version:?}, got {revised:?}"
        );
        let updated_at_after = instant(&revised, "UpdatedAt", case);
        if revises {
            assert!(
                updated_at_after > updated_at_before,
                "case {case:?}: a revision must move UpdatedAt, got {updated_at_after} after {updated_at_before}"
            );
        } else {
            assert_eq!(
                updated_at_after, updated_at_before,
                "case {case:?}: a request that records nothing new must leave UpdatedAt where it was"
            );
        }

        // The answer is the profile as STORED, so the read agrees with it, and a second
        // read agrees with the first.
        let read_back = read_profile(&client, &arn, case).await;
        assert_eq!(
            read_back, revised,
            "case {case:?}: GetProfile must report exactly what UpdateProfile answered"
        );
        assert_eq!(
            read_profile(&client, &arn, case).await,
            read_back,
            "case {case:?}: a second GetProfile must report the same profile"
        );
    }
}

/// Rule 2, over a SEQUENCE of revisions: each one that changes the profile moves it one
/// count further and the answers accumulate, so the version is a count of revisions rather
/// than a flag.
#[tokio::test]
async fn successive_revisions_advance_the_count_and_accumulate() {
    let revisions: Vec<(&str, Value)> = vec![
        (
            "first",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [template_choice(0, 4)],
                }]
            }),
        ),
        (
            "second",
            json!({ "ProfileDescription": "revised a second time" }),
        ),
        // Records nothing new, so the count must stay where the second revision left it.
        ("third-records-nothing", json!({ "ProfileQuestions": [] })),
        (
            "fourth",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(1),
                        "SelectedChoiceIds": [template_choice(1, 0)],
                    },
                ]
            }),
        ),
        (
            "fifth",
            json!({
                "ProfileDescription": "revised a last time",
                "ProfileQuestions": [{
                    "QuestionId": template_question(3),
                    "SelectedChoiceIds": [],
                }],
            }),
        ),
    ];

    let client = Client::new();
    let mut questions = starting_questions();
    let (arn, first_version) = stored_profile(&client, "sequenced-profile", &questions).await;
    let mut count: u64 = first_version
        .parse()
        .expect("a minted ProfileVersion is a count");

    for (case, body) in revisions {
        let expected_questions = match body.get("ProfileQuestions") {
            Some(updates) => merged(&questions, updates),
            None => questions.clone(),
        };
        let stored_description = read_description(&client, &arn, case).await;
        let description_changes = body
            .get("ProfileDescription")
            .and_then(Value::as_str)
            .is_some_and(|asked| Some(asked) != stored_description.as_deref());
        if description_changes || expected_questions != questions {
            count += 1;
        }
        questions = expected_questions;

        let answer = client.update_profile(&arn, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateProfile 200, got {answer:?}"
        );
        let revised = profile_of(&answer);
        assert_eq!(
            revised.get("ProfileVersion").and_then(Value::as_str),
            Some(count.to_string().as_str()),
            "case {case:?}: expected the count to stand at {count}, got {revised:?}"
        );
        let expected_reported = reported_questions(&questions);
        assert_eq!(
            revised.get("ProfileQuestions"),
            Some(&expected_reported),
            "case {case:?}: the answers must accumulate to {expected_reported:?}, got {revised:?}"
        );
    }
}

/// The `ProfileDescription` the stored profile carries, for a test that has to tell a
/// revision of it from a restatement of it.
async fn read_description(client: &Client, profile_arn: &str, case: &str) -> Option<String> {
    read_profile(client, profile_arn, case)
        .await
        .get("ProfileDescription")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Rule 3. A revision moves the version a read resolves at, and leaves a workload's
/// association pinned where it was until the upgrade re-pins it.
#[tokio::test]
async fn a_revision_moves_the_version_a_read_and_an_upgrade_resolve_at() {
    let case = "revised-and-associated";
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create(case, "tok-workload")).await,
        case,
    );
    let (arn, pinned) = stored_profile(&client, case, &starting_questions()).await;

    let associated = client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateProfiles"),
            Some(json!({ "ProfileArns": [arn] })),
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case {case:?}: expected AssociateProfiles 200, got {associated:?}"
    );

    let answer = client
        .update_profile(
            &arn,
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [template_choice(0, 3)],
                }]
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateProfile 200, got {answer:?}"
    );
    let latest = next_version(&pinned, case);
    assert_eq!(
        profile_of(&answer).get("ProfileVersion").and_then(Value::as_str),
        Some(latest.as_str()),
        "case {case:?}: the revision must move the profile to {latest:?}, got {answer:?}"
    );

    // A profile is read AT a version: the one it is now at resolves, the one it moved off
    // names no readable revision.
    let at_latest = client.get_profile(&arn, Some(&latest)).await;
    assert_eq!(
        at_latest.status, 200,
        "case {case:?}: the version the profile is at must resolve, got {at_latest:?}"
    );
    let at_pinned = client.get_profile(&arn, Some(&pinned)).await;
    assert_not_found(
        case,
        &at_pinned,
        "PROFILE",
        &format!("{arn}.{pinned}"),
        &format!("version {pinned} of a profile with ARN {arn}"),
    );

    // The association is a member of the WORKLOAD, so revising the profile leaves it where
    // it was pinned; the upgrade is what moves it to the version the profile is now at.
    let workload_profiles = |body: &Value| -> Vec<Value> {
        body.get("Workload")
            .and_then(|workload| workload.get("Profiles"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let before = client.get(&workload_id).await;
    assert_eq!(
        workload_profiles(&before.body),
        vec![json!({ "ProfileArn": arn, "ProfileVersion": pinned })],
        "case {case:?}: revising a profile must not move a workload's association, got {before:?}"
    );

    let upgraded = client
        .send(
            "PUT",
            &format!(
                "/workloads/{workload_id}/profiles/{}/upgrade",
                super::workload_harness::encode_path_segment(&arn)
            ),
            Some(json!({ "ClientRequestToken": "tok-upgrade" })),
        )
        .await;
    assert_eq!(
        upgraded.status, 200,
        "case {case:?}: expected UpgradeProfileVersion 200, got {upgraded:?}"
    );
    let after = client.get(&workload_id).await;
    assert_eq!(
        workload_profiles(&after.body),
        vec![json!({ "ProfileArn": arn, "ProfileVersion": latest })],
        "case {case:?}: the upgrade must re-pin the association at {latest:?}, got {after:?}"
    );
}

/// Rule 4. The rows differ along WHAT the request cannot record and along which layer
/// refuses it — the boundary's constraint walk for a value outside a published bound, the
/// handler for an entry no profile can record, including an identifier the published
/// profile template does not name — and every row asserts the stored profile is untouched
/// afterwards.
#[tokio::test]
async fn a_revision_a_profile_cannot_record_writes_nothing() {
    // (case, the request body, the expected status and wire error code, whether the
    // handler names `ProfileQuestions` in `Fields`)
    let cases: Vec<(&str, Value, u16, &str, bool)> = vec![
        (
            "an-update-without-a-question",
            json!({ "ProfileQuestions": [{ "SelectedChoiceIds": [template_choice(0, 0)] }] }),
            400,
            "ValidationException",
            true,
        ),
        (
            "one-update-without-a-question-among-several",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 4)],
                    },
                    { "SelectedChoiceIds": [template_choice(3, 3)] },
                ]
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "the-same-question-twice",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 0)],
                    },
                    {
                        "QuestionId": template_question(0),
                        "SelectedChoiceIds": [template_choice(0, 4)],
                    },
                ]
            }),
            400,
            "ValidationException",
            true,
        ),
        // An identifier the published template does not name: a question it does not ask,
        // and a choice the named question does not offer.
        (
            "a-question-the-template-does-not-ask",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "not-a-template-question",
                    "SelectedChoiceIds": [template_choice(0, 0)],
                }]
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a-question-id-at-the-maximum-length-the-template-does-not-ask",
            json!({ "ProfileQuestions": [{ "QuestionId": "q".repeat(MAX_QUESTION_ID) }] }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a-choice-the-question-does-not-offer",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": ["not-a-choice"],
                }]
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a-choice-of-another-published-question",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [template_choice(1, 0)],
                }]
            }),
            400,
            "ValidationException",
            true,
        ),
        (
            "a-choice-id-at-the-maximum-length-the-question-does-not-offer",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": ["c".repeat(MAX_CHOICE_ID)],
                }]
            }),
            400,
            "ValidationException",
            true,
        ),
        // One past each published bound: refused by the boundary walk, before the handler.
        (
            "description-one-under-the-minimum-length",
            json!({ "ProfileDescription": "d".repeat(MIN_DESCRIPTION - 1) }),
            400,
            "ValidationException",
            false,
        ),
        (
            "description-one-past-the-maximum-length",
            json!({ "ProfileDescription": "d".repeat(MAX_DESCRIPTION + 1) }),
            400,
            "ValidationException",
            false,
        ),
        (
            "question-id-past-the-maximum-length",
            json!({ "ProfileQuestions": [{ "QuestionId": "q".repeat(MAX_QUESTION_ID + 1) }] }),
            400,
            "ValidationException",
            false,
        ),
        (
            "choice-id-past-the-maximum-length",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": ["c".repeat(MAX_CHOICE_ID + 1)],
                }]
            }),
            400,
            "ValidationException",
            false,
        ),
    ];

    for (case, body, status, error_code, named_by_the_handler) in cases {
        let client = Client::new();
        let start = starting_questions();
        let (arn, _) = stored_profile(&client, &format!("refused-{case}"), &start).await;
        age_profile(&client, &arn, AGE);
        let before = read_profile(&client, &arn, case).await;

        let answer = client.update_profile(&arn, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, error_code),
            "case {case:?}: expected {status} {error_code}, got {answer:?}"
        );
        if named_by_the_handler {
            let fields = answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                fields.iter().any(|field| field.get("Name").and_then(Value::as_str)
                    == Some("ProfileQuestions")),
                "case {case:?}: the refusal must name ProfileQuestions in Fields, got {answer:?}"
            );
        }
        assert_eq!(
            read_profile(&client, &arn, case).await,
            before,
            "case {case:?}: a refused revision must leave the stored profile as it was"
        );
    }
}

/// Rule 4, for an ARN that resolves to nothing: the rows differ along the ARN's resource
/// id, and a stored profile sits beside each one so the 404 is about resolution.
#[tokio::test]
async fn a_revision_of_a_profile_that_is_not_stored_is_not_found() {
    let client = Client::new();
    let (stored_arn, _) = stored_profile(&client, "resolved-profile", &starting_questions()).await;
    let before = read_profile(&client, &stored_arn, "resolved-profile").await;

    for profile_id in ["0", "00000000000000000000000000000000", "deadbeef", "abc123"] {
        let absent = absent_profile_arn(profile_id);
        let answer = client
            .update_profile(&absent, json!({ "ProfileDescription": "a revision of nothing" }))
            .await;
        assert_not_found(
            profile_id,
            &answer,
            "PROFILE",
            &absent,
            &format!("profile with ARN {absent}"),
        );
    }

    assert_eq!(
        read_profile(&client, &stored_arn, "resolved-profile").await,
        before,
        "a revision that resolved nothing must leave the stored profile as it was"
    );
}

/// Rule 4, at the bound the published template sets: a profile answers each question that
/// template asks at most once, so a profile answering all of them is as large as a
/// question map gets, re-answering one of them records no new question, and an entry
/// naming a question the template does not ask is refused and writes nothing.
#[tokio::test]
async fn the_published_template_bounds_a_profiles_question_map() {
    let client = Client::new();
    let published = state::PROFILE_TEMPLATE_QUESTIONS.len();
    // Every question the published template asks, answered with a choice it offers.
    let start: Questions = (0..published)
        .map(|index| {
            (
                template_question(index).to_string(),
                [template_choice(index, 0).to_string()].into_iter().collect(),
            )
        })
        .collect();
    let (arn, version) = stored_profile(&client, "bounded-profile", &start).await;
    assert_eq!(
        start.len(),
        published,
        "case {published:?} questions: the published table must ask each question once, got {start:?}"
    );

    // Re-answering a question the profile already holds records no new question, so the
    // map stays at the published size.
    let accepted = client
        .update_profile(
            &arn,
            json!({
                "ProfileQuestions": [{
                    "QuestionId": template_question(0),
                    "SelectedChoiceIds": [template_choice(0, 1)],
                }]
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {published:?} questions: a revision that adds no question must be served, got {accepted:?}"
    );
    let at_bound = profile_of(&accepted);
    let revised_version = next_version(&version, "bounded");
    assert_eq!(
        at_bound.get("ProfileVersion").and_then(Value::as_str),
        Some(revised_version.as_str()),
        "case {published:?} questions: expected ProfileVersion {revised_version:?}, got {accepted:?}"
    );
    assert_eq!(
        at_bound
            .get("ProfileQuestions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(published),
        "case {published:?} questions: the map must stay at the published size, got {at_bound:?}"
    );

    // A question the template does not ask cannot be added to it.
    let refused = client
        .update_profile(
            &arn,
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "one-question-the-template-never-asks",
                    "SelectedChoiceIds": [template_choice(0, 0)],
                }]
            }),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "case {published:?} questions: a question the template does not ask must be refused, got {refused:?}"
    );
    assert_eq!(
        read_profile(&client, &arn, "bounded").await,
        at_bound,
        "case {published:?} questions: a refused revision must leave the stored profile as it was"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Well-Architected Tool profile lifecycle:
//! `CreateProfile` and the `GetProfile` that reads it back.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. `CreateProfile` mints a profile, persists every member the request carries, and
//!    answers its `ProfileArn` and `ProfileVersion`; a later `GetProfile` of that ARN
//!    reports those members plus the ones the SERVICE computes (`Owner`, `CreatedAt`,
//!    `UpdatedAt`, `ProfileVersion`), derived from the profile's own stored state.
//! 2. A profile question's selection is a SET keyed by its question: the stored profile
//!    holds one entry per `QuestionId` with the choices selected for it, so the order the
//!    questions arrive in, and a choice named twice, do not change what is recorded.
//! 3. A `ClientRequestToken` makes the create idempotent: the same token with the same
//!    parameters returns the original profile, the same token with different parameters
//!    is a `409 ConflictException`, and a fresh token creates a second profile — even
//!    under a name an existing profile already carries, which no published rule meters.
//! 4. A `ProfileQuestions` entry a profile cannot record — one naming no question, or two
//!    naming the same question — is a `400 ValidationException` naming the member, and
//!    nothing is stored.
//! 5. A `ProfileArn` and a `ProfileVersion` are RESOLVED against the store: an ARN naming
//!    no stored profile, or a version the stored profile is not at, is a
//!    `404 ResourceNotFoundException` carrying the identifier as sent and the missing
//!    resource's type.
//! 6. The question count one profile records is bounded by this emulator's own quota,
//!    answered with the modelled `402 ServiceQuotaExceededException`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::workload_harness::{Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The published bounds of a profile question's identifiers:
/// `QuestionId` is `@length(1, 128)` and `ChoiceId` `@length(1, 64)`
/// (`smithy_query(query="shape", shape="QuestionId,ChoiceId")`).
const MAX_QUESTION_ID: usize = 128;
/// See [`MAX_QUESTION_ID`].
const MAX_CHOICE_ID: usize = 64;

/// The members of `Profile$ProfileQuestions` that describe the QUESTION rather than the
/// answer. The service reads them off the profile template it publishes; this emulator
/// carries none (`GetProfileTemplate` is a stub with its own issue), so a stored
/// profile's questions report none of them rather than an invented catalog.
const QUESTION_DESCRIPTION_MEMBERS: [&str; 5] = [
    "QuestionTitle",
    "QuestionDescription",
    "QuestionChoices",
    "MinSelectedChoices",
    "MaxSelectedChoices",
];

/// A minimal valid `CreateProfile` request; a caller overrides the members its case
/// varies.
pub(crate) fn minimal_profile(name: &str, token: &str) -> Value {
    json!({
        "ProfileName": name,
        "ProfileDescription": "a profile under test",
        "ProfileQuestions": [{
            "QuestionId": "priority",
            "SelectedChoiceIds": ["priority_cost"],
        }],
        "ClientRequestToken": token,
    })
}

/// A well-formed `ProfileArn` (the modelled
/// `^arn:aws[-a-z]*:wellarchitected:[a-z]{2}(-gov)?-[a-z]+-\d:\d{12}:profile/[a-z0-9]+$`)
/// that no stored profile carries.
pub(crate) fn absent_profile_arn(profile_id: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:profile/{profile_id}",
        engine::emulator_account_id()
    )
}

pub(crate) fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn profile_of(answer: &Answer) -> Value {
    answer
        .body
        .get("Profile")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `(ProfileArn, ProfileVersion)` a successful `CreateProfile` answered with, checked
/// against the modelled `ProfileArn` pattern and its `@length(..2084)`.
pub(crate) fn created(answer: &Answer, case: &str) -> (String, String) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateProfile 200, got {answer:?}"
    );
    let arn = member(answer, "ProfileArn").to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:us-west-2:{}:profile/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "case {case:?}: ProfileArn must be {expected_prefix}<[a-z0-9]+>, got {arn:?}"
    );
    assert!(
        arn.len() <= 2084,
        "case {case:?}: ProfileArn must satisfy @length(..2084), got {} characters in {arn:?}",
        arn.len()
    );
    let version = member(answer, "ProfileVersion").to_string();
    assert!(
        (1..=32).contains(&version.len())
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case {case:?}: ProfileVersion must satisfy @length(1, 32) and ^[A-Za-z0-9-]+$, got {version:?}"
    );
    (arn, version)
}

/// The `ProfileQuestions` a request's updates must be recorded as: one entry per
/// `QuestionId` in question-id order, each carrying the choices selected for it as a set.
/// Computed here from the request, independently of the code under test.
pub(crate) fn expected_questions(updates: &Value) -> Value {
    let mut recorded: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
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
        recorded.insert(question_id, choices);
    }
    Value::Array(
        recorded
            .into_iter()
            .map(|(question_id, choices)| {
                json!({
                    "QuestionId": question_id,
                    "SelectedChoiceIds": choices.into_iter().collect::<Vec<String>>(),
                })
            })
            .collect(),
    )
}

/// Rules 1 and 2. The rows differ along what the request carries — how many questions it
/// answers, how many choices each answer selects, the order the questions arrive in,
/// whether a choice is named twice, whether tags are present, and the name, description,
/// question-id and choice-id lengths at their published bounds — because the stored
/// profile is projected from its own state, not from one recorded request.
#[tokio::test]
async fn a_created_profile_round_trips_through_get() {
    let cases: Vec<(&str, Value)> = vec![
        ("minimal", json!({})),
        ("no-questions-answered", json!({ "ProfileQuestions": [] })),
        (
            "question-without-a-selection",
            json!({ "ProfileQuestions": [{ "QuestionId": "priority" }] }),
        ),
        (
            "question-with-an-empty-selection",
            json!({
                "ProfileQuestions": [{ "QuestionId": "priority", "SelectedChoiceIds": [] }]
            }),
        ),
        (
            "several-choices-selected",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "priority",
                    "SelectedChoiceIds": ["priority_cost", "priority_perf", "priority_rel"],
                }]
            }),
        ),
        // Rule 2: the recorded profile is keyed by question and holds a SET of choices,
        // so neither the order the questions arrive in nor a choice named twice changes
        // what is stored.
        (
            "questions-out-of-order",
            json!({
                "ProfileQuestions": [
                    { "QuestionId": "workload-type", "SelectedChoiceIds": ["type_batch"] },
                    { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                    { "QuestionId": "business-objective", "SelectedChoiceIds": [] },
                ]
            }),
        ),
        (
            "choices-out-of-order-and-repeated",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "priority",
                    "SelectedChoiceIds": [
                        "priority_rel", "priority_cost", "priority_rel", "priority_cost"
                    ],
                }]
            }),
        ),
        (
            "tags",
            json!({ "Tags": { "Env": "test", "Team": "core" } }),
        ),
        // `ProfileName` and `ProfileDescription` are both @length(3, 100): each at its
        // published minimum, then at its published maximum.
        (
            "name-and-description-at-the-minimum-length",
            json!({ "ProfileName": "abc", "ProfileDescription": "xyz" }),
        ),
        (
            "name-and-description-at-the-maximum-length",
            json!({ "ProfileName": "n".repeat(100), "ProfileDescription": "d".repeat(100) }),
        ),
        (
            "question-and-choice-ids-at-the-maximum-length",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "q".repeat(MAX_QUESTION_ID),
                    "SelectedChoiceIds": ["c".repeat(MAX_CHOICE_ID)],
                }]
            }),
        ),
        // The question quota is a published bound of this emulator's own: a profile
        // answering exactly as many questions as it meters is accepted.
        (
            "questions-at-the-quota",
            json!({
                "ProfileQuestions": (0..state::MAX_QUESTIONS_PER_PROFILE)
                    .map(|index| json!({
                        "QuestionId": format!("question-{index:03}"),
                        "SelectedChoiceIds": [format!("choice-{index:03}")],
                    }))
                    .collect::<Vec<Value>>()
            }),
        ),
    ];

    // One client, so every profile shares a store and the ARNs must stay distinct.
    let client = Client::new();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for (case, overrides) in cases {
        let mut request = minimal_profile(&format!("profile-{case}"), &format!("tok-{case}"));
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            request[name] = value.clone();
        }

        let answer = client.create_profile(request.clone()).await;
        let (arn, version) = created(&answer, case);
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: every created profile must get its own ARN, but {arn:?} repeated"
        );

        let fetched = client.get_profile(&arn, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetProfile must resolve the created ARN, got {fetched:?}"
        );
        let profile = profile_of(&fetched);

        // The members the REQUEST set come back exactly as sent.
        for name in ["ProfileName", "ProfileDescription"] {
            assert_eq!(
                profile.get(name),
                request.get(name),
                "case {case:?}: {name} must round-trip as sent, got {profile:?}"
            );
        }
        let expected_tags = request.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            profile.get("Tags"),
            Some(&expected_tags),
            "case {case:?}: expected Tags {expected_tags:?}, got {profile:?}"
        );

        // Rule 2: the questions, as the profile records them.
        let expected = expected_questions(&request["ProfileQuestions"]);
        assert_eq!(
            profile.get("ProfileQuestions"),
            Some(&expected),
            "case {case:?}: expected ProfileQuestions {expected:?}, got {profile:?}"
        );
        for question in profile
            .get("ProfileQuestions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            for name in QUESTION_DESCRIPTION_MEMBERS {
                assert_eq!(
                    question.get(name),
                    None,
                    "case {case:?}: no profile template is carried, so {name} must be absent, got {question:?}"
                );
            }
        }

        // The members the SERVICE computes.
        assert_eq!(
            profile.get("ProfileArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: the profile must report its own ARN, got {profile:?}"
        );
        assert_eq!(
            profile.get("ProfileVersion").and_then(Value::as_str),
            Some(version.as_str()),
            "case {case:?}: the profile must report the version the create answered, got {profile:?}"
        );
        assert_eq!(
            profile.get("Owner").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: the creating account owns the profile, got {profile:?}"
        );
        let created_at = profile.get("CreatedAt").and_then(Value::as_f64);
        assert!(
            created_at.is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: CreatedAt must be the epoch-seconds timestamp of the create, got {profile:?}"
        );
        assert_eq!(
            profile.get("UpdatedAt").and_then(Value::as_f64),
            created_at,
            "case {case:?}: nothing has revised a new profile, so UpdatedAt must equal CreatedAt, got {profile:?}"
        );
        // Only a profile reached through a share invitation carries one, and profile
        // shares are not implemented.
        assert_eq!(
            profile.get("ShareInvitationId"),
            None,
            "case {case:?}: an unshared profile carries no ShareInvitationId, got {profile:?}"
        );

        // A read must not change what a later caller sees.
        let reread = profile_of(&client.get_profile(&arn, None).await);
        assert_eq!(
            reread, profile,
            "case {case:?}: a second GetProfile must report the same profile"
        );
    }
}

/// Rule 3. The rows differ along WHAT the replayed request changes: nothing, only the
/// order of members a set is recorded from (still the same request), and a member the
/// profile records (a different request under a token that already names one).
#[tokio::test]
async fn the_client_request_token_decides_what_a_retry_means() {
    // (case, the overrides the retry carries, whether it is the SAME request)
    let cases: Vec<(&str, Value, bool)> = vec![
        ("identical", json!({}), true),
        (
            "questions-reordered",
            json!({
                "ProfileQuestions": [
                    { "QuestionId": "workload-type", "SelectedChoiceIds": ["type_batch"] },
                    { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                ]
            }),
            true,
        ),
        (
            "choice-repeated",
            json!({
                "ProfileQuestions": [
                    { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                    {
                        "QuestionId": "workload-type",
                        "SelectedChoiceIds": ["type_batch", "type_batch"],
                    },
                ]
            }),
            true,
        ),
        (
            "different-description",
            json!({ "ProfileDescription": "a different profile" }),
            false,
        ),
        (
            "different-selection",
            json!({
                "ProfileQuestions": [
                    { "QuestionId": "priority", "SelectedChoiceIds": ["priority_perf"] },
                    { "QuestionId": "workload-type", "SelectedChoiceIds": ["type_batch"] },
                ]
            }),
            false,
        ),
        (
            "different-tags",
            json!({ "Tags": { "Env": "other" } }),
            false,
        ),
    ];

    for (case, overrides, same_request) in cases {
        let client = Client::new();
        let token = format!("tok-{case}");
        let mut first = minimal_profile("retried-profile", &token);
        first["ProfileQuestions"] = json!([
            { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
            { "QuestionId": "workload-type", "SelectedChoiceIds": ["type_batch"] },
        ]);
        first["Tags"] = json!({ "Env": "test" });

        let original = client.create_profile(first.clone()).await;
        let (arn, version) = created(&original, case);

        let mut retry = first.clone();
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            retry[name] = value.clone();
        }
        let replayed = client.create_profile(retry).await;

        if same_request {
            assert_eq!(
                (member(&replayed, "ProfileArn"), member(&replayed, "ProfileVersion")),
                (arn.as_str(), version.as_str()),
                "case {case:?}: a retry of the same request must answer the original profile, got {replayed:?}"
            );
        } else {
            assert_eq!(
                (replayed.status, replayed.error_code.as_str()),
                (409, "ConflictException"),
                "case {case:?}: the same token with different parameters must conflict, got {replayed:?}"
            );
            assert_eq!(
                member(&replayed, "ResourceId"),
                arn,
                "case {case:?}: the conflict must name the profile the token already created, got {replayed:?}"
            );
            assert_eq!(
                member(&replayed, "ResourceType"),
                "PROFILE",
                "case {case:?}: the conflict must name the resource type, got {replayed:?}"
            );
        }

        assert_eq!(
            client.store().lock().profiles.len(),
            1,
            "case {case:?}: a retry under a spent token must never create a second profile"
        );

        // A FRESH token is a new request, even for the identical body under a name the
        // stored profile already carries: no published rule meters a profile name.
        let mut fresh = first.clone();
        fresh["ClientRequestToken"] = json!(format!("fresh-{case}"));
        let second = client.create_profile(fresh).await;
        let (second_arn, _) = created(&second, case);
        assert_ne!(
            second_arn, arn,
            "case {case:?}: a fresh token must create a second profile, got {second:?}"
        );
    }
}

/// Rule 4. The rows differ along WHICH `ProfileQuestions` entry cannot be recorded, and
/// whether the model or the handler refuses it.
#[tokio::test]
async fn a_question_update_a_profile_cannot_record_is_refused() {
    // (case, the ProfileQuestions the request carries, whether the handler names the
    // member in Fields — a value the MODEL refuses never reaches the handler)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "update-without-a-question",
            json!([{ "SelectedChoiceIds": ["priority_cost"] }]),
            true,
        ),
        (
            "one-update-without-a-question-among-several",
            json!([
                { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                { "SelectedChoiceIds": ["type_batch"] },
            ]),
            true,
        ),
        (
            "the-same-question-twice",
            json!([
                { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                { "QuestionId": "priority", "SelectedChoiceIds": ["priority_perf"] },
            ]),
            true,
        ),
        // `QuestionId` is @length(1, 128) and `ChoiceId` @length(1, 64): a value one past
        // either published bound is refused by the boundary walk, before the handler.
        (
            "empty-question-id",
            json!([{ "QuestionId": "", "SelectedChoiceIds": ["priority_cost"] }]),
            false,
        ),
        (
            "question-id-past-the-maximum-length",
            json!([{ "QuestionId": "q".repeat(MAX_QUESTION_ID + 1) }]),
            false,
        ),
        (
            "choice-id-past-the-maximum-length",
            json!([{
                "QuestionId": "priority",
                "SelectedChoiceIds": ["c".repeat(MAX_CHOICE_ID + 1)],
            }]),
            false,
        ),
    ];

    for (case, questions, named_by_the_handler) in cases {
        let client = Client::new();
        let mut request = minimal_profile("refused-profile", &format!("tok-{case}"));
        request["ProfileQuestions"] = questions;

        let answer = client.create_profile(request).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {answer:?}"
        );
        if named_by_the_handler {
            let fields = answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                fields
                    .iter()
                    .any(|field| field.get("Name").and_then(Value::as_str)
                        == Some("ProfileQuestions")),
                "case {case:?}: the refusal must name ProfileQuestions in Fields, got {answer:?}"
            );
        }
        assert!(
            client.store().lock().profiles.is_empty(),
            "case {case:?}: a refused create must store nothing"
        );
    }
}

/// Rule 5, for an identifier that names nothing: a path-bound `ProfileArn` and a
/// query-bound `ProfileVersion` are both RESOLVED. The rows differ along the ARN's
/// resource id and along the version asked for, including the version the stored profile
/// IS at, which must resolve.
#[tokio::test]
async fn a_profile_arn_and_version_are_resolved_against_the_store() {
    let client = Client::new();
    let created_answer = client
        .create_profile(minimal_profile("resolved-profile", "tok-resolved"))
        .await;
    let (arn, version) = created(&created_answer, "resolved");

    // An ARN that names no stored profile, over several resource ids.
    for profile_id in ["0", "00000000000000000000000000000000", "deadbeef", "abc123"] {
        let absent = absent_profile_arn(profile_id);
        let answer = client.get_profile(&absent, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {profile_id:?}: an ARN naming no stored profile must be 404, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            absent,
            "case {profile_id:?}: the 404 must carry the ARN as sent, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "PROFILE",
            "case {profile_id:?}: the 404 must name the missing resource's type, got {answer:?}"
        );
    }

    // The version the stored profile is at resolves; any other version does not.
    let at_version = client.get_profile(&arn, Some(&version)).await;
    assert_eq!(
        at_version.status, 200,
        "case {version:?}: the version the profile is at must resolve, got {at_version:?}"
    );
    assert_eq!(
        profile_of(&at_version),
        profile_of(&client.get_profile(&arn, None).await),
        "case {version:?}: reading at the stored version must report the same profile as reading without one"
    );

    for absent_version in ["2", "0", "99", "LATEST"] {
        let answer = client.get_profile(&arn, Some(absent_version)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {absent_version:?}: a version the profile is not at must be 404, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceId"),
            format!("{arn}.{absent_version}"),
            "case {absent_version:?}: the 404 must name the profile and the version asked for, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            "PROFILE",
            "case {absent_version:?}: the 404 must name the missing resource's type, got {answer:?}"
        );
    }
}

/// Rule 6: one question past this emulator's published ceiling is refused with the
/// modelled quota error, and nothing is stored. The accepted boundary — exactly as many
/// questions as the quota meters — is a row of
/// [`a_created_profile_round_trips_through_get`].
#[tokio::test]
async fn more_questions_than_the_quota_meters_is_a_quota_failure() {
    let client = Client::new();
    let mut request = minimal_profile("over-quota-profile", "tok-over-quota");
    request["ProfileQuestions"] = json!((0..=state::MAX_QUESTIONS_PER_PROFILE)
        .map(|index| json!({
            "QuestionId": format!("question-{index:03}"),
            "SelectedChoiceIds": [format!("choice-{index:03}")],
        }))
        .collect::<Vec<Value>>());

    let answer = client.create_profile(request).await;
    let case = state::MAX_QUESTIONS_PER_PROFILE + 1;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case {case:?} questions: expected the modelled quota failure, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceType"),
        "PROFILE",
        "case {case:?} questions: the quota failure must name the metered resource, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ServiceCode"),
        "wellarchitected",
        "case {case:?} questions: the quota failure must name this service, got {answer:?}"
    );
    assert!(
        !member(&answer, "QuotaCode").is_empty(),
        "case {case:?} questions: QuotaCode is @required, got {answer:?}"
    );
    assert!(
        client.store().lock().profiles.is_empty(),
        "case {case:?} questions: a refused create must store nothing"
    );
}

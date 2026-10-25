// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteProfile`: the operation that removes a
//! Well-Architected Tool profile. The `CreateProfile` / `GetProfile` pair it is exercised
//! against is pinned by `super::profile_lifecycle`; this suite is about what the DELETE
//! does.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `DeleteProfile` removes EXACTLY the profile its `ProfileArn` resolves to, answering
//!    an empty 200: that ARN answers 404 to `GetProfile` and to a later delete carrying a
//!    token this service has not spent, while every other stored profile still reports its
//!    own members unchanged.
//! 2. A `ProfileArn` is RESOLVED, not merely validated: one that names no stored profile
//!    is answered `404 ResourceNotFoundException` carrying the ARN as sent, the resource
//!    type `PROFILE` and the message this service words for a missing profile — the same
//!    answer `GetProfile` gives, since the delete resolves the profile the same way.
//! 3. A deleted profile leaves nothing keyed to it behind: the `ClientRequestToken` that
//!    created it resolves to nothing afterwards, so replaying that create mints a fresh
//!    profile instead of answering an ARN that no longer resolves.
//! 4. An association names a profile that has to resolve, so deleting a profile a workload
//!    is associated with drops that association and leaves the workload's other
//!    associations exactly as they stood.
//!
//! The `@idempotencyToken` rule on the delete's own `ClientRequestToken` — a retry under
//! the ORIGINAL token replays the completed delete, a token spent on another target is a
//! conflict — is the rule of every implemented delete and is pinned over all of them in
//! `super::delete_idempotency`; rule 1 below only checks that this operation takes part in
//! it.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::profile_lifecycle::{absent_profile_arn, created};
use super::workload_harness::{
    assert_not_found, created_id, created_profile, minimal_create, profiles_of, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// Every assertion the answer to a successful `DeleteProfile` has to satisfy: the modelled
/// empty 200 (`DELETE /profiles/{ProfileArn} -> 200` over a `Unit` output).
fn assert_deleted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteProfile 200, got {answer:?}"
    );
    assert_eq!(
        answer.body,
        Value::Null,
        "case {case:?}: DeleteProfile models no output members, so the body is empty, \
         got {answer:?}"
    );
    assert!(
        answer.error_code.is_empty(),
        "case {case:?}: a successful DeleteProfile carries no error code, got {answer:?}"
    );
}

/// The 404 envelope every operation that resolves a `ProfileArn` answers for an ARN that
/// names no stored profile, worded in ONE place by this service.
fn assert_profile_absent(answer: &Answer, profile_arn: &str, case: &str) {
    assert_not_found(
        case,
        answer,
        state::PROFILE_RESOURCE_TYPE,
        profile_arn,
        &format!("profile with ARN {profile_arn}"),
    );
}

/// One row of the table below: a `CreateProfile` request whose profile is then deleted.
/// The rows differ along how many questions the profile answers, how many choices each
/// answer selects, how many tags it carries and where its name and description sit in the
/// modelled `@length(3, 100)` — so the delete is exercised over profiles that differ in
/// everything a profile can differ in, not over four copies of one.
struct Case {
    name: &'static str,
    profile_name: String,
    profile_description: String,
    questions: Vec<(&'static str, Vec<&'static str>)>,
    tags: Vec<(&'static str, &'static str)>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no-questions-no-tags",
            profile_name: "pro".to_string(),
            profile_description: "min".to_string(),
            questions: Vec::new(),
            tags: Vec::new(),
        },
        Case {
            name: "one-question-one-choice",
            profile_name: "single-question-profile".to_string(),
            profile_description: "answers one question".to_string(),
            questions: vec![("priority", vec!["priority_security"])],
            tags: vec![("owner", "platform")],
        },
        Case {
            name: "three-questions-zero-one-and-two-choices",
            profile_name: "many-question-profile".to_string(),
            profile_description: "answers three questions".to_string(),
            questions: vec![
                ("workload_type", vec!["workload_type_analytics", "zz-other"]),
                ("budget", vec![]),
                ("priority", vec!["priority_cost"]),
            ],
            tags: vec![("owner", "platform"), ("tier", "gold")],
        },
        Case {
            name: "name-and-description-at-the-upper-length-bound",
            profile_name: "n".repeat(100),
            profile_description: "d".repeat(100),
            questions: vec![("budget", vec!["budget_under_100k"])],
            tags: vec![("only", "")],
        },
    ]
}

/// The `CreateProfile` body for a case. `Tags` is omitted when the case carries none:
/// `TagMap` is `@length(min: 1)`, so an empty map is a request the boundary refuses.
fn create_body(case: &Case, token: &str) -> Value {
    let mut body = json!({
        "ProfileName": case.profile_name,
        "ProfileDescription": case.profile_description,
        "ProfileQuestions": case
            .questions
            .iter()
            .map(|(question_id, choices)| json!({
                "QuestionId": question_id,
                "SelectedChoiceIds": choices,
            }))
            .collect::<Vec<Value>>(),
        "ClientRequestToken": token,
    });
    if !case.tags.is_empty() {
        body["Tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

/// Rule 1: the delete removes exactly the profile it names, and every row of the table
/// takes its turn at being the deleted one.
#[tokio::test]
async fn deleting_a_profile_removes_exactly_that_profile() {
    let client = Client::new();
    let mut stored: Vec<(&'static str, String)> = Vec::new();
    for case in cases() {
        let answer = client
            .create_profile(create_body(&case, &format!("tok-{}", case.name)))
            .await;
        stored.push((case.name, created(&answer, case.name).0));
    }

    for (index, (case, arn)) in stored.iter().enumerate() {
        let token = format!("del-{case}");
        assert_deleted(&client.delete_profile(arn, &token).await, case);

        assert_profile_absent(
            &client.get_profile(arn, None).await,
            arn,
            &format!("{case}/GetProfile"),
        );

        // The same token is the delete's `@idempotencyToken`, so the retry replays the
        // completed delete rather than resolving an ARN the original request removed —
        // and it does not bring the profile back.
        assert_deleted(
            &client.delete_profile(arn, &token).await,
            &format!("{case}/retry-under-the-original-token"),
        );
        assert_profile_absent(
            &client.get_profile(arn, None).await,
            arn,
            &format!("{case}/after-the-retry"),
        );

        // A delete under a token this service has NOT spent is a new request, so it
        // resolves the ARN and finds nothing.
        assert_profile_absent(
            &client.delete_profile(arn, &format!("{token}-fresh")).await,
            arn,
            &format!("{case}/delete-under-a-fresh-token"),
        );

        // Every profile not yet deleted is untouched by this delete.
        for (other_case, other_arn) in &stored[index + 1..] {
            let still_there = client.get_profile(other_arn, None).await;
            assert_eq!(
                still_there.status, 200,
                "case {case:?}: deleting it must leave {other_case:?} in place, \
                 got {still_there:?}"
            );
            assert_eq!(
                still_there
                    .body
                    .get("Profile")
                    .and_then(|profile| profile.get("ProfileArn"))
                    .and_then(Value::as_str),
                Some(other_arn.as_str()),
                "case {case:?}: {other_case:?} must still report its own ARN, \
                 got {still_there:?}"
            );
        }
    }

    assert!(
        client.store().lock().profiles.is_empty(),
        "case \"table\": deleting every row empties the profile store"
    );
}

/// Rule 2: an ARN that names no stored profile is the lookup's 404, on the delete exactly
/// as on the read. The rows differ along how the absent ARN is spelled — all values the
/// modelled pattern accepts, because an identifier a route binds is RESOLVED — and one
/// profile exists throughout, so the rows fail on resolution and not on an empty store.
/// Every row carries a token this service has not spent, so each is a new request rather
/// than a retry.
#[tokio::test]
async fn deleting_an_absent_profile_is_a_not_found_naming_the_arn_and_the_type() {
    let client = Client::new();
    let (present, _) = created_profile(&client, "present-profile").await;

    let absent: Vec<(&str, String)> = vec![
        ("all-zeros", absent_profile_arn(&"0".repeat(32))),
        ("all-f", absent_profile_arn(&"f".repeat(32))),
        ("short-resource-part", absent_profile_arn("ab12")),
        (
            "another-account",
            "arn:aws:wellarchitected:us-west-2:999999999999:profile/0123456789abcdef".to_string(),
        ),
        (
            "another-region",
            format!(
                "arn:aws:wellarchitected:eu-west-1:{}:profile/0123456789abcdef",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (row, arn) in absent {
        assert_ne!(
            arn, present,
            "case {row:?}: the row must not name the profile that exists"
        );
        assert_profile_absent(
            &client.delete_profile(&arn, &format!("tok-{row}")).await,
            &arn,
            &format!("{row}/DeleteProfile"),
        );
        assert_profile_absent(
            &client.get_profile(&arn, None).await,
            &arn,
            &format!("{row}/GetProfile"),
        );
    }

    // The profile that DOES exist still resolves, so no refused delete removed it.
    let fetched = client.get_profile(&present, None).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored profile must still resolve, got {fetched:?}"
    );
    assert_eq!(
        client.store().lock().profiles.len(),
        1,
        "case \"present\": a refused delete removes nothing"
    );
}

/// Rule 3: the create token of a DELETED profile resolves to nothing, so replaying that
/// create mints a fresh profile instead of answering the ARN the delete removed. Every
/// row of the table is replayed, so the rule holds whatever the profile carried.
#[tokio::test]
async fn replaying_the_create_token_of_a_deleted_profile_creates_afresh() {
    let client = Client::new();
    for case in cases() {
        let token = format!("revive-{}", case.name);
        let (arn, _) = created(
            &client.create_profile(create_body(&case, &token)).await,
            case.name,
        );
        assert_deleted(
            &client
                .delete_profile(&arn, &format!("del-{}", case.name))
                .await,
            case.name,
        );

        let replay = client.create_profile(create_body(&case, &token)).await;
        let (replay_arn, _) = created(&replay, case.name);
        assert_ne!(
            replay_arn, arn,
            "case {:?}: the deleted profile's ARN must not be answered again, got {replay:?}",
            case.name
        );
        let fetched = client.get_profile(&replay_arn, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: the profile the replay created must resolve, got {fetched:?}",
            case.name
        );
        // The ARN the delete removed stays removed: the replay created a profile, it did
        // not restore one.
        assert_profile_absent(
            &client.get_profile(&arn, None).await,
            &arn,
            &format!("{}/the-deleted-arn", case.name),
        );
    }
}

/// `AssociateProfiles`: `PATCH /workloads/{WorkloadId}/associateProfiles`.
async fn associate_profiles(client: &Client, workload_id: &str, profile_arns: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateProfiles"),
            Some(json!({ "ProfileArns": profile_arns })),
        )
        .await
}

/// The `ProfileArn` of every association a workload reports, in the order it reports them.
async fn associated_arns(client: &Client, workload_id: &str, case: &str) -> Vec<String> {
    profiles_of(client, workload_id, case)
        .await
        .iter()
        .map(|profile| member_of(profile, "ProfileArn"))
        .collect()
}

fn member_of(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Rule 4. The rows differ along how many workloads hold an association with the deleted
/// profile — one profile can be reviewed by several workloads, while a workload holds at
/// most one profile — because every association names the deleted profile and each of them
/// goes with it; a bystander workload holding another profile keeps its own.
#[tokio::test]
async fn deleting_an_associated_profile_drops_that_association() {
    let client = Client::new();
    for holders in [1usize, 2, 3] {
        let case = format!("{holders}-workloads-hold-it");
        let (target, _) = created_profile(&client, &format!("target-profile-{holders}")).await;

        let mut workloads: Vec<String> = Vec::new();
        for index in 0..holders {
            let workload_id = created_id(
                &client
                    .create(minimal_create(
                        &format!("holder-workload-{holders}-{index}"),
                        &format!("tok-w-{holders}-{index}"),
                    ))
                    .await,
                &case,
            );
            let associated =
                associate_profiles(&client, &workload_id, std::slice::from_ref(&target)).await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}/holder-{index}: expected AssociateProfiles 200, got {associated:?}"
            );
            assert_eq!(
                associated_arns(&client, &workload_id, &case).await,
                vec![target.clone()],
                "case {case:?}/holder-{index}: the workload must report the association it \
                 was given"
            );
            workloads.push(workload_id);
        }

        // A workload holding ANOTHER profile: its association must survive the delete.
        let (bystander_profile, _) =
            created_profile(&client, &format!("bystander-profile-{holders}")).await;
        let bystander = created_id(
            &client
                .create(minimal_create(
                    &format!("bystander-workload-{holders}"),
                    &format!("tok-b-{holders}"),
                ))
                .await,
            &case,
        );
        assert_eq!(
            associate_profiles(&client, &bystander, std::slice::from_ref(&bystander_profile))
                .await
                .status,
            200,
            "case {case:?}: the bystander workload must take its own association"
        );

        assert_deleted(
            &client.delete_profile(&target, &format!("del-{case}")).await,
            &case,
        );

        for (index, workload_id) in workloads.iter().enumerate() {
            assert_eq!(
                associated_arns(&client, workload_id, &case).await,
                Vec::<String>::new(),
                "case {case:?}/holder-{index}: the deleted profile's association must go \
                 with it"
            );
        }
        assert_eq!(
            associated_arns(&client, &bystander, &case).await,
            vec![bystander_profile.clone()],
            "case {case:?}: a workload holding another profile keeps its association"
        );
        // The profile that was NOT deleted still resolves, so the delete took one profile
        // and the associations that named it.
        let fetched = client.get_profile(&bystander_profile, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: a profile the delete did not name must still resolve, \
             got {fetched:?}"
        );
        assert_eq!(
            fetched
                .body
                .get("Profile")
                .map(|profile| member_of(profile, "ProfileArn"))
                .unwrap_or_default(),
            bystander_profile,
            "case {case:?}: the kept profile must report its own ARN, got {fetched:?}"
        );
    }
}


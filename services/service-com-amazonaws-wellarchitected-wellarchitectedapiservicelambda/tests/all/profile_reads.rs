// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetProfile` — the read that reports the Well-Architected
//! Tool profile a `ProfileArn` resolves to (`GET /profiles/{ProfileArn}`, with the
//! optional `ProfileVersion` bound to the query string;
//! `smithy_query(query="operation", operation="GetProfile")`).
//!
//! The round trip of ONE created profile, its refused creates and its quota live in
//! `super::profile_lifecycle`. What this suite adds are the rules that only show up with
//! SEVERAL profiles in one store, each over a table of rows rather than one request:
//!
//! 1. IMPLEMENTED. `GetProfile` computes its answer from the store, so a request it can
//!    serve is answered 200 with the profile — never the scaffold channel's 501.
//! 2. RESOLUTION BY THE WHOLE ARN. A read is answered from the profile its `ProfileArn`
//!    identifies and from no other: with profiles stored that differ along every member
//!    a profile carries, each ARN reports its own members, and an ARN that is
//!    well-formed but differs from a stored one in ANY component — partition, Region,
//!    account or resource id — resolves to nothing and is answered
//!    `404 ResourceNotFoundException` carrying the ARN as sent.
//! 3. PURE READ. "Get profile information" reports; it does not revise. Any number of
//!    reads — of every stored profile, in any order, at the version the profile is at or
//!    without one, and reads that MISS — leave the stored profiles byte-identical and
//!    every later read's answer unchanged.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profile
//! request builder, the create assertions and the expected question projection live in
//! `super::profile_lifecycle`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::profile_lifecycle::{
    absent_profile_arn, created, expected_questions, member, minimal_profile, profile_of,
};
use super::workload_harness::Client;

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The Region every request in this suite is signed for — the harness' SigV4 credential
/// scope, which is how the handler learns the Region it mints an ARN in.
const SIGNING_REGION: &str = "us-west-2";

/// A `ProfileArn` assembled from its parts, so a row can vary ONE component of an ARN
/// that a stored profile otherwise matches. Every value produced here satisfies the
/// modelled
/// `^arn:aws[-a-z]*:wellarchitected:[a-z]{2}(-gov)?-[a-z]+-\d:\d{12}:profile/[a-z0-9]+$`
/// (`smithy_query(query="shape", shape="ProfileArn")`), so the answer is about
/// resolution and not about the value's syntax.
fn profile_arn(partition: &str, region: &str, account: &str, profile_id: &str) -> String {
    format!("arn:{partition}:wellarchitected:{region}:{account}:profile/{profile_id}")
}

/// The resource id of a minted `ProfileArn` — everything after `profile/`.
fn profile_id_of(profile_arn: &str) -> String {
    profile_arn
        .rsplit("profile/")
        .next()
        .expect("a minted ProfileArn ends in profile/<id>")
        .to_string()
}

/// The stored profiles as the state holds them, in ARN order, rendered so two snapshots
/// can be compared for equality. The HTTP boundary is still what every assertion about
/// behaviour goes through; this is only how a test says "the store did not move".
fn store_snapshot(client: &Client) -> String {
    let state = client.store();
    let store = state.lock();
    let ordered: BTreeMap<&String, String> = store
        .profiles
        .iter()
        .map(|(arn, record)| (arn, format!("{record:?}")))
        .collect();
    format!("{ordered:?}")
}

/// One profile the tests span, as the request that creates it.
///
/// The rows differ along what a read has to report: how many questions the profile
/// answers (none, one, several), how many choices one question selects, whether a choice
/// is named twice, whether the profile carries tags, and the name and description
/// lengths at their published `@length(3, 100)` bounds — so a read that reported a
/// neighbour's state, or a projection that lost a member, cannot pass.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        ("no-questions", json!({ "ProfileQuestions": [] })),
        (
            "one-question-one-choice",
            json!({
                "ProfileQuestions": [
                    { "QuestionId": "priority", "SelectedChoiceIds": ["priority_cost"] },
                ]
            }),
        ),
        (
            "several-questions-and-choices",
            json!({
                "ProfileQuestions": [
                    {
                        "QuestionId": "priority",
                        "SelectedChoiceIds": ["priority_rel", "priority_cost"],
                    },
                    { "QuestionId": "workload-type", "SelectedChoiceIds": ["type_batch"] },
                    { "QuestionId": "business-objective", "SelectedChoiceIds": [] },
                ]
            }),
        ),
        (
            "a-choice-named-twice",
            json!({
                "ProfileQuestions": [{
                    "QuestionId": "priority",
                    "SelectedChoiceIds": ["priority_perf", "priority_perf"],
                }]
            }),
        ),
        (
            "tagged",
            json!({ "Tags": { "Env": "test", "Team": "core" } }),
        ),
        (
            "name-and-description-at-the-minimum-length",
            json!({ "ProfileName": "abc", "ProfileDescription": "xyz" }),
        ),
        (
            "name-and-description-at-the-maximum-length",
            json!({ "ProfileName": "n".repeat(100), "ProfileDescription": "d".repeat(100) }),
        ),
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
    ]
}

/// One created profile: the request it was created from, and the `(ProfileArn,
/// ProfileVersion)` the create answered.
struct Created {
    case: &'static str,
    request: Value,
    arn: String,
    version: String,
}

/// Create every row of [`rows`] in ONE store, so every read afterwards has neighbours to
/// confuse it with.
async fn seed(client: &Client) -> Vec<Created> {
    let mut created_profiles = Vec::new();
    for (case, overrides) in rows() {
        let mut request = minimal_profile(&format!("profile-{case}"), &format!("tok-{case}"));
        for (name, value) in overrides.as_object().expect("overrides are an object") {
            request[name] = value.clone();
        }
        let answer = client.create_profile(request.clone()).await;
        let (arn, version) = created(&answer, case);
        created_profiles.push(Created {
            case,
            request,
            arn,
            version,
        });
    }
    created_profiles
}

/// Assert the profile `answer` reports is the one `expected` was created from: the
/// members the request carried, as the request carried them, plus the ARN and version
/// the create answered.
fn assert_reports_its_own_profile(answer: &super::workload_harness::Answer, expected: &Created) {
    let case = expected.case;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetProfile must serve a stored profile from the store, got {answer:?}"
    );
    let profile = profile_of(answer);
    assert_eq!(
        profile.get("ProfileArn").and_then(Value::as_str),
        Some(expected.arn.as_str()),
        "case {case:?}: the read must report the profile the ARN names, got {profile:?}"
    );
    assert_eq!(
        profile.get("ProfileVersion").and_then(Value::as_str),
        Some(expected.version.as_str()),
        "case {case:?}: the read must report the version the create answered, got {profile:?}"
    );
    for name in ["ProfileName", "ProfileDescription"] {
        assert_eq!(
            profile.get(name),
            expected.request.get(name),
            "case {case:?}: {name} must be this profile's own, got {profile:?}"
        );
    }
    let expected_tags = expected
        .request
        .get("Tags")
        .cloned()
        .unwrap_or_else(|| json!({}));
    assert_eq!(
        profile.get("Tags"),
        Some(&expected_tags),
        "case {case:?}: expected Tags {expected_tags:?}, got {profile:?}"
    );
    let questions = expected_questions(&expected.request["ProfileQuestions"]);
    assert_eq!(
        profile.get("ProfileQuestions"),
        Some(&questions),
        "case {case:?}: expected ProfileQuestions {questions:?}, got {profile:?}"
    );
    assert_eq!(
        profile.get("Owner").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}: the creating account owns the profile, got {profile:?}"
    );
}

/// Rules 1 and 2: every stored profile resolves to ITS OWN state, whichever order the
/// reads arrive in and whether or not they name the version the profile is at.
#[tokio::test]
async fn each_stored_profile_resolves_to_its_own_state() {
    let client = Client::new();
    let profiles = seed(&client).await;
    assert_eq!(
        client.store().lock().profiles.len(),
        profiles.len(),
        "case \"seed\": every row must have created exactly one profile"
    );

    // Forward, then reversed, then every profile read at the version it is at: a read
    // that leaked a neighbour's state, or that depended on the order of the reads before
    // it, fails one of these passes.
    for expected in &profiles {
        assert_reports_its_own_profile(&client.get_profile(&expected.arn, None).await, expected);
    }
    for expected in profiles.iter().rev() {
        assert_reports_its_own_profile(&client.get_profile(&expected.arn, None).await, expected);
    }
    for expected in &profiles {
        let at_version = client
            .get_profile(&expected.arn, Some(&expected.version))
            .await;
        assert_reports_its_own_profile(&at_version, expected);
        assert_eq!(
            profile_of(&at_version),
            profile_of(&client.get_profile(&expected.arn, None).await),
            "case {:?}: reading at the stored version must report the same profile as reading \
             without one",
            expected.case
        );
    }

    // No two profiles were reported as the same resource: the ARNs are distinct and each
    // read reported its own.
    let mut arns: Vec<&str> = profiles.iter().map(|row| row.arn.as_str()).collect();
    arns.sort_unstable();
    let distinct = arns.len();
    arns.dedup();
    assert_eq!(
        arns.len(),
        distinct,
        "case \"seed\": every created profile must carry its own ARN, got {arns:?}"
    );
}

/// Rule 2 for an ARN that names nothing: resolution is on the WHOLE ARN, so a
/// well-formed ARN differing from a stored one in any single component resolves to
/// nothing. The rows differ along WHICH component differs — partition, Region, account,
/// resource id — and the resource-id rows along its form.
#[tokio::test]
async fn a_profile_arn_differing_in_any_component_resolves_to_nothing() {
    let client = Client::new();
    let stored = client
        .create_profile(minimal_profile("component-profile", "tok-component"))
        .await;
    let (arn, version) = created(&stored, "component");
    let profile_id = profile_id_of(&arn);
    // The truncated row below drops one character, so it is only a well-formed
    // `ProfileArn` (whose resource part is `[a-z0-9]+`) while the minted id is longer
    // than one character.
    assert!(
        profile_id.len() > 1,
        "case \"component\": the minted resource id must be long enough to truncate, got \
         {profile_id:?}"
    );
    let account = engine::emulator_account_id();

    let cases: Vec<(&str, String)> = vec![
        // Another partition: the modelled `arn:aws[-a-z]*` admits them, and a profile of
        // this store is not in one.
        (
            "another-partition",
            profile_arn("aws-cn", "cn-north-1", account, &profile_id),
        ),
        (
            "gov-partition",
            profile_arn("aws-us-gov", "us-gov-west-1", account, &profile_id),
        ),
        // Another Region, same account and resource id.
        (
            "another-region",
            profile_arn("aws", "eu-west-3", account, &profile_id),
        ),
        (
            "another-region-same-prefix",
            profile_arn("aws", "us-west-1", account, &profile_id),
        ),
        // Another account, same Region and resource id.
        (
            "another-account",
            profile_arn("aws", SIGNING_REGION, "210987654321", &profile_id),
        ),
        (
            "all-nines-account",
            profile_arn("aws", SIGNING_REGION, "999999999999", &profile_id),
        ),
        // Another resource id, same partition, Region and account.
        (
            "resource-id-with-a-suffix",
            profile_arn("aws", SIGNING_REGION, account, &format!("{profile_id}0")),
        ),
        (
            "resource-id-truncated",
            profile_arn(
                "aws",
                SIGNING_REGION,
                account,
                &profile_id[..profile_id.len() - 1],
            ),
        ),
        (
            "unrelated-resource-id",
            profile_arn("aws", SIGNING_REGION, account, "deadbeef00"),
        ),
    ];

    for (case, absent) in cases {
        assert_ne!(
            absent, arn,
            "case {case:?}: the row must differ from the ARN that resolves"
        );
        // Without a version, and at the version the stored profile IS at: neither
        // resolves, because the profile is resolved first.
        for (shape, answer) in [
            ("no-version", client.get_profile(&absent, None).await),
            (
                "at-the-stored-version",
                client.get_profile(&absent, Some(&version)).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}/{shape}: an ARN naming no stored profile must be a 404, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                absent,
                "case {case:?}/{shape}: the 404 must carry the ARN as sent, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "PROFILE",
                "case {case:?}/{shape}: the 404 must name the missing resource's type, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                format!(
                    "[ResourceNotFound] No profile with ARN {absent} was found in account \
                     {account}."
                ),
                "case {case:?}/{shape}: the 404 message must name the ARN asked for and the \
                 account it was looked for in, got {answer:?}"
            );
        }
    }

    // The ARN the create minted still resolves, so every row above failed on resolution
    // and not on a broken route.
    let present = client.get_profile(&arn, None).await;
    assert_eq!(
        present.status, 200,
        "case \"stored\": the minted ARN must still resolve, got {present:?}"
    );
}

/// Rule 3: reading reports, it never revises. A batch of reads that spans every stored
/// profile, both ways of naming a version, and reads that MISS leaves the stored
/// profiles byte-identical and every answer unchanged.
#[tokio::test]
async fn reading_a_profile_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let profiles = seed(&client).await;

    let before_store = store_snapshot(&client);
    let before: Vec<Value> = {
        let mut answers = Vec::new();
        for expected in &profiles {
            answers.push(profile_of(&client.get_profile(&expected.arn, None).await));
        }
        answers
    };

    // Reads that succeed, reads at a version, and reads that miss — on an absent ARN and
    // on a version the stored profile is not at.
    let absent = absent_profile_arn("absent00");
    for round in 0..3 {
        for expected in &profiles {
            let case = format!("{}/round-{round}", expected.case);
            assert_eq!(
                client.get_profile(&expected.arn, None).await.status,
                200,
                "case {case:?}: a repeated read must still resolve"
            );
            assert_eq!(
                client
                    .get_profile(&expected.arn, Some(&expected.version))
                    .await
                    .status,
                200,
                "case {case:?}: a repeated read at the stored version must still resolve"
            );
            for absent_version in ["2", "LATEST"] {
                let answer = client
                    .get_profile(&expected.arn, Some(absent_version))
                    .await;
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: a version the profile is not at must be a 404, got {answer:?}"
                );
            }
        }
        let missed = client.get_profile(&absent, None).await;
        assert_eq!(
            missed.status, 404,
            "case \"absent/round-{round}\": an absent ARN must be a 404, got {missed:?}"
        );
    }

    assert_eq!(
        store_snapshot(&client),
        before_store,
        "case \"store\": no read may change a stored profile"
    );
    for (expected, before_profile) in profiles.iter().zip(before) {
        let after = profile_of(&client.get_profile(&expected.arn, None).await);
        assert_eq!(
            after, before_profile,
            "case {:?}: the profile a later caller reads must be the one the first read \
             reported",
            expected.case
        );
    }
}

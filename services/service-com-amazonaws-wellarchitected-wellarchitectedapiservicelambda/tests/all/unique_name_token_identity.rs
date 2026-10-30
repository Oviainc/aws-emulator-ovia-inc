// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule every `@idempotencyToken` operation of this
//! service that carries a NAME follows: a retry is compared on the request's parameters as
//! the service RESOLVES them, so a name whose uniqueness the service meters loosely counts
//! as the name it names and not as the spelling it was named by.
//!
//! "If you retry a request with the same client request token and the same parameters after
//! the original request has completed successfully, the result of the original request is
//! returned" (`smithy_query(query="doc", shape="CreateMilestoneInput",
//! member="ClientRequestToken")`), and the comparison that decides which name a request
//! names is the service's own: "the name must be unique within an account within an Amazon
//! Web Services Region. Spaces and capitalization are ignored when checking for uniqueness"
//! (`smithy_query(query="doc", shape="WorkloadName")`), the same comparison that decides
//! "milestone names must be unique within a workload" (`smithy_query(query="doc",
//! shape="MilestoneName")`) and a review template's name within the account. Two spellings
//! differing only in capitalization or whitespace are therefore ONE name, so a retry under a
//! spent token spelling it the other way carries the SAME parameters: it replays the
//! original request's result rather than being refused as a different request (409).
//!
//! The rules pinned here, each over a TABLE of spellings rather than one recorded request:
//!
//! 1. `CreateMilestone`: every pair of spellings — the one the milestone was recorded under
//!    and the one the retry names it by — replays the original `MilestoneNumber`, records no
//!    second milestone and leaves the milestone stored under the spelling it was created
//!    with.
//! 2. Taking the name as the name it names does not flatten the fingerprint: a token
//!    replayed with a GENUINELY different name, or against a different workload, is still
//!    another request and the modelled 409.
//! 3. `UpgradeLensReview` follows the rule for the milestone it keeps the review as: the
//!    retry replays the empty success and records no second milestone.
//! 4. `UpgradeProfileVersion` follows it the same way.
//! 5. `CreateWorkload` follows it for the workload name it creates under: the retry answers
//!    the `WorkloadId` the original create minted, and creates no second workload.
//! 6. `CreateReviewTemplate` follows it for the template name, the same way.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`; what a replay left behind is read back through
//! `GetMilestone`, `ListMilestones`, `GetWorkload` and the shared store counters, because a
//! replayed upgrade answers no members at all.

use serde_json::{json, Value};

use super::workload_harness::{
    created_arn, created_id, created_profile, encode_path_segment, member, minimal_create,
    minimal_template, stored_template_count, stored_workload_count, Answer, Client,
};

/// `MilestoneName` and `TemplateName` are both `@length(min: 3, max: 100)`
/// (`smithy_query(query="shape", shape="MilestoneName")` /
/// `smithy_query(query="shape", shape="TemplateName")`), and `WorkloadName` is
/// `@length(min: 3, max: 100)` too, so every spelling a row submits stays inside that bound.
const NAME_RANGE: (usize, usize) = (3, 100);

/// The stored spellings every table below is run for: a plain name, one carrying inner
/// spaces and one mixing capitalization, so the rows differ in what the loose comparison has
/// to drop before the two spellings are one name.
const STORED_NAMES: [&str; 3] = ["ms-plain", "Release Candidate", "MiXeD Case Name"];

/// How a retry respells a stored name: the label and the transformation. Each row is a
/// spelling the service's comparison treats as the SAME name, so each row is a RETRY.
fn respellings(stored_name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("identical", stored_name.to_string()),
        ("upper", stored_name.to_uppercase()),
        ("lower", stored_name.to_lowercase()),
        ("padded", format!("  {stored_name} ")),
        ("whitespace-removed", stored_name.replace(' ', "")),
        ("tab-separated", stored_name.replace(' ', "\t")),
        ("inner-space-added", stored_name.replace('-', " - ")),
    ]
}

/// Every row has to stay inside the modelled length of the name it spells, so a refusal a
/// row draws is the handler's and not the boundary validator's.
fn assert_in_range(case: &str, submitted: &str) {
    assert!(
        (NAME_RANGE.0..=NAME_RANGE.1).contains(&submitted.chars().count()),
        "case {case:?}: the row must stay inside @length{NAME_RANGE:?}, got {submitted:?}"
    );
}

/// `POST /workloads/{WorkloadId}/milestones`, the wire form of `CreateMilestone`: both
/// `MilestoneName` and `ClientRequestToken` are bound to no HTTP member, so both travel in
/// the body.
async fn create_milestone(client: &Client, workload_id: &str, name: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": token })),
        )
        .await
}

/// The `MilestoneNumber`s `ListMilestones` reports for a workload, in the order it reports
/// them.
async fn milestone_numbers(client: &Client, workload_id: &str, case: &str) -> Vec<i64> {
    let answer = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestonesSummaries"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListMilestones 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("MilestoneNumber").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

/// The `MilestoneName` `GetMilestone` reports for one number of one workload — the spelling
/// the milestone is STORED under, which a replay must leave as it was.
async fn stored_milestone_name(
    client: &Client,
    workload_id: &str,
    number: i64,
    case: &str,
) -> String {
    let answer = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{number}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetMilestone 200, got {answer:?}"
    );
    answer
        .body
        .get("Milestone")
        .and_then(|milestone| milestone.get("MilestoneName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A workload created over HTTP, named after the case so no two rows share one.
async fn workload(client: &Client, name: &str) -> String {
    created_id(
        &client
            .create(minimal_create(name, &format!("tok-wl-{name}")))
            .await,
        name,
    )
}

/// The `MilestoneNumber` a successful `CreateMilestone` answered with.
fn created_number(answer: &Answer, case: &str) -> i64 {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateMilestone 200, got {answer:?}"
    );
    answer
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: MilestoneNumber must be present, got {answer:?}"))
}

/// The refusal a token replayed over a DIFFERENT request gets: 409 `ConflictException` and
/// the message naming the token.
fn assert_token_conflict(case: &str, answer: &Answer, token: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        format!(
            "[Conflict] A request with ClientRequestToken {token} was already submitted with \
             different parameters."
        ),
        "case {case:?}: expected the replayed-token conflict message, got {answer:?}"
    );
}

/// Rule 1. The rows differ along the stored spelling and, for each, along how the retry
/// respells it.
#[tokio::test]
async fn a_create_milestone_retry_respelling_the_name_replays_the_milestone() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        for (spelling, retried) in respellings(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_in_range(&case, &retried);

            // One client and one workload per row, so a row observes only its own writes.
            let client = Client::new();
            let workload_id = workload(&client, &format!("ms-host-{index}-{spelling}")).await;
            let token = format!("tok-ms-{index}-{spelling}");

            let original = create_milestone(&client, &workload_id, stored_name, &token).await;
            let number = created_number(&original, &case);

            let replay = create_milestone(&client, &workload_id, &retried, &token).await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: a retry naming the same milestone must replay, got {replay:?}"
            );
            assert_eq!(
                replay.body.get("MilestoneNumber").and_then(Value::as_i64),
                Some(number),
                "case {case:?}: expected the original MilestoneNumber {number}, got {replay:?}"
            );
            assert_eq!(
                member(&replay, "WorkloadId"),
                workload_id,
                "case {case:?}: the replay must name the original workload, got {replay:?}"
            );

            // The replay recorded nothing: the one milestone is still the one milestone,
            // and it is still stored under the spelling it was created with.
            assert_eq!(
                milestone_numbers(&client, &workload_id, &case).await,
                vec![number],
                "case {case:?}: one token must have recorded exactly one milestone"
            );
            assert_eq!(
                stored_milestone_name(&client, &workload_id, number, &case).await,
                *stored_name,
                "case {case:?}: a replay must leave the stored spelling as it was"
            );
        }
    }
}

/// Rule 2. A token is still spent per REQUEST: a name the comparison does not treat as the
/// stored one, and the same name under another workload, are each another request.
#[tokio::test]
async fn the_milestone_name_identity_does_not_flatten_the_token_fingerprint() {
    // Each row is a name that differs from the stored one by more than capitalization and
    // whitespace: a different word, a different character, and the same letters in another
    // order.
    const STORED_NAME: &str = "Release Candidate";
    let different_names = [
        ("other-word", "Release Candidates"),
        ("other-character", "Release Candidat3"),
        ("reordered", "Candidate Release"),
    ];

    for (spelling, different) in different_names {
        let case = format!("name/{spelling}");
        assert_in_range(&case, different);

        let client = Client::new();
        let workload_id = workload(&client, &format!("ms-diff-{spelling}")).await;
        let token = format!("tok-ms-diff-{spelling}");
        let original = create_milestone(&client, &workload_id, STORED_NAME, &token).await;
        let number = created_number(&original, &case);

        let refused = create_milestone(&client, &workload_id, different, &token).await;
        assert_token_conflict(&case, &refused, &token);
        assert_eq!(
            member(&refused, "ResourceType"),
            "MILESTONE",
            "case {case:?}: the conflict must name the resource type, got {refused:?}"
        );
        assert_eq!(
            member(&refused, "ResourceId"),
            format!("{workload_id}.{number}"),
            "case {case:?}: the conflict must name the milestone the token recorded, got \
             {refused:?}"
        );
        assert_eq!(
            milestone_numbers(&client, &workload_id, &case).await,
            vec![number],
            "case {case:?}: the refusal must have recorded nothing"
        );
    }

    // The workload the milestone is OF is part of the parameters too, under every spelling
    // of the name.
    for (spelling, retried) in respellings(STORED_NAME) {
        let case = format!("workload/{spelling}");
        let client = Client::new();
        let first = workload(&client, &format!("ms-host-a-{spelling}")).await;
        let second = workload(&client, &format!("ms-host-b-{spelling}")).await;
        let token = format!("tok-ms-host-{spelling}");
        created_number(
            &create_milestone(&client, &first, STORED_NAME, &token).await,
            &case,
        );

        let refused = create_milestone(&client, &second, &retried, &token).await;
        assert_token_conflict(&case, &refused, &token);
        assert!(
            milestone_numbers(&client, &second, &case).await.is_empty(),
            "case {case:?}: the refused request must have recorded no milestone of the other \
             workload"
        );
    }
}

/// `PUT /workloads/{WorkloadId}/lensReviews/{LensAlias}/upgrade`, the wire form of
/// `UpgradeLensReview`: both labels are non-greedy, so each travels percent-encoded in one
/// path segment, and `MilestoneName` and `ClientRequestToken` travel in the body.
async fn upgrade_lens_review(
    client: &Client,
    workload_id: &str,
    milestone_name: &str,
    token: &str,
) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/workloads/{}/lensReviews/{}/upgrade",
                encode_path_segment(workload_id),
                encode_path_segment("wellarchitected")
            ),
            Some(json!({ "MilestoneName": milestone_name, "ClientRequestToken": token })),
        )
        .await
}

/// Rule 3. The rows differ along the stored spelling and how the retry respells it.
#[tokio::test]
async fn an_upgrade_lens_review_retry_respelling_its_milestone_name_replays() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        for (spelling, retried) in respellings(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_in_range(&case, &retried);

            let client = Client::new();
            let workload_id = workload(&client, &format!("ulr-host-{index}-{spelling}")).await;
            let token = format!("tok-ulr-{index}-{spelling}");

            let original = upgrade_lens_review(&client, &workload_id, stored_name, &token).await;
            assert_eq!(
                original.status, 200,
                "case {case:?}: expected UpgradeLensReview 200, got {original:?}"
            );
            let recorded = milestone_numbers(&client, &workload_id, &case).await;

            let replay = upgrade_lens_review(&client, &workload_id, &retried, &token).await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: a retry naming the same milestone must replay, got {replay:?}"
            );
            assert_eq!(
                milestone_numbers(&client, &workload_id, &case).await,
                recorded,
                "case {case:?}: the replay must record no second milestone"
            );
        }
    }
}

/// `PUT /workloads/{WorkloadId}/profiles/{ProfileArn}/upgrade`, the wire form of
/// `UpgradeProfileVersion`. The ARN is a non-greedy `@httpLabel`, so it travels
/// percent-encoded in one path segment, exactly as an SDK sends it.
async fn upgrade_profile_version(
    client: &Client,
    workload_id: &str,
    profile_arn: &str,
    milestone_name: &str,
    token: &str,
) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/workloads/{workload_id}/profiles/{}/upgrade",
                encode_path_segment(profile_arn)
            ),
            Some(json!({ "MilestoneName": milestone_name, "ClientRequestToken": token })),
        )
        .await
}

/// Rule 4. The rows differ along the stored spelling and how the retry respells it.
#[tokio::test]
async fn an_upgrade_profile_version_retry_respelling_its_milestone_name_replays() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        for (spelling, retried) in respellings(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_in_range(&case, &retried);

            let client = Client::new();
            let host = format!("upv-host-{index}-{spelling}");
            let workload_id = workload(&client, &host).await;
            let (profile_arn, _) = created_profile(&client, &host).await;
            let associated = client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload_id}/associateProfiles"),
                    Some(json!({ "ProfileArns": [profile_arn] })),
                )
                .await;
            assert_eq!(
                associated.status, 200,
                "case {case:?}: expected AssociateProfiles 200, got {associated:?}"
            );
            let token = format!("tok-upv-{index}-{spelling}");

            let original =
                upgrade_profile_version(&client, &workload_id, &profile_arn, stored_name, &token)
                    .await;
            assert_eq!(
                original.status, 200,
                "case {case:?}: expected UpgradeProfileVersion 200, got {original:?}"
            );
            let recorded = milestone_numbers(&client, &workload_id, &case).await;

            let replay =
                upgrade_profile_version(&client, &workload_id, &profile_arn, &retried, &token)
                    .await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: a retry naming the same milestone must replay, got {replay:?}"
            );
            assert_eq!(
                milestone_numbers(&client, &workload_id, &case).await,
                recorded,
                "case {case:?}: the replay must record no second milestone"
            );
        }
    }
}

/// Rule 5. The same rule for the workload name a `CreateWorkload` creates under: the rows
/// differ along the stored spelling and how the retry respells it.
#[tokio::test]
async fn a_create_workload_retry_respelling_the_name_replays_the_workload() {
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        for (spelling, retried) in respellings(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_in_range(&case, &retried);

            let client = Client::new();
            let token = format!("tok-cw-{index}-{spelling}");
            let created = client.create(minimal_create(stored_name, &token)).await;
            let original = created_id(&created, &case);

            let replay = client.create(minimal_create(&retried, &token)).await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: a retry naming the same workload must replay, got {replay:?}"
            );
            assert_eq!(
                member(&replay, "WorkloadId"),
                original,
                "case {case:?}: expected the original WorkloadId, got {replay:?}"
            );
            assert_eq!(
                stored_workload_count(&client),
                1,
                "case {case:?}: one token must have created exactly one workload"
            );

            // The replay wrote nothing: the workload still reports the spelling it was
            // created under.
            let fetched = client.get(&original).await;
            assert_eq!(
                fetched
                    .body
                    .get("Workload")
                    .and_then(|workload| workload.get("WorkloadName"))
                    .and_then(Value::as_str),
                Some(*stored_name),
                "case {case:?}: a replay must leave the stored spelling as it was, got {fetched:?}"
            );
        }
    }
}

/// Rule 6. The same rule for the template name a `CreateReviewTemplate` creates under.
#[tokio::test]
async fn a_create_review_template_retry_respelling_the_name_replays_the_template() {
    // `TemplateName`'s pattern is `^[A-Za-z0-9-_.,:/()@!&?#+'’\s]+$`
    // (`smithy_query(query="shape", shape="TemplateName")`), which every spelling below
    // satisfies.
    for (index, stored_name) in STORED_NAMES.iter().enumerate() {
        for (spelling, retried) in respellings(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_in_range(&case, &retried);

            let client = Client::new();
            let token = format!("tok-crt-{index}-{spelling}");
            let original = created_arn(
                &client
                    .create_review_template(minimal_template(stored_name, &token))
                    .await,
                &case,
            );

            let replay = client
                .create_review_template(minimal_template(&retried, &token))
                .await;
            assert_eq!(
                replay.status, 200,
                "case {case:?}: a retry naming the same template must replay, got {replay:?}"
            );
            assert_eq!(
                member(&replay, "TemplateArn"),
                original,
                "case {case:?}: expected the original TemplateArn, got {replay:?}"
            );
            assert_eq!(
                stored_template_count(&client),
                1,
                "case {case:?}: one token must have created exactly one review template"
            );
        }
    }
}

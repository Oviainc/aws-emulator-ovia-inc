// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE rule this crate follows for a `WorkloadId` carried as
//! a path label: it is RESOLVED, not validated.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. A `WorkloadId` bound to the request path is resolved by the handler that owns the
//!    lookup, never refused by the boundary validator: an id naming no stored workload is
//!    answered `404 ResourceNotFoundException` with `ResourceType` `WORKLOAD`, the id AS
//!    SENT and the shared not-found message, WHATEVER the id's syntax. The rows differ
//!    along that syntax — well-formed-but-absent, and six shapes the modelled
//!    `@length(32, 32)` / `@pattern("^[0-9a-f]{32}$")` refuses — crossed with every
//!    operation of the workload plane that resolves one, so the answer cannot depend on
//!    either.
//! 2. Dropping that ONE member's constraint traits does not widen the boundary walk: every
//!    other member of the same shapes is still refused with `400 ValidationException` —
//!    whether the request names a workload that exists or one whose id is malformed, which
//!    is what shows the walk still runs for a request whose path id it no longer judges.
//!
//! The operations the table covers are the ones `super::lookup_error_envelopes` and
//! `super::workload_shares` do not: those pin rule 1 on `GetWorkload`, `UpdateWorkload`,
//! `DeleteWorkload`, `CreateMilestone`, `CreateWorkloadShare` and `ListWorkloadShares`
//! already.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the request builders and the lookup-failure assertion are
//! the shared `super::workload_harness`'s.

use serde_json::json;

use super::workload_harness::{
    assert_not_found, created_id, created_profile, encode_path_segment, member, minimal_create,
    official_lens_arn, Answer, Client, SIGNED_REGION,
};

/// The alias of the one AWS-official lens every workload here reviews (`minimal_create`
/// applies it).
const FRAMEWORK_LENS: &str = "wellarchitected";

/// A lens alias AWS publishes that these workloads do not review, so `AssociateLenses` has
/// a lens to add that is not already applied.
const UNAPPLIED_LENS: &str = "serverless";

/// One best practice of the framework lens, as `super::check_summaries` reads it back from
/// the published catalog: the pillar, the question it asks and the choice it offers, which
/// is what the two check lists name.
const PILLAR_ID: &str = "security";
/// See [`PILLAR_ID`].
const CHECK_QUESTION_ID: &str = "permissions";
/// See [`PILLAR_ID`].
const CHOICE_ID: &str = "sec_permissions_analyze_cross_account";

/// A question of the framework lens, which the answer reads and writes name.
const ANSWER_QUESTION_ID: &str = "organization-priorities";

/// The one `IntegratingService` the model publishes
/// (`smithy_query(query="shape", shape="IntegratingService")`).
const JIRA: &str = "JIRA";

/// The resource type a missing workload is reported under.
const WORKLOAD_RESOURCE_TYPE: &str = "WORKLOAD";

/// The ids rule 1 is asserted over: two the modelled `WorkloadId` constraints ACCEPT and
/// that no workload carries, then six the constraints refuse — one short of the fixed
/// `@length(32, 32)`, one over it, uppercase hex, a non-hex digit in an otherwise
/// well-formed id, and values of an altogether different shape.
fn unresolvable_ids() -> Vec<(&'static str, String)> {
    vec![
        ("well-formed-absent-zeros", "0".repeat(32)),
        ("well-formed-absent-f", "f".repeat(32)),
        ("one-character-short", "0".repeat(31)),
        ("one-character-long", "0".repeat(33)),
        ("uppercase-hex", "A".repeat(32)),
        ("hex-with-a-non-hex-digit", format!("{}z", "0".repeat(31))),
        ("not-hex-at-all", "not-a-workload-id".to_string()),
        ("digits-only-but-short", "12345".to_string()),
    ]
}

/// The body the two check lists carry: every member but the workload id travels there,
/// which is where restJson1 binds a member the model gives no HTTP binding.
fn best_practice() -> serde_json::Value {
    json!({
        "LensArn": official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS),
        "PillarId": PILLAR_ID,
        "QuestionId": CHECK_QUESTION_ID,
        "ChoiceId": CHOICE_ID,
    })
}

/// Every operation of the workload plane that resolves a path-bound `WorkloadId` and whose
/// shape this suite covers, issued against `workload_id` and answered in order.
///
/// `share_id` and `profile_arn` are well-formed values of their own members, so the only
/// thing a row varies is the workload id: an identifier the boundary WOULD refuse on its
/// own merits (a `ShareId` outside its pattern, an ARN outside the `ProfileArn` pattern)
/// would mask the answer under test. `case` keys the idempotency tokens, so no row replays
/// another's.
async fn workload_plane_operations(
    client: &Client,
    workload_id: &str,
    share_id: &str,
    profile_arn: &str,
    case: &str,
) -> Vec<(&'static str, Answer)> {
    let workload = encode_path_segment(workload_id);
    let share = encode_path_segment(share_id);
    let profile = encode_path_segment(profile_arn);
    let lens = encode_path_segment(FRAMEWORK_LENS);

    vec![
        (
            "UpdateWorkloadShare",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/shares/{share}"),
                    Some(json!({ "PermissionType": "READONLY" })),
                )
                .await,
        ),
        (
            "DeleteWorkloadShare",
            client
                .send(
                    "DELETE",
                    &format!(
                        "/workloads/{workload}/shares/{share}?ClientRequestToken=share-{case}"
                    ),
                    None,
                )
                .await,
        ),
        (
            "ListLensReviews",
            client
                .send("GET", &format!("/workloads/{workload}/lensReviews"), None)
                .await,
        ),
        (
            "GetLensReview",
            client
                .send(
                    "GET",
                    &format!("/workloads/{workload}/lensReviews/{lens}"),
                    None,
                )
                .await,
        ),
        (
            "ListLensReviewImprovements",
            client
                .send(
                    "GET",
                    &format!("/workloads/{workload}/lensReviews/{lens}/improvements"),
                    None,
                )
                .await,
        ),
        (
            "ListAnswers",
            client
                .send(
                    "GET",
                    &format!("/workloads/{workload}/lensReviews/{lens}/answers"),
                    None,
                )
                .await,
        ),
        (
            "GetAnswer",
            client
                .send(
                    "GET",
                    &format!(
                        "/workloads/{workload}/lensReviews/{lens}/answers/{ANSWER_QUESTION_ID}"
                    ),
                    None,
                )
                .await,
        ),
        (
            "UpdateAnswer",
            client
                .send(
                    "PATCH",
                    &format!(
                        "/workloads/{workload}/lensReviews/{lens}/answers/{ANSWER_QUESTION_ID}"
                    ),
                    Some(json!({ "Notes": "a note the workload has to exist to hold" })),
                )
                .await,
        ),
        (
            "UpdateLensReview",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/lensReviews/{lens}"),
                    Some(json!({ "LensNotes": "a review note" })),
                )
                .await,
        ),
        (
            "ListCheckDetails",
            client
                .send(
                    "POST",
                    &format!("/workloads/{workload}/checks"),
                    Some(best_practice()),
                )
                .await,
        ),
        (
            "ListCheckSummaries",
            client
                .send(
                    "POST",
                    &format!("/workloads/{workload}/checkSummaries"),
                    Some(best_practice()),
                )
                .await,
        ),
        (
            "UpdateIntegration",
            client
                .send(
                    "POST",
                    &format!("/workloads/{workload}/updateIntegration"),
                    Some(json!({
                        "ClientRequestToken": format!("integration-{case}"),
                        "IntegratingService": JIRA,
                    })),
                )
                .await,
        ),
        (
            "AssociateLenses",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/associateLenses"),
                    Some(json!({ "LensAliases": [UNAPPLIED_LENS] })),
                )
                .await,
        ),
        (
            "UpgradeLensReview",
            client
                .send(
                    "PUT",
                    &format!("/workloads/{workload}/lensReviews/{lens}/upgrade"),
                    Some(json!({ "MilestoneName": format!("upgrade-{case}") })),
                )
                .await,
        ),
        (
            "AssociateProfiles",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/associateProfiles"),
                    Some(json!({ "ProfileArns": [profile_arn] })),
                )
                .await,
        ),
        (
            "UpgradeProfileVersion",
            client
                .send(
                    "PUT",
                    &format!("/workloads/{workload}/profiles/{profile}/upgrade"),
                    Some(json!({})),
                )
                .await,
        ),
        (
            "DisassociateProfiles",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/disassociateProfiles"),
                    Some(json!({ "ProfileArns": [profile_arn] })),
                )
                .await,
        ),
        (
            "DisassociateLenses",
            client
                .send(
                    "PATCH",
                    &format!("/workloads/{workload}/disassociateLenses"),
                    Some(json!({ "LensAliases": [UNAPPLIED_LENS] })),
                )
                .await,
        ),
    ]
}

/// A workload reviewing the framework lens, one share of it and one profile, which is what
/// the table's requests need well-formed identifiers for.
async fn fixture(client: &Client, name: &str) -> (String, String, String) {
    let workload_id = created_id(&client.create(minimal_create(name, &format!("create-{name}"))).await, name);
    let created = client
        .create_share(
            &workload_id,
            json!({
                "SharedWith": "111122223333",
                "PermissionType": "READONLY",
                "ClientRequestToken": format!("share-{name}"),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateWorkloadShare must succeed, got {created:?}"
    );
    let share_id = member(&created, "ShareId").to_string();
    assert!(
        !share_id.is_empty(),
        "case {name:?}: CreateWorkloadShare must answer a ShareId, got {created:?}"
    );
    let (profile_arn, _version) = created_profile(client, &format!("profile-{name}")).await;
    (workload_id, share_id, profile_arn)
}

/// Rule 1: every operation of the workload plane answers an unresolvable `WorkloadId` with
/// the lookup's 404, whatever the id's syntax.
#[tokio::test]
async fn an_unresolvable_workload_id_is_the_lookups_not_found_on_every_workload_plane_operation() {
    let client = Client::new();
    let (present, share_id, profile_arn) = fixture(&client, "present").await;

    for (case, workload_id) in unresolvable_ids() {
        assert_ne!(
            workload_id, present,
            "case {case:?}: the row must not name the workload that exists"
        );
        for (operation, answer) in
            workload_plane_operations(&client, &workload_id, &share_id, &profile_arn, case).await
        {
            assert_not_found(
                &format!("{case}/{operation}"),
                &answer,
                WORKLOAD_RESOURCE_TYPE,
                &workload_id,
                &format!("workload with ID {workload_id}"),
            );
        }
    }

    // The same table against the workload that DOES exist resolves it: no row is the
    // WORKLOAD 404 above, so every refusal in the loop is the lookup missing and not a
    // route, a label decode or a request body the boundary would have refused anyway. What
    // each row answers BEYOND that is each operation's own suite's to pin.
    for (operation, answer) in
        workload_plane_operations(&client, &present, &share_id, &profile_arn, "present").await
    {
        let missing_workload =
            answer.status == 404 && member(&answer, "ResourceType") == WORKLOAD_RESOURCE_TYPE;
        assert!(
            !missing_workload,
            "case \"present\"/{operation}: the stored workload must resolve, got {answer:?}"
        );
    }

    // And it still reads back whole through `GetWorkload`.
    let fetched = client.get(&present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored workload must still resolve, got {fetched:?}"
    );
}

/// Rule 2: the members BESIDE the path id are still judged at the boundary, for a request
/// naming a workload that exists and for one whose path id is malformed alike.
#[tokio::test]
async fn the_members_beside_the_path_id_are_still_refused_at_the_boundary() {
    let client = Client::new();
    let (present, share_id, _profile_arn) = fixture(&client, "boundary").await;
    let absent = "not-a-workload-id";

    // Each row breaks ONE member's published constraint: a `@pattern`, an enum value set,
    // a `@length` bound and a `@range` bound, across the path, the query string and the
    // body (every bound from `smithy_query(query="operation", operation=<the operation>)`).
    for (case, method, path, query, body) in [
        (
            "share-id-outside-its-pattern",
            "PATCH",
            format!("/workloads/{{workload}}/shares/{}", "not-a-share-id"),
            String::new(),
            Some(json!({ "PermissionType": "READONLY" })),
        ),
        (
            "permission-type-outside-the-enum",
            "PATCH",
            format!("/workloads/{{workload}}/shares/{share_id}"),
            String::new(),
            Some(json!({ "PermissionType": "OWNER" })),
        ),
        (
            "delete-token-one-above-the-maximum",
            "DELETE",
            format!("/workloads/{{workload}}/shares/{share_id}"),
            format!("?ClientRequestToken={}", "t".repeat(2049)),
            None,
        ),
        (
            "list-max-results-one-below-the-range",
            "GET",
            "/workloads/{workload}/lensReviews".to_string(),
            "?MaxResults=0".to_string(),
            None,
        ),
        (
            "list-milestone-number-one-above-the-range",
            "GET",
            "/workloads/{workload}/lensReviews".to_string(),
            "?MilestoneNumber=101".to_string(),
            None,
        ),
        (
            "answer-milestone-number-one-below-the-range",
            "GET",
            format!("/workloads/{{workload}}/lensReviews/{FRAMEWORK_LENS}/answers/{ANSWER_QUESTION_ID}"),
            "?MilestoneNumber=0".to_string(),
            None,
        ),
        (
            "lens-aliases-empty",
            "PATCH",
            "/workloads/{workload}/associateLenses".to_string(),
            String::new(),
            Some(json!({ "LensAliases": [] })),
        ),
        (
            "profile-arn-outside-its-pattern",
            "PATCH",
            "/workloads/{workload}/associateProfiles".to_string(),
            String::new(),
            Some(json!({ "ProfileArns": ["not-a-profile-arn"] })),
        ),
        (
            "notes-one-above-the-maximum",
            "PATCH",
            format!("/workloads/{{workload}}/lensReviews/{FRAMEWORK_LENS}/answers/{ANSWER_QUESTION_ID}"),
            String::new(),
            Some(json!({ "Notes": "n".repeat(2085) })),
        ),
        (
            "lens-notes-one-above-the-maximum",
            "PATCH",
            format!("/workloads/{{workload}}/lensReviews/{FRAMEWORK_LENS}"),
            String::new(),
            Some(json!({ "LensNotes": "n".repeat(2085) })),
        ),
        (
            "integrating-service-outside-the-enum",
            "POST",
            "/workloads/{workload}/updateIntegration".to_string(),
            String::new(),
            Some(json!({ "ClientRequestToken": "tok-enum", "IntegratingService": "NOT_JIRA" })),
        ),
        (
            "milestone-name-one-below-the-minimum",
            "PUT",
            format!("/workloads/{{workload}}/lensReviews/{FRAMEWORK_LENS}/upgrade"),
            String::new(),
            Some(json!({ "MilestoneName": "ab" })),
        ),
        (
            "pillar-id-one-below-the-minimum-length",
            "POST",
            "/workloads/{workload}/checkSummaries".to_string(),
            String::new(),
            Some(json!({
                "LensArn": official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS),
                "PillarId": "",
                "QuestionId": CHECK_QUESTION_ID,
                "ChoiceId": CHOICE_ID,
            })),
        ),
        (
            "check-max-results-one-above-the-range",
            "POST",
            "/workloads/{workload}/checks".to_string(),
            String::new(),
            Some(json!({
                "LensArn": official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS),
                "PillarId": PILLAR_ID,
                "QuestionId": CHECK_QUESTION_ID,
                "ChoiceId": CHOICE_ID,
                "MaxResults": 51,
            })),
        ),
    ] {
        // The id of the workload the request names changes nothing about the OTHER member's
        // refusal: it is decided by the boundary walk, which still runs for a shape whose
        // path id it no longer judges.
        for (host, workload_id) in [("a-stored-workload", present.as_str()), ("an-absent-workload", absent)] {
            let uri = format!(
                "{}{query}",
                path.replace("{workload}", &encode_path_segment(workload_id))
            );
            let answer = client.send(method, &uri, body.clone()).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{host}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}/{host}: expected ValidationException, got {answer:?}"
            );
        }
    }
}

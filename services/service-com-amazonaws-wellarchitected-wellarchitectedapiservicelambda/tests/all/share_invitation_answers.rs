// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateShareInvitation`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. Answering the invitation a principal was offered moves the share it mediates out of
//!    `PENDING` into the `ShareStatus` the answer names — `ACCEPT` into `ACCEPTED`,
//!    `REJECT` into `REJECTED` — and answers with that invitation: its own
//!    `ShareInvitationId`, the resource kind `WORKLOAD` and the shared workload's id, and
//!    none of the members belonging to invitations of another resource kind. The share
//!    keeps everything else it was created with, and the answered invitation stops being
//!    outstanding while every other one is untouched.
//! 2. An invitation is answerable only while its share is still waiting on it: a share in
//!    any of the seven other published `ShareStatus` members — already answered, or
//!    revoked or expired by its sharer — is `409 ConflictException`, and the answer that
//!    was refused changes nothing.
//! 3. A well-formed `ShareInvitationId` that names no invitation THIS principal was
//!    offered is `404 ResourceNotFoundException` naming the id exactly as sent: an id
//!    never minted, an id of another id space (a `ShareId`), and the invitation of a
//!    share offered to another principal.
//! 4. A `ShareInvitationAction` outside the published set, and a `ShareInvitationId`
//!    outside the modelled `^[0-9a-f]{32}$`, are `400 ValidationException`, and the share
//!    stays as it was.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`; the shares the invitations stand for are seeded through
//! `super::share_invitations::seed` (see `Client::record_workload_share` for why a share
//! addressed to the caller has no HTTP path in a single-account emulator).

use serde_json::{json, Value};

use super::share_invitations::{
    invitation_ids, invitations, member, seed, Seeded, PERMISSION_TYPES, SHARE_STATUSES,
};
use super::workload_harness::Client;

/// Each published `ShareInvitationAction` member paired with the `ShareStatus` member the
/// answered share moves into: the actions are `ACCEPT | REJECT`
/// (`smithy_query(query="shape", shape="ShareInvitationAction")`) and both target statuses
/// are published members of `ShareStatus` (`smithy_query(query="shape",
/// shape="ShareStatus")`).
const ANSWERS: [(&str, &str); 2] = [("ACCEPT", "ACCEPTED"), ("REJECT", "REJECTED")];

/// The `ShareStatus` a share carries while it is still waiting on its invitation.
const PENDING: &str = "PENDING";

/// The members of `ShareInvitation` that belong to an invitation of another resource kind
/// — `smithy_query(query="shape", shape="ShareInvitation")` publishes these beside
/// `ShareInvitationId`, `ShareResourceType` and `WorkloadId`.
const OTHER_KIND_MEMBERS: [&str; 4] = ["LensAlias", "LensArn", "ProfileArn", "TemplateArn"];

/// The `WorkloadShareSummary` `ListWorkloadShares` reports for one share of one workload.
async fn share_summary(client: &Client, workload_id: &str, share_id: &str, case: &str) -> Value {
    let listed = client.list_shares(workload_id, "").await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected ListWorkloadShares 200, got {listed:?}"
    );
    listed
        .body
        .get("WorkloadShareSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| member(summary, "ShareId") == share_id)
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: the share under test must still be reported, got {listed:?}")
        })
}

/// The `Status` of one share of one workload.
async fn share_status(client: &Client, workload_id: &str, share_id: &str, case: &str) -> String {
    member(&share_summary(client, workload_id, share_id, case).await, "Status").to_string()
}

/// The `ShareInvitationId`s the calling principal still has outstanding.
async fn outstanding(client: &Client, case: &str) -> Vec<String> {
    let listed = client.list_share_invitations("").await;
    invitation_ids(&invitations(&listed, case))
}

/// Answer one invitation.
async fn answer(client: &Client, share_invitation_id: &str, action: &str) -> super::workload_harness::Answer {
    client
        .update_share_invitation(
            share_invitation_id,
            json!({ "ShareInvitationAction": action }),
        )
        .await
}

/// Rule 1: every published answer, crossed with every published permission the share
/// grants, records that answer on the share and returns the invitation it answered.
#[tokio::test]
async fn answering_an_invitation_records_the_answer_on_the_share_it_mediates() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    let mut rows: Vec<(String, Seeded, &str, &str)> = Vec::new();
    for (action, expected_status) in ANSWERS {
        for permission_type in PERMISSION_TYPES {
            let case = format!(
                "{}-{}",
                action.to_lowercase(),
                permission_type.to_lowercase()
            );
            let seeded = seed(&client, &case, caller, permission_type, PENDING).await;
            rows.push((case, seeded, action, expected_status));
        }
    }

    for index in 0..rows.len() {
        let (case, seeded, action, expected_status) = &rows[index];
        assert!(
            outstanding(&client, case).await.contains(&seeded.invitation_id),
            "case {case:?}: the seeded invitation must be outstanding before it is answered"
        );

        let answered = answer(&client, &seeded.invitation_id, action).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected UpdateShareInvitation 200, got {answered:?}"
        );
        let invitation = answered
            .body
            .get("ShareInvitation")
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: UpdateShareInvitation must answer with the ShareInvitation, \
                     got {answered:?}"
                )
            });
        assert_eq!(
            member(&invitation, "ShareInvitationId"),
            seeded.invitation_id,
            "case {case:?}: the answer must name the invitation it answered, got {invitation:?}"
        );
        assert_eq!(
            member(&invitation, "ShareResourceType"),
            "WORKLOAD",
            "case {case:?}: a share of a workload is a WORKLOAD invitation, got {invitation:?}"
        );
        assert_eq!(
            member(&invitation, "WorkloadId"),
            seeded.workload_id,
            "case {case:?}: the invitation must name the shared workload, got {invitation:?}"
        );
        for absent in OTHER_KIND_MEMBERS {
            assert_eq!(
                invitation.get(absent),
                None,
                "case {case:?}: a workload invitation carries no {absent}, got {invitation:?}"
            );
        }

        // Round trip: the share the invitation mediates now carries the answer, and keeps
        // everything else it was created with.
        let summary = share_summary(&client, &seeded.workload_id, &seeded.share_id, case).await;
        assert_eq!(
            member(&summary, "Status"),
            *expected_status,
            "case {case:?}: answering {action} must move the share to {expected_status}, \
             got {summary:?}"
        );
        assert_eq!(
            member(&summary, "PermissionType"),
            seeded.permission_type,
            "case {case:?}: the answer must not change the permission the share grants, \
             got {summary:?}"
        );
        assert_eq!(
            member(&summary, "SharedWith"),
            caller,
            "case {case:?}: the answer must not change who the share was offered to, \
             got {summary:?}"
        );

        // The answered invitation is no longer waiting on an answer; every invitation that
        // has not been answered yet still is.
        let still_outstanding = outstanding(&client, case).await;
        for (other_case, other, _, _) in &rows[..=index] {
            assert!(
                !still_outstanding.contains(&other.invitation_id),
                "case {case:?}: the answered invitation {other_case:?} must stop being \
                 outstanding, got {still_outstanding:?}"
            );
        }
        for (other_case, other, _, _) in &rows[index + 1..] {
            assert!(
                still_outstanding.contains(&other.invitation_id),
                "case {case:?}: answering one invitation must leave {other_case:?} \
                 outstanding, got {still_outstanding:?}"
            );
        }
    }
}

/// Rule 2, the answer is final: an invitation already answered cannot be answered again,
/// whichever answer it was given and whichever answer the second request names.
#[tokio::test]
async fn an_invitation_can_only_be_answered_once() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    for (first, first_status) in ANSWERS {
        for (second, _) in ANSWERS {
            let case = format!("{}-then-{}", first.to_lowercase(), second.to_lowercase());
            let seeded = seed(&client, &case, caller, "CONTRIBUTOR", PENDING).await;

            let answered = answer(&client, &seeded.invitation_id, first).await;
            assert_eq!(
                answered.status, 200,
                "case {case:?}: the first answer must be served, got {answered:?}"
            );

            let again = answer(&client, &seeded.invitation_id, second).await;
            assert_eq!(
                again.status, 409,
                "case {case:?}: an invitation already answered must be a 409, got {again:?}"
            );
            assert_eq!(
                again.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {again:?}"
            );
            assert_eq!(
                member(&again.body, "ResourceId"),
                seeded.invitation_id,
                "case {case:?}: the conflict must name the invitation, got {again:?}"
            );
            assert_eq!(
                share_status(&client, &seeded.workload_id, &seeded.share_id, &case).await,
                first_status,
                "case {case:?}: the refused second answer must change nothing"
            );
        }
    }
}

/// Rule 2, over the statuses: a share in any published `ShareStatus` other than `PENDING`
/// is no longer waiting on its invitation, so neither answer can be given. The `PENDING`
/// row is the control that the same request is served there.
#[tokio::test]
async fn an_invitation_whose_share_left_pending_is_a_conflict() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    for status in SHARE_STATUSES {
        for (action, expected_status) in ANSWERS {
            let case = format!("{}-{}", status.to_lowercase(), action.to_lowercase());
            let seeded = seed(&client, &case, caller, "READONLY", status).await;
            let answered = answer(&client, &seeded.invitation_id, action).await;

            if status == PENDING {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: a share still waiting on its invitation must be \
                     answerable, got {answered:?}"
                );
                assert_eq!(
                    share_status(&client, &seeded.workload_id, &seeded.share_id, &case).await,
                    expected_status,
                    "case {case:?}: the answer must be recorded on the share"
                );
                continue;
            }

            assert_eq!(
                answered.status, 409,
                "case {case:?}: a share in {status} has no invitation left to answer, \
                 got {answered:?}"
            );
            assert_eq!(
                answered.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {answered:?}"
            );
            assert_eq!(
                share_status(&client, &seeded.workload_id, &seeded.share_id, &case).await,
                status,
                "case {case:?}: a refused answer must leave the share's status alone"
            );
        }
    }
}

/// Rule 3: a well-formed `ShareInvitationId` that names no invitation this principal was
/// offered is a 404 naming the id as sent. The rows are ids never minted, an id of another
/// id space (the `ShareId` of the share the invitation mediates), and the invitation of a
/// share offered over HTTP to another principal.
#[tokio::test]
async fn an_invitation_this_principal_was_not_offered_is_not_found() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    // One invitation this caller CAN answer, so every 404 below is a real miss and not an
    // empty store — and it is still outstanding at the end.
    let mine = seed(&client, "not-found-host", caller, "CONTRIBUTOR", PENDING).await;

    // A share of the same workload offered to ANOTHER principal, created over the wire.
    // Its invitation is not this caller's to answer.
    let offered_elsewhere = client
        .create_share(
            &mine.workload_id,
            json!({
                "SharedWith": "111122223333",
                "PermissionType": "READONLY",
                "ClientRequestToken": "tok-offered-elsewhere",
            }),
        )
        .await;
    assert_eq!(
        offered_elsewhere.status, 200,
        "case \"offered-elsewhere\": expected CreateWorkloadShare 200, got {offered_elsewhere:?}"
    );
    let elsewhere_share_id = member(&offered_elsewhere.body, "ShareId").to_string();
    let elsewhere_invitation_id = client
        .store()
        .lock()
        .workload_shares_of(&mine.workload_id)
        .iter()
        .find(|share| share.share_id == elsewhere_share_id)
        .map(|share| share.share_invitation_id.clone())
        .expect("the share created over HTTP raised an invitation");

    let rows: Vec<(&str, String)> = vec![
        ("never-minted-zeros", "0".repeat(32)),
        ("never-minted-f", "f".repeat(32)),
        (
            "never-minted-mixed",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        // The invitation is its own resource: the ShareId of a share is not it.
        ("the-shares-own-id", mine.share_id.clone()),
        // Offered to another principal, so not this caller's invitation.
        ("offered-elsewhere", elsewhere_invitation_id.clone()),
    ];

    for (case, share_invitation_id) in rows {
        assert_ne!(
            share_invitation_id, mine.invitation_id,
            "case {case:?}: the row must not name the invitation this caller holds"
        );
        for (action, _) in ANSWERS {
            let refused = answer(&client, &share_invitation_id, action).await;
            assert_eq!(
                refused.status, 404,
                "case {case:?}/{action}: expected HTTP 404, got {refused:?}"
            );
            assert_eq!(
                refused.error_code, "ResourceNotFoundException",
                "case {case:?}/{action}: expected ResourceNotFoundException, got {refused:?}"
            );
            assert_eq!(
                member(&refused.body, "ResourceId"),
                share_invitation_id,
                "case {case:?}/{action}: the 404 must name the id as sent, got {refused:?}"
            );
            assert_eq!(
                member(&refused.body, "ResourceType"),
                "SHARE_INVITATION",
                "case {case:?}/{action}: expected the ResourceType of an invitation, \
                 got {refused:?}"
            );
        }
    }

    // Nothing above moved a share: this caller's invitation is still outstanding, and the
    // share offered elsewhere still waits on its own.
    assert!(
        outstanding(&client, "after-the-misses")
            .await
            .contains(&mine.invitation_id),
        "case \"after-the-misses\": a refused answer must leave this caller's invitation \
         outstanding"
    );
    assert_eq!(
        share_status(
            &client,
            &mine.workload_id,
            &elsewhere_share_id,
            "offered-elsewhere"
        )
        .await,
        PENDING,
        "case \"offered-elsewhere\": a share this caller cannot answer must stay PENDING"
    );
}

/// Rule 4: a `ShareInvitationAction` outside the two published members is a bad request,
/// not an answer. The rows differ along case, along a value from a neighbouring enum, and
/// along the member being absent altogether.
///
/// A `ShareInvitationId` outside the modelled `@pattern("^[0-9a-f]{32}$")` is refused the
/// same way, at the boundary, before the invitation is resolved: the rows are five shapes
/// the pattern rejects, each tested on both published answers.
#[tokio::test]
async fn an_unpublished_answer_or_a_malformed_invitation_id_is_refused() {
    let client = Client::new();
    let seeded = seed(
        &client,
        "refused-answer",
        engine::emulator_account_id(),
        "CONTRIBUTOR",
        PENDING,
    )
    .await;

    let rows: Vec<(&str, Value)> = vec![
        ("lowercase-accept", json!({"ShareInvitationAction": "accept"})),
        ("mixed-case-reject", json!({"ShareInvitationAction": "Reject"})),
        // A `ShareStatus` member is not a `ShareInvitationAction` member.
        ("a-share-status", json!({"ShareInvitationAction": "ACCEPTED"})),
        ("empty", json!({"ShareInvitationAction": ""})),
        ("unknown", json!({"ShareInvitationAction": "REVOKE"})),
        ("absent", json!({})),
    ];

    for (case, body) in rows {
        let refused = client
            .update_share_invitation(&seeded.invitation_id, body)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {refused:?}"
        );
        assert_eq!(
            share_status(&client, &seeded.workload_id, &seeded.share_id, case).await,
            PENDING,
            "case {case:?}: a refused answer must leave the share waiting on its invitation"
        );
    }

    // An id the modelled pattern rejects is refused at the boundary, whatever the answer.
    for (case, share_invitation_id) in [
        ("one-character-short", "0".repeat(31)),
        ("one-character-long", "0".repeat(33)),
        ("uppercase-hex", "A".repeat(32)),
        ("hex-with-a-non-hex-digit", format!("{}z", "0".repeat(31))),
        ("not-hex-at-all", "not-an-invitation-id".to_string()),
    ] {
        for (action, _) in ANSWERS {
            let refused = answer(&client, &share_invitation_id, action).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}/{action}: expected HTTP 400 for an id outside the modelled \
                 pattern, got {refused:?}"
            );
            assert_eq!(
                refused.error_code, "ValidationException",
                "case {case:?}/{action}: expected ValidationException, got {refused:?}"
            );
        }
    }

    // The invitation is still answerable after every refusal.
    let answered = answer(&client, &seeded.invitation_id, "ACCEPT").await;
    assert_eq!(
        answered.status, 200,
        "case \"answerable-after-refusals\": expected UpdateShareInvitation 200, got {answered:?}"
    );
}

// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for answering the invitation a REVIEW-TEMPLATE share raises:
//! `UpdateShareInvitation` on a `TEMPLATE` invitation, beside the `WORKLOAD` rows of
//! `super::share_invitation_answers` and the `LENS`/`PROFILE` rows of
//! `super::lens_share_invitation_answers`.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. An invitation is named by its `ShareInvitationId` alone and names no resource, so
//!    every invitation `ListShareInvitations` reports can be answered, whatever resource
//!    kind it offers: a `TEMPLATE` invitation answers HTTP 200 with the invitation —
//!    reporting its own `ShareInvitationId`, `ShareResourceType=TEMPLATE` and the shared
//!    template's `TemplateArn`, and none of the members belonging to invitations of
//!    another kind — moves the share it mediates out of `PENDING` into the `ShareStatus`
//!    the answer names, and stops being outstanding while every other invitation is left
//!    alone. The rows cross both published answers with review templates differing along
//!    their name and the lenses they apply.
//! 2. A template invitation is answerable only while its share is still waiting on it: a
//!    share in any of the seven other published `ShareStatus` members — answered already,
//!    or revoked or expired by its sharer — is `409 ConflictException` and the refused
//!    answer changes nothing. The `PENDING` row is the control that the same request is
//!    served there.
//! 3. A well-formed `ShareInvitationId` that names no invitation THIS principal was
//!    offered is `404 ResourceNotFoundException` naming the id as sent: the invitation of
//!    a template share offered over HTTP to another principal, the `ShareId` of a share
//!    this caller does hold, and ids never minted.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the review-template builder are the shared
//! `super::workload_harness`'s, the `CreateTemplateShare` / `ListTemplateShares` spellings
//! and the share seed are `super::template_shares`', and the answer request and its table
//! of published answers are `super::share_invitation_answers`' — one spelling of each for
//! every suite.
//!
//! The share an invitation stands for is seeded through `record_template_share` for the
//! reason its doc gives: an invitation is offered to the principal a template was shared
//! WITH, and a share whose recipient is the caller has no HTTP path in a single-account
//! emulator. The answer under test and the reads of its result — `ListShareInvitations`
//! and `ListTemplateShares` — still cross the HTTP boundary.

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::share_invitation_answers::{answer, ANSWERS};
use super::share_invitations::{invitation_ids, invitations, member, SHARE_STATUSES};
use super::template_shares::{
    create_share, created_share_id, list_shares, record_template_share, summaries,
    NEW_SHARE_STATUS,
};
use super::workload_harness::{review_template, Client};

/// The members of `ShareInvitation` that name the shared resource —
/// `smithy_query(query="shape", shape="ShareInvitation")` publishes exactly these beside
/// `ShareInvitationId` and `ShareResourceType`. A `TEMPLATE` invitation fills `TemplateArn`
/// and none of the others.
const RESOURCE_MEMBERS: [&str; 5] = [
    "WorkloadId",
    "LensAlias",
    "LensArn",
    "ProfileArn",
    "TemplateArn",
];

/// The review templates the rows of rule 1 share: a template holds at most one share per
/// principal, so each row needs its own, and they differ along the template's name and the
/// lenses it applies — both AWS-official lens aliases of the published catalog
/// (`smithy_query(query="shape", shape="LensAlias")` bounds an alias only by length; the
/// catalog this crate's other review-template suites seed with is `wellarchitected`).
const TEMPLATES: [(&str, &[&str]); 2] = [
    ("Tpl-answered-alpha", &["wellarchitected"]),
    (
        "Tpl-answered-beta-with-a-longer-name",
        &["wellarchitected", "serverless"],
    ),
];

/// One seeded `TEMPLATE` invitation to answer.
struct Row {
    case: String,
    template_arn: String,
    share_id: String,
    invitation_id: String,
    action: &'static str,
    expected_status: &'static str,
}

/// The `Status` `ListTemplateShares` reports for one share of one review template.
async fn share_status(client: &Client, template_arn: &str, share_id: &str, case: &str) -> String {
    let listed = list_shares(client, template_arn, "").await;
    summaries(&listed, case)
        .iter()
        .find(|summary| member(summary, "ShareId") == share_id)
        .map(|summary| member(summary, "Status").to_string())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the template share under test must still be reported, got {listed:?}")
        })
}

/// Move the stored share `share_id` of the review template `template_arn` to `status`, as
/// its sharer revoking it or the service expiring it would.
///
/// Every `ShareStatus` other than `PENDING` and the two an answer records is reached by
/// something this emulator does not serve (`DeleteTemplateShare` revokes a share by
/// removing it outright), so the status a refusal is asserted against is seeded here —
/// exactly as `super::lens_share_invitation_answers` seeds a lens share's.
fn set_template_share_status(client: &Client, template_arn: &str, share_id: &str, status: &str) {
    let handle = client.store();
    let mut store = handle.lock();
    let share = store
        .template_shares
        .get_mut(template_arn)
        .and_then(|shares| shares.get_mut(share_id))
        .expect("the review-template share under test must be stored");
    share.status = status.to_string();
}

/// The `ShareInvitationId`s the calling principal still has outstanding.
async fn outstanding(client: &Client, case: &str) -> Vec<String> {
    let listed = client.list_share_invitations("").await;
    invitation_ids(&invitations(&listed, case))
}

/// Rule 1: both published answers, on the `TEMPLATE` invitation of each seeded review
/// template, are recorded on the share and answered with the template invitation.
#[tokio::test]
async fn answering_a_template_invitation_records_the_answer_on_the_share_it_mediates() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    let mut rows: Vec<Row> = Vec::new();
    for (action, expected_status) in ANSWERS {
        for (name, lenses) in TEMPLATES {
            let case = format!("{}-{}", action.to_lowercase(), name.to_lowercase());
            let template_arn = review_template(&client, &format!("{name}-{action}"), lenses).await;
            let (share_id, invitation_id) =
                record_template_share(&client, &template_arn, caller, NEW_SHARE_STATUS);
            rows.push(Row {
                case,
                template_arn,
                share_id,
                invitation_id,
                action,
                expected_status,
            });
        }
    }

    for index in 0..rows.len() {
        let row = &rows[index];
        let case = row.case.as_str();
        let before = outstanding(&client, case).await;
        assert!(
            before.contains(&row.invitation_id),
            "case {case:?}: the seeded template invitation must be outstanding before it is \
             answered, got {before:?}"
        );

        let answered = answer(&client, &row.invitation_id, row.action).await;
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
            row.invitation_id,
            "case {case:?}: the answer must name the invitation it answered, got {invitation:?}"
        );
        assert_eq!(
            member(&invitation, "ShareResourceType"),
            "TEMPLATE",
            "case {case:?}: a share of a review template is a TEMPLATE invitation, \
             got {invitation:?}"
        );
        assert_eq!(
            member(&invitation, "TemplateArn"),
            row.template_arn,
            "case {case:?}: the invitation must name the shared review template, \
             got {invitation:?}"
        );
        for absent in RESOURCE_MEMBERS {
            if absent == "TemplateArn" {
                continue;
            }
            assert_eq!(
                invitation.get(absent),
                None,
                "case {case:?}: a TEMPLATE invitation carries no {absent}, got {invitation:?}"
            );
        }

        // Round trip: the share the invitation mediates now carries the answer, read back
        // through the operation that reports a review template's shares.
        assert_eq!(
            share_status(&client, &row.template_arn, &row.share_id, case).await,
            row.expected_status,
            "case {case:?}: answering {} must move the share to {}",
            row.action,
            row.expected_status
        );

        // The answered invitation is no longer waiting on an answer; every invitation that
        // has not been answered yet still is.
        let still_outstanding = outstanding(&client, case).await;
        for other in &rows[..=index] {
            assert!(
                !still_outstanding.contains(&other.invitation_id),
                "case {case:?}: the answered invitation {:?} must stop being outstanding, \
                 got {still_outstanding:?}",
                other.case
            );
        }
        for other in &rows[index + 1..] {
            assert!(
                still_outstanding.contains(&other.invitation_id),
                "case {case:?}: answering one invitation must leave {:?} outstanding, \
                 got {still_outstanding:?}",
                other.case
            );
        }
    }
}

/// Rule 2: a template share that has left `PENDING` has no invitation left to answer, in
/// every published `ShareStatus` the share can stand in, for either answer.
#[tokio::test]
async fn a_template_invitation_whose_share_left_pending_is_a_conflict() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let template_arn = review_template(&client, "Tpl-conflict", &["wellarchitected"]).await;
    let (share_id, invitation_id) =
        record_template_share(&client, &template_arn, caller, NEW_SHARE_STATUS);

    for status in SHARE_STATUSES {
        for (action, expected_status) in ANSWERS {
            let case = format!("{}-{}", status.to_lowercase(), action.to_lowercase());
            set_template_share_status(&client, &template_arn, &share_id, status);
            let answered = answer(&client, &invitation_id, action).await;

            if status == state::DEFAULT_SHARE_STATUS {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: a template share still waiting on its invitation must be \
                     answerable, got {answered:?}"
                );
                assert_eq!(
                    share_status(&client, &template_arn, &share_id, &case).await,
                    expected_status,
                    "case {case:?}: the answer must be recorded on the template share"
                );
                continue;
            }

            assert_eq!(
                answered.status, 409,
                "case {case:?}: a template share in {status} has no invitation left to answer, \
                 got {answered:?}"
            );
            assert_eq!(
                answered.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {answered:?}"
            );
            assert_eq!(
                member(&answered.body, "ResourceId"),
                invitation_id,
                "case {case:?}: the conflict must name the invitation, got {answered:?}"
            );
            assert_eq!(
                share_status(&client, &template_arn, &share_id, &case).await,
                status,
                "case {case:?}: a refused answer must leave the template share's status alone"
            );
        }
    }
}

/// Rule 3: a template invitation this principal was not offered is a 404 naming the id as
/// sent, and nothing it names moves.
#[tokio::test]
async fn a_template_invitation_this_principal_was_not_offered_is_not_found() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let template_arn = review_template(&client, "Tpl-not-found", &["wellarchitected"]).await;

    // One template invitation this caller CAN answer, so every 404 below is a real miss
    // and not an empty store — and it is still outstanding at the end.
    let (mine_share_id, mine) =
        record_template_share(&client, &template_arn, caller, NEW_SHARE_STATUS);

    // A share of the same template offered to ANOTHER principal, created over the wire:
    // its invitation is not this caller's to answer.
    let elsewhere = create_share(
        &client,
        &template_arn,
        "111122223333",
        "tok-template-offered-elsewhere",
    )
    .await;
    let elsewhere_share_id = created_share_id(&elsewhere, "offered-elsewhere");
    let elsewhere_invitation_id = {
        let handle = client.store();
        let store = handle.lock();
        store
            .template_shares_of(&template_arn)
            .iter()
            .find(|stored| stored.share_id == elsewhere_share_id)
            .map(|stored| stored.share_invitation_id.clone())
            .expect("the template share created over HTTP raised an invitation")
    };

    let rows: Vec<(&str, String)> = vec![
        ("never-minted-zeros", "0".repeat(32)),
        (
            "never-minted-mixed",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        // The invitation is its own resource: the ShareId of a share is not it.
        ("the-shares-own-id", mine_share_id.clone()),
        ("offered-elsewhere", elsewhere_invitation_id),
    ];

    for (case, share_invitation_id) in rows {
        assert_ne!(
            share_invitation_id, mine,
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

    // Nothing above moved a share: this caller's invitation is still outstanding, and
    // every share of the template still waits on its own invitation.
    assert!(
        outstanding(&client, "after-the-misses").await.contains(&mine),
        "case \"after-the-misses\": a refused answer must leave this caller's invitation \
         outstanding"
    );
    for share_id in [mine_share_id, elsewhere_share_id] {
        assert_eq!(
            share_status(&client, &template_arn, &share_id, "after-the-misses").await,
            NEW_SHARE_STATUS,
            "case \"after-the-misses\": a refused answer must leave every template share \
             waiting on its invitation"
        );
    }
}

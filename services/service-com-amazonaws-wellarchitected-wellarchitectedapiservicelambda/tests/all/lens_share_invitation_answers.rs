// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for answering a share invitation of ANY shared resource kind:
//! `UpdateShareInvitation` on the `LENS` invitation a lens share raises and the `PROFILE`
//! invitation a profile share raises, beside the `WORKLOAD` invitation
//! `super::share_invitation_answers` covers on its own.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one recorded request:
//!
//! 1. An invitation is named by its `ShareInvitationId` alone and names no resource, so
//!    the one answer path serves every invitation `ListShareInvitations` reports, whatever
//!    resource kind it offers. The rows cross every kind of share this service holds with
//!    both published answers: each answers HTTP 200 with the invitation, reporting its own
//!    `ShareInvitationId`, the `ShareResourceType` of the resource it offers and exactly
//!    the members of THAT kind — a lens invitation the shared lens's ARN in both `LensArn`
//!    and `LensAlias` ("For custom lenses, this is the lens ARN" —
//!    `smithy_query(query="shape", shape="LensAlias")`), a profile invitation the shared
//!    profile's `ProfileArn`, a workload invitation the shared workload's id — moves the
//!    share it mediates out of `PENDING` into the `ShareStatus` the answer names, and stops
//!    being outstanding while every other invitation is left alone.
//! 2. A lens invitation is answerable only while its share is still waiting on it: a
//!    share in any of the seven other published `ShareStatus` members is
//!    `409 ConflictException` and the refused answer changes nothing. The `PENDING` row is
//!    the control that the same request is served there.
//! 3. A lens `ShareInvitationId` that names no invitation THIS principal was offered is
//!    `404 ResourceNotFoundException` naming the id as sent: the invitation of a lens
//!    share offered over HTTP to another principal, and ids never minted.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client is the shared `super::workload_harness`, the lens publish
//! and share helpers are `super::lens_shares`', the profile ones `super::profile_shares`',
//! and the workload rows are seeded exactly as `super::share_invitations` seeds one.
//!
//! A share whose recipient is the CALLER has no HTTP path in a single-account emulator (see
//! `super::lens_shares::record_lens_share`), so the shares the invitations stand for are
//! seeded through the shared state; the answer under test and the reads of its result —
//! `ListShareInvitations` for every kind, and `ListProfileShares` for a profile share's
//! status — still cross the HTTP boundary. A lens share's status is read back through the
//! store because no operation reports one yet (`ListLensShares` is a stub with its own
//! issue).

use serde_json::Value;

use super::custom_lens_import::lens_document;
use super::lens_shares::{
    create_share, created_share_id, publish_lenses, record_lens_share, stored_shares,
};
use super::profile_shares::record_profile_share;
use super::share_invitation_answers::{answer, ANSWERS};
use super::share_invitations::{invitation_ids, invitations, member, seed};
use super::workload_harness::{created_profile, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The members of `ShareInvitation` that name the shared resource —
/// `smithy_query(query="shape", shape="ShareInvitation")` publishes exactly these beside
/// `ShareInvitationId` and `ShareResourceType`. An invitation fills the ones belonging to
/// the kind of resource it offers and no other.
const RESOURCE_MEMBERS: [&str; 5] = [
    "WorkloadId",
    "LensAlias",
    "LensArn",
    "ProfileArn",
    "TemplateArn",
];

/// The `PermissionType` the workload rows share with, one of the two published members.
const PERMISSION_TYPE: &str = "CONTRIBUTOR";

/// Where the share an invitation mediates is read back from, per resource kind.
#[derive(Debug, Clone)]
enum Share {
    Workload { workload_id: String, share_id: String },
    Lens { lens_arn: String, share_id: String },
    Profile { profile_arn: String, share_id: String },
}

/// One seeded invitation to answer: the kind of resource it offers, the answer it is
/// given, the `ShareStatus` that answer moves its share to, the resource members the
/// answered invitation must report, and where its share is read back from.
struct Row {
    case: String,
    kind: &'static str,
    invitation_id: String,
    action: &'static str,
    expected_status: &'static str,
    expected_members: Vec<(&'static str, String)>,
    share: Share,
}

/// The `ShareStatus` of the share an invitation mediates, read through the operation that
/// reports that kind of share.
async fn share_status(client: &Client, share: &Share, case: &str) -> String {
    match share {
        Share::Workload {
            workload_id,
            share_id,
        } => super::share_invitation_answers::share_status(client, workload_id, share_id, case)
            .await,
        Share::Lens { lens_arn, share_id } => stored_shares(client, lens_arn)
            .into_iter()
            .find(|stored| &stored.share_id == share_id)
            .map(|stored| stored.status)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the lens share under test must still be stored")
            }),
        Share::Profile {
            profile_arn,
            share_id,
        } => {
            let listed = super::profile_shares::list_shares(client, profile_arn, "").await;
            super::profile_shares::summaries(&listed, case)
                .iter()
                .find(|summary| member(summary, "ShareId") == share_id)
                .map(|summary| member(summary, "Status").to_string())
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the profile share under test must still be reported, \
                         got {listed:?}"
                    )
                })
        }
    }
}

/// Move the stored share `share_id` of the lens `alias` to `status`, as its sharer
/// revoking it or the service expiring it would.
///
/// Every `ShareStatus` other than `PENDING` and the two an answer records is reached by
/// something this emulator does not serve yet (`DeleteLensShare` revokes a share but
/// removes it outright), so the status a refusal is asserted against is seeded here,
/// exactly as `super::share_invitations` seeds a workload share at a given status.
fn set_lens_share_status(client: &Client, alias: &str, share_id: &str, status: &str) {
    let handle = client.store();
    let mut store = handle.lock();
    let share = store
        .lens_shares
        .get_mut(state::lens_alias(alias))
        .and_then(|shares| shares.get_mut(share_id))
        .expect("the lens share under test must be stored");
    share.status = status.to_string();
}

/// The `ShareInvitationId`s the calling principal still has outstanding.
async fn outstanding(client: &Client, case: &str) -> Vec<String> {
    let listed = client.list_share_invitations("").await;
    invitation_ids(&invitations(&listed, case))
}

/// Rule 1: both published answers, on an invitation of each kind of resource this service
/// holds shares of, are recorded on the share and answered with the invitation of that
/// kind.
#[tokio::test]
async fn answering_an_invitation_of_any_shared_resource_kind_records_the_answer() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    // One published lens per lens row: a lens holds at most one share per principal, and
    // the invitation under test is the one offered to the caller.
    let documents: Vec<(&'static str, Value)> = vec![
        (
            "lens-accept",
            lens_document("Answered-accept-lens", None, &[("governance", 1, 2)]),
        ),
        (
            "lens-reject",
            lens_document(
                "Answered-reject-lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
        ),
    ];
    let lenses = publish_lenses(&client, &documents).await;

    let mut rows: Vec<Row> = Vec::new();
    for (index, (action, expected_status)) in ANSWERS.iter().enumerate() {
        let lens_arn = lenses[index].1.clone();
        let (lens_share_id, lens_invitation_id) = record_lens_share(&client, &lens_arn, caller);
        rows.push(Row {
            case: format!("lens-{}", action.to_lowercase()),
            kind: "LENS",
            invitation_id: lens_invitation_id,
            action,
            expected_status,
            // A custom lens is named by its ARN in both members a lens invitation carries.
            expected_members: vec![
                ("LensArn", lens_arn.clone()),
                ("LensAlias", lens_arn.clone()),
            ],
            share: Share::Lens {
                lens_arn,
                share_id: lens_share_id,
            },
        });

        let case = format!("workload-{}", action.to_lowercase());
        let seeded = seed(
            &client,
            &case,
            caller,
            PERMISSION_TYPE,
            state::DEFAULT_SHARE_STATUS,
        )
        .await;
        rows.push(Row {
            case,
            kind: "WORKLOAD",
            invitation_id: seeded.invitation_id.clone(),
            action,
            expected_status,
            expected_members: vec![("WorkloadId", seeded.workload_id.clone())],
            share: Share::Workload {
                workload_id: seeded.workload_id,
                share_id: seeded.share_id,
            },
        });

        // The PROFILE row: a share of a profile minted over HTTP, offered to the caller.
        let case = format!("profile-{}", action.to_lowercase());
        let (profile_arn, _) = created_profile(&client, &format!("Answered-{case}")).await;
        let (profile_share_id, profile_invitation_id) =
            record_profile_share(&client, &profile_arn, caller, state::DEFAULT_SHARE_STATUS);
        rows.push(Row {
            case,
            kind: "PROFILE",
            invitation_id: profile_invitation_id,
            action,
            expected_status,
            expected_members: vec![("ProfileArn", profile_arn.clone())],
            share: Share::Profile {
                profile_arn,
                share_id: profile_share_id,
            },
        });
    }

    for index in 0..rows.len() {
        let row = &rows[index];
        let case = row.case.as_str();
        let before = outstanding(&client, case).await;
        assert!(
            before.contains(&row.invitation_id),
            "case {case:?}: the seeded invitation must be outstanding before it is answered, \
             got {before:?}"
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
            row.kind,
            "case {case:?}: expected a {} invitation, got {invitation:?}",
            row.kind
        );
        for (name, expected) in &row.expected_members {
            assert_eq!(
                member(&invitation, name),
                expected.as_str(),
                "case {case:?}: the invitation must name the shared resource in {name}, \
                 got {invitation:?}"
            );
        }
        for absent in RESOURCE_MEMBERS {
            if row
                .expected_members
                .iter()
                .any(|(filled, _)| *filled == absent)
            {
                continue;
            }
            assert_eq!(
                invitation.get(absent),
                None,
                "case {case:?}: a {} invitation carries no {absent}, got {invitation:?}",
                row.kind
            );
        }

        // Round trip: the share the invitation mediates now carries the answer.
        assert_eq!(
            share_status(&client, &row.share, case).await,
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

/// Rule 2: a lens share that has left `PENDING` has no invitation left to answer, in every
/// published `ShareStatus` the share can stand in, for either answer.
#[tokio::test]
async fn a_lens_invitation_whose_share_left_pending_is_a_conflict() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let lens_arn = super::lens_shares::one_published_lens(&client, "conflict", "Conflict-lens").await;
    let (share_id, invitation_id) = record_lens_share(&client, &lens_arn, caller);
    let share = Share::Lens {
        lens_arn: lens_arn.clone(),
        share_id: share_id.clone(),
    };

    for status in super::share_invitations::SHARE_STATUSES {
        for (action, expected_status) in ANSWERS {
            let case = format!("{}-{}", status.to_lowercase(), action.to_lowercase());
            set_lens_share_status(&client, &lens_arn, &share_id, status);
            let answered = answer(&client, &invitation_id, action).await;

            if status == state::DEFAULT_SHARE_STATUS {
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: a lens share still waiting on its invitation must be \
                     answerable, got {answered:?}"
                );
                assert_eq!(
                    share_status(&client, &share, &case).await,
                    expected_status,
                    "case {case:?}: the answer must be recorded on the lens share"
                );
                continue;
            }

            assert_eq!(
                answered.status, 409,
                "case {case:?}: a lens share in {status} has no invitation left to answer, \
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
                share_status(&client, &share, &case).await,
                status,
                "case {case:?}: a refused answer must leave the lens share's status alone"
            );
        }
    }
}

/// Rule 3: a lens invitation this principal was not offered is a 404 naming the id as
/// sent, and nothing it names moves.
#[tokio::test]
async fn a_lens_invitation_this_principal_was_not_offered_is_not_found() {
    let client = Client::new();
    let caller = engine::emulator_account_id();
    let lens_arn =
        super::lens_shares::one_published_lens(&client, "not-found", "Not-found-lens").await;

    // One lens invitation this caller CAN answer, so every 404 below is a real miss and
    // not an empty store.
    let (_, mine) = record_lens_share(&client, &lens_arn, caller);

    // A share of the same lens offered to ANOTHER principal, created over the wire: its
    // invitation is not this caller's to answer.
    let elsewhere = create_share(
        &client,
        &lens_arn,
        "111122223333",
        "tok-lens-offered-elsewhere",
    )
    .await;
    let elsewhere_share_id = created_share_id(&elsewhere, "offered-elsewhere");
    let elsewhere_invitation_id = stored_shares(&client, &lens_arn)
        .into_iter()
        .find(|stored| stored.share_id == elsewhere_share_id)
        .map(|stored| stored.invitation_id)
        .expect("the lens share created over HTTP raised an invitation");

    let rows: Vec<(&str, String)> = vec![
        ("never-minted-zeros", "0".repeat(32)),
        (
            "never-minted-mixed",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
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

    // Nothing above moved a share: this caller's invitation is still outstanding, and the
    // share offered elsewhere still waits on its own.
    assert!(
        outstanding(&client, "after-the-misses").await.contains(&mine),
        "case \"after-the-misses\": a refused answer must leave this caller's invitation \
         outstanding"
    );
    for stored in stored_shares(&client, &lens_arn) {
        assert_eq!(
            stored.status,
            state::DEFAULT_SHARE_STATUS,
            "case \"after-the-misses\": a refused answer must leave every lens share waiting \
             on its invitation, got {stored:?}"
        );
    }
}

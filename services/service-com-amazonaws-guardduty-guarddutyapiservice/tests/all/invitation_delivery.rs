// SPDX-License-Identifier: CC0-1.0

//! Sending an invitation, end to end over the wire: `InviteMembers` →
//! `ListInvitations` / `GetInvitationsCount` → `AcceptInvitation` /
//! `DeclineInvitations` / `DeleteInvitations` → the administrator reads.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together. Nothing here is seeded through the state module: every invitation these
//! tests answer was delivered by a request.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. Sending an invitation writes the MEMBER side of the relationship as well as the
//!    administrator's own member records: an `InviteMembers` that invited at least one
//!    recorded account delivers one PENDING invitation into the inbox the Region's
//!    endpoint serves, carrying the account that sent it, an id the service minted,
//!    the pending relationship status and the instant it was sent — which is what both
//!    inbox reads then report, and what they agree on. A call that invited nobody
//!    delivers nothing.
//! 2. That delivered invitation is exactly what the operations that ANSWER one act on:
//!    the reported `(accountId, invitationId)` pair is accepted under both names the
//!    acceptance is bound under, the administrator reads then report the relationship
//!    it established — with the invitation's own `invitedAt` — and the invitation
//!    leaves the inbox, so a second acceptance is refused and the relationship can be
//!    left again through `DisassociateFromAdministratorAccount`. `DeclineInvitations`
//!    and `DeleteInvitations` take the same invitation out instead, after which the
//!    acceptance is refused and no relationship exists.
//! 3. One invitation is pending per inviting account: a re-send while it is pending
//!    re-sends that same invitation — same id, same instant, the inbox still holding
//!    one — while a send after it has been answered delivers a FRESH invitation under
//!    a different id.
//! 4. The inbox an invitation is delivered to is the one the Region the send was signed
//!    for serves: another Region's endpoint reports its own inbox and not this one.
//! 5. A send the service refuses, and a send naming only accounts the administrator
//!    holds no member record for, deliver nothing at all.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call_in, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::invitation_listing::{enabled_detector_in, page_invitations};
use crate::member_association::{
    administrator, create_members_in, unprocessed_ids, MALFORMED_ACCOUNT_IDS, MEMBERS,
    NON_MEMBER_ACCOUNT_IDS,
};

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE a 400 is enveloped in, as `x-amzn-errortype` carries it.
const BAD_REQUEST: &str = "BadRequestException";

/// `Invitation.RelationshipStatus` while an invitation is pending.
const INVITED: &str = "Invited";

/// `Master.RelationshipStatus` / `Administrator.RelationshipStatus` once the
/// invitation has been accepted.
const ENABLED: &str = "Enabled";

/// The Region a row uses unless it names another.
const REGION: &str = "us-east-1";

/// Regions to send an invitation through, differing along the partition's geography so
/// no row can pass on the default Region alone.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-southeast-2"];

/// The two names GuardDuty binds the SAME acceptance under — "this operation is
/// deprecated, use AcceptAdministratorInvitation instead"
/// (`smithy_query(operation="AcceptInvitation")`) — as
/// `(path segment, administrator member, the read's output member)`.
const ACCEPTANCES: [(&str, &str, &str); 2] = [
    ("master", "masterId", "master"),
    ("administrator", "administratorId", "administrator"),
];

// ---------------------------------------------------------------------------
// Rule 1: a send delivers one pending invitation to the Region's inbox.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sending_an_invitation_delivers_one_pending_invitation_to_the_regions_inbox() {
    // Rows differing along the Region the send is signed for and along HOW MANY of the
    // administrator's recorded accounts one call invites — one, a strict subset, and
    // every member it holds.
    let rows: [(&str, usize); 3] = [(REGIONS[0], 1), (REGIONS[1], 2), (REGIONS[2], MEMBERS.len())];

    for (region, invited_count) in rows {
        let case = format!("{invited_count} account(s) invited through {region}");
        let router = new_router();
        let detector_id = detector_with_members(&router, region, &case).await;

        // Before the send the inbox is empty: the delivery below is what fills it.
        assert!(
            page_invitations(&router, region, "", &case).await.is_empty(),
            "case {case:?}: an inbox must hold nothing before an invitation is sent"
        );
        assert_eq!(
            invitations_count(&router, region, &case).await,
            0,
            "case {case:?}: an inbox holding nothing must count 0"
        );

        let invited: Vec<&str> = MEMBERS[..invited_count].iter().map(|(id, _)| *id).collect();
        let sent_at = send(&router, region, &detector_id, &invited, &case).await;

        let listed = page_invitations(&router, region, "", &case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: a send delivers one pending invitation, got {listed:?}"
        );
        let invitation = listed[0].clone();
        // The invitation was sent BY the administrator the send was signed for, so the
        // account it reports is that account and never one of the invited ids.
        assert_eq!(
            invitation["accountId"].as_str(),
            Some(administrator().as_str()),
            "case {case:?}: expected the inviting account, got {invitation:?}"
        );
        for account_id in &invited {
            assert_ne!(
                invitation["accountId"].as_str(),
                Some(*account_id),
                "case {case:?}: the invitation reports the account that SENT it, got {invitation:?}"
            );
        }
        assert_eq!(
            invitation["relationshipStatus"].as_str(),
            Some(INVITED),
            "case {case:?}: a delivered invitation is pending, got {invitation:?}"
        );
        let invitation_id = invitation_id(&invitation, &case);
        assert!(
            is_minted_id(&invitation_id),
            "case {case:?}: expected a minted 32-hex invitation id, got {invitation_id:?}"
        );
        assert_ne!(
            invitation_id, detector_id,
            "case {case:?}: the invitation carries its own id, not the detector's"
        );
        let invited_at = invitation["invitedAt"].as_str().unwrap_or_default();
        assert!(
            is_instant(invited_at),
            "case {case:?}: expected an ISO-8601 invitedAt, got {invitation:?}"
        );
        assert!(
            invited_at >= sent_at.as_str(),
            "case {case:?}: expected the instant the invitation was sent, got {invited_at:?} \
             for a send at {sent_at:?}"
        );

        // Both inbox reads read the same record, and neither changes it.
        assert_eq!(
            invitations_count(&router, region, &case).await,
            1,
            "case {case:?}: the count must size the inbox the listing reported"
        );
        assert_eq!(
            page_invitations(&router, region, "", &case).await,
            listed,
            "case {case:?}: reading the inbox must not change what a later caller sees"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the delivered invitation is what every answer acts on.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_delivered_invitation_is_what_each_answer_operation_acts_on() {
    for answer in Answer::ALL {
        let case = format!("answering a delivered invitation by {}", answer.label());
        let router = new_router();
        let detector_id = detector_with_members(&router, REGION, &case).await;
        let invited: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();
        send(&router, REGION, &detector_id, &invited, &case).await;

        let invitation = only_invitation(&router, REGION, &case).await;
        let sender = invitation["accountId"].as_str().unwrap_or_default().to_string();
        let invitation_id = invitation_id(&invitation, &case);
        let invited_at = invitation["invitedAt"].clone();

        match answer {
            Answer::Accept(acceptance) => {
                let (status, body) = accept(
                    &router,
                    REGION,
                    acceptance,
                    &detector_id,
                    &sender,
                    &invitation_id,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: accepting the delivered invitation expected 200, got \
                     {status} {body:?}"
                );

                // BOTH administrator reads report the one relationship the acceptance
                // established, carrying the invitation's own instant.
                for read in ACCEPTANCES {
                    let reported = administrator_account(&router, REGION, read, &detector_id).await;
                    assert_eq!(
                        reported["accountId"].as_str(),
                        Some(sender.as_str()),
                        "case {case:?}: expected the administrator account, got {reported:?}"
                    );
                    assert_eq!(
                        reported["invitationId"].as_str(),
                        Some(invitation_id.as_str()),
                        "case {case:?}: expected the accepted invitation id, got {reported:?}"
                    );
                    assert_eq!(
                        reported["relationshipStatus"].as_str(),
                        Some(ENABLED),
                        "case {case:?}: expected the accepted relationship status, got {reported:?}"
                    );
                    assert_eq!(
                        reported["invitedAt"], invited_at,
                        "case {case:?}: expected the invitation's own invitedAt, got {reported:?}"
                    );
                }

                // And the relationship can be left again, which is the operation the
                // dead inbox made unreachable too.
                let (status, body) = disassociate(&router, REGION, &detector_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: leaving the relationship expected 200, got {status} {body:?}"
                );
                let reported = administrator_account(&router, REGION, acceptance, &detector_id).await;
                assert_eq!(
                    reported["accountId"],
                    Value::Null,
                    "case {case:?}: expected no administrator after leaving, got {reported:?}"
                );
            }
            Answer::Decline | Answer::Delete => {
                let (status, body) = answer_by_account(&router, REGION, answer, &[&sender]).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {body:?}"
                );
                assert!(
                    unprocessed_ids(&body).is_empty(),
                    "case {case:?}: the delivered invitation must be answerable, got {body:?}"
                );

                // Declining or deleting answers the invitation WITHOUT establishing
                // anything, so the acceptance is refused afterwards.
                let (status, body) = accept(
                    &router,
                    REGION,
                    ACCEPTANCES[0],
                    &detector_id,
                    &sender,
                    &invitation_id,
                )
                .await;
                assert_refusal(&case, status, &body);
                let reported =
                    administrator_account(&router, REGION, ACCEPTANCES[0], &detector_id).await;
                assert_eq!(
                    reported["accountId"],
                    Value::Null,
                    "case {case:?}: an answered invitation establishes nothing, got {reported:?}"
                );
            }
        }

        // However it was answered, the invitation is out of the inbox and cannot be
        // answered twice.
        assert!(
            page_invitations(&router, REGION, "", &case).await.is_empty(),
            "case {case:?}: an answered invitation must leave the inbox"
        );
        assert_eq!(
            invitations_count(&router, REGION, &case).await,
            0,
            "case {case:?}: an answered invitation must leave the count at 0"
        );
        let (status, body) = accept(
            &router,
            REGION,
            ACCEPTANCES[1],
            &detector_id,
            &sender,
            &invitation_id,
        )
        .await;
        assert_refusal(&case, status, &body);
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one invitation pending per inviting account.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resend_keeps_the_pending_invitation_and_a_send_after_an_answer_delivers_a_fresh_one() {
    // Rows differing along what happened to the first invitation: nothing at all, and
    // each of the three ways it can be answered.
    let rows: [(Option<Answer>, &str); 4] = [
        (None, "still pending"),
        (Some(Answer::Accept(ACCEPTANCES[1])), "accepted"),
        (Some(Answer::Decline), "declined"),
        (Some(Answer::Delete), "deleted"),
    ];

    for (answered, label) in rows {
        let case = format!("a second send after the first invitation was {label}");
        let router = new_router();
        let detector_id = detector_with_members(&router, REGION, &case).await;
        let first_batch = [MEMBERS[0].0];
        send(&router, REGION, &detector_id, &first_batch, &case).await;
        let first = only_invitation(&router, REGION, &case).await;
        let first_id = invitation_id(&first, &case);
        let sender = first["accountId"].as_str().unwrap_or_default().to_string();

        if let Some(answer) = answered {
            match answer {
                Answer::Accept(acceptance) => {
                    let (status, body) = accept(
                        &router,
                        REGION,
                        acceptance,
                        &detector_id,
                        &sender,
                        &first_id,
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: accepting expected 200, got {status} {body:?}"
                    );
                }
                Answer::Decline | Answer::Delete => {
                    let (status, body) =
                        answer_by_account(&router, REGION, answer, &[&sender]).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: answering expected 200, got {status} {body:?}"
                    );
                }
            }
        }

        // A second send, naming a DIFFERENT subset of the administrator's members, so
        // the row cannot pass by repeating the first request.
        let second_batch = [MEMBERS[0].0, MEMBERS[MEMBERS.len() - 1].0];
        send(&router, REGION, &detector_id, &second_batch, &case).await;
        let second = only_invitation(&router, REGION, &case).await;
        let second_id = invitation_id(&second, &case);

        if answered.is_none() {
            // The invitation was still pending, so the re-send re-sent THAT invitation:
            // nothing about it moved.
            assert_eq!(
                second, first,
                "case {case:?}: a re-send must leave the pending invitation exactly as it was"
            );
        } else {
            // The first invitation was answered, so it is no longer pending and the
            // second send delivers one of its own.
            assert_ne!(
                second_id, first_id,
                "case {case:?}: a send after an answer must deliver a fresh invitation id"
            );
            assert_eq!(
                second["relationshipStatus"].as_str(),
                Some(INVITED),
                "case {case:?}: the fresh invitation must be pending, got {second:?}"
            );
            assert!(
                is_minted_id(&second_id),
                "case {case:?}: expected a minted 32-hex invitation id, got {second_id:?}"
            );
        }
        assert_eq!(
            invitations_count(&router, REGION, &case).await,
            1,
            "case {case:?}: the inbox holds one invitation per inviting account"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the inbox is the Region's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invitation_is_delivered_only_to_the_inbox_of_the_region_it_was_sent_through() {
    let router = new_router();
    let mut detectors: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let case = format!("preparing {region}");
        let detector_id = detector_with_members(&router, region, &case).await;
        detectors.push((region, detector_id));
    }

    let mut delivered: Vec<(&str, String)> = Vec::new();
    for (index, (region, detector_id)) in detectors.iter().enumerate() {
        let case = format!("after sending through {region}");
        send(&router, region, detector_id, &[MEMBERS[0].0], &case).await;
        delivered.push((
            region,
            invitation_id(&only_invitation(&router, region, &case).await, &case),
        ));

        // Only the endpoints a send has gone through hold an invitation; the rest of
        // the partition is untouched.
        for (other, _) in &detectors[index + 1..] {
            assert_eq!(
                invitations_count(&router, other, &case).await,
                0,
                "case {case:?}: {other} must report its own empty inbox"
            );
            assert!(
                page_invitations(&router, other, "", &case).await.is_empty(),
                "case {case:?}: {other} must report its own empty inbox"
            );
        }
    }

    // Each Region's inbox holds exactly the invitation delivered through it, and no two
    // of them are the same invitation.
    for (region, id) in &delivered {
        let case = format!("the inbox of {region}");
        let invitation = only_invitation(&router, region, &case).await;
        assert_eq!(
            invitation_id(&invitation, &case),
            *id,
            "case {case:?}: expected the invitation delivered through it, got {invitation:?}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> =
        delivered.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        delivered.len(),
        "every delivered invitation carries an id of its own, got {delivered:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a send that invited nobody, and a refused send, deliver nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_send_that_invited_nobody_and_a_refused_send_deliver_nothing() {
    let router = new_router();
    let case = "a send that delivers nothing";
    let detector_id = detector_with_members(&router, REGION, case).await;

    // Accounts the administrator holds no member record for: reported, not invited, so
    // nothing was sent and nothing is delivered.
    for (account_id, why) in NON_MEMBER_ACCOUNT_IDS {
        let case = format!("no member record: {account_id} ({why})");
        let (status, body) = invite(&router, REGION, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            vec![account_id.to_string()],
            "case {case:?}: expected that account reported unprocessed, got {body:?}"
        );
        assert_empty_inbox(&router, REGION, &case).await;
    }

    // A value that cannot be an account id at all refuses the whole call, so the
    // well-formed id beside it is not invited and nothing is delivered either.
    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        let case = format!("malformed account id: {account_id} ({why})");
        let (status, body) = invite(&router, REGION, &detector_id, &[MEMBERS[0].0, account_id]).await;
        assert_refusal(&case, status, &body);
        assert_empty_inbox(&router, REGION, &case).await;
    }

    // A well-formed detector id this account/Region does not own identifies no inbox to
    // deliver to: one nothing minted, and this Region's own detector addressed through
    // another Region's endpoint.
    for (region, candidate, why) in [
        (REGION, ABSENT_DETECTOR_ID, "a well-formed id nothing minted"),
        (
            REGIONS[1],
            detector_id.as_str(),
            "another Region's endpoint",
        ),
    ] {
        let case = format!("detector {candidate} through {region} ({why})");
        let (status, body) = invite(&router, region, candidate, &[MEMBERS[0].0]).await;
        assert_refusal(&case, status, &body);
        assert_empty_inbox(&router, REGION, &case).await;
        assert_empty_inbox(&router, region, &case).await;
    }
}

// ---------------------------------------------------------------------------
// The calls and the shared helpers.
// ---------------------------------------------------------------------------

/// How a delivered invitation can be answered: accepted under either of the two names
/// the acceptance is bound under, declined, or deleted.
#[derive(Debug, Clone, Copy)]
enum Answer {
    Accept((&'static str, &'static str, &'static str)),
    Decline,
    Delete,
}

impl Answer {
    const ALL: [Self; 4] = [
        Self::Accept(ACCEPTANCES[0]),
        Self::Accept(ACCEPTANCES[1]),
        Self::Decline,
        Self::Delete,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Accept((segment, _, _)) => segment,
            Self::Decline => "declining",
            Self::Delete => "deleting",
        }
    }
}

/// Create an enabled detector through `region`'s endpoint and record [`MEMBERS`]
/// against it, returning the detector id.
async fn detector_with_members(router: &Router, region: &str, case: &str) -> String {
    let detector_id = enabled_detector_in(router, region).await;
    let (status, body) = create_members_in(router, region, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case:?}: expected every account recorded, got {body:?}"
    );
    detector_id
}

/// `InviteMembers` through `region`'s endpoint — `POST
/// /detector/{DetectorId}/member/invite` (`smithy_query(operation="InviteMembers")`).
async fn invite(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/invite"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// A successful `InviteMembers` that invited every account it named, returning the
/// instant the call was made as text — a lower bound on the `invitedAt` the delivered
/// invitation must carry.
async fn send(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
    case: &str,
) -> String {
    let before = now_text();
    let (status, body) = invite(router, region, detector_id, account_ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: invite-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case:?}: expected every recorded account invited, got {body:?}"
    );
    before
}

/// The one invitation the inbox `region`'s endpoint serves holds, failing the test if
/// it holds any other number.
async fn only_invitation(router: &Router, region: &str, case: &str) -> Value {
    let listed = page_invitations(router, region, "", case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly one pending invitation, got {listed:?}"
    );
    listed[0].clone()
}

/// The `invitationId` of one reported invitation.
fn invitation_id(invitation: &Value, case: &str) -> String {
    invitation["invitationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: an invitation must report its id, got {invitation:?}")
        })
        .to_string()
}

/// One `GetInvitationsCount` call — `GET /invitation/count`
/// (`smithy_query(operation="GetInvitationsCount")`).
async fn invitations_count(router: &Router, region: &str, case: &str) -> i64 {
    let (status, body) = call_in(router, region, "GET", "/invitation/count", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-invitations-count expected 200, got {status} {body:?}"
    );
    body.get("invitationsCount")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an invitationsCount number, got {body:?}")
        })
}

/// Both inbox reads report an empty inbox through `region`'s endpoint.
async fn assert_empty_inbox(router: &Router, region: &str, case: &str) {
    assert!(
        page_invitations(router, region, "", case).await.is_empty(),
        "case {case:?}: expected {region} to hold no invitation"
    );
    assert_eq!(
        invitations_count(router, region, case).await,
        0,
        "case {case:?}: expected {region} to count no invitation"
    );
}

/// One acceptance: `POST /detector/{DetectorId}/{segment}` carrying the administrator
/// member that name of the operation uses.
async fn accept(
    router: &Router,
    region: &str,
    acceptance: (&str, &str, &str),
    detector_id: &str,
    administrator_id: &str,
    invitation_id: &str,
) -> (u16, Value) {
    let (segment, member, _) = acceptance;
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/{segment}"),
        Some(json!({member: administrator_id, "invitationId": invitation_id})),
    )
    .await
}

/// `DeclineInvitations` (`POST /invitation/decline`) or `DeleteInvitations`
/// (`POST /invitation/delete`): both name the accounts that SENT the invitations.
async fn answer_by_account(
    router: &Router,
    region: &str,
    answer: Answer,
    account_ids: &[&str],
) -> (u16, Value) {
    let path = match answer {
        Answer::Decline => "/invitation/decline",
        Answer::Delete => "/invitation/delete",
        Answer::Accept(_) => panic!("an acceptance names a detector: use accept()"),
    };
    call_in(
        router,
        region,
        "POST",
        path,
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// The administrator relationship one of the two reads reports —
/// `GET /detector/{DetectorId}/{segment}`.
async fn administrator_account(
    router: &Router,
    region: &str,
    read: (&str, &str, &str),
    detector_id: &str,
) -> Value {
    let (segment, _, output_member) = read;
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/{segment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "reading /{segment}: expected 200, got {status} {body:?}"
    );
    body[output_member].clone()
}

/// `DisassociateFromAdministratorAccount` — `POST
/// /detector/{DetectorId}/administrator/disassociate`.
async fn disassociate(router: &Router, region: &str, detector_id: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/administrator/disassociate"),
        None,
    )
    .await
}

/// Assert a response is this service's 400 client rejection, in all three places the
/// class and the envelope appear.
fn assert_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        BAD_REQUEST,
        "case {case:?}: expected the {BAD_REQUEST} envelope, got {body:?}"
    );
}

/// Is `value` an ISO-8601 millisecond instant, the form every timestamp this service
/// reports as text takes?
fn is_instant(value: &str) -> bool {
    value.len() == 24 && value.contains('T') && value.ends_with('Z')
}

/// Is `value` an id this service MINTED — 32 lowercase hexadecimal characters, the
/// shape it mints a `DetectorId` and a finding id in?
fn is_minted_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// The current instant in the text form the reads report, computed here rather than
/// read from the emulator so an assertion is against the instant and not the renderer.
fn now_text() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_millis() as i64;
    service_com_amazonaws_guardduty_guarddutyapiservice::state::iso8601(millis)
}

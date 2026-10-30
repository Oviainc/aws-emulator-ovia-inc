// SPDX-License-Identifier: CC0-1.0

//! The two halves one send writes must agree about WHICH invitation is pending.
//!
//! Sending an invitation writes the administrator's own member records
//! (`GetMembers` / `ListMembers`) and delivers the invitation to the inbox the
//! Region's endpoint serves (`ListInvitations`), and an invitation stays pending only
//! until the account it was delivered to answers it — so an acceptance, a decline and
//! a delete each end the pendency of the send they take out of that inbox, for the
//! member records of that send as much as for the inbox itself.
//!
//! The rule under test, asserted over a SET of inputs rather than one call: a member
//! the administrator reports as `Invited` names the instant at which the invitation
//! this store holds pending from that administrator was sent. So a send while that
//! invitation is pending re-sends it and moves NEITHER half, and a send after it has
//! been accepted, declined or deleted delivers a fresh invitation AND stamps the
//! member records it names with the instant of that send.
//!
//! Every request goes over the wire through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization and the
//! handlers are exercised together; nothing here is seeded through the state module.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, new_router, Router};
use crate::invitation_listing::{enabled_detector_in, page_invitations};
use crate::member_association::{
    create_members_in, get_members_in, member_of, unprocessed_ids, MEMBERS,
};

/// `Member.RelationshipStatus` / `Invitation.RelationshipStatus` while an invitation
/// is pending.
const INVITED: &str = "Invited";

/// How a delivered invitation is answered before the next send: accepted under either
/// of the two names the acceptance is bound under — `(path segment, administrator
/// member)`, "this operation is deprecated, use AcceptAdministratorInvitation
/// instead" (`smithy_query(operation="AcceptInvitation")`) — declined, or deleted.
#[derive(Debug, Clone, Copy)]
enum Answer {
    Accept((&'static str, &'static str)),
    Decline,
    Delete,
}

impl Answer {
    fn label(self) -> &'static str {
        match self {
            Self::Accept((segment, _)) => segment,
            Self::Decline => "declined",
            Self::Delete => "deleted",
        }
    }
}

/// One row of the table: what happens to the first invitation, the Region the whole
/// row is signed for, and WHICH of the administrator's recorded accounts each of the
/// two sends names (as indices into [`MEMBERS`]).
#[derive(Debug, Clone, Copy)]
struct Row {
    answer: Option<Answer>,
    region: &'static str,
    batch: &'static [usize],
}

/// The rows, differing along the answer the first invitation gets (none and each of
/// the four ways one can be answered), along the Region the send is signed for, and
/// along how many of the administrator's members — and which — one call invites.
const ROWS: [Row; 5] = [
    Row {
        answer: None,
        region: "us-east-1",
        batch: &[0],
    },
    Row {
        answer: Some(Answer::Decline),
        region: "eu-west-1",
        batch: &[0, 1, 2],
    },
    Row {
        answer: Some(Answer::Delete),
        region: "ap-southeast-2",
        batch: &[2],
    },
    Row {
        answer: Some(Answer::Accept(("administrator", "administratorId"))),
        region: "us-east-1",
        batch: &[0, 2],
    },
    Row {
        answer: Some(Answer::Accept(("master", "masterId"))),
        region: "eu-west-1",
        batch: &[1],
    },
];

#[tokio::test]
async fn a_send_after_an_answer_writes_both_halves_and_a_resend_writes_neither() {
    for row in ROWS {
        let label = row.answer.map_or("still pending", Answer::label);
        let batch = account_ids(row.batch);
        let case = format!(
            "{} account(s) invited through {}, first invitation {label}",
            batch.len(),
            row.region
        );
        let router = new_router();
        let detector_id = detector_with_members(&router, row.region, &case).await;

        // The first send: both halves carry the instant it was made at.
        send(&router, row.region, &detector_id, &batch, &case).await;
        let first = only_invitation(&router, row.region, &case).await;
        let first_id = invitation_id(&first, &case);
        let first_sent_at = invited_at(&first, &case);
        let sender = account_id_of(&first, &case);
        let members_once = invited_members(&router, row.region, &detector_id, &batch, &case).await;
        for (account_id, member) in &members_once {
            assert_member_names(&case, account_id, member, &first_sent_at);
        }

        if let Some(answer) = row.answer {
            let (status, body) = answer_invitation(
                &router,
                row.region,
                answer,
                &detector_id,
                &sender,
                &first_id,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: answering ({label}) expected 200, got {status} {body:?}"
            );
            assert!(
                page_invitations(&router, row.region, "", &case)
                    .await
                    .is_empty(),
                "case {case:?}: an answered invitation must leave the inbox"
            );
        }

        // The second send, naming the same accounts: whether it writes anything is
        // the question both halves must answer the same way.
        send(&router, row.region, &detector_id, &batch, &case).await;
        let second = only_invitation(&router, row.region, &case).await;
        let second_sent_at = invited_at(&second, &case);
        let members_twice = invited_members(&router, row.region, &detector_id, &batch, &case).await;

        if row.answer.is_none() {
            // Still pending, so the re-send re-sent that same invitation: neither
            // half moved.
            assert_eq!(
                second, first,
                "case {case:?}: a re-send must leave the pending invitation as it was, got {second:?}"
            );
            assert_eq!(
                members_twice, members_once,
                "case {case:?}: a re-send must leave the member records as they were, got {members_twice:?} after {members_once:?}"
            );
        } else {
            // The first invitation was answered, so nothing of that send is pending:
            // the second send is a fresh invitation on the inbox half AND on the
            // member half.
            assert_ne!(
                invitation_id(&second, &case),
                first_id,
                "case {case:?}: a send after an answer must deliver a fresh invitation, got {second:?}"
            );
            assert!(
                second_sent_at > first_sent_at,
                "case {case:?}: expected the fresh invitation to be sent after {first_sent_at:?}, got {second_sent_at:?}"
            );
            for (account_id, member) in &members_twice {
                assert!(
                    member["invitedAt"].as_str().is_some_and(|at| at > first_sent_at.as_str()),
                    "case {case:?} {account_id}: expected an invitedAt after the answered invitation's {first_sent_at:?}, got {member:?}"
                );
            }
        }

        // The rule, in both branches: an `Invited` member names the instant at which
        // the invitation the inbox holds was sent.
        for (account_id, member) in &members_twice {
            assert_member_names(&case, account_id, member, &second_sent_at);
        }

        // Only the accounts the sends named were invited at all.
        let untouched: Vec<&str> = MEMBERS
            .iter()
            .map(|(account_id, _)| *account_id)
            .filter(|account_id| !batch.contains(account_id))
            .collect();
        if !untouched.is_empty() {
            let (status, read) =
                get_members_in(&router, row.region, &detector_id, &untouched).await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-members expected 200, got {status} {read:?}"
            );
            for account_id in untouched {
                let member = member_of(&read, account_id).unwrap_or_else(|| {
                    panic!("case {case:?} {account_id}: expected the record to be retained, got {read:?}")
                });
                assert!(
                    member.get("invitedAt").is_none(),
                    "case {case:?} {account_id}: expected no invitedAt for an account no send named, got {member:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_answer_in_turn_leaves_the_member_naming_the_invitation_that_is_pending() {
    // One emulator answering invitation after invitation, so the rule is asserted
    // over a SEQUENCE of answers and not only over a single first invitation. The
    // acceptance comes last: it establishes a relationship the answers before it
    // refuse.
    let region = "us-west-2";
    let answers = [
        Answer::Decline,
        Answer::Delete,
        Answer::Accept(("administrator", "administratorId")),
    ];
    let batch = account_ids(&[0, 1]);
    let case = format!("answering in turn through {region}");
    let router = new_router();
    let detector_id = detector_with_members(&router, region, &case).await;

    let mut previous_sent_at = String::new();
    for answer in answers {
        let case = format!("{case}: {}", answer.label());
        send(&router, region, &detector_id, &batch, &case).await;
        let invitation = only_invitation(&router, region, &case).await;
        let sent_at = invited_at(&invitation, &case);
        assert!(
            sent_at > previous_sent_at,
            "case {case:?}: expected a fresh invitation sent after {previous_sent_at:?}, got {sent_at:?}"
        );
        for (account_id, member) in invited_members(&router, region, &detector_id, &batch, &case)
            .await
            .iter()
        {
            assert_member_names(&case, account_id, member, &sent_at);
        }

        let (status, body) = answer_invitation(
            &router,
            region,
            answer,
            &detector_id,
            &account_id_of(&invitation, &case),
            &invitation_id(&invitation, &case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: answering expected 200, got {status} {body:?}"
        );
        previous_sent_at = sent_at;
    }
}

// ---------------------------------------------------------------------------
// The calls and the assertions they share.
// ---------------------------------------------------------------------------

/// The ids [`MEMBERS`] lists at `indices`, which is what a row's batch names.
fn account_ids(indices: &[usize]) -> Vec<&'static str> {
    indices.iter().map(|index| MEMBERS[*index].0).collect()
}

/// An enabled detector through `region`'s endpoint with [`MEMBERS`] recorded against
/// it, returning the detector id.
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
/// /detector/{DetectorId}/member/invite` — asserting it invited every account it
/// named.
async fn send(router: &Router, region: &str, detector_id: &str, account_ids: &[&str], case: &str) {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/invite"),
        Some(json!({"accountIds": account_ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: invite-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case:?}: expected every recorded account invited, got {body:?}"
    );
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
    text(invitation, "invitationId", case)
}

/// The `accountId` of one reported invitation — the account that SENT it, which is
/// what an acceptance, a decline and a delete each name.
fn account_id_of(invitation: &Value, case: &str) -> String {
    text(invitation, "accountId", case)
}

/// The `invitedAt` of one reported invitation: the instant it was sent, as text.
fn invited_at(invitation: &Value, case: &str) -> String {
    text(invitation, "invitedAt", case)
}

fn text(value: &Value, member: &str, case: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {value:?}"))
        .to_string()
}

/// `GetMembers` through `region`'s endpoint for each account of `batch`, as
/// `(accountId, the reported member)`.
async fn invited_members(
    router: &Router,
    region: &str,
    detector_id: &str,
    batch: &[&'static str],
    case: &str,
) -> Vec<(&'static str, Value)> {
    let (status, read) = get_members_in(router, region, detector_id, batch).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-members expected 200, got {status} {read:?}"
    );
    batch
        .iter()
        .map(|account_id| {
            let member = member_of(&read, account_id).unwrap_or_else(|| {
                panic!("case {case:?} {account_id}: expected a member record, got {read:?}")
            });
            (*account_id, member)
        })
        .collect()
}

/// The rule: a member reported as `Invited` names `sent_at` — the instant at which
/// the invitation the inbox holds pending from its administrator was sent — and the
/// relationship moved at that same instant.
fn assert_member_names(case: &str, account_id: &str, member: &Value, sent_at: &str) {
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some(INVITED),
        "case {case:?} {account_id}: expected relationshipStatus {INVITED}, got {member:?}"
    );
    assert_eq!(
        member["invitedAt"].as_str(),
        Some(sent_at),
        "case {case:?} {account_id}: expected the invitedAt of the pending invitation, got {member:?}"
    );
    assert_eq!(
        member["updatedAt"].as_str(),
        Some(sent_at),
        "case {case:?} {account_id}: expected the relationship to have moved when the invitation was sent, got {member:?}"
    );
}

/// Answer one delivered invitation: an acceptance names the DETECTOR and the
/// `(accountId, invitationId)` pair the inbox reported (`POST
/// /detector/{DetectorId}/{segment}`), while `DeclineInvitations` (`POST
/// /invitation/decline`) and `DeleteInvitations` (`POST /invitation/delete`) name the
/// accounts that SENT the invitations.
async fn answer_invitation(
    router: &Router,
    region: &str,
    answer: Answer,
    detector_id: &str,
    sender: &str,
    invitation_id: &str,
) -> (u16, Value) {
    match answer {
        Answer::Accept((segment, member)) => {
            call_in(
                router,
                region,
                "POST",
                &format!("/detector/{detector_id}/{segment}"),
                Some(json!({member: sender, "invitationId": invitation_id})),
            )
            .await
        }
        Answer::Decline | Answer::Delete => {
            let path = if matches!(answer, Answer::Decline) {
                "/invitation/decline"
            } else {
                "/invitation/delete"
            };
            call_in(
                router,
                region,
                "POST",
                path,
                Some(json!({"accountIds": [sender]})),
            )
            .await
        }
    }
}

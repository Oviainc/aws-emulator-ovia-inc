// SPDX-License-Identifier: CC0-1.0

//! The two monitoring calls — `StopMonitoringMembers` and `StartMonitoringMembers` —
//! over every relationship an invitation's SEND can leave a member in, and over the
//! relationship GuardDuty really does monitor, in the same batch.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test: GuardDuty monitors a member account under its administrator
//! only once that ACCOUNT has answered the invitation — "the invited accounts can
//! either accept or decline the invitation from their GuardDuty accounts"
//! (`smithy_query(query="doc",shape="InviteMembers")`) — so a monitoring call acts on
//! a MONITORED member and on no other, whatever else the member record may already
//! be associated with the administrator for; every account it does not act on is
//! reported in `UnprocessedAccounts` with the reason it was not processed, and
//! nothing stored for it moves.
//!
//! The send's outcome is where this suite differs from `member_monitoring`,
//! `member_monitoring_restart` and `member_monitoring_outstanding_invitation`, whose
//! tables span the relationships a member reaches from an address that can receive
//! mail (`Created`, `Invited`, `Removed`): an invitation announced to an address in a
//! domain that can never receive mail leaves the member `EmailVerificationFailed`,
//! which is an ASSOCIATED relationship — the send delivered the invitation — and not
//! a monitored one, because that account has still not answered. Neither monitoring
//! call may move it, and because neither does, no sequence of them does either.
//!
//! The cases are the cross product of what the rule generalises over: the address the
//! invitation was announced to, over the whole catalogue the `member_invitation_\
//! email_verification` suite owns (both outcomes of a send, spanning the reserved
//! special-use namespaces, their depth and their case), and the sequence of monitoring
//! calls sent to it, over the table `member_monitoring_outstanding_invitation` owns
//! (each call alone, each order of the pair, and the pair repeated). The other side of
//! the rule — the member GuardDuty does monitor, named in the very same batch — is the
//! last test, so neither guard can be satisfied by a call that acts on nothing at all.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    RELATIONSHIP_DISABLED, RELATIONSHIP_ENABLED,
};

use crate::detector_lifecycle::{enabled_detector, new_router, router_over, Router};
// The member-suite helpers, projections and messages live in one place, so no two
// suites drift on what a member read or a reported account looks like.
use crate::member_association::{
    create_members, get_members, invite_members, member_of, unprocessed_ids, unprocessed_result,
    MEMBERS, NOT_AN_ASSOCIATED_MEMBER,
};
// The catalogue of addresses and the relationship each send ends in, and the monitoring
// harness and its sequences: both owned by the suites that introduced them.
use crate::member_invitation_email_verification::{Row, ROWS};
use crate::member_monitoring_outstanding_invitation::{
    member_call, relationship_status, SEQUENCES, START, STOP,
};
use crate::member_relationship_scope::monitor_member;

/// Every account id the catalogue names, in the order [`ROWS`] lists them.
fn all_ids() -> Vec<&'static str> {
    ROWS.iter().map(|row| row.account_id).collect()
}

/// Every address the catalogue names, as `CreateMembers` carries them.
fn all_accounts() -> Vec<(&'static str, &'static str)> {
    ROWS.iter().map(|row| (row.account_id, row.email)).collect()
}

/// One row's name in an assertion: the address and why it is a row.
fn case_of(row: &Row, context: &str) -> String {
    format!("{context} {} ({})", row.email, row.why)
}

/// `GetMembers` over `account_ids`, asserting the modelled success so a read that
/// started refusing cannot be compared as a body.
async fn read_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
    case: &str,
) -> Value {
    let (status, body) = get_members(router, detector_id, account_ids).await;
    assert_eq!(
        status, 200,
        "case {case}: get-members expected 200, got {status} {body:?}"
    );
    body
}

/// Create an enabled detector, record every [`ROWS`] account against it and announce
/// an invitation to each, returning the detector id.
async fn detector_with_invited_addresses(router: &Router, case: &str) -> String {
    let detector_id = enabled_detector(router).await;
    let (status, body) = create_members(router, &detector_id, &all_accounts()).await;
    assert_eq!(
        status, 200,
        "case {case}: create-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case}: create-members expected every account processed, got {body:?}"
    );
    let ids = all_ids();
    let (status, body) = invite_members(router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "case {case}: invite-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case}: invite-members expected every account processed whatever its address \
         resolves to, got {body:?}"
    );
    detector_id
}

// ---------------------------------------------------------------------------
// The rule: no sequence of monitoring calls moves a member whose own answer to
// the invitation is still outstanding, whatever the send left it in.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_monitoring_sequence_moves_a_member_that_has_not_answered_its_invitation() {
    for sequence in SEQUENCES {
        let case = format!("{sequence:?}");
        // One store per sequence: a sequence must not see what another one did.
        let router = new_router();
        let detector_id = detector_with_invited_addresses(&router, &case).await;
        let ids = all_ids();

        // The premise: each record really is in the relationship its address's send
        // ends in, with the instant the invitation was sent recorded for it.
        let before = read_members(&router, &detector_id, &ids, &case).await;
        for row in &ROWS {
            let cell = case_of(row, &case);
            assert_eq!(
                relationship_status(&before, row.account_id, &cell),
                row.expected,
                "case {cell}: expected {}, got {before:?}",
                row.expected
            );
            let member = member_of(&before, row.account_id)
                .unwrap_or_else(|| panic!("case {cell}: expected a member record, got {before:?}"));
            assert!(
                !member["invitedAt"].is_null(),
                "case {cell}: expected the send's instant recorded, got {member:?}"
            );
        }

        // Every call of the sequence succeeds and names each account it cannot act
        // on, with the reason it was not processed.
        for (position, step) in sequence.iter().enumerate() {
            let (status, body) = member_call(&router, &detector_id, step, &ids).await;
            assert_eq!(
                status, 200,
                "case {case}: member/{step} at {position} expected 200, got {status} {body:?}"
            );
            assert_eq!(
                unprocessed_ids(&body),
                ids.iter()
                    .map(|account_id| (*account_id).to_string())
                    .collect::<Vec<String>>(),
                "case {case}: member/{step} at {position} must report every account it cannot \
                 act on, once in request order, got {body:?}"
            );
            for row in &ROWS {
                assert_eq!(
                    unprocessed_result(&body, row.account_id).as_deref(),
                    Some(NOT_AN_ASSOCIATED_MEMBER),
                    "case {}: member/{step} at {position} expected the \
                     not-an-associated-member reason, got {body:?}",
                    case_of(row, &case)
                );
            }
        }

        // Nothing the sequence named moved: not the relationship — so no composition
        // of the two calls reaches the monitored one — not the invitation's instant,
        // and not the relationship's own `updatedAt`.
        let after = read_members(&router, &detector_id, &ids, &case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the sequence to leave every member exactly as it was, got \
             {after:?} after {before:?}"
        );
        for row in &ROWS {
            let cell = case_of(row, &case);
            assert_eq!(
                relationship_status(&after, row.account_id, &cell),
                row.expected,
                "case {cell}: expected the member to keep the relationship its send ended in, \
                 got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The other side of the rule: in the SAME batch, the member GuardDuty monitors
// is the one — and the only one — the calls move.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_batch_moves_the_monitored_member_and_reports_every_unanswered_one() {
    // The monitored relationship is driven through this service's state module,
    // because GuardDuty reaches it when the member ACCOUNT answers the invitation and
    // that is not a call this administrator can send for it; every call under test is
    // still sent over the wire.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    let (monitored, monitored_email) = MEMBERS[0];

    let mut accounts = all_accounts();
    accounts.push((monitored, monitored_email));
    let (status, body) = create_members(&router, &detector_id, &accounts).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    let unanswered = all_ids();
    let (status, body) = invite_members(&router, &detector_id, &unanswered).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    monitor_member(&engine, &detector_id, monitored);

    // The batch names the monitored account among the unanswered ones, so the
    // per-account outcome is the handler's and not the request's shape: the first
    // half of the catalogue, then the monitored account, then the rest.
    let (first, rest) = unanswered.split_at(unanswered.len() / 2);
    let mut requested: Vec<&str> = first.to_vec();
    requested.push(monitored);
    requested.extend_from_slice(rest);

    // Stopping and restarting are inverses over the one stored relationship, so the
    // monitored member walks `Enabled` -> `Disabled` -> `Enabled` while every account
    // still owing an answer is reported, unmoved, by both calls.
    for (step, moved_to) in [(STOP, RELATIONSHIP_DISABLED), (START, RELATIONSHIP_ENABLED)] {
        let before = read_members(&router, &detector_id, &unanswered, step).await;
        let (status, body) = member_call(&router, &detector_id, step, &requested).await;
        assert_eq!(
            status, 200,
            "case {step}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            unanswered
                .iter()
                .map(|account_id| (*account_id).to_string())
                .collect::<Vec<String>>(),
            "case {step}: expected every account owing an answer reported, in request order, and \
             the monitored one processed, got {body:?}"
        );
        for row in &ROWS {
            assert_eq!(
                unprocessed_result(&body, row.account_id).as_deref(),
                Some(NOT_AN_ASSOCIATED_MEMBER),
                "case {}: expected the not-an-associated-member reason, got {body:?}",
                case_of(row, step)
            );
        }

        // The monitored member is where the call moved it, and the accounts it
        // reported are exactly where they were.
        let monitored_read = read_members(&router, &detector_id, &[monitored], step).await;
        assert_eq!(
            relationship_status(&monitored_read, monitored, step),
            moved_to,
            "case {step}: expected the monitored member to move to {moved_to}, got \
             {monitored_read:?}"
        );
        let after = read_members(&router, &detector_id, &unanswered, step).await;
        assert_eq!(
            after, before,
            "case {step}: expected every reported account unchanged, got {after:?} after \
             {before:?}"
        );
    }
}

// SPDX-License-Identifier: CC0-1.0

//! What `InviteMembers` leaves a member's `RelationshipStatus` in, end to end over
//! the wire and read back through `GetMembers` and `ListMembers`.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, asserted over a TABLE of addresses rather than over one
//! recorded call:
//!
//! > An invitation is announced to the address `CreateMembers` recorded for the
//! > account, and GuardDuty resolves that address's domain as it sends: an address
//! > whose domain can receive mail leaves the member `Invited`, awaiting the account's
//! > answer, while one in a domain that can never receive mail leaves it
//! > `EmailVerificationFailed` — the outcome of the SEND, so it is what the very next
//! > read reports. Either way the invitation itself was delivered, so the member is
//! > associated with its administrator and a re-send changes nothing at all.
//!
//! The rows differ along what the rule generalises over: the NAMESPACE the address's
//! domain is in (each reserved one, and ordinary domains), its DEPTH (the reserved
//! name itself and a sub-domain of it), its CASE, and the near-misses that merely
//! LOOK reserved.

use serde_json::Value;

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::member_association::{
    create_members, disassociate_members, get_members, invite_members, member_of, unprocessed_ids,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    RELATIONSHIP_CREATED, RELATIONSHIP_EMAIL_VERIFICATION_FAILED, RELATIONSHIP_INVITED,
    RELATIONSHIP_REMOVED,
};

/// One address under test: the account it belongs to, the address itself, why it is a
/// row, and the relationship the send leaves the member in.
struct Row {
    account_id: &'static str,
    email: &'static str,
    why: &'static str,
    expected: &'static str,
}

/// The addresses whose domain can NEVER receive mail: the reserved special-use
/// top-level domains `.invalid`, `.test`, `.example` and `.localhost`, and the
/// reserved second-level names `example.com`, `example.net` and `example.org` —
/// the names set aside for documentation, examples and testing, which have no mail
/// exchanger (RFC 2606 §2–3, RFC 6761 §6, the source of
/// `state::email_domain_receives_mail`'s catalogue). A sub-domain of a reserved name
/// is in the same namespace, and DNS names do not distinguish case.
///
/// Beside them, addresses in ordinary domains — including the ones that merely LOOK
/// reserved, where the reserved word is part of a longer label or of another
/// namespace, which is the boundary of the catalogue.
const ROWS: [Row; 14] = [
    Row {
        account_id: "100000000001",
        email: "member@example.com",
        why: "the reserved documentation name",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000002",
        email: "member@example.net",
        why: "the second reserved documentation name",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000003",
        email: "member@example.org",
        why: "the third reserved documentation name",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000004",
        email: "member@mail.example.com",
        why: "a sub-domain of a reserved name",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000005",
        email: "Member@EXAMPLE.COM",
        why: "a reserved name in upper case",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000006",
        email: "member@some-host.invalid",
        why: "the reserved invalid top-level domain",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000007",
        email: "member@gd.test",
        why: "the reserved test top-level domain",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000008",
        email: "member@my.example",
        why: "the reserved example top-level domain",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000009",
        email: "member@box.localhost",
        why: "the reserved localhost top-level domain",
        expected: RELATIONSHIP_EMAIL_VERIFICATION_FAILED,
    },
    Row {
        account_id: "100000000010",
        email: "member@example-corp.com",
        why: "an ordinary domain",
        expected: RELATIONSHIP_INVITED,
    },
    Row {
        account_id: "100000000011",
        email: "first.last@sub.example-corp.co.uk",
        why: "an ordinary domain under a two-label suffix",
        expected: RELATIONSHIP_INVITED,
    },
    Row {
        account_id: "100000000012",
        email: "m+tag@example.io",
        why: "a reserved label under another top-level domain",
        expected: RELATIONSHIP_INVITED,
    },
    Row {
        account_id: "100000000013",
        email: "ops@not-invalid.com",
        why: "a reserved word inside a longer label",
        expected: RELATIONSHIP_INVITED,
    },
    Row {
        account_id: "100000000014",
        email: "ops@testing.example-corp.com",
        why: "a reserved word as a label of an ordinary domain",
        expected: RELATIONSHIP_INVITED,
    },
];

/// `ListMembers` over every member of the detector, associated or not, so the two
/// readers can be compared on one member.
async fn list_members(router: &Router, detector_id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/member?onlyAssociated=false"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-members: expected 200, got {status} {body:?}"
    );
    body
}

/// The member `account_id` as `GetMembers` reports it.
async fn read_member(router: &Router, detector_id: &str, account_id: &str, case: &str) -> Value {
    let (status, body) = get_members(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-members expected 200, got {status} {body:?}"
    );
    member_of(&body, account_id)
        .unwrap_or_else(|| panic!("case {case:?}: expected a member, got {body:?}"))
}

/// The `relationshipStatus` of a reported member.
fn status_of(member: &Value) -> &str {
    member["relationshipStatus"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn an_invitation_records_the_outcome_of_announcing_it_to_the_recorded_address() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    for row in &ROWS {
        let case = format!("{} ({})", row.email, row.why);

        // Recorded and not invited yet: every row starts from the same relationship,
        // so what the send leaves behind is the only thing that varies.
        let (status, body) =
            create_members(&router, &detector_id, &[(row.account_id, row.email)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-members expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case:?}: create-members must process the account, got {body:?}"
        );
        let recorded = read_member(&router, &detector_id, row.account_id, &case).await;
        assert_eq!(
            status_of(&recorded),
            RELATIONSHIP_CREATED,
            "case {case:?}: a recorded account must start {RELATIONSHIP_CREATED}, got {recorded:?}"
        );
        assert!(
            recorded["invitedAt"].is_null(),
            "case {case:?}: an un-invited member has no invitedAt, got {recorded:?}"
        );

        // The send: the account is processed whatever the address resolves to, since
        // `UnprocessedAccounts` reports accounts that were not processed and this one
        // was.
        let (status, body) = invite_members(&router, &detector_id, &[row.account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: invite-members expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case:?}: the invited account must be processed, got {body:?}"
        );

        // The outcome is what the very next read reports — no span, no polling.
        let invited = read_member(&router, &detector_id, row.account_id, &case).await;
        assert_eq!(
            status_of(&invited),
            row.expected,
            "case {case:?}: expected {:?}, got {invited:?}",
            row.expected
        );
        assert!(
            !invited["invitedAt"].is_null(),
            "case {case:?}: the send records when the invitation was sent, got {invited:?}"
        );
        assert_eq!(
            invited["email"].as_str(),
            Some(row.email),
            "case {case:?}: the recorded address must be retained, got {invited:?}"
        );

        // The two readers agree: a projection of one member, not two.
        let listed = list_members(&router, &detector_id).await;
        let same = member_of(&listed, row.account_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected the member listed, got {listed:?}"));
        assert_eq!(
            same, invited,
            "case {case:?}: ListMembers and GetMembers must report one member, got {same:?} and \
             {invited:?}"
        );

        // A re-send changes nothing: one invitation is pending for the pair, so the
        // outcome already recorded stands — `InvitedAt` and `UpdatedAt` included.
        let (status, body) = invite_members(&router, &detector_id, &[row.account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-send expected 200, got {status} {body:?}"
        );
        let again = read_member(&router, &detector_id, row.account_id, &case).await;
        assert_eq!(
            again, invited,
            "case {case:?}: a re-send must change nothing, expected {invited:?}, got {again:?}"
        );
    }
}

/// Whatever the send's outcome, the invitation WAS delivered, so the member is
/// associated with its administrator: `DisassociateMembers` ends the association and
/// reports no unprocessed account — the same answer it gives a member whose address
/// could be verified, and not the "not associated" report a merely recorded account
/// gets.
#[tokio::test]
async fn a_member_whose_address_failed_verification_is_still_associated() {
    for row in &ROWS {
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let case = format!("{} ({})", row.email, row.why);

        let (status, body) =
            create_members(&router, &detector_id, &[(row.account_id, row.email)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-members expected 200, got {status} {body:?}"
        );
        let (status, body) = invite_members(&router, &detector_id, &[row.account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: invite-members expected 200, got {status} {body:?}"
        );
        let invited = read_member(&router, &detector_id, row.account_id, &case).await;
        assert_eq!(
            status_of(&invited),
            row.expected,
            "case {case:?}: expected {:?} before the disassociation, got {invited:?}",
            row.expected
        );

        let (status, body) = disassociate_members(&router, &detector_id, &[row.account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: disassociate-members expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case:?}: an associated member must be processed, got {body:?}"
        );
        let removed = read_member(&router, &detector_id, row.account_id, &case).await;
        assert_eq!(
            status_of(&removed),
            RELATIONSHIP_REMOVED,
            "case {case:?}: expected {RELATIONSHIP_REMOVED} after the disassociation, got \
             {removed:?}"
        );
    }
}

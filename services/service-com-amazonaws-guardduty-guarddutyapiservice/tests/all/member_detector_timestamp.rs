// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a detector's own `UpdatedAt` records when a member
//! account is recorded against it or dropped from it.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test, asserted over a SET of member calls rather than one of them:
//! a call that changes WHICH records a detector holds changes the detector record
//! itself, so `GetDetector`'s `UpdatedAt` moves to the instant of that call — while a
//! call that leaves that set alone moves it nowhere, whether it moved one record's own
//! contents (a relationship move, a re-stated detail), recorded nothing (an account
//! reported in `unprocessedAccounts`, a refusal) or changed nothing at all (a read).
//! The answer does not depend on WHICH collection of the detector the records live in,
//! so this suite is the member family of the class `detector_sub_resource_creates`,
//! `detector_sub_resource_deletes`, `filter_detector_timestamp` and
//! `investigation_detector_timestamp` assert over the detector's other collections.
//!
//! The rows differ along what the rule generalises over: how many accounts one call
//! records and drops, the form of the email each record carries, the relationship the
//! records are in when they are dropped
//! (`smithy_query(operation="InviteMembers,DisassociateMembers")` are the calls that
//! move it), and which rule the refused create breaks.
//!
//! `UpdatedAt` is an ISO-8601 string with a fixed-width millisecond field
//! (`smithy_query(operation="GetDetector")` models it as a String), so these
//! assertions order the instants as text, which is all a client has.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
// The member-call harness, the account tables and the reported reasons live once, in
// the member-association suite: reusing them is what keeps the member suites from
// drifting apart on what a member read looks like.
use crate::member_association::{
    create_members, disassociate_members, get_members, invite_members, member_of, unprocessed_ids,
    MALFORMED_ACCOUNT_IDS, MEMBERS, NON_MEMBER_ACCOUNT_IDS,
};
// The detector's own `UpdatedAt` is read through the one helper the clock suites
// already have.
use crate::refused_create_clock::detector_updated_at;

/// One row: the accounts to record in one call, the create that is refused instead of
/// recording them, and the relationship the records are moved into before the delete.
struct Row {
    case: &'static str,
    /// The `(accountId, email)` pairs ONE `CreateMembers` records.
    accounts: Vec<(&'static str, &'static str)>,
    /// A `CreateMembers` entry this service refuses, so nothing is recorded and the
    /// detector is unchanged by it.
    refused: (&'static str, &'static str),
    /// The member-batch calls that move the recorded records' relationship before the
    /// delete, in order: each changes the records, not WHICH records the detector
    /// holds.
    relationship: Vec<&'static str>,
}

/// The rows. `MEMBERS` spans the account ids and the email forms a record carries;
/// the relationship rows span the states a member reaches through the calls this
/// administrator can send (`Created` with no call at all, `Invited` after a send, and
/// `Removed` after a disassociation, which is the state a disassociation RETAINS the
/// record in).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one member, left where the create put it",
            accounts: MEMBERS[..1].to_vec(),
            // A value that cannot be an account id at all.
            refused: (MALFORMED_ACCOUNT_IDS[0].0, "refused@example.com"),
            relationship: Vec::new(),
        },
        Row {
            case: "two members, invited before the delete",
            accounts: MEMBERS[..2].to_vec(),
            // An email with no `@` at all breaks the rules `AccountDetail.Email`
            // documents.
            refused: (NON_MEMBER_ACCOUNT_IDS[3].0, "not-an-email"),
            relationship: vec!["invite"],
        },
        Row {
            case: "three members, invited and then disassociated before the delete",
            accounts: MEMBERS.to_vec(),
            // A grouped account id: 12 characters, none of them refused at the
            // modelled boundary.
            refused: (MALFORMED_ACCOUNT_IDS[4].0, "grouped@example.com"),
            relationship: vec!["invite", "disassociate"],
        },
    ]
}

/// `GetDetector`, so an assertion can see the detector's own timestamps.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// `DeleteMembers` over the wire.
async fn delete_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/delete"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// The account ids of a row, in the order the row lists them.
fn ids(row: &Row) -> Vec<&'static str> {
    row.accounts.iter().map(|(id, _)| *id).collect()
}

// ---------------------------------------------------------------------------
// The rule: recording a member against a detector, and dropping it again, each move
// the detector's UpdatedAt; a call that leaves the set of records alone does not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recording_or_dropping_a_member_moves_the_detectors_updated_at() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass on another row's
        // write.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let accounts = ids(&row);

        let before = read_detector(&router, &detector_id, case).await;
        let created_at = before["createdAt"].clone();
        let updated_before = before["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {before:?}"))
            .to_string();

        // A refused create records nothing, so the detector is unchanged by it.
        let (status, refusal) = create_members(&router, &detector_id, &[row.refused]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refused create must answer 400, got {status} {refusal:?}"
        );
        let after_refusal = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_refusal, updated_before,
            "case {case:?}: a refused create must not move the detector's updatedAt, expected \
             {updated_before:?}, got {after_refusal:?}"
        );

        // A read is not a change either.
        let after_read = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_read, updated_before,
            "case {case:?}: reading the detector must not move its updatedAt, expected \
             {updated_before:?}, got {after_read:?}"
        );

        // The accepted create: the detector now holds records it did not hold before,
        // so its UpdatedAt moves.
        let (status, recorded) = create_members(&router, &detector_id, &row.accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {recorded:?}"
        );
        assert!(
            unprocessed_ids(&recorded).is_empty(),
            "case {case:?}: the create must record every account it names, got {recorded:?}"
        );

        let after_create = read_detector(&router, &detector_id, case).await;
        let updated_after_create = after_create["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {after_create:?}"))
            .to_string();
        assert!(
            updated_after_create > updated_before,
            "case {case:?}: the create changed the detector, so its updatedAt must move past \
             {updated_before:?}, got {updated_after_create:?}"
        );
        assert_eq!(
            after_create["createdAt"], created_at,
            "case {case:?}: recording a member must not move the detector's createdAt, expected \
             {created_at:?}, got {:?}",
            after_create["createdAt"]
        );

        // Round trip: each account the create recorded is a member a read resolves with
        // the email it was given, so the moved UpdatedAt records a change that really
        // landed in the store.
        let (status, read_back) = get_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected get-members to answer 200, got {status} {read_back:?}"
        );
        for (account_id, email) in &row.accounts {
            let member = member_of(&read_back, account_id).unwrap_or_else(|| {
                panic!("case {case:?}: expected a member record for {account_id}, got {read_back:?}")
            });
            assert_eq!(
                member["email"].as_str(),
                Some(*email),
                "case {case:?}: expected {account_id} to read back with email {email:?}, got \
                 {member:?}"
            );
        }

        // A call that records nothing leaves the set of records alone, so it moves
        // nothing: an account the administrator holds no record for is REPORTED rather
        // than dropped.
        let absent = NON_MEMBER_ACCOUNT_IDS[0].0;
        let (status, reported) = delete_members(&router, &detector_id, &[absent]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete of an account with no record to answer 200, got \
             {status} {reported:?}"
        );
        assert_eq!(
            unprocessed_ids(&reported),
            vec![absent.to_string()],
            "case {case:?}: the delete must report the account it could not process, got \
             {reported:?}"
        );
        let after_reported = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_reported, updated_after_create,
            "case {case:?}: a delete that dropped no record must not move the detector's \
             updatedAt, expected {updated_after_create:?}, got {after_reported:?}"
        );

        // Moving the records' relationship changes the records, not WHICH records the
        // detector holds, so the detector's own UpdatedAt stays where the create put it.
        for segment in &row.relationship {
            let (status, moved) = match *segment {
                "invite" => invite_members(&router, &detector_id, &accounts).await,
                "disassociate" => disassociate_members(&router, &detector_id, &accounts).await,
                other => panic!("case {case:?}: no relationship call named {other:?}"),
            };
            assert_eq!(
                status, 200,
                "case {case:?}: expected the {segment:?} call to answer 200, got {status} \
                 {moved:?}"
            );
        }
        // Re-stating the details of records the detector already holds records no new
        // record either, whatever it moves about those records.
        let (status, restated) = create_members(&router, &detector_id, &row.accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-stating create to answer 200, got {status} \
             {restated:?}"
        );
        let after_moves = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_moves, updated_after_create,
            "case {case:?}: the detector holds the same records after a relationship move and a \
             re-stated create, so its updatedAt must stay at {updated_after_create:?}, got \
             {after_moves:?}"
        );

        // The delete is the other half of the same lifecycle: the detector holds fewer
        // records, so its UpdatedAt moves again.
        let (status, dropped) = delete_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {dropped:?}"
        );
        assert!(
            unprocessed_ids(&dropped).is_empty(),
            "case {case:?}: the delete must drop every record it names, got {dropped:?}"
        );
        let after_delete = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            after_delete > updated_after_create,
            "case {case:?}: the delete changed the detector too, so its updatedAt must move past \
             {updated_after_create:?}, got {after_delete:?}"
        );

        // Round trip the other way: the records are gone, which is the change the moved
        // instant records.
        let (status, emptied) = get_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected get-members to answer 200, got {status} {emptied:?}"
        );
        for account_id in &accounts {
            assert!(
                member_of(&emptied, account_id).is_none(),
                "case {case:?}: expected no member record for the dropped {account_id}, got \
                 {emptied:?}"
            );
        }

        // A second delete of the same ids drops nothing, so it moves nothing.
        let (status, again) = delete_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated delete to answer 200, got {status} {again:?}"
        );
        assert_eq!(
            unprocessed_ids(&again),
            accounts
                .iter()
                .map(|account_id| (*account_id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: the repeated delete must report every id it could not process, got \
             {again:?}"
        );
        let after_second_delete = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_second_delete, after_delete,
            "case {case:?}: a delete that dropped no record must not move the detector's \
             updatedAt, expected {after_delete:?}, got {after_second_delete:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every change to the set moves the instant again, so the detector's UpdatedAt names
// the LAST such change and not the first — including the call that drops one record
// while reporting another.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_member_recorded_or_dropped_moves_the_instant_again() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut previous = detector_updated_at(&router, &detector_id, "the fresh detector").await;

    // One account per call, so each call is its own change to the set.
    for (account_id, email) in MEMBERS {
        let case = account_id;
        let (status, recorded) = create_members(&router, &detector_id, &[(account_id, email)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status} {recorded:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each member recorded against the detector must move its updatedAt \
             past {previous:?}, got {now:?}"
        );
        previous = now;
    }

    // The deletes move it on in the same way, one per change — and a call naming one
    // held record beside an account the administrator holds none for still changed the
    // set, so it moves it too.
    let absent = NON_MEMBER_ACCOUNT_IDS[1].0;
    for (account_id, _) in MEMBERS {
        let case = account_id;
        let (status, dropped) = delete_members(&router, &detector_id, &[absent, account_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {dropped:?}"
        );
        assert_eq!(
            unprocessed_ids(&dropped),
            vec![absent.to_string()],
            "case {case:?}: the delete must report only the account it could not process, got \
             {dropped:?}"
        );
        let now = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            now > previous,
            "case {case:?}: each member dropped from the detector must move its updatedAt past \
             {previous:?}, got {now:?}"
        );
        previous = now;
    }
}

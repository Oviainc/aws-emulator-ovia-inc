// SPDX-License-Identifier: CC0-1.0

//! What an ACCEPTED `InviteMembers` that sends no invitation leaves behind, end to end
//! over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, the one the `refused_mutation_clock`, `refused_create_clock`,
//! `no_op_mutation_clock`, `no_op_batch_clock` and `member_batch_no_op_clock` suites
//! assert for the refused calls and the no-op writes of the other families: a mutating
//! operation takes an instant off the service's monotonic "this resource changed now"
//! clock only once a write has really landed, so a call that leaves both sides of the
//! relationship exactly as it found them consumes no instant — and the
//! `CreatedAt`/`UpdatedAt` a LATER accepted call stamps therefore still reports the
//! wall-clock instant of that change rather than one pushed into the future by the
//! calls before it.
//!
//! Sending an invitation writes TWO sides — the administrator's member records and the
//! pending-invitation inbox the Region's endpoint serves — so "did this call write"
//! is answerable only after both have been walked, which is what makes this operation
//! worth its own table. The rows differ along what the rule generalises over: why the
//! call sends nothing (every account named is one the administrator holds no member
//! record for, every account named already has an invitation pending, or the two
//! mixed), how many accounts one request names (one, two, the modelled maximum of
//! `AccountIds`), whether a named id is repeated, and the relationship the named
//! members are left in beforehand.
//!
//! Observable per request, three ways: what each call ANSWERED (the accounts it could
//! not process, once per distinct id in request order), what the store still holds
//! afterwards (every member record, the inbox's invitations with the ids and
//! `InvitedAt` they were handed, and the detector's own `UpdatedAt`), and the instant
//! the next genuinely-sending call stamps on the member it moves — which must be the
//! wall-clock instant of that send rather than one the burst spent ahead of it.

use serde_json::Value;

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
use crate::invitation_count::invitations_count;
use crate::invitation_listing::page_invitations;
// The member harness the member suites share: the accounts to record, well-formed ids
// no `CreateMembers` records, the calls, and the projections of an answer.
use crate::member_association::{
    create_members, get_members, invite_members, member_of, unprocessed_ids, unprocessed_result,
    DEFAULT_REGION, MEMBERS, NON_MEMBER_ACCOUNT_IDS, NOT_AN_ASSOCIATED_MEMBER,
};
use crate::refused_create_clock::{as_millis, detector_updated_at, READING_SLACK_MILLIS};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

/// `Member.RelationshipStatus` for an account whose invitation is pending.
const INVITED: &str = "Invited";

/// The recorded account every row keeps un-invited, so the call AFTER the burst has a
/// member to move: `(accountId, email)`, 12 decimal digits and the email
/// `CreateMembers` records (`smithy_query(shape="AccountId")`). Its domain can
/// receive mail, so the send that moves it leaves it [`INVITED`].
const MOVER: (&str, &str) = ("555566667777", "mover@example-corp.com");

/// How many copies of the call one burst sends. A call that sends no invitation
/// consumes no instant, so the burst may be any size; it is this large because a
/// handler that reads the clock BEFORE it knows whether it writes spends at least one
/// millisecond of it per call, and the burst must advance that clock by more than
/// [`READING_SLACK_MILLIS`] past the wall-clock time the burst itself takes for the
/// difference to be an assertion.
const NO_SEND_CALLS_PER_BURST: usize = 300;

/// The largest number of accounts one `InviteMembers` may name.
///
/// Source: the `length` trait of the list itself — `AccountIds` is `len 1..50`
/// (`smithy_query(shape="AccountIds")`) — which the generated validation enforces at
/// the boundary, so no handler or state constant carries it and this is the only place
/// it is written down on the test side.
const MAX_ACCOUNT_IDS: usize = 50;

/// One row: the invitations to send before the measurement, the accepted call the
/// burst repeats, what that call must report, and how many invitations the Region's
/// inbox holds while it runs.
struct Row {
    case: &'static str,
    /// Accounts invited ONCE before the measurement starts, so the burst's call finds
    /// their invitation already pending.
    seed: Vec<String>,
    /// The `AccountIds` the burst's call names.
    named: Vec<String>,
    /// The accounts the answer must report, in request order — `UnprocessedAccounts`
    /// reports the accounts the call could not process, once per DISTINCT id because
    /// `AccountIds` names a SET.
    reported: Vec<String>,
    /// The invitations the inbox holds throughout the burst.
    pending: i64,
}

/// Every account [`MEMBERS`] records.
fn member_ids() -> Vec<String> {
    MEMBERS.iter().map(|(id, _)| (*id).to_string()).collect()
}

/// Well-formed account ids no `CreateMembers` in this suite records, enough of them to
/// fill one request to [`MAX_ACCOUNT_IDS`]: 12 decimal digits each, taken from a
/// prefix no row above records.
fn unrecorded_fill(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("7000{index:08}"))
        .collect()
}

fn rows() -> Vec<Row> {
    let members = member_ids();
    let absent: Vec<String> = NON_MEMBER_ACCOUNT_IDS
        .iter()
        .map(|(id, _)| (*id).to_string())
        .collect();
    // Bound once: the accounts the full-request row names are exactly the accounts it
    // must report, none of them a member.
    let filled = unrecorded_fill(MAX_ACCOUNT_IDS);
    vec![
        Row {
            case: "one account the administrator holds no member record for",
            seed: Vec::new(),
            named: vec![absent[0].clone()],
            reported: vec![absent[0].clone()],
            pending: 0,
        },
        Row {
            case: "two such accounts, at both ends of the account-id space",
            seed: Vec::new(),
            named: vec![absent[0].clone(), absent[1].clone()],
            reported: vec![absent[0].clone(), absent[1].clone()],
            pending: 0,
        },
        Row {
            case: "the same such account named twice in one call",
            seed: Vec::new(),
            named: vec![absent[3].clone(), absent[3].clone()],
            reported: vec![absent[3].clone()],
            pending: 0,
        },
        Row {
            case: "the modelled maximum of account ids, none of them a member",
            seed: Vec::new(),
            named: filled.clone(),
            reported: filled,
            pending: 0,
        },
        Row {
            case: "a member whose invitation is still pending",
            seed: vec![members[0].clone()],
            named: vec![members[0].clone()],
            reported: Vec::new(),
            pending: 1,
        },
        Row {
            case: "every recorded member, each with an invitation still pending, one repeated",
            seed: members.clone(),
            named: vec![
                members[0].clone(),
                members[1].clone(),
                members[2].clone(),
                members[1].clone(),
            ],
            reported: Vec::new(),
            pending: 1,
        },
        Row {
            case: "a member whose invitation is pending, beside accounts with no member record",
            seed: vec![members[1].clone()],
            named: vec![
                members[1].clone(),
                absent[2].clone(),
                absent[0].clone(),
                absent[2].clone(),
            ],
            reported: vec![absent[2].clone(), absent[0].clone()],
            pending: 1,
        },
    ]
}

/// `InviteMembers` over the wire, asserting the modelled success and answering the body.
async fn send(
    router: &Router,
    detector_id: &str,
    account_ids: &[String],
    case: &str,
    what: &str,
) -> Value {
    let named: Vec<&str> = account_ids.iter().map(String::as_str).collect();
    let (status, body) = invite_members(router, detector_id, &named).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {what} expected 200, got {status} {body:?}"
    );
    body
}

/// The members `account_ids` names, as `GetMembers` reports them.
async fn members_read(router: &Router, detector_id: &str, account_ids: &[&str], case: &str) -> Value {
    let (status, body) = get_members(router, detector_id, account_ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-members expected 200, got {status} {body:?}"
    );
    body
}

/// One string member of the record a read reports for `account_id`.
fn member_field(read: &Value, account_id: &str, field: &str, case: &str) -> String {
    let member = member_of(read, account_id).unwrap_or_else(|| {
        panic!("case {case:?}: expected a member record for {account_id}, got {read:?}")
    });
    member[field]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {field}, got {member:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The rule: an accepted `InviteMembers` that sends no invitation writes nothing and
// consumes no instant, so the next call that really does send one still stamps a
// wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invite_members_calls_that_send_nothing_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes: there is one detector per account per Region.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let mut recorded: Vec<(&str, &str)> = MEMBERS.to_vec();
        recorded.push(MOVER);
        let (status, created) = create_members(&router, &detector_id, &recorded).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-members expected 200, got {status} {created:?}"
        );
        // The wall clock just after the accepted write that stamped the member this
        // row measures through: the span from here to the sending call below carries
        // the burst, so the clock may advance across it only by the time that really
        // passes plus the instants the seeding invite and the send themselves take.
        let wall_before = now_millis();
        if !row.seed.is_empty() {
            send(&router, &detector_id, &row.seed, case, "the seeding invite").await;
        }

        let watched: Vec<&str> = recorded.iter().map(|(id, _)| *id).collect();
        let before_members = members_read(&router, &detector_id, &watched, case).await;
        let before_invitations = page_invitations(&router, DEFAULT_REGION, "", case).await;
        assert_eq!(
            before_invitations.len() as i64,
            row.pending,
            "case {case:?}: the measurement starts with {} invitation(s) pending, got \
             {before_invitations:?}",
            row.pending
        );

        // The instant the member this row measures through carries before the burst,
        // and the detector's own: every call the service sees from here to the sending
        // call sends nothing, so neither may move.
        let before_detector = detector_updated_at(&router, &detector_id, case).await;
        let before_mover = member_field(&before_members, MOVER.0, "updatedAt", case);

        for attempt in 0..NO_SEND_CALLS_PER_BURST {
            let answered = send(
                &router,
                &detector_id,
                &row.named,
                case,
                &format!("no-send call {attempt}"),
            )
            .await;
            assert_eq!(
                unprocessed_ids(&answered),
                row.reported,
                "case {case:?}: call {attempt} must report each account it could not process \
                 once, in request order, got {answered:?}"
            );
            for account_id in &row.reported {
                let reason = unprocessed_result(&answered, account_id);
                assert_eq!(
                    reason.as_deref(),
                    Some(NOT_AN_ASSOCIATED_MEMBER),
                    "case {case:?}: call {attempt} must report {account_id} with the reason it \
                     was not processed, got {reason:?}"
                );
            }
        }

        // A call that sends no invitation leaves every stored member exactly as it was,
        // the inbox holding exactly the invitations it held — with the ids and
        // `invitedAt` they were handed — and the detector that holds them untouched.
        let after_members = members_read(&router, &detector_id, &watched, case).await;
        assert_eq!(
            after_members, before_members,
            "case {case:?}: {NO_SEND_CALLS_PER_BURST} calls that send nothing must leave every \
             member exactly as it was, expected {before_members:?}, got {after_members:?}"
        );
        let after_invitations = page_invitations(&router, DEFAULT_REGION, "", case).await;
        assert_eq!(
            after_invitations, before_invitations,
            "case {case:?}: {NO_SEND_CALLS_PER_BURST} calls that send nothing must leave the \
             inbox exactly as it was, expected {before_invitations:?}, got {after_invitations:?}"
        );
        let counted = invitations_count(&router, DEFAULT_REGION, case).await;
        assert_eq!(
            counted, row.pending,
            "case {case:?}: the inbox must still hold {} invitation(s), got {counted}",
            row.pending
        );
        let after_detector = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_detector, before_detector,
            "case {case:?}: {NO_SEND_CALLS_PER_BURST} calls that send nothing must not move the \
             detector's updatedAt, expected {before_detector:?}, got {after_detector:?}"
        );

        // The control: the one recorded account no call above invited is invited now, so
        // this call really does send an invitation — and the instant it stamps is a
        // WALL-CLOCK instant, not one the burst pushed into the future.
        let answered = send(
            &router,
            &detector_id,
            &[MOVER.0.to_string()],
            case,
            "the sending call",
        )
        .await;
        let wall_at_send = now_millis();
        assert!(
            unprocessed_ids(&answered).is_empty(),
            "case {case:?}: the sending call must process the member it names, got {answered:?}"
        );

        // Round trip: the send is what a read of that member reports.
        let moved_read = members_read(&router, &detector_id, &[MOVER.0], case).await;
        let status_now = member_field(&moved_read, MOVER.0, "relationshipStatus", case);
        assert_eq!(
            status_now, INVITED,
            "case {case:?}: the sending call must move {}'s relationship to {INVITED:?}, got \
             {status_now:?}",
            MOVER.0
        );
        let invited_at = member_field(&moved_read, MOVER.0, "invitedAt", case);
        let after_mover = member_field(&moved_read, MOVER.0, "updatedAt", case);
        assert_eq!(
            invited_at, after_mover,
            "case {case:?}: the send stamps one instant on the member it moves, so its invitedAt \
             {invited_at:?} and its updatedAt {after_mover:?} must be the same"
        );

        // The measurement: how far the clock moved from the instant that opened the
        // span to the instant this send stamped, against how far the WALL clock moved
        // over the same span.
        let elapsed = wall_at_send - wall_before;
        let stamped = as_millis(&after_mover, case);
        let advance = stamped - as_millis(&before_mover, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {NO_SEND_CALLS_PER_BURST} calls that send nothing consume no instant, \
             so the clock may advance only by the {elapsed} ms that really passed (plus \
             {READING_SLACK_MILLIS} ms of slack), got {advance} ms (from {before_mover:?} to \
             {after_mover:?})"
        );
        assert!(
            stamped <= wall_at_send + READING_SLACK_MILLIS,
            "case {case:?}: the sending call must stamp {}'s updatedAt with the wall-clock \
             instant of the send, so {after_mover:?} ({stamped} ms) may not be more than \
             {READING_SLACK_MILLIS} ms past the {wall_at_send} ms the wall clock read just \
             after it",
            MOVER.0
        );

        // And the other side of the relationship the send writes: the Region's inbox
        // holds an invitation from the inviting account afterwards — the one it already
        // held, unchanged, where one was pending, since one invitation is pending per
        // inviting account.
        let sent_invitations = page_invitations(&router, DEFAULT_REGION, "", case).await;
        if row.pending == 0 {
            assert_eq!(
                sent_invitations.len(),
                1,
                "case {case:?}: the sending call must deliver the invitation the inbox then \
                 holds, got {sent_invitations:?}"
            );
            let delivered = &sent_invitations[0];
            let delivered_at = delivered["invitedAt"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected the invitation's invitedAt, got {delivered:?}")
            });
            assert_eq!(
                delivered_at, after_mover,
                "case {case:?}: the delivered invitation carries the instant the send stamped, \
                 expected {after_mover:?}, got {delivered_at:?}"
            );
        } else {
            assert_eq!(
                sent_invitations, before_invitations,
                "case {case:?}: one invitation is pending per inviting account, so the sending \
                 call must leave the pending one as it was, expected {before_invitations:?}, got \
                 {sent_invitations:?}"
            );
        }

        // The clock the burst left behind stamps a wall-clock instant on a resource of
        // another family too, which is where a consumed instant would have shown.
        let (status, probe) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/ipset"),
            Some(serde_json::json!({
                "name": "probe-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-ip-set.txt",
                "activate": false,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {probe:?}"
        );
        let wall_at_probe = now_millis();
        let stamped_detector = detector_updated_at(&router, &detector_id, case).await;
        let detector_millis = as_millis(&stamped_detector, case);
        assert!(
            detector_millis <= wall_at_probe + READING_SLACK_MILLIS,
            "case {case:?}: the accepted create must stamp the detector's updatedAt with the \
             wall-clock instant of that change, so {stamped_detector:?} ({detector_millis} ms) \
             may not be more than {READING_SLACK_MILLIS} ms past the {wall_at_probe} ms the wall \
             clock read just after it"
        );
    }
}

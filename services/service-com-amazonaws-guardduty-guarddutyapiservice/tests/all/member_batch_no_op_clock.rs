// SPDX-License-Identifier: CC0-1.0

//! What an ACCEPTED call of a member-batch operation that moves no member leaves
//! behind, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test, the one the `refused_mutation_clock`, `no_op_mutation_clock`
//! and `no_op_batch_clock` suites assert for the refused calls, the no-op
//! single-record writes and the per-record batches: a member-batch operation takes an
//! instant off the service's monotonic "this resource changed now" clock only once a
//! write has really landed, so a call that moves no member — every account it names
//! already holds the relationship the request asks for, or is not one this operation
//! can process — consumes no instant, and the `UpdatedAt` the next genuinely-writing
//! call stamps still reports the wall-clock instant of that change rather than one
//! pushed into the future by the no-ops before it.
//!
//! What this suite adds to `no_op_batch_clock`, which measures the clock through the
//! DETECTOR's own `UpdatedAt`: the same rule from the MEMBER's side and over the
//! relationship states only a sequence of member calls can reach. Observable per
//! request, three ways:
//!
//! * the clock a mutation reads is what the detector's own `UpdatedAt` records when a
//!   sub-resource is added to it (`smithy_query(operation="GetDetector")`), so a burst
//!   of calls that move no member between two accepted mutations shows whether the
//!   burst consumed instants: the clock may advance across it only by the wall-clock
//!   time that really passed, whereas a handler reading the clock before it knows it
//!   is going to write spends an instant per call on top of that;
//! * the `UpdatedAt` the member itself carries after the burst is followed by a call
//!   that really does move that member
//!   (`smithy_query(shape="Member",member="UpdatedAt")`), which must be the wall-clock
//!   instant of that move;
//! * and what each call of the burst ANSWERED: the accounts it could not process, once
//!   per distinct id in request order, plus the invitations the Region's inbox still
//!   holds — so a call that consumed no instant is also shown to have reported and
//!   delivered what it owed.
//!
//! The rows differ along what the rule generalises over: the operation, the reason the
//! call moves nobody (the relationship the request asks for is the one the member
//! already holds, the account has no member record, the account's association is
//! already ended, an invitation is still pending), the relationship the members are
//! left in beforehand, how many accounts one call names, and whether the named id is
//! repeated.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
// The inbox count is read through the one helper that call already has, in the suite
// that owns `GetInvitationsCount`.
use crate::invitation_count::invitations_count;
use crate::member_association::{
    create_members, get_members, member_of, unprocessed_ids, DEFAULT_REGION, MEMBERS,
    NON_MEMBER_ACCOUNT_IDS,
};
use crate::refused_create_clock::{as_millis, detector_updated_at, READING_SLACK_MILLIS};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

/// Where each member-batch operation is bound under `/detector/{DetectorId}`, from
/// its own `http` binding (`smithy_query(operation="StartMonitoringMembers,\
/// StopMonitoringMembers,InviteMembers,DisassociateMembers")`).
const START: &str = "member/start";
const STOP: &str = "member/stop";
const INVITE: &str = "member/invite";
const DISASSOCIATE: &str = "member/disassociate";

/// How many calls one burst sends. A call that writes nothing consumes no instant, so
/// the burst may be any size; it is this large because a handler that reads the clock
/// BEFORE it knows whether it writes spends at least one millisecond of it per call,
/// and the burst must advance that clock by more than [`READING_SLACK_MILLIS`] past
/// the wall-clock time the burst itself takes for the difference to be an assertion.
const NO_OPS_PER_BURST: usize = 300;

/// One call of a member-batch operation: the segment it is bound at and the accounts
/// its `AccountIds` names.
type Batch = (&'static str, Vec<&'static str>);

/// One row: the relationship to put the members in, the accepted call that moves none
/// of them, and the call afterwards that does move one.
struct Row {
    case: &'static str,
    /// The calls that prepare the members, in order, before the measurement starts.
    seed: Vec<Batch>,
    /// The call the burst repeats: accepted, and moving no member.
    no_op: Batch,
    /// The accounts the burst's answer must report as unprocessed, in request order —
    /// `UnprocessedAccounts` reports the accounts the call could not process, and a
    /// repeated id is reported once because `AccountIds` names a SET.
    reported: Vec<&'static str>,
    /// The invitations the Region's inbox must still hold after the burst, for the row
    /// whose no-op is a send that would deliver one.
    invitations: Option<i64>,
    /// A call that DOES move a member, with the account it moves: the control showing
    /// the clock the burst left behind still stamps a wall-clock instant.
    moving: Batch,
    moved: &'static str,
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn member_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

fn rows() -> Vec<Row> {
    let all = member_ids();
    let (first, second, third) = (all[0], all[1], all[2]);
    let absent = NON_MEMBER_ACCOUNT_IDS[0].0;
    let other_absent = NON_MEMBER_ACCOUNT_IDS[1].0;
    vec![
        Row {
            case: "start monitoring an account with no member record",
            seed: Vec::new(),
            no_op: (START, vec![absent]),
            reported: vec![absent],
            invitations: None,
            moving: (STOP, vec![first]),
            moved: first,
        },
        Row {
            case: "start monitoring two accounts with no member record",
            seed: Vec::new(),
            no_op: (START, vec![absent, other_absent]),
            reported: vec![absent, other_absent],
            invitations: None,
            moving: (STOP, vec![first]),
            moved: first,
        },
        Row {
            case: "start monitoring the same unknown account twice in one call",
            seed: Vec::new(),
            no_op: (START, vec![absent, absent]),
            reported: vec![absent],
            invitations: None,
            moving: (STOP, vec![second]),
            moved: second,
        },
        Row {
            case: "start monitoring members that were recorded and never stopped",
            seed: Vec::new(),
            no_op: (START, all.clone()),
            reported: Vec::new(),
            invitations: None,
            moving: (STOP, vec![third]),
            moved: third,
        },
        Row {
            case: "start monitoring members whose monitoring was already restarted",
            seed: vec![(STOP, all.clone()), (START, all.clone())],
            no_op: (START, all.clone()),
            reported: Vec::new(),
            invitations: None,
            moving: (STOP, vec![first]),
            moved: first,
        },
        Row {
            case: "start monitoring members whose invitation is still outstanding",
            seed: vec![(INVITE, all.clone())],
            no_op: (START, all.clone()),
            reported: Vec::new(),
            invitations: Some(1),
            moving: (STOP, vec![second]),
            moved: second,
        },
        Row {
            case: "stop monitoring members whose monitoring was already stopped",
            seed: vec![(STOP, all.clone())],
            no_op: (STOP, all.clone()),
            reported: Vec::new(),
            invitations: None,
            moving: (START, vec![first]),
            moved: first,
        },
        Row {
            case: "stop monitoring one account with no member record",
            seed: Vec::new(),
            no_op: (STOP, vec![other_absent]),
            reported: vec![other_absent],
            invitations: None,
            moving: (STOP, vec![third]),
            moved: third,
        },
        Row {
            case: "disassociate members whose association is already ended",
            seed: vec![(DISASSOCIATE, all.clone())],
            no_op: (DISASSOCIATE, all.clone()),
            reported: all.clone(),
            invitations: None,
            // A retained record can be invited again, which re-associates it: the one
            // call that moves a member this row left disassociated.
            moving: (INVITE, vec![first]),
            moved: first,
        },
        Row {
            case: "disassociate an account with no member record",
            seed: Vec::new(),
            no_op: (DISASSOCIATE, vec![absent]),
            reported: vec![absent],
            invitations: None,
            moving: (DISASSOCIATE, vec![second]),
            moved: second,
        },
        Row {
            case: "invite members whose invitation is still pending",
            seed: vec![(INVITE, all.clone())],
            no_op: (INVITE, all.clone()),
            reported: Vec::new(),
            // The re-send delivers no second invitation: one is pending per inviting
            // account, and the pending one keeps the id it was handed.
            invitations: Some(1),
            moving: (STOP, vec![third]),
            moved: third,
        },
        Row {
            case: "invite an account with no member record",
            seed: Vec::new(),
            no_op: (INVITE, vec![absent, other_absent]),
            reported: vec![absent, other_absent],
            invitations: Some(0),
            moving: (INVITE, vec![first]),
            moved: first,
        },
    ]
}

/// Send one member-batch call, asserting the modelled success, and answer the body.
async fn send(
    router: &Router,
    detector_id: &str,
    (segment, accounts): &Batch,
    case: &str,
    what: &str,
) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{segment}"),
        Some(json!({"accountIds": accounts})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {what} expected 200, got {status} {body:?}"
    );
    body
}

/// The members `account_ids` names, as `GetMembers` reports them.
async fn members_read(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
    case: &str,
) -> Value {
    let (status, body) = get_members(router, detector_id, account_ids).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-members expected 200, got {status} {body:?}"
    );
    body
}

/// The `updatedAt` a read reports for one member.
fn member_updated_at(read: &Value, account_id: &str, case: &str) -> String {
    let member = member_of(read, account_id).unwrap_or_else(|| {
        panic!("case {case:?}: expected a member record for {account_id}, got {read:?}")
    });
    member["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an updatedAt, got {member:?}"))
        .to_string()
}

/// How many invitations the Region's inbox holds.
async fn pending_invitations(router: &Router, case: &str) -> i64 {
    invitations_count(router, DEFAULT_REGION, case).await
}

// ---------------------------------------------------------------------------
// The rule: a member-batch call that moves no member consumes no instant, so the
// next accepted mutation still stamps a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn member_batch_calls_that_move_no_member_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes: there is one detector per account per Region.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let (status, recorded) = create_members(&router, &detector_id, &MEMBERS).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-members expected 200, got {status} {recorded:?}"
        );
        for seed in &row.seed {
            send(&router, &detector_id, seed, case, "the seeding call").await;
        }

        let watched = member_ids();
        let before_members = members_read(&router, &detector_id, &watched, case).await;

        // The instant the last ACCEPTED write stamped, and the wall clock just after
        // it: between these two readings and the accepted create below, every call the
        // service sees moves no member, so the clock may advance only by the time that
        // really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        for attempt in 0..NO_OPS_PER_BURST {
            let answered = send(
                &router,
                &detector_id,
                &row.no_op,
                case,
                &format!("no-op call {attempt}"),
            )
            .await;
            assert_eq!(
                unprocessed_ids(&answered),
                row.reported
                    .iter()
                    .map(|account_id| (*account_id).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: call {attempt} must report each account it could not \
                 process once, in request order, got {answered:?}"
            );
        }

        // A call that moves no member leaves every stored member exactly as it was.
        let after_members = members_read(&router, &detector_id, &watched, case).await;
        assert_eq!(
            after_members, before_members,
            "case {case:?}: {NO_OPS_PER_BURST} calls that move no member must leave every \
             member exactly as it was, expected {before_members:?}, got {after_members:?}"
        );

        // Nor does such a call deliver an invitation that was already pending, or one
        // for an account it could not process at all.
        if let Some(expected) = row.invitations {
            let counted = pending_invitations(&router, case).await;
            assert_eq!(
                counted, expected,
                "case {case:?}: {NO_OPS_PER_BURST} calls that move no member must leave the \
                 inbox holding {expected} invitation(s), got {counted}"
            );
        }

        // Nor is the detector that holds the members touched.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {NO_OPS_PER_BURST} calls that move no member must not move the \
             detector's updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );

        // The accepted create right after the burst: it writes, so it stamps the
        // instant it happened — and that instant is a WALL-CLOCK instant, not one the
        // burst pushed into the future.
        let probe_collection = format!("/detector/{detector_id}/ipset");
        let (status, created) = call(
            &router,
            "POST",
            &probe_collection,
            Some(json!({
                "name": "probe-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-ip-set.txt",
                "activate": false,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {created:?}"
        );
        let minted = created["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {created:?}"))
            .to_string();

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted create changed the detector, so its updatedAt must \
             move past {after_burst:?}, got {stamped:?}"
        );
        // The measurement: how far the clock moved across the burst, against how far
        // the WALL clock moved across the same span.
        let elapsed = wall_after - wall_before;
        let advance = as_millis(&stamped, case) - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + READING_SLACK_MILLIS,
            "case {case:?}: {NO_OPS_PER_BURST} calls that move no member consume no instant, \
             so the clock may advance only by the {elapsed} ms that really passed (plus \
             {READING_SLACK_MILLIS} ms of slack), got {advance} ms (from {before_burst:?} to \
             {stamped:?})"
        );

        // Round trip: the IPSet the probing create minted is one a read resolves, so
        // the timestamp asserted above records a change that really landed.
        let (status, read_back) =
            call(&router, "GET", &format!("{probe_collection}/{minted}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted ipSetId to answer 200, got {status} \
             {read_back:?}"
        );

        // The control: a call of the same family that DOES move a member moves that
        // member's own `UpdatedAt`, and the instant it stamps is the wall-clock instant
        // of the move rather than one the burst spent ahead of it.
        let before_move = member_updated_at(&before_members, row.moved, case);
        send(&router, &detector_id, &row.moving, case, "the moving call").await;
        let wall_at_move = now_millis();
        let moved_read = members_read(&router, &detector_id, &[row.moved], case).await;
        let after_move = member_updated_at(&moved_read, row.moved, case);
        assert!(
            after_move > before_move,
            "case {case:?}: the moving call must move {}'s updatedAt past {before_move:?}, \
             got {after_move:?}",
            row.moved
        );
        let stamp = as_millis(&after_move, case);
        assert!(
            stamp <= wall_at_move + READING_SLACK_MILLIS,
            "case {case:?}: the moving call must stamp {}'s updatedAt with the wall-clock \
             instant of the move, so {after_move:?} ({stamp} ms) may not be more than \
             {READING_SLACK_MILLIS} ms past the {wall_at_move} ms the wall clock read \
             just after it",
            row.moved
        );
    }
}

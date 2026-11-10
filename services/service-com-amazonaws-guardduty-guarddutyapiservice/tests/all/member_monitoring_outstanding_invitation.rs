// SPDX-License-Identifier: CC0-1.0

//! SEQUENCES of the two monitoring calls — `StopMonitoringMembers` and
//! `StartMonitoringMembers` — against a member whose invitation the member ACCOUNT
//! has not answered, and against the neighbouring relationships.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rule under test: GuardDuty monitors a member account under its administrator
//! only once that ACCOUNT has answered the invitation — "the invited accounts can
//! either accept or decline the invitation from their GuardDuty accounts"
//! (`smithy_query(query="doc",shape="InviteMembers")`) — so neither monitoring call
//! writes a relationship for an account the administrator does not monitor, and
//! because neither writes one, no SEQUENCE of them writes one either.
//!
//! What the `member_monitoring` and `member_monitoring_restart` suites assert per
//! CALL, this one asserts per SEQUENCE: the two calls are inverses over one stored
//! field, so a composition of them must not walk a relationship neither call can move
//! on its own — an account only recorded, or one whose invitation is still waiting
//! for its answer — into the monitored relationship, which is the status the service
//! reserves for an account that accepted or whose monitoring had genuinely been
//! stopped.
//!
//! The rows are the CROSS PRODUCT of what the rule generalises over: the relationship
//! the member record is in (recorded and not yet invited, invited and unanswered,
//! disassociated) and the sequence of monitoring calls sent to it (each call alone,
//! each order of the pair, and the pair repeated), observed through `GetMembers` and
//! `ListMembers` around the whole sequence.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, new_router, Router};
// The member-suite tables, helpers and projections live in one place, so no two suites
// drift on what a member read looks like. `MEMBERS` is the table of accounts to
// record, differing along the account id and the email's shape.
use crate::member_association::{
    create_members, get_members, member_of, unprocessed_ids, unprocessed_result, MEMBERS,
    NOT_AN_ASSOCIATED_MEMBER,
};

/// Where the two monitoring calls are bound under `/detector/{DetectorId}/member`,
/// from their own `http` bindings (`smithy_query(operation="StopMonitoringMembers,\
/// StartMonitoringMembers")`). Shared with the monitoring suites beside this one.
pub(crate) const STOP: &str = "stop";
pub(crate) const START: &str = "start";

/// One member-batch call: the path segment under `/detector/{DetectorId}/member` and
/// the accounts its `AccountIds` names.
pub(crate) async fn member_call(
    router: &Router,
    detector_id: &str,
    step: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/{step}"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// `ListMembers`, narrowed (or not) to the ASSOCIATED members.
async fn list_members(router: &Router, detector_id: &str, only_associated: bool) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/member?onlyAssociated={only_associated}"),
        None,
    )
    .await
}

/// `ListMembers`, narrowed (or not) to the ASSOCIATED members, asserting the
/// modelled success so a read that started refusing cannot be compared as a body.
async fn listed_members(
    router: &Router,
    detector_id: &str,
    only_associated: bool,
    case: &str,
) -> Value {
    let (status, body) = list_members(router, detector_id, only_associated).await;
    assert_eq!(
        status, 200,
        "case {case}: list-members onlyAssociated={only_associated} expected 200, got {status} \
         {body:?}"
    );
    body
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn all_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

/// Create an enabled detector, record [`MEMBERS`] against it and drive every one of
/// them through `steps`, returning the detector id.
async fn detector_with_members_driven(router: &Router, steps: &[&str], case: &str) -> String {
    let detector_id = enabled_detector(router).await;
    let (status, body) = create_members(router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "case {case}: create-members expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {case}: create-members expected every account processed, got {body:?}"
    );
    let ids = all_ids();
    for step in steps {
        let (status, body) = member_call(router, &detector_id, step, &ids).await;
        assert_eq!(
            status, 200,
            "case {case}: member/{step} expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: member/{step} expected every member processed, got {body:?}"
        );
    }
    detector_id
}

/// The `relationshipStatus` a read reports for one account.
pub(crate) fn relationship_status(read: &Value, account_id: &str, case: &str) -> String {
    let member = member_of(read, account_id)
        .unwrap_or_else(|| panic!("case {case}: expected a member record, got {read:?}"));
    member["relationshipStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a relationshipStatus, got {member:?}"))
        .to_string()
}

/// Every relationship a member record reaches through the requests this administrator
/// can send, with the member calls that drive a freshly recorded account into it.
///
/// None of them is a relationship GuardDuty MONITORS under this administrator: that
/// one begins with the member account's own answer to the invitation, which is not a
/// request this administrator can send.
const RELATIONSHIPS: [(&str, &[&str]); 3] = [
    ("Created", &[]),
    ("Invited", &["invite"]),
    ("Removed", &["invite", "disassociate"]),
];

/// The monitoring-call sequences one row sends: each call alone, each order of the
/// pair, and the pair repeated — so neither a single call, nor an order of them, nor a
/// repetition can accumulate into a write. Shared with the monitoring suites beside this
/// one, so the sequences every one of them spans are enumerated once.
pub(crate) const SEQUENCES: [&[&str]; 5] = [
    &[STOP],
    &[START],
    &[STOP, START],
    &[START, STOP],
    &[STOP, START, STOP, START],
];

// ---------------------------------------------------------------------------
// The rule: no sequence of monitoring calls writes a relationship for an account
// the administrator does not monitor.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_sequence_of_monitoring_calls_moves_a_member_the_administrator_does_not_monitor() {
    for (relationship, steps) in RELATIONSHIPS {
        for sequence in SEQUENCES {
            let case = format!("{relationship} then {sequence:?}");
            // One store per row: each row drives the same accounts into a different
            // relationship, so it starts from a detector of its own.
            let router = new_router();
            let detector_id = detector_with_members_driven(&router, steps, &case).await;
            let ids = all_ids();

            // The row's premise: every record really is in the relationship it names.
            let (status, before) = get_members(&router, &detector_id, &ids).await;
            assert_eq!(
                status, 200,
                "case {case}: get-members expected 200, got {status} {before:?}"
            );
            for account_id in &ids {
                assert_eq!(
                    relationship_status(&before, account_id, &case),
                    relationship,
                    "case {case} {account_id}: expected the row's relationship, got {before:?}"
                );
            }
            let associated_before = listed_members(&router, &detector_id, true, &case).await;
            let all_before = listed_members(&router, &detector_id, false, &case).await;

            // Every call of the sequence answers 200 and names each account it cannot
            // process, with the reason it was not processed.
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
                    "case {case}: member/{step} at {position} must report each account it \
                     cannot process, once in request order, got {body:?}"
                );
                for account_id in &ids {
                    assert_eq!(
                        unprocessed_result(&body, account_id).as_deref(),
                        Some(NOT_AN_ASSOCIATED_MEMBER),
                        "case {case} {account_id}: member/{step} at {position} expected the \
                         not-an-associated-member reason, got {body:?}"
                    );
                }
            }

            // Nothing the sequence named moved: not the relationship — so no
            // composition of the two calls reaches the monitored one — not the
            // invitation's instant, and not the relationship's own `updatedAt`.
            let (status, after) = get_members(&router, &detector_id, &ids).await;
            assert_eq!(
                status, 200,
                "case {case}: get-members after the sequence expected 200, got {status} {after:?}"
            );
            assert_eq!(
                after, before,
                "case {case}: expected the sequence to leave every member exactly as it was, \
                 got {after:?} after {before:?}"
            );
            for account_id in &ids {
                assert_eq!(
                    relationship_status(&after, account_id, &case),
                    relationship,
                    "case {case} {account_id}: expected the member to keep the row's \
                     relationship, got {after:?}"
                );
            }

            // Nor the listing the members are reported in, under either narrowing.
            let associated_after = listed_members(&router, &detector_id, true, &case).await;
            assert_eq!(
                associated_after, associated_before,
                "case {case}: expected the associated listing to be unchanged, got \
                 {associated_after:?} after {associated_before:?}"
            );
            let all_after = listed_members(&router, &detector_id, false, &case).await;
            assert_eq!(
                all_after, all_before,
                "case {case}: expected the full listing to be unchanged, got {all_after:?} \
                 after {all_before:?}"
            );
        }
    }
}

// SPDX-License-Identifier: CC0-1.0

//! `StopMonitoringMembers`: stopping GuardDuty monitoring for member accounts.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Stopping monitoring moves ONLY the relationship of the named member accounts,
//!    to `Disabled`, and retains everything else about the record — so a later
//!    `GetMembers` still reports the account, its email and its administrator — and
//!    leaves every member the call did not name where it was.
//! 2. Stopping is a DESIRED STATE, not a transition: a second stop of the same
//!    accounts answers 200, reports nothing unprocessed and changes nothing at all,
//!    `UpdatedAt` included.
//! 3. A stopped member is still ASSOCIATED with the administrator: `ListMembers`
//!    with `onlyAssociated=true` still lists it, and `DisassociateMembers` still
//!    processes it — only that operation ends the association.
//! 4. An account the administrator is not associated with — one it holds no member
//!    record for, and one a disassociation already moved — is not an error: the call
//!    answers HTTP 200 and names the account in `unprocessedAccounts` with the
//!    reason it was not processed, once per DISTINCT id in request order, and
//!    persists nothing for it.
//! 5. A value that cannot be an account id at all, and a `DetectorId` this
//!    account/Region does not own, are each refused with HTTP 400 under the
//!    `InvalidInputException` class — never answered with a hollow 200.

use serde_json::{json, Value};

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
// The message, projection and administrator helpers the member suites share: one
// definition, so two suites cannot drift apart on what a member read looks like.
// The member-suite tables, helpers and projections live in one place, so the two
// suites cannot drift on what a malformed or non-member account id is, nor on what a
// member read looks like. `MEMBERS` is the table of accounts to record, differing
// along the account id and the email's shape; this suite stops monitoring for them.
use crate::member_association::{
    administrator, create_members, disassociate_members, get_members, member_of, not_a_member,
    unprocessed_ids, unprocessed_result, MALFORMED_ACCOUNT_IDS, MEMBERS,
    NON_MEMBER_ACCOUNT_IDS,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// `StopMonitoringMembers` over the wire.
async fn stop_monitoring_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/stop"),
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

/// Create an enabled detector and record [`MEMBERS`] against it, returning the
/// detector id.
async fn detector_with_members(router: &Router) -> String {
    let detector_id = enabled_detector(router).await;
    let (status, body) = create_members(router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "create-members: expected every account processed, got {body:?}"
    );
    detector_id
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn all_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

// ---------------------------------------------------------------------------
// Rules 1 and 3: stop -> read, over a subset.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_monitoring_moves_only_the_named_members_to_disabled() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let administrator = administrator();
    let ids = all_ids();

    let (status, created) = get_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "get-members after create: expected 200, got {status} {created:?}"
    );

    // A strict, NON-CONTIGUOUS subset, so the call is observable both ways and the
    // moved members are not merely a prefix of the stored order.
    let stopped = [MEMBERS[0].0, MEMBERS[2].0];
    let (status, body) = stop_monitoring_members(&router, &detector_id, &stopped).await;
    assert_eq!(
        status, 200,
        "stop-monitoring-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "stop-monitoring-members: expected every member processed, got {body:?}"
    );

    let (status, after) = get_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "get-members after stop: expected 200, got {status} {after:?}"
    );
    for (account_id, email) in MEMBERS {
        let expected_status = if stopped.contains(&account_id) {
            "Disabled"
        } else {
            "Created"
        };
        let member = member_of(&after, account_id).unwrap_or_else(|| {
            panic!("case {account_id}: expected the record to be retained, got {after:?}")
        });
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(expected_status),
            "case {account_id}: expected relationshipStatus {expected_status}, got {member:?}"
        );
        // Only the relationship moves: the details are RETAINED.
        assert_eq!(
            member["email"].as_str(),
            Some(email),
            "case {account_id}: expected the email to be retained, got {member:?}"
        );
        assert_eq!(
            member["masterId"].as_str(),
            Some(administrator.as_str()),
            "case {account_id}: expected masterId to be the administrator, got {member:?}"
        );
        assert_eq!(
            member["administratorId"].as_str(),
            Some(administrator.as_str()),
            "case {account_id}: expected administratorId to be the administrator, got {member:?}"
        );
        let before = member_of(&created, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {created:?}"));
        let moved = member["updatedAt"] != before["updatedAt"];
        assert_eq!(
            moved,
            stopped.contains(&account_id),
            "case {account_id}: expected updatedAt to move only for a stopped member, got {:?} then {:?}",
            before["updatedAt"],
            member["updatedAt"]
        );
    }

    // Rule 3: monitoring stopped, association kept — the narrowed listing still
    // names every member.
    for only_associated in [true, false] {
        let (status, listed) = list_members(&router, &detector_id, only_associated).await;
        assert_eq!(
            status, 200,
            "case onlyAssociated={only_associated}: expected 200, got {status} {listed:?}"
        );
        for account_id in &ids {
            assert!(
                member_of(&listed, account_id).is_some(),
                "case onlyAssociated={only_associated} {account_id}: expected a stopped member to still be associated, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_stopped_member_can_still_be_disassociated() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let account_id = MEMBERS[1].0;

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");

    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "disassociate: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {account_id}: expected a stopped member to still be associated, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
    let member = member_of(&read, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Removed"),
        "case {account_id}: expected the disassociation to move the relationship, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: stopping is a desired state, so a repeat changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_monitoring_again_changes_nothing() {
    let ids = all_ids();
    // Rows differing along the COUNT of accounts one call names: one, a few, and
    // every member the administrator holds.
    let batches: [&[&str]; 3] = [&ids[..1], &ids[..2], &ids[..]];

    for batch in batches {
        // A fresh engine per row: there is only one detector per account per Region,
        // so each row gets its own emulator rather than a second detector.
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (status, first) = stop_monitoring_members(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: first stop expected 200, got {status} {first:?}"
        );
        let (_, once) = get_members(&router, &detector_id, batch).await;
        // The first stop must have DONE something, or the comparison below would
        // pass over two unchanged reads.
        for account_id in batch {
            let member = member_of(&once, account_id).unwrap_or_else(|| {
                panic!("case {batch:?} {account_id}: expected a member, got {once:?}")
            });
            assert_eq!(
                member["relationshipStatus"].as_str(),
                Some("Disabled"),
                "case {batch:?} {account_id}: expected the first stop to disable monitoring, got {member:?}"
            );
        }

        let (status, second) = stop_monitoring_members(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: second stop expected 200, got {status} {second:?}"
        );
        assert!(
            unprocessed_ids(&second).is_empty(),
            "case {batch:?}: expected a stopped member to still be processed, got {second:?}"
        );
        let (_, twice) = get_members(&router, &detector_id, batch).await;
        assert_eq!(
            twice, once,
            "case {batch:?}: expected the second stop to change nothing, got {twice:?} after {once:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an account that is not an associated member is REPORTED, not refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_that_is_not_an_associated_member_is_reported_not_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let reason = not_a_member();

    // The administrator's own account is not one of its members either, so it is a
    // row of this table beside the ids nothing recorded.
    let administrator = administrator();
    let mut rows: Vec<(&str, &str)> = NON_MEMBER_ACCOUNT_IDS.to_vec();
    rows.push((administrator.as_str(), "the administrator's own account"));

    for (account_id, why) in rows {
        let (status, body) = stop_monitoring_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id} ({why}): expected 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            vec![account_id.to_string()],
            "case {account_id} ({why}): expected exactly that account unprocessed, got {body:?}"
        );
        assert_eq!(
            unprocessed_result(&body, account_id).as_deref(),
            Some(reason.as_str()),
            "case {account_id} ({why}): expected the not-a-member reason, got {body:?}"
        );

        // The reported account had nothing persisted for it.
        let (status, read) = get_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id} ({why}): get-members expected 200, got {status} {read:?}"
        );
        assert!(
            member_of(&read, account_id).is_none(),
            "case {account_id} ({why}): expected no member record, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_disassociated_member_is_reported_and_keeps_its_relationship() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let (account_id, email) = MEMBERS[1];
    let reason = not_a_member();

    let (status, _) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "disassociate: expected 200, got {status}");
    let (_, removed) = get_members(&router, &detector_id, &[account_id]).await;
    let before = member_of(&removed, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a retained record, got {removed:?}"));

    let (status, body) = stop_monitoring_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {account_id}: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_result(&body, account_id).as_deref(),
        Some(reason.as_str()),
        "case {account_id}: a disassociated member is no longer associated, got {body:?}"
    );

    let (_, after) = get_members(&router, &detector_id, &[account_id]).await;
    let member = member_of(&after, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a retained record, got {after:?}"));
    assert_eq!(
        member, before,
        "case {account_id}: expected the reported account to be unchanged, got {member:?} after {before:?}"
    );
    assert_eq!(
        member["email"].as_str(),
        Some(email),
        "case {account_id}: expected the email to be retained, got {member:?}"
    );
}

#[tokio::test]
async fn only_the_accounts_that_are_not_members_are_reported_once_in_request_order() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let member_id = MEMBERS[2].0;
    let absent_one = NON_MEMBER_ACCOUNT_IDS[0].0;
    let absent_two = NON_MEMBER_ACCOUNT_IDS[1].0;

    // A mixed request: the member is processed, the rest reported, in request order,
    // and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [absent_two, member_id, absent_one, absent_two, member_id];
    let (status, body) = stop_monitoring_members(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed stop: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed stop: expected each absent account once, in request order, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    let member = member_of(&read, member_id)
        .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Disabled"),
        "case {member_id}: expected the member in a mixed request to be stopped, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: requests the service does not understand are refused 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_cannot_be_an_account_id_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let expected = invalid_parameter("accountId");
    let member_id = MEMBERS[0].0;

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // The whole call is refused, so a well-formed id sent alongside is not
        // stopped either.
        let (status, body) =
            stop_monitoring_members(&router, &detector_id, &[member_id, account_id]).await;
        assert_eq!(
            status, 400,
            "case {account_id} ({why}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case {account_id} ({why}): expected the {CLASS} class, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {account_id} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {account_id} ({why}): expected the parameter message, got {body:?}"
        );

        let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
        let member = member_of(&read, member_id)
            .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some("Created"),
            "case {account_id} ({why}): expected a refused call to stop nothing, got {member:?}"
        );
    }
}

#[tokio::test]
async fn stopping_monitoring_on_a_detector_this_account_does_not_own_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let member_id = MEMBERS[0].0;

    // A detector another Region's endpoint serves is not owned here: a detector is a
    // regional resource.
    let (status, elsewhere) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-detector in eu-west-1: expected 200, got {status} {elsewhere:?}"
    );
    let other_region_id = elsewhere["detectorId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-detector in eu-west-1: expected a detectorId, got {elsewhere:?}")
        })
        .to_string();

    let rejections: [(&str, &str); 3] = [
        (
            "deadbeefdeadbeefdeadbeefdeadbeeg",
            "32 characters, but 'g' is not hexadecimal",
        ),
        (ABSENT_DETECTOR_ID, "a well-formed id nothing minted"),
        (
            other_region_id.as_str(),
            "a detector another Region's endpoint serves",
        ),
    ];

    for (candidate, why) in rejections {
        let expected = if candidate.len() == 32 && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
            not_owned("detectorId")
        } else {
            invalid_parameter("detectorId")
        };
        let (status, body) = stop_monitoring_members(&router, candidate, &[member_id]).await;
        assert_eq!(
            status, 400,
            "case {candidate} ({why}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case {candidate} ({why}): expected the {CLASS} class, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {candidate} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {candidate} ({why}): expected the classified message, got {body:?}"
        );
    }

    // The detector that was named by a refused call is untouched.
    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    let member = member_of(&read, member_id)
        .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Created"),
        "case {member_id}: expected a refused call to stop nothing, got {member:?}"
    );
}

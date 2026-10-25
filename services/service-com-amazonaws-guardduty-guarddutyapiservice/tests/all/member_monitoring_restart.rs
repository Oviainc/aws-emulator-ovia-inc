// SPDX-License-Identifier: CC0-1.0

//! `StartMonitoringMembers`: turning GuardDuty monitoring back on for member
//! accounts whose monitoring was stopped.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. Restarting monitoring moves ONLY the relationship of the named member
//!    accounts whose monitoring was stopped, from `Disabled` to the monitored
//!    status `Enabled`, and retains everything else about the record — so a later
//!    `GetMembers` still reports the account, its email and its administrator — and
//!    leaves every member the call did not name where it was.
//! 2. Monitoring is a DESIRED STATE, not a transition: a member that is not
//!    stopped is processed with nothing changed at all, `UpdatedAt` included —
//!    whether it was already restarted, never stopped, or still carries an
//!    outstanding invitation.
//! 3. Stopping and restarting are inverses, so monitoring can be stopped and
//!    restarted repeatedly, and a restarted member is still ASSOCIATED with the
//!    administrator: `ListMembers` with `onlyAssociated=true` still lists it and
//!    `DisassociateMembers` still processes it.
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
// The member-suite tables, helpers and projections live in one place, so no two
// suites drift on what a malformed or non-member account id is, nor on what a member
// read looks like. `MEMBERS` is the table of accounts to record, differing along the
// account id and the email's shape; this suite stops and restarts monitoring of them.
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

/// `StartMonitoringMembers` over the wire.
async fn start_monitoring_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/start"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// `StopMonitoringMembers` over the wire: the call this suite restarts monitoring
/// after.
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

/// `InviteMembers` over the wire, for the row whose member carries an outstanding
/// invitation.
async fn invite_members(router: &Router, detector_id: &str, account_ids: &[&str]) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/invite"),
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

/// The `relationshipStatus` a read reports for `account_id`, panicking when the
/// record is missing.
fn relationship_status(body: &Value, account_id: &str) -> String {
    let member = member_of(body, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a member record, got {body:?}"));
    member["relationshipStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {account_id}: expected a relationshipStatus, got {member:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rules 1 and 3: stop -> start -> read, over a subset.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restarting_monitoring_moves_only_the_named_stopped_members_to_enabled() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let administrator = administrator();
    let ids = all_ids();

    let (status, body) = stop_monitoring_members(&router, &detector_id, &ids).await;
    assert_eq!(status, 200, "stop: expected 200, got {status} {body:?}");
    let (status, stopped_read) = get_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "get-members after stop: expected 200, got {status} {stopped_read:?}"
    );

    // A strict, NON-CONTIGUOUS subset, so the call is observable both ways and the
    // moved members are not merely a prefix of the stored order.
    let restarted = [MEMBERS[0].0, MEMBERS[2].0];
    let (status, body) = start_monitoring_members(&router, &detector_id, &restarted).await;
    assert_eq!(
        status, 200,
        "start-monitoring-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "start-monitoring-members: expected every member processed, got {body:?}"
    );

    let (status, after) = get_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "get-members after start: expected 200, got {status} {after:?}"
    );
    for (account_id, email) in MEMBERS {
        let expected_status = if restarted.contains(&account_id) {
            "Enabled"
        } else {
            "Disabled"
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
        let before = member_of(&stopped_read, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {stopped_read:?}"));
        if restarted.contains(&account_id) {
            assert_ne!(
                member["updatedAt"], before["updatedAt"],
                "case {account_id}: expected updatedAt to move for a restarted member, got {:?}",
                member["updatedAt"]
            );
        } else {
            // A member the call did not name is left exactly where it was.
            assert_eq!(
                member, before,
                "case {account_id}: expected an unnamed member to be unchanged, got {member:?} after {before:?}"
            );
        }
    }

    // Rule 3: a restarted member is still associated — the narrowed listing still
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
                "case onlyAssociated={only_associated} {account_id}: expected a restarted member to still be associated, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_restarted_member_can_still_be_disassociated() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let account_id = MEMBERS[1].0;

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");
    let (status, body) = start_monitoring_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "start: expected 200, got {status} {body:?}");

    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "disassociate: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {account_id}: expected a restarted member to still be associated, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        relationship_status(&read, account_id),
        "Removed",
        "case {account_id}: expected the disassociation to move the relationship, got {read:?}"
    );
}

#[tokio::test]
async fn monitoring_can_be_stopped_and_restarted_repeatedly() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();
    // Three cycles, so the pair is shown to be inverses rather than a one-shot
    // transition the second call could not undo.
    for cycle in 1..=3 {
        let (status, body) = stop_monitoring_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: stop expected 200, got {status} {body:?}"
        );
        let (_, read) = get_members(&router, &detector_id, &ids).await;
        for account_id in &ids {
            assert_eq!(
                relationship_status(&read, account_id),
                "Disabled",
                "case cycle {cycle} {account_id}: expected monitoring to stop, got {read:?}"
            );
        }

        let (status, body) = start_monitoring_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: start expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case cycle {cycle}: expected every member processed, got {body:?}"
        );
        let (_, read) = get_members(&router, &detector_id, &ids).await;
        for account_id in &ids {
            assert_eq!(
                relationship_status(&read, account_id),
                "Enabled",
                "case cycle {cycle} {account_id}: expected monitoring to restart, got {read:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: monitoring is a desired state, so a start that owes nothing changes
// nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restarting_monitoring_again_changes_nothing() {
    let ids = all_ids();
    // Rows differing along the COUNT of accounts one call names: one, a few, and
    // every member the administrator holds.
    let batches: [&[&str]; 3] = [&ids[..1], &ids[..2], &ids[..]];

    for batch in batches {
        // A fresh engine per row: there is only one detector per account per Region,
        // so each row gets its own emulator rather than a second detector.
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (status, body) = stop_monitoring_members(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: stop expected 200, got {status} {body:?}"
        );

        let (status, first) = start_monitoring_members(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: first start expected 200, got {status} {first:?}"
        );
        let (_, once) = get_members(&router, &detector_id, batch).await;
        // The first start must have DONE something, or the comparison below would
        // pass over two unchanged reads.
        for account_id in batch {
            assert_eq!(
                relationship_status(&once, account_id),
                "Enabled",
                "case {batch:?} {account_id}: expected the first start to restart monitoring, got {once:?}"
            );
        }

        let (status, second) = start_monitoring_members(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: second start expected 200, got {status} {second:?}"
        );
        assert!(
            unprocessed_ids(&second).is_empty(),
            "case {batch:?}: expected a monitored member to still be processed, got {second:?}"
        );
        let (_, twice) = get_members(&router, &detector_id, batch).await;
        assert_eq!(
            twice, once,
            "case {batch:?}: expected the second start to change nothing, got {twice:?} after {once:?}"
        );
    }
}

#[tokio::test]
async fn restarting_monitoring_of_a_member_that_was_not_stopped_changes_nothing() {
    // Rows differing along the relationship the member is in when monitoring is
    // turned on: recorded and not yet invited, invited and not yet answered. Neither
    // is a member whose monitoring was stopped, so neither moves — an outstanding
    // invitation is answered by the member account, never by this call.
    let rows: [(&str, &str); 2] = [
        ("Created", "recorded by CreateMembers and not yet invited"),
        ("Invited", "invited and not yet answered"),
    ];

    for (expected_status, why) in rows {
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let ids = all_ids();
        if expected_status == "Invited" {
            let (status, body) = invite_members(&router, &detector_id, &ids).await;
            assert_eq!(
                status, 200,
                "case {expected_status} ({why}): invite expected 200, got {status} {body:?}"
            );
        }
        let (_, before) = get_members(&router, &detector_id, &ids).await;
        for account_id in &ids {
            assert_eq!(
                relationship_status(&before, account_id),
                expected_status,
                "case {expected_status} ({why}) {account_id}: expected the row's relationship, got {before:?}"
            );
        }

        let (status, body) = start_monitoring_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case {expected_status} ({why}): expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {expected_status} ({why}): expected every associated member processed, got {body:?}"
        );

        let (_, after) = get_members(&router, &detector_id, &ids).await;
        assert_eq!(
            after, before,
            "case {expected_status} ({why}): expected a member that was not stopped to be unchanged, got {after:?} after {before:?}"
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
        let (status, body) = start_monitoring_members(&router, &detector_id, &[account_id]).await;
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

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");
    let (status, _) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "disassociate: expected 200, got {status}");
    let (_, removed) = get_members(&router, &detector_id, &[account_id]).await;
    let before = member_of(&removed, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a retained record, got {removed:?}"));

    let (status, body) = start_monitoring_members(&router, &detector_id, &[account_id]).await;
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

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[member_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");

    // A mixed request: the member is processed, the rest reported, in request order,
    // and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [absent_two, member_id, absent_one, absent_two, member_id];
    let (status, body) = start_monitoring_members(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed start: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed start: expected each absent account once, in request order, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    assert_eq!(
        relationship_status(&read, member_id),
        "Enabled",
        "case {member_id}: expected the member in a mixed request to be restarted, got {read:?}"
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

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[member_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // The whole call is refused, so a well-formed id sent alongside is not
        // restarted either.
        let (status, body) =
            start_monitoring_members(&router, &detector_id, &[member_id, account_id]).await;
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
        assert_eq!(
            relationship_status(&read, member_id),
            "Disabled",
            "case {account_id} ({why}): expected a refused call to restart nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn restarting_monitoring_on_a_detector_this_account_does_not_own_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let member_id = MEMBERS[0].0;

    let (status, _) = stop_monitoring_members(&router, &detector_id, &[member_id]).await;
    assert_eq!(status, 200, "stop: expected 200, got {status}");

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
        let expected = if candidate.len() == 32 && candidate.bytes().all(|b| b.is_ascii_hexdigit())
        {
            not_owned("detectorId")
        } else {
            invalid_parameter("detectorId")
        };
        let (status, body) = start_monitoring_members(&router, candidate, &[member_id]).await;
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
    assert_eq!(
        relationship_status(&read, member_id),
        "Disabled",
        "case {member_id}: expected a refused call to restart nothing, got {read:?}"
    );
}

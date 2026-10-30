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
//! 1. Monitoring is restarted only for an account GuardDuty MONITORS under this
//!    administrator — "use this operation to restart monitoring of accounts that you
//!    stopped monitoring with the StopMonitoringMembers operation"
//!    (`smithy_query(query="doc",shape="StartMonitoringMembers")`) — and that
//!    monitoring begins with the member account's own acceptance of the
//!    administrator's invitation. Every other account the request names — one the
//!    administrator holds no member record for, one it has only recorded, one whose
//!    invitation that account has not answered, and one a disassociation moved — is
//!    not an error: the call answers HTTP 200 and names the account in
//!    `unprocessedAccounts` with the reason it was not processed, once per DISTINCT
//!    id in request order.
//! 2. A reported account changes NOTHING: its stored relationship, its email and its
//!    `UpdatedAt` are exactly what they were. In particular an outstanding
//!    invitation is answered by the member account and never by this call, so
//!    restarting monitoring does not promote an invited member to a monitored one.
//! 3. The association an invitation established survives a reported start:
//!    `ListMembers` with `onlyAssociated=true` still lists the member, and
//!    `DisassociateMembers` still processes it.
//! 4. A value that cannot be an account id at all, and a `DetectorId` this
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
// account id and the email's shape; this suite restarts monitoring of them.
use crate::member_association::{
    administrator, create_members, disassociate_members, get_members, invite_members, member_of,
    unprocessed_ids, unprocessed_result, MALFORMED_ACCOUNT_IDS, MEMBERS, NON_MEMBER_ACCOUNT_IDS,
    NOT_AN_ASSOCIATED_MEMBER,
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

/// Drive `account_id`'s relationship through the member requests `steps` names, each
/// a path segment under `/detector/{DetectorId}/member`
/// (`smithy_query(operation="InviteMembers,DisassociateMembers")`).
async fn drive_relationship(router: &Router, detector_id: &str, account_id: &str, steps: &[&str]) {
    for step in steps {
        let (status, body) = call(
            router,
            "POST",
            &format!("/detector/{detector_id}/member/{step}"),
            Some(json!({"accountIds": [account_id]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "member/{step} for {account_id}: expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "member/{step} for {account_id}: expected the member processed, got {body:?}"
        );
    }
}

/// Every relationship a member record reaches through the requests this
/// administrator can send, with the requests that drive a freshly recorded account
/// into it.
///
/// None of them is a monitored relationship: GuardDuty monitors a member account
/// under its administrator only once that account has accepted the invitation, which
/// is that account's own call to make.
const RELATIONSHIP_STATUSES: [(&str, &[&str]); 3] = [
    ("Created", &[]),
    ("Invited", &["invite"]),
    ("Removed", &["invite", "disassociate"]),
];

// ---------------------------------------------------------------------------
// Rules 1 and 2: every account the administrator does not monitor is reported,
// and nothing stored for it moves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_member_the_administrator_does_not_monitor_is_reported_and_unchanged() {
    for (relationship, steps) in RELATIONSHIP_STATUSES {
        let case = format!("a member in {relationship}");
        // One store per row: each row drives the SAME account into a different
        // relationship, so it starts from a detector of its own.
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (target, target_email) = MEMBERS[0];
        drive_relationship(&router, &detector_id, target, steps).await;

        // The row's premise: the record really is in the relationship it names.
        let (_, before) = get_members(&router, &detector_id, &[target]).await;
        let stored = member_of(&before, target)
            .unwrap_or_else(|| panic!("case {case}: expected a member record, got {before:?}"));
        assert_eq!(
            stored["relationshipStatus"].as_str(),
            Some(relationship),
            "case {case}: expected the row's relationship, got {stored:?}"
        );

        let (status, body) = start_monitoring_members(&router, &detector_id, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            vec![target.to_string()],
            "case {case}: expected the account reported, got {body:?}"
        );
        assert_eq!(
            unprocessed_result(&body, target).as_deref(),
            Some(NOT_AN_ASSOCIATED_MEMBER),
            "case {case}: expected the not-an-associated-member reason, got {body:?}"
        );

        // Rule 2: the reported account is exactly where it was — an outstanding
        // invitation is not answered on the member account's behalf.
        let (_, after) = get_members(&router, &detector_id, &[target]).await;
        let read = member_of(&after, target)
            .unwrap_or_else(|| panic!("case {case}: expected the record retained, got {after:?}"));
        assert_eq!(
            read, stored,
            "case {case}: expected a reported account to be unchanged, got {read:?} after {stored:?}"
        );
        assert_eq!(
            read["email"].as_str(),
            Some(target_email),
            "case {case}: expected the email to be retained, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_stop_and_a_start_of_an_invited_member_leave_it_invited() {
    // The two monitoring calls are inverses over one stored field, so neither of
    // them can move a relationship the other would refuse: repeated so a sequence of
    // calls cannot walk an unanswered invitation into a monitored relationship.
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();
    let (status, body) = invite_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    let (_, invited) = get_members(&router, &detector_id, &ids).await;

    for cycle in 1..=3 {
        let (status, stopped) = stop_monitoring_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: stop expected 200, got {status} {stopped:?}"
        );
        let (status, started) = start_monitoring_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: start expected 200, got {status} {started:?}"
        );
        for account_id in &ids {
            assert_eq!(
                unprocessed_result(&stopped, account_id).as_deref(),
                Some(NOT_AN_ASSOCIATED_MEMBER),
                "case cycle {cycle} {account_id}: expected the stop to report the member, got {stopped:?}"
            );
            assert_eq!(
                unprocessed_result(&started, account_id).as_deref(),
                Some(NOT_AN_ASSOCIATED_MEMBER),
                "case cycle {cycle} {account_id}: expected the start to report the member, got {started:?}"
            );
        }
        let (_, after) = get_members(&router, &detector_id, &ids).await;
        assert_eq!(
            after, invited,
            "case cycle {cycle}: expected the invitation to stand unanswered, got {after:?} after {invited:?}"
        );
        for account_id in &ids {
            assert_eq!(
                relationship_status(&after, account_id),
                "Invited",
                "case cycle {cycle} {account_id}: expected the member to stay invited, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the association an invitation established survives a reported start.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invited_member_is_still_associated_after_a_reported_start() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();
    let (status, body) = invite_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    let (status, body) = start_monitoring_members(&router, &detector_id, &ids).await;
    assert_eq!(status, 200, "start: expected 200, got {status} {body:?}");

    for only_associated in [true, false] {
        let (status, listed) = list_members(&router, &detector_id, only_associated).await;
        assert_eq!(
            status, 200,
            "case onlyAssociated={only_associated}: expected 200, got {status} {listed:?}"
        );
        for account_id in &ids {
            assert!(
                member_of(&listed, account_id).is_some(),
                "case onlyAssociated={only_associated} {account_id}: expected an invited member to be associated, got {listed:?}"
            );
        }
    }

    // And the association is still one `DisassociateMembers` ends.
    let account_id = MEMBERS[1].0;
    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "disassociate: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case {account_id}: expected an invited member to still be associated, got {body:?}"
    );
    let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        relationship_status(&read, account_id),
        "Removed",
        "case {account_id}: expected the disassociation to move the relationship, got {read:?}"
    );
}

#[tokio::test]
async fn an_account_that_is_not_an_associated_member_is_reported_not_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;

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
            Some(NOT_AN_ASSOCIATED_MEMBER),
            "case {account_id} ({why}): expected the not-an-associated-member reason, got {body:?}"
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
async fn every_account_a_mixed_request_names_is_reported_once_in_request_order() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let member_id = MEMBERS[2].0;
    let invited_id = MEMBERS[1].0;
    let absent_one = NON_MEMBER_ACCOUNT_IDS[0].0;
    let absent_two = NON_MEMBER_ACCOUNT_IDS[1].0;
    drive_relationship(&router, &detector_id, invited_id, &["invite"]).await;

    // A mixed request: none of these accounts is monitored under the administrator,
    // so each is reported once, in request order, and a repeated id is reported once
    // because `AccountIds` names a SET.
    let requested = [
        absent_two,
        member_id,
        absent_one,
        absent_two,
        member_id,
        invited_id,
    ];
    let (status, body) = start_monitoring_members(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed start: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![
            absent_two.to_string(),
            member_id.to_string(),
            absent_one.to_string(),
            invited_id.to_string(),
        ],
        "mixed start: expected each distinct account once, in request order, got {body:?}"
    );

    for (account_id, expected_status) in [(member_id, "Created"), (invited_id, "Invited")] {
        let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            relationship_status(&read, account_id),
            expected_status,
            "case {account_id}: expected the relationship to stand, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: requests the service does not understand are refused 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_cannot_be_an_account_id_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let expected = invalid_parameter("accountId");
    let member_id = MEMBERS[0].0;

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // The whole call is refused, so a well-formed id sent alongside is not
        // reported either.
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
        assert!(
            body["unprocessedAccounts"].is_null(),
            "case {account_id} ({why}): expected no unprocessedAccounts on a refusal, got {body:?}"
        );

        let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
        assert_eq!(
            relationship_status(&read, member_id),
            "Created",
            "case {account_id} ({why}): expected a refused call to restart nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn restarting_monitoring_on_a_detector_this_account_does_not_own_is_refused() {
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
        "Created",
        "case {member_id}: expected a refused call to restart nothing, got {read:?}"
    );
}

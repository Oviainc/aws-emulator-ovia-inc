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
//! 1. Monitoring is stopped only for an account GuardDuty MONITORS under this
//!    administrator, which is an account whose own acceptance started the
//!    monitoring. Every other account the request names — one the administrator
//!    holds no member record for, one it has only recorded, one whose invitation
//!    that account has not answered, and one a disassociation moved — is not an
//!    error: the call answers HTTP 200 and names the account in
//!    `unprocessedAccounts` with the reason it was not processed, once per DISTINCT
//!    id in request order.
//! 2. A reported account changes NOTHING: its stored relationship, its email and its
//!    `UpdatedAt` are exactly what they were, and `ListMembers` still reports the
//!    members it did before — a refused per-account stop writes no relationship
//!    status for the account it refused.
//! 3. A value that cannot be an account id at all, and a `DetectorId` this
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
// `MEMBERS` is the table of accounts to record, differing along the account id and
// the email's shape; this suite stops monitoring for them.
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

/// Drive `account_id`'s relationship through the member requests `steps` names.
///
/// Each step is a path segment under `/detector/{DetectorId}/member`
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
/// GuardDuty monitors a member account under its administrator only once that
/// account has accepted the invitation — "the status of the relationship between the
/// member and the administrator"
/// (`smithy_query(query="doc",shape="Member",member="RelationshipStatus")`) — which
/// is that account's own call to make, so none of these relationships is one this
/// administrator can stop monitoring for.
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

        let (status, body) = stop_monitoring_members(&router, &detector_id, &[target]).await;
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

        // Rule 2: the reported account is exactly where it was — the relationship
        // the call refused to act on is not rewritten.
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
async fn an_account_with_no_member_record_is_reported_not_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;

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
    // so each is reported once, in request order, however it got into the store —
    // and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [
        absent_two,
        member_id,
        absent_one,
        absent_two,
        member_id,
        invited_id,
    ];
    let (status, body) = stop_monitoring_members(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed stop: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![
            absent_two.to_string(),
            member_id.to_string(),
            absent_one.to_string(),
            invited_id.to_string(),
        ],
        "mixed stop: expected each distinct account once, in request order, got {body:?}"
    );

    // The records the request named are where they were, and the listing is too.
    for (account_id, expected_status) in [(member_id, "Created"), (invited_id, "Invited")] {
        let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
        let member = member_of(&read, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {read:?}"));
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(expected_status),
            "case {account_id}: expected the relationship to stand, got {member:?}"
        );
    }
    let (status, listed) = list_members(&router, &detector_id, false).await;
    assert_eq!(
        status, 200,
        "list-members: expected 200, got {status} {listed:?}"
    );
    for account_id in all_ids() {
        assert!(
            member_of(&listed, account_id).is_some(),
            "case {account_id}: expected every record to be listed, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_disassociated_member_is_reported_and_keeps_its_relationship() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let (account_id, email) = MEMBERS[1];

    drive_relationship(&router, &detector_id, account_id, &["invite"]).await;
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
        Some(NOT_AN_ASSOCIATED_MEMBER),
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
async fn an_invited_member_is_still_listed_as_associated_after_a_reported_stop() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();
    let (status, body) = invite_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );

    let (status, body) = stop_monitoring_members(&router, &detector_id, &ids).await;
    assert_eq!(status, 200, "stop: expected 200, got {status} {body:?}");
    assert_eq!(
        unprocessed_ids(&body),
        ids.iter().map(|id| (*id).to_string()).collect::<Vec<String>>(),
        "stop: expected every invited member reported, got {body:?}"
    );

    // The association the invitation established is untouched by the reported stop:
    // the narrowed listing still names every member.
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
}

// ---------------------------------------------------------------------------
// Rule 3: requests the service does not understand are refused 400.
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
        assert!(
            body["unprocessedAccounts"].is_null(),
            "case {account_id} ({why}): expected no unprocessedAccounts on a refusal, got {body:?}"
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

// SPDX-License-Identifier: CC0-1.0

//! `InviteMembers`: sending the invitation that moves a recorded member account's
//! relationship on from `Created`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Inviting moves ONLY the relationship of the named member accounts, to
//!    `Invited`, and records the instant the invitation was sent as the member's
//!    `invitedAt`; everything else about the record is retained, every member the
//!    call did not name is left where it was, and both reads of a member
//!    (`GetMembers` and `ListMembers`) report the same thing.
//! 2. One invitation is pending per administrator/member pair: a second invite of the
//!    same accounts answers 200, reports nothing unprocessed and changes nothing at
//!    all — `invitedAt` and `updatedAt` included.
//! 3. The condition is the member RECORD, not the association: a member a
//!    `DisassociateMembers` moved to `Removed` is invited again by this call, with no
//!    second `CreateMembers`, and is associated again afterwards.
//! 4. An account the administrator holds no member record for is not an error: the
//!    call answers HTTP 200 and names the account in `unprocessedAccounts` with the
//!    reason it was not processed, once per DISTINCT id in request order, and
//!    persists nothing for it.
//! 5. The announcement members (`message`, `disableEmailNotification`) govern the
//!    email the invitation is announced by, which nothing reports: they change
//!    neither the outcome nor what a read reports.
//! 6. A value that cannot be an account id at all, and a `DetectorId` this
//!    account/Region does not own, are each refused with HTTP 400 under the
//!    `InvalidInputException` class — never answered with a hollow 200.

use serde_json::{json, Map, Value};

use crate::client_rejections::{invalid_parameter, not_owned};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
// The message, projection and administrator helpers the member suites share: one
// definition, so two suites cannot drift apart on what a malformed or non-member
// account id is, nor on what a member read looks like. `MEMBERS` is the table of
// accounts to record, differing along the account id and the email's shape; this
// suite invites them.
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

/// `Member.RelationshipStatus` for a recorded, un-invited account.
const CREATED: &str = "Created";

/// `Member.RelationshipStatus` for an account whose invitation is pending.
const INVITED: &str = "Invited";

/// `Member.RelationshipStatus` for a disassociated account.
const REMOVED: &str = "Removed";

/// The announcement members of `InviteMembers`, each with why: `DisableEmailNotification`
/// (a `Boolean`) and `Message` (a `String` carrying no length constraint —
/// `smithy_query(operation="InviteMembers")`), so the rows span absent, both boolean
/// values, and a message that is empty, short and long.
const ANNOUNCEMENTS: [(Option<bool>, Option<&str>, &str); 5] = [
    (None, None, "neither announcement member"),
    (Some(true), None, "email notification disabled, no message"),
    (
        Some(false),
        Some("Please join our GuardDuty organization."),
        "email notification left on, with a message",
    ),
    (None, Some(""), "an empty message"),
    (
        Some(true),
        Some(
            "A long invitation message, carrying punctuation, digits (0123456789) and \
             enough text to be more than one line of body JSON.",
        ),
        "a long message",
    ),
];

/// `InviteMembers` over the wire, carrying whichever announcement members the row has.
async fn invite_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
    disable_email_notification: Option<bool>,
    message: Option<&str>,
) -> (u16, Value) {
    let mut body = Map::new();
    body.insert("accountIds".to_string(), json!(account_ids));
    if let Some(disable) = disable_email_notification {
        body.insert("disableEmailNotification".to_string(), json!(disable));
    }
    if let Some(message) = message {
        body.insert("message".to_string(), json!(message));
    }
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/invite"),
        Some(Value::Object(body)),
    )
    .await
}

/// `InviteMembers` with no announcement members, for the rules that do not vary along
/// them.
async fn invite(router: &Router, detector_id: &str, account_ids: &[&str]) -> (u16, Value) {
    invite_members(router, detector_id, account_ids, None, None).await
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

/// Is `value` an ISO-8601 millisecond instant, the form every GuardDuty timestamp
/// member on this service takes?
fn is_instant(value: Option<&str>) -> bool {
    value.is_some_and(|at| at.len() == 24 && at.contains('T') && at.ends_with('Z'))
}

// ---------------------------------------------------------------------------
// Rules 1 and 5: invite -> read, over a subset and over the announcement members.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inviting_moves_only_the_named_members_to_invited_and_records_when() {
    let administrator = administrator();
    let ids = all_ids();
    // A strict, NON-CONTIGUOUS subset, so the call is observable both ways and the
    // invited members are not merely a prefix of the stored order.
    let invited = [MEMBERS[0].0, MEMBERS[2].0];

    for (disable_email, message, why) in ANNOUNCEMENTS {
        // A fresh engine per row: there is only one detector per account per Region,
        // so each row gets its own emulator rather than a second detector.
        let router = new_router();
        let detector_id = detector_with_members(&router).await;

        let (status, created) = get_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case {why}: get-members after create expected 200, got {status} {created:?}"
        );

        let (status, body) = invite_members(
            &router,
            &detector_id,
            &invited,
            disable_email,
            message,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why}: invite-members expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {why}: expected every recorded member processed, got {body:?}"
        );

        let (status, after) = get_members(&router, &detector_id, &ids).await;
        assert_eq!(
            status, 200,
            "case {why}: get-members after invite expected 200, got {status} {after:?}"
        );
        for (account_id, email) in MEMBERS {
            let was_invited = invited.contains(&account_id);
            let expected_status = if was_invited { INVITED } else { CREATED };
            let member = member_of(&after, account_id).unwrap_or_else(|| {
                panic!("case {why} {account_id}: expected the record to be retained, got {after:?}")
            });
            assert_eq!(
                member["relationshipStatus"].as_str(),
                Some(expected_status),
                "case {why} {account_id}: expected relationshipStatus {expected_status}, got {member:?}"
            );
            // The instant the invitation was sent is recorded for an invited member
            // and invented for nobody else.
            if was_invited {
                assert!(
                    is_instant(member["invitedAt"].as_str()),
                    "case {why} {account_id}: expected an ISO-8601 invitedAt, got {member:?}"
                );
                // One clock for the change: the invitation was sent at the instant the
                // relationship moved.
                assert_eq!(
                    member["invitedAt"], member["updatedAt"],
                    "case {why} {account_id}: expected invitedAt to be the instant the relationship moved, got {member:?}"
                );
            } else {
                assert!(
                    member.get("invitedAt").is_none(),
                    "case {why} {account_id}: expected no invitedAt for an un-invited member, got {member:?}"
                );
            }
            // Only the relationship moves: the details are RETAINED.
            assert_eq!(
                member["email"].as_str(),
                Some(email),
                "case {why} {account_id}: expected the email to be retained, got {member:?}"
            );
            assert_eq!(
                member["masterId"].as_str(),
                Some(administrator.as_str()),
                "case {why} {account_id}: expected masterId to be the administrator, got {member:?}"
            );
            assert_eq!(
                member["administratorId"].as_str(),
                Some(administrator.as_str()),
                "case {why} {account_id}: expected administratorId to be the administrator, got {member:?}"
            );

            let before = member_of(&created, account_id).unwrap_or_else(|| {
                panic!("case {why} {account_id}: expected a member, got {created:?}")
            });
            let moved = member["updatedAt"] != before["updatedAt"];
            assert_eq!(
                moved,
                was_invited,
                "case {why} {account_id}: expected updatedAt to move only for an invited member, got {:?} then {:?}",
                before["updatedAt"],
                member["updatedAt"]
            );

            // Two reads of one member must not disagree, and a read must not change
            // what a later caller sees.
            let (_, listed) = list_members(&router, &detector_id, false).await;
            let from_list = member_of(&listed, account_id).unwrap_or_else(|| {
                panic!("case {why} {account_id}: expected the listing to report the member, got {listed:?}")
            });
            assert_eq!(
                from_list, member,
                "case {why} {account_id}: expected both reads to agree, got {from_list:?} and {member:?}"
            );
        }

        // An invited member is ASSOCIATED with the administrator: the narrowed
        // listing still names every member.
        for only_associated in [true, false] {
            let (status, listed) = list_members(&router, &detector_id, only_associated).await;
            assert_eq!(
                status, 200,
                "case {why} onlyAssociated={only_associated}: expected 200, got {status} {listed:?}"
            );
            for account_id in &ids {
                assert!(
                    member_of(&listed, account_id).is_some(),
                    "case {why} onlyAssociated={only_associated} {account_id}: expected an invited member to be associated, got {listed:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_announcement_members_do_not_change_what_a_read_reports() {
    let account_id = MEMBERS[1].0;
    let mut projections: Vec<(&str, Value)> = Vec::new();

    for (disable_email, message, why) in ANNOUNCEMENTS {
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (status, body) = invite_members(
            &router,
            &detector_id,
            &[account_id],
            disable_email,
            message,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {why}: expected 200, got {status} {body:?}"
        );
        let (_, read) = get_members(&router, &detector_id, &[account_id]).await;
        let mut member = member_of(&read, account_id)
            .unwrap_or_else(|| panic!("case {why}: expected a member, got {read:?}"));
        // The instants differ between rows because each row is its own emulator and
        // its own call; everything else must be identical.
        if let Some(object) = member.as_object_mut() {
            object.remove("invitedAt");
            object.remove("updatedAt");
        }
        projections.push((why, member));
    }

    let (first_why, first) = projections[0].clone();
    for (why, projection) in &projections[1..] {
        assert_eq!(
            projection, &first,
            "case {why}: expected the same member as case {first_why}, got {projection:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: one pending invitation per pair, so a repeat changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inviting_again_changes_nothing() {
    let ids = all_ids();
    // Rows differing along the COUNT of accounts one call names: one, a few, and
    // every member the administrator holds.
    let batches: [&[&str]; 3] = [&ids[..1], &ids[..2], &ids[..]];

    for batch in batches {
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (status, first) = invite(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: first invite expected 200, got {status} {first:?}"
        );
        let (_, once) = get_members(&router, &detector_id, batch).await;
        // The first invite must have DONE something, or the comparison below would
        // pass over two unchanged reads.
        for account_id in batch {
            let member = member_of(&once, account_id).unwrap_or_else(|| {
                panic!("case {batch:?} {account_id}: expected a member, got {once:?}")
            });
            assert_eq!(
                member["relationshipStatus"].as_str(),
                Some(INVITED),
                "case {batch:?} {account_id}: expected the first invite to send an invitation, got {member:?}"
            );
        }

        let (status, second) = invite(&router, &detector_id, batch).await;
        assert_eq!(
            status, 200,
            "case {batch:?}: second invite expected 200, got {status} {second:?}"
        );
        assert!(
            unprocessed_ids(&second).is_empty(),
            "case {batch:?}: expected an invited member to still be processed, got {second:?}"
        );
        let (_, twice) = get_members(&router, &detector_id, batch).await;
        assert_eq!(
            twice, once,
            "case {batch:?}: expected the second invite to change nothing, got {twice:?} after {once:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a retained record is invited again without being re-created.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_disassociated_member_is_invited_again_without_being_recreated() {
    for (account_id, email) in MEMBERS {
        let router = new_router();
        let detector_id = detector_with_members(&router).await;

        let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id}: disassociate expected 200, got {status} {body:?}"
        );
        let (_, removed) = get_members(&router, &detector_id, &[account_id]).await;
        let before = member_of(&removed, account_id).unwrap_or_else(|| {
            panic!("case {account_id}: expected a retained record, got {removed:?}")
        });
        assert_eq!(
            before["relationshipStatus"].as_str(),
            Some(REMOVED),
            "case {account_id}: expected the disassociation to move the relationship, got {before:?}"
        );

        let (status, body) = invite(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id}: invite expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {account_id}: expected a retained record to be invitable, got {body:?}"
        );

        let (_, after) = get_members(&router, &detector_id, &[account_id]).await;
        let member = member_of(&after, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {after:?}"));
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(INVITED),
            "case {account_id}: expected the invitation to re-associate the member, got {member:?}"
        );
        assert!(
            is_instant(member["invitedAt"].as_str()),
            "case {account_id}: expected an ISO-8601 invitedAt, got {member:?}"
        );
        assert_eq!(
            member["email"].as_str(),
            Some(email),
            "case {account_id}: expected the retained email, got {member:?}"
        );

        // Associated again: the narrowed listing names it.
        let (_, listed) = list_members(&router, &detector_id, true).await;
        assert!(
            member_of(&listed, account_id).is_some(),
            "case {account_id}: expected an invited member to be associated again, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an account with no member record is REPORTED, not refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_with_no_member_record_is_reported_not_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let reason = not_a_member();

    // The administrator's own account is never one of its own members, so it is a row
    // of this table beside the ids nothing recorded.
    let administrator = administrator();
    let mut rows: Vec<(&str, &str)> = NON_MEMBER_ACCOUNT_IDS.to_vec();
    rows.push((administrator.as_str(), "the administrator's own account"));

    for (account_id, why) in rows {
        let (status, body) = invite(&router, &detector_id, &[account_id]).await;
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
async fn only_the_accounts_with_no_record_are_reported_once_in_request_order() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let member_id = MEMBERS[2].0;
    let absent_one = NON_MEMBER_ACCOUNT_IDS[0].0;
    let absent_two = NON_MEMBER_ACCOUNT_IDS[1].0;

    // A mixed request: the member is invited, the rest reported, in request order,
    // and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [absent_two, member_id, absent_one, absent_two, member_id];
    let (status, body) = invite(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed invite: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed invite: expected each absent account once, in request order, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    let member = member_of(&read, member_id)
        .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some(INVITED),
        "case {member_id}: expected the member in a mixed request to be invited, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: requests the service does not understand are refused 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_cannot_be_an_account_id_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let expected = invalid_parameter("accountId");
    let member_id = MEMBERS[0].0;

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // The whole call is refused, so a well-formed id sent alongside is not
        // invited either.
        let (status, body) = invite(&router, &detector_id, &[member_id, account_id]).await;
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
            Some(CREATED),
            "case {account_id} ({why}): expected a refused call to invite nothing, got {member:?}"
        );
        assert!(
            member.get("invitedAt").is_none(),
            "case {account_id} ({why}): expected a refused call to record no invitation, got {member:?}"
        );
    }
}

#[tokio::test]
async fn inviting_through_a_detector_this_account_does_not_own_is_refused() {
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
        let (status, body) = invite(&router, candidate, &[member_id]).await;
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
        Some(CREATED),
        "case {member_id}: expected a refused call to invite nothing, got {member:?}"
    );
}

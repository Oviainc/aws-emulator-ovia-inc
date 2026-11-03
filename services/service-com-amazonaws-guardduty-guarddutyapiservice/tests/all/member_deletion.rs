// SPDX-License-Identifier: CC0-1.0

//! `DeleteMembers`: dropping the member records a GuardDuty administrator holds.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. Deleting REMOVES the stored member details a disassociation retains — "to
//!    remove the details associated with a member account, the delegated
//!    administrator must invoke the DeleteMembers API"
//!    (`smithy_query(query="doc",shape="DisassociateMembers")`). What it drops is a
//!    record whose relationship is NOT an association, so a record is dropped in
//!    every relationship a member reaches here, `Removed` — the state a
//!    disassociation RETAINS the record in — included, and afterwards neither read of
//!    a member (`GetMembers`, `ListMembers`) reports the account at all. Every member
//!    the call did not name is left exactly where it was.
//! 2. Deleting is idempotent in the only sense a delete can be: once the record is
//!    gone there is no member record to drop, so a second delete of the same id
//!    answers HTTP 200 and reports it in `unprocessedAccounts` with the reason no
//!    record was dropped, once per DISTINCT id in request order, and persists
//!    nothing for it.
//! 3. The record is GONE, not emptied: re-recording the same account id with
//!    `CreateMembers` yields a fresh record — the new email, `Created` again and no
//!    `invitedAt` — rather than the deleted one's details.
//! 4. While the named detector's stored organization configuration auto-enables
//!    GuardDuty for EVERY account of the organization, the whole call is refused with
//!    HTTP 400 under the `InvalidInputException` class and no record is dropped: the
//!    organization owns the relationship, so the account has to leave it first.
//! 5. A value that cannot be a `DetectorId` or an account id at all, and a
//!    well-formed `DetectorId` this account/Region does not own, are each refused
//!    with HTTP 400 under the `InvalidInputException` class — never answered with a
//!    hollow 200, and never deleting anything.
//! 6. Members hang off the detector that administers them, so a delete is scoped to
//!    the named detector in the Region the request was signed for: one
//!    administrator's delete leaves another's records alone.

use serde_json::{json, Value};

use crate::client_rejections::{
    account_ids_format, invalid_parameter, not_owned, MALFORMED_DETECTOR_IDS,
};
use crate::detector_lifecycle::{
    call, call_in, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::invitation_listing::enabled_detector_in;
// The member-call harness, the shared messages and the account-id tables live once,
// in the member-association suite: reusing them is what keeps two member suites from
// drifting apart on what a member read looks like or what a malformed account id is.
use crate::member_association::{
    administrator, create_members, create_members_in, get_members, get_members_in, member_of,
    unprocessed_ids, unprocessed_result, MALFORMED_ACCOUNT_IDS, MEMBERS, MEMBER_NOT_DELETED,
    NON_MEMBER_ACCOUNT_IDS, NOT_AN_ASSOCIATED_MEMBER,
};
// The organization precondition's own table and message live once, in the suite that
// owns that rule for the operations it already covered.
use crate::organization_configuration::update_ok_in;
use crate::organization_managed_members::{organization_managed, AUTO_ENABLE_MEMBERS};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// The action the organization refusal of this operation names, read from the handler
/// so the test and the service cannot drift apart.
const DELETE_ACTION: &str =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::DELETE_MEMBER_ACTION;

/// The Region these cases run in: the one the member suites call through.
const REGION: &str = crate::member_association::DEFAULT_REGION;

/// A second Region, which is how a second administrator's detector is reached:
/// GuardDuty holds one detector per account per Region.
const OTHER_REGION: &str = "us-west-2";

/// Every `relationshipStatus` a stored member record reaches through the requests
/// this administrator can send, with the member requests that drive a freshly
/// recorded account into it — each a path segment under
/// `/detector/{DetectorId}/member` (`smithy_query(operation="InviteMembers")` and the
/// disassociation operation beside it).
///
/// The rule under test generalises over the relationship, so the rows span it: a
/// delete drops the record in every one of these states, including `Removed`, which
/// is the state a disassociation RETAINS the record in. `Enabled` and `Disabled` are
/// not rows: GuardDuty reaches them only once the member ACCOUNT has accepted the
/// administrator's invitation, which is that account's call to make and not one this
/// administrator can send.
const RELATIONSHIP_STATUSES: [(&str, &[&str]); 3] = [
    ("Created", &[]),
    ("Invited", &["invite"]),
    ("Removed", &["invite", "disassociate"]),
];

/// `DeleteMembers` through `region`'s endpoint.
async fn delete_members_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/delete"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// `DeleteMembers` through the default Region's endpoint, which is the one
/// [`create_members`] and [`get_members`] call through.
async fn delete_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    delete_members_in(router, REGION, detector_id, account_ids).await
}

/// `ListMembers` over every member the detector holds, associated or not.
async fn list_members(router: &Router, region: &str, detector_id: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
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

/// The account ids `ListMembers` reports, in the order it lists them.
async fn listed_ids(router: &Router, region: &str, detector_id: &str) -> Vec<String> {
    let body = list_members(router, region, detector_id).await;
    body["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| {
            member["accountId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Create an enabled detector in `region` and record [`MEMBERS`] against it,
/// returning the detector id.
async fn detector_with_members_in(router: &Router, region: &str) -> String {
    let detector_id = enabled_detector_in(router, region).await;
    let (status, body) = create_members_in(router, region, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members in {region}: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "create-members in {region}: expected every account processed, got {body:?}"
    );
    detector_id
}

/// [`detector_with_members_in`] in the Region the member suites call through.
async fn detector_with_members(router: &Router) -> String {
    detector_with_members_in(router, REGION).await
}

/// Every recorded account id, in the order [`MEMBERS`] lists them.
fn all_ids() -> Vec<&'static str> {
    MEMBERS.iter().map(|(id, _)| *id).collect()
}

/// Drive `account_id`'s relationship through the member requests `steps` names, each
/// of which must process the account.
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

/// Assert that `GetMembers` reports no record at all for `account_id`: it is named in
/// `unprocessedAccounts` with the not-a-member reason and carries no `members` entry.
async fn assert_no_record(router: &Router, region: &str, detector_id: &str, case: &str, account_id: &str) {
    let (status, body) = get_members_in(router, region, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {case}: expected get-members to answer 200, got {status} {body:?}"
    );
    assert!(
        member_of(&body, account_id).is_none(),
        "case {case}: expected no member record for {account_id}, got {body:?}"
    );
    assert_eq!(
        unprocessed_result(&body, account_id),
        Some(NOT_AN_ASSOCIATED_MEMBER.to_string()),
        "case {case}: expected {account_id} reported as no member, got {body:?}"
    );
    let listed = listed_ids(router, region, detector_id).await;
    assert!(
        !listed.iter().any(|id| id == account_id),
        "case {case}: expected list-members to omit {account_id}, got {listed:?}"
    );
}

/// Assert that `account_id` still reads back as a member with `email` and
/// `relationship_status`.
async fn assert_record(
    router: &Router,
    region: &str,
    detector_id: &str,
    case: &str,
    account_id: &str,
    email: &str,
    relationship_status: &str,
) {
    let (status, body) = get_members_in(router, region, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {case}: expected get-members to answer 200, got {status} {body:?}"
    );
    let member = member_of(&body, account_id).unwrap_or_else(|| {
        panic!("case {case}: expected a member record for {account_id}, got {body:?}")
    });
    assert_eq!(
        member["email"].as_str(),
        Some(email),
        "case {case}: expected {account_id} to keep its email, got {member:?}"
    );
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some(relationship_status),
        "case {case}: expected {account_id} to carry relationshipStatus {relationship_status}, got {member:?}"
    );
}

/// Assert that every account in [`MEMBERS`] still reads back with its recorded email
/// and `Created` relationship — nothing was deleted.
async fn assert_every_member_retained(router: &Router, region: &str, detector_id: &str, case: &str) {
    for (account_id, email) in MEMBERS {
        assert_record(
            router,
            region,
            detector_id,
            case,
            account_id,
            email,
            "Created",
        )
        .await;
    }
    let listed = listed_ids(router, region, detector_id).await;
    let expected: Vec<String> = all_ids().iter().map(|id| (*id).to_string()).collect();
    assert_eq!(
        listed, expected,
        "case {case}: expected list-members to report every recorded member, got {listed:?}"
    );
}

/// The `message` an error response carries.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Assert one client rejection: the status, the error class, the error shape and the
/// classified message.
fn assert_rejection(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case}: expected the error class {CLASS}, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case}: expected the error shape {ENVELOPE}, got {body:?}"
    );
    assert_eq!(
        message(body),
        expected_message,
        "case {case}: expected the classified message, got {body:?}"
    );
    assert!(
        body["unprocessedAccounts"].is_null(),
        "case {case}: expected no unprocessedAccounts on a refusal, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the record is dropped whatever relationship it carries, and only the
// named accounts are touched.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_drops_the_record_whatever_relationship_status_it_carries() {
    for (relationship, steps) in RELATIONSHIP_STATUSES {
        let case = format!("a member in {relationship}");
        // One store per row: each row drives the SAME account into a different
        // relationship, so it starts from a detector of its own.
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        let (target, target_email) = MEMBERS[0];
        drive_relationship(&router, &detector_id, target, steps).await;

        // The row's premise: the record really is in the relationship it names.
        assert_record(
            &router,
            REGION,
            &detector_id,
            &case,
            target,
            target_email,
            relationship,
        )
        .await;

        let (status, body) = delete_members(&router, &detector_id, &[target]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected the member processed, got {body:?}"
        );

        // The details are GONE: neither read reports the account any more.
        assert_no_record(&router, REGION, &detector_id, &case, target).await;

        // And every member the call did not name is exactly where it was.
        for (account_id, email) in MEMBERS.iter().skip(1) {
            assert_record(
                &router,
                REGION,
                &detector_id,
                &case,
                account_id,
                email,
                "Created",
            )
            .await;
        }
        let listed = listed_ids(&router, REGION, &detector_id).await;
        let expected: Vec<String> = MEMBERS
            .iter()
            .skip(1)
            .map(|(id, _)| (*id).to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case}: expected list-members to report only the members left, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_every_member_leaves_an_empty_member_list() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();

    let (status, body) = delete_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "delete-every-member: expected HTTP 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "delete-every-member: expected every member processed, got {body:?}"
    );

    let listed = listed_ids(&router, REGION, &detector_id).await;
    assert!(
        listed.is_empty(),
        "delete-every-member: expected an empty member list, got {listed:?}"
    );
    for (account_id, _) in MEMBERS {
        assert_no_record(
            &router,
            REGION,
            &detector_id,
            "delete-every-member",
            account_id,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an account with no record is reported, once per distinct id, in
// request order.
// ---------------------------------------------------------------------------

/// One row of the unknown-account table: the case name, the `accountIds` the request
/// carries, the ids the response must report as unprocessed in order, and the members
/// the call deletes.
type UnknownAccountCase = (&'static str, Vec<&'static str>, Vec<&'static str>, Vec<&'static str>);

#[tokio::test]
async fn an_account_with_no_record_is_reported_rather_than_refused() {
    let first_member = MEMBERS[0].0;
    let second_member = MEMBERS[1].0;
    let non_members: Vec<&str> = NON_MEMBER_ACCOUNT_IDS.iter().map(|(id, _)| *id).collect();
    // Request shapes differing along the count, the repetition and whether the named
    // account is a member of this administrator at all. Each row carries what the
    // response must report, in order, and which members the call deletes.
    let cases: [UnknownAccountCase; 6] = [
        (
            "a single account with no record",
            vec![non_members[0]],
            vec![non_members[0]],
            vec![],
        ),
        (
            "every account with no record",
            non_members.clone(),
            non_members.clone(),
            vec![],
        ),
        (
            "one account with no record named three times",
            vec![non_members[1], non_members[1], non_members[1]],
            vec![non_members[1]],
            vec![],
        ),
        (
            "a member and an account with no record",
            vec![first_member, non_members[2]],
            vec![non_members[2]],
            vec![first_member],
        ),
        (
            "an account with no record before two members",
            vec![non_members[3], first_member, second_member],
            vec![non_members[3]],
            vec![first_member, second_member],
        ),
        (
            "two accounts with no record, interleaved with a member",
            vec![non_members[0], first_member, non_members[1]],
            vec![non_members[0], non_members[1]],
            vec![first_member],
        ),
    ];

    for (case, request, expected_unprocessed, deleted) in cases {
        let router = new_router();
        let detector_id = detector_with_members(&router).await;

        let (status, body) = delete_members(&router, &detector_id, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            expected_unprocessed
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected each distinct unknown account reported once, in request order, got {body:?}"
        );
        for account_id in &expected_unprocessed {
            assert_eq!(
                unprocessed_result(&body, account_id),
                Some(MEMBER_NOT_DELETED.to_string()),
                "case {case}: expected the undeletable-member reason for {account_id}, got {body:?}"
            );
        }

        // Nothing is persisted for an account with no record, and exactly the
        // members the request named are gone.
        let listed = listed_ids(&router, REGION, &detector_id).await;
        let expected: Vec<String> = all_ids()
            .iter()
            .filter(|id| !deleted.contains(id))
            .map(|id| (*id).to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {case}: expected only the named members deleted, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_second_delete_of_the_same_accounts_reports_them_as_no_member() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let ids = all_ids();

    let (status, first) = delete_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "first delete: expected HTTP 200, got {status} {first:?}"
    );
    assert!(
        unprocessed_ids(&first).is_empty(),
        "first delete: expected every member processed, got {first:?}"
    );

    // The record is gone, so the SAME request now has nothing to delete: every id is
    // reported, in request order, and the answer is still 200.
    let (status, second) = delete_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "second delete: expected HTTP 200, got {status} {second:?}"
    );
    assert_eq!(
        unprocessed_ids(&second),
        ids.iter().map(|id| (*id).to_string()).collect::<Vec<String>>(),
        "second delete: expected every account reported, got {second:?}"
    );
    for account_id in &ids {
        assert_eq!(
            unprocessed_result(&second, account_id),
            Some(MEMBER_NOT_DELETED.to_string()),
            "second delete: expected the undeletable-member reason for {account_id}, got {second:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the record is gone, so re-recording the account starts afresh.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn re_recording_a_deleted_account_yields_a_fresh_record() {
    // A second email per account, differing from the recorded one, so a stale record
    // would be observable: the rows span the same email shapes [`MEMBERS`] does.
    const REPLACEMENT_EMAILS: [&str; 3] = [
        "member-one-again@example-corp.net",
        "second.address@example-corp.org",
        "m2+tag@example.io",
    ];

    for (index, (account_id, original_email)) in MEMBERS.iter().enumerate() {
        let case = format!("re-recording {account_id}");
        let router = new_router();
        let detector_id = detector_with_members(&router).await;

        // Invite first, so the deleted record carries an `invitedAt` a retained one
        // would still report.
        drive_relationship(&router, &detector_id, account_id, &["invite"]).await;
        let (_, invited) = get_members(&router, &detector_id, &[account_id]).await;
        let member = member_of(&invited, account_id).unwrap_or_else(|| {
            panic!("case {case}: expected a member record, got {invited:?}")
        });
        assert!(
            member["invitedAt"].as_str().is_some(),
            "case {case}: expected an invitedAt before the delete, got {member:?}"
        );

        let (status, body) = delete_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the delete to answer 200, got {status} {body:?}"
        );

        let replacement = REPLACEMENT_EMAILS[index];
        let (status, body) =
            create_members(&router, &detector_id, &[(account_id, replacement)]).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the re-create to answer 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected the account recorded again, got {body:?}"
        );

        let (_, after) = get_members(&router, &detector_id, &[account_id]).await;
        let member = member_of(&after, account_id).unwrap_or_else(|| {
            panic!("case {case}: expected a member record again, got {after:?}")
        });
        assert_eq!(
            member["email"].as_str(),
            Some(replacement),
            "case {case}: expected the new email rather than {original_email}, got {member:?}"
        );
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some("Created"),
            "case {case}: expected a fresh record to be Created, got {member:?}"
        );
        assert!(
            member["invitedAt"].is_null(),
            "case {case}: expected the deleted record's invitedAt to be gone, got {member:?}"
        );
        assert_eq!(
            member["masterId"].as_str(),
            Some(administrator().as_str()),
            "case {case}: expected masterId to be the administrator, got {member:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the organization precondition.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_an_organization_that_auto_enables_every_account_refuses_the_delete() {
    for (value, managed) in AUTO_ENABLE_MEMBERS {
        let case = format!("delete with autoEnableOrganizationMembers {value}");
        let router = new_router();
        let detector_id = detector_with_members(&router).await;
        update_ok_in(
            &router,
            REGION,
            &detector_id,
            json!({"autoEnableOrganizationMembers": value}),
        )
        .await;
        let ids = all_ids();

        let (status, body) = delete_members(&router, &detector_id, &ids).await;
        if managed {
            assert_rejection(&case, status, &body, &organization_managed(DELETE_ACTION));
            // Refused before anything is written: every record is still there.
            assert_every_member_retained(&router, REGION, &detector_id, &case).await;
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case {case}: expected every member processed, got {body:?}"
        );
        let listed = listed_ids(&router, REGION, &detector_id).await;
        assert!(
            listed.is_empty(),
            "case {case}: expected every record dropped, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_organization_refusal_holds_whatever_accounts_the_request_names() {
    let first_member = MEMBERS[0].0;
    let non_member = NON_MEMBER_ACCOUNT_IDS[0].0;
    let requests: [(&str, Vec<&str>); 5] = [
        ("every recorded member", all_ids()),
        ("a single member", vec![first_member]),
        ("one member named twice", vec![first_member, first_member]),
        ("an account that is no member", vec![non_member]),
        (
            "a member and a non-member together",
            vec![first_member, non_member],
        ),
    ];

    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    update_ok_in(
        &router,
        REGION,
        &detector_id,
        json!({"autoEnableOrganizationMembers": "ALL"}),
    )
    .await;

    for (shape, account_ids) in &requests {
        let case = format!("delete naming {shape}");
        let (status, body) = delete_members(&router, &detector_id, account_ids).await;
        assert_rejection(&case, status, &body, &organization_managed(DELETE_ACTION));
        assert_every_member_retained(&router, REGION, &detector_id, &case).await;
    }

    // Lifting the configuration lets the SAME call through, which is what makes the
    // refusal the organization's and not the request's.
    update_ok_in(
        &router,
        REGION,
        &detector_id,
        json!({"autoEnableOrganizationMembers": "NONE"}),
    )
    .await;
    let ids = all_ids();
    let (status, body) = delete_members(&router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "after the auto-enablement was lifted: expected HTTP 200, got {status} {body:?}"
    );
    let listed = listed_ids(&router, REGION, &detector_id).await;
    assert!(
        listed.is_empty(),
        "after the auto-enablement was lifted: expected every record dropped, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a request the service does not understand is refused as such, and
// deletes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_the_service_does_not_understand_is_refused_and_deletes_nothing() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;

    // A value that cannot be an account id at all is a parameter failure, not an
    // unprocessed account: the whole call is refused.
    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        let case = format!("an account id that is {why}");
        let (status, body) = delete_members(&router, &detector_id, &[account_id]).await;
        assert_rejection(&case, status, &body, &account_ids_format("accountIds"));
        assert_every_member_retained(&router, REGION, &detector_id, &case).await;
    }

    // A malformed account id refuses the call even when every other id in the list is
    // a member: the parameter is checked before anything is looked up or written.
    let case = "a member beside a malformed account id";
    let (status, body) = delete_members(
        &router,
        &detector_id,
        &[MEMBERS[0].0, MALFORMED_ACCOUNT_IDS[0].0],
    )
    .await;
    assert_rejection(case, status, &body, &account_ids_format("accountIds"));
    assert_every_member_retained(&router, REGION, &detector_id, case).await;

    // A value that cannot be a detector id at all is the same kind of failure.
    for (candidate, why) in MALFORMED_DETECTOR_IDS {
        let case = format!("a detector id that is {why}");
        let (status, body) = delete_members(&router, candidate, &[MEMBERS[0].0]).await;
        assert_rejection(&case, status, &body, &invalid_parameter("detectorId"));
    }

    // A well-formed detector id this account/Region does not own is an OWNERSHIP
    // failure instead.
    let case = "a detector this account does not own";
    let (status, body) = delete_members(&router, ABSENT_DETECTOR_ID, &[MEMBERS[0].0]).await;
    assert_rejection(case, status, &body, &not_owned("detectorId"));
    assert_every_member_retained(&router, REGION, &detector_id, case).await;
}

// ---------------------------------------------------------------------------
// Rule 6: members hang off the named detector, in the Region the request was
// signed for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_is_scoped_to_the_named_detector_in_its_own_region() {
    let router = new_router();
    let here = detector_with_members_in(&router, REGION).await;
    let there = detector_with_members_in(&router, OTHER_REGION).await;
    let ids = all_ids();

    let (status, body) = delete_members_in(&router, REGION, &here, &ids).await;
    assert_eq!(
        status, 200,
        "delete in {REGION}: expected HTTP 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "delete in {REGION}: expected every member processed, got {body:?}"
    );

    // The other administrator's records are untouched: the delete named one detector.
    assert_every_member_retained(&router, OTHER_REGION, &there, "the other Region").await;
    let listed = listed_ids(&router, REGION, &here).await;
    assert!(
        listed.is_empty(),
        "delete in {REGION}: expected every record dropped there, got {listed:?}"
    );

    // And the detector is reachable only through its OWN Region's endpoint: the same
    // id, signed for another Region, is not owned there.
    let case = "the detector id through another Region's endpoint";
    let (status, body) = delete_members_in(&router, OTHER_REGION, &here, &ids).await;
    assert_rejection(case, status, &body, &not_owned("detectorId"));
    assert_every_member_retained(&router, OTHER_REGION, &there, case).await;
}

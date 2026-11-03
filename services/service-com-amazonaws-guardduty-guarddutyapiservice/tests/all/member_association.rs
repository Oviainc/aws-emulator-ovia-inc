// SPDX-License-Identifier: CC0-1.0

//! The GuardDuty member-association loop: `CreateMembers` → `GetMembers` →
//! `DisassociateMembers` → `GetMembers`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. An account the administrator is not ASSOCIATED with — one it holds no member
//!    record for, one it has recorded and not invited yet, and one a disassociation
//!    already moved — is not an error: the call answers HTTP 200 and names the
//!    account in `unprocessedAccounts` with the reason it was not processed — once
//!    per DISTINCT id, in request order — and persists nothing for it.
//! 2. An invitation is what associates a recorded account with its administrator,
//!    and disassociating a member moves only the RELATIONSHIP: the record is
//!    retained, so `GetMembers` still reports the account and its email with
//!    `relationshipStatus` `Removed`. A second disassociation changes nothing, and
//!    re-creating the account records it afresh.
//! 3. A value that cannot be an account id at all, an email address breaking one of
//!    the rules `AccountDetail.Email` documents, and a `DetectorId` this
//!    account/Region does not own are each refused with HTTP 400 under the
//!    `InvalidInputException` class — never answered with a hollow 200.
//! 4. Members hang off the detector that administers them: they are scoped to that
//!    detector's Region and are dropped when the detector is, and the store is
//!    bounded by GuardDuty's member quota — an account past it is reported, not
//!    recorded.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// Member accounts to record: 12 decimal digits each, which is the form `AccountId`
/// takes (`len 12..12` — `smithy_query(shape="AccountId")`). The rows differ along
/// the account id and along the email's shape, which is all `CreateMembers` stores.
///
/// Every address is in a domain that can RECEIVE mail
/// (`state::email_domain_receives_mail`), so a member of this fixture is one whose
/// invitation is announced and awaits an answer; the addresses whose domain can
/// never receive it are the `member_invitation` suite's own rows.
pub(crate) const MEMBERS: [(&str, &str); 3] = [
    ("111122223333", "member-one@example-corp.com"),
    ("222233334444", "first.last@sub.example-corp.com"),
    ("333344445555", "m+tag@example.io"),
];

/// Well-formed account ids no `CreateMembers` in a test records, differing along the
/// digit space: the low and high ends, an id one digit away from a recorded member,
/// and a plausible mid-range id.
pub(crate) const NON_MEMBER_ACCOUNT_IDS: [(&str, &str); 4] = [
    ("000000000000", "the low end of the account-id space"),
    ("999999999999", "the high end of the account-id space"),
    ("111122223334", "one digit away from a recorded member"),
    ("424242424242", "a mid-range account id"),
];

/// Values that cannot be an `AccountId` at all, each with why.
///
/// An account id is 12 decimal digits; the model constrains the LENGTH only, so
/// every row below is exactly 12 characters and reaches the handler instead of being
/// refused at the boundary.
pub(crate) const MALFORMED_ACCOUNT_IDS: [(&str, &str); 5] = [
    ("abcdefghijkl", "12 letters: not digits at all"),
    ("12345678901a", "11 digits and a letter"),
    ("1234 6789012", "12 characters, one of them a space"),
    ("+11122223333", "a leading plus sign"),
    ("1111-2222-33", "grouped with hyphens"),
];

/// The administrator account every member recorded by these tests belongs to: the
/// account this emulator serves.
pub(crate) fn administrator() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The reason the member operations report an account that is not an ASSOCIATED
/// member of the current account with — one with no member record, one recorded and
/// not invited yet, one whose invitation the account has not answered, and one a
/// disassociation moved.
///
/// GuardDuty's wording is FIXED text: it names the administrator as "the current
/// account" and carries no account id, so the sentence is the same whatever account
/// the emulator serves.
pub(crate) const NOT_AN_ASSOCIATED_MEMBER: &str = "The request is rejected because the given account ID is not an associated member of the current account.";

/// The reason `DeleteMembers` reports an account whose member record it did not
/// drop with: GuardDuty's fixed text, which also carries no account id.
pub(crate) const MEMBER_NOT_DELETED: &str = "The request is rejected because the current account cannot delete the given member account ID since it is still associated to it.";

/// The reason `DisassociateMembers` reports an account that is not associated with
/// the administrator with — one with no member record, one recorded and not invited
/// yet, and one a disassociation already moved.
pub(crate) const NOT_ASSOCIATED: &str = "The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.";

/// The message a value that cannot be a value of `member` is refused with.
fn invalid_parameter(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

/// The message a LIST of account ids carrying a value that cannot be an account id
/// at all is refused with, naming the list `member` the request sent.
fn account_ids_format(member: &str) -> String {
    format!(
        "The request failed because parameter {member} has accountId values that are formatted incorrectly."
    )
}

/// The same format rejection where the account id sits in a STRUCTURE member of its
/// own — `AccountDetail.AccountId`, which is what `CreateMembers` carries.
pub(crate) const ACCOUNT_ID_FORMAT: &str =
    "The request failed because the 'accountId' was formatted incorrectly.";

/// The wire name of the `AccountIds` list the batch members operations carry
/// (`smithy_query(operation="DisassociateMembers")`).
pub(crate) const ACCOUNT_IDS_MEMBER: &str = "accountIds";

/// The message an `AccountDetail.Email` that breaks one of the documented email
/// rules is refused with: GuardDuty's own wording for that rule, read from the
/// handler so the test and the service cannot drift apart.
fn email_format_message() -> String {
    service_com_amazonaws_guardduty_guarddutyapiservice::state::MEMBER_EMAIL_FORMAT_MESSAGE
        .to_string()
}

/// The message a well-formed id this account does not own is refused with.
fn not_owned(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// The Region the member suites call through unless a case names another: the one
/// [`call`] signs for.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// `CreateMembers` through `region`'s endpoint. The region-less [`create_members`]
/// is this with [`DEFAULT_REGION`], so a suite that spans two administrators and a
/// suite that stays in one share ONE member-call harness.
pub(crate) async fn create_members_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    accounts: &[(&str, &str)],
) -> (u16, Value) {
    let details: Vec<Value> = accounts
        .iter()
        .map(|(account_id, email)| json!({"accountId": account_id, "email": email}))
        .collect();
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member"),
        Some(json!({"accountDetails": details})),
    )
    .await
}

pub(crate) async fn create_members(
    router: &Router,
    detector_id: &str,
    accounts: &[(&str, &str)],
) -> (u16, Value) {
    create_members_in(router, DEFAULT_REGION, detector_id, accounts).await
}

/// `GetMembers` through `region`'s endpoint, with [`get_members`] over it exactly as
/// [`create_members_in`] carries [`create_members`].
pub(crate) async fn get_members_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/member/get"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

pub(crate) async fn get_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    get_members_in(router, DEFAULT_REGION, detector_id, account_ids).await
}

pub(crate) async fn disassociate_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/disassociate"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// `InviteMembers` over the wire, with no announcement members: the one call that
/// moves a recorded account into a relationship with its administrator, so the
/// suites that need an ASSOCIATED member send it from here rather than each
/// building its own.
pub(crate) async fn invite_members(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member/invite"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// The `members` entry for `account_id`, if the response carries one.
pub(crate) fn member_of(body: &Value, account_id: &str) -> Option<Value> {
    body["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["accountId"].as_str() == Some(account_id))
        .cloned()
}

/// The account ids in `unprocessedAccounts`, in the order the response lists them.
pub(crate) fn unprocessed_ids(body: &Value) -> Vec<String> {
    body["unprocessedAccounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `result` reported for `account_id` in `unprocessedAccounts`, if any.
pub(crate) fn unprocessed_result(body: &Value, account_id: &str) -> Option<String> {
    body["unprocessedAccounts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["accountId"].as_str() == Some(account_id))
        .map(|entry| entry["result"].as_str().unwrap_or_default().to_string())
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

/// [`detector_with_members`] with every member INVITED, which is what associates a
/// recorded account with its administrator: the operations that act on the
/// association process a member only from the invitation on.
pub(crate) async fn detector_with_invited_members(router: &Router) -> String {
    let detector_id = detector_with_members(router).await;
    let ids: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();
    let (status, body) = invite_members(router, &detector_id, &ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "invite-members: expected every account processed, got {body:?}"
    );
    detector_id
}

// ---------------------------------------------------------------------------
// Rule 2: create -> read -> disassociate -> read.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_member_association_loop_is_observable_through_stored_state() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let administrator = administrator();
    let all_ids: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();

    let (status, created) = get_members(&router, &detector_id, &all_ids).await;
    assert_eq!(
        status, 200,
        "get-members after create: expected 200, got {status} {created:?}"
    );
    assert!(
        unprocessed_ids(&created).is_empty(),
        "get-members after create: expected no unprocessed accounts, got {created:?}"
    );
    for (account_id, email) in MEMBERS {
        let member = member_of(&created, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {created:?}"));
        assert_eq!(
            member["email"].as_str(),
            Some(email),
            "case {account_id}: expected the recorded email, got {member:?}"
        );
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some("Created"),
            "case {account_id}: expected a recorded account to be Created, got {member:?}"
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
        assert!(
            member["updatedAt"].as_str().is_some_and(|at| at.ends_with('Z')),
            "case {account_id}: expected an ISO-8601 updatedAt, got {member:?}"
        );
        // The member account's own detector is not a resource this emulator holds,
        // so the optional member is absent rather than invented.
        assert!(
            member["detectorId"].is_null(),
            "case {account_id}: expected no member detectorId, got {member:?}"
        );
    }

    // An invitation is what associates a recorded account with this administrator:
    // only from there on is there an association to end.
    let (status, invited) = invite_members(&router, &detector_id, &all_ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {invited:?}"
    );
    assert!(
        unprocessed_ids(&invited).is_empty(),
        "invite-members: expected every member processed, got {invited:?}"
    );
    let (status, created) = get_members(&router, &detector_id, &all_ids).await;
    assert_eq!(
        status, 200,
        "get-members after invite: expected 200, got {status} {created:?}"
    );

    // Disassociate a strict SUBSET, so the call is observable both ways.
    let disassociated: Vec<&str> = MEMBERS.iter().take(2).map(|(id, _)| *id).collect();
    let (status, body) = disassociate_members(&router, &detector_id, &disassociated).await;
    assert_eq!(
        status, 200,
        "disassociate-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "disassociate-members: expected every member processed, got {body:?}"
    );

    let (status, after) = get_members(&router, &detector_id, &all_ids).await;
    assert_eq!(
        status, 200,
        "get-members after disassociate: expected 200, got {status} {after:?}"
    );
    for (account_id, email) in MEMBERS {
        let expected_status = if disassociated.contains(&account_id) {
            "Removed"
        } else {
            "Invited"
        };
        let member = member_of(&after, account_id).unwrap_or_else(|| {
            panic!("case {account_id}: expected the record to be retained, got {after:?}")
        });
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(expected_status),
            "case {account_id}: expected relationshipStatus {expected_status}, got {member:?}"
        );
        // The details are RETAINED across a disassociation: only DeleteMembers
        // removes them.
        assert_eq!(
            member["email"].as_str(),
            Some(email),
            "case {account_id}: expected the email to be retained, got {member:?}"
        );
        let before = member_of(&created, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {created:?}"));
        let moved = member["updatedAt"] != before["updatedAt"];
        assert_eq!(
            moved,
            disassociated.contains(&account_id),
            "case {account_id}: expected updatedAt to move only for a disassociated member, got {:?} then {:?}",
            before["updatedAt"],
            member["updatedAt"]
        );
    }
}

#[tokio::test]
async fn disassociating_a_member_twice_changes_nothing_more() {
    let router = new_router();
    let detector_id = detector_with_invited_members(&router).await;
    let (account_id, email) = MEMBERS[0];

    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "first disassociate: expected 200, got {status}");
    assert!(
        unprocessed_ids(&body).is_empty(),
        "first disassociate: expected an invited member processed, got {body:?}"
    );
    let (_, once) = get_members(&router, &detector_id, &[account_id]).await;
    let first = member_of(&once, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {once:?}"));

    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "second disassociate: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_result(&body, account_id).as_deref(),
        Some(NOT_ASSOCIATED),
        "second disassociate: a disassociated member is no longer associated, got {body:?}"
    );
    let (_, twice) = get_members(&router, &detector_id, &[account_id]).await;
    let second = member_of(&twice, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {twice:?}"));
    assert_eq!(
        second, first,
        "case {account_id}: expected the second disassociation to change nothing, got {second:?} after {first:?}"
    );
    assert_eq!(
        second["email"].as_str(),
        Some(email),
        "case {account_id}: expected the email to be retained, got {second:?}"
    );
}

#[tokio::test]
async fn recreating_a_disassociated_account_records_it_again() {
    let router = new_router();
    let detector_id = detector_with_invited_members(&router).await;
    let (account_id, email) = MEMBERS[1];
    let new_email = "renamed@example.net";

    let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
    assert_eq!(status, 200, "disassociate: expected 200, got {status}");
    assert!(
        unprocessed_ids(&body).is_empty(),
        "disassociate: expected an invited member processed, got {body:?}"
    );

    // "You can create an association with these member accounts again only by
    // calling the CreateMembers API" — and the call records the details it carries,
    // so a changed email is observable too.
    let (status, body) = create_members(&router, &detector_id, &[(account_id, new_email)]).await;
    assert_eq!(
        status, 200,
        "re-create: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "re-create: expected the account processed, got {body:?}"
    );

    let (_, after) = get_members(&router, &detector_id, &[account_id]).await;
    let member = member_of(&after, account_id)
        .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {after:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Created"),
        "case {account_id}: expected a re-created account to be recorded afresh, got {member:?}"
    );
    assert_eq!(
        member["email"].as_str(),
        Some(new_email),
        "case {account_id}: expected the re-created email, got {member:?} (was {email})"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: an account that is not a member is REPORTED, not refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_with_no_member_record_is_reported_not_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let reason = NOT_AN_ASSOCIATED_MEMBER;
    // The administrator's own account is not one of its members either, so it is
    // the fifth row of this table.
    let administrator = administrator();
    let mut rows: Vec<(&str, &str)> = NON_MEMBER_ACCOUNT_IDS.to_vec();
    rows.push((administrator.as_str(), "the administrator's own account"));

    for (account_id, why) in rows {
        let (status, body) = disassociate_members(&router, &detector_id, &[account_id]).await;
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
            Some(NOT_ASSOCIATED),
            "case {account_id} ({why}): expected the not-associated reason, got {body:?}"
        );

        // The refused disassociation persisted NOTHING for the account.
        let (status, read) = get_members(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 200,
            "case {account_id} ({why}): get-members expected 200, got {status} {read:?}"
        );
        assert!(
            member_of(&read, account_id).is_none(),
            "case {account_id} ({why}): expected no member record, got {read:?}"
        );
        assert_eq!(
            unprocessed_result(&read, account_id).as_deref(),
            Some(reason),
            "case {account_id} ({why}): expected get-members to report it unprocessed, got {read:?}"
        );
    }
}

#[tokio::test]
async fn only_the_accounts_that_are_not_members_are_reported() {
    let router = new_router();
    let detector_id = detector_with_invited_members(&router).await;
    let (member_id, _) = MEMBERS[2];
    let (absent_one, _) = NON_MEMBER_ACCOUNT_IDS[0];
    let (absent_two, _) = NON_MEMBER_ACCOUNT_IDS[1];

    // A mixed request: the members are processed, the rest reported, in request
    // order, and a repeated id is reported once because `AccountIds` names a SET.
    let requested = [absent_two, member_id, absent_one, absent_two];
    let (status, body) = disassociate_members(&router, &detector_id, &requested).await;
    assert_eq!(
        status, 200,
        "mixed disassociate: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        vec![absent_two.to_string(), absent_one.to_string()],
        "mixed disassociate: expected each absent account once, in request order, got {body:?}"
    );

    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    let member = member_of(&read, member_id)
        .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Removed"),
        "case {member_id}: expected the member in a mixed request to be disassociated, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: requests the service does not understand are refused 400.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_that_cannot_be_an_account_id_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    // GuardDuty words the format rejection in terms of the parameter that carried
    // the malformed value: the `accountIds` LIST for the batch operations, and the
    // `accountId` member itself where the id sits inside an `AccountDetail`.
    let batch_expected = account_ids_format(ACCOUNT_IDS_MEMBER);

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        // Every operation that takes an account id classifies it the same way.
        let responses = [
            (
                "DisassociateMembers",
                disassociate_members(&router, &detector_id, &[account_id]).await,
                batch_expected.as_str(),
            ),
            (
                "GetMembers",
                get_members(&router, &detector_id, &[account_id]).await,
                batch_expected.as_str(),
            ),
            (
                "CreateMembers",
                create_members(&router, &detector_id, &[(account_id, "valid@example.com")]).await,
                ACCOUNT_ID_FORMAT,
            ),
        ];
        for (operation, (status, body), expected) in responses {
            assert_eq!(
                status, 400,
                "case {operation} {account_id} ({why}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                CLASS,
                "case {operation} {account_id} ({why}): expected the {CLASS} class, got {body:?}"
            );
            assert_eq!(
                error_shape(&body),
                ENVELOPE,
                "case {operation} {account_id} ({why}): expected the {ENVELOPE} envelope, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected),
                "case {operation} {account_id} ({why}): expected the format-rejection message, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_members_operation_on_a_detector_this_account_does_not_own_is_refused() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let (member_id, _) = MEMBERS[0];

    // A detector another Region's endpoint serves is not owned here: a detector is
    // a regional resource.
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
        .unwrap_or_else(|| panic!("create-detector in eu-west-1: expected a detectorId, got {elsewhere:?}"))
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
        let responses = [
            (
                "DisassociateMembers",
                disassociate_members(&router, candidate, &[member_id]).await,
            ),
            (
                "GetMembers",
                get_members(&router, candidate, &[member_id]).await,
            ),
            (
                "CreateMembers",
                create_members(&router, candidate, &[(member_id, "member-one@example.com")]).await,
            ),
        ];
        for (operation, (status, body)) in responses {
            assert_eq!(
                status, 400,
                "case {operation} {candidate} ({why}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                CLASS,
                "case {operation} {candidate} ({why}): expected the {CLASS} class, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {operation} {candidate} ({why}): expected the classified message, got {body:?}"
            );
        }
    }

    // Nothing above touched the members the owned detector administers.
    let (_, read) = get_members(&router, &detector_id, &[member_id]).await;
    let member = member_of(&read, member_id)
        .unwrap_or_else(|| panic!("case {member_id}: expected a member, got {read:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some("Created"),
        "case {member_id}: expected the refused calls to change nothing, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3, continued: the email rules `AccountDetail.Email` documents.
// ---------------------------------------------------------------------------

/// Email addresses and whether `AccountDetail.Email` accepts them.
///
/// Every row is one of the rules the member documents
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`): 6 to 64
/// characters, 7-bit ASCII only, exactly one `@`, no whitespace or
/// `" ' ( ) < > [ ] : , \ | % &` and no leading dot in the local name, and a domain
/// name of letters, digits, hyphen and dot that neither begins nor ends with a dot
/// or hyphen and contains at least one dot.
const EMAIL_RULES: [(&str, bool, &str); 14] = [
    ("a@b.com", true, "7 characters: just past the minimum"),
    ("member@example.com", true, "an ordinary address"),
    ("m+tag@example.io", true, "a plus in the local name"),
    (
        "first.last@sub.example-corp.com",
        true,
        "dots in the local name, a hyphen and two dots in the domain",
    ),
    ("a@b.c", false, "5 characters: one below the minimum"),
    ("memberexample.com", false, "no @ at all"),
    ("a@b@example.com", false, "two @ symbols"),
    (".member@example.com", false, "the local name begins with a dot"),
    ("mem ber@example.com", false, "whitespace in the local name"),
    (
        "mem(ber@example.com",
        false,
        "a forbidden character in the local name",
    ),
    ("member@examplecom", false, "the domain has no dot"),
    ("member@-example.com", false, "the domain begins with a hyphen"),
    ("member@example.com.", false, "the domain ends with a dot"),
    (
        "member@exa_mple.com",
        false,
        "an underscore, which the domain cannot carry",
    ),
];

#[tokio::test]
async fn create_members_applies_the_documented_email_rules() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let expected_message = email_format_message();

    for (index, (email, valid, why)) in EMAIL_RULES.iter().enumerate() {
        // A fresh account id per row, so one row cannot mask another.
        let account_id = format!("9000000000{index:02}");
        let (status, body) = create_members(&router, &detector_id, &[(&account_id, email)]).await;
        if *valid {
            assert_eq!(
                status, 200,
                "case {email:?} ({why}): expected 200, got {status} {body:?}"
            );
            let (_, read) = get_members(&router, &detector_id, &[&account_id]).await;
            let member = member_of(&read, &account_id).unwrap_or_else(|| {
                panic!("case {email:?} ({why}): expected a member, got {read:?}")
            });
            assert_eq!(
                member["email"].as_str(),
                Some(*email),
                "case {email:?} ({why}): expected the recorded email, got {member:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {email:?} ({why}): expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                CLASS,
                "case {email:?} ({why}): expected the {CLASS} class, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {email:?} ({why}): expected the parameter message, got {body:?}"
            );
            let (_, read) = get_members(&router, &detector_id, &[&account_id]).await;
            assert!(
                member_of(&read, &account_id).is_none(),
                "case {email:?} ({why}): a refused create must persist nothing, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_email_is_refused_past_its_published_maximum_length() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let domain = "@example.com";

    // "The email address must be a minimum of 6 and a maximum of 64 characters
    // long": the bound itself is accepted, one character past it is refused. The
    // model carries `len 1..64` for `Email`, so the upper bound is also the
    // modelled constraint and the refusal may come from the boundary validator.
    for (length, accepted) in [(64_usize, true), (65_usize, false)] {
        let email = format!(
            "{}{domain}",
            "a".repeat(length - domain.len())
        );
        assert_eq!(email.len(), length, "case {length}: built the wrong length");
        let account_id = format!("8000000000{length}");
        let (status, body) = create_members(&router, &detector_id, &[(&account_id, &email)]).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {length} characters: expected 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {length} characters: expected 400, got {status} {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_member_store_is_bounded_by_the_member_quota() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    // GuardDuty's quota is 5,000 member accounts per administrator account per
    // Region, and `AccountDetails` carries at most 50 per call
    // (`smithy_query(operation="CreateMembers")`), so the quota is reached in
    // `QUOTA / BATCH` calls and the next account is past it. The bound comes from
    // the handler's own constant, so the test cannot drift from it.
    const QUOTA: usize =
        service_com_amazonaws_guardduty_guarddutyapiservice::state::MAX_MEMBERS_PER_DETECTOR;
    const BATCH: usize = 50;
    let quota_reason = format!(
        "The request is rejected because the current account cannot have more than {QUOTA} member accounts."
    );

    for batch in 0..QUOTA / BATCH {
        let ids: Vec<String> = (0..BATCH)
            .map(|index| format!("{:012}", batch * BATCH + index + 1))
            .collect();
        let accounts: Vec<(&str, &str)> = ids
            .iter()
            .map(|id| (id.as_str(), "filler@example.com"))
            .collect();
        let (status, body) = create_members(&router, &detector_id, &accounts).await;
        assert_eq!(
            status, 200,
            "case batch {batch}: expected 200, got {status} {body:?}"
        );
        assert!(
            unprocessed_ids(&body).is_empty(),
            "case batch {batch}: expected every account under the quota recorded, got {body:?}"
        );
    }

    // One past the bound: reported, not recorded.
    let past_the_bound = format!("{:012}", QUOTA + 1);
    let (status, body) =
        create_members(&router, &detector_id, &[(&past_the_bound, "past@example.com")]).await;
    assert_eq!(
        status, 200,
        "case past the quota: expected 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_result(&body, &past_the_bound).as_deref(),
        Some(quota_reason.as_str()),
        "case past the quota: expected the quota reason, got {body:?}"
    );
    let (_, read) = get_members(&router, &detector_id, &[&past_the_bound]).await;
    assert!(
        member_of(&read, &past_the_bound).is_none(),
        "case past the quota: expected no member record, got {read:?}"
    );

    // An account already recorded is still updatable at the bound: the quota caps
    // the store's SIZE, not the operation.
    let at_the_bound = format!("{QUOTA:012}");
    let (status, body) =
        create_members(&router, &detector_id, &[(&at_the_bound, "renamed@example.com")]).await;
    assert_eq!(
        status, 200,
        "case at the quota: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "case at the quota: expected the recorded account processed, got {body:?}"
    );
    let (_, read) = get_members(&router, &detector_id, &[&at_the_bound]).await;
    let member = member_of(&read, &at_the_bound)
        .unwrap_or_else(|| panic!("case at the quota: expected a member, got {read:?}"));
    assert_eq!(
        member["email"].as_str(),
        Some("renamed@example.com"),
        "case at the quota: expected the updated email, got {member:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: members hang off the detector that administers them.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn members_are_scoped_to_the_detector_that_administers_them() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let (member_id, _) = MEMBERS[0];
    let reason = NOT_AN_ASSOCIATED_MEMBER;

    // Another Region's detector administers its own members, so this account is not
    // a member there.
    let (_, elsewhere) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    let other_region_id = elsewhere["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector in eu-west-1: expected a detectorId, got {elsewhere:?}"))
        .to_string();
    let (status, read) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{other_region_id}/member/get"),
        Some(json!({"accountIds": [member_id]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "get-members in eu-west-1: expected 200, got {status} {read:?}"
    );
    assert_eq!(
        unprocessed_result(&read, member_id).as_deref(),
        Some(reason),
        "case {member_id}: expected another Region's detector to hold no member, got {read:?}"
    );

    // The members are dropped with the detector that administers them, the same
    // cascade `DeleteDetector` performs on findings and filters.
    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "delete-detector: expected 200, got {status} {body:?}"
    );
    let replacement = enabled_detector(&router).await;
    let (status, read) = get_members(&router, &replacement, &[member_id]).await;
    assert_eq!(
        status, 200,
        "get-members after replacing the detector: expected 200, got {status} {read:?}"
    );
    assert!(
        member_of(&read, member_id).is_none(),
        "case {member_id}: expected the members to be dropped with the detector, got {read:?}"
    );
    assert_eq!(
        unprocessed_result(&read, member_id).as_deref(),
        Some(reason),
        "case {member_id}: expected the replacement detector to hold no member, got {read:?}"
    );
}
